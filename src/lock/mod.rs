//! 查看占用、解除占用和删除。Windows 与 macOS 的实现分开放在各自的文件里。

use std::path::Path;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{block_reason, delete_path, inspect, is_critical, release};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{block_reason, delete_path, inspect, is_critical, release};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("ffrm 目前只支持 Windows 和 macOS");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locker {
    pub pid: u32,
    pub app_name: String,
    pub image: Option<String>,
    pub app_type: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub path: std::path::PathBuf,
    pub lockers: Vec<Locker>,
    pub sharing_violation: bool,
    pub list_denied: bool,
}

pub fn display_name(locker: &Locker) -> String {
    if !locker.app_name.is_empty() {
        return locker.app_name.clone();
    }
    locker
        .image
        .as_deref()
        .and_then(|image| {
            Path::new(image)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "未知程序".to_string())
}

pub fn critical_block(lockers: &[Locker]) -> Option<String> {
    let critical: Vec<_> = lockers
        .iter()
        .filter(|locker| is_critical(locker))
        .collect();
    if critical.is_empty() {
        return None;
    }
    let listed = critical
        .iter()
        .map(|locker| format!("pid {} {}", locker.pid, display_name(locker)))
        .collect::<Vec<_>>()
        .join("，");
    Some(format!("占用者包含关键系统进程，已拒绝解除占用: {listed}"))
}
