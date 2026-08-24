use crate::{
    constants::DATA_DIR,
    events::{EventEmitter, ManagerEvent},
    tunnel_store::TunnelConfig,
    util,
};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
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
    config: Option<TunnelConfig>,
}

pub struct SshManager {
    emitter: EventEmitter,
    state: Mutex<State>,
    directory: PathBuf,
}

impl SshManager {
    pub fn new() -> Self {
        Self {
            emitter: EventEmitter::default(),
            state: Mutex::new(State {
                process: None,
                status: "stopped".to_string(),
                public_endpoint: String::new(),
                config: None,
            }),
            directory: DATA_DIR.join("ssh"),
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

    fn binary_path_bundled(&self) -> PathBuf {
        self.directory.join(if cfg!(target_os = "windows") {
            "ssh.exe"
        } else {
            "ssh"
        })
    }

    fn find_in_path(exe: &str) -> Option<PathBuf> {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(exe);
            if candidate.exists() && candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(target_os = "windows")]
            {
                let candidate_exe = dir.join(format!("{exe}.exe"));
                if candidate_exe.exists() && candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
        None
    }

    pub fn resolve_binary(&self) -> Option<String> {
        let bundled = self.binary_path_bundled();
        if bundled.exists() && bundled.is_file() {
            return Some(bundled.to_string_lossy().to_string());
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Check /usr/bin/ssh inside flatpak sandbox or host
            for p in ["/usr/bin/ssh", "/bin/ssh", "/app/bin/ssh"] {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(p.to_string());
                }
            }
        }
        if let Some(p) = Self::find_in_path("ssh") {
            return Some(p.to_string_lossy().to_string());
        }
        // Flatpak host fallback via flatpak-spawn
        if Self::find_in_path("flatpak-spawn").is_some() {
            // Check if host has ssh
            let probe = Command::new("flatpak-spawn")
                .args(["--host", "which", "ssh"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Ok(s) = probe
                && s.success()
            {
                return Some("flatpak-spawn".to_string());
            }
        }
        None
    }

    fn is_flatpak_spawn(&self, binary: &str) -> bool {
        binary == "flatpak-spawn" || binary.ends_with("/flatpak-spawn")
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
        _progress: Option<Box<dyn Fn(u64, u64) + Send + 'static>>,
    ) -> (bool, String) {
        if let Some(bin) = self.resolve_binary() {
            return (true, bin);
        }
        (
            false,
            "SSH client not found. Please install openssh-client (provides `ssh`) or bundle it in the Flatpak."
                .to_string(),
        )
    }

    /// Validate SSH config fields. Returns (ok, msg).
    fn validate_config(config: &TunnelConfig) -> Result<(), String> {
        let host = config
            .extra
            .get("ssh_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if host.is_empty() {
            return Err("SSH host is required".to_string());
        }
        let remote_host = config
            .extra
            .get("ssh_remote_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let direction = config
            .extra
            .get("ssh_direction")
            .and_then(Value::as_str)
            .unwrap_or("local");
        if direction != "dynamic" && remote_host.is_empty() {
            return Err("Remote host is required (e.g. localhost or db.internal)".to_string());
        }
        if direction != "dynamic" {
            let remote_port = config
                .extra
                .get("ssh_remote_port")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if remote_port == 0 || remote_port > 65535 {
                return Err("Remote port must be 1-65535".to_string());
            }
        }
        let ssh_port = config
            .extra
            .get("ssh_port")
            .and_then(Value::as_u64)
            .unwrap_or(22);
        if ssh_port == 0 || ssh_port > 65535 {
            return Err("SSH port must be 1-65535".to_string());
        }
        Ok(())
    }

    fn build_args(&self, config: &TunnelConfig, binary: &str) -> Result<Vec<String>, String> {
        Self::validate_config(config)?;
        let host = config
            .extra
            .get("ssh_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let user = config
            .extra
            .get("ssh_user")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let ssh_port = config
            .extra
            .get("ssh_port")
            .and_then(Value::as_u64)
            .unwrap_or(22) as u16;
        let remote_host = config
            .extra
            .get("ssh_remote_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let remote_port = config
            .extra
            .get("ssh_remote_port")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u16;
        let bind_address = config
            .extra
            .get("ssh_bind_address")
            .and_then(Value::as_str)
            .unwrap_or("127.0.0.1")
            .trim()
            .to_string();
        let bind_address = if bind_address.is_empty() {
            "127.0.0.1".to_string()
        } else {
            bind_address
        };
        let direction = config
            .extra
            .get("ssh_direction")
            .and_then(Value::as_str)
            .unwrap_or("local")
            .to_string();
        let key_path = config
            .extra
            .get("ssh_key_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let local_port = config.port;

        let _ = binary;
        let mut args: Vec<String> = Vec::new();
        args.push("-N".to_string());
        args.push("-o".to_string());
        args.push("ExitOnForwardFailure=yes".to_string());
        args.push("-o".to_string());
        args.push("ServerAliveInterval=30".to_string());
        args.push("-o".to_string());
        args.push("ServerAliveCountMax=3".to_string());
        // StrictHostKeyChecking: accept-new is safe default, avoids blocking on prompt
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=accept-new".to_string());

        // Use app-local known_hosts to avoid polluting host file inside flatpak
        let known_hosts = self.directory.join("known_hosts");
        if let Some(parent) = known_hosts.parent() {
            let _ = fs::create_dir_all(parent);
        }
        args.push("-o".to_string());
        args.push(format!(
            "UserKnownHostsFile={}",
            known_hosts.to_string_lossy()
        ));

        if !key_path.is_empty() {
            let key_pb = PathBuf::from(&key_path);
            // If relative, assume inside DATA_DIR/ssh/keys
            let resolved = if key_pb.is_absolute() {
                key_pb
            } else {
                self.directory.join(&key_pb)
            };
            if resolved.exists() {
                args.push("-i".to_string());
                args.push(resolved.to_string_lossy().to_string());
            } else if Path::new(&key_path).exists() {
                args.push("-i".to_string());
                args.push(key_path.clone());
            }
        }

        if ssh_port != 22 {
            args.push("-p".to_string());
            args.push(ssh_port.to_string());
        }

        // Build forwarding spec
        match direction.as_str() {
            "remote" => {
                // -R [bind_address:]remote_port:remote_host:remote_port_local? Actually -R remote forwards: ssh -R [bind_address:]port:host:hostport
                // For remote, local_port is on remote side, forward to remote_host:remote_port (where remote_host is typically localhost)
                // We treat local_port as the port on remote server, remote_host:remote_port as destination on local side? Let's define:
                // For remote: local_port (our config.port) is the remote server's listening port, forwarding to remote_host:remote_port on client side (usually localhost:local_service)
                // Example: -R 8080:localhost:3000 means remote's 8080 -> client's localhost:3000
                let spec = format!("{bind_address}:{local_port}:{remote_host}:{remote_port}");
                args.push("-R".to_string());
                args.push(spec);
            }
            "dynamic" => {
                let spec = format!("{bind_address}:{local_port}");
                args.push("-D".to_string());
                args.push(spec);
            }
            _ => {
                // local (default): -L [bind_address:]port:host:hostport
                let spec = format!("{bind_address}:{local_port}:{remote_host}:{remote_port}");
                args.push("-L".to_string());
                args.push(spec);
            }
        }

        // Extra args if any (split by whitespace, simple)
        if let Some(extra) = config.extra.get("ssh_extra_args").and_then(Value::as_str) {
            for part in extra.split_whitespace() {
                if !part.is_empty() {
                    args.push(part.to_string());
                }
            }
        }

        let target = if user.is_empty() {
            host.clone()
        } else {
            format!("{user}@{host}")
        };
        args.push(target);

        Ok(args)
    }

    fn endpoint_label(&self, config: &TunnelConfig) -> String {
        let direction = config
            .extra
            .get("ssh_direction")
            .and_then(Value::as_str)
            .unwrap_or("local");
        let bind = config
            .extra
            .get("ssh_bind_address")
            .and_then(Value::as_str)
            .unwrap_or("127.0.0.1");
        let local_port = config.port;
        match direction {
            "dynamic" => format!("socks5://{bind}:{local_port}"),
            "remote" => {
                let remote_host = config
                    .extra
                    .get("ssh_remote_host")
                    .and_then(Value::as_str)
                    .unwrap_or("localhost");
                let remote_port = config
                    .extra
                    .get("ssh_remote_port")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let ssh_host = config
                    .extra
                    .get("ssh_host")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                format!("{ssh_host}:{local_port} → {remote_host}:{remote_port}")
            }
            _ => {
                let remote_host = config
                    .extra
                    .get("ssh_remote_host")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let remote_port = config
                    .extra
                    .get("ssh_remote_port")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                format!("{bind}:{local_port} → {remote_host}:{remote_port}")
            }
        }
    }

    pub fn start_with_config(self: &Arc<Self>, config: &TunnelConfig) -> (bool, String) {
        if self.is_running() {
            return (true, String::new());
        }

        let binary = match self.resolve_binary() {
            Some(b) => b,
            None => {
                self.set_status("error");
                return (false, "SSH client not found. Install openssh-client.".to_string());
            }
        };

        if let Err(e) = Self::validate_config(config) {
            self.set_status("error");
            return (false, e);
        }

        let args = match self.build_args(config, &binary) {
            Ok(a) => a,
            Err(e) => {
                self.set_status("error");
                return (false, e);
            }
        };

        // Optional integrity check if bundled binary
        let bundled_path = self.binary_path_bundled();
        if Path::new(&binary) == bundled_path {
            if let Err(e) = util::check_binary_integrity(Path::new(&binary)) {
                self.set_status("error");
                return (false, format!("SSH binary corrupted: {e}"));
            }
        }

        {
            let mut state = self.state.lock().unwrap();
            state.config = Some(config.clone());
        }
        self.set_endpoint("");
        self.set_status("starting");

        let mut command = if self.is_flatpak_spawn(&binary) {
            let mut c = Command::new(&binary);
            let mut spawn_args = vec!["--host".to_string(), "ssh".to_string()];
            // For host ssh, ensure paths are absolute (host-visible)
            // known_hosts and key paths already absolute via DATA_DIR which is under $HOME/.var/app - host can read it
            spawn_args.extend(args.clone());
            c.args(spawn_args);
            c
        } else {
            let mut c = Command::new(&binary);
            c.args(&args);
            c
        };
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        util::command_no_window(&mut command);
        util::disable_setuid_on_child(&mut command);

        // Ensure key file has correct perms if used
        if let Some(key) = config.extra.get("ssh_key_path").and_then(Value::as_str) {
            if !key.trim().is_empty() {
                let pb = if Path::new(key).is_absolute() {
                    PathBuf::from(key)
                } else {
                    self.directory.join(key)
                };
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if pb.exists() {
                        let _ = fs::set_permissions(&pb, fs::Permissions::from_mode(0o600));
                    }
                }
            }
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.set_status("error");
                return (false, format!("Failed to execute ssh: {e}"));
            }
        };

        let stderr = child.stderr.take();
        let stdout = child.stdout.take();
        self.state.lock().unwrap().process = Some(child);

        // Set endpoint immediately for UI
        let label = self.endpoint_label(config);
        self.set_endpoint(&label);

        // Spawn readers: check for immediate errors
        if let Some(stderr) = stderr {
            let manager = self.clone();
            thread::spawn(move || manager.read_stderr(BufReader::new(stderr)));
        }
        if let Some(stdout) = stdout {
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line) {
                    if n == 0 {
                        break;
                    }
                    line.clear();
                }
                // stdout not critical for ssh -N
            });
        }

        // Give ssh a moment to fail fast (e.g. bad host, auth)
        thread::sleep(Duration::from_millis(400));
        if !self.is_running() {
            let status = self.status();
            if status.starts_with("error:") {
                return (false, status.trim_start_matches("error:").trim().to_string());
            }
            // If process exited quickly without error status, treat as error
            return (false, "SSH exited immediately. Check host, user, key and remote.".to_string());
        }

        self.set_status("running");
        (true, String::new())
    }

    // For compatibility with ManagerHandle::start(port, protocol)
    pub fn start(self: &Arc<Self>, port: u16, _protocol: &str) -> (bool, String) {
        // Try to find config by port in store (fallback)
        let tunnels = crate::tunnel_store::load_tunnels();
        if let Some(cfg) = tunnels.into_iter().find(|c| c.port == port && c.provider == "SSH") {
            return self.start_with_config(&cfg);
        }
        (false, "SSH tunnel config not found".to_string())
    }

    fn read_stderr(self: Arc<Self>, reader: BufReader<std::process::ChildStderr>) {
        let mut last_error = String::new();
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            // Detect common ssh errors
            let lower = line.to_lowercase();
            if lower.contains("permission denied")
                || lower.contains("no such file")
                || lower.contains("could not resolve hostname")
                || lower.contains("connection refused")
                || lower.contains("connection timed out")
                || lower.contains("port already in use")
                || lower.contains("bind")
                || lower.contains("authentic")
                || lower.contains("host key verification failed")
                || lower.contains("invalid")
            {
                last_error = line.clone();
            }
            // Emit as output for debugging
            self.emitter.emit(
                "output-received",
                ManagerEvent::OutputReceived(line.clone()),
            );
            if !last_error.is_empty() && lower.contains("error") {
                break;
            }
        }

        // Wait for child
        let exit_status = {
            let mut state = self.state.lock().unwrap();
            if let Some(mut child) = state.process.take() {
                let _ = child.wait();
            }
            None::<()>
        };
        let _ = exit_status;

        let was_stopping = self.status() == "stopping";
        if was_stopping {
            self.set_endpoint("");
            self.set_status("stopped");
            return;
        }

        if !last_error.is_empty() {
            self.set_endpoint("");
            self.set_status(&format!("error: {last_error}"));
        } else {
            // If ssh exited without explicit error, check if we were running
            // It likely was stopped externally
            if self.status() != "stopped" {
                self.set_endpoint("");
                self.set_status("stopped");
            }
        }
    }

    pub fn stop(self: &Arc<Self>) {
        let child = self.state.lock().unwrap().process.take();
        if let Some(mut child) = child {
            self.set_status("stopping");
            let manager = self.clone();
            thread::spawn(move || {
                util::terminate_child(&mut child, Duration::from_secs(3));
                manager.set_endpoint("");
                manager.set_status("stopped");
            });
            return;
        }
        self.set_endpoint("");
        self.set_status("stopped");
    }

    /// Import a key file into DATA_DIR/ssh/keys/<id> and return stored relative path
    pub fn import_key(&self, source_path: &Path) -> Result<String, String> {
        if !source_path.exists() {
            return Err("Key file not found".to_string());
        }
        let data = fs::read(source_path).map_err(|e| format!("Failed to read key: {e}"))?;
        if data.is_empty() {
            return Err("Key file is empty".to_string());
        }
        // Basic validation: should contain PRIVATE KEY or be non-empty
        let text = String::from_utf8_lossy(&data);
        if !text.contains("PRIVATE KEY") && !text.contains("OPENSSH") && data.len() < 10 {
            return Err("File does not look like a private key".to_string());
        }
        let keys_dir = self.directory.join("keys");
        fs::create_dir_all(&keys_dir).map_err(|e| format!("Failed to create keys dir: {e}"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let dest = keys_dir.join(format!("{id}.key"));
        fs::write(&dest, &data).map_err(|e| format!("Failed to copy key: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
        }
        // Return relative path from ssh directory for portability
        Ok(format!("keys/{id}.key"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel_store::TunnelConfig;
    use std::collections::HashMap;

    fn make_config(port: u16, extra: HashMap<String, Value>) -> TunnelConfig {
        TunnelConfig {
            id: "test-id".to_string(),
            provider: "SSH".to_string(),
            protocol: "TCP".to_string(),
            port,
            label: "test".to_string(),
            autostart: false,
            public_url: String::new(),
            extra,
        }
    }

    #[test]
    fn test_ssh_manager_initial_state() {
        let m = SshManager::new();
        assert_eq!(m.status(), "stopped");
        assert_eq!(m.public_endpoint(), "");
        assert!(!m.is_running());
    }

    #[test]
    fn test_validate_config() {
        let mut extra = HashMap::new();
        extra.insert("ssh_host".to_string(), Value::String("example.com".to_string()));
        extra.insert("ssh_remote_host".to_string(), Value::String("localhost".to_string()));
        extra.insert("ssh_remote_port".to_string(), Value::Number(5432.into()));
        let cfg = make_config(8080, extra);
        assert!(SshManager::validate_config(&cfg).is_ok());
    }

    #[test]
    fn test_build_args_local() {
        let m = SshManager::new();
        let mut extra = HashMap::new();
        extra.insert("ssh_host".to_string(), Value::String("example.com".to_string()));
        extra.insert("ssh_user".to_string(), Value::String("alice".to_string()));
        extra.insert("ssh_port".to_string(), Value::Number(2222.into()));
        extra.insert("ssh_remote_host".to_string(), Value::String("db.internal".to_string()));
        extra.insert("ssh_remote_port".to_string(), Value::Number(5432.into()));
        extra.insert("ssh_bind_address".to_string(), Value::String("127.0.0.1".to_string()));
        extra.insert("ssh_direction".to_string(), Value::String("local".to_string()));
        let cfg = make_config(8080, extra);
        let args = m.build_args(&cfg, "ssh").unwrap();
        assert!(args.contains(&"-L".to_string()));
        assert!(args.contains(&"127.0.0.1:8080:db.internal:5432".to_string()));
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"alice@example.com".to_string()));
    }

    #[test]
    fn test_build_args_dynamic() {
        let m = SshManager::new();
        let mut extra = HashMap::new();
        extra.insert("ssh_host".to_string(), Value::String("example.com".to_string()));
        extra.insert("ssh_direction".to_string(), Value::String("dynamic".to_string()));
        let cfg = make_config(1080, extra);
        let args = m.build_args(&cfg, "ssh").unwrap();
        assert!(args.contains(&"-D".to_string()));
        assert!(args.contains(&"127.0.0.1:1080".to_string()));
    }
}
