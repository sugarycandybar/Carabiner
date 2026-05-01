import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio, GLib

from carabiner.window import CarabinerWindow
from carabiner.backend.events import set_main_thread_dispatcher

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
            flags=Gio.ApplicationFlags.FLAGS_NONE
        )
        set_main_thread_dispatcher(gtk_main_thread_dispatcher)

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = CarabinerWindow(application=self)
        win.present()
