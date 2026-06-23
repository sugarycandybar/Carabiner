#![allow(deprecated)]

use crate::util::t;
use gtk::{
    gio,
    glib::{self, variant::ToVariant},
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use uuid::Uuid;

const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const BACKGROUND_INTERFACE: &str = "org.freedesktop.portal.Background";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";

fn background_options(autostart: bool, handle_token: &str) -> HashMap<&'static str, glib::Variant> {
    HashMap::from([
        ("handle_token", handle_token.to_variant()),
        (
            "reason",
            t("Carabiner can keep selected network tunnels running in the background.")
                .to_variant(),
        ),
        ("autostart", autostart.to_variant()),
        (
            "commandline",
            vec!["carabiner", "--background"].to_variant(),
        ),
    ])
}

fn result_bool(results: &HashMap<String, glib::Variant>, key: &str) -> bool {
    results
        .get(key)
        .and_then(|value| value.get::<bool>())
        .unwrap_or(false)
}

pub fn request_background<F>(autostart: bool, callback: F)
where
    F: Fn(bool, bool, bool, String) + 'static,
{
    let callback = Rc::new(callback);
    let bus = match gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) {
        Ok(bus) => bus,
        Err(err) => {
            callback(false, false, false, err.to_string());
            return;
        }
    };

    let handle_token = format!("carabiner_{}", Uuid::new_v4().simple());
    let sender_name = bus
        .unique_name()
        .unwrap_or_default()
        .trim_start_matches(':')
        .replace('.', "_");
    let expected_handle =
        format!("/org/freedesktop/portal/desktop/request/{sender_name}/{handle_token}");
    let subscription_id: Rc<RefCell<Option<gio::SignalSubscriptionId>>> =
        Rc::new(RefCell::new(None));

    let response_callback = {
        let callback = callback.clone();
        let subscription_id = subscription_id.clone();
        move |connection: &gio::DBusConnection,
              _sender: &str,
              _object_path: &str,
              _interface: &str,
              _signal_name: &str,
              parameters: &glib::Variant| {
            if let Some(current_id) = subscription_id.borrow_mut().take() {
                connection.signal_unsubscribe(current_id);
            }

            let Some((response, results)) =
                parameters.get::<(u32, HashMap<String, glib::Variant>)>()
            else {
                callback(false, false, false, t("Invalid portal response."));
                return;
            };

            if response != 0 {
                callback(
                    false,
                    false,
                    false,
                    t("Background permission was not granted."),
                );
                return;
            }

            callback(
                true,
                result_bool(&results, "background"),
                result_bool(&results, "autostart"),
                String::new(),
            );
        }
    };

    let id = bus.signal_subscribe(
        Some(PORTAL_BUS_NAME),
        Some(REQUEST_INTERFACE),
        Some("Response"),
        Some(&expected_handle),
        None,
        gio::DBusSignalFlags::NONE,
        response_callback,
    );
    *subscription_id.borrow_mut() = Some(id);

    let params = ("", background_options(autostart, &handle_token)).to_variant();
    let reply_type = glib::VariantTy::new("(o)").ok();
    let handle = bus.call_sync(
        Some(PORTAL_BUS_NAME),
        PORTAL_OBJECT_PATH,
        BACKGROUND_INTERFACE,
        "RequestBackground",
        Some(&params),
        reply_type,
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
    );

    match handle {
        Ok(value) => {
            if let Some((handle,)) = value.get::<(String,)>()
                && handle != expected_handle
            {
                if let Some(current_id) = subscription_id.borrow_mut().take() {
                    bus.signal_unsubscribe(current_id);
                }
                let response_callback = {
                    let callback = callback.clone();
                    let subscription_id = subscription_id.clone();
                    move |connection: &gio::DBusConnection,
                          _sender: &str,
                          _object_path: &str,
                          _interface: &str,
                          _signal_name: &str,
                          parameters: &glib::Variant| {
                        if let Some(current_id) = subscription_id.borrow_mut().take() {
                            connection.signal_unsubscribe(current_id);
                        }
                        let Some((response, results)) =
                            parameters.get::<(u32, HashMap<String, glib::Variant>)>()
                        else {
                            callback(false, false, false, t("Invalid portal response."));
                            return;
                        };
                        if response != 0 {
                            callback(
                                false,
                                false,
                                false,
                                t("Background permission was not granted."),
                            );
                            return;
                        }
                        callback(
                            true,
                            result_bool(&results, "background"),
                            result_bool(&results, "autostart"),
                            String::new(),
                        );
                    }
                };
                let new_id = bus.signal_subscribe(
                    Some(PORTAL_BUS_NAME),
                    Some(REQUEST_INTERFACE),
                    Some("Response"),
                    Some(&handle),
                    None,
                    gio::DBusSignalFlags::NONE,
                    response_callback,
                );
                *subscription_id.borrow_mut() = Some(new_id);
            }
        }
        Err(err) => {
            if let Some(current_id) = subscription_id.borrow_mut().take() {
                bus.signal_unsubscribe(current_id);
            }
            callback(false, false, false, err.to_string());
        }
    }
}

pub fn set_background_status(message: &str) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        return;
    };
    let mut options = HashMap::new();
    options.insert(
        "message",
        message.chars().take(95).collect::<String>().to_variant(),
    );
    let params = (options,).to_variant();
    let _ = bus.call_sync(
        Some(PORTAL_BUS_NAME),
        PORTAL_OBJECT_PATH,
        BACKGROUND_INTERFACE,
        "SetStatus",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
    );
}
