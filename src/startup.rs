use crate::{
    managers::{get_manager_for_tunnel, get_shared_playit_manager},
    portal::set_background_status,
    settings::load_settings,
    tunnel_store::{load_tunnels, managers_snapshot},
    util::t,
};
use std::thread;

fn start_items() -> usize {
    let settings = load_settings();
    let tunnels = load_tunnels();
    let autostart_tunnels = tunnels
        .iter()
        .filter(|tunnel| tunnel.autostart && tunnel.provider.to_lowercase() != "playit")
        .cloned()
        .collect::<Vec<_>>();
    let mut started = 0;

    if settings.get_bool("playit_agent_autostart") {
        let manager = get_shared_playit_manager();
        let (ok, _msg) = manager.start_agent(None);
        if ok {
            started += 1;
        }
    }

    for config in autostart_tunnels {
        let manager = get_manager_for_tunnel(&config);
        if manager.as_ngrok().is_some() {
            for other in managers_snapshot() {
                if other.as_ngrok().is_some()
                    && other.identity_key() != manager.identity_key()
                    && other.is_running()
                {
                    other.stop();
                }
            }
        }

        if manager
            .start(config.port, &config.protocol.to_lowercase())
            .0
        {
            started += 1;
        }
    }

    started
}

pub fn start_configured_items<F>(callback: Option<F>)
where
    F: FnOnce(usize) + Send + 'static,
{
    thread::spawn(move || {
        let started = start_items();
        if started > 0 {
            set_background_status(&format!("{} {}", started, t("tunnels running")));
        }

        if let Some(callback) = callback {
            gtk::glib::MainContext::default().invoke(move || callback(started));
        }
    });
}
