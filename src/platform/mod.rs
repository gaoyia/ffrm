//! 窗口标题栏、前置和打开链接。系统差异放在对应文件里。

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    open_url, prepare_window, set_topmost, title_inset, titlebar, toggle_zoom, ui_font,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{open_url, prepare_window, set_topmost, title_inset, titlebar, ui_font};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("ffrm 目前只支持 Windows 和 macOS");
