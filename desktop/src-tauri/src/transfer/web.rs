use super::*;
use axum::{
    body::{Body, Bytes},
    http::{Method, Uri, header},
    routing::{any, delete},
};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::pin::Pin;

static WEB: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../dist");
static CLI: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/windows-mcp-transfer.py"
));
pub(super) fn client_metadata() -> Value {
    json!({
        "path":"/client/windows-mcp-transfer.py",
        "sha256":format!("{:x}",Sha256::digest(CLI)),
        "runtime":"Python >=3.10; uv can prepare Python and the isolated HTTPX/Rich dependencies",
        "command":"uv run --script windows-mcp-transfer.py",
        "tokenEnv":"WINDOWS_MCP_TRANSFER_TOKEN",
        "urlEnv":"WINDOWS_MCP_TRANSFER_URL",
        "commands":["upload","download","list","status","cancel"],
        "upload":"upload <CLIENT_LOCAL_PATH> --to <REMOTE_PARENT_DIRECTORY>",
        "download":"download <REMOTE_PATH> --to <CLIENT_LOCAL_DESTINATION>",
        "progress":"--json emits JSON Lines progress and verified checksums"
    })
}
async fn client_script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/x-python; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"windows-mcp-transfer.py\"",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        CLI,
    )
        .into_response()
}
pub(super) fn router(manager: TransferManager, token: String) -> Router {
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/tasks", get(tasks))
        .route("/v1/tasks/{id}", delete(cancel))
        .route("/files", any(files_root))
        .route("/files/", any(files_root))
        .route("/files/{*path}", any(files))
        .layer(DefaultBodyLimit::disable())
        .route_layer(middleware::from_fn_with_state(
            Auth {
                token,
                manager: Some(manager.clone()),
            },
            auth,
        ))
        .route(
            "/v1/session",
            post(login)
                .delete(logout)
                .layer(DefaultBodyLimit::max(1024)),
        )
        .route("/client/windows-mcp-transfer.py", get(client_script))
        .fallback(static_file)
        .with_state(manager)
}
pub(super) fn origin_allowed(headers: &HeaderMap) -> bool {
    match headers.get("origin") {
        None => true,
        Some(origin) => headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|host| origin.as_bytes() == format!("http://{host}").as_bytes()),
    }
}
fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|s| s.strip_prefix("wmcp_transfer="))
}
pub(super) fn session_valid(manager: &TransferManager, headers: &HeaderMap) -> bool {
    cookie(headers).is_some_and(|id| {
        manager
            .lock()
            .sessions
            .get(id)
            .is_some_and(|expiry| *expiry > now())
    })
}
#[derive(Deserialize)]
struct Login {
    token: String,
}
async fn login(
    State(manager): State<TransferManager>,
    headers: HeaderMap,
    Json(login): Json<Login>,
) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut supplied = HeaderMap::new();
    let Ok(value) = format!("Bearer {}", login.token).parse() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    supplied.insert(header::AUTHORIZATION, value);
    let mut data = manager.lock();
    if data.server.is_none() || !authorized(&supplied, &data.settings.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code":"unauthorized"})),
        )
            .into_response();
    }
    data.sessions.retain(|_, expiry| *expiry > now());
    if data.sessions.len() >= 16 {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(session) = id() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    data.sessions
        .insert(session.clone(), now() + 8 * 60 * 60 * 1000);
    (
        [(
            header::SET_COOKIE,
            format!("wmcp_transfer={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800"),
        )],
        Json(json!({"connected":true})),
    )
        .into_response()
}
async fn logout(State(manager): State<TransferManager>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(session) = cookie(&headers) {
        manager.lock().sessions.remove(session);
    }
    (
        [(
            header::SET_COOKIE,
            "wmcp_transfer=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        )],
        Json(json!({"disconnected":true})),
    )
        .into_response()
}
async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() || path == "transfer" {
        "index.html"
    } else {
        path
    };
    let Some(file) = WEB.get_file(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    };
    let body = if path == "index.html" {
        Body::from(String::from_utf8_lossy(file.contents()).replace(
            "<head>",
            "<head><meta name=\"windows-mcp-transfer\" content=\"1\">",
        ))
    } else {
        Body::from(file.contents())
    };
    ([(header::CONTENT_TYPE,mime),(header::CACHE_CONTROL,"no-store"),(header::X_CONTENT_TYPE_OPTIONS,"nosniff"),(header::X_FRAME_OPTIONS,"DENY"),(header::CONTENT_SECURITY_POLICY,"default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'")],body).into_response()
}
async fn info() -> Json<Value> {
    Json(
        json!({"name":std::env::var("COMPUTERNAME").unwrap_or_else(|_|"Windows".into()),"protocol":"windows-mcp-transfer/1","engine":"Dufs 0.46.0","client":client_metadata()}),
    )
}
async fn tasks(State(manager): State<TransferManager>) -> Json<Value> {
    let tasks:Vec<_>=manager.snapshot().tasks.iter().filter(|t|t.direction!="send").map(|t|json!({"id":t.id,"name":t.name,"direction":t.direction,"status":t.status,"totalBytes":t.total_bytes,"transferredBytes":t.transferred_bytes,"startedAt":t.started_at,"finishedAt":t.finished_at,"error":t.error.as_ref().map(|e|json!({"code":e.code}))})).collect();
    Json(json!({"tasks":tasks}))
}
async fn cancel(
    State(manager): State<TransferManager>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        if manager
            .lock()
            .tasks
            .iter()
            .any(|t| t.id == id && t.direction == "send")
        {
            return Err(error(
                "transfer_path",
                "Cannot cancel local outbound tasks from the file sharing page",
            ));
        }
        manager.cancel(&id)?;
        Ok(json!({"cancelled":true}))
    })
    .await
}
fn upstream_url(
    backend: &Backend,
    path: &str,
    query: Option<&str>,
) -> Result<reqwest::Url, AppError> {
    let mut url = reqwest::Url::parse(&backend.url).map_err(network)?;
    {
        let mut parts = url
            .path_segments_mut()
            .map_err(|_| network("Invalid backend URL"))?;
        parts.pop_if_empty();
        if !path.is_empty() {
            parts.extend(path.split('/'));
        }
    }
    url.set_query(query);
    Ok(url)
}
fn destination(root: &Path, path: &str) -> Result<PathBuf, AppError> {
    relative(path)?;
    let mut target = root.to_path_buf();
    let parts: Vec<_> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        target.push(part);
        if target.exists() {
            ordinary(&target)?;
            if i + 1 < parts.len() && !target.is_dir() {
                return Err(error("transfer_path", "A parent is not a directory"));
            }
        }
    }
    Ok(target)
}
fn async_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(network)
}
async fn files_root(
    State(manager): State<TransferManager>,
    request: Request,
) -> Result<Response, ApiError> {
    proxy(manager, String::new(), request).await
}
async fn files(
    State(manager): State<TransferManager>,
    axum::extract::Path(path): axum::extract::Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    proxy(manager, path.trim_end_matches('/').to_string(), request).await
}
async fn proxy(
    manager: TransferManager,
    path: String,
    request: Request,
) -> Result<Response, ApiError> {
    let backend = manager.backend()?;
    if !path.is_empty() {
        destination(&backend.root, &path)?;
    }
    let query = request.uri().query().map(str::to_string);
    if let Some(query) = &query {
        if !matches!(query.as_str(), "json" | "hash" | "zip") {
            return Err(error("transfer_path", "Unsupported file query").into());
        }
    }
    let method = request.method().clone();
    let client = async_client()?;
    if method == Method::PUT {
        return upload(manager, backend, path, request, client).await;
    }
    if !matches!(method.as_str(), "GET" | "HEAD" | "MKCOL") {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    if method.as_str() == "MKCOL" && path.is_empty() {
        return Err(error("transfer_path", "Choose a folder name").into());
    }
    let mut url = upstream_url(&backend, &path, query.as_deref())?;
    if !path.is_empty() && destination(&backend.root, &path)?.is_dir() {
        url.path_segments_mut()
            .map_err(|_| network("Invalid URL"))?
            .push("");
    }
    let mut outgoing = client
        .request(method.clone(), url)
        .basic_auth("transfer", Some(&backend.token));
    if let Some(range) = request.headers().get(header::RANGE) {
        outgoing = outgoing.header(header::RANGE, range);
    }
    let response = outgoing.send().await.map_err(network)?;
    let status = response.status();
    let metadata = query.as_deref().is_some_and(|q| q == "json" || q == "hash");
    let mut builder = axum::http::Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    for name in [
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    if metadata {
        builder = builder.header(
            header::CONTENT_TYPE,
            if query.as_deref() == Some("json") {
                "application/json"
            } else {
                "text/plain"
            },
        );
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/octet-stream");
        let name = path
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("Shared");
        let name = if query.as_deref() == Some("zip") {
            format!("{name}.zip")
        } else {
            name.to_string()
        };
        let encoded: String = name
            .as_bytes()
            .iter()
            .map(|b| format!("%{b:02X}"))
            .collect();
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"download\"; filename*=UTF-8''{encoded}"),
        );
    }
    if method != Method::GET || !status.is_success() || metadata {
        return builder
            .body(Body::from_stream(response.bytes_stream()))
            .map_err(|e| ApiError(io_error(e)));
    }
    let mut task = Task::new("download", "Browser".into(), path.clone())?;
    task.status = "transferring".into();
    task.total_bytes = response.content_length().unwrap_or(0);
    task.file_count = 1;
    manager.add_task(task.clone())?;
    let state = Download {
        stream: Box::pin(response.bytes_stream()),
        manager,
        id: task.id,
        cancel: task.cancel,
        done: false,
    };
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        if state.cancel.load(Ordering::Relaxed) {
            state.done = true;
            state
                .manager
                .finish(&state.id, Err(error("transfer_cancelled", "")));
            return Some((Err(network("Download cancelled")), state));
        }
        match state.stream.next().await {
            Some(Ok(bytes)) => {
                state
                    .manager
                    .update(&state.id, |t| t.transferred_bytes += bytes.len() as u64);
                Some((Ok(bytes), state))
            }
            Some(Err(e)) => {
                state.done = true;
                state.manager.finish(&state.id, Err(network(e)));
                Some((Err(network("Download interrupted")), state))
            }
            None => {
                state.done = true;
                state.manager.update(&state.id, |t| {
                    if t.total_bytes == 0 {
                        t.total_bytes = t.transferred_bytes;
                    }
                });
                state
                    .manager
                    .finish(&state.id, Ok("Browser download".into()));
                None
            }
        }
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| ApiError(io_error(e)))
}
struct Download {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    manager: TransferManager,
    id: String,
    cancel: Arc<AtomicBool>,
    done: bool,
}
impl Drop for Download {
    fn drop(&mut self) {
        if !self.done {
            self.manager.finish(
                &self.id,
                Err(error("transfer_cancelled", "Browser disconnected")),
            );
        }
    }
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let path = self.0.clone();
        thread::spawn(move || {
            for _ in 0..30 {
                if !path.exists() || fs::remove_dir_all(&path).is_ok() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    }
}
async fn upload(
    manager: TransferManager,
    backend: Backend,
    path: String,
    request: Request,
    client: reqwest::Client,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(error("transfer_path", "Missing file name").into());
    }
    let expected = request
        .headers()
        .get("x-content-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(error("transfer_integrity", "A SHA-256 checksum is required").into());
    }
    let total = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| error("transfer_limit", "Content-Length is required"))?;
    if total > MAX_TOTAL_BYTES {
        return Err(error("transfer_limit", "File exceeds 1 TiB").into());
    }
    let target = destination(&backend.root, &path)?;
    if target.exists() {
        return Err(error("transfer_conflict", "A file with this name already exists").into());
    }
    let mut task = Task::new("receive", "Browser / MCP".into(), path.clone())?;
    task.total_bytes = total;
    task.file_count = 1;
    task.status = "transferring".into();
    manager.add_task(task.clone())?;
    let stage_name = format!(".wmcp-{}", task.id);
    let stage_path = backend.root.join(&stage_name);
    if let Err(e) = fs::create_dir(&stage_path) {
        let e = io_error(e);
        manager.finish(&task.id, Err(e.clone()));
        return Err(e.into());
    }
    let _cleanup = Stage(stage_path.clone());
    let result = async {
        let state = Arc::new(Mutex::new((Sha256::new(), 0u64)));
        let digest = state.clone();
        let progress = manager.clone();
        let task_id = task.id.clone();
        let flag = task.cancel.clone();
        let stream = futures_util::stream::unfold((request.into_body().into_data_stream(), std::time::Instant::now(), false), move |(mut input, mut last, done)| {
            let flag=flag.clone(); let digest=digest.clone(); let progress=progress.clone(); let task_id=task_id.clone();
            async move {
                if done {return None;}
                loop {
                    if let Err(e)=check_cancel(&flag) {return Some((Err(e),(input,last,true)));}
                    if last.elapsed()>Duration::from_secs(30) {return Some((Err(network("Upload idle timeout")),(input,last,true)));}
                    tokio::select! {
                        chunk=input.next()=>{
                            let chunk=chunk?;
                            last=std::time::Instant::now();
                            let result=(|| {
                                let bytes=chunk.map_err(network)?;let mut hash=digest.lock().unwrap();hash.1+=bytes.len() as u64;
                                if hash.1>total{return Err(error("transfer_integrity","File size exceeds declared length"));}
                                hash.0.update(&bytes);drop(hash);progress.update(&task_id,|t|t.transferred_bytes+=bytes.len() as u64);Ok::<_,AppError>(bytes)
                            })();
                            let failed=result.is_err();return Some((result,(input,last,failed)));
                        },
                        _=tokio::time::sleep(Duration::from_millis(100))=>{}
                    }
                }
            }
        });
        let upload_url = upstream_url(&backend, &format!("{stage_name}/payload"), None)?;
        let response = client
            .put(upload_url.clone())
            .basic_auth("transfer", Some(&backend.token))
            .header(header::CONTENT_LENGTH, total)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(network(format!("File service HTTP {}", response.status())));
        }
        check_cancel(&task.cancel)?;
        manager.update(&task.id, |t| t.status = "verifying".into());
        {
            let digest = state.lock().unwrap();
            if digest.1 != total || format!("{:x}", digest.0.clone().finalize()) != expected {
                return Err(error(
                    "transfer_integrity",
                    "Transferred content does not match SHA-256",
                ));
            }
        }
        let mut hash_url = upload_url;
        hash_url.set_query(Some("hash"));
        let hash_response = client
            .get(hash_url)
            .basic_auth("transfer", Some(&backend.token))
            .send()
            .await
            .map_err(network)?;
        let hash_status = hash_response.status();
        let stored_hash = hash_response.text().await.map_err(network)?;
        if !hash_status.is_success() || stored_hash.trim() != expected {
            return Err(error(
                "transfer_integrity",
                "Stored file SHA-256 verification failed",
            ));
        }
        check_cancel(&task.cancel)?;
        // Dufs owns HTTP I/O; publication policy keeps incomplete/unverified files private.
        let final_target = destination(&backend.root, &path)?;
        if let Some(parent) = final_target.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        destination(&backend.root, &path)?;
        platform::rename_new(&stage_path.join("payload"), &final_target)?;
        Ok(path.clone())
    }
    .await;
    match result {
        Ok(destination) => {
            manager.finish(&task.id, Ok(destination));
            Ok((
                StatusCode::CREATED,
                Json(json!({"verified":true,"sha256":expected,"taskId":task.id})),
            )
                .into_response())
        }
        Err(mut e) => {
            if task.cancel.load(Ordering::Relaxed) {
                e = error("transfer_cancelled", "");
            }
            manager.finish(&task.id, Err(e.clone()));
            Err(e.into())
        }
    }
}
