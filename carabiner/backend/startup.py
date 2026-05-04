from __future__ import annotations

import threading

from gi.repository import GLib

from carabiner.backend.ngrok_manager import NgrokManager
from carabiner.backend.portal import set_background_status
from carabiner.backend.settings import load_settings
from carabiner.backend.tunnel_store import MANAGER_REGISTRY, load_tunnels


def _start_items() -> int:
    from carabiner.window import get_manager_for_tunnel, get_shared_playit_manager

    settings = load_settings()
    tunnels = load_tunnels()
    autostart_tunnels = [
        t for t in tunnels
        if t.get("autostart") and str(t.get("provider", "")).lower() != "playit"
    ]
    started = 0

    if settings.get("playit_agent_autostart"):
        manager = get_shared_playit_manager()
        ok, _msg = manager.start_agent()
        if ok:
            started += 1

    for t_config in autostart_tunnels:
        manager = get_manager_for_tunnel(t_config)
        if isinstance(manager, NgrokManager):
            for other in MANAGER_REGISTRY.values():
                if isinstance(other, NgrokManager) and other != manager and other.is_running:
                    other.stop()

        ok = manager.start(int(t_config["port"]), str(t_config["protocol"]).lower())
        if ok:
            started += 1

    return started


def start_configured_items(callback=None):
    def run():
        started = _start_items()
        if started == 1:
            set_background_status("1 tunnel running")
        elif started > 1:
            set_background_status(f"{started} tunnels running")

        if callback:
            GLib.idle_add(callback, started)

    threading.Thread(target=run, daemon=True).start()
