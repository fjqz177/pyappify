use lazy_static::lazy_static;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const BASE_DIR: &str = "data";
const APPS_DIR: &str = "apps";
pub const PYTHON_ROOT_DIR: &str = "python";
const WORKING_DIR_NAME: &str = "working";

lazy_static! {
    static ref CWD: PathBuf = env::current_dir().expect("Failed to get current directory");
}
pub fn get_log_dir() -> PathBuf {
    PathBuf::from(BASE_DIR).join("logs")
}
fn get_base_dir() -> PathBuf {
    CWD.join(BASE_DIR)
}
pub fn get_python_dir(app_name: &str) -> PathBuf {
    get_app_base_path(app_name).join(PYTHON_ROOT_DIR)
}

pub fn get_cwd() -> PathBuf {
    CWD.clone()
}

/// Scan the uv-managed layout (`cpython-<ver>-*/python.exe`) under a Python root.
fn find_managed_python_exe(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(String, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("cpython-") {
                continue;
            }
            let exe = entry.path().join("python.exe");
            if exe.is_file() && best.as_ref().map_or(true, |(n, _)| name > *n) {
                best = Some((name, exe));
            }
        }
    }
    best.map(|(_, exe)| exe)
}

/// Resolve the app's Python interpreter:
/// 1. uv-managed layout (`cpython-*/python.exe`, newest patch wins);
/// 2. legacy raw layout (`python/python.exe`) — kept only until migration.
pub fn get_python_exe(app_name: &str, use_pythonw: bool) -> PathBuf {
    let python_dir = get_python_dir(app_name);
    let managed = find_managed_python_exe(&python_dir);
    match managed {
        Some(exe) if use_pythonw => exe.with_file_name("pythonw.exe"),
        Some(exe) => exe,
        None => python_dir.join(if use_pythonw { "pythonw.exe" } else { "python.exe" }),
    }
}

pub fn get_apps_dir() -> PathBuf {
    get_base_dir().join(APPS_DIR)
}
pub fn get_app_repo_path(app_name: &str) -> PathBuf {
    get_app_base_path(app_name).join("repo")
}

pub fn get_app_base_path(app_name: &str) -> PathBuf {
    get_apps_dir().join(app_name)
}

pub fn get_app_working_dir_path(app_name: &str) -> PathBuf {
    get_app_base_path(app_name).join(WORKING_DIR_NAME)
}
/// uv package cache ("App Install Directory" option in settings). The old
/// `cache/pip` directory is left untouched for historical installations.
pub fn get_pip_cache_dir() -> PathBuf {
    CWD.join("cache").join("uv")
}

pub fn get_config_dir() -> PathBuf {
    get_base_dir().join("config")
}

pub fn get_start_dir(app_handle: AppHandle) -> PathBuf {
    app_handle
        .path()
        .config_dir()
        .map(|path| path.join("Microsoft\\Windows\\Start Menu\\Programs"))
        .unwrap()
}

fn strip_extended_path_prefix(path_str: &str) -> String {
    if let Some(stripped) = path_str.strip_prefix("\\\\?\\") {
        stripped.to_string()
    } else {
        path_str.to_string()
    }
}

pub fn path_to_abs(path: &Path) -> String {
    if let Ok(absolute_path_buf) = path.canonicalize() {
        if let Some(s_ref) = absolute_path_buf.to_str() {
            return strip_extended_path_prefix(s_ref);
        }
    } else {
        if let Ok(current_dir) = env::current_dir() {
            let absolute_path_buf = current_dir.join(path);
            if let Some(s_ref) = absolute_path_buf.to_str() {
                return strip_extended_path_prefix(s_ref);
            }
        }
    }

    let path_cow = path.to_string_lossy();
    strip_extended_path_prefix(&path_cow)
}
