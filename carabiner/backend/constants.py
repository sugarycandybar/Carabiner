"""
Central constants for Carabiner application.
"""
import os
import sys
from pathlib import Path

# Application identity
APP_ID = "io.github.sugarycandybar.Carabiner"
APP_NAME = "Carabiner"
APP_VERSION = "1.0.0"
APP_WEBSITE = "https://github.com/sugarycandybar/Carabiner"

def _default_data_dir() -> Path:
    # Use XDG_DATA_HOME if set (standard for Linux/Flatpak)
    xdg_data = os.environ.get("XDG_DATA_HOME")
    if xdg_data:
        return Path(xdg_data) / "carabiner"

    if sys.platform == "win32":
        local_app_data = os.environ.get("LOCALAPPDATA")
        if local_app_data:
            return Path(local_app_data) / "Carabiner"
        return Path.home() / "AppData" / "Local" / "Carabiner"

    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "Carabiner"

    return Path.home() / ".local" / "share" / "carabiner"

DATA_DIR = Path(os.environ.get("CARABINER_DATA_DIR", _default_data_dir()))

for d in [DATA_DIR]:
    d.mkdir(parents=True, exist_ok=True)
