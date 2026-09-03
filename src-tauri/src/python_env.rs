// src/python_env.rs
//! uv-first Python environment management (pure-uv redesign).
//!
//! The launcher no longer downloads Python via a hardcoded KNOWN_PATCHES list
//! and no longer runs `pip`: `uv` owns the whole environment lifecycle.
//! - Python discovery/install: `uv python install` with `UV_PYTHON_INSTALL_DIR`
//!   pointed at `data/apps/<app>/python/` (uv's managed layout is
//!   `cpython-<ver>-<target>/python.exe` under that root).
//! - Dependency sync: `uv pip install` (hash-pinned requirements work as-is).
//!
//! `uv.exe` ships as a sidecar next to the launcher executable (bundled by
//! pyappify-action). Missing uv is a hard, user-readable error — the pure-uv
//! branch keeps no pip/python.org fallback chain.

use crate::config_manager::GLOBAL_CONFIG_STATE;
use crate::utils::command::new_cmd;
use crate::utils::error::Error;
use crate::utils::path::{get_python_dir, get_python_exe};
use crate::utils::process::RemovePythonEnvsExt;
use crate::{emit_info, err, utils::command};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tracing::warn;

pub const PIP_UPDATE_NEEDED_MARKER: &str = ".pip_update_needed.tmp";

// Placeholder a profile's pip_args may use for the GPU torch CUDA index URL;
// install_requirements() expands it to the user-selected torch mirror. Kept as
// a token (not a hardcoded URL) so CPU profiles that don't need torch stay
// untouched and the user gets to choose the mirror.
pub const PIP_TORCH_INDEX_URL_PLACEHOLDER: &str = "{PIP_TORCH_INDEX_URL}";

// The uv-managed Python layout prefix under the install dir.
const UV_MANAGED_PREFIX: &str = "cpython-";

/// Expand the {PIP_TORCH_INDEX_URL} placeholder in a pip_args string into the
/// user-selected torch (CUDA) index URL. Supports both space-separated
/// (`--extra-index-url {PH}`) and equals-attached (`--extra-index-url={PH}`)
/// forms; split_whitespace() otherwise keeps the equals form as one token and
/// would pass the placeholder to uv untouched (failing the install).
pub fn expand_torch_placeholder(pip_args: &str, torch_url: &str) -> Vec<String> {
    pip_args
        .split_whitespace()
        .map(|arg| {
            if arg == PIP_TORCH_INDEX_URL_PLACEHOLDER {
                torch_url.to_string()
            } else if let Some(prefix) = arg.strip_suffix(PIP_TORCH_INDEX_URL_PLACEHOLDER) {
                format!("{prefix}{torch_url}")
            } else {
                arg.to_string()
            }
        })
        .collect()
}

/// Locate the uv executable:
/// 1. `UV_EXECUTABLE` env (explicit override, e.g. during development);
/// 2. `uv.exe` next to the launcher executable (the shipped sidecar);
/// 3. `uv` on PATH.
fn locate_uv() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("UV_EXECUTABLE").map(PathBuf::from) {
        if explicit.is_file() {
            return Ok(explicit);
        }
        warn!(
            "UV_EXECUTABLE is set to '{}' but it is not a file; falling back.",
            explicit.display()
        );
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join("uv.exe");
            if sidecar.is_file() {
                return Ok(sidecar);
            }
        }
    }
    if let Ok(output) = StdCommand::new("uv").arg("--version").output() {
        if output.status.success() {
            return Ok(PathBuf::from("uv"));
        }
    }
    Err(anyhow!(
        "uv.exe was not found. It must ship next to the launcher executable (or be provided via UV_EXECUTABLE / PATH)."
    ))
}

/// Environment applied to every uv invocation: per-app Python install dir,
/// uv cache dir from config, and the optional Python source mirror.
fn uv_env(app_name: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    env.push((
        "UV_PYTHON_INSTALL_DIR".to_string(),
        get_python_dir(app_name).to_string_lossy().into_owned(),
    ));

    if let Some(config_state) = GLOBAL_CONFIG_STATE.get() {
        let (uv_cache_dir, python_source) = {
            let config = config_state.lock().unwrap();
            (
                config.get_effective_pip_cache_dir(),
                config.get_effective_python_source(),
            )
        };
        if let Some(cache_dir) = uv_cache_dir {
            env.push((
                "UV_CACHE_DIR".to_string(),
                cache_dir.to_string_lossy().into_owned(),
            ));
        }
        if let Some(source) = python_source {
            env.push(("UV_PYTHON_INSTALL_MIRROR".to_string(), source));
        }
    }
    env
}

/// Run uv with the given args, streaming output to the app console.
async fn run_uv(app_name: &str, args: &[&str], description: &str) -> Result<(), Error> {
    let uv = locate_uv()?;
    let mut cmd = new_cmd(uv);
    cmd.args(args).envs(uv_env(app_name));
    cmd.clear_python_envs();
    command::run_command_and_stream_output(cmd, app_name, description).await?;
    Ok(())
}

/// Run uv and capture stdout (used for version finding / diagnostics).
#[allow(dead_code)]
async fn run_uv_capture(app_name: &str, args: &[&str]) -> Result<String, Error> {
    let uv = locate_uv()?;
    let mut cmd = new_cmd(uv);
    cmd.args(args).envs(uv_env(app_name));
    cmd.clear_python_envs();
    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to run uv {} for '{}'", args.join(" "), app_name))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err!(
            "uv {} failed for '{}': {}",
            args.join(" "),
            app_name,
            stderr.trim()
        ));
    }
    emit_info!(
        app_name,
        "{}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Normalize "X.Y" or "X.Y.Z" to the "X.Y" major.minor key.
fn parse_major_minor(version_str: &str) -> Result<String> {
    let parts: Vec<&str> = version_str.split('.').collect();
    match parts.len() {
        2 | 3 => Ok(format!("{}.{}", parts[0], parts[1])),
        _ => Err(anyhow!(
            "Invalid version format: {}. Expected X.Y or X.Y.Z",
            version_str
        )),
    }
}

/// Scan the uv-managed install root for `cpython-<ver>-*/python.exe` and return
/// the newest patch whose major.minor matches `spec`. Touches no network.
fn resolve_uv_python_exe(app_name: &str, spec: &str) -> Result<PathBuf> {
    let major_minor = parse_major_minor(spec)?;
    let root = get_python_dir(app_name);
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if root.is_dir() {
        for entry in std::fs::read_dir(&root)?.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(UV_MANAGED_PREFIX) {
                continue;
            }
            let ver_part = name
                .strip_prefix(UV_MANAGED_PREFIX)
                .unwrap_or("")
                .split('-')
                .next()
                .unwrap_or("")
                .to_string();
            let Ok(candidate_mm) = parse_major_minor(&ver_part) else {
                continue;
            };
            if candidate_mm != major_minor {
                continue;
            }
            let exe = entry.path().join("python.exe");
            if exe.is_file() {
                candidates.push((ver_part, exe));
            }
        }
    }
    let (_, best) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .ok_or_else(|| {
            anyhow!(
                "No uv-managed Python matching '{}' found under '{}'. Run setup again.",
                major_minor,
                root.display()
            )
        })?;
    Ok(best)
}

/// True when this app's Python lives in the uv-managed layout (either freshly
/// installed by uv, or already migrated).
pub fn is_uv_managed_python(app_name: &str) -> bool {
    let root = get_python_dir(app_name);
    if !root.is_dir() {
        return false;
    }
    std::fs::read_dir(&root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(UV_MANAGED_PREFIX))
}

#[cfg(target_os = "windows")]
fn get_python_version_from_exe(python_exe_path: &Path) -> Result<String> {
    use std::os::windows::process::CommandExt;
    if !python_exe_path.is_file() {
        return Err(anyhow!(
            "Python executable not found at {}",
            python_exe_path.display()
        ));
    }
    let version_cmd_output = StdCommand::new(python_exe_path)
        .creation_flags(0x08000000)
        .env("PYTHONNOUSERSITE", "1")
        .arg("--version")
        .output()
        .with_context(|| {
            format!("Failed to execute {} --version", python_exe_path.display())
        })?;

    let trimmed_stdout = String::from_utf8_lossy(&version_cmd_output.stdout)
        .trim()
        .to_string();

    if !version_cmd_output.status.success() {
        return Err(anyhow!(
            "Python --version command failed for {}: Stdout: '{}'",
            python_exe_path.display(),
            trimmed_stdout
        ));
    }
    if !trimmed_stdout.starts_with("Python ") {
        return Err(anyhow!(
            "Python --version for {} produced no usable 'Python x.y.z' output: '{}'",
            python_exe_path.display(),
            trimmed_stdout
        ));
    }
    trimmed_stdout
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow!(
                "Could not parse version from Python --version output: '{}' for {}",
                trimmed_stdout,
                python_exe_path.display()
            )
        })
}

/// Ensure the app's uv-managed Python matches `version_spec`, installing through
/// uv when needed (first install, version pin change). Returns the interpreter path.
#[cfg(target_os = "windows")]
pub async fn ensure_python_via_uv(app_name: &str, version_spec: &str) -> Result<PathBuf> {
    emit_info!(
        app_name,
        "Ensuring Python '{}' via uv (install dir: {})",
        version_spec,
        get_python_dir(app_name).display()
    );

    // Fast path: a matching uv-managed Python already exists (no network).
    if let Ok(exe) = resolve_uv_python_exe(app_name, version_spec) {
        emit_info!(app_name, "Using uv-managed Python {}", exe.display());
        return Ok(exe);
    }

    run_uv(
        app_name,
        &["python", "install", version_spec],
        &format!("uv python install {}", version_spec),
    )
    .await?;

    let exe = resolve_uv_python_exe(app_name, version_spec)?;
    let actual_version = get_python_version_from_exe(&exe)?;
    emit_info!(
        app_name,
        "uv-managed Python ready ({} at {})",
        actual_version,
        exe.display()
    );
    Ok(exe)
}

/// Translate a profile's pip_args into the uv-supported surface. The
/// documented contract is: index overrides, dependency flags and the torch
/// placeholder pass through; anything else is left for uv to reject loudly
/// (pure-uv policy: explicit failure beats silent dropping).
fn translate_pip_args(pip_args: &str, torch_index_url: &str) -> Vec<String> {
    expand_torch_placeholder(pip_args, torch_index_url)
        .into_iter()
        .filter(|arg| {
            let flag = arg.split('=').next().unwrap_or("");
            // Subshell-only noise flag; uv has no equivalent.
            flag != "--no-warn-script-location"
        })
        .collect()
}

#[cfg(target_os = "windows")]
pub async fn install_requirements(
    app_name: &str,
    requirements: &str,
    project_dir: &Path,
    pip_args: &str,
) -> Result<(), Error> {
    let python_exe = get_python_exe(app_name, false);
    if !python_exe.is_file() {
        return Err(err!(
            "Managed Python executable not found at {}. Run setup again.",
            python_exe.display()
        ));
    }
    if !project_dir.is_dir() {
        return Err(err!(
            "Project directory for uv execution not found or not a directory: {}",
            project_dir.display()
        ));
    }
    let config_state = GLOBAL_CONFIG_STATE.get().ok_or_else(|| {
        anyhow!("GLOBAL_CONFIG_STATE not initialized. Call init_config_manager first.")
    })?;
    let (pip_index_url, torch_index_url) = {
        let config = config_state.lock().unwrap();
        (
            config.get_effective_pip_index_url(),
            config.get_effective_torch_index_url(),
        )
    };

    let uv_install_desc = if requirements.ends_with(".txt") {
        let requirements_path = project_dir.join(requirements);
        format!("uv pip install -r {}", requirements_path.display())
    } else {
        format!("uv pip install {}", requirements)
    };

    let uv = locate_uv()?;
    let mut uv_install_cmd = new_cmd(uv);
    uv_install_cmd.envs(uv_env(app_name));
    uv_install_cmd.clear_python_envs();

    let mut use_config_index_url = true;
    if !pip_args.is_empty() {
        if pip_args
            .split_whitespace()
            .any(|arg| arg == "--index-url" || arg == "-i")
        {
            use_config_index_url = false;
        }
        // Expand the torch-source placeholder ({PIP_TORCH_INDEX_URL}) into the
        // user-selected CUDA index URL (GPU variant). This keeps the mirror choice
        // in the user's hands while leaving the main --index-url untouched.
        uv_install_cmd.args(translate_pip_args(pip_args, &torch_index_url));
    }
    if requirements.ends_with(".txt") {
        let requirements_path = project_dir.join(requirements);
        if !requirements_path.is_file() {
            return Err(err!(
                "Requirements file not found at {}",
                requirements_path.display()
            ));
        }
        uv_install_cmd.arg("-r").arg(&requirements_path);
    } else {
        uv_install_cmd.arg(requirements);
    }
    uv_install_cmd.arg("--python").arg(&python_exe);
    if use_config_index_url {
        emit_info!(app_name, "set --index-url {:?}", pip_index_url);
        if let Some(index_url) = pip_index_url {
            uv_install_cmd.arg("--index-url").arg(index_url);
        }
    }

    let marker_path = project_dir.join(PIP_UPDATE_NEEDED_MARKER);
    std::fs::File::create(&marker_path).ok();

    command::run_command_and_stream_output(uv_install_cmd, app_name, &uv_install_desc).await?;

    if marker_path.exists() {
        let _ = std::fs::remove_file(&marker_path);
    }

    emit_info!(
        app_name,
        "Successfully installed requirements from '{}' via uv.",
        requirements
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub async fn install_requirements(
    _app_name: &str,
    _requirements: &str,
    _project_dir: &Path,
    _pip_args: &str,
) -> Result<(), Error> {
    Err(err!("install_requirements is only implemented for Windows."))
}

#[cfg(target_os = "windows")]
pub async fn setup_python_env(app_name: String, python_version_spec: &str) -> Result<PathBuf> {
    ensure_python_via_uv(&app_name, python_version_spec).await
}

#[cfg(not(target_os = "windows"))]
pub fn setup_python_env(_app_name: String, _python_version_spec: &str) -> Result<PathBuf> {
    Err(anyhow!("setup_python_env is only implemented for Windows."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_space_separated_placeholder() {
        let args = expand_torch_placeholder(
            "--extra-index-url {PIP_TORCH_INDEX_URL}",
            "https://mirror.nju.edu.cn/pytorch/whl/cu126",
        );
        assert_eq!(
            args,
            vec![
                "--extra-index-url",
                "https://mirror.nju.edu.cn/pytorch/whl/cu126"
            ]
        );
    }

    #[test]
    fn expands_equals_attached_placeholder() {
        let args = expand_torch_placeholder(
            "--extra-index-url={PIP_TORCH_INDEX_URL}",
            "https://download.pytorch.org/whl/cu126",
        );
        assert_eq!(
            args,
            vec!["--extra-index-url=https://download.pytorch.org/whl/cu126"]
        );
    }

    #[test]
    fn leaves_args_without_placeholder_untouched() {
        let args = expand_torch_placeholder(
            "--no-deps --index-url https://pypi.tuna.tsinghua.edu.cn/simple",
            "https://download.pytorch.org/whl/cu126",
        );
        assert_eq!(
            args,
            vec![
                "--no-deps",
                "--index-url",
                "https://pypi.tuna.tsinghua.edu.cn/simple"
            ]
        );
    }

    #[test]
    fn translates_pip_args_for_uv() {
        let args = translate_pip_args(
            "--no-deps --extra-index-url {PIP_TORCH_INDEX_URL} --no-warn-script-location",
            "https://mirror.nju.edu.cn/pytorch/whl/cu126",
        );
        assert_eq!(
            args,
            vec![
                "--no-deps",
                "--extra-index-url",
                "https://mirror.nju.edu.cn/pytorch/whl/cu126"
            ]
        );
    }

    #[test]
    fn parses_major_minor_specs() {
        assert_eq!(parse_major_minor("3.12").unwrap(), "3.12");
        assert_eq!(parse_major_minor("3.12.10").unwrap(), "3.12");
        assert!(parse_major_minor("12").is_err());
        assert!(parse_major_minor("").is_err());
    }
}
