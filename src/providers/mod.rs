use crate::config::{AgentConfig, Provider, ProviderKind};
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: Role,
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// For role==Tool: id of the tool call this answers.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl ChatMsg {
    pub fn user(text: impl Into<String>) -> Self {
        ChatMsg { role: Role::User, text: text.into(), tool_calls: vec![], tool_call_id: None }
    }
    pub fn system(text: impl Into<String>) -> Self {
        ChatMsg { role: Role::System, text: text.into(), tool_calls: vec![], tool_call_id: None }
    }
    pub fn tool_result(id: impl Into<String>, text: impl Into<String>) -> Self {
        ChatMsg { role: Role::Tool, text: text.into(), tool_calls: vec![], tool_call_id: Some(id.into()) }
    }
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

#[derive(Debug, Default)]
pub struct AssistantMsg {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Generation stopped at the token limit (finish_reason=length /
    /// stop_reason=max_tokens): the tail of the reply — possibly including
    /// tool call arguments — is missing.
    pub truncated: bool,
    /// Tool calls whose streamed JSON arguments were unparseable (typical of
    /// a length-truncated stream). These are NOT executed; names only.
    pub dropped_tools: Vec<String>,
    /// Chain-of-thought characters streamed for this reply. Lets the agent
    /// tell "thinking ate the whole budget" apart from a truly empty reply.
    pub reasoning_chars: usize,
}

/// HTTP behavior tuned for slow local/remote inference endpoints: big models
/// can spend minutes loading or prefilling before the first byte, and thinking
/// models stay silent between chunks while they reason.
#[derive(Debug, Clone)]
pub struct RequestCfg {
    pub connect: Duration,
    /// Max wait for response headers (model may still be loading).
    pub first_byte: Duration,
    /// Max silence between streamed bytes before the stream is dead.
    pub idle: Duration,
    /// Retries for connect-phase failures and 429/5xx, before any output.
    pub retries: usize,
    pub backoffs: Vec<Duration>,
    /// Completion ceiling. The value actually sent is clamped per request to
    /// the room left in the provider's context window (see completion_tokens).
    pub max_tokens: u32,
    /// Thinking depth forwarded as `reasoning_effort` when set (OpenAI path).
    pub reasoning_effort: Option<String>,
    /// Cap on thinking tokens, forwarded as `max_reasoning_tokens` (llama.cpp
    /// / Ollama extensions) when set. Never sent to backends that might 400.
    pub max_reasoning_tokens: Option<u32>,
}

/// Rough prompt-token estimate (~4 chars/token incl. tool schemas) used only
/// to size the completion budget — deliberately conservative.
fn rough_prompt_tokens(msgs: &[ChatMsg], tools: &[ToolSpec]) -> usize {
    let mut chars = 0usize;
    for m in msgs {
        chars += m.text.chars().count();
        for tc in &m.tool_calls {
            chars += tc.name.len() + tc.args.to_string().len();
        }
    }
    for t in tools {
        chars += t.name.len() + t.description.len() + t.schema.to_string().len();
    }
    chars / 4 + msgs.len() * 4 + 32
}

/// Completion tokens to request: the ceiling clamped down to the space left
/// after the prompt, so a large context can never trigger an n_ctx overflow.
pub fn completion_tokens(window: usize, prompt_est: usize, ceiling: u32) -> u32 {
    // 512-token safety margin for chat-template scaffolding + tool framing
    let room = window.saturating_sub(prompt_est).saturating_sub(512).max(256);
    (ceiling as usize).min(room) as u32
}

impl Default for RequestCfg {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            first_byte: Duration::from_secs(600),
            idle: Duration::from_secs(300),
            retries: 2,
            backoffs: vec![Duration::from_secs(3), Duration::from_secs(10)],
            max_tokens: 16_384,
            reasoning_effort: None,
            max_reasoning_tokens: None,
        }
    }
}

impl RequestCfg {
    pub fn from_agent(cfg: &AgentConfig) -> Self {
        Self {
            first_byte: cfg.first_byte_timeout(),
            idle: cfg.idle_timeout(),
            retries: cfg.max_retries(),
            max_tokens: cfg.max_tokens(),
            reasoning_effort: cfg.reasoning_effort(),
            max_reasoning_tokens: cfg.max_reasoning_tokens(),
            ..Default::default()
        }
    }

    fn backoff(&self, attempt: usize) -> Duration {
        self.backoffs
            .get(attempt)
            .copied()
            .unwrap_or_else(|| *self.backoffs.last().unwrap_or(&Duration::from_secs(10)))
    }
}

fn http(connect: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .build()
        .expect("http client")
}

/// Runs a future on a private current-thread runtime — callers are sync threads.
fn block<T>(fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("spawn http runtime")?
        .block_on(fut)
}

/// Resolves as soon as the UI sets `abort` (Esc), so select! arms can drop the
/// in-flight request and close the connection instead of blocking forever.
async fn wait_abort(abort: &AtomicBool) {
    while !abort.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Sleeps unless aborted first; true when aborted.
async fn sleep_or_abort(abort: &AtomicBool, d: Duration) -> bool {
    tokio::select! {
        biased;
        _ = wait_abort(abort) => true,
        _ = tokio::time::sleep(d) => false,
    }
}

/// Accumulates streamed bytes and yields complete lines (chunks split anywhere).
#[derive(Default)]
struct SseLines {
    buf: Vec<u8>,
}

impl SseLines {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
            if !s.is_empty() {
                out.push(s);
            }
        }
        if self.buf.len() > 4 << 20 {
            self.buf.clear(); // providers frame with newlines; a runaway buffer is garbage
        }
        out
    }
}

async fn read_err_text(resp: reqwest::Response) -> String {
    tokio::time::timeout(Duration::from_secs(30), resp.text())
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

fn base(p: &Provider) -> String {
    let mut b = p.base_url.trim_end_matches('/').to_string();
    if p.kind == ProviderKind::Anthropic && !b.ends_with("/v1") {
        b.push_str("/v1");
    }
    b
}

// ---------- model listing ----------

pub fn fetch_models(p: &Provider) -> Result<Vec<String>> {
    block(fetch_models_async(p))
}

async fn fetch_models_async(p: &Provider) -> Result<Vec<String>> {
    let url = format!("{}/models", base(p));
    let mut req = http(Duration::from_secs(10)).get(&url);
    if !p.resolved_key().is_empty() {
        req = match p.kind {
            ProviderKind::Openai => req.bearer_auth(p.resolved_key()),
            ProviderKind::Anthropic => req.header("x-api-key", p.resolved_key()).header("anthropic-version", "2023-06-01"),
        };
    }
    let resp = tokio::time::timeout(Duration::from_secs(60), req.send())
        .await
        .map_err(|_| anyhow!("no response from {url} after 60s"))?
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body: Value = tokio::time::timeout(Duration::from_secs(30), resp.json())
        .await
        .map_err(|_| anyhow!("timed out reading {url} body"))?
        .with_context(|| format!("GET {url} (status {status})"))?;
    if !status.is_success() {
        return Err(anyhow!("HTTP {status}: {}", body.to_string()));
    }
    let arr = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = arr
        .iter()
        .filter_map(|m| {
            m.get("id")
                .or_else(|| m.get("name"))
                .and_then(|i| i.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

// ---------- chat ----------

fn openai_tools(tools: &[ToolSpec]) -> Value {
    json!(tools.iter().map(|t| json!({
        "type": "function",
        "function": { "name": t.name, "description": t.description, "parameters": t.schema }
    }))
    .collect::<Vec<_>>())
}

fn openai_messages(msgs: &[ChatMsg]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for m in msgs {
        match m.role {
            Role::System => out.push(json!({"role": "system", "content": m.text})),
            Role::User => out.push(json!({"role": "user", "content": m.text})),
            Role::Assistant => {
                if m.tool_calls.is_empty() {
                    out.push(json!({"role": "assistant", "content": m.text}));
                } else {
                    let tcs: Vec<Value> = m
                        .tool_calls
                        .iter()
                        .map(|tc| json!({"id": tc.id, "type": "function", "function": {"name": tc.name, "arguments": tc.args.to_string()}}))
                        .collect();
                    out.push(json!({"role": "assistant", "content": if m.text.is_empty() { Value::Null } else { json!(m.text) }, "tool_calls": tcs}));
                }
            }
            Role::Tool => out.push(json!({"role": "tool", "tool_call_id": m.tool_call_id, "content": m.text})),
        }
    }
    json!(out)
}

fn anthropic_parts(msgs: &[ChatMsg]) -> (Value, Value) {
    let mut system = String::new();
    let mut out: Vec<Value> = Vec::new();
    for m in msgs {
        match m.role {
            Role::System => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&m.text);
            }
            Role::User => out.push(json!({"role": "user", "content": [{"type": "text", "text": m.text}]})),
            Role::Tool => {
                // merge consecutive tool results into one user message
                let block = json!({"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.text});
                let merge = out.last_mut().and_then(|last| {
                    if last.get("role").and_then(|r| r.as_str()) == Some("user")
                        && last.get("content").map(|c| c.is_array()).unwrap_or(false)
                        && last["content"]
                            .as_array()
                            .map(|a| a.iter().all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")))
                            .unwrap_or(false)
                    {
                        Some(last)
                    } else {
                        None
                    }
                });
                if let Some(last) = merge {
                    last["content"].as_array_mut().unwrap().push(block);
                } else {
                    out.push(json!({"role": "user", "content": [block]}));
                }
            }
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.text.is_empty() {
                    blocks.push(json!({"type": "text", "text": m.text}));
                }
                for tc in &m.tool_calls {
                    blocks.push(json!({"type": "tool_use", "id": tc.id, "name": tc.name, "input": tc.args}));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
        }
    }
    (json!(system), json!(out))
}

pub struct StreamCtx<'a> {
    pub on_text: &'a mut dyn FnMut(&str),
    /// Chain-of-thought deltas from thinking models (Qwen3 `reasoning_content`
    /// and friends). Never part of the saved answer.
    pub on_reasoning: &'a mut dyn FnMut(&str),
    /// Human-facing progress notices (retry countdowns etc.).
    pub on_note: &'a mut dyn FnMut(&str),
    pub abort: &'a AtomicBool,
}

pub fn chat(
    p: &Provider,
    model: &str,
    msgs: &[ChatMsg],
    tools: &[ToolSpec],
    cfg: &RequestCfg,
    ctx: &mut StreamCtx,
) -> Result<AssistantMsg> {
    block(async {
        match p.kind {
            ProviderKind::Openai => chat_openai(p, model, msgs, tools, cfg, ctx).await,
            ProviderKind::Anthropic => chat_anthropic(p, model, msgs, tools, cfg, ctx).await,
        }
    })
}

/// Wraps a request future so Esc (abort) cancels it within ~100ms and a dead
/// endpoint fails after `first` instead of hanging the agent forever.
async fn send_with_guard(
    abort: &AtomicBool,
    first: Duration,
    req: impl Future<Output = Result<reqwest::Response, reqwest::Error>>,
) -> Result<Option<reqwest::Response>> {
    tokio::select! {
        biased;
        _ = wait_abort(abort) => Ok(None),
        r = tokio::time::timeout(first, req) => match r {
            Err(_) => Err(anyhow!(
                "no response after {}s — is the model still loading or the endpoint down? (tune agent.first_byte_timeout_secs)",
                first.as_secs()
            )),
            Ok(inner) => Ok(Some(inner?)),
        },
    }
}

fn retryable_status(code: u16) -> bool {
    matches!(code, 429 | 500 | 502 | 503)
}

/// Sends a request with bounded retries for connect-phase failures and
/// transient server statuses. Only ever runs before any response body has
/// been consumed, so retrying cannot duplicate side effects. Esc aborts even
/// during the backoff sleep. Returns Ok(None) when aborted.
async fn guarded_send(
    abort: &AtomicBool,
    cfg: &RequestCfg,
    on_note: &mut dyn FnMut(&str),
    mut make_req: impl FnMut() -> reqwest::RequestBuilder,
) -> Result<Option<reqwest::Response>> {
    let mut attempt = 0usize;
    loop {
        match send_with_guard(abort, cfg.first_byte, make_req().send()).await {
            Ok(Some(resp)) => {
                let status = resp.status();
                if !retryable_status(status.as_u16()) || attempt >= cfg.retries {
                    return Ok(Some(resp));
                }
                // honor Retry-After when the server bothers to send one
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s.min(120)))
                    .unwrap_or_else(|| cfg.backoff(attempt));
                let detail = truncate(&read_err_text(resp).await, 160);
                attempt += 1;
                on_note(&format!(
                    "↻ endpoint busy (HTTP {} · retry {attempt}/{}) — waiting {}s · {detail}",
                    status.as_u16(),
                    cfg.retries,
                    wait.as_secs()
                ));
                if sleep_or_abort(abort, wait).await {
                    return Ok(None);
                }
            }
            Ok(None) => return Ok(None),
            Err(e) if attempt < cfg.retries => {
                let wait = cfg.backoff(attempt);
                attempt += 1;
                on_note(&format!(
                    "↻ connection failed (retry {attempt}/{}) — waiting {}s · {}",
                    cfg.retries,
                    wait.as_secs(),
                    truncate(&e.to_string(), 140)
                ));
                if sleep_or_abort(abort, wait).await {
                    return Ok(None);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

async fn chat_openai(
    p: &Provider,
    model: &str,
    msgs: &[ChatMsg],
    tools: &[ToolSpec],
    cfg: &RequestCfg,
    ctx: &mut StreamCtx<'_>,
) -> Result<AssistantMsg> {
    let url = format!("{}/chat/completions", base(p));
    let window = p.context_window.unwrap_or(32_768);
    let budget = completion_tokens(window, rough_prompt_tokens(msgs, tools), cfg.max_tokens);
    let mut body = json!({
        "model": model,
        "messages": openai_messages(msgs),
        "tools": openai_tools(tools),
        "stream": true,
        "max_tokens": budget,
    });
    // Thinking models (Qwen3.x & co.) accept a reasoning-effort level; some
    // chat templates 400 on unknown values, so only validated ones arrive here.
    if let Some(e) = &cfg.reasoning_effort {
        body["reasoning_effort"] = json!(e);
    }
    // llama.cpp/Ollama extension: hard cap on thinking so a reasoning model
    // can never spend the whole completion budget before answering.
    if let Some(n) = cfg.max_reasoning_tokens {
        body["max_reasoning_tokens"] = json!(n);
    }
    let client = http(cfg.connect);
    let mk = || {
        let mut req = client.post(&url).json(&body);
        if !p.resolved_key().is_empty() {
            req = req.bearer_auth(p.resolved_key());
        }
        req
    };
    let Some(resp) = guarded_send(ctx.abort, cfg, ctx.on_note, mk).await? else {
        return Ok(AssistantMsg::default());
    };
    if !resp.status().is_success() {
        let code = resp.status();
        let text = read_err_text(resp).await;
        return Err(anyhow!("HTTP {code}: {}", truncate(&text, 600)));
    }
    let mut acc = AssistantMsg::default();
    let mut tc_ids: Vec<String> = Vec::new();
    let mut tc_names: Vec<String> = Vec::new();
    let mut tc_args: Vec<String> = Vec::new();

    let mut stream = resp.bytes_stream();
    let mut lines = SseLines::default();
    'stream: loop {
        let next = tokio::select! {
            biased;
            _ = wait_abort(ctx.abort) => break 'stream,
            r = tokio::time::timeout(cfg.idle, stream.next()) => r,
        };
        match next {
            Err(_) => {
                return Err(anyhow!(
                    "stream stalled — no data from provider for {}s (tune agent.idle_timeout_secs)",
                    cfg.idle.as_secs()
                ))
            }
            Ok(None) => break 'stream,
            Ok(Some(Err(e))) => return Err(e).context("reading stream"),
            Ok(Some(Ok(bytes))) => {
                for line in lines.push(&bytes) {
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data == "[DONE]" {
                        break 'stream;
                    }
                    let v: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(err) = v.get("error") {
                        return Err(anyhow!("provider error: {}", err));
                    }
                    let choice = &v["choices"][0];
                    if choice["finish_reason"].as_str() == Some("length") {
                        acc.truncated = true;
                    }
                    let delta = choice.get("delta");
                    // Thinking models (Qwen3 & co.) stream their chain of
                    // thought in reasoning_content while content stays empty.
                    // Surface it live so the UI never looks frozen, but keep
                    // it out of the saved answer.
                    if let Some(r) = delta.and_then(|d| d.get("reasoning_content")).and_then(|c| c.as_str()) {
                        if !r.is_empty() {
                            acc.reasoning_chars += r.chars().count();
                            (ctx.on_reasoning)(r);
                        }
                    }
                    if let Some(t) = delta.and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
                        if !t.is_empty() {
                            acc.text.push_str(t);
                            (ctx.on_text)(t);
                        }
                    }
                    if let Some(calls) = delta.and_then(|d| d.get("tool_calls")).and_then(|c| c.as_array()) {
                        for c in calls {
                            let idx = c["index"].as_u64().unwrap_or(0) as usize;
                            while tc_ids.len() <= idx {
                                tc_ids.push(String::new());
                                tc_names.push(String::new());
                                tc_args.push(String::new());
                            }
                            if let Some(id) = c["id"].as_str() {
                                tc_ids[idx] = id.to_string();
                            }
                            if let Some(n) = c["function"]["name"].as_str() {
                                tc_names[idx] = n.to_string();
                            }
                            if let Some(a) = c["function"]["arguments"].as_str() {
                                tc_args[idx].push_str(a);
                            }
                        }
                    }
                }
            }
        }
    }
    for i in 0..tc_ids.len() {
        if tc_names[i].is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&tc_args[i]) {
            Ok(args) => acc.tool_calls.push(ToolCall {
                id: if tc_ids[i].is_empty() { format!("call_{i}") } else { tc_ids[i].clone() },
                name: tc_names[i].clone(),
                args,
            }),
            // Never execute a tool with guessed/empty arguments: unparseable
            // streamed JSON means the model never finished the call.
            Err(_) => acc.dropped_tools.push(tc_names[i].clone()),
        }
    }
    Ok(acc)
}

async fn chat_anthropic(
    p: &Provider,
    model: &str,
    msgs: &[ChatMsg],
    tools: &[ToolSpec],
    cfg: &RequestCfg,
    ctx: &mut StreamCtx<'_>,
) -> Result<AssistantMsg> {
    let url = format!("{}/messages", base(p));
    let (system, messages) = anthropic_parts(msgs);
    let tool_defs: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}))
        .collect();
    let window = p.context_window.unwrap_or(32_768);
    let budget = completion_tokens(window, rough_prompt_tokens(msgs, tools), cfg.max_tokens);
    let body = json!({
        "model": model,
        "max_tokens": budget,
        "system": system,
        "messages": messages,
        "tools": tool_defs,
        "stream": true,
    });
    let client = http(cfg.connect);
    let mk = || {
        client
            .post(&url)
            .header("x-api-key", p.resolved_key())
            .header("anthropic-version", "2023-06-01")
            .json(&body)
    };
    let Some(resp) = guarded_send(ctx.abort, cfg, ctx.on_note, mk).await? else {
        return Ok(AssistantMsg::default());
    };
    if !resp.status().is_success() {
        let code = resp.status();
        let text = read_err_text(resp).await;
        return Err(anyhow!("HTTP {code}: {}", truncate(&text, 600)));
    }

    #[derive(Deserialize)]
    struct Ev {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        index: u64,
        #[serde(default)]
        delta: Value,
        #[serde(default)]
        content_block: Value,
    }

    let mut acc = AssistantMsg::default();
    // block index -> (id,name) for tool_use blocks
    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut block_json: Vec<String> = Vec::new();

    let mut stream = resp.bytes_stream();
    let mut lines = SseLines::default();
    'stream: loop {
        let next = tokio::select! {
            biased;
            _ = wait_abort(ctx.abort) => break 'stream,
            r = tokio::time::timeout(cfg.idle, stream.next()) => r,
        };
        match next {
            Err(_) => {
                return Err(anyhow!(
                    "stream stalled — no data from provider for {}s (tune agent.idle_timeout_secs)",
                    cfg.idle.as_secs()
                ))
            }
            Ok(None) => break 'stream,
            Ok(Some(Err(e))) => return Err(e).context("reading stream"),
            Ok(Some(Ok(bytes))) => {
                for line in lines.push(&bytes) {
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data == "[DONE]" {
                        break 'stream;
                    }
                    let ev: Ev = match serde_json::from_str(data) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    match ev.kind.as_str() {
                        "message_delta" => {
                            if ev.delta["stop_reason"].as_str() == Some("max_tokens") {
                                acc.truncated = true;
                            }
                        }
                        "content_block_start" => {
                            let idx = ev.content_block["index"].as_u64().unwrap_or(blocks.len() as u64) as usize;
                            let btype = ev.content_block["type"].as_str().unwrap_or("");
                            let id = ev.content_block["id"].as_str().unwrap_or("").to_string();
                            let name = ev.content_block["name"].as_str().unwrap_or("").to_string();
                            while blocks.len() <= idx {
                                blocks.push((String::new(), String::new()));
                                block_json.push(String::new());
                            }
                            if btype == "tool_use" {
                                blocks[idx] = (id, name);
                            }
                        }
                        "content_block_delta" => {
                            let idx = ev.index as usize;
                            if let Some(t) = ev.delta["text"].as_str() {
                                acc.text.push_str(t);
                                (ctx.on_text)(t);
                            } else if let Some(t) = ev.delta["thinking"].as_str() {
                                if !t.is_empty() {
                                    acc.reasoning_chars += t.chars().count();
                                    (ctx.on_reasoning)(t);
                                }
                            } else if let Some(j) = ev.delta["partial_json"].as_str() {
                                if idx < block_json.len() {
                                    block_json[idx].push_str(j);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for (i, (id, name)) in blocks.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&block_json[i]) {
            Ok(args) => acc.tool_calls.push(ToolCall {
                id: if id.is_empty() { format!("tu_{i}") } else { id.clone() },
                name: name.clone(),
                args,
            }),
            Err(_) => acc.dropped_tools.push(name.clone()),
        }
    }
    Ok(acc)
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = n;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Serves canned HTTP replies, one per incoming connection.
    fn mock_endpoint(replies: Vec<(u16, &'static str)>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (code, body) in replies {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf); // drain request head (best effort)
                let reason = if code == 200 { "OK" } else { "Err" };
                let head = format!(
                    "HTTP/1.1 {code} {reason}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(body.as_bytes());
                let _ = sock.flush();
                drop(sock);
            }
        });
        port
    }

    fn test_provider(port: u16) -> Provider {
        Provider {
            name: "test".into(),
            kind: ProviderKind::Openai,
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: String::new(),
            models: vec![],
            default_model: None,
            context_window: None,
        }
    }

    fn fast_cfg(retries: usize) -> RequestCfg {
        RequestCfg {
            retries,
            backoffs: vec![Duration::from_millis(50)],
            first_byte: Duration::from_secs(15),
            idle: Duration::from_secs(15),
            ..Default::default()
        }
    }

    #[test]
    fn sse_lines_accumulates_across_chunks() {
        let mut l = SseLines::default();
        assert_eq!(l.push(b"data: he"), Vec::<String>::new());
        assert_eq!(l.push(b"llo\n\r\ndata: x\n"), vec!["data: hello", "data: x"]);
    }

    #[test]
    fn completion_budget_honours_ceiling_but_never_overflows_the_window() {
        // Roomy window + small prompt → ceiling, less the 512 safety margin.
        assert_eq!(completion_tokens(250_000, 1_000, 250_000), 248_488);
        // Ceiling larger than remaining room → clamped to the room.
        assert_eq!(completion_tokens(8_192, 6_000, 250_000), 8_192 - 6_000 - 512);
        // Prompt nearly fills the window → floor of 256, never 0/negative.
        assert_eq!(completion_tokens(4_096, 4_090, 250_000), 256);
        // A modest ceiling is respected even when room is huge.
        assert_eq!(completion_tokens(250_000, 100, 16_384), 16_384);
    }

    #[test]
    fn reasoning_streams_separate_from_answer_and_truncated_tools_are_dropped() {
        // Qwen3-style: thinking arrives in reasoning_content; the tool call's
        // JSON args are cut off by finish_reason=length. The partial call
        // must NOT be executed with guessed arguments.
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"reasoning_content\":\"think \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"run_command\",\"arguments\":\"{\\\"comm\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let port = mock_endpoint(vec![(200, body)]);
        let p = test_provider(port);
        let abort = AtomicBool::new(false);
        let mut reasoning = String::new();
        let msgs = vec![ChatMsg::user("hi")];
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |t| reasoning.push_str(t),
                on_note: &mut |_| {},
                abort: &abort,
            };
            chat(&p, "m", &msgs, &[], &fast_cfg(0), &mut ctx).unwrap()
        };
        assert_eq!(reasoning, "think ");
        assert!(out.text.is_empty(), "reasoning must not leak into the answer");
        assert!(out.tool_calls.is_empty(), "truncated tool call must be dropped");
        assert_eq!(out.dropped_tools, vec!["run_command".to_string()]);
        assert!(out.truncated);
    }

    #[test]
    fn thinking_starvation_is_flagged() {
        // Qwen3-style failure mode: the whole budget goes to reasoning, the
        // stream ends at the length limit with no answer and no tool calls.
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"reasoning_content\":\"hmm \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"more thinking\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let port = mock_endpoint(vec![(200, body)]);
        let p = test_provider(port);
        let abort = AtomicBool::new(false);
        let msgs = vec![ChatMsg::user("hi")];
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |_| {},
                on_note: &mut |_| {},
                abort: &abort,
            };
            chat(&p, "m", &msgs, &[], &fast_cfg(0), &mut ctx).unwrap()
        };
        assert!(out.text.is_empty());
        assert!(out.tool_calls.is_empty());
        assert!(out.truncated);
        assert_eq!(out.reasoning_chars, "hmm more thinking".chars().count());
    }

    #[test]
    fn transient_503_is_retried_then_succeeds() {
        let ok = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let port = mock_endpoint(vec![(503, "model loading"), (200, ok)]);
        let p = test_provider(port);
        let abort = AtomicBool::new(false);
        let mut notes = Vec::new();
        let msgs = vec![ChatMsg::user("hi")];
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |_| {},
                on_note: &mut |n| notes.push(n.to_string()),
                abort: &abort,
            };
            chat(&p, "m", &msgs, &[], &fast_cfg(1), &mut ctx).unwrap()
        };
        assert_eq!(out.text, "hi");
        assert_eq!(notes.len(), 1, "one retry notice expected: {notes:?}");
        assert!(notes[0].contains("retry"), "notice should mention the retry: {notes:?}");
    }

    #[test]
    fn esc_abort_interrupts_a_hanging_request() {
        // A server that accepts the connection but never answers: without an
        // abort-aware select this would hang for first_byte (600s by default).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let _c = listener.accept();
            std::thread::sleep(Duration::from_secs(30));
        });
        let p = test_provider(port);
        let abort = std::sync::Arc::new(AtomicBool::new(false));
        {
            let a = abort.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                a.store(true, Ordering::Relaxed);
            });
        }
        let msgs = vec![ChatMsg::user("hi")];
        let t = std::time::Instant::now();
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |_| {},
                on_note: &mut |_| {},
                abort: &abort,
            };
            chat(&p, "m", &msgs, &[], &RequestCfg::default(), &mut ctx)
        };
        assert!(out.is_ok(), "aborted request should return cleanly: {out:?}");
        assert!(out.unwrap().text.is_empty());
        assert!(t.elapsed() < Duration::from_secs(10), "abort took {:?}", t.elapsed());
    }

    #[test]
    fn dead_endpoint_fails_fast() {
        // Nothing listening → connection refused immediately, not a hang.
        let p = test_provider(1);
        let abort = AtomicBool::new(false);
        let msgs = vec![ChatMsg::user("hi")];
        let t = std::time::Instant::now();
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |_| {},
                on_note: &mut |_| {},
                abort: &abort,
            };
            chat(&p, "m", &msgs, &[], &fast_cfg(0), &mut ctx)
        };
        assert!(out.is_err());
        assert!(t.elapsed() < Duration::from_secs(5), "dead endpoint should fail fast");
    }

    /// Opt-in smoke test against a real (slow) endpoint — validates that a
    /// thinking model streams reasoning separately, honors max_tokens + the
    /// context-window clamp, accepts reasoning_effort, and terminates instead
    /// of hanging. Run with:
    ///   PC_LIVE_URL=http://host:port/v1 PC_LIVE_KEY=... PC_LIVE_MODEL=... \
    ///   [PC_LIVE_CTX=250000] cargo test live_endpoint -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_endpoint_thinks_without_hanging() {
        let (Ok(url), Ok(model)) = (std::env::var("PC_LIVE_URL"), std::env::var("PC_LIVE_MODEL")) else {
            panic!("set PC_LIVE_URL and PC_LIVE_MODEL");
        };
        let p = Provider {
            name: "live".into(),
            kind: ProviderKind::Openai,
            base_url: url,
            api_key: std::env::var("PC_LIVE_KEY").unwrap_or_default(),
            models: vec![],
            default_model: Some(model.clone()),
            context_window: std::env::var("PC_LIVE_CTX").ok().and_then(|v| v.parse().ok()),
        };
        let abort = AtomicBool::new(false);
        let mut reasoning = 0usize;
        let t = std::time::Instant::now();
        // xhigh is this model's default; low should still be accepted (a bad
        // value would make the chat template reject the request with a 400).
        let rcfg = RequestCfg { reasoning_effort: Some("low".into()), max_tokens: 250_000, ..Default::default() };
        let out = {
            let mut ctx = StreamCtx {
                on_text: &mut |_| {},
                on_reasoning: &mut |r| reasoning += r.chars().count(),
                on_note: &mut |n| println!("note: {n}"),
                abort: &abort,
            };
            chat(&p, &model, &[ChatMsg::user("Reply with the single word: pong")], &[], &rcfg, &mut ctx)
                .expect("live request failed (reasoning_effort or max_tokens rejected?)")
        };
        println!(
            "done in {:.1}s · answer={} chars · reasoning={reasoning} · truncated={}",
            t.elapsed().as_secs_f64(),
            out.text.chars().count(),
            out.truncated,
        );
        assert!(!out.text.is_empty() || reasoning > 0, "endpoint produced nothing at all");
    }
}
