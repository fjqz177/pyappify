// src/command.rs
use crate::utils::error::Error;
use crate::{emit_error, emit_info, ensure_some, err};
use once_cell::sync::Lazy;
use std::ffi::OsStr;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tracing::{debug, error, info};
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

/// PID of the currently running environment operation (uv python install /
/// uv pip install). Only one runs at a time (app dir lock serializes them).
static CURRENT_OPERATION_PID: Lazy<Mutex<Option<u32>>> = Lazy::new(|| Mutex::new(None));
static OPERATION_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Kill the running environment operation (whole process tree) and mark the
/// current run as cancelled. Returns false when nothing is running.
pub fn cancel_current_operation() -> bool {
    let pid = match CURRENT_OPERATION_PID.lock().unwrap().take() {
        Some(pid) => pid,
        None => return false,
    };
    OPERATION_CANCELLED.store(true, Ordering::SeqCst);
    info!("Cancelling operation with pid {}", pid);
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    true
}

fn is_operation_cancelled() -> bool {
    OPERATION_CANCELLED.swap(false, Ordering::SeqCst)
}

pub async fn run_command_and_stream_output(
    mut command: Command,
    app_name: &str,
    command_description: &str,
) -> Result<ExitStatus, Error> {
    emit_info!(
        app_name,
        "executing command: '{}'. Full details: {:?}",
        command_description,
        command
    );

    command.creation_flags(0x08000000);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        let msg = format!("Failed to spawn command ({}): {}", command_description, e);
        error!(error = %e, command = %command_description, %msg);
        err!(msg)
    })?;

    let child_pid_obj = child
        .id()
        .ok_or_else(|| err!("Failed to get spawned command pid"))?;
    let child_pid = child_pid_obj.to_string();
    {
        // Only one environment operation at a time; replace any stale entry.
        *CURRENT_OPERATION_PID.lock().unwrap() = Some(child_pid_obj);
        OPERATION_CANCELLED.store(false, Ordering::SeqCst);
    }
    info!(pid = %child_pid, cmd_desc = %command_description, "Command spawned");

    let stdout = ensure_some!(
        child.stdout.take(),
        "Could not capture stdout from command ({})",
        command_description
    )
    .map_err(|e| {
        emit_error!(app_name, "{}", e.to_string());
        err!(e.to_string())
    })?;

    let stderr = ensure_some!(
        child.stderr.take(),
        "Could not capture stderr from command ({})",
        command_description
    )
    .map_err(|e| {
        emit_error!(app_name, "{}", e.to_string());
        err!(e.to_string())
    })?;

    let mut stdout_buf_reader = tokio::io::BufReader::new(stdout);
    let mut stderr_buf_reader = tokio::io::BufReader::new(stderr);

    let app_name_for_stdout = app_name.to_string();
    let stdout_task = tokio::spawn(async move {
        let mut buffer = String::new();
        loop {
            match stdout_buf_reader.read_line(&mut buffer).await {
                Ok(0) => break,
                Ok(_) => {
                    emit_info!(app_name_for_stdout, "{}", buffer.as_str());
                    buffer.clear();
                }
                Err(e) => {
                    emit_error!(app_name_for_stdout, "Error reading stdout line: {}", e);
                    break;
                }
            }
        }
    });

    let app_name_for_stderr = app_name.to_string();
    let stderr_task = tokio::spawn(async move {
        let mut buffer = String::new();
        loop {
            match stderr_buf_reader.read_line(&mut buffer).await {
                Ok(0) => break,
                Ok(_) => {
                    let err_string = buffer.to_string();
                    buffer.clear();
                    if !err_string.trim().is_empty()
                        && !err_string.contains("A new release of pip is available")
                        && !err_string.contains("[notice] To update, run")
                    {
                        emit_error!(app_name_for_stderr, "{}", err_string);
                    } else {
                        debug!("not emitting black listed stderr {}", err_string);
                    }
                }
                Err(e) => {
                    emit_error!(app_name_for_stderr, "Error reading stderr line: {}", e);
                    break;
                }
            }
        }
    });

    let status = child.wait().await?;

    {
        let mut guard = CURRENT_OPERATION_PID.lock().unwrap();
        if guard.as_ref() == Some(&child_pid_obj) {
            *guard = None;
        }
    }

    if let Err(e) = tokio::try_join!(stdout_task, stderr_task) {
        error!(error = %e, cmd_desc = %command_description, "Log reading task encountered an error. This does not necessarily mean the command itself failed.");
    }

    if is_operation_cancelled() {
        emit_info!(
            app_name,
            "Operation cancelled by the user. The environment will be completed (idempotently) on the next start."
        );
        return Err(err!("Operation cancelled by the user."));
    }

    if !status.success() {
        return Err(err!("Command failed ({}): {}", command_description, status));
    }

    Ok(status)
}

pub fn command_to_string(command: &std::process::Command) -> String {
    let program_path = command.get_program();
    let arguments: Vec<&str> = command.get_args().filter_map(|arg| arg.to_str()).collect();
    let mut command_string = String::new();
    if let Some(path) = program_path.to_str() {
        command_string.push_str(path);
    } else {
        command_string.push_str("<non-UTF8 program path>");
    }
    for arg in arguments {
        command_string.push(' ');
        if arg.contains(' ') || arg.contains('"') {
            command_string.push('"');
            command_string.push_str(arg.replace('"', "\"\"").as_str());
            command_string.push('"');
        } else {
            command_string.push_str(arg);
        }
    }
    command_string
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().is_positive() }
}

pub fn new_cmd<S: AsRef<OsStr>>(executable: S) -> Command {
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    command
}

#[cfg(not(windows))]
pub async fn is_admin() -> bool {
    if let Ok(output) = Command::new("id").arg("-u").output().await {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim() == "0";
        }
    }
    false
}
