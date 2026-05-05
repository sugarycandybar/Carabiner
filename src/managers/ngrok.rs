use crate::{
    constants::DATA_DIR,
    events::{EventEmitter, ManagerEvent},
    util,
};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Cursor},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

struct State {
    process: Option<Child>,
    status: String,
    public_endpoint: String,
    port: u16,
    protocol: String,
}

pub struct NgrokManager {
    emitter: EventEmitter,
    state: Mutex<State>,
    directory: PathBuf,
}

impl NgrokManager {
    pub fn new() -> Self {
        Self {
            emitter: EventEmitter::default(),
            state: Mutex::new(State {
                process: None,
                status: "stopped".to_string(),
                public_endpoint: String::new(),
                port: 25565,
                protocol: "tcp".to_string(),
            }),
            directory: DATA_DIR.join("ngrok"),
        }
    }

    pub fn connect<F>(&self, signal_name: &'static str, callback: F) -> u64
    where
        F: Fn(ManagerEvent) + Send + Sync + 'static,
    {
        self.emitter.connect(signal_name, callback)
    }

    pub fn disconnect(&self, handler_id: u64) -> bool {
        self.emitter.disconnect(handler_id)
    }

    pub fn status(&self) -> String {
        self.state.lock().unwrap().status.clone()
    }

    pub fn public_endpoint(&self) -> String {
        self.state.lock().unwrap().public_endpoint.clone()
    }

    pub fn is_running(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(child) = state.process.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                state.process = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn binary_path(&self) -> PathBuf {
        self.directory.join(if cfg!(target_os = "windows") {
            "ngrok.exe"
        } else {
            "ngrok"
        })
    }

    pub fn resolve_binary(&self) -> Option<String> {
        let bundled = self.binary_path();
        if bundled.exists() && bundled.is_file() {
            return Some(bundled.to_string_lossy().to_string());
        }
        util::which("ngrok")
    }

    pub fn is_installed(&self) -> bool {
        self.resolve_binary().is_some()
    }

    fn set_status(&self, status: &str) {
        let should_emit = {
            let mut state = self.state.lock().unwrap();
            if state.status == status {
                false
            } else {
                state.status = status.to_string();
                true
            }
        };
        if should_emit {
            self.emitter.emit(
                "status-changed",
                ManagerEvent::StatusChanged(status.to_string()),
            );
        }
    }

    fn set_endpoint(&self, endpoint: &str) {
        {
            let mut state = self.state.lock().unwrap();
            state.public_endpoint = endpoint.to_string();
        }
        self.emitter.emit(
            "endpoint-changed",
            ManagerEvent::EndpointChanged {
                endpoint: endpoint.to_string(),
                claim_url: String::new(),
            },
        );
    }

    pub fn install_latest_binary(&self) -> (bool, String) {
        let sys_name = util::platform_name();
        let machine = util::machine_name();
        let mut is_zip = false;

        let url = if sys_name == "windows" {
            is_zip = true;
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-windows-amd64.zip".to_string()
        } else if sys_name == "darwin" {
            is_zip = true;
            if machine == "arm64" || machine == "aarch64" {
                "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-arm64.zip".to_string()
            } else {
                "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-amd64.zip".to_string()
            }
        } else if machine == "arm64" || machine == "aarch64" {
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-arm64.tgz".to_string()
        } else {
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-amd64.tgz".to_string()
        };

        if let Err(err) = fs::create_dir_all(&self.directory) {
            return (false, err.to_string());
        }

        let result = reqwest::blocking::Client::new()
            .get(url)
            .header(reqwest::header::USER_AGENT, util::user_agent())
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.bytes());

        let Ok(payload) = result else {
            return (
                false,
                result.err().map(|e| e.to_string()).unwrap_or_default(),
            );
        };

        if is_zip {
            let reader = Cursor::new(payload);
            let mut archive = match zip::ZipArchive::new(reader) {
                Ok(archive) => archive,
                Err(err) => return (false, err.to_string()),
            };
            if let Err(err) = archive.extract(&self.directory) {
                return (false, err.to_string());
            }
        } else {
            let reader = Cursor::new(payload);
            let decoder = flate2::read::GzDecoder::new(reader);
            let mut archive = tar::Archive::new(decoder);
            if let Err(err) = archive.unpack(&self.directory) {
                return (false, err.to_string());
            }
        }

        let bin_path = self.binary_path();
        #[cfg(unix)]
        if bin_path.exists() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755));
        }

        (true, bin_path.to_string_lossy().to_string())
    }

    pub fn set_auth_token(&self, token: &str) -> (bool, String) {
        let Some(binary) = self.resolve_binary() else {
            return (false, "ngrok binary not found".to_string());
        };

        let output = Command::new(binary)
            .args(["config", "add-authtoken", token])
            .output();

        match output {
            Ok(output) if output.status.success() => (true, "Auth token added".to_string()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                (
                    false,
                    if stderr.is_empty() {
                        "Failed to set auth token".to_string()
                    } else {
                        stderr
                    },
                )
            }
            Err(err) => (false, err.to_string()),
        }
    }

    pub fn has_auth_token(&self) -> bool {
        let mut paths = Vec::new();

        if let Some(xdg_config) = util::config_home() {
            paths.push(xdg_config.join("ngrok").join("ngrok.yml"));
        }

        paths.push(util::home().join(".config").join("ngrok").join("ngrok.yml"));
        paths.push(util::home().join(".ngrok2").join("ngrok.yml"));

        #[cfg(target_os = "windows")]
        {
            if let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") {
                paths.push(PathBuf::from(local_appdata).join("ngrok").join("ngrok.yml"));
            }
            paths.push(
                util::home()
                    .join("AppData")
                    .join("Local")
                    .join("ngrok")
                    .join("ngrok.yml"),
            );
        }

        paths.into_iter().any(|path| {
            path.exists()
                && path.is_file()
                && fs::read_to_string(path)
                    .map(|content| content.contains("authtoken:"))
                    .unwrap_or(false)
        })
    }

    pub fn start(self: &Arc<Self>, port: u16, protocol: &str) -> bool {
        if self.is_running() {
            return true;
        }

        let Some(binary) = self.resolve_binary() else {
            self.set_status("error");
            return false;
        };

        {
            let mut state = self.state.lock().unwrap();
            state.port = port;
            state.protocol = protocol.to_string();
        }
        self.set_endpoint("");
        self.set_status("starting");

        let mut command = Command::new(binary);
        command
            .args([protocol, &port.to_string(), "--log", "stdout"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        util::command_no_window(&mut command);

        let Ok(mut child) = command.spawn() else {
            self.set_status("error");
            return false;
        };
        let stdout = child.stdout.take();
        self.state.lock().unwrap().process = Some(child);

        let manager = self.clone();
        thread::spawn(move || manager.read_output(stdout));
        true
    }

    fn fetch_url(self: Arc<Self>) {
        for _ in 0..10 {
            if !self.is_running() {
                break;
            }

            let response = reqwest::blocking::get("http://127.0.0.1:4040/api/tunnels")
                .and_then(|response| response.error_for_status())
                .and_then(|response| response.text());

            if let Ok(text) = response {
                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                    if let Some(tunnel) = data
                        .get("tunnels")
                        .and_then(Value::as_array)
                        .and_then(|tunnels| tunnels.first())
                    {
                        let endpoint = tunnel
                            .get("public_url")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !endpoint.is_empty() {
                            self.set_endpoint(endpoint);
                            self.set_status("running");
                            return;
                        }
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
        }

        if self.is_running() && self.public_endpoint().is_empty() {
            self.set_status("running");
        }
    }

    fn read_output(self: Arc<Self>, stdout: Option<std::process::ChildStdout>) {
        let fetch_manager = self.clone();
        thread::spawn(move || fetch_manager.fetch_url());

        let mut last_error = String::new();
        if let Some(stdout) = stdout {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if line.contains("lvl=crit") || line.contains("lvl=error") {
                    if let Some((_, err)) = line.split_once("err=") {
                        last_error = err.trim().trim_matches('"').to_string();
                        break;
                    }
                } else if line.contains("ERROR:") && last_error.is_empty() {
                    let new_err = line.replace("ERROR:", "").trim().to_string();
                    if !new_err.is_empty() {
                        last_error = new_err;
                    }
                }
            }
        }

        if let Some(mut child) = self.state.lock().unwrap().process.take() {
            let _ = child.wait();
        }
        self.set_endpoint("");
        if last_error.is_empty() {
            self.set_status("stopped");
        } else {
            self.set_status(&format!("error: {last_error}"));
        }
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.state.lock().unwrap().process.take() {
            self.set_status("stopping");
            util::terminate_child(&mut child, Duration::from_secs(3));
        }
        self.set_endpoint("");
        self.set_status("stopped");
    }
}
