mod application;
mod config {
    include!(concat!(env!("OUT_DIR"), "/config.rs"));
}
mod constants;
mod events;
mod managers;
mod portal;
mod settings;
mod startup;
mod tunnel_store;
mod util;
mod window;

use config::{GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};
use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let lang = settings::load_settings().get_string("language");
    if !lang.is_empty() {
        unsafe { std::env::set_var("LANGUAGE", &lang) };
    }

    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("Unable to set the text domain encoding");
    textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    if let Ok(resources) = gio::Resource::load(PKGDATADIR.to_owned() + "/carabiner.gresource") {
        gio::resources_register(&resources);
    }

    application::run()
}
