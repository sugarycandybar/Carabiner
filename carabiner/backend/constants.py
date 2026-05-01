"""
Central constants for Carabiner application.
"""
import os
import sys
from pathlib import Path

# Application identity
APP_ID = "io.github.sugarycandybar.Carabiner"
APP_NAME = "Carabiner"
APP_VERSION = "0.1.0"
APP_WEBSITE = "https://github.com/sugarycandybar/Carabiner"

def _default_data_dir() -> Path:
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
