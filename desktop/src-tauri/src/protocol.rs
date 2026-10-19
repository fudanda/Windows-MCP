use crate::{AppError, config::Config};
use reqwest::{
    blocking::{Client, Response},
    header::HeaderValue,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub server_name: String,
    pub server_version: String,
    pub tool_count: usize,
    pub latency_ms: u128,
    pub verified_urls: Vec<String>,
}

fn decode_text(text: &str) -> Result<Value, AppError> {
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                if value.get("result").is_some() || value.get("error").is_some() {
                    return Ok(value);
                }
            }
        }
    }
    Err(AppError::new(
        "protocol_failed",
        "Invalid JSON-RPC response",
    ))
}

fn decode(response: Response) -> Result<Value, AppError> {
    if !response.status().is_success() {
        return Err(AppError::new(
            "connection_failed",
            format!("HTTP {}", response.status().as_u16()),
        ));
    }
    let value = decode_text(&response.text().map_err(AppError::connection)?)?;
    if let Some(error) = value.get("error") {
        return Err(AppError::new("protocol_failed", error.to_string()));
    }
    Ok(value)
}

pub fn probe(config: &Config) -> Result<Probe, AppError> {
    let mut urls = vec![config.local_url()];
    if config.lan_enabled {
        let addresses = crate::network::addresses()?;
        if addresses.is_empty() {
            return Err(AppError::new(
                "lan_unavailable",
                "No active private IPv4 address",
            ));
        }
        urls.extend(
            addresses
                .into_iter()
                .map(|address| format!("http://{}:{}/mcp", address.ip, config.port)),
        );
    }
    probe_urls(config, &urls)
}

pub fn probe_local(config: &Config) -> Result<Probe, AppError> {
    probe_urls(config, &[config.local_url()])
}

fn probe_urls(config: &Config, urls: &[String]) -> Result<Probe, AppError> {
    let started = Instant::now();
    let mut result: Option<Probe> = None;
    for (index, url) in urls.iter().enumerate() {
        let probe = probe_at(config, url).map_err(|error| {
            AppError::new(
                if index == 0 {
                    &error.code
                } else {
                    "lan_connection_failed"
                },
                format!(
                    "{url}: {}",
                    error.detail.replace(&config.token, "[REDACTED]")
                ),
            )
        })?;
        if let Some(result) = &mut result {
            if result.server_name != probe.server_name
                || result.server_version != probe.server_version
                || result.tool_count != probe.tool_count
            {
                return Err(AppError::new(
                    "lan_connection_failed",
                    format!("{url}: MCP server identity or tool count differs from loopback"),
                ));
            }
            result.verified_urls.push(url.clone());
        } else {
            result = Some(probe);
        }
    }
    let mut result =
        result.ok_or_else(|| AppError::new("lan_unavailable", "No MCP endpoint to verify"))?;
    result.latency_ms = started.elapsed().as_millis();
    Ok(result)
}

fn probe_at(config: &Config, url: &str) -> Result<Probe, AppError> {
    let started = Instant::now();
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(AppError::connection)?;
    let response = client.post(url).bearer_auth(&config.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"windows-mcp-manager","version":"0.1.0"}}}))
        .send().map_err(AppError::connection)?;
    let session = response.headers().get("mcp-session-id").cloned();
    let mut version = String::from("2025-03-26");
    // Cleanup also covers an invalid initialize response carrying a session ID.
    let result = (|| {
        let initialize = decode(response)?;
        version = initialize["result"]["protocolVersion"]
            .as_str()
            .ok_or_else(|| AppError::new("protocol_failed", "Missing protocol version"))?
            .into();
        let server_name = initialize["result"]["serverInfo"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| AppError::new("protocol_failed", "Missing server identity"))?;
        let send = |body: Value| {
            let mut request = client
                .post(url)
                .bearer_auth(&config.token)
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", &version)
                .json(&body);
            if let Some(id) = &session {
                request = request.header("mcp-session-id", id);
            }
            request.send().map_err(AppError::connection)
        };
        // Always release a stateful session, including on failed tool discovery.
        let response = send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "protocol_failed",
                "Initialize notification rejected",
            ));
        }
        let tools = decode(send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?)?;
        let tools = tools["result"]["tools"]
            .as_array()
            .ok_or_else(|| AppError::new("protocol_failed", "Missing tool list"))?;
        Ok(Probe {
            server_name: server_name.into(),
            server_version: initialize["result"]["serverInfo"]["version"]
                .as_str()
                .unwrap_or("")
                .into(),
            tool_count: tools.len(),
            latency_ms: started.elapsed().as_millis(),
            verified_urls: vec![url.into()],
        })
    })();
    if let Some(id) = session {
        let _ = client
            .delete(url)
            .bearer_auth(&config.token)
            .header(
                "MCP-Protocol-Version",
                HeaderValue::from_str(&version).unwrap_or(HeaderValue::from_static("2025-03-26")),
            )
            .header("mcp-session-id", id)
            .send();
    }
    result
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_json_and_sse_without_assuming_content_type() {
        assert_eq!(
            decode_text(r#"{"result":{"tools":[]}}"#).unwrap()["result"]["tools"],
            json!([])
        );
        assert_eq!(
            decode_text("event: message\ndata: {\"id\":1,\"result\":{}}\n\n").unwrap()["id"],
            1
        );
        assert!(decode_text("invalid").is_err());
    }
}
