use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
    WINHTTP_QUERY_STATUS_CODE,
};

const RELEASES_LATEST: &str = "/gaoyia/ffrm/releases/latest";
const EXE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    Available(String),
    Missing,
    Failed,
}

#[derive(Debug)]
enum FetchError {
    Missing,
    Network,
}

struct Session(*mut c_void);

impl Drop for Session {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub fn check(current: &str) -> Status {
    match fetch_tag() {
        Ok(tag) => compare(current, &tag),
        Err(FetchError::Missing) => Status::Missing,
        Err(FetchError::Network) => Status::Failed,
    }
}

pub fn install() -> Result<(), ()> {
    let current = std::env::current_exe().map_err(|_| ())?;
    let (retired, incoming) = stage_paths(&current).ok_or(())?;
    let tag = fetch_tag().map_err(|_| ())?;
    let url = release_exe(&tag);
    if download(&url, &incoming).is_err() {
        let _ = fs::remove_file(&incoming);
        return Err(());
    }
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
    release_tag(&latest_location()?)
}

fn release_exe(tag: &str) -> String {
    format!("https://github.com/gaoyia/ffrm/releases/download/{tag}/ffrm.exe")
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

fn latest_location() -> Result<String, FetchError> {
    let exchange = open_request("github.com", RELEASES_LATEST, "*/*", false)?;
    unsafe {
        if WinHttpSendRequest(
            exchange.request.0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            0,
            0,
        ) == 0
        {
            return Err(FetchError::Network);
        }
        if WinHttpReceiveResponse(exchange.request.0, std::ptr::null_mut()) == 0 {
            return Err(FetchError::Network);
        }
        let status = status_code(exchange.request.0)?;
        if status == 404 {
            return Err(FetchError::Missing);
        }
        if !(300..400).contains(&status) {
            return Err(FetchError::Network);
        }
        query_header(exchange.request.0, WINHTTP_QUERY_LOCATION)
    }
}

fn download(url: &str, dest: &Path) -> Result<(), FetchError> {
    let (host, path) = split_https(url).ok_or(FetchError::Network)?;
    let bytes = http_get(&host, &path, "application/octet-stream", EXE_LIMIT)?;
    if bytes.len() < 64 || bytes.first_chunk::<2>() != Some(b"MZ") {
        return Err(FetchError::Network);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|_| FetchError::Network)?;
    }
    let mut file = fs::File::create(dest).map_err(|_| FetchError::Network)?;
    file.write_all(&bytes).map_err(|_| FetchError::Network)?;
    Ok(())
}

struct Exchange {
    _session: Session,
    _connection: Session,
    request: Session,
}

fn open_request(host: &str, path: &str, accept: &str, follow: bool) -> Result<Exchange, FetchError> {
    unsafe {
        let agent = wide("ffrm");
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err(FetchError::Network);
        }
        let session = Session(session);
        if WinHttpSetTimeouts(session.0, 8000, 8000, 30000, 120000) == 0 {
            return Err(FetchError::Network);
        }
        let protocols = WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
        if WinHttpSetOption(
            session.0 as *const c_void,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &protocols as *const u32 as *const c_void,
            std::mem::size_of_val(&protocols) as u32,
        ) == 0
        {
            return Err(FetchError::Network);
        }

        let host = wide(host);
        let connection = WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0);
        if connection.is_null() {
            return Err(FetchError::Network);
        }
        let connection = Session(connection);
        let method = wide("GET");
        let path = wide(path);
        let request = WinHttpOpenRequest(
            connection.0,
            method.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            return Err(FetchError::Network);
        }
        let request = Session(request);
        if !follow {
            let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            if WinHttpSetOption(
                request.0 as *const c_void,
                WINHTTP_OPTION_REDIRECT_POLICY,
                &never as *const u32 as *const c_void,
                std::mem::size_of_val(&never) as u32,
            ) == 0
            {
                return Err(FetchError::Network);
            }
        }
        let headers = wide(&format!("User-Agent: ffrm\r\nAccept: {accept}\r\n"));
        if WinHttpAddRequestHeaders(
            request.0,
            headers.as_ptr(),
            u32::MAX,
            WINHTTP_ADDREQ_FLAG_ADD,
        ) == 0
        {
            return Err(FetchError::Network);
        }
        Ok(Exchange {
            _session: session,
            _connection: connection,
            request,
        })
    }
}

fn http_get(host: &str, path: &str, accept: &str, max: usize) -> Result<Vec<u8>, FetchError> {
    let exchange = open_request(host, path, accept, true)?;
    unsafe {
        if WinHttpSendRequest(
            exchange.request.0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            0,
            0,
        ) == 0
        {
            return Err(FetchError::Network);
        }
        if WinHttpReceiveResponse(exchange.request.0, std::ptr::null_mut()) == 0 {
            return Err(FetchError::Network);
        }
        let status = status_code(exchange.request.0)?;
        if status != 200 {
            return Err(FetchError::Network);
        }
        read_body(exchange.request.0, max)
    }
}

fn query_header(request: *mut c_void, level: u32) -> Result<String, FetchError> {
    let mut bytes = 0u32;
    unsafe {
        WinHttpQueryHeaders(
            request,
            level,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut bytes,
            std::ptr::null_mut(),
        );
    }
    if bytes < 2 {
        return Err(FetchError::Network);
    }
    let mut units = vec![0u16; bytes as usize / 2];
    let ok = unsafe {
        WinHttpQueryHeaders(
            request,
            level,
            std::ptr::null(),
            units.as_mut_ptr() as *mut c_void,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(FetchError::Network);
    }
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    Ok(String::from_utf16_lossy(&units[..end]))
}

fn status_code(request: *mut c_void) -> Result<u32, FetchError> {
    let mut status = 0u32;
    let mut length = std::mem::size_of::<u32>() as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut length,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(FetchError::Network)
    } else {
        Ok(status)
    }
}

fn read_body(request: *mut c_void, max: usize) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    loop {
        let mut buffer = [0u8; 8192];
        let mut read = 0u32;
        let ok = unsafe {
            WinHttpReadData(
                request,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut read,
            )
        };
        if ok == 0 {
            return Err(FetchError::Network);
        }
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read as usize]);
        if body.len() > max {
            return Err(FetchError::Network);
        }
    }
    Ok(body)
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
    command.spawn().map(|_| ()).map_err(|_| ())
}

fn split_https(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), rest[slash..].to_string()))
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

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
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
            "https://github.com/gaoyia/ffrm/releases/download/v0.2.0/ffrm.exe"
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
