use gpui::{SharedString, TitlebarOptions, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub fn ui_font() -> &'static str {
    "Microsoft YaHei UI"
}

pub fn title_inset() -> f32 {
    0.
}

pub fn titlebar(title: impl Into<SharedString>) -> TitlebarOptions {
    TitlebarOptions {
        title: Some(title.into()),
        appears_transparent: true,
        traffic_light_position: None,
    }
}

pub fn prepare_window(window: &Window, pinned: bool) {
    apply_window_icon(window);
    if pinned {
        set_topmost(window, true);
    }
}

pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn window_hwnd(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get() as windows_sys::Win32::Foundation::HWND)
}

fn apply_window_icon(window: &Window) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, WM_SETICON,
    };
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    if module.is_null() {
        return;
    }
    unsafe {
        let big = LoadImageW(module, 1 as *const u16, IMAGE_ICON, 32, 32, 0);
        let small = LoadImageW(module, 1 as *const u16, IMAGE_ICON, 16, 16, 0);
        if !big.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        }
        if !small.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
    }
}

pub fn set_topmost(window: &Window, topmost: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let after = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    unsafe {
        SetWindowPos(
            hwnd,
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn toggle_zoom(window: &Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_MAXIMIZE, SW_RESTORE};
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let command = if window.is_maximized() {
        SW_RESTORE
    } else {
        SW_MAXIMIZE
    };
    unsafe {
        ShowWindowAsync(hwnd, command);
    }
}
