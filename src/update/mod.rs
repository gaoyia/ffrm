//! 检查并安装新版本。网络和替换程序的实现按系统分文件存放。

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{check, clear_retired, install, InstallStep, Status};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{check, clear_retired, install, InstallStep, Status};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("ffrm 目前只支持 Windows 和 macOS");
