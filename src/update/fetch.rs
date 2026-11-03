//! 用 ureq 检查发布页并流式下载安装包。两个系统共用，不依赖系统自带的下载工具。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use ureq::Agent;

const RELEASES_LATEST: &str = "https://github.com/gaoyia/ffrm/releases/latest";
const FILE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum FetchError {
    Missing,
    Network,
}

pub fn latest_location() -> Result<String, FetchError> {
    let response = agent(0, Duration::from_secs(20))
        .get(RELEASES_LATEST)
        .header("user-agent", "ffrm")
        .call()
        .map_err(classify)?;
    location_of(response)
}

pub fn download(
    url: &str,
    dest: &Path,
    report: &mut dyn FnMut(Option<u8>),
) -> Result<(), FetchError> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|_| FetchError::Network)?;
        }
    }
    let response = agent(10, Duration::from_secs(120))
        .get(url)
        .header("user-agent", "ffrm")
        .call()
        .map_err(classify)?;
    if response.status().as_u16() == 404 {
        return Err(FetchError::Missing);
    }
    if !response.status().is_success() {
        return Err(FetchError::Network);
    }
    let total = response.body().content_length().filter(|len| *len > 0);
    if let Some(total) = total {
        if total > FILE_LIMIT {
            return Err(FetchError::Network);
        }
    }
    let mut reader = response
        .into_body()
        .into_with_config()
        .limit(FILE_LIMIT)
        .reader();
    let mut file = File::create(dest).map_err(|_| FetchError::Network)?;
    copy_with_progress(&mut reader, &mut file, total, report)?;
    file.flush().map_err(|_| FetchError::Network)?;
    Ok(())
}

fn agent(redirects: u32, timeout: Duration) -> Agent {
    let config = Agent::config_builder()
        .max_redirects(redirects)
        .timeout_global(Some(timeout))
        .build();
    Agent::new_with_config(config)
}

fn classify(error: ureq::Error) -> FetchError {
    match error {
        ureq::Error::StatusCode(404) => FetchError::Missing,
        _ => FetchError::Network,
    }
}

fn location_of(response: ureq::http::Response<ureq::Body>) -> Result<String, FetchError> {
    if response.status().as_u16() == 404 {
        return Err(FetchError::Missing);
    }
    if !response.status().is_redirection() {
        return Err(FetchError::Network);
    }
    response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .map(ToString::to_string)
        .ok_or(FetchError::Network)
}

fn copy_with_progress(
    reader: &mut impl Read,
    file: &mut File,
    total: Option<u64>,
    report: &mut dyn FnMut(Option<u8>),
) -> Result<(), FetchError> {
    let mut received = 0u64;
    let mut reported = None;
    let mut emit = |percent: Option<u8>| {
        if reported != Some(percent) {
            reported = Some(percent);
            report(percent);
        }
    };
    if total.is_none() {
        emit(None);
    } else {
        emit(Some(0));
    }
    let mut buf = [0u8; 8192];
    loop {
        let read = reader.read(&mut buf).map_err(|_| FetchError::Network)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|_| FetchError::Network)?;
        received += read as u64;
        if received > FILE_LIMIT {
            return Err(FetchError::Network);
        }
        if let Some(total) = total {
            emit(Some(download_percent(received, total)));
        }
    }
    if received < 64 {
        return Err(FetchError::Network);
    }
    if total.is_some() {
        emit(Some(100));
    }
    Ok(())
}

fn download_percent(received: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    (received.saturating_mul(100) / total).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn download_percent_follows_the_received_share() {
        assert_eq!(download_percent(0, 200), 0);
        assert_eq!(download_percent(50, 200), 25);
        assert_eq!(download_percent(200, 200), 100);
        assert_eq!(download_percent(250, 200), 100);
        assert_eq!(download_percent(1, 0), 0);
    }

    #[test]
    fn redirect_location_is_read_without_following_it() {
        let body = b"HTTP/1.1 302 Found\r\nLocation: https://github.com/gaoyia/ffrm/releases/tag/v0.2.0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let url = serve(body);
        let response = agent(0, Duration::from_secs(5)).get(&url).call().unwrap();
        assert_eq!(
            location_of(response).unwrap(),
            "https://github.com/gaoyia/ffrm/releases/tag/v0.2.0"
        );
    }

    #[test]
    fn stream_reports_percent_while_the_body_arrives() {
        let payload = vec![b'x'; 64 * 1024];
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let mut response = header.into_bytes();
        response.extend_from_slice(&payload);
        let url = serve(&response);
        let dest = std::env::temp_dir().join(format!("ffrm-fetch-{}", std::process::id()));
        let _ = fs::remove_file(&dest);
        let mut percents = Vec::new();
        download(&url, &dest, &mut |percent| percents.push(percent)).unwrap();
        let saved = fs::read(&dest).unwrap();
        let _ = fs::remove_file(&dest);
        assert_eq!(saved, payload);
        assert_eq!(percents.first().copied(), Some(Some(0)));
        assert_eq!(percents.last().copied(), Some(Some(100)));
        assert!(percents
            .iter()
            .flatten()
            .any(|percent| (1..100).contains(percent)));
        let mut previous = 0u8;
        for percent in percents.iter().flatten() {
            assert!(*percent >= previous);
            previous = *percent;
        }
    }

    fn serve(response: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.to_vec();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut pending = Vec::new();
            let mut byte = [0u8; 1];
            while !pending.windows(4).any(|window| window == b"\r\n\r\n") {
                if stream.read(&mut byte).ok() != Some(1) {
                    return;
                }
                pending.push(byte[0]);
                if pending.len() > 8192 {
                    return;
                }
            }
            let _ = stream.write_all(&response);
        });
        format!("http://{address}/file")
    }
}
