#![allow(deprecated)]

use crate::{
    constants::{APP_ID, APP_NAME, APP_VERSION, APP_WEBSITE},
    events::ManagerEvent,
    managers::{
        ManagerHandle, get_manager_for_tunnel, get_provider_manager, get_shared_playit_manager,
        ngrok::NgrokManager, playit::PlayitManager,
    },
    portal::{request_background, set_background_status},
    settings::{load_settings, save_settings},
    tunnel_store::{
        TunnelConfig, add_tunnel, load_tunnels, managers_snapshot, remove_tunnel, stop_all_tunnels,
        update_tunnel_autostart, update_tunnel_label, update_tunnel_url,
    },
};
use adw::prelude::*;
use crossbeam_channel::{Receiver, unbounded};
use gtk::{gio, glib};
use std::{cell::RefCell, rc::Rc, sync::Arc, thread, time::Duration};

fn set_switch_active(switch: &gtk::Switch, active: bool) {
    switch.set_active(active);
    switch.set_state(active);
}

fn add_toast(overlay: &adw::ToastOverlay, text: &str) {
    overlay.add_toast(adw::Toast::new(text));
}

fn show_message(parent: Option<&gtk::Window>, title: &str, body: &str) {
    let dialog = adw::MessageDialog::builder()
        .heading(title)
        .body(body)
        .body_use_markup(true)
        .build();
    dialog.add_response("ok", "Close");
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.connect_response(Some("ok"), |dialog, _| dialog.close());
    dialog.present();
}

fn show_error_for_widget(widget: &impl IsA<gtk::Widget>, title: &str, body: &str) {
    let parent = widget
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    show_message(parent.as_ref(), title, body);
}

fn drain_receiver<T: 'static, F: FnMut(T) + 'static>(receiver: Receiver<T>, mut on_msg: F) {
    glib::timeout_add_local(Duration::from_millis(100), move || {
        while let Ok(msg) = receiver.try_recv() {
            on_msg(msg);
        }
        glib::ControlFlow::Continue
    });
}

#[derive(Clone)]
pub struct CarabinerWindow {
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    toolbar_view: adw::ToolbarView,
    quit_requested: Rc<RefCell<bool>>,
    release_background_hold: Rc<dyn Fn()>,
}

impl CarabinerWindow {
    pub fn new(app: &adw::Application, release_background_hold: Rc<dyn Fn()>) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(420)
            .default_height(560)
            .title("Carabiner")
            .build();

        let toast_overlay = adw::ToastOverlay::new();
        window.set_content(Some(&toast_overlay));

        let toolbar_view = adw::ToolbarView::new();
        toast_overlay.set_child(Some(&toolbar_view));

        let header = adw::HeaderBar::new();

        let add_btn = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Tunnel")
            .build();
        header.pack_start(&add_btn);

        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_btn);

        let menu = gio::Menu::new();
        menu.append(Some("Preferences"), Some("win.preferences"));
        menu.append(Some("Keyboard Shortcuts"), Some("win.show-shortcuts"));
        menu.append(Some("About Carabiner"), Some("win.about"));
        menu_btn.set_menu_model(Some(&menu));

        let menu_action = gio::SimpleAction::new("menu", None);
        {
            let menu_btn = menu_btn.clone();
            menu_action.connect_activate(move |_, _| {
                menu_btn.popup();
            });
        }

        toolbar_view.add_top_bar(&header);

        let this = Self {
            window,
            toast_overlay,
            toolbar_view,
            quit_requested: Rc::new(RefCell::new(false)),
            release_background_hold,
        };

        {
            let this = this.clone();
            add_btn.connect_clicked(move |_| this.show_setup_dialog());
        }

        let preferences_action = gio::SimpleAction::new("preferences", None);
        {
            let this = this.clone();
            preferences_action
                .connect_activate(move |_, _| PreferencesDialog::new().present(&this.window));
        }
        this.window.add_action(&preferences_action);

        let add_tunnel_action = gio::SimpleAction::new("add-tunnel", None);
        {
            let this = this.clone();
            add_tunnel_action.connect_activate(move |_, _| this.show_setup_dialog());
        }
        this.window.add_action(&add_tunnel_action);

        this.window.add_action(&menu_action);

        let about_action = gio::SimpleAction::new("about", None);
        {
            let this = this.clone();
            about_action.connect_activate(move |_, _| this.show_about());
        }
        this.window.add_action(&about_action);

        let show_shortcuts_action = gio::SimpleAction::new("show-shortcuts", None);
        {
            let window = this.window.clone();
            show_shortcuts_action.connect_activate(move |_, _| {
                let builder = gtk::Builder::from_resource(
                    "/io/github/sugarycandybar/Carabiner/shortcuts-dialog.ui",
                );
                let shortcuts: adw::ShortcutsDialog = builder
                    .object("shortcuts_dialog")
                    .expect("shortcuts_dialog not found");
                shortcuts.present(Some(&window));
            });
        }
        this.window.add_action(&show_shortcuts_action);

        let quit_action = gio::SimpleAction::new("quit", None);
        {
            let this = this.clone();
            quit_action.connect_activate(move |_, _| {
                *this.quit_requested.borrow_mut() = true;
                this.window.close();
            });
        }
        this.window.add_action(&quit_action);

        {
            let this = this.clone();
            this.window
                .clone()
                .connect_close_request(move |_| this.on_close_request().into());
        }

        this.refresh_ui(None);
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn add_toast(&self, text: &str) {
        add_toast(&self.toast_overlay, text);
    }

    fn on_close_request(&self) -> bool {
        if load_settings().get_bool("run_in_background") && !*self.quit_requested.borrow() {
            self.window.set_visible(false);
            set_background_status("Carabiner running");
            return true;
        }

        (self.release_background_hold)();
        stop_all_tunnels();
        false
    }

    fn show_setup_dialog(&self) {
        let dialog = SetupDialog::new({
            let this = self.clone();
            Rc::new(move |toast| this.refresh_ui(toast))
        });
        dialog.present(&self.window);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(APP_NAME)
            .application_icon(APP_ID)
            .developer_name("Sugarycandybar")
            .version(APP_VERSION)
            .comments("Create and manage secure network tunnels.")
            .issue_url(format!("{APP_WEBSITE}/issues"))
            .license_type(gtk::License::Gpl30)
            .build();

        about.add_link("Website", APP_WEBSITE);

        about.add_acknowledgement_section(
            Some("Tunnel Services"),
            &[
                "playit https://playit.gg",
                "ngrok https://ngrok.com",
                "Cloudflare https://www.cloudflare.com",
            ],
        );

        about.add_other_app(
            "io.github.sugarycandybar.Hosty",
            "Hosty",
            "Host Minecraft servers",
        );

        about.add_other_app(
            "io.github.sugarycandybar.Crucible",
            "Crucible",
            "View specs and stress test hardware",
        );

        about.present(Some(&self.window));
    }

    fn refresh_ui(&self, toast_msg: Option<String>) {
        if let Some(toast_msg) = toast_msg {
            self.add_toast(&toast_msg);
        }

        let tunnels = load_tunnels();
        if tunnels.is_empty() {
            let status_page = adw::StatusPage::new();
            status_page.set_title("No Tunnels");
            status_page.set_description(Some(
                "Create a network tunnel to securely share local ports.",
            ));
            status_page.set_icon_name(Some("network-server-symbolic"));

            let btn = gtk::Button::with_label("Add Tunnel");
            btn.add_css_class("suggested-action");
            btn.add_css_class("pill");
            btn.set_halign(gtk::Align::Center);
            {
                let this = self.clone();
                btn.connect_clicked(move |_| this.show_setup_dialog());
            }

            let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
            box_.append(&status_page);
            box_.append(&btn);
            box_.set_valign(gtk::Align::Center);
            self.toolbar_view.set_content(Some(&box_));
            return;
        }

        let page = adw::PreferencesPage::new();
        let mut providers = tunnels
            .iter()
            .map(|tunnel| tunnel.provider.clone())
            .collect::<Vec<_>>();
        providers.sort();
        providers.dedup();

        for provider in providers {
            let group = adw::PreferencesGroup::new();
            group.set_title(&provider);
            page.add(&group);

            if provider == "Playit" {
                group.add(&PlayitAgentRow::new(&self.toast_overlay).row);
            }

            for config in tunnels.iter().filter(|tunnel| tunnel.provider == provider) {
                let row = TunnelRow::new(config.clone(), {
                    let this = self.clone();
                    Rc::new(move |toast| this.refresh_ui(toast))
                });
                group.add(&row.row);
            }
        }

        self.toolbar_view.set_content(Some(&page));
    }
}

struct PreferencesDialog {
    dialog: adw::Dialog,
}

impl PreferencesDialog {
    fn new() -> Self {
        let dialog = adw::Dialog::builder()
            .title("Preferences")
            .content_width(400)
            .build();
        let settings = Rc::new(RefCell::new(load_settings()));
        let updating_startup_switches = Rc::new(RefCell::new(false));

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Preferences", "")));
        toolbar_view.add_top_bar(&header);

        let page = adw::PreferencesPage::new();

        let group = adw::PreferencesGroup::new();
        group.set_title("Tunnel Tokens");
        page.add(&group);

        let playit_row = adw::EntryRow::new();
        playit_row.set_title("Playit Token");
        playit_row.set_text(&settings.borrow().get_string("playit_token"));
        playit_row.set_show_apply_button(true);
        {
            let settings = settings.clone();
            playit_row.connect_apply(move |row| {
                settings
                    .borrow_mut()
                    .set_string("playit_token", row.text().as_str());
                settings.borrow().save();
            });
        }
        group.add(&playit_row);

        let ngrok_row = adw::EntryRow::new();
        ngrok_row.set_title("Ngrok Token");
        ngrok_row.set_text(&settings.borrow().get_string("ngrok_token"));
        ngrok_row.set_show_apply_button(true);
        {
            let settings = settings.clone();
            ngrok_row.connect_apply(move |row| {
                settings
                    .borrow_mut()
                    .set_string("ngrok_token", row.text().as_str());
                settings.borrow().save();
            });
        }
        group.add(&ngrok_row);

        let startup_group = adw::PreferencesGroup::new();
        startup_group.set_title("Startup");
        page.add(&startup_group);

        let background_row = adw::ActionRow::new();
        background_row.set_title("Run in Background");
        let background_switch = gtk::Switch::new();
        background_switch.set_valign(gtk::Align::Center);
        set_switch_active(
            &background_switch,
            settings.borrow().get_bool("run_in_background"),
        );
        background_row.add_suffix(&background_switch);
        background_row.set_activatable_widget(Some(&background_switch));
        startup_group.add(&background_row);

        let login_row = adw::ActionRow::new();
        login_row.set_title("Start on Login");
        let login_switch = gtk::Switch::new();
        login_switch.set_valign(gtk::Align::Center);
        set_switch_active(&login_switch, settings.borrow().get_bool("start_on_login"));
        login_row.set_sensitive(settings.borrow().get_bool("run_in_background"));
        login_row.add_suffix(&login_switch);
        login_row.set_activatable_widget(Some(&login_switch));
        startup_group.add(&login_row);

        {
            let settings = settings.clone();
            let updating = updating_startup_switches.clone();
            let login_switch = login_switch.clone();
            let login_row_clone = login_row.clone();
            background_switch.connect_state_set(move |switch, state| {
                login_row_clone.set_sensitive(state);

                if *updating.borrow() {
                    return false.into();
                }

                if !state {
                    settings.borrow_mut().set_bool("run_in_background", false);
                    settings.borrow_mut().set_bool("start_on_login", false);
                    settings.borrow().save();
                    *updating.borrow_mut() = true;
                    set_switch_active(&login_switch, false);
                    *updating.borrow_mut() = false;
                    request_background(false, |_, _, _, _| {});
                    return false.into();
                }

                let switch_clone = switch.clone();
                let login_switch_clone = login_switch.clone();
                let settings = settings.clone();
                let updating = updating.clone();
                let autostart_requested = settings.borrow().get_bool("start_on_login");
                request_background(
                    autostart_requested,
                    move |ok, background_allowed, _autostart_enabled, message| {
                        let run_in_background = ok && background_allowed;
                        settings
                            .borrow_mut()
                            .set_bool("run_in_background", run_in_background);
                        *updating.borrow_mut() = true;
                        set_switch_active(&switch_clone, run_in_background);
                        if !run_in_background {
                            settings.borrow_mut().set_bool("start_on_login", false);
                            set_switch_active(&login_switch_clone, false);
                            if !message.is_empty() {
                                show_error_for_widget(
                                    &switch_clone,
                                    "Background Permission",
                                    &message,
                                );
                            }
                        }
                        *updating.borrow_mut() = false;
                        settings.borrow().save();
                    },
                );
                false.into()
            });
        }

        {
            let settings = settings.clone();
            let updating = updating_startup_switches.clone();
            let background_switch = background_switch.clone();
            login_switch.connect_state_set(move |switch, state| {
                if *updating.borrow() {
                    return false.into();
                }

                if !state {
                    settings.borrow_mut().set_bool("start_on_login", false);
                    settings.borrow().save();
                    request_background(false, |_, _, _, _| {});
                    return false.into();
                }

                let switch_clone = switch.clone();
                let background_switch_clone = background_switch.clone();
                let settings = settings.clone();
                let updating = updating.clone();
                request_background(
                    state,
                    move |ok, background_allowed, autostart_enabled, message| {
                        let run_in_background = ok && background_allowed;
                        let start_on_login = ok && background_allowed && autostart_enabled && state;
                        settings
                            .borrow_mut()
                            .set_bool("run_in_background", run_in_background);
                        settings
                            .borrow_mut()
                            .set_bool("start_on_login", start_on_login);
                        *updating.borrow_mut() = true;
                        set_switch_active(&background_switch_clone, run_in_background);
                        set_switch_active(&switch_clone, start_on_login);
                        *updating.borrow_mut() = false;
                        if state && !start_on_login && !message.is_empty() {
                            show_error_for_widget(&switch_clone, "Startup Permission", &message);
                        }
                        settings.borrow().save();
                    },
                );
                false.into()
            });
        }

        toolbar_view.set_content(Some(&page));
        dialog.set_child(Some(&toolbar_view));

        Self { dialog }
    }

    fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
    }
}

struct PlayitAgentRow {
    row: adw::ExpanderRow,
}

impl PlayitAgentRow {
    fn new(toast_overlay: &adw::ToastOverlay) -> Self {
        let manager = get_shared_playit_manager();
        let row = adw::ExpanderRow::new();
        row.set_title("Playit Agent");
        row.set_subtitle("Stopped");

        let suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        suffix_box.set_valign(gtk::Align::Center);

        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        spinner.set_size_request(16, 16);
        spinner.set_visible(false);
        suffix_box.append(&spinner);

        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        suffix_box.append(&switch);
        row.add_suffix(&suffix_box);

        // Inner autostart row moved into the expander
        let autostart_row = adw::ActionRow::new();
        autostart_row.set_title("Start on Carabiner Launch");
        let autostart_switch = gtk::Switch::new();
        autostart_switch.set_valign(gtk::Align::Center);
        set_switch_active(
            &autostart_switch,
            load_settings().get_bool("playit_agent_autostart"),
        );
        autostart_row.add_suffix(&autostart_switch);
        autostart_row.set_activatable_widget(Some(&autostart_switch));
        row.add_row(&autostart_row);

        let error_open = Rc::new(RefCell::new(false));
        Self::update_status(&row, &spinner, &switch, &error_open, &manager.status());

        let (start_tx, start_rx) = unbounded::<(bool, String)>();
        {
            let row = row.clone();
            let switch = switch.clone();
            let spinner = spinner.clone();
            let error_open = error_open.clone();
            drain_receiver(start_rx, move |(ok, msg)| {
                if !ok {
                    if !*error_open.borrow() {
                        *error_open.borrow_mut() = true;
                        show_error_for_widget(&row, "Agent Error", &msg);
                        *error_open.borrow_mut() = false;
                    }
                    Self::update_status(&row, &spinner, &switch, &error_open, "stopped");
                }
            });
        }

        {
            let manager = manager.clone();
            let start_tx = start_tx.clone();
            switch.connect_state_set(move |_, state| {
                if state {
                    if !manager.is_running() {
                        let manager = manager.clone();
                        let start_tx = start_tx.clone();
                        thread::spawn(move || {
                            let _ = start_tx.send(manager.start_agent(None));
                        });
                    }
                } else {
                    let _ = manager.stop();
                }
                true.into()
            });
        }

        // hook autostart switch to persistent settings
        {
            autostart_switch.connect_state_set(move |_, state| {
                let mut settings = load_settings();
                settings.set_bool("playit_agent_autostart", state);
                settings.save();
                false.into()
            });
        }

        let (tx, rx) = unbounded();
        let status_id = manager.connect("status-changed", move |event| {
            let _ = tx.send(event);
        });
        {
            let manager_handle = manager.clone();
            row.connect_destroy(move |_| {
                manager_handle.disconnect(status_id);
            });
        }

        {
            let row = row.clone();
            let switch = switch.clone();
            let spinner = spinner.clone();
            let error_open = error_open.clone();
            let toast_overlay = toast_overlay.clone();
            drain_receiver(rx, move |event| {
                if let ManagerEvent::StatusChanged(status) = event {
                    Self::update_status(&row, &spinner, &switch, &error_open, &status);
                    if let Some(msg) = status.strip_prefix("error:") {
                        if !*error_open.borrow() {
                            *error_open.borrow_mut() = true;
                            show_error_for_widget(&row, "Agent Error", msg.trim());
                            add_toast(&toast_overlay, "Agent Error");
                            *error_open.borrow_mut() = false;
                        }
                    }
                }
            });
        }

        Self { row }
    }

    fn update_status(
        row: &adw::ExpanderRow,
        spinner: &gtk::Spinner,
        switch: &gtk::Switch,
        _error_open: &Rc<RefCell<bool>>,
        status: &str,
    ) {
        let is_busy = matches!(status, "starting" | "creating" | "stopping");
        switch.set_sensitive(!is_busy);
        if is_busy {
            spinner.set_visible(true);
            spinner.start();
        } else {
            spinner.set_visible(false);
            spinner.stop();
        }
        if status == "running" {
            row.set_subtitle("Running");
            set_switch_active(switch, true);
        } else if status == "stopped" {
            row.set_subtitle("Stopped");
            set_switch_active(switch, false);
        } else if status.starts_with("error:") {
            row.set_subtitle("Error");
            set_switch_active(switch, false);
        } else if status == "starting" {
            row.set_subtitle("Starting...");
        } else {
            row.set_subtitle(&format!("{}...", capitalize(status)));
        }
    }
}

struct TunnelRow {
    row: adw::ExpanderRow,
}

impl TunnelRow {
    fn new(config: TunnelConfig, on_delete: Rc<dyn Fn(Option<String>)>) -> Self {
        let manager = get_manager_for_tunnel(&config);
        let row = adw::ExpanderRow::new();
        let label = config.label.trim().to_string();
        let title = if label.is_empty() {
            format!("Port {} • {}", config.port, config.protocol)
        } else {
            label
        };
        row.set_title(&title);
        row.set_subtitle("Stopped");

        let public_url = Rc::new(RefCell::new({
            let endpoint = manager.public_endpoint();
            if endpoint.is_empty() {
                config.public_url.clone()
            } else {
                endpoint
            }
        }));
        let config = Rc::new(RefCell::new(config));
        let is_cycling_hostname = Rc::new(RefCell::new(false));
        let info_url_row: Rc<RefCell<Option<adw::ActionRow>>> = Rc::new(RefCell::new(None));
        let error_open = Rc::new(RefCell::new(false));

        let suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        suffix_box.set_valign(gtk::Align::Center);
        row.add_suffix(&suffix_box);

        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        spinner.set_size_request(16, 16);
        spinner.set_visible(false);
        suffix_box.append(&spinner);

        let main_copy_btn = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Copy tunnel link")
            .build();
        main_copy_btn.add_css_class("flat");
        suffix_box.append(&main_copy_btn);

        let switch = if config.borrow().provider != "Playit" {
            let switch = gtk::Switch::new();
            switch.set_valign(gtk::Align::Center);
            suffix_box.append(&switch);
            Some(switch)
        } else {
            None
        };
        let (start_tx, start_rx) = unbounded::<(bool, String)>();

        let info_row = adw::ActionRow::new();
        info_row.set_title("Tunnel Info");
        info_row.set_activatable(true);
        let info_btn = gtk::Button::builder()
            .icon_name("dialog-information-symbolic")
            .valign(gtk::Align::Center)
            .build();
        info_btn.add_css_class("flat");
        info_row.add_suffix(&info_btn);
        row.add_row(&info_row);

        if config.borrow().provider != "Playit" {
            let autostart_row = adw::ActionRow::new();
            autostart_row.set_title("Start on Carabiner Launch");
            let autostart_switch = gtk::Switch::new();
            autostart_switch.set_valign(gtk::Align::Center);
            set_switch_active(&autostart_switch, config.borrow().autostart);
            autostart_row.add_suffix(&autostart_switch);
            autostart_row.set_activatable_widget(Some(&autostart_switch));
            {
                let config = config.clone();
                autostart_switch.connect_state_set(move |_, state| {
                    config.borrow_mut().autostart = state;
                    update_tunnel_autostart(&config.borrow().id, state);
                    false.into()
                });
            }
            row.add_row(&autostart_row);
        }

        let delete_row = adw::ActionRow::new();
        delete_row.set_title("Delete Tunnel");
        let delete_btn = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .build();
        delete_btn.add_css_class("destructive-action");
        delete_row.add_suffix(&delete_btn);
        delete_row.set_activatable_widget(Some(&delete_btn));
        row.add_row(&delete_row);

        {
            let public_url = public_url.clone();
            let row = row.clone();
            main_copy_btn.connect_clicked(move |_| {
                let url = public_url.borrow().clone();
                if !url.is_empty() {
                    row.clipboard().set_text(&url);
                    if let Some(root) = row.root() {
                        if let Ok(win) = root.downcast::<adw::ApplicationWindow>() {
                            if let Some(overlay) = win
                                .content()
                                .and_then(|w| w.downcast::<adw::ToastOverlay>().ok())
                            {
                                add_toast(&overlay, "Copied to clipboard");
                            }
                        }
                    }
                }
            });
        }

        if let Some(switch) = &switch {
            let manager = manager.clone();
            let config = config.clone();
            let row_for_error = row.clone();
            let start_tx = start_tx.clone();
            switch.connect_state_set(move |_, state| {
                if state {
                    if !manager.is_running() {
                        let manager = manager.clone();
                        let config = config.borrow().clone();
                        let start_tx = start_tx.clone();
                        thread::spawn(move || {
                            if config.provider == "Ngrok" {
                                for other in managers_snapshot() {
                                    if other.as_ngrok().is_some()
                                        && other.identity_key() != manager.identity_key()
                                        && other.is_running()
                                    {
                                        other.stop();
                                    }
                                }
                            }
                            let ok = manager.start(config.port, &config.protocol.to_lowercase());
                            let msg = if ok {
                                String::new()
                            } else {
                                "Failed to start tunnel.".to_string()
                            };
                            let _ = start_tx.send((ok, msg));
                        });
                    }
                } else {
                    manager.stop();
                }
                let _ = &row_for_error;
                true.into()
            });
        }

        {
            let this = TunnelRowRefs {
                row: row.clone(),
                manager: manager.clone(),
                config: config.clone(),
                public_url: public_url.clone(),
                main_copy_btn: main_copy_btn.clone(),
                spinner: spinner.clone(),
                switch: switch.clone(),
                info_url_row: info_url_row.clone(),
                is_cycling_hostname: is_cycling_hostname.clone(),
                error_open: error_open.clone(),
            };
            this.update_status_ui(&manager.status());
        }

        let refs = TunnelRowRefs {
            row: row.clone(),
            manager: manager.clone(),
            config: config.clone(),
            public_url: public_url.clone(),
            main_copy_btn: main_copy_btn.clone(),
            spinner: spinner.clone(),
            switch: switch.clone(),
            info_url_row: info_url_row.clone(),
            is_cycling_hostname: is_cycling_hostname.clone(),
            error_open: error_open.clone(),
        };

        {
            let refs = refs.clone();
            drain_receiver(start_rx, move |(ok, msg)| {
                if !ok {
                    show_error_for_widget(&refs.row, "Error", &msg);
                    refs.update_status_ui("stopped");
                }
            });
        }

        let (tx, rx) = unbounded();
        let status_id = manager.connect("status-changed", {
            let tx = tx.clone();
            move |event| {
                let _ = tx.send(event);
            }
        });
        let endpoint_id = manager.connect("endpoint-changed", move |event| {
            let _ = tx.send(event);
        });
        {
            let manager = manager.clone();
            row.connect_destroy(move |_| {
                manager.disconnect(status_id);
                manager.disconnect(endpoint_id);
            });
        }
        drain_receiver(rx, move |event| match event {
            ManagerEvent::StatusChanged(status) => refs.update_status_ui(&status),
            ManagerEvent::EndpointChanged { endpoint, .. } => {
                if refs.config.borrow().provider != "Playit" {
                    *refs.public_url.borrow_mut() = endpoint;
                }
                refs.update_status_ui(&refs.manager.status());
            }
            ManagerEvent::OutputReceived(_) => {}
        });

        {
            let refs = TunnelRowRefs {
                row: row.clone(),
                manager: manager.clone(),
                config: config.clone(),
                public_url: public_url.clone(),
                main_copy_btn: main_copy_btn.clone(),
                spinner: spinner.clone(),
                switch: switch.clone(),
                info_url_row: info_url_row.clone(),
                is_cycling_hostname: is_cycling_hostname.clone(),
                error_open: error_open.clone(),
            };
            info_row.connect_activated(move |_| refs.show_info_dialog());
        }
        {
            let refs = TunnelRowRefs {
                row: row.clone(),
                manager: manager.clone(),
                config: config.clone(),
                public_url: public_url.clone(),
                main_copy_btn: main_copy_btn.clone(),
                spinner: spinner.clone(),
                switch,
                info_url_row,
                is_cycling_hostname,
                error_open,
            };
            info_btn.connect_clicked(move |_| refs.show_info_dialog());
        }

        {
            let config = config.clone();
            let delete_btn_for_parent = delete_btn.clone();
            delete_btn.connect_clicked(move |_| {
                let label = config.borrow().label.trim().to_string();
                let name = if label.is_empty() {
                    format!("{} port {}", config.borrow().provider, config.borrow().port)
                } else {
                    label
                };
                let dialog = adw::MessageDialog::builder()
                    .heading("Delete Tunnel?")
                    .body(format!("\"{name}\" will be permanently removed."))
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("delete", "Delete");
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                dialog.set_close_response("cancel");
                let config = config.clone();
                let on_delete = on_delete.clone();
                dialog.connect_response(Some("delete"), move |dialog, _| {
                    remove_tunnel(&config.borrow().id);
                    dialog.close();
                    on_delete(Some("Tunnel deleted".to_string()));
                });
                dialog.connect_response(Some("cancel"), |dialog, _| dialog.close());
                if let Some(parent) = delete_btn_for_parent
                    .root()
                    .and_then(|root| root.downcast::<gtk::Window>().ok())
                {
                    dialog.set_transient_for(Some(&parent));
                }
                dialog.present();
            });
        }

        Self { row }
    }
}

#[derive(Clone)]
struct TunnelRowRefs {
    row: adw::ExpanderRow,
    manager: ManagerHandle,
    config: Rc<RefCell<TunnelConfig>>,
    public_url: Rc<RefCell<String>>,
    spinner: gtk::Spinner,
    main_copy_btn: gtk::Button,
    switch: Option<gtk::Switch>,
    info_url_row: Rc<RefCell<Option<adw::ActionRow>>>,
    is_cycling_hostname: Rc<RefCell<bool>>,
    error_open: Rc<RefCell<bool>>,
}

impl TunnelRowRefs {
    fn update_status_ui(&self, status: &str) {
        let is_playit = self.config.borrow().provider == "Playit";
        let is_busy = if is_playit {
            matches!(status, "creating" | "stopping")
        } else {
            matches!(status, "starting" | "creating" | "stopping")
        };
        if let Some(switch) = &self.switch {
            switch.set_sensitive(!is_busy);
        }
        self.spinner.set_visible(is_busy);
        if is_busy {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }

        if self.config.borrow().provider == "Playit" {
            if let Some(playit) = self.manager.as_playit() {
                let protocol = self.config.borrow().protocol.to_lowercase();
                for tunnel in playit.tunnels_for(&protocol) {
                    if tunnel.port == Some(self.config.borrow().port) && !tunnel.hostname.is_empty()
                    {
                        *self.public_url.borrow_mut() = tunnel.hostname.clone();
                        update_tunnel_url(&self.config.borrow().id, &tunnel.hostname);
                        break;
                    }
                }
            }
        }

        let mut display_text = String::new();
        if self.config.borrow().provider == "Playit" && !self.public_url.borrow().is_empty() {
            display_text = self.public_url.borrow().clone();
        }

        if display_text.is_empty() {
            display_text = if status == "running" {
                "Running".to_string()
            } else if status == "stopped" {
                "Stopped".to_string()
            } else if status.starts_with("error:") {
                "Error".to_string()
            } else if status == "starting" {
                "Starting...".to_string()
            } else if status == "creating" {
                "Creating tunnel...".to_string()
            } else {
                format!("{}...", capitalize(status))
            };
        }
        self.row.set_subtitle(&display_text);

        if status == "running" {
            if let Some(switch) = &self.switch {
                set_switch_active(switch, true);
            }
        } else if status == "stopped" {
            if let Some(switch) = &self.switch {
                set_switch_active(switch, false);
            }
        } else if let Some(msg) = status.strip_prefix("error:") {
            if let Some(switch) = &self.switch {
                set_switch_active(switch, false);
            }
            let mut msg = msg.trim().to_string();
            if msg.contains("ERR_NGROK_8013") {
                msg = "Ngrok requires a credit or debit card to use TCP endpoints on a free account. This card will not be charged.\n\n<a href=\"https://dashboard.ngrok.com/settings#id-verification\">Click here to add a card to your account</a>".to_string();
            }
            if !*self.error_open.borrow() {
                *self.error_open.borrow_mut() = true;
                show_error_for_widget(&self.row, "Tunnel Error", &msg);
                *self.error_open.borrow_mut() = false;
            }
        }

        let mut show_url = !self.public_url.borrow().is_empty();
        if self.config.borrow().provider != "Playit" && status != "running" {
            show_url = false;
        }
        self.main_copy_btn.set_visible(show_url);

        if let Some(row) = self.info_url_row.borrow().as_ref() {
            if *self.is_cycling_hostname.borrow() {
                row.set_subtitle("Creating...");
            } else if self.public_url.borrow().is_empty() {
                row.set_subtitle("Not available");
            } else {
                row.set_subtitle(&self.public_url.borrow());
            }
        }
    }

    fn show_info_dialog(&self) {
        let Some(parent) = self
            .row
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok())
        else {
            return;
        };

        let dialog = adw::Dialog::builder()
            .content_width(380)
            .content_height(420)
            .build();
        let toast_overlay = adw::ToastOverlay::new();
        dialog.set_child(Some(&toast_overlay));
        let toolbar = adw::ToolbarView::new();
        toast_overlay.set_child(Some(&toolbar));
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Tunnel Info", "")));
        toolbar.add_top_bar(&header);
        let page = adw::PreferencesPage::new();
        toolbar.set_content(Some(&page));
        let group = adw::PreferencesGroup::new();
        page.add(&group);

        let local_port_row = adw::ActionRow::new();
        local_port_row.set_title("Local Port");
        local_port_row.set_subtitle(&format!(
            "Port {} • {}",
            self.config.borrow().port,
            self.config.borrow().protocol
        ));
        group.add(&local_port_row);

        let provider_row = adw::ActionRow::new();
        provider_row.set_title("Provider");
        provider_row.set_subtitle(&self.config.borrow().provider);
        group.add(&provider_row);

        let info_url_row = adw::ActionRow::new();
        info_url_row.set_title("Public URL");
        let public_url_subtitle = self.public_url.borrow().clone();
        info_url_row.set_subtitle(if public_url_subtitle.is_empty() {
            "Not available"
        } else {
            &public_url_subtitle
        });
        info_url_row.set_selectable(true);
        group.add(&info_url_row);
        *self.info_url_row.borrow_mut() = Some(info_url_row.clone());

        let label_row = adw::EntryRow::new();
        label_row.set_title("Label");
        label_row.set_text(&self.config.borrow().label);
        label_row.set_show_apply_button(true);
        {
            let refs = self.clone();
            let dialog = dialog.clone();
            label_row.connect_apply(move |row| {
                let new_label = row.text().trim().to_string();
                update_tunnel_label(&refs.config.borrow().id, &new_label);
                refs.config.borrow_mut().label = new_label.clone();
                if new_label.is_empty() {
                    refs.row.set_title(&format!(
                        "Port {} • {}",
                        refs.config.borrow().port,
                        refs.config.borrow().protocol
                    ));
                } else {
                    refs.row.set_title(&new_label);
                }
                dialog.close();
                if let Some(root) = refs.row.root() {
                    if let Ok(win) = root.downcast::<adw::ApplicationWindow>() {
                        if let Some(overlay) = win
                            .content()
                            .and_then(|w| w.downcast::<adw::ToastOverlay>().ok())
                        {
                            add_toast(&overlay, "Label updated");
                        }
                    }
                }
            });
        }
        group.add(&label_row);

        if self.config.borrow().provider == "Playit" {
            let cycle_row = adw::ActionRow::new();
            cycle_row.set_title("Cycle Hostname");
            cycle_row.set_subtitle("Get a new public link");
            let cycle_btn = gtk::Button::builder()
                .icon_name("view-refresh-symbolic")
                .valign(gtk::Align::Center)
                .build();
            cycle_btn.add_css_class("flat");
            cycle_row.add_suffix(&cycle_btn);
            cycle_row.set_activatable_widget(Some(&cycle_btn));
            group.add(&cycle_row);

            let refs = self.clone();
            let toast_overlay = toast_overlay.clone();
            cycle_btn.connect_clicked(move |btn| {
                if *refs.is_cycling_hostname.borrow() {
                    return;
                }
                *refs.is_cycling_hostname.borrow_mut() = true;
                if let Some(row) = refs.info_url_row.borrow().as_ref() {
                    row.set_subtitle("Creating...");
                }
                cycle_row.set_subtitle("Cycling...");
                btn.set_sensitive(false);

                let (tx, rx) = unbounded();
                let manager = refs.manager.as_playit();
                let port = refs.config.borrow().port;
                let protocol = refs.config.borrow().protocol.to_lowercase();
                let label = if refs.config.borrow().label.is_empty() {
                    "carabiner".to_string()
                } else {
                    refs.config.borrow().label.clone()
                };
                thread::spawn(move || {
                    let mut public_url = None;
                    if let Some(playit) = manager {
                        if !playit.initialized() {
                            let _ = playit.initialize();
                        }
                        if playit.initialized() {
                            playit.delete_tunnels(port, &protocol);
                            public_url = playit
                                .get_tunnel(port, &protocol, true, &label)
                                .ok()
                                .flatten()
                                .and_then(|tunnel| {
                                    if tunnel.hostname.is_empty() {
                                        None
                                    } else {
                                        Some(tunnel.hostname)
                                    }
                                });
                        }
                    }
                    let _ = tx.send(public_url);
                });

                let refs_done = refs.clone();
                let cycle_row_done = cycle_row.clone();
                let btn_done = btn.clone();
                let toast_overlay = toast_overlay.clone();
                drain_receiver(rx, move |public_url| {
                    *refs_done.is_cycling_hostname.borrow_mut() = false;
                    if let Some(public_url) = public_url {
                        *refs_done.public_url.borrow_mut() = public_url.clone();
                        update_tunnel_url(&refs_done.config.borrow().id, &public_url);
                    }
                    if let Some(row) = refs_done.info_url_row.borrow().as_ref() {
                        if refs_done.public_url.borrow().is_empty() {
                            row.set_subtitle("Not available");
                        } else {
                            row.set_subtitle(&refs_done.public_url.borrow());
                        }
                    }
                    cycle_row_done.set_subtitle("Get a new public link");
                    btn_done.set_sensitive(true);
                    add_toast(&toast_overlay, "Tunnel hostname cycled");
                });
            });
        }

        dialog.present(Some(&parent));
    }
}

struct SetupDialog {
    dialog: adw::Dialog,
}

impl SetupDialog {
    fn new(on_saved: Rc<dyn Fn(Option<String>)>) -> Self {
        let dialog = adw::Dialog::builder()
            .content_width(400)
            .content_height(500)
            .build();
        let nav_view = adw::NavigationView::new();
        dialog.set_child(Some(&nav_view));
        let provider_page = SetupProviderPage::new(&nav_view, {
            let dialog = dialog.clone();
            Rc::new(move |toast| {
                dialog.close();
                on_saved(toast);
            })
        });
        nav_view.add(&provider_page);
        Self { dialog }
    }

    fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
    }
}

struct SetupProviderPage;

impl SetupProviderPage {
    fn new(
        nav_view: &adw::NavigationView,
        on_saved: Rc<dyn Fn(Option<String>)>,
    ) -> adw::NavigationPage {
        let toolbar_view = adw::ToolbarView::new();
        let page = adw::NavigationPage::new(&toolbar_view, "Add Tunnel");
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let original_page = adw::PreferencesPage::new();
        toolbar_view.set_content(Some(&original_page));
        let group = adw::PreferencesGroup::new();
        group.set_title("Select Provider");
        original_page.add(&group);

        let subtitles = [
            ("Cloudflare", "Free HTTP · No account"),
            ("Ngrok", "Free UDP · Paid TCP"),
            ("Playit", "Free UDP · Free TCP"),
        ];

        for (provider, subtitle) in subtitles {
            let row = adw::ActionRow::new();
            row.set_title(provider);
            row.set_subtitle(subtitle);
            row.set_activatable(true);
            let icon = gtk::Image::from_icon_name("go-next-symbolic");
            row.add_suffix(&icon);
            let nav_view = nav_view.clone();
            let on_saved = on_saved.clone();
            row.connect_activated(move |_| {
                provider_selected(provider, &nav_view, on_saved.clone());
            });
            group.add(&row);
        }

        page
    }
}

fn provider_selected(
    provider: &str,
    nav_view: &adw::NavigationView,
    on_saved: Rc<dyn Fn(Option<String>)>,
) {
    let manager = get_provider_manager(provider);

    let push_details: Rc<dyn Fn()> = {
        let nav_view = nav_view.clone();
        let on_saved = on_saved.clone();
        let provider = provider.to_string();
        Rc::new(move || {
            nav_view.push(&setup_details_page(&provider, on_saved.clone()));
        })
    };

    let push_auth_if_needed = Rc::new({
        let nav_view = nav_view.clone();
        let manager = manager.clone();
        let push_details = push_details.clone();
        move || {
            if let Some(playit) = manager.as_playit() {
                let (linked, _) = playit.validate_existing_link(3);
                if !linked {
                    nav_view.push(&setup_playit_auth_page(playit, push_details.clone()));
                } else {
                    push_details();
                }
            } else if let Some(ngrok) = manager.as_ngrok() {
                if !ngrok.has_auth_token() {
                    nav_view.push(&setup_ngrok_auth_page(ngrok, push_details.clone()));
                } else {
                    push_details();
                }
            } else {
                push_details();
            }
        }
    });

    if !manager.is_installed() {
        let page = setup_download_page(provider, nav_view, manager, push_auth_if_needed);
        nav_view.push(&page);
    } else {
        push_auth_if_needed();
    }
}

fn setup_download_page(
    provider: &str,
    nav_view: &adw::NavigationView,
    manager: ManagerHandle,
    on_complete: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    let page = adw::NavigationPage::new(&toolbar, "Download Binary");
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let status = adw::StatusPage::new();
    status.set_title(&format!("Downloading {provider}"));
    status.set_description(Some(&format!(
        "Please wait while {provider} is being downloaded..."
    )));

    let progress_bar = gtk::ProgressBar::new();
    progress_bar.set_show_text(true);
    progress_bar.set_hexpand(true);
    progress_bar.set_valign(gtk::Align::Center);
    progress_bar.set_margin_start(32);
    progress_bar.set_margin_end(32);
    progress_bar.set_margin_bottom(16);

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 16);
    box_.append(&status);
    box_.append(&progress_bar);
    box_.set_valign(gtk::Align::Center);
    toolbar.set_content(Some(&box_));

    let (tx, rx) = unbounded();
    let (progress_tx, progress_rx) = unbounded::<f64>();

    let manager_for_thread = manager.clone();
    thread::spawn(move || {
        let result = manager_for_thread.install_latest_binary(Some(Box::new(
            move |downloaded: u64, total: u64| {
                let frac = if total > 0 {
                    downloaded as f64 / total as f64
                } else {
                    -1.0
                };
                let _ = progress_tx.send(frac);
            },
        )));
        let _ = tx.send(result);
    });

    glib::timeout_add_local(Duration::from_millis(100), {
        let progress_bar = progress_bar.clone();
        move || {
            while let Ok(frac) = progress_rx.try_recv() {
                if frac < 0.0 {
                    progress_bar.pulse();
                    progress_bar.set_text(None);
                } else {
                    progress_bar.set_fraction(frac.min(1.0));
                    progress_bar.set_text(Some(&format!("{:.0}%", (frac * 100.0).min(100.0))));
                }
            }
            glib::ControlFlow::Continue
        }
    });

    let nav_view = nav_view.clone();
    drain_receiver(rx, move |(ok, msg)| {
        nav_view.pop();
        if ok {
            on_complete();
        } else {
            show_error_for_widget(&nav_view, "Download Failed", &msg);
        }
    });

    page
}

fn setup_details_page(provider: &str, on_saved: Rc<dyn Fn(Option<String>)>) -> adw::NavigationPage {
    let toolbar_view = adw::ToolbarView::new();
    let page = adw::NavigationPage::new(&toolbar_view, "Tunnel Details");
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);
    let prefs = adw::PreferencesPage::new();
    toolbar_view.set_content(Some(&prefs));
    let group = adw::PreferencesGroup::new();
    group.set_title(&format!("{provider} Settings"));
    prefs.add(&group);

    let label_row = adw::EntryRow::new();
    label_row.set_title("Label (optional)");
    group.add(&label_row);

    let protocol_row = adw::ComboRow::new();
    protocol_row.set_title("Protocol");
    let protocol_model = if provider == "Playit" {
        gtk::StringList::new(&["TCP", "UDP"])
    } else if provider == "Ngrok" {
        gtk::StringList::new(&["TCP", "HTTP"])
    } else {
        let model = gtk::StringList::new(&["HTTP"]);
        protocol_row.set_selected(0);
        protocol_row.set_sensitive(false);
        model
    };
    protocol_row.set_model(Some(&protocol_model));
    group.add(&protocol_row);

    let port_row = adw::ActionRow::new();
    port_row.set_title("Local Port");
    let port_spin = gtk::SpinButton::with_range(1.0, 65535.0, 1.0);
    let initial_protocol = protocol_model
        .string(protocol_row.selected())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "TCP".to_string());
    let initial_port = match initial_protocol.as_str() {
        "HTTP" => 8080.0,
        "UDP" => 19132.0,
        _ => 25565.0,
    };
    port_spin.set_value(initial_port);
    port_spin.set_valign(gtk::Align::Center);
    port_row.add_suffix(&port_spin);
    group.add(&port_row);

    let port_spin_clone = port_spin.clone();
    protocol_row.connect_selected_notify(move |row| {
        let selected = row.selected();
        if let Some(model) = row
            .model()
            .and_then(|m| m.downcast::<gtk::StringList>().ok())
        {
            if let Some(protocol) = model.string(selected) {
                match protocol.as_str() {
                    "TCP" => port_spin_clone.set_value(25565.0),
                    "HTTP" => port_spin_clone.set_value(8080.0),
                    "UDP" => port_spin_clone.set_value(19132.0),
                    _ => {}
                }
            }
        }
    });

    let btn_group = adw::PreferencesGroup::new();
    prefs.add(&btn_group);
    let save_btn = gtk::Button::with_label("Save Tunnel");
    save_btn.add_css_class("suggested-action");
    save_btn.add_css_class("pill");
    save_btn.set_margin_top(24);
    btn_group.add(&save_btn);

    let provider = provider.to_string();
    save_btn.connect_clicked(move |_| {
        let port = port_spin.value() as u16;
        let protocol = protocol_model
            .string(protocol_row.selected())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "TCP".to_string());
        let label = label_row.text().trim().to_string();
        add_tunnel(&provider, &protocol, port, &label);
        on_saved(Some("Tunnel created".to_string()));
    });

    page
}

fn setup_ngrok_auth_page(
    manager: Arc<NgrokManager>,
    next_step: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    let page = adw::NavigationPage::new(&toolbar, "Ngrok Setup");
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let prefs = adw::PreferencesPage::new();
    toolbar.set_content(Some(&prefs));
    let group = adw::PreferencesGroup::new();
    group.set_title("Auth Token");
    group.set_description(Some(
        "Get your Ngrok Auth Token and paste it here. You only need to do this once.",
    ));
    prefs.add(&group);
    let link = gtk::LinkButton::with_label(
        "https://dashboard.ngrok.com/get-started/your-authtoken",
        "Get Ngrok Auth Token",
    );
    link.set_margin_bottom(16);
    group.add(&link);
    let token_entry = adw::EntryRow::new();
    token_entry.set_title("Token");
    group.add(&token_entry);
    let btn_group = adw::PreferencesGroup::new();
    prefs.add(&btn_group);
    let save_btn = gtk::Button::with_label("Save & Continue");
    save_btn.add_css_class("suggested-action");
    save_btn.add_css_class("pill");
    save_btn.set_margin_top(12);
    btn_group.add(&save_btn);

    save_btn.connect_clicked(move |btn| {
        let token = token_entry.text().trim().to_string();
        if token.is_empty() {
            return;
        }
        btn.set_sensitive(false);
        btn.set_label("Saving...");
        token_entry.set_sensitive(false);

        let (tx, rx) = unbounded();
        let manager = manager.clone();
        thread::spawn(move || {
            let _ = tx.send(manager.set_auth_token(&token));
        });
        let btn = btn.clone();
        let token_entry = token_entry.clone();
        let next_step = next_step.clone();
        drain_receiver(rx, move |(ok, msg)| {
            if ok {
                let mut settings = load_settings();
                settings.set_string("ngrok_token", token_entry.text().trim());
                save_settings(&settings);
                next_step();
            } else {
                btn.set_sensitive(true);
                token_entry.set_sensitive(true);
                btn.set_label("Save & Continue");
                show_error_for_widget(&btn, "Error", &format!("Failed to set token: {msg}"));
            }
        });
    });

    page
}

fn setup_playit_auth_page(
    manager: Arc<PlayitManager>,
    next_step: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    let page = adw::NavigationPage::new(&toolbar, "Playit Setup");
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let status = adw::StatusPage::new();
    status.set_title("Checking Playit Account");
    status.set_description(Some("Please wait..."));
    let spinner = gtk::Spinner::new();
    spinner.start();
    spinner.set_size_request(48, 48);
    spinner.set_halign(gtk::Align::Center);
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 16);
    box_.append(&status);
    box_.append(&spinner);
    box_.set_valign(gtk::Align::Center);
    toolbar.set_content(Some(&box_));

    let (tx, rx) = unbounded();
    let manager_for_thread = manager.clone();
    thread::spawn(move || {
        let (valid, _msg) = manager_for_thread.validate_existing_link(3);
        let _ = tx.send(valid);
    });

    let toolbar_clone = toolbar.clone();
    drain_receiver(rx, move |is_linked| {
        build_playit_auth_ui(
            &toolbar_clone,
            manager.clone(),
            next_step.clone(),
            is_linked,
        );
    });

    page
}

fn build_playit_auth_ui(
    toolbar: &adw::ToolbarView,
    manager: Arc<PlayitManager>,
    next_step: Rc<dyn Fn()>,
    is_linked: bool,
) {
    let prefs = adw::PreferencesPage::new();
    toolbar.set_content(Some(&prefs));
    if is_linked {
        let group = adw::PreferencesGroup::new();
        group.set_title("Account Linked");
        group.set_description(Some(
            "Your Playit account is already linked and ready to go.",
        ));
        prefs.add(&group);
        let btn_group = adw::PreferencesGroup::new();
        prefs.add(&btn_group);
        let continue_btn = gtk::Button::with_label("Continue");
        continue_btn.add_css_class("suggested-action");
        continue_btn.add_css_class("pill");
        {
            let next_step = next_step.clone();
            continue_btn.connect_clicked(move |_| next_step());
        }
        btn_group.add(&continue_btn);
        let relink_btn = gtk::Button::with_label("Link a Different Account");
        relink_btn.add_css_class("pill");
        relink_btn.set_margin_top(12);
        {
            let toolbar = toolbar.clone();
            relink_btn.connect_clicked(move |_| {
                build_playit_auth_ui(&toolbar, manager.clone(), next_step.clone(), false);
            });
        }
        btn_group.add(&relink_btn);
        return;
    }

    let group = adw::PreferencesGroup::new();
    group.set_title("Link Account");
    group.set_description(Some(
        "Visit the link below to get your claim code, then paste it here.",
    ));
    prefs.add(&group);
    let link = gtk::LinkButton::with_label(manager.setup_url(), "Open Playit Setup in Browser");
    link.set_margin_bottom(16);
    group.add(&link);
    let code_entry = adw::EntryRow::new();
    code_entry.set_title("Claim Code");
    group.add(&code_entry);
    let btn_group = adw::PreferencesGroup::new();
    prefs.add(&btn_group);
    let link_btn = gtk::Button::with_label("Link Account");
    link_btn.add_css_class("suggested-action");
    link_btn.add_css_class("pill");
    link_btn.set_margin_top(12);
    btn_group.add(&link_btn);

    link_btn.connect_clicked(move |btn| {
        let code = code_entry.text().trim().to_string();
        if code.is_empty() {
            return;
        }
        btn.set_sensitive(false);
        btn.set_label("Linking...");
        code_entry.set_sensitive(false);
        let (tx, rx) = unbounded();
        let manager = manager.clone();
        thread::spawn(move || {
            let _ = tx.send(manager.link_account(&code));
        });
        let btn = btn.clone();
        let code_entry = code_entry.clone();
        let next_step = next_step.clone();
        drain_receiver(rx, move |(ok, msg)| {
            if ok {
                let mut settings = load_settings();
                settings.set_string("playit_token", code_entry.text().trim());
                save_settings(&settings);
                next_step();
            } else {
                btn.set_sensitive(true);
                code_entry.set_sensitive(true);
                btn.set_label("Link Account");
                show_error_for_widget(&btn, "Link Failed", &msg);
            }
        });
    });
}

fn capitalize(status: &str) -> String {
    let mut chars = status.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
