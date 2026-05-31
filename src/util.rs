use crate::constants::home_dir;
use sha2::{Digest, Sha256};
use std::{
    env,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

pub fn platform_name() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else {
        "linux".to_string()
    }
}

pub fn playit_platform_name() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else {
        "linux".to_string()
    }
}

pub fn machine_name() -> String {
    env::consts::ARCH.to_lowercase()
}

pub fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        match unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } {
            0 => {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..len]).to_string()
            }
            _ => "localhost".to_string(),
        }
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string())
    }
}

pub fn command_no_window(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = command;
    }
}

pub fn terminate_child(child: &mut Child, timeout: Duration) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(child.id() as i32, libc::SIGTERM);
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

pub fn user_agent() -> &'static str {
    "Carabiner/1.0"
}

pub fn download_with_progress(
    url: &str,
    timeout_secs: u64,
    progress: impl Fn(u64, u64),
) -> Result<Vec<u8>, String> {
    let mut last_error = String::new();
    let max_retries = 3;

    for attempt in 0..max_retries {
        if attempt > 0 {
            thread::sleep(Duration::from_secs(1 << attempt));
        }

        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                last_error = friendly_download_error(&e.to_string());
                continue;
            }
        };

        let response = match client
            .get(url)
            .header(reqwest::header::USER_AGENT, user_agent())
            .send()
            .and_then(|r| r.error_for_status())
        {
            Ok(r) => r,
            Err(e) => {
                last_error = friendly_download_error(&e.to_string());
                continue;
            }
        };

        let total = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut buf = Vec::with_capacity(total as usize);

        let mut reader = response.take(200 * 1024 * 1024);
        let mut chunk = [0u8; 16384];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    downloaded += n as u64;
                    progress(downloaded, total);
                }
                Err(e) => {
                    last_error = friendly_download_error(&e.to_string());
                    break;
                }
            }
        }

        if last_error.is_empty() {
            return Ok(buf);
        }
    }

    Err(format!(
        "Download failed after {max_retries} attempts: {last_error}"
    ))
}

pub fn friendly_download_error(msg: &str) -> String {
    if msg.contains("timed out") || msg.contains("timeout") {
        "Connection timed out. Please check your internet connection and try again.".to_string()
    } else if msg.contains("404 ") || msg.contains("Not Found") {
        "Download file not found. The release may have been removed.".to_string()
    } else if msg.contains("403 ") || msg.contains("Forbidden") {
        "Access denied. The download server rejected the request.".to_string()
    } else if msg.contains("connection refused") || msg.contains("Connection refused") {
        "Could not connect to the download server. It may be temporarily unavailable.".to_string()
    } else if msg.to_lowercase().contains("dns") || msg.to_lowercase().contains("resolve") {
        "Could not resolve the download server address. Please check your DNS settings.".to_string()
    } else if msg.to_lowercase().contains("tls")
        || msg.to_lowercase().contains("ssl")
        || msg.to_lowercase().contains("certificate")
    {
        "A security error occurred during the download. Your connection may be intercepted."
            .to_string()
    } else if msg.contains("cancelled") {
        "Download was cancelled.".to_string()
    } else {
        format!("An unexpected error occurred: {msg}")
    }
}

pub fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
}

pub fn home() -> PathBuf {
    home_dir()
}

pub fn verify_sha256(data: &[u8], expected_hex: &str) -> Result<(), String> {
    let expected = expected_hex.trim().to_lowercase();
    if expected.is_empty() {
        return Err("empty hash".to_string());
    }
    let actual: String = Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch: expected {expected}, got {actual}"
        ))
    }
}

pub fn fetch_sha256(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let text = client
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("failed to fetch checksum: {e}"))?;

    Ok(text)
}

pub fn parse_sha256_from_output(output: &str, filename: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let hash = parts[0];
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let candidate = parts.get(1).copied().unwrap_or("");
        let candidate = candidate.trim_start_matches('*').trim_start_matches('(');
        let candidate = candidate.trim_end_matches(')');
        if candidate == filename || candidate.ends_with(&format!("/{filename}")) {
            return Some(hash.to_string());
        }
    }
    output.lines().next().map(|l| {
        l.split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    })
}

pub fn save_binary_hash(bin_path: &Path, data: &[u8]) -> Result<(), String> {
    let hash: String = Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let hash_path = hash_path(bin_path);
    fs::write(&hash_path, &hash).map_err(|e| format!("Failed to write hash file: {e}"))
}

pub fn check_binary_integrity(bin_path: &Path) -> Result<(), String> {
    let hash_path = hash_path(bin_path);

    if !hash_path.exists() {
        let data = fs::read(bin_path).map_err(|e| format!("Cannot read binary: {e}"))?;
        return save_binary_hash(bin_path, &data);
    }

    let expected = fs::read_to_string(&hash_path)
        .map_err(|_| "integrity file not found".to_string())?
        .trim()
        .to_string();
    if expected.is_empty() {
        return Err("integrity file is empty".to_string());
    }
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("integrity file has invalid format".to_string());
    }
    let data = fs::read(bin_path).map_err(|e| format!("Cannot read binary: {e}"))?;
    let actual: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "Binary hash mismatch (expected {expected}, got {actual})"
        ))
    }
}

fn hash_path(bin_path: &Path) -> PathBuf {
    let mut p = bin_path.to_path_buf();
    let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    p.set_file_name(format!("{name}.sha256"));
    p
}

#[cfg(unix)]
pub fn disable_setuid_on_child(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn disable_setuid_on_child(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_name() {
        let name = platform_name();
        assert!(!name.is_empty());
        #[cfg(target_os = "windows")]
        assert_eq!(name, "windows");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "darwin");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "linux");
    }

    #[test]
    fn test_playit_platform_name() {
        let name = playit_platform_name();
        assert!(!name.is_empty());
        #[cfg(target_os = "windows")]
        assert_eq!(name, "windows");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "macos");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "linux");
    }

    #[test]
    fn test_machine_name() {
        let name = machine_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn test_hostname() {
        let name = hostname();
        assert!(!name.is_empty());
        assert_ne!(name, "localhost");
    }

    #[test]
    fn test_user_agent() {
        assert_eq!(user_agent(), "Carabiner/1.0");
    }
}
