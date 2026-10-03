use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf, Prefix};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_MORE_DATA, ERROR_SHARING_VIOLATION,
    ERROR_SUCCESS, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileAttributesW, GetLongPathNameW, RemoveDirectoryW, SetFileAttributesW,
    FILE_ATTRIBUTE_READONLY, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
};
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmForceShutdown, RmGetList, RmRegisterResources, RmShutdown, RmStartSession,
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

const DELETE_ACCESS: u32 = 0x0001_0000;
const RM_CRITICAL: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locker {
    pub pid: u32,
    pub app_name: String,
    pub image: Option<String>,
    pub app_type: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub path: PathBuf,
    pub lockers: Vec<Locker>,
    pub sharing_violation: bool,
    pub list_denied: bool,
}

pub fn inspect(path: &Path) -> Result<Inspection, String> {
    let path = normalize_existing(path)?;
    let (lockers, list_denied) = query_lockers(&path)?;
    let sharing_violation = is_sharing_violation(&path);
    Ok(Inspection {
        path,
        lockers,
        sharing_violation,
        list_denied,
    })
}

pub fn release(path: &Path, force: bool) -> Result<Vec<Locker>, String> {
    let inspection = inspect(path)?;
    if inspection.lockers.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(reason) = critical_block(&inspection.lockers) {
        return Err(reason);
    }
    let mut shutdown_error = match shutdown(&inspection.path, force) {
        Ok(()) => None,
        Err(error) => Some(error),
    };
    if !force && still_locked(path)? {
        shutdown_error = match shutdown(path, true) {
            Ok(()) => None,
            Err(error) => Some(error),
        };
    }
    for _ in 0..3 {
        if !still_locked(path)? {
            return Ok(inspection.lockers);
        }
        let current = inspect(path)?;
        let targets = if current.lockers.is_empty() {
            inspection.lockers.clone()
        } else {
            current.lockers
        };
        if let Some(reason) = critical_block(&targets) {
            return Err(reason);
        }
        for locker in &targets {
            end_process(locker.pid)?;
        }
    }
    if still_locked(path)? {
        if let Some(error) = shutdown_error {
            return Err(error);
        }
        return Err(format!("{} 仍被占用", path.display()));
    }
    Ok(inspection.lockers)
}

pub fn delete_path(path: &Path, recursive: bool) -> Result<PathBuf, String> {
    if let Some(reason) = block_reason(path) {
        return Err(reason.to_string());
    }
    let path = normalize_existing(path)?;
    if let Some(reason) = block_reason(&path) {
        return Err(reason.to_string());
    }

    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|err| format!("无法读取 {}: {err}", path.display()))?;
    if metadata.file_type().is_symlink() || is_reparse_point(&path) {
        remove_link(&path)?;
        return Ok(path);
    }
    if metadata.is_dir() {
        remove_directory(&path, recursive)?;
        return Ok(path);
    }
    clear_readonly(&path)?;
    std::fs::remove_file(&path).map_err(|err| format!("删除 {} 失败: {err}", path.display()))?;
    Ok(path)
}

pub fn block_reason(path: &Path) -> Option<&'static str> {
    if path.as_os_str().is_empty() || is_drive_only(path) {
        return Some("不能删除磁盘根目录");
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if is_filesystem_root(&canon) || is_unc_share_root(&canon) {
        return Some("不能删除磁盘根目录");
    }
    for known in protected_dirs() {
        if same_path(&canon, &known) {
            return Some("不能删除系统目录或用户主目录本身");
        }
    }
    None
}

pub fn is_critical(locker: &Locker) -> bool {
    if locker.pid <= 4 || locker.app_type == RM_CRITICAL {
        return true;
    }
    const NAMES: &[&str] = &[
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "winlogon.exe",
        "services.exe",
        "lsass.exe",
        "dwm.exe",
        "fontdrvhost.exe",
    ];
    let mut names = Vec::new();
    if let Some(image) = &locker.image {
        if let Some(file_name) = Path::new(image).file_name() {
            names.push(file_name.to_string_lossy().to_ascii_lowercase());
        }
    }
    if !locker.app_name.is_empty() {
        names.push(locker.app_name.to_ascii_lowercase());
    }
    names.iter().any(|name| NAMES.contains(&name.as_str()))
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

fn normalize_existing(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("路径是空的".to_string());
    }
    let absolute = std::path::absolute(path)
        .map_err(|err| format!("无法解析路径 {}: {err}", path.display()))?;
    if !absolute.exists() {
        return Err(format!("路径不存在: {}", absolute.display()));
    }
    Ok(long_path(&absolute))
}

fn query_lockers(path: &Path) -> Result<(Vec<Locker>, bool), String> {
    let session = RestartSession::start()?;
    match session.register(path) {
        Ok(()) => {}
        Err(ListError::Denied) => return Ok((Vec::new(), true)),
        Err(ListError::Other(message)) => return Err(message),
    }
    match session.list() {
        Ok(lockers) => Ok((lockers, false)),
        Err(ListError::Denied) => Ok((Vec::new(), true)),
        Err(ListError::Other(message)) => Err(message),
    }
}

fn still_locked(path: &Path) -> Result<bool, String> {
    match inspect(path) {
        Ok(inspection) => Ok(inspection.sharing_violation || !inspection.lockers.is_empty()),
        Err(error) if error.contains("路径不存在") => Ok(false),
        Err(error) => Err(error),
    }
}

fn end_process(pid: u32) -> Result<(), String> {
    if pid <= 4 || pid == unsafe { GetCurrentProcessId() } {
        return Err(format!("拒绝结束 pid {pid}"));
    }
    let access = PROCESS_TERMINATE | 0x0010_0000;
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        let code = unsafe { GetLastError() };
        if code == 87 {
            return Ok(());
        }
        return Err(format!("无法结束 pid {pid}: {}", win_message(code)));
    }
    let ended = unsafe { TerminateProcess(handle, 1) };
    if ended == 0 {
        let code = unsafe { GetLastError() };
        unsafe { CloseHandle(handle) };
        if code == 87 {
            return Ok(());
        }
        return Err(format!("无法结束 pid {pid}: {}", win_message(code)));
    }
    unsafe {
        WaitForSingleObject(handle, 3000);
        CloseHandle(handle);
    }
    Ok(())
}

fn shutdown(path: &Path, force: bool) -> Result<(), String> {
    let session = RestartSession::start()?;
    session.register(path).map_err(|error| match error {
        ListError::Denied => "没有权限解除占用".to_string(),
        ListError::Other(message) => message,
    })?;
    let flags = if force { RmForceShutdown as u32 } else { 0 };
    let code = unsafe { RmShutdown(session.0, flags, None) };
    if code != ERROR_SUCCESS {
        return Err(format!("解除占用失败: {}", win_message(code)));
    }
    Ok(())
}

enum ListError {
    Denied,
    Other(String),
}

struct RestartSession(u32);

impl RestartSession {
    fn start() -> Result<Self, String> {
        let mut handle = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let code = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        if code != ERROR_SUCCESS {
            return Err(format!("无法开始检查文件占用: {}", win_message(code)));
        }
        Ok(Self(handle))
    }

    fn register(&self, path: &Path) -> Result<(), ListError> {
        let wide = wide_path(path);
        let name = wide.as_ptr();
        let code = unsafe {
            RmRegisterResources(self.0, 1, &name, 0, std::ptr::null(), 0, std::ptr::null())
        };
        if code == ERROR_ACCESS_DENIED {
            return Err(ListError::Denied);
        }
        if code != ERROR_SUCCESS {
            return Err(ListError::Other(format!(
                "无法登记 {}: {}",
                path.display(),
                win_message(code)
            )));
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<Locker>, ListError> {
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut reasons = 0u32;
        let code = unsafe {
            RmGetList(
                self.0,
                &mut needed,
                &mut count,
                std::ptr::null_mut(),
                &mut reasons,
            )
        };
        if code == ERROR_SUCCESS && needed == 0 {
            return Ok(Vec::new());
        }
        if code == ERROR_ACCESS_DENIED {
            return Err(ListError::Denied);
        }
        if code != ERROR_MORE_DATA && !(code == ERROR_SUCCESS && needed > 0) {
            return Err(ListError::Other(format!(
                "无法列出占用进程: {}",
                win_message(code)
            )));
        }
        if needed == 0 {
            return Ok(Vec::new());
        }
        if needed > 4096 {
            return Err(ListError::Other("占用进程过多，已停止处理".to_string()));
        }
        let mut infos = Vec::with_capacity(needed as usize);
        for _ in 0..needed {
            infos.push(unsafe { std::mem::zeroed::<RM_PROCESS_INFO>() });
        }
        count = needed;
        let code = unsafe {
            RmGetList(
                self.0,
                &mut needed,
                &mut count,
                infos.as_mut_ptr(),
                &mut reasons,
            )
        };
        if code == ERROR_ACCESS_DENIED {
            return Err(ListError::Denied);
        }
        if code != ERROR_SUCCESS {
            return Err(ListError::Other(format!(
                "无法列出占用进程: {}",
                win_message(code)
            )));
        }
        let mut lockers = Vec::new();
        for info in infos.into_iter().take(count as usize) {
            let pid = info.Process.dwProcessId;
            if lockers.iter().any(|locker: &Locker| locker.pid == pid) {
                continue;
            }
            lockers.push(Locker {
                pid,
                app_name: wide_to_string(&info.strAppName),
                image: process_image(pid),
                app_type: info.ApplicationType as u32,
            });
        }
        lockers.sort_by_key(|locker| locker.pid);
        Ok(lockers)
    }
}

impl Drop for RestartSession {
    fn drop(&mut self) {
        unsafe {
            RmEndSession(self.0);
        }
    }
}

fn process_image(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buffer = [0u16; 32768];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 || size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

fn is_sharing_violation(path: &Path) -> bool {
    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            DELETE_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return unsafe { GetLastError() } == ERROR_SHARING_VIOLATION;
    }
    unsafe {
        CloseHandle(handle);
    }
    false
}

fn remove_directory(path: &Path, recursive: bool) -> Result<(), String> {
    if !recursive {
        let mut entries = std::fs::read_dir(path)
            .map_err(|err| format!("无法读取目录 {}: {err}", path.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "{} 是非空目录，删除请加 --recursive",
                path.display()
            ));
        }
        std::fs::remove_dir(path)
            .map_err(|err| format!("删除目录 {} 失败: {err}", path.display()))?;
        return Ok(());
    }
    std::fs::remove_dir_all(path)
        .map_err(|err| format!("删除目录 {} 失败: {err}", path.display()))?;
    Ok(())
}

fn remove_link(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|err| format!("无法读取 {}: {err}", path.display()))?;
    if metadata.file_type().is_symlink() && !metadata.is_dir() {
        std::fs::remove_file(path).map_err(|err| format!("删除 {} 失败: {err}", path.display()))?;
        return Ok(());
    }
    let wide = wide_path(path);
    let ok = unsafe { RemoveDirectoryW(wide.as_ptr()) };
    if ok == 0 {
        return Err(format!(
            "删除链接 {} 失败: {}",
            path.display(),
            win_message(unsafe { GetLastError() })
        ));
    }
    Ok(())
}

fn is_reparse_point(path: &Path) -> bool {
    let wide = wide_path(path);
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    attributes != INVALID_FILE_ATTRIBUTES && attributes & 0x400 != 0
}

fn clear_readonly(path: &Path) -> Result<(), String> {
    let wide = wide_path(path);
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_READONLY == 0 {
        return Ok(());
    }
    let ok = unsafe { SetFileAttributesW(wide.as_ptr(), attributes & !FILE_ATTRIBUTE_READONLY) };
    if ok == 0 {
        return Err(format!(
            "无法去掉 {} 的只读属性: {}",
            path.display(),
            win_message(unsafe { GetLastError() })
        ));
    }
    Ok(())
}

fn protected_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(windows) = std::env::var_os("SystemRoot") {
        let windows = PathBuf::from(&windows);
        dirs.push(windows.join("System32"));
        dirs.push(windows.join("SysWOW64"));
        dirs.push(windows);
    }
    if let Some(drive) = std::env::var_os("SystemDrive") {
        dirs.push(PathBuf::from(drive).join("Users"));
    }
    for name in [
        "USERPROFILE",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ] {
        if let Some(value) = std::env::var_os(name) {
            dirs.push(PathBuf::from(value));
        }
    }
    dirs
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn is_drive_only(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    trimmed.len() == 2 && trimmed.as_bytes().get(1) == Some(&b':')
}

fn is_filesystem_root(path: &Path) -> bool {
    let mut saw_root = false;
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => saw_root = true,
            Component::Normal(_) | Component::CurDir | Component::ParentDir => return false,
        }
    }
    saw_root
}

fn is_unc_share_root(path: &Path) -> bool {
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(..)) => {
            components.all(|component| !matches!(component, Component::Normal(_)))
        }
        _ => false,
    }
}

fn long_path(path: &Path) -> PathBuf {
    let wide = wide_path(path);
    let mut buffer = vec![0u16; 32768];
    let length =
        unsafe { GetLongPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return path.to_path_buf();
    }
    PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().to_string()
}

fn win_message(code: u32) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            std::ptr::null(),
        )
    };
    if length == 0 {
        return format!("Windows 错误 {code}");
    }
    let message = String::from_utf16_lossy(&buffer[..length as usize]);
    let message = message.trim();
    if message.is_empty() {
        format!("Windows 错误 {code}")
    } else {
        format!("{message}（{code}）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn blocks_drive_root_and_windows_dir() {
        assert_eq!(block_reason(Path::new(r"C:\")), Some("不能删除磁盘根目录"));
        assert_eq!(block_reason(Path::new("C:")), Some("不能删除磁盘根目录"));
        let windows = std::env::var("SystemRoot").unwrap();
        assert!(block_reason(Path::new(&windows)).is_some());
        let profile = std::env::var("USERPROFILE").unwrap();
        assert!(block_reason(Path::new(&profile)).is_some());
    }

    #[test]
    fn allows_a_normal_temp_file() {
        let path = std::env::temp_dir().join("ffrm-allow-check.txt");
        fs::write(&path, b"ok").unwrap();
        assert_eq!(block_reason(&path), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn deletes_readonly_file_and_empty_dir() {
        let file = std::env::temp_dir().join("ffrm-readonly-delete.txt");
        fs::write(&file, b"gone").unwrap();
        let wide = wide_path(&file);
        unsafe {
            let attributes = GetFileAttributesW(wide.as_ptr());
            SetFileAttributesW(wide.as_ptr(), attributes | FILE_ATTRIBUTE_READONLY);
        }
        delete_path(&file, false).unwrap();
        assert!(!file.exists());

        let dir = std::env::temp_dir().join("ffrm-empty-dir");
        fs::create_dir_all(&dir).unwrap();
        delete_path(&dir, false).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn refuses_nonempty_dir_without_recursive_and_removes_with_it() {
        let dir = std::env::temp_dir().join("ffrm-nested-dir");
        let child = dir.join("child.txt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&child, b"x").unwrap();
        assert!(delete_path(&dir, false).is_err());
        assert!(child.exists());
        delete_path(&dir, true).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn inspects_unlocked_temp_file() {
        let file = std::env::temp_dir().join("ffrm-inspect.txt");
        fs::write(&file, b"free").unwrap();
        let inspection = inspect(&file).unwrap();
        assert!(inspection.lockers.is_empty());
        assert!(!inspection.sharing_violation);
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn detects_critical_processes() {
        let system = Locker {
            pid: 4,
            app_name: "System".to_string(),
            image: None,
            app_type: RM_CRITICAL,
        };
        assert!(is_critical(&system));
        let notepad = Locker {
            pid: 1200,
            app_name: "记事本".to_string(),
            image: Some(r"C:\Windows\System32\notepad.exe".to_string()),
            app_type: 1,
        };
        assert!(!is_critical(&notepad));
        let lsass = Locker {
            pid: 800,
            app_name: "Local Security Authority Process".to_string(),
            image: Some(r"C:\Windows\System32\lsass.exe".to_string()),
            app_type: 3,
        };
        assert!(is_critical(&lsass));
    }
}
