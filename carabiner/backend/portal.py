from __future__ import annotations

import uuid

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib


PORTAL_BUS_NAME = "org.freedesktop.portal.Desktop"
PORTAL_OBJECT_PATH = "/org/freedesktop/portal/desktop"
BACKGROUND_INTERFACE = "org.freedesktop.portal.Background"
REQUEST_INTERFACE = "org.freedesktop.portal.Request"


def _background_options(autostart: bool, handle_token: str):
    return {
        "handle_token": GLib.Variant("s", handle_token),
        "reason": GLib.Variant(
            "s",
            "Carabiner can keep selected network tunnels running in the background.",
        ),
        "autostart": GLib.Variant("b", bool(autostart)),
        "commandline": GLib.Variant("as", ["carabiner", "--background"]),
    }


def _result_bool(results, key: str) -> bool:
    value = results.get(key, False)
    if isinstance(value, GLib.Variant):
        return bool(value.unpack())
    return bool(value)


def request_background(autostart: bool, callback):
    """Request background/autostart permission through xdg-desktop-portal.

    callback receives: (ok, background_allowed, autostart_enabled, message)
    """
    try:
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    except Exception as e:
        callback(False, False, False, str(e))
        return

    handle_token = f"carabiner_{uuid.uuid4().hex}"
    sender_name = bus.get_unique_name().lstrip(":").replace(".", "_")
    expected_handle = f"/org/freedesktop/portal/desktop/request/{sender_name}/{handle_token}"
    subscription_id = 0

    def on_response(connection, sender, object_path, interface_name, signal_name, parameters):
        if getattr(on_response, "subscription_id", 0):
            connection.signal_unsubscribe(on_response.subscription_id)
            on_response.subscription_id = 0

        response, results = parameters.unpack()
        if response != 0:
            callback(False, False, False, "Background permission was not granted.")
            return

        callback(
            True,
            _result_bool(results, "background"),
            _result_bool(results, "autostart"),
            "",
        )

    on_response.subscription_id = 0
    subscription_id = bus.signal_subscribe(
        PORTAL_BUS_NAME,
        REQUEST_INTERFACE,
        "Response",
        expected_handle,
        None,
        Gio.DBusSignalFlags.NONE,
        on_response,
    )
    on_response.subscription_id = subscription_id

    def on_request_done(connection, result):
        try:
            handle = connection.call_finish(result).unpack()[0]
        except Exception as e:
            if getattr(on_response, "subscription_id", 0):
                connection.signal_unsubscribe(on_response.subscription_id)
                on_response.subscription_id = 0
            callback(False, False, False, str(e))
            return

        if handle != expected_handle:
            if getattr(on_response, "subscription_id", 0):
                connection.signal_unsubscribe(on_response.subscription_id)
            on_response.subscription_id = connection.signal_subscribe(
                PORTAL_BUS_NAME,
                REQUEST_INTERFACE,
                "Response",
                handle,
                None,
                Gio.DBusSignalFlags.NONE,
                on_response,
            )

    bus.call(
        PORTAL_BUS_NAME,
        PORTAL_OBJECT_PATH,
        BACKGROUND_INTERFACE,
        "RequestBackground",
        GLib.Variant("(sa{sv})", ("", _background_options(autostart, handle_token))),
        GLib.VariantType.new("(o)"),
        Gio.DBusCallFlags.NONE,
        -1,
        None,
        on_request_done,
    )


def set_background_status(message: str):
    try:
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        bus.call(
            PORTAL_BUS_NAME,
            PORTAL_OBJECT_PATH,
            BACKGROUND_INTERFACE,
            "SetStatus",
            GLib.Variant("(a{sv})", ({
                "message": GLib.Variant("s", message[:95]),
            },)),
            None,
            Gio.DBusCallFlags.NONE,
            -1,
            None,
            None,
        )
    except Exception:
        pass
