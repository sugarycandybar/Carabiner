pub mod cloudflare;
pub mod ngrok;
pub mod playit;

use crate::{events::ManagerEvent, tunnel_store};
use cloudflare::CloudflareManager;
use ngrok::NgrokManager;
use once_cell::sync::Lazy;
use playit::PlayitManager;
use std::sync::Arc;

static SHARED_PLAYIT_MANAGER: Lazy<Arc<PlayitManager>> =
    Lazy::new(|| Arc::new(PlayitManager::new()));

#[derive(Clone)]
pub enum ManagerHandle {
    Playit(Arc<PlayitManager>),
    Cloudflare(Arc<CloudflareManager>),
    Ngrok(Arc<NgrokManager>),
}

impl ManagerHandle {
    pub fn status(&self) -> String {
        match self {
            Self::Playit(manager) => manager.status(),
            Self::Cloudflare(manager) => manager.status(),
            Self::Ngrok(manager) => manager.status(),
        }
    }

    pub fn public_endpoint(&self) -> String {
        match self {
            Self::Playit(manager) => manager.public_endpoint(),
            Self::Cloudflare(manager) => manager.public_endpoint(),
            Self::Ngrok(manager) => manager.public_endpoint(),
        }
    }

    pub fn is_running(&self) -> bool {
        match self {
            Self::Playit(manager) => manager.is_running(),
            Self::Cloudflare(manager) => manager.is_running(),
            Self::Ngrok(manager) => manager.is_running(),
        }
    }

    pub fn is_installed(&self) -> bool {
        match self {
            Self::Playit(manager) => manager.is_installed(),
            Self::Cloudflare(manager) => manager.is_installed(),
            Self::Ngrok(manager) => manager.is_installed(),
        }
    }

    pub fn install_latest_binary(
        &self,
        progress: Option<Box<dyn Fn(u64, u64) + Send + 'static>>,
    ) -> (bool, String) {
        match self {
            Self::Playit(manager) => manager.install_latest_binary(progress),
            Self::Cloudflare(manager) => manager.install_latest_binary(progress),
            Self::Ngrok(manager) => manager.install_latest_binary(progress),
        }
    }

    pub fn start(&self, port: u16, protocol: &str) -> bool {
        match self {
            Self::Playit(manager) => manager.start(port, protocol, "", false, false).0,
            Self::Cloudflare(manager) => manager.start(port, protocol),
            Self::Ngrok(manager) => manager.start(port, protocol),
        }
    }

    pub fn stop(&self) {
        match self {
            Self::Playit(manager) => {
                let _ = manager.stop();
            }
            Self::Cloudflare(manager) => manager.stop(),
            Self::Ngrok(manager) => manager.stop(),
        }
    }

    pub fn connect<F>(&self, signal_name: &'static str, callback: F) -> u64
    where
        F: Fn(ManagerEvent) + Send + Sync + 'static,
    {
        match self {
            Self::Playit(manager) => manager.connect(signal_name, callback),
            Self::Cloudflare(manager) => manager.connect(signal_name, callback),
            Self::Ngrok(manager) => manager.connect(signal_name, callback),
        }
    }

    pub fn disconnect(&self, handler_id: u64) -> bool {
        match self {
            Self::Playit(manager) => manager.disconnect(handler_id),
            Self::Cloudflare(manager) => manager.disconnect(handler_id),
            Self::Ngrok(manager) => manager.disconnect(handler_id),
        }
    }

    pub fn as_playit(&self) -> Option<Arc<PlayitManager>> {
        match self {
            Self::Playit(manager) => Some(manager.clone()),
            _ => None,
        }
    }

    pub fn as_ngrok(&self) -> Option<Arc<NgrokManager>> {
        match self {
            Self::Ngrok(manager) => Some(manager.clone()),
            _ => None,
        }
    }

    pub fn identity_key(&self) -> usize {
        match self {
            Self::Playit(manager) => tunnel_store::manager_ptr(manager),
            Self::Cloudflare(manager) => tunnel_store::manager_ptr(manager),
            Self::Ngrok(manager) => tunnel_store::manager_ptr(manager),
        }
    }
}

pub fn get_shared_playit_manager() -> Arc<PlayitManager> {
    SHARED_PLAYIT_MANAGER.clone()
}

pub fn get_provider_manager(provider: &str) -> ManagerHandle {
    match provider {
        "Playit" => ManagerHandle::Playit(get_shared_playit_manager()),
        "Cloudflare" => ManagerHandle::Cloudflare(Arc::new(CloudflareManager::new())),
        _ => ManagerHandle::Ngrok(Arc::new(NgrokManager::new())),
    }
}

pub fn get_manager_for_tunnel(config: &crate::tunnel_store::TunnelConfig) -> ManagerHandle {
    if config.provider == "Playit" {
        return ManagerHandle::Playit(get_shared_playit_manager());
    }

    if let Some(manager) = tunnel_store::stored_manager(&config.id) {
        return manager;
    }

    let manager = get_provider_manager(&config.provider);
    tunnel_store::remember_manager(&config.id, manager.clone());
    manager
}
