use crate::{
    constants::DATA_DIR,
    events::{EventEmitter, ManagerEvent},
    util,
};
use regex::Regex;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

struct State {
    process: Option<Child>,
    status: String,
    public_endpoint: String,
    port: u16,
    protocol: String,
}

pub struct CloudflareManager {
    emitter: EventEmitter,
    state: Mutex<State>,
    directory: PathBuf,
}

impl CloudflareManager {
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
            directory: DATA_DIR.join("cloudflare"),
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
            "cloudflared.exe"
        } else {
            "cloudflared"
        })
    }

    pub fn resolve_binary(&self) -> Option<String> {
        let bundled = self.binary_path();
        if bundled.exists() && bundled.is_file() {
            return Some(bundled.to_string_lossy().to_string());
        }
        util::which("cloudflared")
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

        let url = if sys_name == "windows" {
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe"
                .to_string()
        } else if sys_name == "darwin" {
            if machine == "arm64" || machine == "aarch64" {
                "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64"
                    .to_string()
            } else {
                "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64"
                    .to_string()
            }
        } else if machine == "arm64" || machine == "aarch64" {
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64"
                .to_string()
        } else {
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
                .to_string()
        };

        let target = self.binary_path();
        if let Some(parent) = target.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return (false, err.to_string());
            }
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

        if let Err(err) = fs::File::create(&target).and_then(|mut file| file.write_all(&payload)) {
            return (false, err.to_string());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
        }

        (true, target.to_string_lossy().to_string())
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
            .args(["tunnel", "--url", &format!("localhost:{port}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        util::command_no_window(&mut command);

        let Ok(mut child) = command.spawn() else {
            self.set_status("error");
            return false;
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        self.state.lock().unwrap().process = Some(child);

        if let Some(stderr) = stderr {
            let manager = self.clone();
            thread::spawn(move || manager.read_stream(Box::new(BufReader::new(stderr)), false));
        }
        if let Some(stdout) = stdout {
            let manager = self.clone();
            thread::spawn(move || manager.read_stream(Box::new(BufReader::new(stdout)), true));
        }
        true
    }

    fn read_stream(self: Arc<Self>, reader: Box<dyn BufRead + Send>, finalize: bool) {
        let url_re = Regex::new(r"https://[a-zA-Z0-9-]+\.trycloudflare\.com").unwrap();
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(found) = url_re.find(&line) {
                if self.public_endpoint().is_empty() {
                    self.set_endpoint(found.as_str());
                    self.set_status("running");
                }
            }
        }

        if !finalize {
            return;
        }

        if let Some(mut child) = self.state.lock().unwrap().process.take() {
            let _ = child.wait();
        }
        self.set_endpoint("");
        self.set_status("stopped");
    }

    pub fn stop(self: &Arc<Self>) {
        let child = self.state.lock().unwrap().process.take();
        if let Some(mut child) = child {
            self.set_status("stopping");
            let manager = self.clone();
            thread::spawn(move || {
                util::terminate_child(&mut child, std::time::Duration::from_secs(3));
                manager.set_endpoint("");
                manager.set_status("stopped");
            });
            return;
        }

        self.set_endpoint("");
        self.set_status("stopped");
    }
}
