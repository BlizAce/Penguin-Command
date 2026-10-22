//! Per-session activity log: every user prompt, assistant reply, tool call
//! and tool result is appended as one JSON line to `sessions/<id>.log.jsonl`.
//! Best-effort — logging must never fail a turn. Entries are truncated
//! individually (`[logging] max_entry_bytes`) so one giant tool dump can't
//! bloat the file; total disk use is bounded by `[storage]` pruning, which
//! deletes whole old session groups together with these logs.

use std::io::Write;

/// Process-wide settings, wired once at startup from `[logging]`.
struct LogCfg {
    enabled: bool,
    max_entry_bytes: usize,
}

static CFG: std::sync::OnceLock<LogCfg> = std::sync::OnceLock::new();

pub fn configure(enabled: bool, max_entry_bytes: usize) {
    let _ = CFG.set(LogCfg { enabled, max_entry_bytes });
}

fn cfg() -> &'static LogCfg {
    CFG.get_or_init(|| LogCfg { enabled: true, max_entry_bytes: 8192 })
}

/// Append one activity event. `kind` is one of: user, goal, assistant,
/// tool_call, tool_result, info, error.
pub fn event(session_id: &str, kind: &str, text: &str) {
    #[cfg(test)]
    {
        let _ = (session_id, kind, text);
        return; // unit tests must not write into the user's config dir
    }
    #[allow(unreachable_code)]
    {
        if !cfg().enabled || session_id.is_empty() {
            return;
        }
        let cap = cfg().max_entry_bytes;
        let body = crate::agent::tools::middle_truncate(text, cap);
        let line = serde_json::json!({
            "ts": crate::session::fmt_ts(crate::session::now_ms()),
            "kind": kind,
            "text": body,
        });
        let path = crate::session::dir().join(format!("{session_id}.log.jsonl"));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}
