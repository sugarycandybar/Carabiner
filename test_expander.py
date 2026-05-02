import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw

def on_activate(app):
    win = Adw.ApplicationWindow(application=app)
    page = Adw.PreferencesPage()
    group = Adw.PreferencesGroup()
    row = Adw.ExpanderRow(title="Test Row")
    
    switch = Gtk.Switch()
    try:
        row.add_action_widget(switch)
        print("add_action_widget success")
    except Exception as e:
        print(f"Error: {e}")
        try:
            row.add_suffix(switch)
            print("add_suffix success")
        except Exception as e:
            print(f"Error: {e}")
            
    group.add(row)
    page.add(group)
    win.set_content(page)
    win.present()
    app.quit()

app = Adw.Application()
app.connect("activate", on_activate)
app.run()
