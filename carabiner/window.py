import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio, GLib
import threading
import time
import re

from carabiner.backend.playit_manager import PlayitManager
from carabiner.backend.cloudflare_manager import CloudflareManager
from carabiner.backend.ngrok_manager import NgrokManager
from carabiner.backend.tunnel_store import load_tunnels, add_tunnel, remove_tunnel, update_tunnel_label, MANAGER_REGISTRY, stop_all_tunnels
from carabiner.backend.constants import APP_ID, APP_NAME, APP_VERSION, APP_WEBSITE

_shared_playit_manager = None

def get_shared_playit_manager():
    """Get or create the single shared PlayitManager instance."""
    global _shared_playit_manager
    if _shared_playit_manager is None:
        _shared_playit_manager = PlayitManager()
    return _shared_playit_manager

def get_provider_manager(provider):
    """Get a manager instance for installation checks."""
    if provider == "Playit":
        return get_shared_playit_manager()
    elif provider == "Cloudflare":
        return CloudflareManager()
    else:
        return NgrokManager()

def get_manager_for_tunnel(t_config):
    """Get or create a persistent manager instance for a specific tunnel."""
    if t_config["provider"] == "Playit":
        return get_shared_playit_manager()

    t_id = t_config["id"]
    if t_id in MANAGER_REGISTRY:
        return MANAGER_REGISTRY[t_id]
    
    mgr = get_provider_manager(t_config["provider"])
    MANAGER_REGISTRY[t_id] = mgr
    return mgr

import json
from carabiner.backend.constants import DATA_DIR

SETTINGS_FILE = DATA_DIR / "settings.json"

def load_settings():
    if not SETTINGS_FILE.exists():
        return {"playit_token": "", "ngrok_token": ""}
    try:
        with open(SETTINGS_FILE, "r") as f:
            return json.load(f)
    except:
        return {"playit_token": "", "ngrok_token": ""}

def save_settings(settings):
    with open(SETTINGS_FILE, "w") as f:
        json.dump(settings, f, indent=2)

class PreferencesDialog(Adw.Dialog):
    def __init__(self):
        super().__init__()
        self.set_title("Preferences")
        self.set_content_width(400)
        
        self.settings = load_settings()
        
        page = Adw.PreferencesPage()
        group = Adw.PreferencesGroup()
        group.set_title("Tunnel Tokens")
        page.add(group)
        
        self.playit_row = Adw.EntryRow(title="Playit Token")
        self.playit_row.set_text(self.settings.get("playit_token", ""))
        self.playit_row.set_show_apply_button(True)
        self.playit_row.connect("apply", self._on_apply_playit)
        group.add(self.playit_row)
        
        self.ngrok_row = Adw.EntryRow(title="Ngrok Token")
        self.ngrok_row.set_text(self.settings.get("ngrok_token", ""))
        self.ngrok_row.set_show_apply_button(True)
        self.ngrok_row.connect("apply", self._on_apply_ngrok)
        group.add(self.ngrok_row)
        
        self.set_child(page)
        
    def _on_apply_playit(self, row):
        self.settings["playit_token"] = row.get_text()
        save_settings(self.settings)
        
    def _on_apply_ngrok(self, row):
        self.settings["ngrok_token"] = row.get_text()
        save_settings(self.settings)

class PlayitAgentRow(Adw.ActionRow):
    """Group-level row with a single switch to start/stop the Playit agent."""

    def __init__(self):
        super().__init__()
        self.manager = get_shared_playit_manager()
        self.set_title("Playit Agent")
        self.set_subtitle("Stopped")

        self.switch = Gtk.Switch()
        self.switch.set_valign(Gtk.Align.CENTER)
        self.switch.connect("state-set", self._on_switch_toggled)
        self.add_suffix(self.switch)

        self.connect("destroy", self._on_destroy)
        self._status_handler = self.manager.connect("status-changed", self._on_status_changed)
        self._update_status(self.manager.status)

    def _on_status_changed(self, manager, status):
        GLib.idle_add(self._update_status, status)

    def _on_destroy(self, widget):
        if hasattr(self, "_status_handler") and self._status_handler:
            self.manager.disconnect(self._status_handler)
            self._status_handler = 0

    def _update_status(self, status):
        is_busy = status in ["starting", "creating", "stopping"]
        self.switch.set_sensitive(not is_busy)

        if status == "running":
            self.set_subtitle("Running")
            self.switch.set_active(True)
            self.switch.set_state(True)
        elif status == "stopped":
            self.set_subtitle("Stopped")
            self.switch.set_active(False)
            self.switch.set_state(False)
        elif status.startswith("error:"):
            msg = status.split("error:", 1)[1].strip()
            self.set_subtitle("Error")
            self.switch.set_active(False)
            self.switch.set_state(False)
            self._show_error("Agent Error", msg)
        elif status == "starting":
            self.set_subtitle("Starting…")
        else:
            self.set_subtitle(status.capitalize() + "…")

    def _on_switch_toggled(self, switch, state):
        if state:
            if not self.manager.is_running:
                self._start_agent()
        else:
            if self.manager.is_running:
                self.manager.stop()
        return True

    def _start_agent(self):
        def start_thread():
            ok, msg = self.manager.start_agent()
            if not ok:
                GLib.idle_add(self._show_error, "Agent Error", msg)
                GLib.idle_add(self._update_status, "stopped")

        threading.Thread(target=start_thread, daemon=True).start()

    def _show_error(self, title, msg):
        if hasattr(self, "_error_dialog") and self._error_dialog:
            return

        win = self.get_root()
        if not win:
            return

        dialog = Adw.MessageDialog(heading=title, body=msg)
        self._error_dialog = dialog
        dialog.set_body_use_markup(True)
        dialog.add_response("ok", "Close")
        dialog.set_response_appearance("ok", Adw.ResponseAppearance.SUGGESTED)
        
        def _on_response(d, r):
            self._error_dialog = None
            
        dialog.connect("response", _on_response)
        dialog.set_transient_for(win)
        dialog.present()


class TunnelRow(Adw.ExpanderRow):
    def __init__(self, tunnel_config, on_delete):
        super().__init__()
        self.config = tunnel_config
        self.on_delete = on_delete

        provider = self.config["provider"]
        port = self.config["port"]
        protocol = self.config["protocol"]
        label = self.config.get("label", "").strip()

        self.set_title(label if label else f"Port {port} • {protocol}")
        self.set_subtitle("Stopped")

        self.manager = get_manager_for_tunnel(self.config)
        self._status_hid = self.manager.connect("status-changed", self._on_status_changed)
        self._endpoint_hid = self.manager.connect("endpoint-changed", self._on_endpoint_changed)
        self.connect("destroy", self._on_destroy)

        # Suffix container
        self.suffix_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        self.suffix_box.set_valign(Gtk.Align.CENTER)
        self.add_suffix(self.suffix_box)

        # Main row copy button
        self.main_copy_btn = Gtk.Button()
        self.main_copy_btn.set_icon_name("edit-copy-symbolic")
        self.main_copy_btn.set_valign(Gtk.Align.CENTER)
        self.main_copy_btn.add_css_class("flat")
        self.main_copy_btn.set_tooltip_text("Copy tunnel link")
        self.main_copy_btn.connect("clicked", self._on_copy_clicked)
        self.suffix_box.append(self.main_copy_btn)

        # Switch (only for non-Playit providers — Playit is agent-level)
        self.switch = None
        if provider != "Playit":
            self.switch = Gtk.Switch()
            self.switch.set_valign(Gtk.Align.CENTER)
            self.switch.connect("state-set", self._on_switch_toggled)
            self.suffix_box.append(self.switch)

        # Local Port Row (removed from main list)
        self.port_row = Adw.ActionRow()
        self.port_row.set_title("Local Port")
        self.port_row.set_subtitle(f"Port {port} • {protocol}")

        # Info row
        self.info_row = Adw.ActionRow()
        self.info_row.set_title("Tunnel Info")
        self.info_row.set_activatable(True)
        self.info_row.connect("activated", self._on_info_clicked)
        
        info_btn = Gtk.Button()
        info_btn.set_icon_name("dialog-information-symbolic")
        info_btn.set_valign(Gtk.Align.CENTER)
        info_btn.add_css_class("flat")
        info_btn.connect("clicked", self._on_info_clicked)
        
        self.info_row.add_suffix(info_btn)
        self.add_row(self.info_row)

        # Inner rows

        # Delete button
        self.delete_row = Adw.ActionRow()
        self.delete_row.set_title("Delete Tunnel")
        self.delete_btn = Gtk.Button()
        self.delete_btn.set_icon_name("user-trash-symbolic")
        self.delete_btn.add_css_class("destructive-action")
        self.delete_btn.set_valign(Gtk.Align.CENTER)
        self.delete_btn.connect("clicked", lambda b: self._on_delete_clicked())
        self.delete_row.add_suffix(self.delete_btn)
        self.add_row(self.delete_row)

        self.public_url = self.manager.public_endpoint or self.config.get("public_url", "")
        self._update_status_ui(self.manager.status)

    def _on_status_changed(self, manager, status):
        GLib.idle_add(self._update_status_ui, status)

    def _on_destroy(self, widget):
        if hasattr(self, "_status_hid") and self._status_hid:
            self.manager.disconnect(self._status_hid)
            self._status_hid = 0
        if hasattr(self, "_endpoint_hid") and self._endpoint_hid:
            self.manager.disconnect(self._endpoint_hid)
            self._endpoint_hid = 0

    def _update_status_ui(self, status):
        is_busy = status in ["starting", "creating", "stopping"]
        if self.switch:
            self.switch.set_sensitive(not is_busy)

        # Retrieve specific tunnel URL for Playit
        if self.config["provider"] == "Playit":
            try:
                for t in self.manager.tunnels.get(self.config["protocol"].lower(), []):
                    if t.port == int(self.config["port"]) and t.hostname:
                        self.public_url = t.hostname
                        from carabiner.backend.tunnel_store import update_tunnel_url
                        update_tunnel_url(self.config["id"], t.hostname)
                        break
            except Exception:
                pass

        display_text = ""
        if self.config["provider"] == "Playit" and self.public_url:
            display_text = self.public_url

        if not display_text:
            if status == "running":
                display_text = "Running"
            elif status == "stopped":
                display_text = "Stopped"
            elif status.startswith("error:"):
                display_text = "Error"
            elif status == "starting":
                display_text = "Starting..."
            elif status == "creating":
                display_text = "Creating tunnel..."
            else:
                display_text = status.capitalize() + "..."

        self.set_subtitle(display_text)

        if status == "running":
            if self.switch:
                self.switch.set_active(True)
                self.switch.set_state(True)
        elif status == "stopped":
            if self.switch:
                self.switch.set_active(False)
                self.switch.set_state(False)
        elif status.startswith("error:"):
            msg = status.split("error:", 1)[1].strip()
            if "ERR_NGROK_8013" in msg:
                msg = "Ngrok requires a credit or debit card to use TCP endpoints on a free account. This card will not be charged.\n\n<a href=\"https://dashboard.ngrok.com/settings#id-verification\">Click here to add a card to your account</a>"
            if self.switch:
                self.switch.set_active(False)
                self.switch.set_state(False)
            self._show_error("Tunnel Error", msg)

        show_url = bool(self.public_url)
        if self.config["provider"] != "Playit" and status != "running":
            show_url = False

        self.main_copy_btn.set_visible(show_url)
        
        if hasattr(self, "_info_url_row") and self._info_url_row:
            self._info_url_row.set_subtitle(self.public_url if self.public_url else "Not available")

    def _on_endpoint_changed(self, manager, endpoint, claim_url):
        if self.config["provider"] == "Playit":
            # Playit manager emits the generic 'best' endpoint. 
            # Individual TunnelRows pull their specific URL from the API cache in _update_status_ui instead.
            GLib.idle_add(self._update_status_ui, manager.status)
            return

        if endpoint:
            self.public_url = endpoint
        else:
            self.public_url = ""
            
        GLib.idle_add(self._update_status_ui, manager.status)

    def _on_copy_clicked(self, btn):
        if self.public_url:
            self.get_clipboard().set(self.public_url)
            win = self.get_root()
            if hasattr(win, "add_toast"):
                win.add_toast("Copied to clipboard")

    def _on_info_clicked(self, row):
        win = self.get_root()
        if not win:
            return

        dialog = Adw.Dialog()
        dialog.set_content_width(380)
        dialog.set_content_height(420)

        toolbar = Adw.ToolbarView()
        dialog.set_child(toolbar)

        header = Adw.HeaderBar()
        header.set_title_widget(Adw.WindowTitle(title="Tunnel Info"))
        toolbar.add_top_bar(header)

        page = Adw.PreferencesPage()
        toolbar.set_content(page)

        group = Adw.PreferencesGroup()
        page.add(group)

        # Local Port row
        local_port_row = Adw.ActionRow()
        local_port_row.set_title("Local Port")
        local_port_row.set_subtitle(f"Port {self.config['port']} • {self.config['protocol']}")
        group.add(local_port_row)

        # Provider row
        provider_row = Adw.ActionRow()
        provider_row.set_title("Provider")
        provider_row.set_subtitle(self.config["provider"])
        group.add(provider_row)

        # Public URL row
        self._info_url_row = Adw.ActionRow()
        self._info_url_row.set_title("Public URL")
        self._info_url_row.set_subtitle(self.public_url if self.public_url else "Not available")
        self._info_url_row.set_subtitle_selectable(True)
        group.add(self._info_url_row)

        # Label row (editable with confirmation)
        self._info_label_row = Adw.EntryRow()
        self._info_label_row.set_title("Label")
        self._info_label_row.set_text(self.config.get("label", ""))
        self._info_label_row.set_show_apply_button(True)
        self._info_label_row.connect("apply", self._on_info_label_applied)
        group.add(self._info_label_row)

        # Cycle button (Playit)
        if self.config["provider"] == "Playit":
            cycle_row = Adw.ActionRow()
            cycle_row.set_title("Cycle Hostname")
            cycle_row.set_subtitle("Get a new public link")
            cycle_btn = Gtk.Button()
            cycle_btn.set_icon_name("view-refresh-symbolic")
            cycle_btn.set_valign(Gtk.Align.CENTER)
            cycle_btn.add_css_class("flat")
            cycle_btn.connect("clicked", lambda b: self._on_refresh_name_clicked())
            cycle_row.add_suffix(cycle_btn)
            group.add(cycle_row)

        dialog.present(win)

    def _on_info_label_applied(self, row):
        new_label = row.get_text().strip()
        update_tunnel_label(self.config["id"], new_label)
        self.config["label"] = new_label
        
        port = self.config["port"]
        protocol = self.config["protocol"]
        self.set_title(new_label if new_label else f"Port {port} • {protocol}")
        
        if new_label:
            if not hasattr(self, '_port_row_added') or not self._port_row_added:
                self.add_row(self.port_row)
                self._port_row_added = True
        else:
            if hasattr(self, '_port_row_added') and self._port_row_added:
                self.remove(self.port_row)
                self._port_row_added = False
        
        dialog = row.get_root()
        if dialog:
            dialog.close()

        win = self.get_root()
        if win and hasattr(win, "add_toast"):
            win.add_toast("Label updated")

    def _on_refresh_name_clicked(self):
        """Cycle the Playit tunnel to a new hostname."""
        if not self.manager.is_running:
            self._show_error("Agent Not Running", "The Playit agent must be running to cycle the tunnel hostname.")
            return

        port = int(self.config["port"])
        protocol = self.config["protocol"].lower()

        def cycle_thread():
            if not self.manager.initialized:
                self.manager.initialize()
            if self.manager.initialized:
                self.manager.delete_tunnels(port, protocol)
                # Ensure a new one is created immediately
                self.manager.get_tunnel(port, protocol=protocol, ensure=True, label=self.config.get("label", "carabiner"))
                
            GLib.idle_add(lambda: self.get_root().add_toast("Tunnel hostname cycled") if hasattr(self.get_root(), "add_toast") else None)

        threading.Thread(target=cycle_thread, daemon=True).start()

    def _on_delete_clicked(self):
        win = self.get_root()
        label = self.config.get("label", "").strip()
        name = label if label else f"{self.config['provider']} port {self.config['port']}"
        dialog = Adw.AlertDialog(
            heading="Delete Tunnel?",
            body=f"\u201c{name}\u201d will be permanently removed."
        )
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("delete", "Delete")
        dialog.set_response_appearance("delete", Adw.ResponseAppearance.DESTRUCTIVE)
        dialog.set_default_response("cancel")
        dialog.set_close_response("cancel")
        
        def _on_response(d, response):
            if response == "delete":
                remove_tunnel(self.config["id"])
                if self.on_delete:
                    self.on_delete("Tunnel deleted")

        dialog.connect("response", _on_response)
        dialog.present(win)

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
            if provider == "Ngrok":
                from carabiner.backend.ngrok_manager import NgrokManager
                for mgr in MANAGER_REGISTRY.values():
                    if isinstance(mgr, NgrokManager) and mgr != self.manager:
                        if mgr.is_running:
                            mgr.stop()

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
        if hasattr(self, "_error_dialog") and self._error_dialog:
            return

        win = self.get_root()
        if not win:
            return

        dialog = Adw.MessageDialog(heading=title)
        self._error_dialog = dialog

        if "<a " in msg:
            # For messages with links, we use a custom label in extra_child to ensure it's clickable
            
            # Extract plain text and link info
            link_match = re.search(r'<a href="([^"]+)">([^<]+)</a>', msg)
            if link_match:
                url = link_match.group(1)
                link_text = link_match.group(2)
                plain_text = re.sub(r'<a [^>]+>[^<]+</a>', '', msg).strip()
                
                dialog.set_body(plain_text)
                
                link_btn = Gtk.LinkButton(uri=url, label=link_text)
                link_btn.set_margin_top(12)
                dialog.set_extra_child(link_btn)
            else:
                dialog.set_body(msg)
                dialog.set_body_use_markup(True)
        else:
            dialog.set_body(msg)
            dialog.set_body_use_markup(True)

        dialog.add_response("ok", "Close")
        dialog.set_response_appearance("ok", Adw.ResponseAppearance.SUGGESTED)
        
        def _on_response(d, r):
            self._error_dialog = None
            d.destroy()
            
        dialog.connect("response", _on_response)
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
            # Update settings.json
            settings = load_settings()
            settings["ngrok_token"] = self.token_entry.get_text().strip()
            save_settings(settings)
            
            self.next_step_cb()
        else:
            self.save_btn.set_sensitive(True)
            self.token_entry.set_sensitive(True)
            self.save_btn.set_label("Save & Continue")
            win = self.get_root()
            dialog = Adw.AlertDialog(heading="Error", body=f"Failed to set token: {msg}")
            dialog.add_response("ok", "OK")
            dialog.connect("response", lambda d, r: None)
            dialog.present(win)


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
            # Update settings.json
            settings = load_settings()
            settings["playit_token"] = self.code_entry.get_text().strip()
            save_settings(settings)
            
            self.next_step_cb()
        else:
            self.link_btn.set_sensitive(True)
            self.code_entry.set_sensitive(True)
            self.link_btn.set_label("Link Account")
            
            win = self.get_root()
            dialog = Adw.AlertDialog(heading="Link Failed", body=msg)
            dialog.add_response("ok", "OK")
            dialog.connect("response", lambda d, r: None)
            dialog.present(win)


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

        # Optional label
        self.label_row = Adw.EntryRow()
        self.label_row.set_title("Label (optional)")
        group.add(self.label_row)

        self.protocol_row = Adw.ComboRow()
        self.protocol_row.set_title("Protocol")

        if provider == "Playit":
            self.protocol_model = Gtk.StringList.new(["TCP", "UDP"])
            self.protocol_row.set_model(self.protocol_model)
        elif provider == "Ngrok":
            self.protocol_model = Gtk.StringList.new(["TCP", "HTTP"])
            self.protocol_row.set_model(self.protocol_model)
        else:  # Cloudflare
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
        label = self.label_row.get_text().strip()
        add_tunnel(self.provider, protocol, port, label=label)
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
        dialog = Adw.AlertDialog(heading="Download Failed", body=msg)
        dialog.add_response("ok", "OK")
        dialog.connect("response", lambda d, r: None)
        dialog.present(win)

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
        
        # Add menu button
        self.menu_btn = Gtk.MenuButton()
        self.menu_btn.set_icon_name("open-menu-symbolic")
        self.header.pack_end(self.menu_btn)
        
        menu = Gio.Menu.new()
        menu.append("Preferences", "win.preferences")
        menu.append("About Carabiner", "win.about")
        menu.append("Quit", "win.quit")
        self.menu_btn.set_menu_model(menu)
        
        # Add actions
        preferences_action = Gio.SimpleAction.new("preferences", None)
        preferences_action.connect("activate", self._on_preferences_activated)
        self.add_action(preferences_action)

        about_action = Gio.SimpleAction.new("about", None)
        about_action.connect("activate", self._on_about_activated)
        self.add_action(about_action)
        
        quit_action = Gio.SimpleAction.new("quit", None)
        quit_action.connect("activate", lambda a, p: self.close())
        self.add_action(quit_action)
        
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
        
    def _on_preferences_activated(self, action, param):
        dialog = PreferencesDialog()
        dialog.present(self)
        
    def _on_about_activated(self, action, param):
        about = Adw.AboutDialog(
            application_name=APP_NAME,
            application_icon=APP_ID,
            developer_name="sugarycandybar",
            version=APP_VERSION,
            website=APP_WEBSITE,
            issue_url=f"{APP_WEBSITE}/issues",
            license_type=Gtk.License.GPL_3_0
        )
        about.present(self)
        
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

            # Group tunnels by provider (alphabetical)
            by_provider = {}
            for t in tunnels:
                by_provider.setdefault(t["provider"], []).append(t)

            for provider in sorted(by_provider.keys()):
                group = Adw.PreferencesGroup()
                group.set_title(provider)
                page.add(group)

                # Playit gets a single agent-level start/stop row
                if provider == "Playit":
                    agent_row = PlayitAgentRow()
                    group.add(agent_row)

                for t_config in by_provider[provider]:
                    row = TunnelRow(t_config, on_delete=self._refresh_ui)
                    group.add(row)

            self.toolbar_view.set_content(page)
