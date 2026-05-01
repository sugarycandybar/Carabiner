import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio, GLib
import threading
import time

from carabiner.backend.playit_manager import PlayitManager
from carabiner.backend.cloudflare_manager import CloudflareManager
from carabiner.backend.ngrok_manager import NgrokManager
from carabiner.backend.tunnel_store import load_tunnels, add_tunnel, remove_tunnel, MANAGER_REGISTRY, stop_all_tunnels

def get_provider_manager(provider):
    """Get a temporary manager instance for installation checks."""
    if provider == "Playit":
        return PlayitManager()
    elif provider == "Cloudflare":
        return CloudflareManager()
    else:
        return NgrokManager()

def get_manager_for_tunnel(t_config):
    """Get or create a persistent manager instance for a specific tunnel."""
    t_id = t_config["id"]
    if t_id in MANAGER_REGISTRY:
        return MANAGER_REGISTRY[t_id]
    
    mgr = get_provider_manager(t_config["provider"])
    MANAGER_REGISTRY[t_id] = mgr
    return mgr

class TunnelRow(Adw.ActionRow):
    def __init__(self, tunnel_config, on_delete):
        super().__init__()
        self.config = tunnel_config
        self.on_delete = on_delete
        
        provider = self.config["provider"]
        port = self.config["port"]
        protocol = self.config["protocol"]
        
        self.set_title(provider)
        self.set_subtitle(f"Port {port} • {protocol}")
        
        self.manager = get_manager_for_tunnel(self.config)
            
        self.manager.connect("status-changed", self._on_status_changed)
        self.manager.connect("endpoint-changed", self._on_endpoint_changed)
        
        # Copy button
        self.copy_btn = Gtk.Button()
        self.copy_btn.set_icon_name("edit-copy-symbolic")
        self.copy_btn.set_valign(Gtk.Align.CENTER)
        self.copy_btn.add_css_class("flat")
        self.copy_btn.set_visible(False)
        self.copy_btn.connect("clicked", self._on_copy_clicked)
        self.add_suffix(self.copy_btn)

        # Switch
        self.switch = Gtk.Switch()
        self.switch.set_valign(Gtk.Align.CENTER)
        self.switch.connect("state-set", self._on_switch_toggled)
        self.add_suffix(self.switch)
        
        # Delete button
        self.delete_btn = Gtk.Button()
        self.delete_btn.set_icon_name("user-trash-symbolic")
        self.delete_btn.set_valign(Gtk.Align.CENTER)
        self.delete_btn.add_css_class("flat")
        self.delete_btn.add_css_class("destructive-action")
        self.delete_btn.connect("clicked", self._on_delete_clicked)
        self.add_suffix(self.delete_btn)
        
        self.public_url = self.manager.public_endpoint
        self._update_status_ui(self.manager.status)

    def _on_status_changed(self, manager, status):
        GLib.idle_add(self._update_status_ui, status)
        
    def _update_status_ui(self, status):
        if status == "running":
            sub = f"Running: {self.public_url}" if self.public_url else "Running..."
            self.set_subtitle(sub)
            self.switch.set_active(True)
            self.switch.set_state(True)
            self.copy_btn.set_visible(bool(self.public_url))
        elif status == "stopped":
            self.set_subtitle(f"Port {self.config['port']} • {self.config['protocol']}")
            self.switch.set_active(False)
            self.switch.set_state(False)
            self.copy_btn.set_visible(False)
        elif status.startswith("error:"):
            msg = status.split("error:", 1)[1].strip()
            
            if "ERR_NGROK_8013" in msg:
                msg = "Ngrok requires a credit or debit card to use TCP endpoints on a free account. This card will NOT be charged.\n\n<a href=\"https://dashboard.ngrok.com/settings#id-verification\">Click here to add a card to your account</a>"
                
            self.set_subtitle(f"Port {self.config['port']} • {self.config['protocol']}")
            self.switch.set_active(False)
            self.switch.set_state(False)
            self.copy_btn.set_visible(False)
            self._show_error("Tunnel Error", msg)
        else:
            self.set_subtitle(status.capitalize() + "...")
            self.copy_btn.set_visible(False)
            
    def _on_endpoint_changed(self, manager, endpoint, claim_url):
        self.public_url = endpoint
        if manager.status == "running":
            GLib.idle_add(self._update_status_ui, manager.status)

    def _on_copy_clicked(self, btn):
        if self.public_url:
            self.get_clipboard().set(self.public_url)
            win = self.get_root()
            if hasattr(win, "add_toast"):
                win.add_toast("Copied to clipboard")

    def _on_delete_clicked(self, btn):
        remove_tunnel(self.config["id"])
        if self.on_delete:
            self.on_delete("Tunnel deleted")

    def _on_switch_toggled(self, switch, state):
        if state:
            if not self.manager.is_running:
                self.start_tunnel()
        else:
            if self.manager.is_running:
                self.manager.stop()
        return True # Handled manually
        
    def start_tunnel(self):
        port = int(self.config["port"])
        protocol = self.config["protocol"].lower()
        provider = self.config["provider"]
        
        def start_thread():
            if isinstance(self.manager, PlayitManager):
                ok, msg = self.manager.start(port, protocol=protocol, allow_unclaimed=False, auto_install=False)
                if not ok:
                    GLib.idle_add(self._show_error, "Error", msg)
                    GLib.idle_add(self._update_status_ui, "stopped")
            else:
                ok = self.manager.start(port, protocol=protocol)
                if not ok:
                    GLib.idle_add(self._show_error, "Error", "Failed to start tunnel.")
                    GLib.idle_add(self._update_status_ui, "stopped")
                    
        threading.Thread(target=start_thread, daemon=True).start()
        
    def _show_error(self, title, msg):
        win = self.get_root()
        dialog = Adw.MessageDialog(heading=title, body=msg)
        dialog.set_body_use_markup(True)
        dialog.add_response("ok", "Close")
        dialog.set_response_appearance("ok", Adw.ResponseAppearance.SUGGESTED)
        dialog.connect("response", lambda d, r: d.close())
        dialog.set_transient_for(win)
        dialog.present()

# --- Setup Flow ---

class SetupNgrokAuthPage(Adw.NavigationPage):
    def __init__(self, manager, next_step_cb):
        super().__init__()
        self.set_title("Ngrok Setup")
        self.manager = manager
        self.next_step_cb = next_step_cb
        
        self.toolbar = Adw.ToolbarView()
        self.set_child(self.toolbar)
        
        header = Adw.HeaderBar()
        self.toolbar.add_top_bar(header)
        
        page = Adw.PreferencesPage()
        self.toolbar.set_content(page)
        
        group = Adw.PreferencesGroup()
        group.set_title("Auth Token")
        group.set_description("Get your Ngrok Auth Token and paste it here. You only need to do this once.")
        page.add(group)
        
        link_btn = Gtk.LinkButton(uri="https://dashboard.ngrok.com/get-started/your-authtoken", label="Get Ngrok Auth Token")
        link_btn.set_margin_bottom(16)
        group.add(link_btn)
        
        self.token_entry = Adw.EntryRow()
        self.token_entry.set_title("Token")
        group.add(self.token_entry)
        
        btn_group = Adw.PreferencesGroup()
        page.add(btn_group)
        
        self.save_btn = Gtk.Button(label="Save & Continue")
        self.save_btn.add_css_class("suggested-action")
        self.save_btn.add_css_class("pill")
        self.save_btn.set_margin_top(12)
        self.save_btn.connect("clicked", self._on_save)
        btn_group.add(self.save_btn)
        
    def _on_save(self, btn):
        token = self.token_entry.get_text().strip()
        if not token:
            return
            
        self.save_btn.set_sensitive(False)
        self.save_btn.set_label("Saving...")
        self.token_entry.set_sensitive(False)
        
        def save_thread():
            ok, msg = self.manager.set_auth_token(token)
            GLib.idle_add(self._on_saved_result, ok, msg)
            
        threading.Thread(target=save_thread, daemon=True).start()
        
    def _on_saved_result(self, ok, msg):
        if ok:
            self.next_step_cb()
        else:
            self.save_btn.set_sensitive(True)
            self.token_entry.set_sensitive(True)
            self.save_btn.set_label("Save & Continue")
            win = self.get_root()
            dialog = Adw.MessageDialog(heading="Error", body=f"Failed to set token: {msg}")
            dialog.add_response("ok", "OK")
            dialog.connect("response", lambda d, r: d.destroy() if hasattr(d, "destroy") else None)
            dialog.set_transient_for(win)
            dialog.present()


class SetupPlayitAuthPage(Adw.NavigationPage):
    def __init__(self, manager, next_step_cb):
        super().__init__()
        self.set_title("Playit Setup")
        self.manager = manager
        self.next_step_cb = next_step_cb
        
        self.toolbar = Adw.ToolbarView()
        self.set_child(self.toolbar)
        
        header = Adw.HeaderBar()
        self.toolbar.add_top_bar(header)
        
        status = Adw.StatusPage()
        status.set_title("Checking Playit Account")
        status.set_description("Please wait...")
        spinner = Gtk.Spinner()
        spinner.start()
        spinner.set_size_request(48, 48)
        spinner.set_halign(Gtk.Align.CENTER)
        
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        box.append(status)
        box.append(spinner)
        box.set_valign(Gtk.Align.CENTER)
        self.toolbar.set_content(box)
        
        threading.Thread(target=self._check_link, daemon=True).start()
        
    def _check_link(self):
        valid, msg = self.manager.validate_existing_link()
        GLib.idle_add(self._build_ui, valid)
        
    def _build_ui(self, is_linked):
        page = Adw.PreferencesPage()
        self.toolbar.set_content(page)
        
        if is_linked:
            group = Adw.PreferencesGroup()
            group.set_title("Account Linked")
            group.set_description("Your Playit account is already linked and ready to go.")
            page.add(group)
            
            btn_group = Adw.PreferencesGroup()
            page.add(btn_group)
            
            continue_btn = Gtk.Button(label="Continue")
            continue_btn.add_css_class("suggested-action")
            continue_btn.add_css_class("pill")
            continue_btn.connect("clicked", lambda b: self.next_step_cb())
            btn_group.add(continue_btn)
            
            relink_btn = Gtk.Button(label="Link a Different Account")
            relink_btn.add_css_class("pill")
            relink_btn.set_margin_top(12)
            relink_btn.connect("clicked", lambda b: self._build_ui(False))
            btn_group.add(relink_btn)
        else:
            group = Adw.PreferencesGroup()
            group.set_title("Link Account")
            group.set_description("Visit the link below to get your claim code, then paste it here.")
            page.add(group)
            
            link_btn = Gtk.LinkButton(uri=self.manager.setup_url, label="Open Playit Setup in Browser")
            link_btn.set_margin_bottom(16)
            group.add(link_btn)
            
            self.code_entry = Adw.EntryRow()
            self.code_entry.set_title("Claim Code")
            group.add(self.code_entry)
            
            btn_group = Adw.PreferencesGroup()
            page.add(btn_group)
            
            self.link_btn = Gtk.Button(label="Link Account")
            self.link_btn.add_css_class("suggested-action")
            self.link_btn.add_css_class("pill")
            self.link_btn.set_margin_top(12)
            self.link_btn.connect("clicked", self._on_link)
            btn_group.add(self.link_btn)
            
    def _on_link(self, btn):
        code = self.code_entry.get_text().strip()
        if not code:
            return
        
        self.link_btn.set_sensitive(False)
        self.link_btn.set_label("Linking...")
        self.code_entry.set_sensitive(False)
        
        threading.Thread(target=self._link_thread, args=(code,), daemon=True).start()
        
    def _link_thread(self, code):
        ok, msg = self.manager.link_account(code)
        GLib.idle_add(self._on_linked, ok, msg)
        
    def _on_linked(self, ok, msg):
        if ok:
            self.next_step_cb()
        else:
            self.link_btn.set_sensitive(True)
            self.code_entry.set_sensitive(True)
            self.link_btn.set_label("Link Account")
            
            win = self.get_root()
            dialog = Adw.MessageDialog(heading="Link Failed", body=msg)
            dialog.add_response("ok", "OK")
            dialog.connect("response", lambda d, r: d.destroy() if hasattr(d, "destroy") else None)
            dialog.set_transient_for(win)
            dialog.present()


class SetupDetailsPage(Adw.NavigationPage):
    def __init__(self, provider, on_saved):
        super().__init__()
        self.set_title("Tunnel Details")
        self.provider = provider
        self.on_saved = on_saved
        
        toolbar_view = Adw.ToolbarView()
        self.set_child(toolbar_view)
        
        header = Adw.HeaderBar()
        toolbar_view.add_top_bar(header)
        
        page = Adw.PreferencesPage()
        toolbar_view.set_content(page)
        
        group = Adw.PreferencesGroup()
        group.set_title(f"{provider} Settings")
        page.add(group)
        
        self.protocol_row = Adw.ComboRow()
        self.protocol_row.set_title("Protocol")
        
        if provider == "Playit":
            self.protocol_model = Gtk.StringList.new(["TCP", "UDP"])
            self.protocol_row.set_model(self.protocol_model)
        elif provider == "Ngrok":
            self.protocol_model = Gtk.StringList.new(["TCP", "HTTP"])
            self.protocol_row.set_model(self.protocol_model)
        else: # Cloudflare
            self.protocol_model = Gtk.StringList.new(["HTTP"])
            self.protocol_row.set_model(self.protocol_model)
            self.protocol_row.set_selected(0)
            self.protocol_row.set_sensitive(False)
            
        group.add(self.protocol_row)
        
        self.port_row = Adw.ActionRow()
        self.port_row.set_title("Local Port")
        self.port_spin = Gtk.SpinButton.new_with_range(1, 65535, 1)
        self.port_spin.set_value(25565)
        self.port_spin.set_valign(Gtk.Align.CENTER)
        self.port_row.add_suffix(self.port_spin)
        group.add(self.port_row)
        
        btn_group = Adw.PreferencesGroup()
        page.add(btn_group)
        
        save_btn = Gtk.Button(label="Save Tunnel")
        save_btn.add_css_class("suggested-action")
        save_btn.add_css_class("pill")
        save_btn.set_margin_top(24)
        save_btn.connect("clicked", self._on_save)
        btn_group.add(save_btn)
        
    def _on_save(self, btn):
        port = int(self.port_spin.get_value())
        protocol = self.protocol_model.get_string(self.protocol_row.get_selected())
        add_tunnel(self.provider, protocol, port)
        if self.on_saved:
            self.on_saved("Tunnel created")


class SetupProviderPage(Adw.NavigationPage):
    def __init__(self, nav_view, on_saved):
        super().__init__()
        self.set_title("Add Tunnel")
        self.nav_view = nav_view
        self.on_saved = on_saved
        
        self.toolbar_view = Adw.ToolbarView()
        self.set_child(self.toolbar_view)
        
        header = Adw.HeaderBar()
        self.toolbar_view.add_top_bar(header)
        
        self.original_page = Adw.PreferencesPage()
        self.toolbar_view.set_content(self.original_page)
        
        group = Adw.PreferencesGroup()
        group.set_title("Select Provider")
        self.original_page.add(group)
        
        providers = ["Cloudflare", "Playit", "Ngrok"]
        for p in providers:
            row = Adw.ActionRow()
            row.set_title(p)
            row.set_activatable(True)
            
            icon = Gtk.Image.new_from_icon_name("go-next-symbolic")
            row.add_suffix(icon)
            
            row.connect("activated", lambda r, prov=p: self._on_provider_selected(prov))
            group.add(row)

    def _create_spinner_view(self, provider):
        status = Adw.StatusPage()
        status.set_title("Downloading Binary")
        status.set_description(f"Please wait while {provider} is being downloaded and installed...")
        
        spinner = Gtk.Spinner()
        spinner.start()
        spinner.set_size_request(48, 48)
        spinner.set_halign(Gtk.Align.CENTER)
        
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        box.append(status)
        box.append(spinner)
        box.set_valign(Gtk.Align.CENTER)
        return box
        
    def _restore_content_and_push(self, push_fn):
        self.toolbar_view.set_content(self.original_page)
        push_fn()
        
    def _restore_content_and_show_error(self, msg):
        self.toolbar_view.set_content(self.original_page)
        win = self.get_root()
        dialog = Adw.MessageDialog(heading="Download Failed", body=msg)
        dialog.add_response("ok", "OK")
        dialog.connect("response", lambda d, r: d.destroy() if hasattr(d, "destroy") else None)
        dialog.set_transient_for(win)
        dialog.present()

    def _on_provider_selected(self, provider):
        manager = get_provider_manager(provider)
        
        def push_details():
            details_page = SetupDetailsPage(provider, self.on_saved)
            self.nav_view.push(details_page)

        def push_auth_if_needed():
            if isinstance(manager, PlayitManager):
                linked, _ = manager.validate_existing_link()
                if not linked:
                    auth_page = SetupPlayitAuthPage(manager, push_details)
                    self.nav_view.push(auth_page)
                else:
                    push_details()
            elif isinstance(manager, NgrokManager) and not manager.has_auth_token():
                auth_page = SetupNgrokAuthPage(manager, push_details)
                self.nav_view.push(auth_page)
            else:
                push_details()

        if not manager.is_installed():
            self.toolbar_view.set_content(self._create_spinner_view(provider))
            
            def download_thread():
                ok, msg = manager.install_latest_binary()
                if ok:
                    time.sleep(0.5)
                    GLib.idle_add(self._restore_content_and_push, push_auth_if_needed)
                else:
                    GLib.idle_add(self._restore_content_and_show_error, msg)
            
            threading.Thread(target=download_thread, daemon=True).start()
        else:
            push_auth_if_needed()


class SetupDialog(Adw.Dialog):
    def __init__(self, on_saved):
        super().__init__()
        self.set_content_width(400)
        self.set_content_height(500)
        self.on_saved = on_saved
        
        self.nav_view = Adw.NavigationView()
        self.set_child(self.nav_view)
        
        provider_page = SetupProviderPage(self.nav_view, self._on_setup_complete)
        self.nav_view.add(provider_page)
        
    def _on_setup_complete(self, toast_msg=None):
        self.close()
        if self.on_saved:
            self.on_saved(toast_msg)


class CarabinerWindow(Adw.ApplicationWindow):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.set_default_size(420, 560)
        self.set_title("Carabiner")
        
        self.toast_overlay = Adw.ToastOverlay()
        self.set_content(self.toast_overlay)
        
        self.toolbar_view = Adw.ToolbarView()
        self.toast_overlay.set_child(self.toolbar_view)
        
        self.header = Adw.HeaderBar()
        
        # Add (+) button
        self.add_btn = Gtk.Button()
        self.add_btn.set_icon_name("list-add-symbolic")
        self.add_btn.connect("clicked", self._on_add_clicked)
        self.header.pack_start(self.add_btn)
        
        self.toolbar_view.add_top_bar(self.header)
        
        self.connect("close-request", self._on_close_request)
        
        self._refresh_ui()

    def add_toast(self, text):
        toast = Adw.Toast(title=text)
        self.toast_overlay.add_toast(toast)
        
    def _on_close_request(self, *args):
        stop_all_tunnels()
        return False # Continue closing
        
    def _show_error(self, title, msg):
        dialog = Adw.MessageDialog(heading=title, body=msg)
        dialog.set_body_use_markup(True)
        dialog.add_response("ok", "OK")
        dialog.connect("response", lambda d, r: d.destroy() if hasattr(d, "destroy") else None)
        dialog.set_transient_for(self)
        dialog.present()
        
    def _on_add_clicked(self, btn):
        dialog = SetupDialog(self._refresh_ui)
        dialog.present(self)
        
    def _refresh_ui(self, toast_msg=None):
        if toast_msg:
            self.add_toast(toast_msg)
            
        tunnels = load_tunnels()
        
        if not tunnels:
            status_page = Adw.StatusPage()
            status_page.set_title("No Tunnels")
            status_page.set_description("Create a network tunnel to securely share local ports.")
            status_page.set_icon_name("network-server-symbolic")
            
            btn = Gtk.Button(label="Add Tunnel")
            btn.add_css_class("suggested-action")
            btn.add_css_class("pill")
            btn.set_halign(Gtk.Align.CENTER)
            btn.connect("clicked", self._on_add_clicked)
            
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
            box.append(status_page)
            box.append(btn)
            box.set_valign(Gtk.Align.CENTER)
            
            self.toolbar_view.set_content(box)
        else:
            page = Adw.PreferencesPage()
            group = Adw.PreferencesGroup()
            group.set_title("Configured Tunnels")
            page.add(group)
            
            for t_config in tunnels:
                row = TunnelRow(t_config, on_delete=self._refresh_ui)
                group.add(row)
                
            self.toolbar_view.set_content(page)
