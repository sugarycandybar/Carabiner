import json

from carabiner.backend.constants import DATA_DIR


SETTINGS_FILE = DATA_DIR / "settings.json"

DEFAULT_SETTINGS = {
    "playit_token": "",
    "ngrok_token": "",
    "run_in_background": False,
    "start_on_login": False,
    "playit_agent_autostart": False,
}


def load_settings():
    settings = dict(DEFAULT_SETTINGS)
    if not SETTINGS_FILE.exists():
        return settings

    try:
        with open(SETTINGS_FILE, "r") as f:
            loaded = json.load(f)
        if isinstance(loaded, dict):
            settings.update(loaded)
    except Exception:
        pass

    return settings


def save_settings(settings):
    merged = dict(DEFAULT_SETTINGS)
    merged.update(settings)
    with open(SETTINGS_FILE, "w") as f:
        json.dump(merged, f, indent=2)
