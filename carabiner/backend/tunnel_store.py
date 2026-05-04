import json
from pathlib import Path
from carabiner.backend.constants import DATA_DIR
import uuid

TUNNELS_FILE = DATA_DIR / "tunnels.json"

def load_tunnels():
    if not TUNNELS_FILE.exists():
        return []
    try:
        with open(TUNNELS_FILE, "r") as f:
            return json.load(f)
    except:
        return []

def save_tunnels(tunnels):
    with open(TUNNELS_FILE, "w") as f:
        json.dump(tunnels, f, indent=2)

def add_tunnel(provider, protocol, port, label=""):
    tunnels = load_tunnels()
    t_id = str(uuid.uuid4())
    tunnels.append({
        "id": t_id,
        "provider": provider,
        "protocol": protocol,
        "port": port,
        "label": label,
    })
    save_tunnels(tunnels)
    
    return t_id

def update_tunnel_url(t_id, url):
    tunnels = load_tunnels()
    for t in tunnels:
        if t["id"] == t_id:
            t["public_url"] = url
            save_tunnels(tunnels)
            break

def update_tunnel_label(t_id, label):
    tunnels = load_tunnels()
    for t in tunnels:
        if t["id"] == t_id:
            t["label"] = label
            save_tunnels(tunnels)
            break

def update_tunnel_autostart(t_id, autostart):
    tunnels = load_tunnels()
    for t in tunnels:
        if t["id"] == t_id:
            t["autostart"] = bool(autostart)
            save_tunnels(tunnels)
            break

def remove_tunnel(t_id):
    tunnels = load_tunnels()
    t_config = next((t for t in tunnels if t["id"] == t_id), None)
    if not t_config:
        return

    tunnels = [t for t in tunnels if t["id"] != t_id]
    save_tunnels(tunnels)
    
    # Also stop and remove from registry
    mgr = MANAGER_REGISTRY.pop(t_id, None)
    
    if t_config["provider"] == "Playit":
        from carabiner.backend.playit_manager import PlayitManager
        p_mgr = mgr if isinstance(mgr, PlayitManager) else PlayitManager()
        
        def _delete_bg():
            if not p_mgr.initialized:
                p_mgr.initialize()
            if p_mgr.initialized:
                p_mgr.delete_tunnels(t_config["port"], t_config["protocol"].lower())
        
        import threading
        threading.Thread(target=_delete_bg, daemon=True).start()
    else:
        if mgr and mgr.is_running:
            mgr.stop()

MANAGER_REGISTRY = {}

def stop_all_tunnels():
    seen = set()
    for t_id, mgr in list(MANAGER_REGISTRY.items()):
        if mgr not in seen:
            seen.add(mgr)
            if mgr.is_running:
                mgr.stop()
    MANAGER_REGISTRY.clear()
    
    # Ensure the shared Playit agent is stopped, even if there are no tunnels left in the registry
    try:
        from carabiner.window import get_shared_playit_manager
        p_mgr = get_shared_playit_manager()
        if p_mgr.is_running:
            p_mgr.stop()
    except Exception:
        pass
