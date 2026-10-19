use crate::{
    AppError,
    config::{self, Config},
    network::{self, Address},
    platform::Job,
    protocol::{self, Probe},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader},
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub timestamp: u64,
    pub message: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub generation: u64,
    pub config: Config,
    pub phase: String,
    pub pid: Option<u32>,
    pub started_at: Option<u64>,
    pub error: Option<AppError>,
    pub probe: Option<Probe>,
    pub logs: Vec<LogLine>,
    pub addresses: Vec<Address>,
    pub backend_available: bool,
    pub config_path: String,
}
struct ManagedChild {
    child: Child,
    job: Option<Job>,
}
impl ManagedChild {
    fn terminate(mut self) {
        // Dropping the private job ends only this backend and its descendants.
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Inner {
    config: Config,
    process: Option<ManagedChild>,
    phase: String,
    generation: u64,
    started_at: Option<u64>,
    error: Option<AppError>,
    probe: Option<Probe>,
    logs: VecDeque<LogLine>,
}
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<Inner>>,
    directory: PathBuf,
    resource_dir: PathBuf,
    repository: PathBuf,
    transfer_bridge: Arc<Mutex<Option<(String, String)>>>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl Manager {
    pub fn new(directory: PathBuf, resource_dir: PathBuf) -> Result<Self, AppError> {
        let config = config::load(&directory)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                process: None,
                phase: "stopped".into(),
                generation: 0,
                started_at: None,
                error: None,
                probe: None,
                logs: VecDeque::new(),
            })),
            directory,
            resource_dir,
            repository: Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
            transfer_bridge: Arc::new(Mutex::new(None)),
        })
    }
    pub fn set_transfer_bridge(&self, bridge: (String, String)) {
        *self.transfer_bridge.lock().unwrap() = Some(bridge);
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn backend(&self) -> Result<(PathBuf, Vec<String>), AppError> {
        let bundled = self.resource_dir.join("backend/windows-mcp-backend.exe");
        if bundled.is_file() {
            return Ok((bundled, vec![]));
        }
        if cfg!(debug_assertions) {
            let python = self.repository.join(".venv/Scripts/python.exe");
            if python.is_file() {
                return Ok((python, vec!["-m".into(), "windows_mcp".into()]));
            }
        }
        Err(AppError::new(
            "backend_missing",
            "Run desktop/scripts/build-backend.ps1",
        ))
    }
    fn append_log(&self, message: &str, token: &str) {
        let message = message.replace(token, "[REDACTED]");
        let message: String = message
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .take(2400)
            .collect();
        if message.trim().is_empty() {
            return;
        }
        let mut inner = self.lock();
        if inner.logs.len() == 200 {
            inner.logs.pop_front();
        }
        inner.logs.push_back(LogLine {
            timestamp: now(),
            message,
        });
    }
    fn read_output(&self, reader: impl std::io::Read + Send + 'static, token: String) {
        let manager = self.clone();
        thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                manager.append_log(&line, &token);
            }
        });
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut inner = self.lock();
        let exit = inner
            .process
            .as_mut()
            .and_then(|p| p.child.try_wait().ok().flatten());
        if let Some(exit) = exit {
            inner.process.take();
            inner.phase = "error".into();
            inner.error = Some(AppError::new(
                "backend_exited",
                format!("Exit code: {exit}"),
            ));
            inner.started_at = None;
            inner.probe = None;
        }
        let addresses = network::addresses().unwrap_or_default();
        Snapshot {
            generation: inner.generation,
            config: inner.config.clone(),
            phase: inner.phase.clone(),
            pid: inner.process.as_ref().map(|p| p.child.id()),
            started_at: inner.started_at,
            error: inner.error.clone(),
            probe: inner.probe.clone(),
            logs: inner.logs.iter().cloned().collect(),
            addresses,
            backend_available: self.backend().is_ok(),
            config_path: self.directory.join("manager.json").to_string_lossy().into(),
        }
    }
    pub fn save(&self, mut config: Config) -> Result<Snapshot, AppError> {
        config.token = config.token.trim().into();
        config.validate()?;
        {
            let mut inner = self.lock();
            if inner.process.is_some() || inner.phase == "stopping" {
                return Err(AppError::new("stop_before_save", ""));
            }
            config::save(&self.directory, &config)?;
            inner.config = config;
            inner.generation += 1;
            inner.error = None;
            inner.phase = "stopped".into();
            inner.probe = None;
        }
        Ok(self.snapshot())
    }
    pub fn language(&self, language: String) -> Result<Snapshot, AppError> {
        {
            let mut inner = self.lock();
            let mut updated = inner.config.clone();
            updated.language = language;
            config::save(&self.directory, &updated)?;
            inner.config = updated;
        }
        Ok(self.snapshot())
    }
    #[cfg(test)]
    pub fn start(&self) -> Result<Snapshot, AppError> {
        let generation = self.lock().generation;
        self.start_if_current(generation)
    }
    pub fn start_if_current(&self, expected_generation: u64) -> Result<Snapshot, AppError> {
        let (generation, config) = {
            let mut inner = self.lock();
            // A stop or configuration save invalidates already queued start requests.
            if inner.generation != expected_generation {
                drop(inner);
                return Ok(self.snapshot());
            }
            if inner.process.is_some() || inner.phase == "stopping" {
                return Err(AppError::new("already_running", ""));
            }
            let config = inner.config.clone();
            config.validate()?;
            network::check_port(config.port, None)?;
            let (executable, mut args) = self.backend()?;
            let reservation = TcpListener::bind((config.host(), config.port))
                .map_err(|e| AppError::new("port_in_use", e.to_string()))?;
            let runtime_config = self.directory.join("backend.toml");
            fs::write(
                &runtime_config,
                "# Managed by Windows-MCP Manager\n[control]\nenabled = true\n",
            )
            .map_err(AppError::storage)?;
            args.extend([
                "serve".into(),
                "--transport".into(),
                "streamable-http".into(),
                "--host".into(),
                config.host().into(),
                "--port".into(),
                config.port.to_string(),
                "--config".into(),
                runtime_config.to_string_lossy().into(),
                "--control".into(),
            ]);
            let job = Job::new()?;
            let mut command = Command::new(executable);
            command
                .args(args)
                .creation_flags(0x0800_0000)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());
            for key in [
                "WINDOWS_MCP_IP_ALLOWLIST",
                "WINDOWS_MCP_CORS_ORIGINS",
                "WINDOWS_MCP_SSL_CERTFILE",
                "WINDOWS_MCP_SSL_KEYFILE",
                "WINDOWS_MCP_OAUTH_CLIENT_ID",
                "WINDOWS_MCP_OAUTH_CLIENT_SECRET",
                "WINDOWS_MCP_TOOLS",
                "WINDOWS_MCP_EXCLUDE_TOOLS",
                "WINDOWS_MCP_STATELESS_HTTP",
                "WINDOWS_MCP_CONTROL",
                "WINDOWS_MCP_CONTROL_IDLE_TIMEOUT",
            ] {
                command.env_remove(key);
            }
            // Never put the bearer token on the command line or in the TOML file.
            command
                .env("WINDOWS_MCP_AUTH_KEY", &config.token)
                .env("WINDOWS_MCP_LANGUAGE", &config.language)
                .env(
                    "WINDOWS_MCP_MANAGER_CONFIG",
                    self.directory.join("manager.json"),
                )
                .env("PYTHONIOENCODING", "utf-8")
                .env("PYTHONUNBUFFERED", "1")
                .env("ANONYMIZED_TELEMETRY", "false");
            if let Some((url, token)) = &*self.transfer_bridge.lock().unwrap() {
                command
                    .env("WINDOWS_MCP_TRANSFER_CONTROL_URL", url)
                    .env("WINDOWS_MCP_TRANSFER_CONTROL_TOKEN", token);
            } else {
                command
                    .env_remove("WINDOWS_MCP_TRANSFER_CONTROL_URL")
                    .env_remove("WINDOWS_MCP_TRANSFER_CONTROL_TOKEN");
            }
            drop(reservation);
            let mut child = command
                .spawn()
                .map_err(|e| AppError::new("process_failed", e.to_string()))?;
            if let Err(error) = job.assign(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            if let Some(stdout) = child.stdout.take() {
                self.read_output(stdout, config.token.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                self.read_output(stderr, config.token.clone());
            }
            inner.process = Some(ManagedChild {
                child,
                job: Some(job),
            });
            inner.generation += 1;
            inner.phase = "starting".into();
            inner.error = None;
            inner.probe = None;
            inner.started_at = None;
            (inner.generation, config)
        };
        self.append_log(
            &format!(
                "Starting official Windows-MCP on {}:{}",
                config.host(),
                config.port
            ),
            &config.token,
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut last_error = None;
        while Instant::now() < deadline {
            {
                let mut inner = self.lock();
                if inner.generation != generation || inner.process.is_none() {
                    drop(inner);
                    return Ok(self.snapshot());
                }
                if let Some(exit) = inner
                    .process
                    .as_mut()
                    .unwrap()
                    .child
                    .try_wait()
                    .map_err(|e| AppError::new("process_failed", e.to_string()))?
                {
                    inner.process.take();
                    inner.phase = "error".into();
                    inner.error = Some(AppError::new(
                        "backend_exited",
                        format!("Exit code: {exit}. See server log."),
                    ));
                    drop(inner);
                    return Ok(self.snapshot());
                }
            }
            match protocol::probe_local(&config) {
                Ok(probe) => {
                    let mut inner = self.lock();
                    if inner.generation == generation && inner.process.is_some() {
                        let pid = inner.process.as_ref().unwrap().child.id();
                        if let Err(error) = network::check_port(config.port, Some(pid)) {
                            inner.process.take().unwrap().terminate();
                            inner.phase = "error".into();
                            inner.error = Some(error);
                            drop(inner);
                            return Ok(self.snapshot());
                        }
                        inner.phase = "running".into();
                        inner.started_at = Some(now());
                        inner.probe = Some(probe);
                    }
                    drop(inner);
                    return Ok(self.snapshot());
                }
                Err(error) => last_error = Some(error),
            }
            thread::sleep(Duration::from_millis(350));
        }
        let mut inner = self.lock();
        if inner.generation == generation {
            if let Some(process) = inner.process.take() {
                process.terminate();
            }
            inner.phase = "error".into();
            inner.error = Some(AppError::new(
                "startup_timeout",
                last_error.map(|e| e.detail).unwrap_or_default(),
            ));
        }
        drop(inner);
        Ok(self.snapshot())
    }
    pub fn stop(&self) -> Snapshot {
        let process = {
            let mut inner = self.lock();
            if inner.phase == "stopping" {
                drop(inner);
                return self.snapshot();
            }
            inner.generation += 1;
            inner.phase = "stopping".into();
            inner.process.take()
        };
        if let Some(process) = process {
            process.terminate();
        }
        let token = {
            let mut inner = self.lock();
            inner.phase = "stopped".into();
            inner.started_at = None;
            inner.probe = None;
            inner.error = None;
            inner.config.token.clone()
        };
        self.append_log("Windows-MCP stopped", &token);
        self.snapshot()
    }
    pub fn test_connection(&self) -> Result<Probe, AppError> {
        let (config, generation, pid) = {
            let mut inner = self.lock();
            if inner.phase != "running" {
                return Err(AppError::new("not_running", ""));
            }
            let pid = inner
                .process
                .as_ref()
                .ok_or_else(|| AppError::new("not_running", ""))?
                .child
                .id();
            inner.probe = None;
            inner.error = None;
            (inner.config.clone(), inner.generation, pid)
        };
        let result: Result<Probe, AppError> = (|| {
            network::check_port(config.port, Some(pid))?;
            let probe = protocol::probe(&config)?;
            network::check_port(config.port, Some(pid))?;
            Ok(probe)
        })();
        let mut inner = self.lock();
        if inner.generation != generation || inner.phase != "running" {
            return Err(AppError::new("not_running", ""));
        }
        match result {
            Ok(probe) => {
                inner.probe = Some(probe.clone());
                Ok(probe)
            }
            Err(error) => {
                inner.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_start_is_cancelled_by_stop() {
        let dir = std::env::temp_dir().join(format!(
            "windows-mcp-cancel-{}",
            config::generate_token().unwrap()
        ));
        let manager = Manager::new(dir.clone(), dir.clone()).unwrap();
        let ticket = manager.snapshot().generation;
        manager.stop();
        let state = manager.start_if_current(ticket).unwrap();
        assert_eq!(state.phase, "stopped");
        assert!(state.pid.is_none());
        fs::remove_file(dir.join("manager.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn log_masks_tokens_and_bounds_history() {
        let dir = std::env::temp_dir().join(format!(
            "windows-mcp-logs-{}",
            config::generate_token().unwrap()
        ));
        let manager = Manager::new(dir.clone(), dir.clone()).unwrap();
        for _ in 0..220 {
            manager.append_log("secret-token\u{001b}", "secret-token");
        }
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.logs.len(), 200);
        assert_eq!(snapshot.logs[0].message, "[REDACTED]");
        fs::remove_file(dir.join("manager.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    #[ignore = "Starts the real official Windows-MCP backend; run explicitly"]
    fn official_backend_lifecycle_and_authentication() {
        struct BackendGuard(Manager);
        impl Drop for BackendGuard {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        struct LegacyGuard(Child);
        impl Drop for LegacyGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "windows-mcp-live-{}",
            config::generate_token().unwrap()
        ));
        let manager = Manager::new(dir.clone(), dir.clone()).unwrap();
        let _guard = BackendGuard(manager.clone());
        let mut config = manager.snapshot().config;
        config.port = TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        config.lan_enabled = true;
        manager.save(config.clone()).unwrap();
        // A specific-IP listener can coexist with a wildcard bind on Windows.
        // Startup must detect it before launching a second backend.
        let occupied = TcpListener::bind(("127.0.0.1", config.port)).unwrap();
        let error = manager.start().err().unwrap();
        assert_eq!(error.code, "port_in_use");
        assert!(error.detail.contains(&format!("127.0.0.1:{}", config.port)));
        assert!(
            error
                .detail
                .contains(&format!("PID {}", std::process::id()))
        );
        assert_eq!(manager.snapshot().phase, "stopped");
        drop(occupied);
        let started = manager.start().unwrap();
        assert_eq!(started.phase, "running", "{:?}", started.error);
        assert!(started.probe.unwrap().tool_count > 0);
        let status = reqwest::blocking::Client::new()
            .post(config.local_url())
            .send()
            .unwrap()
            .status();
        assert_eq!(status.as_u16(), 401);
        assert_eq!(manager.start().err().unwrap().code, "already_running");
        assert_eq!(
            manager.save(config.clone()).err().unwrap().code,
            "stop_before_save"
        );
        let probe = manager.test_connection().unwrap();
        assert!(probe.tool_count > 0);
        assert!(probe.verified_urls.contains(&config.local_url()));
        for address in network::addresses().unwrap() {
            assert!(
                probe
                    .verified_urls
                    .contains(&format!("http://{}:{}/mcp", address.ip, config.port))
            );
        }
        let address = network::addresses().unwrap().into_iter().next().unwrap();
        let python = manager.repository.join(".venv/Scripts/python.exe");
        let legacy = Command::new(python)
            .args(["-c", "import socket,sys; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind((sys.argv[1],int(sys.argv[2]))); s.listen(1); print('ready',flush=True); sys.stdin.read()", &address.ip, &config.port.to_string()])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .spawn().unwrap();
        let mut legacy = LegacyGuard(legacy);
        let mut ready = String::new();
        BufReader::new(legacy.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let conflict = manager.test_connection().err().unwrap();
        assert_eq!(conflict.code, "legacy_instance");
        assert!(conflict.detail.contains(&address.ip));
        let failed = manager.snapshot();
        assert!(failed.probe.is_none());
        assert_eq!(failed.phase, "running");
        assert_eq!(failed.error.unwrap().code, "legacy_instance");
        assert!(legacy.0.try_wait().unwrap().is_none());
        drop(legacy);
        assert!(manager.test_connection().unwrap().tool_count > 0);
        assert!(manager.snapshot().error.is_none());
        assert_eq!(manager.stop().phase, "stopped");
        assert!(TcpListener::bind(("127.0.0.1", config.port)).is_ok());
        fs::remove_file(dir.join("manager.json")).unwrap();
        fs::remove_file(dir.join("backend.toml")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
