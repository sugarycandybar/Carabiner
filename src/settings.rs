use crate::constants::DATA_DIR;
use serde_json::{Map, Value};
use std::{fs, path::PathBuf};

fn settings_file() -> PathBuf {
    DATA_DIR.join("settings.json")
}

fn default_map() -> Map<String, Value> {
    Map::from_iter([
        ("playit_token".to_string(), Value::String(String::new())),
        ("ngrok_token".to_string(), Value::String(String::new())),
        ("run_in_background".to_string(), Value::Bool(false)),
        ("start_on_login".to_string(), Value::Bool(false)),
        ("playit_agent_autostart".to_string(), Value::Bool(false)),
    ])
}

#[derive(Clone, Debug)]
pub struct Settings {
    values: Map<String, Value>,
}

impl Settings {
    pub fn get_string(&self, key: &str) -> String {
        self.values
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.values
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn set_string(&mut self, key: &str, value: impl Into<String>) {
        self.values
            .insert(key.to_string(), Value::String(value.into()));
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.values.insert(key.to_string(), Value::Bool(value));
    }

    pub fn save(&self) {
        save_settings(self);
    }
}

pub fn load_settings() -> Settings {
    let mut values = default_map();
    let path = settings_file();

    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(Value::Object(loaded)) = serde_json::from_str::<Value>(&text) {
            for (key, value) in loaded {
                values.insert(key, value);
            }
        }
    }

    Settings { values }
}

pub fn save_settings(settings: &Settings) {
    let mut values = default_map();
    for (key, value) in &settings.values {
        values.insert(key.clone(), value.clone());
    }

    let path = settings_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&Value::Object(values)) {
        let _ = fs::write(path, format!("{text}\n"));
    }
}
