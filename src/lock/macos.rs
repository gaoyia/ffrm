//! 用 libproc 查看谁打开了文件，再用信号让进程退出。

use std::ffi::{c_void, CStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use super::{critical_block, display_name, Inspection, Locker};

const PROC_PIDLISTFDS: i32 = 1;
const PROC_PIDVNODEPATHINFO: i32 = 9;
const PROC_PIDFDVNODEPATHINFO: i32 = 2;
const PROX_FDTYPE_VNODE: u32 = 1;
const FD_INFO_SIZE: usize = 8;
const VNODE_WITH_PATH_SIZE: usize = 1200;
const VNODE_PATH_OFFSET: usize = 176;
const CWD_INFO_SIZE: usize = 2352;
const CWD_PATH_OFFSET: usize = 152;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;
const ESRCH: i32 = 3;

pub fn inspect(path: &Path) -> Result<Inspection, String> {
    let path = normalize_existing(path)?;
    let targets = match_targets(&path);
    let directory = is_directory(&path);
    let mut lockers = Vec::new();
    let self_pid = std::process::id();
    for pid in all_pids()? {
        if pid == self_pid {
            continue;
        }
        if !process_holds(pid as i32, &targets, directory) {
            continue;
        }
        lockers.push(Locker {
            pid,
            app_name: process_name(pid as i32),
            image: process_path(pid as i32),
            app_type: 0,
        });
    }
    lockers.sort_by_key(|locker| locker.pid);
    Ok(Inspection {
        path,
        lockers,
        sharing_violation: false,
        list_denied: false,
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
    let first = if force { SIGKILL } else { SIGTERM };
    for locker in &inspection.lockers {
        signal(locker.pid, first)?;
    }
    if wait_until_free(path, Duration::from_secs(3))? {
        return Ok(inspection.lockers);
    }
    if !force {
        let current = inspect(path)?;
        if let Some(reason) = critical_block(&current.lockers) {
            return Err(reason);
        }
        let targets = if current.lockers.is_empty() {
            inspection.lockers.clone()
        } else {
            current.lockers
        };
        for locker in &targets {
            signal(locker.pid, SIGKILL)?;
        }
    }
    if wait_until_free(path, Duration::from_secs(2))? {
        return Ok(inspection.lockers);
    }
    Err(format!("{} 仍被占用", path.display()))
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
    if metadata.file_type().is_symlink() {
        clear_user_flags(&path);
        std::fs::remove_file(&path)
            .map_err(|err| format!("删除 {} 失败: {err}", path.display()))?;
        return Ok(path);
    }
    if metadata.is_dir() {
        if recursive {
            clear_tree(&path);
        } else {
            clear_user_flags(&path);
        }
        remove_directory(&path, recursive)?;
        return Ok(path);
    }
    clear_user_flags(&path);
    std::fs::remove_file(&path).map_err(|err| format!("删除 {} 失败: {err}", path.display()))?;
    Ok(path)
}

pub fn block_reason(path: &Path) -> Option<&'static str> {
    if path.as_os_str().is_empty() || is_filesystem_root(path) {
        return Some("不能删除磁盘根目录");
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if is_filesystem_root(&canon) {
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
    if locker.pid <= 1 {
        return true;
    }
    const NAMES: &[&str] = &["kernel_task", "launchd", "windowserver", "loginwindow"];
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

fn wait_until_free(path: &Path, limit: Duration) -> Result<bool, String> {
    let start = Instant::now();
    loop {
        if !still_locked(path)? {
            return Ok(true);
        }
        if start.elapsed() >= limit {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn still_locked(path: &Path) -> Result<bool, String> {
    match inspect(path) {
        Ok(inspection) => Ok(!inspection.lockers.is_empty()),
        Err(error) if error.contains("路径不存在") => Ok(false),
        Err(error) => Err(error),
    }
}

fn signal(pid: u32, sig: i32) -> Result<(), String> {
    if pid <= 1 || pid == std::process::id() {
        return Err(format!("拒绝结束 pid {pid}"));
    }
    let rc = unsafe { kill(pid as i32, sig) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(ESRCH) {
        return Ok(());
    }
    Err(format!(
        "无法结束 pid {pid} {}：{err}",
        display_name(&Locker {
            pid,
            app_name: process_name(pid as i32),
            image: process_path(pid as i32),
            app_type: 0,
        })
    ))
}

fn normalize_existing(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("路径是空的".to_string());
    }
    let absolute = std::path::absolute(path)
        .map_err(|err| format!("无法解析路径 {}: {err}", path.display()))?;
    if std::fs::symlink_metadata(&absolute).is_err() {
        return Err(format!("路径不存在: {}", absolute.display()));
    }
    Ok(absolute)
}

fn match_targets(path: &Path) -> Vec<PathBuf> {
    let mut targets = vec![path.to_path_buf()];
    if let Ok(canon) = path.canonicalize() {
        if canon != path {
            targets.push(canon);
        }
    }
    targets
}

fn is_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn process_holds(pid: i32, targets: &[PathBuf], directory: bool) -> bool {
    if directory {
        if let Some(cwd) = process_cwd(pid) {
            if path_matches(&cwd, targets, true) {
                return true;
            }
        }
    }
    vnode_holds(pid, targets, directory)
}

fn path_matches(found: &str, targets: &[PathBuf], directory: bool) -> bool {
    let found = Path::new(found);
    if found.as_os_str().is_empty() {
        return false;
    }
    targets
        .iter()
        .any(|target| found == target || (directory && found.starts_with(target)))
}

fn all_pids() -> Result<Vec<u32>, String> {
    let guessed = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if guessed < 0 {
        return Err("无法列出进程".to_string());
    }
    let capacity = (guessed as usize).saturating_add(128).max(64);
    let mut buffer = vec![0i32; capacity];
    let got = unsafe {
        proc_listallpids(
            buffer.as_mut_ptr().cast(),
            (buffer.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if got < 0 {
        return Err("无法列出进程".to_string());
    }
    let count = (got as usize).min(buffer.len());
    let mut pids = Vec::new();
    for pid in &buffer[..count] {
        if *pid > 0 {
            pids.push(*pid as u32);
        }
    }
    Ok(pids)
}

fn vnode_holds(pid: i32, targets: &[PathBuf], directory: bool) -> bool {
    let mut byte_len = FD_INFO_SIZE * 256;
    for _ in 0..5 {
        let mut buffer = vec![0u8; byte_len];
        let written = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDLISTFDS,
                0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as i32,
            )
        };
        if written <= 0 {
            return false;
        }
        if written as usize >= buffer.len() {
            byte_len = buffer.len().saturating_mul(2);
            continue;
        }
        let count = written as usize / FD_INFO_SIZE;
        for index in 0..count {
            let start = index * FD_INFO_SIZE;
            let fd = i32::from_ne_bytes(buffer[start..start + 4].try_into().unwrap_or([0; 4]));
            let kind =
                u32::from_ne_bytes(buffer[start + 4..start + 8].try_into().unwrap_or([0; 4]));
            if kind != PROX_FDTYPE_VNODE {
                continue;
            }
            if let Some(path) = vnode_path(pid, fd) {
                if path_matches(&path, targets, directory) {
                    return true;
                }
            }
        }
        return false;
    }
    false
}

fn vnode_path(pid: i32, fd: i32) -> Option<String> {
    let mut buffer = vec![0u8; VNODE_WITH_PATH_SIZE];
    let written = unsafe {
        proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDVNODEPATHINFO,
            buffer.as_mut_ptr().cast(),
            buffer.len() as i32,
        )
    };
    if written < VNODE_PATH_OFFSET as i32 {
        return None;
    }
    c_string(&buffer[VNODE_PATH_OFFSET..])
}

fn process_cwd(pid: i32) -> Option<String> {
    let mut buffer = vec![0u8; CWD_INFO_SIZE];
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDVNODEPATHINFO,
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as i32,
        )
    };
    if written < CWD_PATH_OFFSET as i32 {
        return None;
    }
    c_string(&buffer[CWD_PATH_OFFSET..])
}

fn process_name(pid: i32) -> String {
    let mut buffer = [0u8; 256];
    let written = unsafe { proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 {
        return String::new();
    }
    c_string(&buffer).unwrap_or_default()
}

fn process_path(pid: i32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    let written = unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 {
        return None;
    }
    c_string(&buffer)
}

fn c_string(buffer: &[u8]) -> Option<String> {
    let text = CStr::from_bytes_until_nul(buffer).ok()?;
    let text = text.to_string_lossy();
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
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

fn clear_tree(path: &Path) {
    clear_user_flags(path);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        clear_tree(&entry.path());
    }
}

fn clear_user_flags(path: &Path) {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    unsafe {
        chflags(bytes.as_ptr().cast(), 0);
    }
}

fn protected_dirs() -> Vec<PathBuf> {
    let mut dirs = [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/Library",
        "/Applications",
        "/Users",
        "/private",
        "/var",
        "/opt",
        "/Volumes",
        "/dev",
        "/etc",
        "/System/Volumes/Data",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home));
    }
    dirs
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
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

#[link(name = "proc")]
extern "C" {
    fn proc_listallpids(buffer: *mut c_void, buffersize: i32) -> i32;
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, buffersize: i32) -> i32;
    fn proc_pidfdinfo(pid: i32, fd: i32, flavor: i32, buffer: *mut c_void, buffersize: i32) -> i32;
    fn proc_name(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn chflags(path: *const i8, flags: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_root_system_and_home() {
        assert_eq!(block_reason(Path::new("/")), Some("不能删除磁盘根目录"));
        assert!(block_reason(Path::new("/System")).is_some());
        assert!(block_reason(Path::new("/Applications")).is_some());
        let home = std::env::var("HOME").unwrap();
        assert!(block_reason(Path::new(&home)).is_some());
    }

    #[test]
    fn allows_and_deletes_a_temp_file_and_directory() {
        let file = std::env::temp_dir().join(format!("ffrm-allow-{}", std::process::id()));
        std::fs::write(&file, b"ok").unwrap();
        assert_eq!(block_reason(&file), None);
        delete_path(&file, false).unwrap();
        assert!(!file.exists());

        let dir = std::env::temp_dir().join(format!("ffrm-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        delete_path(&dir, false).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn refuses_nonempty_dir_without_recursive() {
        let dir = std::env::temp_dir().join(format!("ffrm-nested-{}", std::process::id()));
        let child = dir.join("child.txt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&child, b"x").unwrap();
        assert!(delete_path(&dir, false).is_err());
        assert!(child.exists());
        delete_path(&dir, true).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn sees_a_process_that_has_the_file_open() {
        let file = std::env::temp_dir().join(format!("ffrm-held-{}", std::process::id()));
        std::fs::write(&file, b"held").unwrap();
        let mut child = std::process::Command::new("/bin/bash")
            .args(["-c", "sleep 30 < \"$1\"", "bash"])
            .arg(&file)
            .spawn()
            .unwrap();
        let mut inspection = None;
        for _ in 0..40 {
            let current = inspect(&file).unwrap();
            if !current.lockers.is_empty() {
                inspection = Some(current);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let inspection = inspection.expect("没有看到打开该文件的进程");
        for locker in &inspection.lockers {
            let _ = signal(locker.pid, SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&file);
        assert!(
            inspection
                .lockers
                .iter()
                .any(|locker| locker.pid != std::process::id()),
            "{:?}",
            inspection.lockers
        );
    }

    #[test]
    fn critical_names_are_refused() {
        let launchd = Locker {
            pid: 1,
            app_name: "launchd".to_string(),
            image: Some("/sbin/launchd".to_string()),
            app_type: 0,
        };
        assert!(is_critical(&launchd));
        let sleep = Locker {
            pid: 400,
            app_name: "sleep".to_string(),
            image: Some("/bin/sleep".to_string()),
            app_type: 0,
        };
        assert!(!is_critical(&sleep));
        let window_server = Locker {
            pid: 300,
            app_name: "WindowServer".to_string(),
            image: None,
            app_type: 0,
        };
        assert!(is_critical(&window_server));
    }
}
