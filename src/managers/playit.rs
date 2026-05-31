#![allow(dead_code)]

use crate::{
    constants::DATA_DIR,
    events::{EventEmitter, ManagerEvent},
    tunnel_store, util,
};
use chrono::{DateTime, Local};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{BufReader, Read, Write},
    net::Ipv4Addr,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static ANSI_ESCAPE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").unwrap());
static ENDPOINT_URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:tcp|udp)://([A-Za-z0-9.-]+:\d{2,5})").unwrap());
static ENDPOINT_HOSTPORT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(((?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}|(?:\d{1,3}\.){3}\d{1,3}):\d{2,5})").unwrap()
});
static SECRET_VALUE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?mi)^\s*(?:secret|secret_key|key)\s*=\s*"([^"]+)"\s*$"#).unwrap());
static VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+)\.(\d+)\.(\d+)").unwrap());
static SAFE_LABEL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9-]").unwrap());
static DASH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"-+").unwrap());
static HEX_LABEL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9a-f-]{32,40}$").unwrap());

#[derive(Clone, Debug)]
pub struct PlayitTunnel {
    pub id: String,
    pub tunnel_type: String,
    pub protocol: String,
    pub status: String,
    pub region: String,
    pub port: Option<u16>,
    pub host: String,
    pub domain: String,
    pub remote_port: Option<u16>,
    pub hostname: String,
    pub created: DateTime<Local>,
    pub in_use: bool,
    cost: u16,
}

impl PlayitTunnel {
    fn from_value(parent: &PlayitManager, tunnel_data: &Value) -> Self {
        let cost = tunnel_data
            .get("port_count")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as u16;
        let id = tunnel_data
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let tunnel_type = tunnel_data
            .get("tunnel_type")
            .and_then(Value::as_str)
            .unwrap_or("both")
            .to_string();
        let protocol = tunnel_data
            .get("port_type")
            .and_then(Value::as_str)
            .unwrap_or("tcp")
            .to_string();
        let status = tunnel_data
            .get("alloc")
            .and_then(|alloc| alloc.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();

        let mut tunnel = Self {
            id,
            tunnel_type,
            protocol,
            status,
            region: String::new(),
            port: None,
            host: String::new(),
            domain: String::new(),
            remote_port: None,
            hostname: String::new(),
            created: Local::now(),
            in_use: false,
            cost,
        };

        if tunnel.status == "pending" {
            return tunnel;
        }

        let alloc_data = tunnel_data
            .get("alloc")
            .and_then(|alloc| alloc.get("data"))
            .unwrap_or(&Value::Null);
        tunnel.region = alloc_data
            .get("region")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if let Some(origin_data) = tunnel_data
            .get("origin")
            .and_then(|origin| origin.get("data"))
        {
            tunnel.port = origin_data
                .get("local_port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            tunnel.host = origin_data
                .get("local_ip")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }

        if tunnel.port.is_none() || tunnel.host.is_empty() {
            let cached = parent.tunnel_cache.get_tunnel(&tunnel.id);
            if let Some(origin_data) = cached.get("origin").and_then(|origin| origin.get("data")) {
                tunnel.port = origin_data
                    .get("local_port")
                    .and_then(Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok());
                tunnel.host = origin_data
                    .get("local_ip")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
        }

        tunnel.domain = alloc_data
            .get("assigned_domain")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        tunnel.remote_port = alloc_data
            .get("port_start")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok());

        if tunnel.tunnel_type == "both" {
            if let Some(remote_port) = tunnel.remote_port {
                tunnel.hostname = format!("{}:{remote_port}", tunnel.domain);
            }
        } else {
            tunnel.hostname = tunnel.domain.clone();
        }

        if let Some(raw_date) = tunnel_data.get("created_at").and_then(Value::as_str) {
            if let Ok(date) = DateTime::parse_from_rfc3339(&raw_date.replace('Z', "+00:00")) {
                tunnel.created = date.with_timezone(&Local);
            }
        }

        tunnel
    }
}

struct TunnelCacheHelper {
    path: PathBuf,
    data: Mutex<HashMap<String, Value>>,
}

impl TunnelCacheHelper {
    fn new(root_path: PathBuf) -> Self {
        let path = root_path.join("tunnel-cache.json");
        let data = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, Value>>(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    fn write_data(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let data = self.data.lock().unwrap().clone();
        if let Ok(text) = serde_json::to_string(&data) {
            let _ = fs::write(&self.path, text);
        }
    }

    fn clear_cache(&self) {
        let _ = fs::remove_file(&self.path);
        self.data.lock().unwrap().clear();
    }

    fn add_tunnel(&self, tunnel_id: &str, data: Value) -> bool {
        self.data
            .lock()
            .unwrap()
            .insert(tunnel_id.to_string(), data);
        self.write_data();
        self.data.lock().unwrap().contains_key(tunnel_id)
    }

    fn remove_tunnel(&self, tunnel_id: &str) -> bool {
        self.data.lock().unwrap().remove(tunnel_id);
        self.write_data();
        !self.data.lock().unwrap().contains_key(tunnel_id)
    }

    fn get_tunnel(&self, tunnel_id: &str) -> Value {
        self.data
            .lock()
            .unwrap()
            .get(tunnel_id)
            .cloned()
            .unwrap_or(Value::Object(Map::new()))
    }
}

struct State {
    process: Option<Child>,
    server_id: Option<String>,
    status: String,
    public_endpoint: String,
    claim_url: String,
    config: HashMap<String, String>,
    initialized: bool,
    agent_id: Option<String>,
    proto_key: Option<String>,
    secret_key: Option<String>,
    active_tunnel_id: Option<String>,
    last_error: String,
    agent_web_url: String,
    tunnels: HashMap<String, Vec<PlayitTunnel>>,
}

pub struct PlayitManager {
    emitter: EventEmitter,
    state: Mutex<State>,
    client: reqwest::blocking::Client,
    directory: PathBuf,
    toml_path: PathBuf,
    tunnel_cache: TunnelCacheHelper,
    git_base: String,
    api_base: String,
    web_base: String,
    link_worker_url: String,
    setup_url: String,
    agent_name: String,
    max_tunnels: u16,
}

impl PlayitManager {
    pub fn new() -> Self {
        let directory = DATA_DIR.join("playit");
        let toml_path = directory.join("playit.toml");
        let tunnels = HashMap::from([
            ("tcp".to_string(), Vec::new()),
            ("udp".to_string(), Vec::new()),
            ("both".to_string(), Vec::new()),
        ]);

        Self {
            emitter: EventEmitter::default(),
            state: Mutex::new(State {
                process: None,
                server_id: None,
                status: "stopped".to_string(),
                public_endpoint: String::new(),
                claim_url: String::new(),
                config: HashMap::new(),
                initialized: false,
                agent_id: None,
                proto_key: None,
                secret_key: None,
                active_tunnel_id: None,
                last_error: String::new(),
                agent_web_url: String::new(),
                tunnels,
            }),
            client: reqwest::blocking::Client::new(),
            directory: directory.clone(),
            toml_path,
            tunnel_cache: TunnelCacheHelper::new(directory),
            git_base: "https://github.com/playit-cloud/playit-agent/releases".to_string(),
            api_base: "https://api.playit.gg".to_string(),
            web_base: "https://playit.gg".to_string(),
            link_worker_url: "https://playit.auto-mcs.com/link".to_string(),
            setup_url: "https://playit.gg/account/setup/wizard/new-account/third-party/third-party-code?partner=carabiner".to_string(),
            agent_name: format!("carabiner ({})", util::hostname()),
            max_tunnels: 4,
        }
    }

    pub fn setup_url(&self) -> &str {
        &self.setup_url
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

    pub fn claim_url(&self) -> String {
        self.state.lock().unwrap().claim_url.clone()
    }

    pub fn server_id(&self) -> Option<String> {
        self.state.lock().unwrap().server_id.clone()
    }

    pub fn initialized(&self) -> bool {
        self.state.lock().unwrap().initialized
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

    pub fn binary_path(&self) -> PathBuf {
        self.directory.join(if cfg!(target_os = "windows") {
            "playit.exe"
        } else {
            "playit"
        })
    }

    pub fn resolve_binary(&self) -> Option<String> {
        let bundled = self.binary_path();
        if bundled.exists() && bundled.is_file() {
            return Some(bundled.to_string_lossy().to_string());
        }
        util::which("playit")
    }

    pub fn is_installed(&self) -> bool {
        self.resolve_binary().is_some()
    }

    pub fn tunnels_for(&self, protocol: &str) -> Vec<PlayitTunnel> {
        self.state
            .lock()
            .unwrap()
            .tunnels
            .get(protocol)
            .cloned()
            .unwrap_or_default()
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

    fn emit_endpoint_changed(&self) {
        let (endpoint, claim_url) = {
            let state = self.state.lock().unwrap();
            (state.public_endpoint.clone(), state.claim_url.clone())
        };
        self.emitter.emit(
            "endpoint-changed",
            ManagerEvent::EndpointChanged {
                endpoint,
                claim_url,
            },
        );
    }

    fn request(&self, endpoint: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}/{}", self.api_base, endpoint.trim_matches('/'));
        let secret = self.state.lock().unwrap().secret_key.clone();
        let mut request = self
            .client
            .post(url)
            .timeout(Duration::from_secs(20))
            .header(reqwest::header::USER_AGENT, util::user_agent());
        if let Some(secret) = secret {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("agent-key {secret}"),
            );
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().map_err(|err| err.to_string())?;
        let status = response.status();
        let text = response.text().map_err(|err| err.to_string())?;
        if !status.is_success() {
            let mut body = text.trim().to_string();
            if body.len() > 240 {
                body.truncate(240);
                body.push_str("...");
            }
            if body.is_empty() {
                return Err(format!("HTTP {}", status.as_u16()));
            }
            return Err(format!("HTTP {}: {body}", status.as_u16()));
        }

        let payload = serde_json::from_str::<Value>(&text).map_err(|_| {
            let mut body = text.trim().to_string();
            if body.len() > 240 {
                body.truncate(240);
                body.push_str("...");
            }
            format!("Invalid JSON response: {body}")
        })?;
        if !payload.is_object() {
            return Err("Invalid playit API response".to_string());
        }
        Ok(payload)
    }

    fn load_config(&self) -> bool {
        if !self.toml_path.exists() {
            return false;
        }
        let Ok(text) = fs::read_to_string(&self.toml_path) else {
            return false;
        };

        let mut data = HashMap::new();
        for line in text.lines() {
            if !line.contains('=') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().trim_matches(['\'', '"']).to_string();
            let value = value.trim().trim_matches(['\'', '"']).to_string();
            if !key.is_empty() {
                data.insert(key, value);
            }
        }

        let has_config = !data.is_empty();
        self.state.lock().unwrap().config = data;
        has_config
    }

    fn write_secret_key(&self, secret_key: &str) -> bool {
        let key = secret_key.trim();
        if key.is_empty() {
            return false;
        }

        if fs::create_dir_all(&self.directory).is_err() {
            return false;
        }
        if fs::write(&self.toml_path, format!("secret_key = \"{key}\"\n")).is_err() {
            return false;
        }

        let mut state = self.state.lock().unwrap();
        state.config = HashMap::from([("secret_key".to_string(), key.to_string())]);
        state.secret_key = Some(key.to_string());
        true
    }

    fn reset_config(&self) -> bool {
        let ok = fs::remove_file(&self.toml_path).is_ok() || !self.toml_path.exists();
        if ok {
            let mut state = self.state.lock().unwrap();
            state.config.clear();
            state.secret_key = None;
        }
        ok
    }

    pub fn secret_path(&self) -> Option<PathBuf> {
        let binary = self.resolve_binary()?;
        let output = Command::new(binary)
            .args(["--stdout", "secret-path"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            return None;
        }
        text.lines()
            .last()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
    }

    pub fn read_claimed_secret(&self) -> String {
        if self.load_config() {
            let secret = self
                .state
                .lock()
                .unwrap()
                .config
                .get("secret_key")
                .cloned()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !secret.is_empty() {
                return secret;
            }
        }

        let Some(path) = self.secret_path() else {
            return String::new();
        };
        if !path.exists() || !path.is_file() {
            return String::new();
        }

        let Ok(text) = fs::read_to_string(path) else {
            return String::new();
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return String::new();
        }

        if let Some(captures) = SECRET_VALUE_RE.captures(&text) {
            return captures
                .get(1)
                .map(|value| value.as_str().trim().to_string())
                .unwrap_or_default();
        }

        let lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if lines.len() == 1 && !lines[0].contains('=') {
            return lines[0].to_string();
        }

        String::new()
    }

    pub fn has_claimed_secret(&self) -> bool {
        !self.read_claimed_secret().is_empty()
    }

    fn detect_version(&self, binary: &str) -> (u64, u64, u64) {
        let output = Command::new(binary)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(captures) = VERSION_RE.captures(&text) {
                let major = captures
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                let minor = captures
                    .get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(17);
                let patch = captures
                    .get(3)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(1);
                return (major, minor, patch);
            }
        }
        (0, 17, 1)
    }

    pub fn install_latest_binary(
        &self,
        progress: Option<Box<dyn Fn(u64, u64) + Send + 'static>>,
    ) -> (bool, String) {
        let release_url = "https://api.github.com/repos/playit-cloud/playit-agent/releases/tags/v0.17.1";
        let response = self
            .client
            .get(release_url)
            .header(reqwest::header::USER_AGENT, util::user_agent())
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json::<Value>());

        let Ok(data) = response else {
            return (
                false,
                response.err().map_or_else(
                    || "Failed to fetch release info".to_string(),
                    |err| {
                        let msg = err.to_string();
                        if msg.contains("timed out") {
                            "Release info request timed out. Check your internet connection."
                                .to_string()
                        } else {
                            format!("Failed to fetch release info: {msg}")
                        }
                    },
                ),
            );
        };

        let Some(assets) = data.get("assets").and_then(Value::as_array) else {
            return (false, "Release assets unavailable".to_string());
        };

        let Some(asset) = self.select_asset(assets) else {
            return (
                false,
                "No compatible playit build found for this platform".to_string(),
            );
        };

        let download_url = asset
            .get("browser_download_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if download_url.is_empty() {
            return (false, "Download URL missing".to_string());
        }

        let target = self.binary_path();
        if let Some(parent) = target.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return (false, err.to_string());
            }
        }

        let payload = match util::download_with_progress(&download_url, 120, |downloaded, total| {
            if let Some(ref cb) = progress {
                cb(downloaded, total);
            }
        }) {
            Ok(data) => data,
            Err(e) => return (false, e),
        };

        if let Err(err) = fs::File::create(&target).and_then(|mut file| file.write_all(&payload)) {
            return (false, format!("Failed to write binary: {err}"));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
        }

        (true, target.to_string_lossy().to_string())
    }

    fn select_asset(&self, assets: &[Value]) -> Option<Value> {
        let sys_name = util::platform_name();
        let machine = util::machine_name();

        let arch_keys: Vec<&str> = match machine.as_str() {
            "x86_64" | "amd64" => vec!["amd64", "x86_64", "x64"],
            "aarch64" | "arm64" => vec!["aarch64", "arm64"],
            _ => vec![machine.as_str()],
        };

        let (os_keys, required_ext): (Vec<&str>, &str) = if sys_name.contains("windows") {
            (vec!["windows", "win"], ".exe")
        } else if sys_name.contains("darwin") || sys_name.contains("mac") {
            (vec!["mac", "darwin", "osx"], "")
        } else {
            (vec!["linux"], "")
        };

        let mut candidates = Vec::new();
        for asset in assets {
            let name = asset
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            if name.is_empty() || name.ends_with(".sha256") || name.ends_with(".sig") {
                continue;
            }
            if !os_keys.iter().any(|key| name.contains(key)) {
                continue;
            }
            if !arch_keys.iter().any(|key| name.contains(key)) {
                continue;
            }
            if !required_ext.is_empty() && !name.ends_with(required_ext) {
                continue;
            }
            candidates.push(asset.clone());
        }

        if let Some(first) = candidates.into_iter().next() {
            return Some(first);
        }

        if os_keys.contains(&"linux") {
            for asset in assets {
                let name = asset
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                if name.starts_with("playit-linux") && !name.ends_with(".sha256") {
                    return Some(asset.clone());
                }
            }
        }

        None
    }

    fn proto_register(&self) -> bool {
        let Some(binary) = self.resolve_binary() else {
            return false;
        };
        let (major, minor, patch) = self.detect_version(&binary);
        let proto_data = json!({
            "agent_version": {
                "official": true,
                "details_website": null,
                "version": {
                    "platform": util::playit_platform_name(),
                    "version": format!("{major}.{minor}.{patch}"),
                },
            },
            "client_addr": "0.0.0.0:0",
            "tunnel_addr": "0.0.0.0:0",
        });

        let Ok(response) = self.request("proto/register", Some(proto_data)) else {
            return false;
        };
        if response.get("status").and_then(Value::as_str) == Some("success") {
            let proto_key = response
                .get("data")
                .and_then(|data| data.get("key"))
                .and_then(Value::as_str)
                .map(str::to_string);
            self.state.lock().unwrap().proto_key = proto_key;
        }
        self.state.lock().unwrap().proto_key.is_some()
    }

    fn is_invalid_agent_key_error(&self, detail: &str) -> bool {
        let lowered = detail.to_lowercase();
        lowered.contains("invalidagentkey") || (lowered.contains("401") && lowered.contains("auth"))
    }

    pub fn link_account(&self, setup_code: &str) -> (bool, String) {
        let code = setup_code.trim();
        if code.is_empty() {
            return (false, "Missing playit setup code".to_string());
        }

        let Some(binary) = self.resolve_binary() else {
            return (false, "playit binary not found".to_string());
        };

        let (major, minor, patch) = self.detect_version(&binary);
        let payload = json!({
            "account_setup_code": code,
            "agent_name": self.agent_name,
            "platform": util::playit_platform_name(),
            "version_major": major,
            "version_minor": minor,
            "version_patch": patch,
        });

        let response = reqwest::blocking::Client::new()
            .post(&self.link_worker_url)
            .json(&payload)
            .timeout(Duration::from_secs(20))
            .send();

        let Ok(response) = response else {
            return (
                false,
                format!(
                    "Failed to reach playit link service: {}",
                    response.err().unwrap()
                ),
            );
        };

        let status = response.status();
        let raw_text = response.text().unwrap_or_default();
        let data = serde_json::from_str::<Value>(&raw_text);
        let Ok(data) = data else {
            return (
                false,
                format!(
                    "Link service returned invalid JSON (HTTP {}): {raw_text}",
                    status.as_u16()
                ),
            );
        };

        if status.as_u16() >= 400 {
            let error_detail = data
                .get("error")
                .or_else(|| data.get("message"))
                .or_else(|| data.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or(&raw_text);
            return (
                false,
                format!(
                    "Link service returned HTTP {}: {error_detail}",
                    status.as_u16()
                ),
            );
        }

        if data.get("status").and_then(Value::as_str).unwrap_or("fail") == "success" {
            let payload_data = data.get("data").unwrap_or(&Value::Null);
            let mut state = self.state.lock().unwrap();
            state.agent_id = payload_data
                .get("agent_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            state.secret_key = payload_data
                .get("agent_secret_key")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
        }

        let secret_key = self
            .state
            .lock()
            .unwrap()
            .secret_key
            .clone()
            .unwrap_or_default();
        if secret_key.is_empty() {
            return (false, format!("Link service did not return a key: {data}"));
        }

        if !self.write_secret_key(&secret_key) {
            return (false, "Failed to write playit.toml".to_string());
        }

        {
            let mut state = self.state.lock().unwrap();
            state.claim_url.clear();
            state.initialized = false;
        }
        self.emit_endpoint_changed();

        if self.initialize_with_retry(15, Duration::from_secs(1)) {
            return (true, "playit account linked".to_string());
        }

        let last_error = self.state.lock().unwrap().last_error.clone();
        if self.is_invalid_agent_key_error(&last_error) {
            self.unlink_account();
            return (
                false,
                "playit rejected the linked key (InvalidAgentKey). Please generate a new setup code and try again"
                    .to_string(),
            );
        }

        (true, "playit account linked (API sync pending)".to_string())
    }

    pub fn validate_existing_link(&self, retry_attempts: usize) -> (bool, String) {
        if self.read_claimed_secret().is_empty() {
            return (false, "not linked".to_string());
        }

        if self.initialize_with_retry(retry_attempts.max(1), Duration::from_millis(500)) {
            return (true, "linked".to_string());
        }

        let detail = {
            let state = self.state.lock().unwrap();
            if state.last_error.is_empty() {
                "unknown error".to_string()
            } else {
                state.last_error.clone()
            }
        };

        if self.is_invalid_agent_key_error(&detail) {
            self.unlink_account();
            return (false, "linked key is invalid and was cleared".to_string());
        }

        (false, detail)
    }

    pub fn unlink_account(&self) -> bool {
        let reset_ok = self.reset_config();
        {
            let mut state = self.state.lock().unwrap();
            state.agent_id = None;
            state.proto_key = None;
            state.secret_key = None;
            state.initialized = false;
            state.tunnels = empty_tunnel_map();
        }
        self.tunnel_cache.clear_cache();
        reset_ok
    }

    pub fn initialize(&self) -> bool {
        {
            let mut state = self.state.lock().unwrap();
            state.last_error.clear();
        }

        if self.resolve_binary().is_none() {
            let mut state = self.state.lock().unwrap();
            state.last_error = "playit binary not found".to_string();
            return false;
        }

        let secret = self.read_claimed_secret();
        if secret.is_empty() {
            let mut state = self.state.lock().unwrap();
            state.initialized = false;
            state.last_error = "playit secret key not found".to_string();
            return false;
        }

        self.state.lock().unwrap().secret_key = Some(secret);

        match self.request("agents/rundata", None) {
            Ok(agent_data) => {
                let agent_id = agent_data
                    .get("data")
                    .and_then(|data| data.get("agent_id"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                let Some(agent_id) = agent_id else {
                    let mut state = self.state.lock().unwrap();
                    state.initialized = false;
                    state.last_error = "agents/rundata did not include agent_id".to_string();
                    return false;
                };

                {
                    let mut state = self.state.lock().unwrap();
                    state.agent_id = Some(agent_id.clone());
                    state.agent_web_url = format!("{}/account/agents/{agent_id}", self.web_base);
                }
                self.proto_register();
                self.retrieve_tunnels();
                let mut state = self.state.lock().unwrap();
                state.initialized = true;
                state.last_error.clear();
                true
            }
            Err(err) => {
                let mut state = self.state.lock().unwrap();
                state.initialized = false;
                state.last_error = err;
                false
            }
        }
    }

    fn initialize_with_retry(&self, max_attempts: usize, delay: Duration) -> bool {
        for attempt in 0..max_attempts.max(1) {
            if self.initialize() {
                return true;
            }
            if attempt + 1 < max_attempts {
                thread::sleep(delay);
            }
        }
        false
    }

    fn retrieve_tunnels(&self) -> HashMap<String, Vec<PlayitTunnel>> {
        {
            let mut state = self.state.lock().unwrap();
            state.tunnels = empty_tunnel_map();
        }

        let agent_id = self.state.lock().unwrap().agent_id.clone();
        let Some(agent_id) = agent_id else {
            return empty_tunnel_map();
        };

        let Ok(data) = self.request("tunnels/list", Some(json!({ "agent_id": agent_id }))) else {
            return self.state.lock().unwrap().tunnels.clone();
        };

        if data.get("status").and_then(Value::as_str) != Some("success") {
            return self.state.lock().unwrap().tunnels.clone();
        }

        let tunnel_items = data
            .get("data")
            .and_then(|payload| payload.get("tunnels"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut tunnels = empty_tunnel_map();
        for tunnel_data in tunnel_items {
            let tunnel = PlayitTunnel::from_value(self, &tunnel_data);
            let key = if tunnels.contains_key(&tunnel.protocol) {
                tunnel.protocol.clone()
            } else {
                "tcp".to_string()
            };
            tunnels.entry(key).or_default().push(tunnel);
        }

        self.state.lock().unwrap().tunnels = tunnels.clone();
        tunnels
    }

    fn return_single_list(&self) -> Vec<PlayitTunnel> {
        let state = self.state.lock().unwrap();
        let mut out = Vec::new();
        out.extend(state.tunnels.get("tcp").cloned().unwrap_or_default());
        out.extend(state.tunnels.get("udp").cloned().unwrap_or_default());
        out.extend(state.tunnels.get("both").cloned().unwrap_or_default());
        out
    }

    fn check_tunnel_limit(&self) -> bool {
        let state = self.state.lock().unwrap();
        let mut tunnel_count = 0u16;
        for key in ["both", "tcp", "udp"] {
            tunnel_count += state
                .tunnels
                .get(key)
                .into_iter()
                .flat_map(|items| items.iter())
                .map(|tunnel| tunnel.cost)
                .sum::<u16>();
        }
        tunnel_count < self.max_tunnels
    }

    fn read_server_port(&self, server_dir: &str) -> u16 {
        let default_port = 25565;
        let path = PathBuf::from(server_dir).join("server.properties");
        let Ok(text) = fs::read_to_string(path) else {
            return default_port;
        };

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("server-port=") {
                if let Ok(parsed) = value.trim().parse::<u16>() {
                    if (1024..=65535).contains(&parsed) {
                        return parsed;
                    }
                    return default_port;
                }
            }
        }
        default_port
    }

    fn create_tunnel(
        &self,
        mut port: u16,
        protocol: &str,
        label: &str,
    ) -> Result<Option<PlayitTunnel>, String> {
        if !(1024..65535).contains(&port) {
            port = match protocol {
                "udp" => 19132,
                _ => 25565,
            };
        }

        if !self.check_tunnel_limit() {
            return Err(format!(
                "This account cannot create more than {} tunnel(s)",
                self.max_tunnels
            ));
        }

        let tunnel_type = match protocol {
            "tcp" => Value::String("minecraft-java".to_string()),
            "udp" => Value::String("minecraft-bedrock".to_string()),
            "both" => Value::Null,
            _ => Value::String("minecraft-java".to_string()),
        };

        let mut safe_label = SAFE_LABEL_RE
            .replace_all(&label.trim().to_lowercase(), "-")
            .to_string();
        safe_label = DASH_RE
            .replace_all(&safe_label, "-")
            .trim_matches('-')
            .to_string();
        if !safe_label.is_empty() && HEX_LABEL_RE.is_match(&safe_label) {
            safe_label = "server".to_string();
        }
        if safe_label.is_empty() {
            safe_label = "server".to_string();
        }
        safe_label.truncate(24);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() % 100000)
            .unwrap_or_default();
        let tunnel_name = format!("{safe_label}-{protocol}-{port}-{timestamp}");

        let agent_id = self
            .state
            .lock()
            .unwrap()
            .agent_id
            .clone()
            .unwrap_or_default();
        let tunnel_data = json!({
            "name": tunnel_name,
            "tunnel_type": tunnel_type,
            "port_type": protocol,
            "port_count": if protocol == "both" { 2 } else { 1 },
            "enabled": true,
            "origin": {
                "type": "agent",
                "data": {
                    "agent_id": agent_id,
                    "local_ip": "127.0.0.1",
                    "local_port": port,
                },
            },
        });

        self.set_status("creating");
        let result = (|| {
            let data = self
                .request("tunnels/create", Some(tunnel_data.clone()))
                .ok()?;
            let tunnel_id = data
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if tunnel_id.is_empty() {
                return None;
            }

            self.tunnel_cache.add_tunnel(&tunnel_id, tunnel_data);

            for _ in 0..30 {
                self.retrieve_tunnels();
                for tunnel in self.tunnels_for(protocol) {
                    if tunnel.id == tunnel_id && !tunnel.hostname.is_empty() {
                        return Some(tunnel);
                    }
                }
                thread::sleep(Duration::from_secs(1));
            }
            None
        })();

        if self.is_running() {
            self.set_status("running");
        } else {
            self.set_status("stopped");
        }
        Ok(result)
    }

    fn delete_tunnel(&self, tunnel: &PlayitTunnel) -> bool {
        let Ok(status) = self.request("tunnels/delete", Some(json!({ "tunnel_id": tunnel.id })))
        else {
            return false;
        };
        if status.get("status").and_then(Value::as_str) != Some("success") {
            return false;
        }

        self.tunnel_cache.remove_tunnel(&tunnel.id);
        let mut state = self.state.lock().unwrap();
        if let Some(bucket) = state.tunnels.get_mut(&tunnel.protocol) {
            bucket.retain(|item| item.id != tunnel.id);
            return bucket.iter().all(|item| item.id != tunnel.id);
        }
        true
    }

    pub fn delete_tunnels(&self, port: u16, protocol: &str) -> usize {
        self.retrieve_tunnels();
        let bucket = self.tunnels_for(protocol);
        let mut deleted_count = 0;
        for tunnel in bucket {
            if tunnel.port == Some(port) && self.delete_tunnel(&tunnel) {
                deleted_count += 1;
            }
        }
        deleted_count
    }

    pub fn get_tunnel(
        &self,
        port: u16,
        protocol: &str,
        ensure: bool,
        label: &str,
    ) -> Result<Option<PlayitTunnel>, String> {
        self.retrieve_tunnels();

        for tunnel in self.tunnels_for(protocol) {
            if tunnel.port == Some(port) && !tunnel.in_use {
                if !tunnel.hostname.is_empty() {
                    return Ok(Some(tunnel));
                }
                if tunnel.status == "pending" {
                    continue;
                }
            }
        }

        if !ensure {
            return Ok(None);
        }

        if !self.check_tunnel_limit() {
            let mut all_tunnels = self.return_single_list();
            all_tunnels.sort_by_key(|tunnel| tunnel.created);
            for tunnel in all_tunnels {
                self.delete_tunnel(&tunnel);
                if self.check_tunnel_limit() {
                    break;
                }
            }
        }

        self.create_tunnel(port, protocol, label)
    }

    fn start_agent_service(self: &Arc<Self>, binary: &str) -> bool {
        if self.is_running() {
            return true;
        }

        let mut command = Command::new(binary);
        command
            .args(["-s", "--secret_path", &self.toml_path.to_string_lossy()])
            .current_dir(&self.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        util::command_no_window(&mut command);

        let Ok(mut child) = command.spawn() else {
            self.state.lock().unwrap().process = None;
            return false;
        };
        let stdout = child.stdout.take();
        self.state.lock().unwrap().process = Some(child);

        let manager = self.clone();
        thread::spawn(move || manager.read_output(stdout));
        true
    }

    pub fn start(
        self: &Arc<Self>,
        port: u16,
        protocol: &str,
        secret: &str,
        auto_install: bool,
        allow_unclaimed: bool,
    ) -> (bool, String) {
        if self.is_running() {
            return (true, "playit is already running".to_string());
        }

        let mut binary = self.resolve_binary();
        if binary.is_none() && auto_install {
            let (ok, msg) = self.install_latest_binary(None);
            if !ok {
                return (false, format!("playit install failed: {msg}"));
            }
            binary = self.resolve_binary();
        }
        let Some(binary) = binary else {
            return (false, "playit binary not found".to_string());
        };

        self.set_status("starting");
        let provided_secret = secret.trim();
        let mut existing_secret = self.read_claimed_secret();
        if !provided_secret.is_empty()
            && existing_secret.is_empty()
            && self.write_secret_key(provided_secret)
        {
            existing_secret = provided_secret.to_string();
        }

        if existing_secret.is_empty() {
            if !allow_unclaimed {
                return (false, "playit is not linked yet".to_string());
            }
            self.state.lock().unwrap().claim_url = self.setup_url.clone();
            self.emit_endpoint_changed();
            return (true, "playit setup is required".to_string());
        }

        if !self.initialized() && !self.initialize_with_retry(25, Duration::from_secs(1)) {
            let detail = self.state.lock().unwrap().last_error.clone();
            let detail = if detail.is_empty() {
                "unknown error".to_string()
            } else {
                detail
            };
            if self.is_invalid_agent_key_error(&detail) {
                self.unlink_account();
                if allow_unclaimed {
                    self.state.lock().unwrap().claim_url = self.setup_url.clone();
                    self.emit_endpoint_changed();
                    return (true, "playit key invalid, setup is required".to_string());
                }
                return (
                    false,
                    "linked playit key is invalid; run setup again".to_string(),
                );
            }
            return (
                false,
                format!("failed to initialize playit API session: {detail}"),
            );
        }

        let tunnel = match self.get_tunnel(port, protocol, true, "carabiner") {
            Ok(tunnel) => tunnel,
            Err(msg) => return (false, msg),
        };

        let Some(tunnel) = tunnel else {
            return (false, "failed to allocate a playit tunnel".to_string());
        };

        self.mark_tunnel_in_use(&tunnel.id, true);
        {
            let mut state = self.state.lock().unwrap();
            state.active_tunnel_id = Some(tunnel.id.clone());
            if !tunnel.hostname.is_empty() {
                state.public_endpoint = tunnel.hostname.clone();
            }
        }
        if !tunnel.hostname.is_empty() {
            self.emit_endpoint_changed();
        }

        if !self.start_agent_service(&binary) {
            self.mark_tunnel_in_use(&tunnel.id, false);
            self.state.lock().unwrap().active_tunnel_id = None;
            return (false, "failed to start playit agent".to_string());
        }

        self.state.lock().unwrap().claim_url.clear();
        self.set_status("running");
        (true, "playit started".to_string())
    }

    pub fn start_agent(
        self: &Arc<Self>,
        tunnel_configs: Option<Vec<tunnel_store::TunnelConfig>>,
    ) -> (bool, String) {
        if self.is_running() {
            return (true, "playit agent is already running".to_string());
        }

        let Some(binary) = self.resolve_binary() else {
            return (false, "playit binary not found".to_string());
        };

        self.set_status("starting");

        if self.read_claimed_secret().is_empty() {
            self.set_status("stopped");
            return (false, "playit is not linked yet".to_string());
        }

        if !self.initialized() && !self.initialize_with_retry(25, Duration::from_secs(1)) {
            let detail = self.state.lock().unwrap().last_error.clone();
            let detail = if detail.is_empty() {
                "unknown error".to_string()
            } else {
                detail
            };
            if self.is_invalid_agent_key_error(&detail) {
                self.unlink_account();
                self.set_status("stopped");
                return (
                    false,
                    "linked playit key is invalid; run setup again".to_string(),
                );
            }
            self.set_status("stopped");
            return (
                false,
                format!("failed to initialize playit API session: {detail}"),
            );
        }

        self.retrieve_tunnels();
        let tunnel_configs = tunnel_configs.unwrap_or_else(tunnel_store::load_tunnels);
        for config in tunnel_configs {
            if config.provider.to_lowercase() == "playit" {
                let _ = self.get_tunnel(
                    config.port,
                    &config.protocol.to_lowercase(),
                    true,
                    if config.label.is_empty() {
                        "carabiner"
                    } else {
                        &config.label
                    },
                );
            }
        }

        if !self.start_agent_service(&binary) {
            self.set_status("stopped");
            return (false, "failed to start playit agent".to_string());
        }

        self.state.lock().unwrap().claim_url.clear();
        self.set_status("running");
        (true, "playit agent started".to_string())
    }

    fn ensure_api_ready(&self, secret: &str, auto_install: bool) -> (bool, String) {
        let mut binary = self.resolve_binary();
        if binary.is_none() && auto_install {
            let (ok, msg) = self.install_latest_binary(None);
            if !ok {
                return (false, format!("playit install failed: {msg}"));
            }
            binary = self.resolve_binary();
        }
        if binary.is_none() {
            return (false, "playit binary not found".to_string());
        }

        let provided_secret = secret.trim();
        let mut existing_secret = self.read_claimed_secret();
        if !provided_secret.is_empty()
            && existing_secret.is_empty()
            && self.write_secret_key(provided_secret)
        {
            existing_secret = provided_secret.to_string();
        }
        if existing_secret.is_empty() {
            return (false, "playit is not linked yet".to_string());
        }

        if !self.initialized() && !self.initialize_with_retry(25, Duration::from_secs(1)) {
            let detail = self.state.lock().unwrap().last_error.clone();
            let detail = if detail.is_empty() {
                "unknown error".to_string()
            } else {
                detail
            };
            if self.is_invalid_agent_key_error(&detail) {
                self.unlink_account();
                return (
                    false,
                    "linked playit key is invalid; run setup again".to_string(),
                );
            }
            return (
                false,
                format!("failed to initialize playit API session: {detail}"),
            );
        }

        (true, String::new())
    }

    fn resolve_tunnel_port(&self, server_dir: &str, protocol: &str, bedrock_port: u16) -> u16 {
        if protocol == "tcp" {
            return self.read_server_port(server_dir);
        }
        if (1024..=65535).contains(&bedrock_port) {
            bedrock_port
        } else {
            19132
        }
    }

    fn list_tunnels_for_port(&self, port: u16, protocol: &str) -> Vec<PlayitTunnel> {
        self.retrieve_tunnels();
        self.tunnels_for(protocol)
            .into_iter()
            .filter(|tunnel| tunnel.port == Some(port))
            .collect()
    }

    fn add_tunnel_for_protocol(
        &self,
        server_id: &str,
        server_dir: &str,
        protocol: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String, String) {
        let (ok, msg) = self.ensure_api_ready(secret, auto_install);
        if !ok {
            return (false, msg, String::new());
        }

        let port = self.resolve_tunnel_port(server_dir, protocol, bedrock_port);
        let tunnel_label = if protocol == "tcp" {
            server_id.to_string()
        } else {
            format!("{server_id}-bedrock")
        };

        let tunnel = match self.get_tunnel(port, protocol, true, &tunnel_label) {
            Ok(tunnel) => tunnel,
            Err(msg) => return (false, msg, String::new()),
        };

        let Some(tunnel) = tunnel else {
            return (
                false,
                format!(
                    "failed to allocate a {} playit tunnel",
                    protocol.to_uppercase()
                ),
                String::new(),
            );
        };

        let endpoint = tunnel.hostname.trim().to_string();
        let tunnel_name = if protocol == "tcp" { "Java" } else { "Bedrock" };
        if endpoint.is_empty() {
            (
                true,
                format!(
                    "{tunnel_name} tunnel created on {} port {port}",
                    protocol.to_uppercase()
                ),
                String::new(),
            )
        } else {
            (
                true,
                format!("{tunnel_name} tunnel ready: {endpoint}"),
                endpoint,
            )
        }
    }

    fn regenerate_tunnel_for_protocol(
        &self,
        server_id: &str,
        server_dir: &str,
        protocol: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String, String) {
        let (ok, msg) = self.ensure_api_ready(secret, auto_install);
        if !ok {
            return (false, msg, String::new());
        }

        let port = self.resolve_tunnel_port(server_dir, protocol, bedrock_port);
        let candidates = self.list_tunnels_for_port(port, protocol);
        let mut deleted_any = false;
        for tunnel in candidates {
            if self.delete_tunnel(&tunnel) {
                deleted_any = true;
            }
        }

        let (ok, msg, endpoint) = self.add_tunnel_for_protocol(
            server_id,
            server_dir,
            protocol,
            secret,
            auto_install,
            bedrock_port,
        );
        if !ok {
            return (false, msg, String::new());
        }

        let tunnel_name = if protocol == "tcp" { "Java" } else { "Bedrock" };
        if deleted_any && !endpoint.is_empty() {
            (
                true,
                format!("{tunnel_name} tunnel domain regenerated: {endpoint}"),
                endpoint,
            )
        } else if deleted_any {
            (
                true,
                format!("{tunnel_name} tunnel domain regenerated"),
                endpoint,
            )
        } else {
            (true, msg, endpoint)
        }
    }

    fn delete_tunnel_for_protocol(
        &self,
        server_dir: &str,
        protocol: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String) {
        let (ok, msg) = self.ensure_api_ready(secret, auto_install);
        if !ok {
            return (false, msg);
        }

        let port = self.resolve_tunnel_port(server_dir, protocol, bedrock_port);
        let candidates = self.list_tunnels_for_port(port, protocol);
        let tunnel_name = if protocol == "tcp" { "Java" } else { "Bedrock" };
        if candidates.is_empty() {
            return (
                false,
                format!("No {} tunnel found", tunnel_name.to_lowercase()),
            );
        }

        let mut deleted_any = false;
        let mut deleted_hostnames = Vec::new();
        let mut deleted_ids = Vec::new();
        for tunnel in candidates {
            if self.delete_tunnel(&tunnel) {
                deleted_any = true;
                if !tunnel.hostname.is_empty() {
                    deleted_hostnames.push(tunnel.hostname.clone());
                }
                if !tunnel.id.is_empty() {
                    deleted_ids.push(tunnel.id.clone());
                }
            }
        }

        if !deleted_any {
            return (
                false,
                format!("Failed to delete {} tunnel", tunnel_name.to_lowercase()),
            );
        }

        {
            let mut state = self.state.lock().unwrap();
            if state
                .active_tunnel_id
                .as_ref()
                .map(|id| deleted_ids.contains(id))
                .unwrap_or(false)
            {
                state.active_tunnel_id = None;
                state.public_endpoint.clear();
            } else if !state.public_endpoint.is_empty()
                && deleted_hostnames.contains(&state.public_endpoint)
            {
                state.public_endpoint.clear();
            }
        }
        self.emit_endpoint_changed();

        (true, format!("{tunnel_name} tunnel deleted"))
    }

    pub fn add_java_tunnel(
        &self,
        server_id: &str,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
    ) -> (bool, String, String) {
        self.add_tunnel_for_protocol(server_id, server_dir, "tcp", secret, auto_install, 19132)
    }

    pub fn regenerate_java_tunnel(
        &self,
        server_id: &str,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
    ) -> (bool, String, String) {
        self.regenerate_tunnel_for_protocol(
            server_id,
            server_dir,
            "tcp",
            secret,
            auto_install,
            19132,
        )
    }

    pub fn delete_java_tunnel(
        &self,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
    ) -> (bool, String) {
        self.delete_tunnel_for_protocol(server_dir, "tcp", secret, auto_install, 19132)
    }

    pub fn add_bedrock_tunnel(
        &self,
        server_id: &str,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String, String) {
        self.add_tunnel_for_protocol(
            server_id,
            server_dir,
            "udp",
            secret,
            auto_install,
            bedrock_port,
        )
    }

    pub fn regenerate_bedrock_tunnel(
        &self,
        server_id: &str,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String, String) {
        self.regenerate_tunnel_for_protocol(
            server_id,
            server_dir,
            "udp",
            secret,
            auto_install,
            bedrock_port,
        )
    }

    pub fn delete_bedrock_tunnel(
        &self,
        server_dir: &str,
        secret: &str,
        auto_install: bool,
        bedrock_port: u16,
    ) -> (bool, String) {
        self.delete_tunnel_for_protocol(server_dir, "udp", secret, auto_install, bedrock_port)
    }

    pub fn stop(&self) -> (bool, String) {
        if !self.is_running() {
            {
                let mut state = self.state.lock().unwrap();
                state.server_id = None;
                state.public_endpoint.clear();
                state.claim_url.clear();
            }
            self.clear_active_tunnel_usage();
            self.emit_endpoint_changed();
            self.set_status("stopped");
            return (true, "playit is not running".to_string());
        }

        if let Some(mut child) = self.state.lock().unwrap().process.take() {
            util::terminate_child(&mut child, Duration::from_secs(4));
        }
        {
            let mut state = self.state.lock().unwrap();
            state.server_id = None;
            state.public_endpoint.clear();
            state.claim_url.clear();
        }
        self.clear_active_tunnel_usage();
        self.emit_endpoint_changed();
        self.set_status("stopped");

        (true, "playit stopped".to_string())
    }

    fn mark_tunnel_in_use(&self, tunnel_id: &str, in_use: bool) {
        let mut state = self.state.lock().unwrap();
        for tunnels in state.tunnels.values_mut() {
            for tunnel in tunnels {
                if tunnel.id == tunnel_id {
                    tunnel.in_use = in_use;
                    return;
                }
            }
        }
    }

    fn clear_active_tunnel_usage(&self) {
        let active_id = self.state.lock().unwrap().active_tunnel_id.clone();
        let Some(active_id) = active_id else {
            return;
        };
        self.mark_tunnel_in_use(&active_id, false);
        self.state.lock().unwrap().active_tunnel_id = None;
    }

    fn read_output(self: Arc<Self>, stdout: Option<std::process::ChildStdout>) {
        let Some(stdout) = stdout else {
            return;
        };
        let mut reader = BufReader::new(stdout);
        let mut buffer = String::new();
        let mut byte = [0u8; 1];

        loop {
            match reader.read(&mut byte) {
                Ok(0) => {
                    if !buffer.is_empty() {
                        self.parse_line_for_endpoints(&buffer);
                        self.emitter.emit(
                            "output-received",
                            ManagerEvent::OutputReceived(buffer.clone()),
                        );
                    }
                    break;
                }
                Ok(_) => {
                    let ch = byte[0] as char;
                    if ch == '\n' || ch == '\r' {
                        if !buffer.is_empty() {
                            self.parse_line_for_endpoints(&buffer);
                            self.emitter.emit(
                                "output-received",
                                ManagerEvent::OutputReceived(buffer.clone()),
                            );
                            buffer.clear();
                        }
                        continue;
                    }
                    buffer.push(ch);
                    if buffer.len() >= 4096 {
                        self.parse_line_for_endpoints(&buffer);
                        self.emitter.emit(
                            "output-received",
                            ManagerEvent::OutputReceived(buffer.clone()),
                        );
                        buffer.clear();
                    }
                }
                Err(_) => break,
            }
        }

        if let Some(mut child) = self.state.lock().unwrap().process.take() {
            let _ = child.wait();
        }
        {
            let mut state = self.state.lock().unwrap();
            state.server_id = None;
            state.public_endpoint.clear();
            state.claim_url.clear();
        }
        self.clear_active_tunnel_usage();
        self.emit_endpoint_changed();
        self.set_status("stopped");
    }

    fn parse_line_for_endpoints(&self, line: &str) {
        let text = ANSI_ESCAPE_RE.replace_all(line, "").trim().to_string();
        if text.is_empty() {
            return;
        }

        for url in text
            .split_whitespace()
            .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
        {
            let clean = url
                .trim_end_matches(['.', ',', ';', ')', ']', '}'])
                .to_string();
            if clean.contains("playit.gg/claim") && clean != self.claim_url() {
                self.state.lock().unwrap().claim_url = clean;
                self.emit_endpoint_changed();
            }
        }

        let mut candidates = Vec::new();
        candidates.extend(
            ENDPOINT_URL_RE
                .captures_iter(&text)
                .filter_map(|capture| capture.get(1).map(|m| m.as_str().to_string())),
        );
        candidates.extend(
            ENDPOINT_HOSTPORT_RE
                .captures_iter(&text)
                .filter_map(|capture| capture.get(1).map(|m| m.as_str().to_string())),
        );

        let best = self.pick_best_endpoint(&candidates);
        if best.is_empty() {
            return;
        }

        let current = self.public_endpoint();
        let current_score = if current.is_empty() {
            -1
        } else {
            self.endpoint_score(&current)
        };
        let best_score = self.endpoint_score(&best);
        if best_score > current_score || (best_score == current_score && best != current) {
            self.state.lock().unwrap().public_endpoint = best;
            self.emit_endpoint_changed();
        }
    }

    fn pick_best_endpoint(&self, candidates: &[String]) -> String {
        let mut best = String::new();
        let mut best_score = -1;
        for endpoint in candidates {
            let score = self.endpoint_score(endpoint);
            if score > best_score {
                best = endpoint.clone();
                best_score = score;
            }
        }
        best
    }

    fn endpoint_score(&self, endpoint: &str) -> i32 {
        if endpoint.is_empty() || !endpoint.contains(':') {
            return -1;
        }
        let Some((host, _port)) = endpoint.rsplit_once(':') else {
            return -1;
        };
        let host = host.trim().to_lowercase();
        if host.is_empty() {
            return -1;
        }
        if self.is_private_or_loopback_ipv4(&host) {
            return -1;
        }
        if host.ends_with("joinmc.link") {
            return 100;
        }
        if host.chars().any(|ch| ch.is_ascii_alphabetic()) {
            return 80;
        }
        if self.is_ipv4(&host) {
            return 40;
        }
        10
    }

    fn is_ipv4(&self, value: &str) -> bool {
        value.parse::<Ipv4Addr>().is_ok()
    }

    fn is_private_or_loopback_ipv4(&self, value: &str) -> bool {
        value
            .parse::<Ipv4Addr>()
            .map(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local())
            .unwrap_or(false)
    }
}

fn empty_tunnel_map() -> HashMap<String, Vec<PlayitTunnel>> {
    HashMap::from([
        ("tcp".to_string(), Vec::new()),
        ("udp".to_string(), Vec::new()),
        ("both".to_string(), Vec::new()),
    ])
}
