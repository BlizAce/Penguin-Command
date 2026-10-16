//! MCP Streamable HTTP transport: POST every JSON-RPC message to a single
//! endpoint; replies arrive as plain JSON or as an SSE stream (`data:` lines).
//! The `Mcp-Session-Id` header from the initialize response is echoed on all
//! later requests, per spec.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

fn block<T>(fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("spawn mcp http runtime")?
        .block_on(fut)
}

/// Pick the JSON-RPC reply out of a body that may be plain JSON or SSE.
pub fn parse_reply(body: &str, expected_id: u64) -> Option<Value> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Plain JSON (possibly several messages concatenated? spec says one).
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            let id = v.get("id").and_then(|i| i.as_u64());
            if id == Some(expected_id) || id.is_none() {
                return Some(v);
            }
        }
    }
    // SSE: collect data: payloads, prefer the one answering our id.
    let mut fallback: Option<Value> = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            if v.get("id").and_then(|i| i.as_u64()) == Some(expected_id) {
                return Some(v);
            }
            if fallback.is_none() && v.get("method").is_none() {
                fallback = Some(v);
            }
        }
    }
    fallback
}

pub struct HttpClient {
    url: String,
    headers: BTreeMap<String, String>,
    session_id: Option<String>,
}

impl HttpClient {
    pub fn new(url: String, headers: BTreeMap<String, String>) -> Self {
        Self { url, headers, session_id: None }
    }

    async fn post(&mut self, msg: &Value, timeout: Duration) -> Result<(u16, Option<String>, String)> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15).min(timeout))
            .build()
            .context("http client")?;
        let mut req = client
            .post(&self.url)
            .timeout(timeout)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(sid) = &self.session_id {
            req = req.header("mcp-session-id", sid.as_str());
        }
        let resp = tokio::time::timeout(timeout, req.json(msg).send())
            .await
            .map_err(|_| anyhow!("transport timeout posting to {}", self.url))?
            .with_context(|| format!("POST {}", self.url))?;
        let status = resp.status().as_u16();
        let session = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let body = tokio::time::timeout(timeout, resp.text())
            .await
            .map_err(|_| anyhow!("timed out reading response body"))?
            .context("reading response body")?;
        Ok((status, session, body))
    }

    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let (status, session, body) = block(self.post(&msg, timeout))?;
        if let Some(sid) = session {
            self.session_id = Some(sid);
        }
        if status >= 400 {
            return Err(anyhow!("transport HTTP {status}: {}", crate::providers::truncate(&body, 300)));
        }
        // An initialize response establishes the session even with no body id.
        let reply = parse_reply(&body, id).ok_or_else(|| anyhow!("no JSON-RPC reply from {} (HTTP {status})", self.url))?;
        if let Some(err) = reply.get("error") {
            return Err(anyhow!("mcp server error: {}: {}", err["code"].as_i64().unwrap_or(-1), err["message"].as_str().unwrap_or("json-rpc error")));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&mut self, method: &str, params: Value, timeout: Duration) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let (status, _, _) = block(self.post(&msg, timeout))?;
        if status >= 400 {
            return Err(anyhow!("notification rejected with HTTP {status}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_reply;
    use serde_json::json;

    #[test]
    fn plain_json_reply() {
        let body = json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}}).to_string();
        assert_eq!(parse_reply(&body, 7).unwrap()["result"]["ok"], true);
        assert!(parse_reply(&body, 8).is_none());
    }

    #[test]
    fn sse_reply_picks_matching_id() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"tools\":[]}}\n\n";
        let got = parse_reply(body, 3).unwrap();
        assert!(got["result"]["tools"].is_array());
    }

    #[test]
    fn sse_fallback_without_id_match() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{\"x\":1}}\n";
        assert_eq!(parse_reply(body, 3).unwrap()["result"]["x"], 1);
    }

    #[test]
    fn empty_and_done_bodies() {
        assert!(parse_reply("", 1).is_none());
        assert!(parse_reply("data: [DONE]\n", 1).is_none());
    }
}
