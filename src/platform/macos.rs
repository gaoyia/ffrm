//! macOS 使用系统红绿灯，不再自绘最小化、缩放和关闭。

use std::ffi::{c_void, CStr};

use gpui::{point, px, SharedString, TitlebarOptions, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const NS_NORMAL_WINDOW_LEVEL: isize = 0;
const NS_FLOATING_WINDOW_LEVEL: isize = 3;

pub fn ui_font() -> &'static str {
    "PingFang SC"
}

/// 给左上角红绿灯留出的宽度。
pub fn title_inset() -> f32 {
    96.
}

pub fn titlebar(title: impl Into<SharedString>) -> TitlebarOptions {
    TitlebarOptions {
        title: Some(title.into()),
        appears_transparent: true,
        // 纵坐标会让系统把按钮放在 48 像素高的标题栏中间。
        traffic_light_position: Some(point(px(20.), px(16.))),
    }
}

pub fn prepare_window(window: &Window, pinned: bool) {
    if pinned {
        set_topmost(window, true);
    }
}

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
}

pub fn set_topmost(window: &Window, topmost: bool) {
    let Some(view) = ns_view(window) else {
        return;
    };
    unsafe {
        let ns_window = msg_send_id(view, sel(c"window"));
        if ns_window.is_null() {
            return;
        }
        let level = if topmost {
            NS_FLOATING_WINDOW_LEVEL
        } else {
            NS_NORMAL_WINDOW_LEVEL
        };
        msg_send_void(ns_window, sel(c"setLevel:"), level);
    }
}

fn ns_view(window: &Window) -> Option<*mut c_void> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    Some(appkit.ns_view.as_ptr())
}

fn sel(name: &CStr) -> *const c_void {
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` 在 Apple Silicon 上按普通 C 调用约定传参。
unsafe fn msg_send_id(receiver: *mut c_void, selector: *const c_void) -> *mut c_void {
    let send: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const ());
    send(receiver, selector)
}

unsafe fn msg_send_void(receiver: *mut c_void, selector: *const c_void, level: isize) {
    let send: unsafe extern "C" fn(*mut c_void, *const c_void, isize) =
        std::mem::transmute(objc_msgSend as *const ());
    send(receiver, selector, level);
}

#[link(name = "objc")]
extern "C" {
    fn sel_registerName(name: *const i8) -> *const c_void;
    fn objc_msgSend();
}
