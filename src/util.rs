use crate::constants::home_dir;
use std::{
    env,
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

pub fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
}

pub fn home() -> PathBuf {
    home_dir()
}
