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

def add_tunnel(provider, protocol, port):
    tunnels = load_tunnels()
    t_id = str(uuid.uuid4())
    tunnels.append({
        "id": t_id,
        "provider": provider,
        "protocol": protocol,
        "port": port
    })
    save_tunnels(tunnels)
    return t_id

def remove_tunnel(t_id):
    tunnels = load_tunnels()
    tunnels = [t for t in tunnels if t["id"] != t_id]
    save_tunnels(tunnels)
    
    # Also stop and remove from registry
    if t_id in MANAGER_REGISTRY:
        mgr = MANAGER_REGISTRY.pop(t_id)
        if mgr.is_running:
            mgr.stop()

MANAGER_REGISTRY = {}

def stop_all_tunnels():
    for t_id, mgr in list(MANAGER_REGISTRY.items()):
        if mgr.is_running:
            mgr.stop()
    MANAGER_REGISTRY.clear()
