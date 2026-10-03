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
    WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

const LATEST_RELEASE: &str = "/repos/gaoyia/ffrm/releases/latest";
const JSON_LIMIT: usize = 1_048_576;
const EXE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    Available(String),
    Missing,
    Failed,
}

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
    let body = fetch_latest().map_err(|_| ())?;
    let url = exe_url(&body).ok_or(())?;
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
    let body = fetch_latest()?;
    tag_name(&body)
        .map(str::to_string)
        .ok_or(FetchError::Network)
}

fn fetch_latest() -> Result<String, FetchError> {
    let bytes = http_get(
        "api.github.com",
        LATEST_RELEASE,
        "application/vnd.github+json",
        JSON_LIMIT,
        true,
    )?;
    String::from_utf8(bytes).map_err(|_| FetchError::Network)
}

fn download(url: &str, dest: &Path) -> Result<(), FetchError> {
    let (host, path) = split_https(url).ok_or(FetchError::Network)?;
    let bytes = http_get(&host, &path, "application/octet-stream", EXE_LIMIT, false)?;
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

fn http_get(
    host: &str,
    path: &str,
    accept: &str,
    max: usize,
    missing_is_distinct: bool,
) -> Result<Vec<u8>, FetchError> {
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
        if WinHttpSendRequest(request.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) == 0 {
            return Err(FetchError::Network);
        }
        if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return Err(FetchError::Network);
        }
        let status = status_code(request.0)?;
        if missing_is_distinct && status == 404 {
            return Err(FetchError::Missing);
        }
        if status != 200 {
            return Err(FetchError::Network);
        }
        read_body(request.0, max)
    }
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

fn exe_url(body: &str) -> Option<&str> {
    let at = body
        .find("\"name\": \"ffrm.exe\"")
        .or_else(|| body.find("\"name\":\"ffrm.exe\""))?;
    let window = body.get(at..body.len().min(at + 800))?;
    json_string(window, "browser_download_url")
}

fn tag_name(body: &str) -> Option<&str> {
    json_string(body, "tag_name")
}

fn json_string<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\"");
    let rest = body.get(body.find(&pattern)? + pattern.len()..)?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
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
    fn tag_name_is_the_first_json_string() {
        let body = r#"{"html_url":"https://github.com/gaoyia/ffrm/releases/tag/v0.2.0","tag_name":"v0.2.0","body":"\"tag_name\": \"v9.9.9\""}"#;
        assert_eq!(tag_name(body), Some("v0.2.0"));
    }

    #[test]
    fn download_url_is_the_exe_asset() {
        let body = r#"{"assets":[{"name":"LICENSE","browser_download_url":"https://github.com/gaoyia/ffrm/releases/download/v0.2.0/LICENSE"},{"name":"ffrm.exe","browser_download_url":"https://github.com/gaoyia/ffrm/releases/download/v0.2.0/ffrm.exe"}]}"#;
        assert_eq!(
            exe_url(body),
            Some("https://github.com/gaoyia/ffrm/releases/download/v0.2.0/ffrm.exe")
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
