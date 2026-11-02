//! 右键入口。Windows 写注册表，macOS 安装访达服务，两边分文件实现。

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{extra_hint, install, remove, state, MenuState};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{extra_hint, install, remove, state, MenuState};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("ffrm 目前只支持 Windows 和 macOS");
