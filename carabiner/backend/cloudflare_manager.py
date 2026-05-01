"""
Cloudflare tunnel manager.
"""
from __future__ import annotations

import os
import platform
import re
import shutil
import subprocess
import sys
import threading
import urllib.request
from pathlib import Path
from typing import Optional

from carabiner.backend.events import EventEmitter
from carabiner.backend.constants import DATA_DIR

URL_RE = re.compile(r"https://[a-zA-Z0-9-]+\.trycloudflare\.com")

class CloudflareManager(EventEmitter):
    def __init__(self):
        super().__init__()
        self._process: Optional[subprocess.Popen] = None
        self._status = "stopped"
        self._public_endpoint = ""
        self._read_thread: Optional[threading.Thread] = None

        self.provider = "cloudflare"
        self.directory = DATA_DIR / "cloudflare"
        self.port = 25565
        self.protocol = "tcp" # Cloudflare trycloudflare tunnels default to http, but we just pass the URL

    @property
    def status(self) -> str:
        return self._status

    @property
    def public_endpoint(self) -> str:
        return self._public_endpoint

    @property
    def is_running(self) -> bool:
        return self._process is not None and self._process.poll() is None

    @property
    def binary_path(self) -> Path:
        filename = "cloudflared.exe" if sys.platform == "win32" else "cloudflared"
        return self.directory / filename

    def resolve_binary(self) -> Optional[str]:
        bundled = self.binary_path
        if bundled.exists() and bundled.is_file():
            return str(bundled)

        system_bin = shutil.which("cloudflared")
        if system_bin:
            return system_bin

        return None

    def is_installed(self) -> bool:
        return self.resolve_binary() is not None

    def _set_status(self, status: str):
        if self._status != status:
            self._status = status
            self.emit_on_main_thread("status-changed", status)

    def _emit_endpoint_changed(self):
        self.emit_on_main_thread("endpoint-changed", self._public_endpoint, "")

    def install_latest_binary(self) -> tuple[bool, str]:
        """Download and install latest cloudflared binary."""
        try:
            sys_name = platform.system().lower()
            machine = platform.machine().lower()

            if sys_name == "windows":
                url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe"
            elif sys_name == "darwin":
                if machine in ["arm64", "aarch64"]:
                    url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64"
                else:
                    url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64"
            else:
                if machine in ["arm64", "aarch64"]:
                    url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64"
                else:
                    url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"

            target = self.binary_path
            target.parent.mkdir(parents=True, exist_ok=True)

            req = urllib.request.Request(url, headers={"User-Agent": "Carabiner/1.0"})
            with urllib.request.urlopen(req, timeout=120.0) as resp:
                payload = resp.read()

            with open(target, "wb") as f:
                f.write(payload)

            if sys.platform != "win32":
                target.chmod(0o755)

            return True, str(target)
        except Exception as e:
            return False, str(e)

    def start(self, port: int, protocol: str = "tcp") -> bool:
        if self.is_running:
            return True

        binary = self.resolve_binary()
        if not binary:
            self._set_status("error")
            return False

        self.port = port
        self.protocol = protocol
        self._public_endpoint = ""
        self._emit_endpoint_changed()
        self._set_status("starting")

        cmd = [binary, "tunnel", "--url", f"localhost:{port}"]

        try:
            self._process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
            )
            self._read_thread = threading.Thread(target=self._read_output, daemon=True)
            self._read_thread.start()
            return True
        except Exception as e:
            self._set_status("error")
            return False

    def _read_output(self):
        if not self._process or not self._process.stdout:
            return

        for line in iter(self._process.stdout.readline, ""):
            line = line.strip()
            if not line:
                continue

            match = URL_RE.search(line)
            if match and not self._public_endpoint:
                self._public_endpoint = match.group(0)
                self._emit_endpoint_changed()
                self._set_status("running")

        self._process.stdout.close()
        self._process.wait()
        self._process = None
        self._public_endpoint = ""
        self._emit_endpoint_changed()
        self._set_status("stopped")

    def stop(self):
        if self._process:
            self._set_status("stopping")
            try:
                self._process.terminate()
                self._process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self._process.kill()
            except Exception:
                pass
            self._process = None
        self._public_endpoint = ""
        self._emit_endpoint_changed()
        self._set_status("stopped")
