use super::*;
use sha2::{Digest, Sha256};
use std::io::Cursor;

struct Fixture {
    root: PathBuf,
    manager: TransferManager,
    url: String,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("wmcp-transfer-test-{}", id().unwrap()));
        let manager = TransferManager::new(
            root.join("config"),
            root.join("inbox"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        manager
            .configure(SettingsPatch {
                port: Some(port),
                lan_enabled: Some(false),
                ..Default::default()
            })
            .unwrap();
        let token = manager.snapshot().settings.token;
        manager.start().unwrap();
        Self {
            root,
            manager,
            url: format!("http://127.0.0.1:{port}"),
            token,
        }
    }
    fn http(&self) -> reqwest::blocking::Client {
        client().unwrap()
    }
    fn put(&self, name: &str, data: Vec<u8>, hash: &str) -> reqwest::blocking::Response {
        self.http()
            .put(wire_url(&self.url, name).unwrap())
            .bearer_auth(&self.token)
            .header("X-Content-SHA256", hash)
            .body(data)
            .send()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.manager.shutdown();
        assert!(self.root.starts_with(std::env::temp_dir()));
        assert!(
            self.root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wmcp-transfer-test-")
        );
        for _ in 0..30 {
            if !self.root.exists() || fs::remove_dir_all(&self.root).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
fn wait(manager: &TransferManager, id: &str) -> Task {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let task = manager
            .snapshot()
            .tasks
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        if !task.active() {
            return task;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Task timeout: {}",
            task.status
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn http_auth_origin_checksum_download_and_conflict() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let script = http
        .get(format!("{}/client/windows-mcp-transfer.py", fixture.url))
        .send()
        .unwrap();
    assert_eq!(script.status(), 200);
    let script = script.bytes().unwrap();
    assert!(script.starts_with(b"# /// script"));
    assert_eq!(hash(&script), web::client_metadata()["sha256"]);
    let connection = fixture
        .manager
        .command(
            serde_json::from_value(json!({"action":"connection"})).unwrap(),
            false,
        )
        .unwrap();
    assert_eq!(connection["client"]["sha256"], hash(&script));
    assert_eq!(
        connection["client"]["path"],
        "/client/windows-mcp-transfer.py"
    );
    assert_eq!(
        http.get(format!("{}/v1/info", fixture.url))
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{}/v1/info", fixture.url))
            .bearer_auth("bad")
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{}/v1/info", fixture.url))
            .bearer_auth(&fixture.token)
            .header("Origin", "http://untrusted.invalid")
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.post(format!("{}/control", fixture.url))
            .bearer_auth(&fixture.token)
            .json(&json!({"action":"stop"}))
            .send()
            .unwrap()
            .status(),
        404
    );
    fs::create_dir(fixture.root.join("inbox/.wmcp-private")).unwrap();
    fs::write(
        fixture.root.join("inbox/.wmcp-private/payload"),
        b"unverified",
    )
    .unwrap();
    for path in [".wmcp-private/payload", ".WMCP-private/payload"] {
        assert_eq!(
            http.get(format!("{}/files/{path}", fixture.url))
                .bearer_auth(&fixture.token)
                .send()
                .unwrap()
                .status(),
            400
        );
    }
    let bytes = vec![73u8; 6 * 1024 * 1024 + 17];
    let expected = hash(&bytes);
    let response = fixture.put("中文/样本.bin", bytes.clone(), &expected);
    assert_eq!(response.status(), 201);
    assert_eq!(response.json::<Value>().unwrap()["verified"], true);
    assert_eq!(
        fs::read(fixture.root.join("inbox/中文/样本.bin")).unwrap(),
        bytes
    );
    let returned = http
        .get(wire_url(&fixture.url, "中文/样本.bin").unwrap())
        .bearer_auth(&fixture.token)
        .send()
        .unwrap()
        .bytes()
        .unwrap();
    assert_eq!(hash(&returned), expected);
    let mut url = wire_url(&fixture.url, "中文/样本.bin").unwrap();
    url.set_query(Some("hash"));
    assert_eq!(
        http.get(url)
            .bearer_auth(&fixture.token)
            .send()
            .unwrap()
            .text()
            .unwrap()
            .trim(),
        expected
    );
    assert_eq!(
        fixture.put("中文/样本.bin", vec![9], &hash(&[9])).status(),
        409
    );
    assert_eq!(
        hash(&fs::read(fixture.root.join("inbox/中文/样本.bin")).unwrap()),
        expected
    );
    assert_eq!(
        fixture
            .put("bad.bin", vec![1, 2, 3], &"0".repeat(64))
            .status(),
        422
    );
    assert!(!fixture.root.join("inbox/bad.bin").exists());
    assert_eq!(fixture.put("empty.bin", vec![], &hash(&[])).status(), 201);
    assert!(
        fixture
            .manager
            .snapshot()
            .tasks
            .iter()
            .any(|t| t.status == "completed" && t.transferred_bytes > 1024 * 1024)
    );
}

#[test]
fn browser_cookie_and_zip_use_library() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let login = http
        .post(format!("{}/v1/session", fixture.url))
        .json(&json!({"token":fixture.token}))
        .send()
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    let cookie = cookie.split(';').next().unwrap();
    assert_eq!(
        http.get(format!("{}/v1/info", fixture.url))
            .header("Cookie", cookie)
            .send()
            .unwrap()
            .status(),
        200
    );
    let data = b"folder test".to_vec();
    assert_eq!(
        fixture
            .put("folder/nested/file.txt", data.clone(), &hash(&data))
            .status(),
        201
    );
    let mut url = wire_url(&fixture.url, "folder").unwrap();
    url.set_query(Some("zip"));
    let response = http.get(url).header("Cookie", cookie).send().unwrap();
    assert_eq!(response.status(), 200);
    let archive = response.bytes().unwrap();
    let zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
    assert!(
        zip.file_names()
            .any(|name| name.ends_with("nested/file.txt"))
    );
    let mut url = wire_url(&fixture.url, "folder").unwrap();
    url.set_query(Some("json"));
    let listing = http.get(url).header("Cookie", cookie).send().unwrap();
    assert_eq!(listing.status(), 200);
    assert!(listing.json::<Value>().unwrap()["paths"].is_array());
    assert_eq!(
        http.delete(format!("{}/v1/session", fixture.url))
            .header("Cookie", cookie)
            .send()
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{}/v1/info", fixture.url))
            .header("Cookie", cookie)
            .send()
            .unwrap()
            .status(),
        401
    );
}

#[test]
fn folder_transfers_work_in_both_directions_and_preserve_empty_directories() {
    let a = Fixture::new();
    let b = Fixture::new();
    let source = a.root.join("source/目录");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::create_dir_all(source.join("nested")).unwrap();
    let data = vec![51u8; 10 * 1024 * 1024];
    fs::write(source.join("nested/data.bin"), &data).unwrap();
    fs::write(source.join("zero"), []).unwrap();
    a.manager
        .add_peer(PeerInput {
            name: "B".into(),
            url: b.url.clone(),
            token: b.token.clone(),
        })
        .unwrap();
    let peer = a.manager.snapshot().settings.peers[0].id.clone();
    let task = a
        .manager
        .send(&peer, vec![source.to_string_lossy().into()])
        .unwrap();
    let finished = wait(&a.manager, &task.id);
    assert_eq!(finished.status, "completed", "{:?}", finished.error);
    assert_eq!(finished.transferred_bytes, finished.total_bytes);
    assert!(b.root.join("inbox/目录/empty").is_dir());
    assert_eq!(
        hash(&fs::read(b.root.join("inbox/目录/nested/data.bin")).unwrap()),
        hash(&data)
    );
    b.manager
        .add_peer(PeerInput {
            name: "A".into(),
            url: a.url.clone(),
            token: a.token.clone(),
        })
        .unwrap();
    let peer = b.manager.snapshot().settings.peers[0].id.clone();
    let task = b
        .manager
        .send(
            &peer,
            vec![b.root.join("inbox/目录").to_string_lossy().into()],
        )
        .unwrap();
    let finished = wait(&b.manager, &task.id);
    assert_eq!(finished.status, "completed", "{:?}", finished.error);
    assert_eq!(
        hash(&fs::read(a.root.join("inbox/目录/nested/data.bin")).unwrap()),
        hash(&data)
    );
    let repeat = a
        .manager
        .send(
            &a.manager.snapshot().settings.peers[0].id,
            vec![source.to_string_lossy().into()],
        )
        .unwrap();
    assert_eq!(
        wait(&a.manager, &repeat.id).error.unwrap().code,
        "transfer_conflict"
    );
}

#[test]
fn persistence_redaction_path_validation_and_cancel() {
    let fixture = Fixture::new();
    let saved = fs::read_to_string(fixture.root.join("config/transfer.json")).unwrap();
    assert!(!saved.contains(&fixture.token));
    let settings = load(&fixture.root.join("config"), &fixture.root.join("unused")).unwrap();
    assert_eq!(settings.token, fixture.token);
    let command = serde_json::from_value(json!({"action":"status"})).unwrap();
    let value = fixture.manager.command(command, false).unwrap();
    assert_eq!(value["settings"]["token"], "[REDACTED]");
    for value in [
        "../escape",
        "x\\escape",
        "x:stream",
        "CON",
        "a/../b",
        ".wmcp-secret",
        ".WMCP-secret",
        "C:/file",
        "x.",
    ] {
        assert!(relative(value).is_err(), "{value}");
    }
    for value in [
        "http://example.com:8002",
        "http://8.8.8.8",
        "http://127.0.0.1@8.8.8.8",
        "http://127.0.0.1:8002/path",
    ] {
        assert!(peer_url(value).is_err());
    }
    assert!(
        fixture
            .manager
            .configure(SettingsPatch {
                port: Some(8003),
                ..Default::default()
            })
            .is_err()
    );
    fixture
        .manager
        .add_peer(PeerInput {
            name: "Offline".into(),
            url: "http://127.0.0.1:1".into(),
            token: fixture.token.clone(),
        })
        .unwrap();
    let peer = fixture.manager.snapshot().settings.peers[0].id.clone();
    let file = fixture.root.join("cancel.bin");
    fs::write(&file, vec![0; 32 * 1024 * 1024]).unwrap();
    let task = fixture
        .manager
        .send(&peer, vec![file.to_string_lossy().into()])
        .unwrap();
    fixture.manager.cancel(&task.id).unwrap();
    assert_eq!(wait(&fixture.manager, &task.id).status, "cancelled");
    fixture.manager.stop();
    assert!(!fixture.manager.snapshot().running);
}
