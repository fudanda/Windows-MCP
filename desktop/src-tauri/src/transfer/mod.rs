mod model;
#[cfg(test)]
mod tests;
mod web;
use crate::{
    AppError,
    platform::{self, Job},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use model::*;
pub use model::{PeerInput, SettingsPatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub direction: String,
    pub peer: String,
    pub name: String,
    pub status: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub file_count: usize,
    pub completed_files: usize,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub error: Option<AppError>,
    pub destination: Option<String>,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}
impl Task {
    fn new(direction: &str, peer: String, name: String) -> Result<Self, AppError> {
        Ok(Self {
            id: id()?,
            direction: direction.into(),
            peer,
            name,
            status: "preparing".into(),
            total_bytes: 0,
            transferred_bytes: 0,
            file_count: 0,
            completed_files: 0,
            started_at: now(),
            finished_at: None,
            error: None,
            destination: None,
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }
    fn active(&self) -> bool {
        !matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}
struct Service {
    stop: oneshot::Sender<()>,
    done: mpsc::Receiver<()>,
}
#[derive(Clone)]
struct Backend {
    root: PathBuf,
    url: String,
    token: String,
}
struct FileServer {
    child: Child,
    job: Option<Job>,
    gateway: Option<Service>,
    backend: Backend,
}
impl Drop for FileServer {
    fn drop(&mut self) {
        if let Some(service) = self.gateway.take() {
            let _ = service.stop.send(());
        }
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Data {
    settings: Settings,
    server: Option<FileServer>,
    tasks: Vec<Task>,
    sessions: HashMap<String, u64>,
}
#[derive(Clone)]
pub struct TransferManager {
    data: Arc<Mutex<Data>>,
    directory: PathBuf,
    resource: PathBuf,
    bridge_url: String,
    bridge_token: String,
    control: Arc<Mutex<Option<Service>>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    pub settings: Settings,
    pub running: bool,
    pub tasks: Vec<Task>,
    pub engine: &'static str,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub action: String,
    pub config: Option<SettingsPatch>,
    pub peer: Option<PeerInput>,
    pub peer_id: Option<String>,
    pub paths: Option<Vec<String>>,
    pub task_id: Option<String>,
}
#[derive(Clone)]
struct Auth {
    token: String,
    manager: Option<TransferManager>,
}
struct ApiError(AppError);
impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "transfer_missing" => StatusCode::NOT_FOUND,
            "transfer_conflict" | "transfer_busy" => StatusCode::CONFLICT,
            "transfer_integrity" => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(self.0)).into_response()
    }
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    else {
        return false;
    };
    value.len() == token.len()
        && value
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}
async fn auth(State(auth): State<Auth>, request: Request, next: Next) -> Response {
    let cookie_ok = auth
        .manager
        .as_ref()
        .is_some_and(|m| web::session_valid(m, request.headers()));
    if !web::origin_allowed(request.headers())
        || (!authorized(request.headers(), &auth.token) && !cookie_ok)
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code":"unauthorized"})),
        )
            .into_response();
    }
    next.run(request).await
}
fn bind(host: &str, port: u16) -> Result<TcpListener, AppError> {
    let socket =
        TcpListener::bind((host, port)).map_err(|e| error("port_in_use", e.to_string()))?;
    socket.set_nonblocking(true).map_err(io_error)?;
    Ok(socket)
}
fn serve(listener: TcpListener, router: Router) -> Service {
    let (stop, stopped) = oneshot::channel();
    let (tx, done) = mpsc::channel();
    tauri::async_runtime::spawn(async move {
        if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        }
        let _ = tx.send(());
    });
    Service { stop, done }
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<Json<T>, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(io_error(e)))?
        .map(Json)
        .map_err(ApiError)
}
impl TransferManager {
    pub fn new(
        directory: PathBuf,
        default_root: PathBuf,
        resource: PathBuf,
    ) -> Result<Self, AppError> {
        let settings = load(&directory, &default_root)?;
        let listener = bind("127.0.0.1", 0)?;
        let bridge_url = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().map_err(io_error)?.port()
        );
        let manager = Self {
            data: Arc::new(Mutex::new(Data {
                settings,
                server: None,
                tasks: vec![],
                sessions: HashMap::new(),
            })),
            directory,
            resource,
            bridge_url,
            bridge_token: crate::config::generate_token()?,
            control: Arc::new(Mutex::new(None)),
        };
        let router = Router::new()
            .route("/control", post(control_command))
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
            .route_layer(middleware::from_fn_with_state(
                Auth {
                    token: manager.bridge_token.clone(),
                    manager: None,
                },
                auth,
            ))
            .with_state(manager.clone());
        *manager.control.lock().unwrap() = Some(serve(listener, router));
        Ok(manager)
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn bridge(&self) -> (String, String) {
        (self.bridge_url.clone(), self.bridge_token.clone())
    }
    pub fn snapshot(&self) -> TransferSnapshot {
        let mut data = self.lock();
        if data
            .server
            .as_mut()
            .is_some_and(|server| server.child.try_wait().ok().flatten().is_some())
        {
            data.server.take();
            data.sessions.clear();
            for task in data.tasks.iter_mut().filter(|t| t.active()) {
                task.cancel.store(true, Ordering::Relaxed);
            }
        }
        TransferSnapshot {
            settings: data.settings.clone(),
            running: data.server.is_some(),
            tasks: data.tasks.iter().rev().cloned().collect(),
            engine: "Dufs 0.46.0",
        }
    }
    fn backend(&self) -> Result<Backend, AppError> {
        self.lock()
            .server
            .as_ref()
            .map(|s| s.backend.clone())
            .ok_or_else(|| error("transfer_not_running", ""))
    }
    fn task_mut<'a>(data: &'a mut Data, id: &str) -> Result<&'a mut Task, AppError> {
        data.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| error("transfer_missing", "Task not found"))
    }
    fn add_task(&self, task: Task) -> Result<(), AppError> {
        let mut data = self.lock();
        if data.tasks.iter().filter(|t| t.active()).count() >= 4 {
            return Err(error("transfer_busy", "At most four active transfers"));
        }
        if data.tasks.len() >= 100 {
            if let Some(i) = data.tasks.iter().position(|t| !t.active()) {
                data.tasks.remove(i);
            }
        }
        data.tasks.push(task);
        Ok(())
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut Task)) {
        if let Ok(task) = Self::task_mut(&mut self.lock(), id) {
            f(task);
        }
    }
    fn finish(&self, id: &str, result: Result<String, AppError>) {
        self.update(id, |t| {
            match result {
                Ok(path) => {
                    t.status = "completed".into();
                    t.destination = Some(path);
                    t.completed_files = t.file_count;
                }
                Err(e) => {
                    t.status = if e.code == "transfer_cancelled" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    t.error = Some(e);
                }
            }
            t.finished_at = Some(now());
        });
    }
    pub fn configure(&self, patch: SettingsPatch) -> Result<TransferSnapshot, AppError> {
        let mut data = self.lock();
        if data.server.is_some() || data.tasks.iter().any(Task::active) {
            return Err(error("transfer_stop_first", ""));
        }
        let mut settings = data.settings.clone();
        if let Some(v) = patch.port {
            settings.port = v;
        }
        if let Some(v) = patch.lan_enabled {
            settings.lan_enabled = v;
        }
        if let Some(v) = patch.shared_dir {
            settings.shared_dir = v;
        }
        if let Some(v) = patch.token {
            settings.token = v;
        }
        save(&self.directory, &settings)?;
        data.settings = settings;
        drop(data);
        Ok(self.snapshot())
    }
    pub fn add_peer(&self, peer: PeerInput) -> Result<TransferSnapshot, AppError> {
        if peer.name.trim().is_empty() || peer.name.len() > 80 || !valid_token(&peer.token) {
            return Err(error("transfer_peer", "Provide a name, LAN URL and token"));
        }
        let url = peer_url(&peer.url)?;
        let mut data = self.lock();
        let mut settings = data.settings.clone();
        if let Some(saved) = settings.peers.iter_mut().find(|p| p.url == url) {
            saved.name = peer.name.trim().into();
            saved.token = peer.token;
        } else {
            settings.peers.push(Peer {
                id: id()?,
                name: peer.name.trim().into(),
                url,
                token: peer.token,
            });
        }
        save(&self.directory, &settings)?;
        data.settings = settings;
        drop(data);
        Ok(self.snapshot())
    }
    pub fn remove_peer(&self, id: &str) -> Result<TransferSnapshot, AppError> {
        let mut data = self.lock();
        let mut settings = data.settings.clone();
        settings.peers.retain(|p| p.id != id);
        save(&self.directory, &settings)?;
        data.settings = settings;
        drop(data);
        Ok(self.snapshot())
    }
    fn executable(&self) -> Result<PathBuf, AppError> {
        let bundled = self.resource.join("file-server/dufs.exe");
        if bundled.is_file() {
            return Ok(bundled);
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("file-server/dufs.exe");
        if cfg!(debug_assertions) && dev.is_file() {
            return Ok(dev);
        }
        Err(error(
            "transfer_engine_missing",
            "Run npm run build:file-server",
        ))
    }
    pub fn start(&self) -> Result<TransferSnapshot, AppError> {
        let mut data = self.lock();
        if data.server.is_some() {
            drop(data);
            return Ok(self.snapshot());
        }
        data.settings.validate()?;
        let executable = self.executable()?;
        let root = PathBuf::from(&data.settings.shared_dir);
        fs::create_dir_all(&root).map_err(io_error)?;
        ordinary(&root)?;
        let root = fs::canonicalize(root).map_err(io_error)?;
        if !root.is_dir() {
            return Err(error("transfer_directory", "Choose a directory"));
        }
        let public = bind(
            if data.settings.lan_enabled {
                "0.0.0.0"
            } else {
                "127.0.0.1"
            },
            data.settings.port,
        )?;
        let reserved = TcpListener::bind(("127.0.0.1", 0)).map_err(io_error)?;
        let port = reserved.local_addr().map_err(io_error)?.port();
        let backend = Backend {
            root,
            url: format!("http://127.0.0.1:{port}"),
            token: crate::config::generate_token()?,
        };
        let mut command = ProcessCommand::new(executable);
        // Clear inherited Dufs configuration; only the app-selected receive directory is exposed.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("DUFS_") {
                command.env_remove(key);
            }
        }
        command
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--allow-upload",
                "--allow-archive",
                "--allow-hash",
                "--hidden",
                ".wmcp-*",
                "--log-format",
                "",
            ])
            .arg(&backend.root)
            .env("DUFS_AUTH", format!("transfer:{}@/:rw", backend.token))
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let job = Job::new()?;
        drop(reserved);
        let mut child = command.spawn().map_err(io_error)?;
        if let Err(e) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let mut server = FileServer {
            child,
            job: Some(job),
            gateway: None,
            backend: backend.clone(),
        };
        let client = client()?;
        let mut ready = false;
        for _ in 0..40 {
            if server.child.try_wait().map_err(io_error)?.is_some() {
                break;
            }
            if client
                .get(format!("{}/__dufs__/health", backend.url))
                .basic_auth("transfer", Some(&backend.token))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !ready {
            return Err(error("transfer_engine_failed", "Dufs did not become ready"));
        }
        let router = web::router(self.clone(), data.settings.token.clone());
        server.gateway = Some(serve(public, router));
        data.server = Some(server);
        drop(data);
        Ok(self.snapshot())
    }
    pub fn stop(&self) -> TransferSnapshot {
        let server = {
            let mut data = self.lock();
            data.sessions.clear();
            for task in data.tasks.iter_mut().filter(|t| t.active()) {
                task.cancel.store(true, Ordering::Relaxed);
                task.status = "cancelling".into();
            }
            data.server.take()
        };
        drop(server);
        self.snapshot()
    }
    pub fn shutdown(&self) {
        self.stop();
        if let Some(service) = self.control.lock().unwrap().take() {
            let _ = service.stop.send(());
            let _ = service.done.recv_timeout(Duration::from_secs(2));
        }
    }
    pub fn cancel(&self, id: &str) -> Result<TransferSnapshot, AppError> {
        let mut data = self.lock();
        let task = Self::task_mut(&mut data, id)?;
        if task.active() {
            task.cancel.store(true, Ordering::Relaxed);
            task.status = "cancelling".into();
        }
        drop(data);
        Ok(self.snapshot())
    }
    pub fn send(&self, peer_id: &str, paths: Vec<String>) -> Result<Task, AppError> {
        let peer = self
            .lock()
            .settings
            .peers
            .iter()
            .find(|p| p.id == peer_id)
            .cloned()
            .ok_or_else(|| error("transfer_peer", "Select a saved device"))?;
        let task = Task::new(
            "send",
            peer.name.clone(),
            paths
                .first()
                .and_then(|p| Path::new(p).file_name())
                .map(|s| s.to_string_lossy().into())
                .unwrap_or_default(),
        )?;
        self.add_task(task.clone())?;
        let manager = self.clone();
        let owned = task.clone();
        thread::spawn(move || {
            let result = manager.send_worker(&owned, &peer, paths).map_err(|mut e| {
                e.detail = e.detail.replace(&peer.token, "[REDACTED]");
                e
            });
            manager.finish(&owned.id, result);
        });
        Ok(task)
    }
    fn send_worker(
        &self,
        task: &Task,
        peer: &Peer,
        paths: Vec<String>,
    ) -> Result<String, AppError> {
        let plan = plan(&paths, &task.cancel)?;
        let total = plan.manifest.validate()?;
        self.update(&task.id, |t| {
            t.total_bytes = total;
            t.file_count = plan.files.len();
            t.name = plan.manifest.name.clone();
            t.status = "transferring".into();
        });
        let client = client()?;
        let info = remote_json(
            client
                .get(format!("{}/v1/info", peer.url))
                .bearer_auth(&peer.token)
                .send()
                .map_err(network)?,
        )?;
        if info["protocol"] != "windows-mcp-transfer/1" {
            return Err(error("transfer_network", "Incompatible transfer service"));
        }
        let root = wire_url(&peer.url, &plan.manifest.name)?;
        let existing = client
            .head(root.clone())
            .bearer_auth(&peer.token)
            .send()
            .map_err(network)?;
        if existing.status() != 404 {
            return Err(error(
                if existing.status() == 401 {
                    "transfer_auth"
                } else {
                    "transfer_conflict"
                },
                "Destination must not already exist",
            ));
        }
        if plan.manifest.directory {
            check_cancel(&task.cancel)?;
            success(
                client
                    .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), root)
                    .bearer_auth(&peer.token)
                    .send()
                    .map_err(network)?,
            )?;
        }
        for entry in plan.manifest.entries.iter().filter(|e| e.directory) {
            check_cancel(&task.cancel)?;
            let path = format!("{}/{}", plan.manifest.name, entry.path);
            success(
                client
                    .request(
                        reqwest::Method::from_bytes(b"MKCOL").unwrap(),
                        wire_url(&peer.url, &path)?,
                    )
                    .bearer_auth(&peer.token)
                    .send()
                    .map_err(network)?,
            )?;
        }
        for (index, path) in plan.files {
            check_cancel(&task.cancel)?;
            ordinary(&path)?;
            let entry = &plan.manifest.entries[index];
            let name = if plan.manifest.directory {
                format!("{}/{}", plan.manifest.name, entry.path)
            } else {
                entry.path.clone()
            };
            let reader = ProgressReader {
                file: fs::File::open(path).map_err(io_error)?,
                manager: self.clone(),
                id: task.id.clone(),
                cancel: task.cancel.clone(),
            };
            let response = client
                .put(wire_url(&peer.url, &name)?)
                .bearer_auth(&peer.token)
                .header("X-Content-SHA256", &entry.sha256)
                .body(reqwest::blocking::Body::sized(reader, entry.size))
                .send();
            check_cancel(&task.cancel)?;
            success(response.map_err(network)?)?;
            self.update(&task.id, |t| t.completed_files += 1);
        }
        check_cancel(&task.cancel)?;
        Ok(format!("{}/files/{}", peer.url, plan.manifest.name))
    }
    pub fn command(&self, command: Command, reveal: bool) -> Result<Value, AppError> {
        let required = |v: Option<String>| {
            v.filter(|s| !s.is_empty())
                .ok_or_else(|| error("transfer_path", "Missing argument"))
        };
        let result = match command.action.as_str() {
            "status" => serde_json::to_value(self.snapshot()).map_err(io_error)?,
            "connection" => {
                let data = self.lock();
                return Ok(
                    json!({"port":data.settings.port,"lanEnabled":data.settings.lan_enabled,"token":data.settings.token,"path":"/","client":web::client_metadata()}),
                );
            }
            "configure" => serde_json::to_value(
                self.configure(
                    command
                        .config
                        .ok_or_else(|| error("transfer_path", "Missing config"))?,
                )?,
            )
            .map_err(io_error)?,
            "start" => serde_json::to_value(self.start()?).map_err(io_error)?,
            "stop" => serde_json::to_value(self.stop()).map_err(io_error)?,
            "add_peer" => serde_json::to_value(
                self.add_peer(
                    command
                        .peer
                        .ok_or_else(|| error("transfer_peer", "Missing peer"))?,
                )?,
            )
            .map_err(io_error)?,
            "remove_peer" => serde_json::to_value(self.remove_peer(&required(command.peer_id)?)?)
                .map_err(io_error)?,
            "send" => serde_json::to_value(self.send(
                &required(command.peer_id)?,
                command.paths.unwrap_or_default(),
            )?)
            .map_err(io_error)?,
            "cancel" => {
                serde_json::to_value(self.cancel(&required(command.task_id)?)?).map_err(io_error)?
            }
            _ => return Err(error("transfer_path", "Unknown action")),
        };
        fn redact(v: &mut Value) {
            match v {
                Value::Object(map) => {
                    for (k, v) in map {
                        if k == "token" {
                            *v = json!("[REDACTED]");
                        } else {
                            redact(v);
                        }
                    }
                }
                Value::Array(items) => {
                    for v in items {
                        redact(v);
                    }
                }
                _ => (),
            }
        }
        let mut result = result;
        if !reveal {
            redact(&mut result);
        }
        Ok(result)
    }
}
struct ProgressReader {
    file: fs::File,
    manager: TransferManager,
    id: String,
    cancel: Arc<AtomicBool>,
}
impl Read for ProgressReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("Transfer cancelled"));
        }
        let n = self.file.read(buffer)?;
        self.manager
            .update(&self.id, |t| t.transferred_bytes += n as u64);
        Ok(n)
    }
}
fn client() -> Result<reqwest::blocking::Client, AppError> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(network)
}
fn network(e: impl std::fmt::Display) -> AppError {
    error("transfer_network", e.to_string())
}
fn success(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response, AppError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status().as_u16();
        Err(error(
            match status {
                401 => "transfer_auth",
                409 | 403 => "transfer_conflict",
                422 => "transfer_integrity",
                _ => "transfer_network",
            },
            format!("HTTP {status}"),
        ))
    }
}
fn remote_json(response: reqwest::blocking::Response) -> Result<Value, AppError> {
    let mut body = String::new();
    success(response)?
        .take(1024 * 1024)
        .read_to_string(&mut body)
        .map_err(io_error)?;
    serde_json::from_str(&body).map_err(network)
}
fn wire_url(base: &str, path: &str) -> Result<reqwest::Url, AppError> {
    relative(path)?;
    let mut url = reqwest::Url::parse(base).map_err(network)?;
    url.path_segments_mut()
        .map_err(|_| error("transfer_peer", "Invalid URL"))?
        .pop_if_empty()
        .push("files")
        .extend(path.split('/'));
    Ok(url)
}
async fn control_command(
    State(manager): State<TransferManager>,
    Json(command): Json<Command>,
) -> Result<Json<Value>, ApiError> {
    blocking(move || manager.command(command, false)).await
}
