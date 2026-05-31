use crate::constants::home_dir;
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

pub fn which(name: &str) -> Option<String> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 && is_executable(candidate) {
        return Some(candidate.to_string_lossy().to_string());
    }

    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let full = dir.join(name);
        if is_executable(&full) {
            return Some(full.to_string_lossy().to_string());
        }

        #[cfg(target_os = "windows")]
        {
            let exe = dir.join(format!("{name}.exe"));
            if is_executable(&exe) {
                return Some(exe.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    path.exists() && path.is_file()
}

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
    fn test_user_agent() {
        assert_eq!(user_agent(), "Carabiner/1.0");
    }
}
