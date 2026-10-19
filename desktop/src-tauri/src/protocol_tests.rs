use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone)]
enum Reply {
    Success,
    Unauthorized,
    Redirect(String),
    ToolsError,
    MalformedInitialize,
    MalformedInitializeWithSession,
    ReflectedCredential,
    DifferentName,
    DifferentVersion,
    DifferentCount,
}

#[derive(Debug)]
struct Request {
    method: String,
    authenticated: bool,
    session: bool,
    protocol: bool,
}

struct Fixture {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new(token: &str, reply: Reply) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&requests);
        let stop = Arc::clone(&stopping);
        let token = token.to_owned();
        let handle = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                if stop.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let is_delete = line.starts_with("DELETE ");
                let mut length = 0;
                let mut authenticated = false;
                let mut session = false;
                let mut protocol = false;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        match name.to_ascii_lowercase().as_str() {
                            "content-length" => length = value.trim().parse().unwrap(),
                            "authorization" => {
                                authenticated = value.trim() == format!("Bearer {token}")
                            }
                            "mcp-session-id" => session = value.trim() == "fixture-session",
                            "mcp-protocol-version" => protocol = value.trim() == "2025-03-26",
                            _ => {}
                        }
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let method = if is_delete {
                    "DELETE"
                } else {
                    body["method"].as_str().unwrap_or("unknown")
                };
                captured.lock().unwrap().push(Request {
                    method: method.to_owned(),
                    authenticated,
                    session,
                    protocol,
                });
                let (status, headers, body) = match (&reply, method) {
                    (Reply::Unauthorized, _) => ("401 Unauthorized", String::new(), String::new()),
                    (Reply::Redirect(target), _) => (
                        "307 Temporary Redirect",
                        format!("Location: {target}\r\n"),
                        String::new(),
                    ),
                    (Reply::MalformedInitialize, "initialize") => (
                        "200 OK",
                        String::new(),
                        json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string(),
                    ),
                    (Reply::MalformedInitializeWithSession, "initialize") => (
                        "200 OK",
                        "Mcp-Session-Id: fixture-session\r\n".into(),
                        json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string(),
                    ),
                    (_, "initialize") => (
                        "200 OK",
                        "Mcp-Session-Id: fixture-session\r\n".into(),
                        json!({
                            "jsonrpc":"2.0", "id":1,
                            "result":{
                                "protocolVersion":"2025-03-26",
                                "capabilities":{"tools":{}},
                                "serverInfo":{
                                    "name":if matches!(reply, Reply::DifferentName) {"different-mcp"} else {"fixture-windows-mcp"},
                                    "version":if matches!(reply, Reply::DifferentVersion) {"9.9.9"} else {"1.2.3"}
                                }
                            }
                        }).to_string(),
                    ),
                    (_, "notifications/initialized" | "DELETE") => {
                        ("202 Accepted", String::new(), String::new())
                    }
                    (Reply::ToolsError, "tools/list") => (
                        "200 OK",
                        String::new(),
                        json!({"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"Tool discovery failed"}}).to_string(),
                    ),
                    (Reply::ReflectedCredential, "tools/list") => (
                        "200 OK",
                        String::new(),
                        json!({"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":format!("Rejected bearer {token}")}}).to_string(),
                    ),
                    (Reply::DifferentCount, "tools/list") => (
                        "200 OK",
                        String::new(),
                        json!({"jsonrpc":"2.0","id":2,"result":{"tools":[]}}).to_string(),
                    ),
                    (_, "tools/list") => (
                        "200 OK",
                        String::new(),
                        json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{
                            "name":"DisplayInventory",
                            "description":"Display inventory fixture",
                            "inputSchema":{"type":"object","properties":{}}
                        }]}}).to_string(),
                    ),
                    _ => ("400 Bad Request", String::new(), String::new()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            requests,
            stopping,
            thread: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.method.clone())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Wake accept without polling or arbitrary sleeps, including test panic.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn config() -> Config {
    Config::new().unwrap()
}

#[test]
fn every_requested_endpoint_completes_authenticated_mcp_and_releases_session() {
    // HP-03: success must describe all endpoints, not just loopback.
    let config = config();
    let local = Fixture::new(&config.token, Reply::Success);
    let second = Fixture::new(&config.token, Reply::Success);
    let urls = vec![local.url(), second.url()];
    let result = probe_urls(&config, &urls).unwrap();
    assert_eq!(result.verified_urls, urls);
    assert_eq!(result.tool_count, 1);
    assert_eq!(result.server_name, "fixture-windows-mcp");
    assert_eq!(result.server_version, "1.2.3");
    for fixture in [&local, &second] {
        assert_eq!(
            fixture.methods(),
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "DELETE"
            ]
        );
        for request in fixture.requests.lock().unwrap().iter() {
            assert!(request.authenticated, "{request:?}");
            if request.method != "initialize" {
                assert!(request.session && request.protocol, "{request:?}");
            }
        }
    }
}

#[test]
fn unauthorized_second_endpoint_cannot_report_loopback_success() {
    // ERR-03.
    let config = config();
    let local = Fixture::new(&config.token, Reply::Success);
    let lan = Fixture::new(&config.token, Reply::Unauthorized);
    let error = probe_urls(&config, &[local.url(), lan.url()])
        .err()
        .unwrap();
    assert_eq!(error.code, "lan_connection_failed");
    assert!(error.detail.contains(&lan.url()), "{}", error.detail);
    assert!(error.detail.contains("401"), "{}", error.detail);
    assert_eq!(local.methods().last().unwrap(), "DELETE");
}

#[test]
fn lan_redirect_is_rejected_without_forwarding_credentials() {
    // ERR-04: a redirect must not turn a wrong LAN endpoint into a success.
    let config = config();
    let local = Fixture::new(&config.token, Reply::Success);
    let target = Fixture::new(&config.token, Reply::Success);
    let lan = Fixture::new(&config.token, Reply::Redirect(target.url()));
    let error = probe_urls(&config, &[local.url(), lan.url()])
        .err()
        .unwrap();
    assert_eq!(error.code, "lan_connection_failed");
    assert!(error.detail.contains(&lan.url()));
    assert!(error.detail.contains("307"), "{}", error.detail);
    assert!(target.methods().is_empty());
}

#[test]
fn invalid_rpc_results_fail_and_tool_discovery_failure_still_releases_session() {
    // ERR-05, EDGE-02.
    let config = config();
    for reply in [Reply::ToolsError, Reply::MalformedInitialize] {
        let fixture = Fixture::new(&config.token, reply.clone());
        let error = probe_urls(&config, &[fixture.url()]).err().unwrap();
        assert_eq!(error.code, "protocol_failed");
        if matches!(reply, Reply::ToolsError) {
            assert_eq!(
                fixture.methods(),
                [
                    "initialize",
                    "notifications/initialized",
                    "tools/list",
                    "DELETE"
                ]
            );
        } else {
            assert!(!fixture.methods().contains(&"tools/list".to_owned()));
        }
    }
}

#[test]
fn server_reflected_credentials_are_redacted_from_errors() {
    // ERR-06: only generated fixture credentials, never real account tokens.
    let config = config();
    let fixture = Fixture::new(&config.token, Reply::ReflectedCredential);
    let error = probe_urls(&config, &[fixture.url()]).err().unwrap();
    assert_eq!(error.code, "protocol_failed");
    assert!(!error.detail.contains(&config.token));
    assert!(error.detail.contains("[REDACTED]"), "{}", error.detail);
    assert_eq!(fixture.methods().last().unwrap(), "DELETE");
}

#[test]
fn malformed_initialize_with_session_still_releases_it() {
    // EDGE-03: the server may create a session before sending invalid metadata.
    let config = config();
    let fixture = Fixture::new(&config.token, Reply::MalformedInitializeWithSession);
    let error = probe_urls(&config, &[fixture.url()]).err().unwrap();
    assert_eq!(error.code, "protocol_failed");
    assert_eq!(fixture.methods(), ["initialize", "DELETE"]);
    let requests = fixture.requests.lock().unwrap();
    let cleanup = requests.last().unwrap();
    assert!(cleanup.authenticated && cleanup.session && cleanup.protocol);
}

#[test]
fn different_lan_identity_or_tool_count_is_not_the_same_server() {
    // ERR-07: all successful handshakes must describe the managed MCP backend.
    let config = config();
    for reply in [
        Reply::DifferentName,
        Reply::DifferentVersion,
        Reply::DifferentCount,
    ] {
        let local = Fixture::new(&config.token, Reply::Success);
        let lan = Fixture::new(&config.token, reply);
        let error = probe_urls(&config, &[local.url(), lan.url()])
            .err()
            .unwrap();
        assert_eq!(error.code, "lan_connection_failed");
        assert!(error.detail.contains(&lan.url()), "{}", error.detail);
        assert_eq!(local.methods().last().unwrap(), "DELETE");
        assert_eq!(lan.methods().last().unwrap(), "DELETE");
    }
}
