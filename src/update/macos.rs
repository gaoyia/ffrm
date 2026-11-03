use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::fetch::{self, FetchError};

extern "C" {
    fn setsid() -> i32;
}

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
    let url = release_binary(&tag);
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
    if let Some((retired, incoming)) = stage_paths(&current) {
        let _ = fs::remove_file(retired);
        let _ = fs::remove_file(incoming);
    }
    // 旧版本把临时文件留在安装包里，签名会被破坏。这里清掉后再按需要重签。
    if let Some(macos) = current.parent().filter(|_| app_bundle(&current).is_some()) {
        let _ = fs::remove_file(macos.join("ffrm.old"));
        let _ = fs::remove_file(macos.join("ffrm.new"));
    }
    if let Some(app) = app_bundle(&current) {
        if !bundle_is_valid(&app) {
            let _ = resign(&app);
        }
    }
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

fn release_binary(tag: &str) -> String {
    format!("https://github.com/gaoyia/ffrm/releases/download/{tag}/ffrm-darwin-arm64")
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
    if !is_macho(dest) {
        return Err(FetchError::Network);
    }
    // 下载得到的文件没有执行权限，替换后无法重新启动。
    make_executable(dest)
}

fn make_executable(path: &Path) -> Result<(), FetchError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|_| FetchError::Network)?;
    let mut permissions = meta.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions).map_err(|_| FetchError::Network)
}

fn is_macho(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    )
}

fn stage_paths(current: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = current.file_name()?;
    let dir = if app_bundle(current).is_some() {
        cache_dir()?
    } else {
        current.parent()?.to_path_buf()
    };
    let mut retired = name.to_os_string();
    retired.push(".old");
    let mut incoming = name.to_os_string();
    incoming.push(".new");
    Some((dir.join(retired), dir.join(incoming)))
}

fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("ffrm"),
    )
}

fn swap_in(current: &Path, incoming: &Path, retired: &Path) -> bool {
    let _ = fs::remove_file(retired);
    if !move_file(current, retired) {
        return false;
    }
    if !move_file(incoming, current) || make_executable(current).is_err() {
        let _ = move_file(retired, current);
        return false;
    }
    if let Some(app) = app_bundle(current) {
        if resign(&app).is_err() {
            let _ = move_file(current, incoming);
            let _ = move_file(retired, current);
            let _ = make_executable(current);
            return false;
        }
    }
    true
}

fn move_file(from: &Path, to: &Path) -> bool {
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() && fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match fs::rename(from, to) {
        Ok(()) => true,
        // 缓存和安装包不在同一个卷时，改名会失败，改成复制。
        Err(error) if error.raw_os_error() == Some(18) => {
            if fs::copy(from, to).is_err() {
                return false;
            }
            fs::remove_file(from).is_ok()
        }
        Err(_) => false,
    }
}

fn resign(app: &Path) -> Result<(), ()> {
    let status = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ())?;
    if status.success() {
        Ok(())
    } else {
        Err(())
    }
}

fn bundle_is_valid(app: &Path) -> bool {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn relaunch(exe: &Path) -> Result<(), ()> {
    if let Some(app) = app_bundle(exe) {
        // 安装包交给启动服务新开实例。open 打开的是 .app，不会带出终端。
        return Command::new("/usr/bin/open")
            .arg("-n")
            .arg(&app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|_| ());
    }
    // 绿色版是裸可执行文件。open 会交给终端去跑，所以这里直接新开会话启动。
    spawn_detached(exe)
}

fn spawn_detached(exe: &Path) -> Result<(), ()> {
    let mut command = Command::new(exe);
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    unsafe {
        command.pre_exec(|| {
            // 脱离旧进程组。旧窗口退出时不会把新进程一起结束。
            if setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| ())
}

fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
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
    fn newer_tag_is_available() {
        assert_eq!(
            compare("0.1.0", "v0.2.0"),
            Status::Available("0.2.0".to_string())
        );
        assert_eq!(compare("0.1.0", "v0.1.0"), Status::Current);
    }

    #[test]
    fn app_stages_outside_the_bundle() {
        let exe = Path::new("/Applications/ffrm.app/Contents/MacOS/ffrm");
        let (retired, incoming) = stage_paths(exe).unwrap();
        assert!(retired.ends_with("Library/Caches/ffrm/ffrm.old"));
        assert!(incoming.ends_with("Library/Caches/ffrm/ffrm.new"));
        assert!(!retired.starts_with("/Applications/ffrm.app"));
    }

    #[test]
    fn portable_stages_beside_the_executable() {
        let exe = Path::new("/Users/me/Desktop/ffrm");
        let (retired, incoming) = stage_paths(exe).unwrap();
        assert_eq!(retired, Path::new("/Users/me/Desktop/ffrm.old"));
        assert_eq!(incoming, Path::new("/Users/me/Desktop/ffrm.new"));
    }

    #[test]
    fn app_bundle_is_reopened_as_a_bundle() {
        let exe = Path::new("/Applications/ffrm.app/Contents/MacOS/ffrm");
        assert_eq!(
            app_bundle(exe).as_deref(),
            Some(Path::new("/Applications/ffrm.app"))
        );
        assert!(app_bundle(Path::new("/Users/me/Desktop/ffrm")).is_none());
    }

    #[test]
    fn downloaded_file_becomes_executable() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("ffrm-exec-{}", std::process::id()));
        fs::write(&path, b"not a real binary").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        make_executable(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        let _ = fs::remove_file(&path);
        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn release_location_gives_the_macos_binary() {
        let location = "https://github.com/gaoyia/ffrm/releases/tag/v0.2.0";
        assert_eq!(release_tag(location).unwrap(), "v0.2.0");
        assert_eq!(
            release_binary("v0.2.0"),
            "https://github.com/gaoyia/ffrm/releases/download/v0.2.0/ffrm-darwin-arm64"
        );
    }
}
