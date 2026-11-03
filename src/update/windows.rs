use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::fetch::{self, FetchError};

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    Available(String),
    Missing,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStep {
    Download(Option<u8>),
    Replace,
}

pub fn check(current: &str) -> Status {
    match fetch_tag() {
        Ok(tag) => compare(current, &tag),
        Err(FetchError::Missing) => Status::Missing,
        Err(FetchError::Network) => Status::Failed,
    }
}

pub fn install(mut report: impl FnMut(InstallStep) + Send) -> Result<(), ()> {
    let current = std::env::current_exe().map_err(|_| ())?;
    let (retired, incoming) = stage_paths(&current).ok_or(())?;
    let tag = fetch_tag().map_err(|_| ())?;
    let url = release_exe(&tag);
    if download(&url, &incoming, &mut report).is_err() {
        let _ = fs::remove_file(&incoming);
        return Err(());
    }
    report(InstallStep::Replace);
    if !swap_in(&current, &incoming, &retired) {
        let _ = fs::remove_file(&incoming);
        return Err(());
    }
    relaunch(&current)
}

pub fn clear_retired() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let Some((retired, incoming)) = stage_paths(&current) else {
        return;
    };
    let _ = fs::remove_file(retired);
    let _ = fs::remove_file(incoming);
}

fn compare(current: &str, tag: &str) -> Status {
    let Some(latest) = version_numbers(tag) else {
        return Status::Failed;
    };
    let Some(current) = version_numbers(current) else {
        return Status::Failed;
    };
    if latest > current {
        Status::Available(display_version(tag))
    } else {
        Status::Current
    }
}

fn fetch_tag() -> Result<String, FetchError> {
    release_tag(&fetch::latest_location()?)
}

fn release_exe(tag: &str) -> String {
    format!("https://github.com/gaoyia/ffrm/releases/download/{tag}/ffrm-win-x64.exe")
}

fn release_tag(location: &str) -> Result<String, FetchError> {
    let marker = "/releases/tag/";
    let Some(at) = location.find(marker) else {
        return Err(FetchError::Network);
    };
    let tag = location[at + marker.len()..]
        .split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        Err(FetchError::Network)
    } else {
        Ok(tag.to_string())
    }
}

fn download(url: &str, dest: &Path, report: &mut dyn FnMut(InstallStep)) -> Result<(), FetchError> {
    fetch::download(url, dest, &mut |percent| {
        report(InstallStep::Download(percent));
    })?;
    if !is_exe(dest) {
        return Err(FetchError::Network);
    }
    Ok(())
}

fn is_exe(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).is_ok() && magic == *b"MZ"
}

fn stage_paths(current: &Path) -> Option<(PathBuf, PathBuf)> {
    let parent = current.parent()?;
    let name = current.file_name()?;
    let mut retired = name.to_os_string();
    retired.push(".old");
    let mut incoming = name.to_os_string();
    incoming.push(".new");
    Some((parent.join(retired), parent.join(incoming)))
}

fn swap_in(current: &Path, incoming: &Path, retired: &Path) -> bool {
    let _ = fs::remove_file(retired);
    if fs::rename(current, retired).is_err() {
        return false;
    }
    if fs::rename(incoming, current).is_err() {
        let _ = fs::rename(retired, current);
        return false;
    }
    true
}

fn relaunch(exe: &Path) -> Result<(), ()> {
    let mut command = Command::new(exe);
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|_| ())
}

fn display_version(tag: &str) -> String {
    tag.trim().trim_start_matches(['v', 'V']).to_string()
}

fn version_numbers(text: &str) -> Option<[u64; 3]> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let text = text.split(['-', '+']).next().unwrap_or(text);
    let mut numbers = [0u64; 3];
    let mut count = 0usize;
    for part in text.split('.') {
        if count == 3 || part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        numbers[count] = part.parse().ok()?;
        count += 1;
    }
    if count == 0 {
        None
    } else {
        Some(numbers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_version_is_current() {
        assert_eq!(compare("0.1.0", "v0.1.0"), Status::Current);
        assert_eq!(compare("0.2.0", "v0.1.0"), Status::Current);
    }

    #[test]
    fn newer_tag_is_available() {
        assert_eq!(
            compare("0.1.0", "v0.2.0"),
            Status::Available("0.2.0".to_string())
        );
    }

    #[test]
    fn release_location_gives_the_tag_and_download() {
        let location = "https://github.com/gaoyia/ffrm/releases/tag/v0.2.0";
        assert_eq!(release_tag(location).unwrap(), "v0.2.0");
        assert_eq!(
            release_exe("v0.2.0"),
            "https://github.com/gaoyia/ffrm/releases/download/v0.2.0/ffrm-win-x64.exe"
        );
    }

    #[test]
    fn swap_replaces_the_current_file() {
        let dir = std::env::temp_dir().join(format!("ffrm-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("ffrm.exe");
        let incoming = dir.join("ffrm.exe.new");
        let retired = dir.join("ffrm.exe.old");
        fs::write(&current, b"old").unwrap();
        fs::write(&incoming, b"new").unwrap();
        assert!(swap_in(&current, &incoming, &retired));
        assert_eq!(fs::read(&current).unwrap(), b"new");
        assert_eq!(fs::read(&retired).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }
}
