use std::path::Path;

use crate::config::Lang;

use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};
use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

const LOCATIONS: [&str; 2] = [
    r"Software\Classes\*\shell\ffrm",
    r"Software\Classes\Directory\shell\ffrm",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuState {
    Absent,
    Current,
    Other,
}

enum Slot {
    Missing,
    Current,
    Other,
}

struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}

pub fn state() -> MenuState {
    let Ok(exe) = std::env::current_exe() else {
        return MenuState::Absent;
    };
    let slots = LOCATIONS.map(|location| classify(location, &exe));
    if slots.iter().all(|slot| matches!(slot, Slot::Missing)) {
        MenuState::Absent
    } else if slots.iter().all(|slot| matches!(slot, Slot::Current)) {
        MenuState::Current
    } else {
        MenuState::Other
    }
}

pub fn install(label: &str, lang: Lang) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|_| missing_exe(lang))?;
    for location in LOCATIONS {
        write_verb(location, label, &exe).map_err(|code| fail(lang, code))?;
    }
    notify_shell();
    Ok(())
}

pub fn remove(lang: Lang) -> Result<(), String> {
    for location in LOCATIONS {
        delete_verb(location).map_err(|code| fail(lang, code))?;
    }
    notify_shell();
    Ok(())
}

pub fn windows_11() -> bool {
    let Ok(key) = open_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        KEY_READ,
    ) else {
        return false;
    };
    let Ok(text) = query_value(key.0, Some("CurrentBuildNumber")) else {
        return false;
    };
    text.trim().parse::<u32>().is_ok_and(|build| build >= 22000)
}

fn classify(location: &str, exe: &Path) -> Slot {
    match read_command(location) {
        Ok(command) if command_uses_exe(&command, exe) => Slot::Current,
        Ok(_) => Slot::Other,
        Err(code) if code == ERROR_FILE_NOT_FOUND => {
            if open_key(HKEY_CURRENT_USER, location, KEY_READ).is_ok() {
                Slot::Other
            } else {
                Slot::Missing
            }
        }
        Err(_) => Slot::Other,
    }
}

fn write_verb(location: &str, label: &str, exe: &Path) -> Result<(), u32> {
    let verb = create_key(location)?;
    set_sz(verb.0, None, label)?;
    set_sz(verb.0, Some("Icon"), &icon_value(exe))?;
    set_sz(verb.0, Some("MultiSelectModel"), "Document")?;
    set_sz(verb.0, Some("NeverDefault"), "")?;
    let command = create_key(&format!(r"{location}\command"))?;
    set_sz(command.0, None, &command_line(exe))?;
    Ok(())
}

fn delete_verb(location: &str) -> Result<(), u32> {
    let name = wide(location);
    let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
    if code == 0 || code == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(code)
    }
}

fn read_command(location: &str) -> Result<String, u32> {
    let key = open_key(HKEY_CURRENT_USER, &format!(r"{location}\command"), KEY_READ)?;
    query_value(key.0, None)
}

fn command_line(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

fn icon_value(exe: &Path) -> String {
    format!("{},0", exe.display())
}

fn command_uses_exe(command: &str, exe: &Path) -> bool {
    first_argument(command)
        .map(|found| same_path(&found, exe))
        .unwrap_or(false)
}

fn first_argument(command: &str) -> Option<String> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    Some(command.split_whitespace().next()?.to_string())
}

fn same_path(stored: &str, exe: &Path) -> bool {
    normalize(stored) == normalize(&exe.display().to_string())
}

fn normalize(path: &str) -> String {
    path.trim()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn notify_shell() {
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

fn missing_exe(lang: Lang) -> String {
    match lang {
        Lang::Zh => "无法定位程序自身的路径".to_string(),
        Lang::En => "Could not find this program".to_string(),
    }
}

fn fail(lang: Lang, code: u32) -> String {
    if code == 5 {
        return match lang {
            Lang::Zh => "没有权限修改右键菜单".to_string(),
            Lang::En => "Permission denied while changing the context menu".to_string(),
        };
    }
    match lang {
        Lang::Zh => format!("无法修改右键菜单（错误 {code}）"),
        Lang::En => format!("Could not change the context menu (error {code})"),
    }
}

fn open_key(root: HKEY, path: &str, access: u32) -> Result<RegKey, u32> {
    let mut key = std::ptr::null_mut();
    let name = wide(path);
    let code = unsafe { RegOpenKeyExW(root, name.as_ptr(), 0, access, &mut key) };
    checked(code)?;
    Ok(RegKey(key))
}

fn create_key(path: &str) -> Result<RegKey, u32> {
    let mut key = std::ptr::null_mut();
    let name = wide(path);
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            name.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    checked(code)?;
    Ok(RegKey(key))
}

fn set_sz(key: HKEY, name: Option<&str>, value: &str) -> Result<(), u32> {
    let name_buf = name.map(wide);
    let name_ptr = name_buf
        .as_ref()
        .map(|buf| buf.as_ptr())
        .unwrap_or(std::ptr::null());
    let data = wide(value);
    let code = unsafe {
        RegSetValueExW(
            key,
            name_ptr,
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        )
    };
    checked(code)
}

fn query_value(key: HKEY, name: Option<&str>) -> Result<String, u32> {
    let name_buf = name.map(wide);
    let name_ptr = name_buf
        .as_ref()
        .map(|buf| buf.as_ptr())
        .unwrap_or(std::ptr::null());
    let mut kind = 0u32;
    let mut size = 0u32;
    let code = unsafe {
        RegQueryValueExW(
            key,
            name_ptr,
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if code != 0 && code != 234 {
        return Err(code);
    }
    if size == 0 {
        return Ok(String::new());
    }
    let mut data = vec![0u8; size as usize];
    let code = unsafe {
        RegQueryValueExW(
            key,
            name_ptr,
            std::ptr::null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        )
    };
    checked(code)?;
    let end = size.min(data.len() as u32) as usize;
    Ok(utf16_bytes(&data[..end]))
}

fn utf16_bytes(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    while units.last() == Some(&0) {
        units.pop();
    }
    String::from_utf16_lossy(&units)
}

fn checked(code: u32) -> Result<(), u32> {
    if code == 0 {
        Ok(())
    } else {
        Err(code)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_matches_this_exe_only() {
        let exe = Path::new(r"C:\Program Files\ffrm.exe");
        let command = command_line(exe);
        assert!(command_uses_exe(&command, exe));
        assert!(command_uses_exe(
            &command,
            Path::new(r"\\?\c:\program files\ffrm.exe")
        ));
        assert!(!command_uses_exe(r#""D:\other\ffrm.exe" "%1""#, exe));
    }

    #[test]
    fn icon_points_at_the_exe() {
        let exe = Path::new(r"C:\Program Files\ffrm.exe");
        assert_eq!(icon_value(exe), r"C:\Program Files\ffrm.exe,0");
    }
}
