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
    /// YOLO mode: auto-approve everything the safety gate would ask about.
    /// Catastrophic hard-blocks (rm -rf /, raw disk writes, …) still apply,
    /// and sudo password prompts are still asked interactively.
    pub yolo: bool,
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
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens.unwrap_or(0).max(256).min(1_048_576)
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
    pub ui: UiConfig,
    pub rules: Vec<Rule>,
}

impl Config {
    pub fn dir() -> PathBuf {
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
        let mut cfg: Config = toml::from_str(&raw)
            .or_else(|_| serde_json::from_str(&raw))
            .with_context(|| format!("parsing {}", path.display()))?;
        cfg.normalize();
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
    fn reasoning_cap_and_yolo_defaults() {
        let mut c = Config::default();
        assert!(!c.agent.yolo, "yolo must be opt-in");
        assert_eq!(c.agent.max_reasoning_tokens(), None);
        // 0 means "don't send the field", same as unset
        c.agent.max_reasoning_tokens = Some(0);
        assert_eq!(c.agent.max_reasoning_tokens(), None);
        c.agent.max_reasoning_tokens = Some(4096);
        assert_eq!(c.agent.max_reasoning_tokens(), Some(4096));
    }
}
