use crate::{
    constants::APP_ID, settings::load_settings, startup::start_configured_items,
    tunnel_store::load_tunnels, window::CarabinerWindow,
};
use adw::prelude::*;
use gtk::{gio, glib};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
struct AppState {
    activate_in_background: Cell<bool>,
    startup_items_started: Cell<bool>,
    background_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    app.set_accels_for_action("win.show-shortcuts", &["<Control>question"]);
    app.set_accels_for_action("win.quit", &["<Control>q", "<Control>w"]);
    app.set_accels_for_action("win.menu", &["F10"]);
    app.set_accels_for_action("win.add-tunnel", &["<Control>n"]);

    let state = Rc::new(AppState::default());

    {
        let state = state.clone();
        app.connect_command_line(move |app, command_line| {
            let background = command_line
                .arguments()
                .iter()
                .any(|arg| arg.to_string_lossy() == "--background");
            state.activate_in_background.set(background);
            app.activate();
            glib::ExitCode::SUCCESS
        });
    }

    {
        let state = state.clone();
        app.connect_activate(move |app| {
            if state.activate_in_background.get() {
                state.activate_in_background.set(false);
                if state.background_hold.borrow().is_none() {
                    *state.background_hold.borrow_mut() = Some(app.hold());
                }
                start_startup_items_once(&state);
                return;
            }

            if let Some(window) = app.active_window() {
                window.present();
            } else {
                let weak_app = app.downgrade();
                let release_state = state.clone();
                let release_background_hold = Rc::new(move || {
                    let _ = weak_app.upgrade();
                    release_state.background_hold.borrow_mut().take();
                });
                let window = CarabinerWindow::new(app, release_background_hold);
                window.present();
            }
            start_startup_items_once(&state);
        });
    }

    app.run()
}

fn start_startup_items_once(state: &Rc<AppState>) {
    if state.startup_items_started.get() {
        return;
    }
    state.startup_items_started.set(true);

    let settings = load_settings();
    let has_autostart_tunnel = load_tunnels()
        .into_iter()
        .any(|tunnel| tunnel.autostart && tunnel.provider.to_lowercase() != "playit");

    if settings.get_bool("playit_agent_autostart") || has_autostart_tunnel {
        start_configured_items::<fn(usize)>(None);
    }
}
