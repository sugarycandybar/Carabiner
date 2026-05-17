use crate::{
    constants::DATA_DIR,
    managers::{ManagerHandle, get_shared_playit_manager},
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use uuid::Uuid;

fn tunnels_file() -> PathBuf {
    DATA_DIR.join("tunnels.json")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TunnelConfig {
    pub id: String,
    pub provider: String,
    pub protocol: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub autostart: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub public_url: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub static MANAGER_REGISTRY: Lazy<Mutex<HashMap<String, ManagerHandle>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn load_tunnels() -> Vec<TunnelConfig> {
    let path = tunnels_file();
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<TunnelConfig>>(&text).unwrap_or_default()
}

pub fn save_tunnels(tunnels: &[TunnelConfig]) {
    let path = tunnels_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(tunnels) {
        let _ = fs::write(path, format!("{text}\n"));
    }
}

pub fn add_tunnel(provider: &str, protocol: &str, port: u16, label: &str) -> String {
    let mut tunnels = load_tunnels();
    let id = Uuid::new_v4().to_string();
    tunnels.push(TunnelConfig {
        id: id.clone(),
        provider: provider.to_string(),
        protocol: protocol.to_string(),
        port,
        label: label.to_string(),
        autostart: false,
        public_url: String::new(),
        extra: HashMap::new(),
    });
    save_tunnels(&tunnels);
    id
}

pub fn update_tunnel_url(tunnel_id: &str, url: &str) {
    update_tunnel(tunnel_id, |tunnel| tunnel.public_url = url.to_string());
}

pub fn update_tunnel_label(tunnel_id: &str, label: &str) {
    update_tunnel(tunnel_id, |tunnel| tunnel.label = label.to_string());
}

pub fn update_tunnel_autostart(tunnel_id: &str, autostart: bool) {
    update_tunnel(tunnel_id, |tunnel| tunnel.autostart = autostart);
}

fn update_tunnel<F>(tunnel_id: &str, update: F)
where
    F: FnOnce(&mut TunnelConfig),
{
    let mut tunnels = load_tunnels();
    if let Some(tunnel) = tunnels.iter_mut().find(|tunnel| tunnel.id == tunnel_id) {
        update(tunnel);
        save_tunnels(&tunnels);
    }
}

pub fn remove_tunnel(tunnel_id: &str) {
    let mut tunnels = load_tunnels();
    let Some(config) = tunnels
        .iter()
        .find(|tunnel| tunnel.id == tunnel_id)
        .cloned()
    else {
        return;
    };

    tunnels.retain(|tunnel| tunnel.id != tunnel_id);
    save_tunnels(&tunnels);

    let manager = MANAGER_REGISTRY.lock().unwrap().remove(tunnel_id);
    if config.provider == "Playit" {
        let playit = manager
            .and_then(|manager| manager.as_playit())
            .unwrap_or_else(get_shared_playit_manager);
        thread::spawn(move || {
            if !playit.initialized() {
                let _ = playit.initialize();
            }
            if playit.initialized() {
                let _ = playit.delete_tunnels(config.port, &config.protocol.to_lowercase());
            }
        });
    } else if let Some(manager) = manager {
        if manager.is_running() {
            manager.stop();
        }
    }
}

pub fn stop_all_tunnels() {
    let managers = {
        let mut registry = MANAGER_REGISTRY.lock().unwrap();
        let values = registry.values().cloned().collect::<Vec<_>>();
        registry.clear();
        values
    };

    let mut seen: Vec<usize> = Vec::new();
    for manager in managers {
        let key = manager.identity_key();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if manager.is_running() {
            manager.stop();
        }
    }

    let playit = get_shared_playit_manager();
    if playit.is_running() {
        let _ = playit.stop();
    }
}

pub fn remember_manager(tunnel_id: &str, manager: ManagerHandle) {
    MANAGER_REGISTRY
        .lock()
        .unwrap()
        .insert(tunnel_id.to_string(), manager);
}

pub fn stored_manager(tunnel_id: &str) -> Option<ManagerHandle> {
    MANAGER_REGISTRY.lock().unwrap().get(tunnel_id).cloned()
}

pub fn managers_snapshot() -> Vec<ManagerHandle> {
    MANAGER_REGISTRY.lock().unwrap().values().cloned().collect()
}

pub fn manager_ptr<T>(arc: &Arc<T>) -> usize {
    Arc::as_ptr(arc) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn setup() {
        INIT.call_once(|| {
            let temp_dir = std::env::temp_dir().join(format!("carabiner_test_{}", uuid::Uuid::new_v4()));
            unsafe {
                std::env::set_var("CARABINER_DATA_DIR", &temp_dir);
            }
            // Ensure DATA_DIR is initialized now with the custom path
            let _ = &*crate::constants::DATA_DIR;
        });
    }

    #[test]
    fn test_add_load_remove_tunnel() {
        setup();
        
        let provider = "Ngrok";
        let protocol = "TCP";
        let port = 8080;
        let label = "My Test Tunnel";

        let id = add_tunnel(provider, protocol, port, label);
        assert!(!id.is_empty());

        let tunnels = load_tunnels();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].id, id);
        assert_eq!(tunnels[0].provider, provider);
        assert_eq!(tunnels[0].protocol, protocol);
        assert_eq!(tunnels[0].port, port);
        assert_eq!(tunnels[0].label, label);

        // Test update url
        update_tunnel_url(&id, "tcp://ngrok.com:12345");
        let tunnels = load_tunnels();
        assert_eq!(tunnels[0].public_url, "tcp://ngrok.com:12345");

        // Test update label
        update_tunnel_label(&id, "Updated Label");
        let tunnels = load_tunnels();
        assert_eq!(tunnels[0].label, "Updated Label");

        // Test update autostart
        update_tunnel_autostart(&id, true);
        let tunnels = load_tunnels();
        assert!(tunnels[0].autostart);

        // Clean up / remove
        remove_tunnel(&id);
        let tunnels = load_tunnels();
        assert!(tunnels.is_empty());
    }
}

