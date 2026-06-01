use crate::{
    constants::DATA_DIR,
    events::{EventEmitter, ManagerEvent},
    util,
};
use regex::Regex;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
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
                port: 8080,
                protocol: "http".to_string(),
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
        None
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

    pub fn install_latest_binary(
        &self,
        progress: Option<Box<dyn Fn(u64, u64) + Send + 'static>>,
    ) -> (bool, String) {
        let sys_name = util::platform_name();
        let machine = util::machine_name();

        let (url, filename) = if sys_name == "windows" {
            (
                "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
                "cloudflared-windows-amd64.exe",
            )
        } else if sys_name == "darwin" {
            if machine == "arm64" || machine == "aarch64" {
                (
                    "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64",
                    "cloudflared-darwin-arm64",
                )
            } else {
                (
                    "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64",
                    "cloudflared-darwin-amd64",
                )
            }
        } else if machine == "arm64" || machine == "aarch64" {
            (
                "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64",
                "cloudflared-linux-arm64",
            )
        } else {
            (
                "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64",
                "cloudflared-linux-amd64",
            )
        };

        let target = self.binary_path();
        if let Some(parent) = target.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return (false, err.to_string());
            }
        }

        let expected_hash = match self.fetch_latest_release() {
            Ok(body) => {
                let prefix = format!("{filename}: ");
                body.lines()
                    .find_map(|line| line.trim().strip_prefix(&prefix))
                    .map(|h| h.trim().to_string())
            }
            Err(e) => return (false, format!("Failed to fetch release checksums: {e}")),
        };

        let payload = match util::download_with_progress(url, 60, |downloaded, total| {
            if let Some(ref cb) = progress {
                cb(downloaded, total);
            }
        }) {
            Ok(data) => data,
            Err(e) => return (false, e),
        };

        if let Some(ref expected) = expected_hash {
            if let Err(e) = util::verify_sha256(&payload, expected) {
                return (false, e);
            }
        }

        if let Err(err) = fs::File::create(&target).and_then(|mut file| file.write_all(&payload)) {
            return (false, format!("Failed to write binary: {err}"));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o700));
        }

        if let Err(e) = util::save_binary_hash(&target, &payload) {
            return (false, e);
        }

        (true, target.to_string_lossy().to_string())
    }

    fn fetch_latest_release(&self) -> Result<String, String> {
        let url = "https://api.github.com/repos/cloudflare/cloudflared/releases/latest";
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let body: String = client
            .get(url)
            .header(reqwest::header::USER_AGENT, util::user_agent())
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|e| format!("failed to fetch release info: {e}"))?;

        let data: Value = serde_json::from_str(&body)
            .map_err(|e| format!("failed to parse release info: {e}"))?;

        let release_body = data
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(release_body)
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

        if util::check_binary_integrity(Path::new(&binary)).is_err() {
            self.set_status("error");
            return false;
        }

        let mut command = Command::new(binary);
        command
            .args(["tunnel", "--url", &format!("localhost:{port}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        util::command_no_window(&mut command);
        util::disable_setuid_on_child(&mut command);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_cloudflare_manager_initial_state() {
        let manager = CloudflareManager::new();
        assert_eq!(manager.status(), "stopped");
        assert_eq!(manager.public_endpoint(), "");
        assert!(!manager.is_running());
    }

    #[test]
    fn test_cloudflare_stream_parsing() {
        let manager = Arc::new(CloudflareManager::new());
        let events = Arc::new(Mutex::new(Vec::new()));

        let events_clone = events.clone();
        manager.connect("endpoint-changed", move |event| {
            if let ManagerEvent::EndpointChanged { endpoint, .. } = event {
                events_clone.lock().unwrap().push(endpoint);
            }
        });

        let simulated_log = "\
2026-05-16T22:37:52Z INF +------------------------------------------------------------+
2026-05-16T22:37:52Z INF |  Your quick Tunnel has been created! Visit link:          |
2026-05-16T22:37:52Z INF |  https://carabiner-test-12345.trycloudflare.com           |
2026-05-16T22:37:52Z INF +------------------------------------------------------------+
";

        let reader = Cursor::new(simulated_log.as_bytes());
        manager.clone().read_stream(Box::new(reader), false);

        assert_eq!(manager.status(), "running");
        assert_eq!(
            manager.public_endpoint(),
            "https://carabiner-test-12345.trycloudflare.com"
        );

        let recorded_events = events.lock().unwrap();
        assert_eq!(recorded_events.len(), 1);
        assert_eq!(
            recorded_events[0],
            "https://carabiner-test-12345.trycloudflare.com"
        );
    }
}
