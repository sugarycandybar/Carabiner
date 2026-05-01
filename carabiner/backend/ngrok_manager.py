"""
Ngrok tunnel manager.
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import threading
import time
import urllib.request
import zipfile
from pathlib import Path
from typing import Optional

from carabiner.backend.events import EventEmitter
from carabiner.backend.constants import DATA_DIR

class NgrokManager(EventEmitter):
    def __init__(self):
        super().__init__()
        self._process: Optional[subprocess.Popen] = None
        self._status = "stopped"
        self._public_endpoint = ""
        self._read_thread: Optional[threading.Thread] = None

        self.provider = "ngrok"
        self.directory = DATA_DIR / "ngrok"
        self.port = 25565
        self.protocol = "tcp"

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
        filename = "ngrok.exe" if sys.platform == "win32" else "ngrok"
        return self.directory / filename

    def resolve_binary(self) -> Optional[str]:
        bundled = self.binary_path
        if bundled.exists() and bundled.is_file():
            return str(bundled)

        system_bin = shutil.which("ngrok")
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
        """Download and install latest ngrok binary."""
        try:
            sys_name = platform.system().lower()
            machine = platform.machine().lower()

            is_zip = False

            if sys_name == "windows":
                url = "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-windows-amd64.zip"
                is_zip = True
            elif sys_name == "darwin":
                is_zip = True
                if machine in ["arm64", "aarch64"]:
                    url = "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-arm64.zip"
                else:
                    url = "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-amd64.zip"
            else:
                if machine in ["arm64", "aarch64"]:
                    url = "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-arm64.tgz"
                else:
                    url = "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-amd64.tgz"

            target_dir = self.directory
            target_dir.mkdir(parents=True, exist_ok=True)
            archive_path = target_dir / ("ngrok.zip" if is_zip else "ngrok.tgz")

            req = urllib.request.Request(url, headers={"User-Agent": "Carabiner/1.0"})
            with urllib.request.urlopen(req, timeout=120.0) as resp:
                payload = resp.read()

            with open(archive_path, "wb") as f:
                f.write(payload)

            if is_zip:
                with zipfile.ZipFile(archive_path, "r") as z:
                    z.extractall(target_dir)
            else:
                with tarfile.open(archive_path, "r:gz") as t:
                    t.extractall(target_dir)

            archive_path.unlink()

            bin_path = self.binary_path
            if sys.platform != "win32" and bin_path.exists():
                bin_path.chmod(0o755)

            return True, str(bin_path)
        except Exception as e:
            return False, str(e)

    def set_auth_token(self, token: str) -> tuple[bool, str]:
        binary = self.resolve_binary()
        if not binary:
            return False, "ngrok binary not found"
        try:
            subprocess.run([binary, "config", "add-authtoken", token], check=True, capture_output=True)
            return True, "Auth token added"
        except subprocess.CalledProcessError as e:
            return False, e.stderr.decode() or "Failed to set auth token"

    def has_auth_token(self) -> bool:
        config_path = Path.home() / ".config" / "ngrok" / "ngrok.yml"
        if not config_path.exists():
            return False
        try:
            with open(config_path, "r") as f:
                content = f.read()
                return "authtoken:" in content
        except:
            return False

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

        cmd = [binary, protocol, str(port), "--log", "stdout"]

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

    def _fetch_url(self):
        # wait a bit for ngrok to start the local api
        for _ in range(10):
            if not self.is_running:
                break
            try:
                req = urllib.request.Request("http://127.0.0.1:4040/api/tunnels")
                with urllib.request.urlopen(req, timeout=2.0) as resp:
                    data = json.loads(resp.read().decode())
                    tunnels = data.get("tunnels", [])
                    if tunnels:
                        self._public_endpoint = tunnels[0].get("public_url", "")
                        self._emit_endpoint_changed()
                        self._set_status("running")
                        return
            except Exception:
                pass
            time.sleep(1.0)
        
        if self.is_running and not self._public_endpoint:
            self._set_status("running")

    def _read_output(self):
        if not self._process or not self._process.stdout:
            return

        # Start a thread to fetch URL
        threading.Thread(target=self._fetch_url, daemon=True).start()

        last_error = ""
        # Keep reading to prevent blocking
        for line in iter(self._process.stdout.readline, ""):
            if "lvl=crit" in line or "lvl=error" in line:
                if "err=" in line:
                    last_error = line.split("err=")[1].strip().strip('"')
                    break
            elif "ERROR:" in line and not last_error:
                new_err = line.replace("ERROR:", "").strip()
                if new_err: last_error = new_err

        self._process.wait()
        self._process = None
        self._public_endpoint = ""
        self._emit_endpoint_changed()
        if last_error:
            self._set_status("error: " + last_error)
        else:
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
