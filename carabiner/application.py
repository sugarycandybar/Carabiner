import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio, GLib

from carabiner.window import CarabinerWindow
from carabiner.backend.events import set_main_thread_dispatcher
from carabiner.backend.settings import load_settings
from carabiner.backend.startup import start_configured_items
from carabiner.backend.tunnel_store import load_tunnels

def gtk_main_thread_dispatcher(callback, *args, **kwargs):
    def wrapper():
        try:
            callback(*args, **kwargs)
        except Exception as e:
            print(f"Exception in UI thread: {e}")
        return GLib.SOURCE_REMOVE
    GLib.idle_add(wrapper)

class CarabinerApplication(Adw.Application):
    def __init__(self):
        super().__init__(
            application_id="io.github.sugarycandybar.Carabiner",
            flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE
        )
        self._activate_in_background = False
        self._startup_items_started = False
        self._background_hold = False
        set_main_thread_dispatcher(gtk_main_thread_dispatcher)

    def do_command_line(self, command_line):
        args = command_line.get_arguments()
        self._activate_in_background = "--background" in args
        self.activate()
        return 0

    def do_activate(self):
        if self._activate_in_background:
            self._activate_in_background = False
            if not self._background_hold:
                self.hold()
                self._background_hold = True
            self._start_startup_items_once()
            return

        win = self.props.active_window
        if not win:
            win = CarabinerWindow(application=self)
        win.present()
        self._start_startup_items_once()

    def _start_startup_items_once(self):
        if self._startup_items_started:
            return
        self._startup_items_started = True
        settings = load_settings()
        if settings.get("playit_agent_autostart") or any(
            t.get("autostart") and str(t.get("provider", "")).lower() != "playit"
            for t in load_tunnels()
        ):
            start_configured_items()

    def release_background_hold(self):
        if self._background_hold:
            self.release()
            self._background_hold = False
