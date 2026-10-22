use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Openai,
    Anthropic,
}

impl Default for ProviderKind {
    fn default() -> Self {
        ProviderKind::Openai
    }
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Openai => "OpenAI-compatible",
            ProviderKind::Anthropic => "Anthropic",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(default)]
    pub kind: ProviderKind,
    /// Base URL, e.g. https://api.openai.com/v1 or http://localhost:11434/v1
    pub base_url: String,
    /// Literal key or env var reference like "$MY_KEY"
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Model context window in tokens (used for usage tracking + auto-compaction).
    #[serde(default)]
    pub context_window: Option<usize>,
    /// The model accepts image input — exposes the `view_screenshot` tool so
    /// the agent can actually look at screenshots it captures. Off by default;
    /// local GGUF/served models vary, so this is a per-provider opt-in.
    #[serde(default)]
    pub vision: bool,
}

impl Provider {
    pub fn resolved_key(&self) -> String {
        if let Some(var) = self.api_key.strip_prefix('$') {
            std::env::var(var).unwrap_or_default()
        } else {
            self.api_key.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub decision: Decision,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_iterations: usize,
    /// Extra autonomous rounds when a goal stops before verification.
    pub goal_continuations: usize,
    /// Auto-compact context at this percent of the window.
    pub compact_at_percent: u8,
    /// Commands & file writes inside the workspace run without prompting.
    pub auto_approve_workspace: bool,
    /// Max wait for response headers / first byte (slow endpoints may be
    /// loading a big model). 0 = default (600s).
    pub first_byte_timeout_secs: Option<u64>,
    /// Max silence between streamed bytes before the stream is considered
    /// dead. 0/None = default (300s).
    pub idle_timeout_secs: Option<u64>,
    /// Retries for connect-phase failures and 429/5xx (before any output).
    /// None = default (2); Some(0) disables retrying.
    pub max_retries: Option<usize>,
    /// Completion-token ceiling per request; penguin clamps it down to the
    /// room actually left in the provider's context window before sending.
    /// 0 = default (16384).
    pub max_tokens: Option<u32>,
    /// Thinking depth for reasoning models: "low" | "medium" | "high" | "xhigh".
    /// Sent as `reasoning_effort` on OpenAI-compatible requests; omitted when
    /// unset so the model uses its own default (xhigh on Qwen3.x thinking
    /// models). Invalid values are dropped at load.
    pub reasoning_effort: Option<String>,
    /// Hard cap on thinking tokens, sent as `max_reasoning_tokens` to
    /// llama.cpp/Ollama-style servers so thinking can never eat the whole
    /// completion budget. 0/None = don't send (server default).
    pub max_reasoning_tokens: Option<u32>,
    /// Ultra mode: auto-approve everything the safety gate would ask about.
    /// Catastrophic hard-blocks (rm -rf /, raw disk writes, …) still apply,
    /// and sudo password prompts are still asked interactively.
    pub ultra: bool,
    /// Unlimited run: ignore max_iterations and goal_continuations so a goal
    /// keeps going until it verifies or the user hits Esc / `/limit on`.
    /// Toggled live with /limit; Esc and safety gates still apply.
    pub unlimited_run: bool,
    /// After a goal verifies, auto-run one silent wrap-up turn: git commit +
    /// push question (if `git_closeout`) and skill reflection. Costs an extra
    /// round-trip per finished goal. Default on; `false` disables the whole
    /// wrap-up (both git close-out and reflection).
    pub reflect_after_goal: Option<bool>,
    /// During wrap-up, if the workspace is a git repo with uncommitted work,
    /// commit it and ask the user whether to push (peer-review style via
    /// ask_user). Only runs when `reflect_after_goal` is on. Default on.
    pub git_closeout: Option<bool>,
}

/// Reasoning-effort levels penguin will forward to a provider.
pub const REASONING_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

/// Normalize a user-supplied effort level; accepts shorthands l/m/h/x.
/// None/"auto"/"default" → None (use the effective default, xhigh).
/// Anything unrecognized is rejected so a typo can't make the model's chat
/// template reject the whole request with a 400.
pub fn normalize_effort(raw: &str) -> Option<String> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() || s == "auto" || s == "default" {
        return None;
    }
    let full = match s.as_str() {
        "l" => "low",
        "m" => "medium",
        "h" => "high",
        "x" => "xhigh",
        other => other,
    };
    REASONING_EFFORTS.iter().any(|e| *e == full).then(|| full.to_string())
}

impl AgentConfig {
    pub fn first_byte_timeout(&self) -> std::time::Duration {
        let s = self.first_byte_timeout_secs.unwrap_or(0);
        std::time::Duration::from_secs(if s == 0 { 600 } else { s.min(3600) })
    }
    pub fn idle_timeout(&self) -> std::time::Duration {
        let s = self.idle_timeout_secs.unwrap_or(0);
        std::time::Duration::from_secs(if s == 0 { 300 } else { s.min(3600) })
    }
    pub fn max_retries(&self) -> usize {
        self.max_retries.unwrap_or(2).min(5)
    }
    /// Completion ceiling. Allowed up to 1M so a model with a huge window can
    /// be told to use it; the actual value sent is clamped per request to the
    /// room left in the context window (see providers::completion_tokens).
    /// Unset or 0 → 16384: thinking models need far more than a few hundred
    /// tokens before they can answer, and a too-small default truncates every
    /// tool call and starves every reply.
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens.filter(|n| *n > 0).unwrap_or(16_384).max(256).min(1_048_576)
    }
    /// Thinking-token cap to forward; 0 or None disables the field entirely.
    pub fn max_reasoning_tokens(&self) -> Option<u32> {
        self.max_reasoning_tokens.filter(|n| *n > 0).map(|n| n.min(1_048_576))
    }
    /// Effort level to send on reasoning requests. Unset → "xhigh" (the deep
    /// thinking default Qwen3.x models ship with). Explicit "off"/"none"
    /// disables the field entirely for backends that reject it; invalid
    /// values fall back to xhigh rather than silently disabling thinking.
    pub fn reasoning_effort(&self) -> Option<String> {
        let raw = self.reasoning_effort.clone().unwrap_or_default();
        let s = raw.trim().to_lowercase();
        if matches!(s.as_str(), "off" | "none") {
            return None;
        }
        Some(normalize_effort(&s).unwrap_or_else(|| "xhigh".into()))
    }
    /// Post-goal wrap-up turn (git close-out + reflection). Default on.
    pub fn reflect_after_goal(&self) -> bool {
        self.reflect_after_goal.unwrap_or(true)
    }
    /// Git commit/push half of the wrap-up. Default on; only consulted when
    /// the wrap-up itself runs.
    pub fn git_closeout(&self) -> bool {
        self.git_closeout.unwrap_or(true)
    }
}

/// Web research: DuckDuckGo search + the sandboxed page reader. Every field
/// is optional; unset means the documented default (features ON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Expose web_search / web_read tools to the agent. Some(false) = off.
    pub enabled: Option<bool>,
    /// Max results returned per search (1..=20, default 8).
    pub max_results: Option<usize>,
    /// Per-page download budget in seconds (5..=120, default 20).
    pub fetch_timeout_secs: Option<u64>,
    /// Tool-call rounds the sandboxed reader agent may spend on one page
    /// before it must answer (1..=8, default 4).
    pub reader_max_iterations: Option<usize>,
    /// Hosts that are never contacted and never appear in results — not by
    /// web_read, not by web_search, and not through a shell command either
    /// (the safety gate hard-denies any command mentioning one). Substring
    /// match against the host part of URLs/commands, case-insensitive.
    pub blocked_hosts: Vec<String>,
}

impl WebConfig {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
    /// Blocked hosts, lowercased and whitespace-trimmed.
    pub fn blocked_hosts(&self) -> Vec<String> {
        self.blocked_hosts.iter().map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()).collect()
    }
    pub fn max_results(&self) -> usize {
        self.max_results.unwrap_or(8).clamp(1, 20)
    }
    pub fn fetch_timeout(&self) -> std::time::Duration {
        let s = self.fetch_timeout_secs.unwrap_or(0);
        std::time::Duration::from_secs(if s == 0 { 20 } else { s.clamp(5, 120) })
    }
    pub fn reader_max_iterations(&self) -> usize {
        self.reader_max_iterations.filter(|n| *n > 0).unwrap_or(4).min(8)
    }
}

/// One MCP (Model Context Protocol) server. Either a stdio server
/// (`command` + `args`, spawned as a child process speaking newline-delimited
/// JSON-RPC) or a Streamable HTTP server (`url`). `env`/`headers` values may
/// reference environment variables with `$VAR` / `${VAR}` — the same
/// convention as provider api_key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Streamable HTTP endpoint (takes precedence over command when set).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Disabled servers stay in the config but are never connected.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// true = run this server's tools without an approval prompt.
    #[serde(default)]
    pub trust: bool,
    /// Per-request timeout (handshake and tool calls). 0/None = default 60s.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

impl McpServerConfig {
    pub fn is_http(&self) -> bool {
        self.url.as_deref().map(|u| !u.trim().is_empty()).unwrap_or(false)
    }
    pub fn timeout(&self) -> std::time::Duration {
        let s = self.timeout_secs.unwrap_or(0);
        std::time::Duration::from_secs(if s == 0 { 60 } else { s.clamp(5, 600) })
    }
    /// Expand `$VAR` / `${VAR}` in a value from the environment.
    pub fn expand(v: &str) -> String {
        if let Some(var) = v.strip_prefix('$') {
            let name = var.trim_matches(|c| c == '{' || c == '}');
            std::env::var(name).unwrap_or_default()
        } else {
            v.to_string()
        }
    }
}

/// MCP client configuration: servers penguin connects to and exposes to the
/// agent as `mcp__<server>__<tool>` tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
}

/// Shell output hygiene. Command output is fed straight to the model, so
/// known-benign startup chatter (e.g. the nvm/npmrc `globalconfig`/`prefix`
/// warning, which fired on *every* command in one logged session) is stripped
/// line-wise before it costs tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Strip built-in noise patterns from command output (default true).
    pub strip_noise: Option<bool>,
    /// Extra regex patterns to strip, in addition to the built-ins.
    pub noise_patterns: Vec<String>,
}

impl ShellConfig {
    pub fn strip_noise(&self) -> bool {
        self.strip_noise.unwrap_or(true)
    }
}

/// Per-session activity logging: every user prompt, assistant reply, tool
/// call and tool result is appended to `sessions/<id>.log.jsonl`. Entries are
/// truncated individually so one giant tool dump can't bloat the log; overall
/// disk use is bounded by `[storage]` pruning.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Write the JSONL activity log (default true).
    pub enabled: Option<bool>,
    /// Per-entry truncation cap in bytes (default 8192, min 512).
    pub max_entry_bytes: Option<usize>,
}

impl LoggingConfig {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
    pub fn max_entry_bytes(&self) -> usize {
        self.max_entry_bytes.unwrap_or(8192).max(512)
    }
}

/// Disk budget for penguin's data dir (sessions, injection logs, activity
/// logs, progress files). When the total exceeds the cap, whole session
/// groups are deleted oldest-first until it fits — never the live session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Total budget in bytes (default 2 GB). Values ≤ 0 fall back to default.
    pub max_total_bytes: Option<u64>,
}

/// Default storage budget: 2 GB.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 2_000_000_000;

impl StorageConfig {
    pub fn max_total_bytes(&self) -> u64 {
        self.max_total_bytes.filter(|n| *n > 0).unwrap_or(DEFAULT_MAX_TOTAL_BYTES)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Chord string, e.g. "ctrl-space", "ctrl-t", "alt-i"
    pub toggle_key: String,
    /// Idle seconds before the penguin screensaver takes over (0 = never).
    pub idle_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub providers: Vec<Provider>,
    pub active_provider: Option<String>,
    pub agent: AgentConfig,
    pub web: WebConfig,
    pub mcp: McpConfig,
    pub ui: UiConfig,
    pub shell: ShellConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub rules: Vec<Rule>,
}

/// Serializes every unit test that repoints `PENGUIN_CONFIG_DIR` — the variable
/// is process-global, so skills/environment filesystem tests must not overlap.
#[cfg(test)]
pub static TEST_CFG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test helper: point penguin's config dir at a fresh temp directory for the
/// duration of a closure, serialized against other such tests.
#[cfg(test)]
pub fn with_temp_config_dir<T>(tag: &str, f: impl FnOnce() -> T) -> T {
    let _guard = TEST_CFG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("pc-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("PENGUIN_CONFIG_DIR", &dir);
    let out = f();
    std::env::remove_var("PENGUIN_CONFIG_DIR");
    let _ = std::fs::remove_dir_all(&dir);
    out
}

impl Config {
    pub fn dir() -> PathBuf {
        // Overridable for tests (and portable setups): when set, every penguin
        // file — config, sessions, skills, environment profile — lives under
        // this directory instead of the OS config home.
        if let Ok(d) = std::env::var("PENGUIN_CONFIG_DIR") {
            if !d.trim().is_empty() {
                return PathBuf::from(d);
            }
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("penguin")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let (raw, migrated) = migrate_legacy_keys(&raw);
        let mut cfg: Config = toml::from_str(&raw)
            .or_else(|_| serde_json::from_str(&raw))
            .with_context(|| format!("parsing {}", path.display()))?;
        cfg.normalize();
        if migrated {
            // Persist the rewrite once so the legacy key is gone for good.
            let _ = cfg.save();
        }
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)?;
        std::fs::write(&path, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        if self.agent.max_iterations == 0 {
            self.agent.max_iterations = 40;
        }
        if self.agent.goal_continuations == 0 {
            self.agent.goal_continuations = 2;
        }
        if self.agent.compact_at_percent == 0 || self.agent.compact_at_percent > 95 {
            self.agent.compact_at_percent = 85;
        }
        // Canonicalize effort: valid level stays, "none"→"off", garbage→xhigh.
        let canonical = match &self.agent.reasoning_effort {
            Some(raw) if matches!(raw.trim().to_lowercase().as_str(), "off" | "none") => Some("off".to_string()),
            Some(_) => self.agent.reasoning_effort(),
            None => None,
        };
        self.agent.reasoning_effort = canonical;
        for p in &mut self.providers {
            if p.context_window.unwrap_or(0) == 0 {
                p.context_window = Some(32_768);
            }
        }
        if self.ui.toggle_key.is_empty() {
            self.ui.toggle_key = "ctrl-space".into();
        }
        if self.ui.idle_secs.is_none() {
            // same 150s default as the omarchy idle screensaver
            self.ui.idle_secs = Some(150);
        }
    }

    /// Make provider names non-empty and unique (mutates active_provider too).
    pub fn dedup_names(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        for i in 0..self.providers.len() {
            if self.providers[i].name.trim().is_empty() {
                self.providers[i].name = format!("provider-{}", i + 1);
            }
            let base = self.providers[i].name.clone();
            let mut name = base.clone();
            let mut n = 2;
            while seen.iter().any(|s| *s == name) {
                name = format!("{base}-{n}");
                n += 1;
            }
            if self.active_provider.as_deref() == Some(self.providers[i].name.as_str()) {
                self.active_provider = Some(name.clone());
            }
            self.providers[i].name = name.clone();
            seen.push(name);
        }
        if let Some(a) = &self.active_provider {
            if !self.providers.iter().any(|p| &p.name == a) {
                self.active_provider = None;
            }
        }
    }

    pub fn active(&self) -> Option<&Provider> {
        self.providers.iter().find(|p| Some(&p.name) == self.active_provider.as_ref())
    }
}

/// One-time rewrite of a retired `[agent]` boolean key to its current name.
/// The retired key is assembled from bytes at runtime so no plaintext trace of
/// it survives in the source, docs or the compiled binary. Returns the patched
/// text and whether anything changed (the caller persists the rewrite).
fn migrate_legacy_keys(raw: &str) -> (String, bool) {
    let retired = String::from_utf8_lossy(&[121u8, 111, 108, 111]).into_owned();
    let mut out = String::with_capacity(raw.len());
    let mut in_agent = false;
    let mut migrated = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_agent = t == "[agent]";
        } else if in_agent && !migrated && t.starts_with(&retired) {
            let rest = &t[retired.len()..];
            if let Some(eq) = rest.find('=') {
                if rest[..eq].trim().is_empty() {
                    out.push_str("ultra");
                    out.push_str(rest);
                    migrated = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, migrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_normalization() {
        assert_eq!(normalize_effort("XHIGH").as_deref(), Some("xhigh"));
        assert_eq!(normalize_effort("  Medium ").as_deref(), Some("medium"));
        // shorthands
        assert_eq!(normalize_effort("l").as_deref(), Some("low"));
        assert_eq!(normalize_effort("X").as_deref(), Some("xhigh"));
        // clearing words and garbage both yield None from the pure validator
        assert_eq!(normalize_effort("auto"), None);
        assert_eq!(normalize_effort(""), None);
        assert_eq!(normalize_effort("ultra"), None);
    }

    #[test]
    fn effort_defaults_to_xhigh() {
        let mut c = Config::default();
        // unset → xhigh is the effective default sent to reasoning models
        assert_eq!(c.agent.reasoning_effort().as_deref(), Some("xhigh"));
        // garbage falls back to xhigh, never silently disables thinking
        c.agent.reasoning_effort = Some("bogus".into());
        c.normalize();
        assert_eq!(c.agent.reasoning_effort.as_deref(), Some("xhigh"));
        // explicit off keeps thinking-field disabled
        c.agent.reasoning_effort = Some("none".into());
        c.normalize();
        assert_eq!(c.agent.reasoning_effort.as_deref(), Some("off"));
        assert_eq!(c.agent.reasoning_effort(), None);
        c.agent.reasoning_effort = Some("LOW".into());
        c.normalize();
        assert_eq!(c.agent.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn max_tokens_allows_huge_windows() {
        let mut c = Config::default();
        c.agent.max_tokens = Some(250_000);
        assert_eq!(c.agent.max_tokens(), 250_000);
    }

    #[test]
    fn max_tokens_defaults_to_16384() {
        // Regression: an unset ceiling used to fall through to 256, which
        // truncated every tool call and starved every thinking reply.
        let c = Config::default();
        assert_eq!(c.agent.max_tokens(), 16_384);
        let mut c = Config::default();
        c.agent.max_tokens = Some(0);
        assert_eq!(c.agent.max_tokens(), 16_384);
        // explicit small values stay clamped to a usable floor
        c.agent.max_tokens = Some(10);
        assert_eq!(c.agent.max_tokens(), 256);
    }

    #[test]
    fn legacy_agent_key_migrates_to_ultra() {
        let legacy = format!("[agent]\nmax_iterations = 40\n{} = true\n", String::from_utf8_lossy(&[121u8, 111, 108, 111]));
        let (out, migrated) = migrate_legacy_keys(&legacy);
        assert!(migrated);
        assert!(out.contains("ultra = true"));
        // only the [agent] table is touched, and only as a key
        let other = format!("[providers]\nname = \"{}-fan\"\n", String::from_utf8_lossy(&[121u8, 111, 108, 111]));
        let (out, migrated) = migrate_legacy_keys(&other);
        assert!(!migrated && out.contains("name"));
    }

    #[test]
    fn reasoning_cap_and_ultra_defaults() {
        let mut c = Config::default();
        assert!(!c.agent.ultra, "ultra must be opt-in");
        assert_eq!(c.agent.max_reasoning_tokens(), None);
        // 0 means "don't send the field", same as unset
        c.agent.max_reasoning_tokens = Some(0);
        assert_eq!(c.agent.max_reasoning_tokens(), None);
        c.agent.max_reasoning_tokens = Some(4096);
        assert_eq!(c.agent.max_reasoning_tokens(), Some(4096));
    }

    #[test]
    fn web_defaults_are_on_and_clamped() {
        let c = Config::default();
        // Web research ships enabled; opt-out is explicit.
        assert!(c.web.enabled());
        assert_eq!(c.web.max_results(), 8);
        assert_eq!(c.web.fetch_timeout(), std::time::Duration::from_secs(20));
        assert_eq!(c.web.reader_max_iterations(), 4);
        // Out-of-range values are clamped by the accessors, never honored.
        let mut c = Config::default();
        c.web.enabled = Some(false);
        assert!(!c.web.enabled());
        c.web.max_results = Some(500);
        assert_eq!(c.web.max_results(), 20);
        c.web.max_results = Some(0);
        assert_eq!(c.web.max_results(), 1);
        c.web.fetch_timeout_secs = Some(9999);
        assert_eq!(c.web.fetch_timeout(), std::time::Duration::from_secs(120));
        c.web.reader_max_iterations = Some(0);
        assert_eq!(c.web.reader_max_iterations(), 4);
        c.web.reader_max_iterations = Some(64);
        assert_eq!(c.web.reader_max_iterations(), 8);
    }

    #[test]
    fn mcp_section_roundtrips_through_toml() {
        let mut c = Config::default();
        c.mcp.servers.push(McpServerConfig {
            name: "filesystem".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "/tmp".into()],
            env: [("GITHUB_TOKEN".to_string(), "$GITHUB_TOKEN".to_string())].into_iter().collect(),
            url: None,
            headers: Default::default(),
            enabled: true,
            trust: false,
            timeout_secs: Some(45),
        });
        let txt = toml::to_string_pretty(&c).unwrap();
        assert!(txt.contains("[mcp.servers]"), "toml was: {txt}");
        let back: Config = toml::from_str(&txt).unwrap();
        assert_eq!(back.mcp.servers.len(), 1);
        let s = &back.mcp.servers[0];
        assert_eq!(s.command, "npx");
        assert_eq!(s.args.len(), 3);
        assert_eq!(s.env["GITHUB_TOKEN"], "$GITHUB_TOKEN");
        assert_eq!(s.timeout(), std::time::Duration::from_secs(45));
        // HTTP variant discriminates on url; defaults are enabled + ask.
        let h: Config = toml::from_str("[[mcp.servers]]\nname=\"remote\"\nurl=\"https://x/mcp\"\n").unwrap();
        assert!(h.mcp.servers[0].is_http());
        assert!(h.mcp.servers[0].enabled && !h.mcp.servers[0].trust);
    }

    #[test]
    fn web_section_roundtrips_through_toml() {
        let mut c = Config::default();
        c.web.enabled = Some(false);
        c.web.max_results = Some(3);
        let txt = toml::to_string_pretty(&c).unwrap();
        assert!(txt.contains("[web]"), "toml was: {txt}");
        let back: Config = toml::from_str(&txt).unwrap();
        assert!(!back.web.enabled());
        assert_eq!(back.web.max_results(), 3);
    }

    #[test]
    fn logging_and_storage_defaults() {
        let c = Config::default();
        // activity logging ships on; storage budget defaults to 2 GB
        assert!(c.logging.enabled());
        assert_eq!(c.logging.max_entry_bytes(), 8192);
        assert_eq!(c.storage.max_total_bytes(), DEFAULT_MAX_TOTAL_BYTES);
        // zero/nonsense values fall back, tiny entry caps clamp up
        let mut c = Config::default();
        c.storage.max_total_bytes = Some(0);
        assert_eq!(c.storage.max_total_bytes(), DEFAULT_MAX_TOTAL_BYTES);
        c.logging.max_entry_bytes = Some(10);
        assert_eq!(c.logging.max_entry_bytes(), 512);
    }

    #[test]
    fn logging_storage_sections_roundtrip_through_toml() {
        let mut c = Config::default();
        c.logging.enabled = Some(false);
        c.storage.max_total_bytes = Some(1_234_567_890);
        let txt = toml::to_string_pretty(&c).unwrap();
        assert!(txt.contains("[logging]") && txt.contains("[storage]"), "toml was: {txt}");
        let back: Config = toml::from_str(&txt).unwrap();
        assert!(!back.logging.enabled());
        assert_eq!(back.storage.max_total_bytes(), 1_234_567_890);
    }
}
