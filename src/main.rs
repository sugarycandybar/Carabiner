mod application;
mod constants;
mod events;
mod managers;
mod portal;
mod settings;
mod startup;
mod tunnel_store;
mod util;
mod window;

fn main() -> gtk::glib::ExitCode {
    application::run()
}
