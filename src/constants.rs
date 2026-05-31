use once_cell::sync::Lazy;
use std::{env, fs, path::PathBuf};

pub const APP_ID: &str = "io.github.sugarycandybar.Carabiner";
pub const APP_NAME: &str = "Carabiner";
pub const APP_VERSION: &str = crate::config::VERSION;
pub const APP_WEBSITE: &str = "https://github.com/sugarycandybar/Carabiner";

pub static DATA_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let path = env::var_os("CARABINER_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let _ = fs::create_dir_all(&path);
    path
});

fn default_data_dir() -> PathBuf {
    if let Some(xdg_data) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg_data).join("carabiner");
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("Carabiner");
        }
        return home_dir().join("AppData").join("Local").join("Carabiner");
    }

    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .join("Library")
            .join("Application Support")
            .join("Carabiner");
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        home_dir().join(".local").join("share").join("carabiner")
    }
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
