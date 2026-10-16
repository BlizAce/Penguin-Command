//! One-shot import of MCP servers already configured for other coding agents:
//! Claude Desktop, Claude Code, opencode, Codex CLI, Gemini CLI and Cursor.
//! Every format boils down to name → {command,args,env} or {url,headers}; we
//! read them without importing any dependency on those tools.

use crate::config::McpServerConfig;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub source: &'static str,
    pub cfg: McpServerConfig,
}

/// Filesystem-safe kebab-case server name.
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn blank_cfg(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: sanitize_name(name),
        command: String::new(),
        args: Vec::new(),
        env: Default::default(),
        url: None,
        headers: Default::default(),
        enabled: true,
        trust: false,
        timeout_secs: None,
    }
}

/// Strip full-line // comments so jsonc files parse; anything stricter is the
/// file's problem (parse failure → candidate list skips it).
fn strip_line_comments(raw: &str) -> String {
    raw.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn load_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&raw).or_else(|_| serde_json::from_str::<Value>(&strip_line_comments(&raw))).ok()
}

/// Claude Desktop / Claude Code / Gemini / Cursor style:
/// `mcpServers: { name: {command,args,env} | {type:"http"|"sse",url,headers} }`
fn from_mcp_servers_map(v: &Value, source: &'static str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(map) = v.as_object() else { return out };
    for (name, entry) in map {
        let mut cfg = blank_cfg(name);
        if cfg.name.is_empty() {
            continue;
        }
        let url = entry["url"].as_str().filter(|u| !u.is_empty());
        if let Some(u) = url.or(entry["httpUrl"].as_str()) {
            cfg.url = Some(u.to_string());
            for (k, hv) in entry["headers"].as_object().map(|h| h.iter()).into_iter().flatten() {
                if let Some(s) = hv.as_str() {
                    cfg.headers.insert(k.clone(), s.to_string());
                }
            }
        } else if let Some(cmd) = entry["command"].as_str().filter(|c| !c.is_empty()) {
            cfg.command = cmd.to_string();
            cfg.args = entry["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            for (k, ev) in entry["env"].as_object().map(|h| h.iter()).into_iter().flatten() {
                if let Some(s) = ev.as_str() {
                    cfg.env.insert(k.clone(), s.to_string());
                }
            }
        } else {
            continue;
        }
        out.push(Candidate { source, cfg });
    }
    out
}

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// opencode: `mcp: { name: {type:"local", command:[…], environment, enabled}
/// | {type:"remote", url, headers, enabled} }`
fn from_opencode(v: &Value, source: &'static str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(map) = v.get("mcp").and_then(|m| m.as_object()) else { return out };
    for (name, entry) in map {
        let mut cfg = blank_cfg(name);
        if cfg.name.is_empty() {
            continue;
        }
        if let Some(false) = entry.get("enabled").and_then(|e| e.as_bool()) {
            continue;
        }
        if let Some(u) = entry["url"].as_str().filter(|u| !u.is_empty()) {
            cfg.url = Some(u.to_string());
            for (k, hv) in entry["headers"].as_object().map(|h| h.iter()).into_iter().flatten() {
                if let Some(s) = hv.as_str() {
                    cfg.headers.insert(k.clone(), s.to_string());
                }
            }
        } else if let Some(arr) = entry.get("command").and_then(|c| c.as_array()).filter(|a| !a.is_empty()) {
            // opencode stores the whole argv as an array; some configs use a string
            let strs: Vec<String> = arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
            if strs.is_empty() {
                continue;
            }
            cfg.command = strs[0].clone();
            cfg.args = strs[1..].to_vec();
            for (k, ev) in entry.get("environment").and_then(|e| e.as_object()).map(|h| h.iter()).into_iter().flatten() {
                if let Some(s) = ev.as_str() {
                    cfg.env.insert(k.clone(), s.to_string());
                }
            }
        } else if let Some(s) = entry.get("command").and_then(|c| c.as_str()) {
            let mut parts = s.split_whitespace();
            cfg.command = parts.next().unwrap_or("").to_string();
            cfg.args = parts.map(|p| p.to_string()).collect();
        } else {
            continue;
        }
        out.push(Candidate { source, cfg });
    }
    out
}

/// Codex CLI: `[mcp_servers.name] command = "…" args = […] env = {…}`
fn from_codex_toml(raw: &str, source: &'static str) -> Vec<Candidate> {
    #[derive(serde::Deserialize)]
    struct Server {
        #[serde(default)]
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
        #[serde(default)]
        url: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Doc {
        #[serde(default)]
        mcp_servers: std::collections::BTreeMap<String, Server>,
    }
    let Ok(doc) = toml::from_str::<Doc>(raw) else { return Vec::new() };
    doc.mcp_servers
        .into_iter()
        .filter_map(|(name, s)| {
            let mut cfg = blank_cfg(&name);
            if cfg.name.is_empty() {
                return None;
            }
            if let Some(u) = s.url.filter(|u| !u.trim().is_empty()) {
                cfg.url = Some(u);
            } else if !s.command.trim().is_empty() {
                cfg.command = s.command;
                cfg.args = s.args;
                cfg.env = s.env;
            } else {
                return None;
            }
            Some(Candidate { source, cfg })
        })
        .collect()
}

/// Scan every known agent config location. Candidates may collide with each
/// other or with existing penguin servers — the caller dedupes and presents.
pub fn discover(workspace: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();

    // Claude Desktop (Linux + macOS paths, both probed).
    if let Some(h) = home() {
        let candidates = [
            h.join(".config/Claude/claude_desktop_config.json"),
            h.join("Library/Application Support/Claude/claude_desktop_config.json"),
        ];
        for p in candidates {
            if let Some(v) = load_json(&p) {
                out.extend(from_mcp_servers_map(&v["mcpServers"], "Claude Desktop"));
            }
        }
        // Claude Code: ~/.claude.json (top-level + per-project).
        if let Some(v) = load_json(&h.join(".claude.json")) {
            out.extend(from_mcp_servers_map(&v["mcpServers"], "Claude Code"));
            for proj in v.get("projects").and_then(|p| p.as_object()).map(|m| m.values()).into_iter().flatten() {
                out.extend(from_mcp_servers_map(&proj["mcpServers"], "Claude Code"));
            }
        }
        // Codex CLI.
        if let Ok(raw) = std::fs::read_to_string(h.join(".codex/config.toml")) {
            out.extend(from_codex_toml(&raw, "Codex CLI"));
        }
        // Gemini CLI.
        if let Some(v) = load_json(&h.join(".gemini/settings.json")) {
            out.extend(from_mcp_servers_map(&v["mcpServers"], "Gemini CLI"));
        }
        // opencode global.
        for p in [h.join(".config/opencode/opencode.json"), h.join(".config/opencode/opencode.jsonc")] {
            if let Some(v) = load_json(&p) {
                out.extend(from_opencode(&v, "opencode"));
            }
        }
    }

    // Project-level: Cursor + opencode.
    for (rel, source) in [(".cursor/mcp.json", "Cursor"), ("opencode.json", "opencode"), ("opencode.jsonc", "opencode")] {
        let p = workspace.join(rel);
        if let Some(v) = load_json(&p) {
            if source == "Cursor" {
                out.extend(from_mcp_servers_map(&v["mcpServers"], source));
            } else {
                out.extend(from_opencode(&v, source));
            }
        }
    }

    // De-dupe by (name, transport target) keeping the first sighting.
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    out.retain(|c| {
        let key = (c.cfg.name.clone(), c.cfg.url.clone().unwrap_or_else(|| format!("{} {}", c.cfg.command, c.cfg.args.join(" "))));
        seen.insert(key)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_style_stdio_entry() {
        let v: Value = serde_json::from_str(r#"{"mcpServers":{"Filesystem":{"command":"npx","args":["-y","srv"],"env":{"K":"$V"}}}}"#).unwrap();
        let c = from_mcp_servers_map(&v["mcpServers"], "Claude Desktop");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].cfg.name, "filesystem");
        assert_eq!(c[0].cfg.command, "npx");
        assert_eq!(c[0].cfg.args, vec!["-y", "srv"]);
        assert_eq!(c[0].cfg.env["K"], "$V");
    }

    #[test]
    fn claude_style_http_entry() {
        let v: Value = serde_json::from_str(r#"{"mcpServers":{"remote":{"type":"http","url":"https://x/mcp","headers":{"Authorization":"Bearer t"}}}}"#).unwrap();
        let c = from_mcp_servers_map(&v["mcpServers"], "Claude Code");
        assert_eq!(c[0].cfg.url.as_deref(), Some("https://x/mcp"));
        assert_eq!(c[0].cfg.headers["Authorization"], "Bearer t");
    }

    #[test]
    fn opencode_local_and_remote() {
        let v: Value = serde_json::from_str(
            r#"{"mcp":{"git":{"type":"local","command":["uvx","mcp-git"],"environment":{"A":"b"},"enabled":true},
            "off":{"type":"local","command":["x"],"enabled":false},
            "web":{"type":"remote","url":"https://w/mcp"}}}"#,
        )
        .unwrap();
        let c = from_opencode(&v, "opencode");
        assert_eq!(c.len(), 2);
        let git = c.iter().find(|x| x.cfg.name == "git").unwrap();
        assert_eq!(git.cfg.command, "uvx");
        assert_eq!(git.cfg.args, vec!["mcp-git"]);
        assert_eq!(git.cfg.env["A"], "b");
        let web = c.iter().find(|x| x.cfg.name == "web").unwrap();
        assert_eq!(web.cfg.url.as_deref(), Some("https://w/mcp"));
    }

    #[test]
    fn codex_toml_entries() {
        let raw = r#"
model = "gpt"
[mcp_servers.time]
command = "uvx"
args = ["mcp-server-time"]
[mcp_servers.db]
url = "http://localhost:9000/mcp"
"#;
        let c = from_codex_toml(raw, "Codex CLI");
        assert_eq!(c.len(), 2);
        let time = c.iter().find(|x| x.cfg.name == "time").unwrap();
        assert_eq!(time.cfg.command, "uvx");
        assert_eq!(time.cfg.args, vec!["mcp-server-time"]);
        let db = c.iter().find(|x| x.cfg.name == "db").unwrap();
        assert_eq!(db.cfg.url.as_deref(), Some("http://localhost:9000/mcp"));
    }

    #[test]
    fn jsonc_comments_tolerated() {
        let raw = "// lead comment\n{\"mcpServers\": {\n  // inner\n  \"a\": {\"command\": \"x\"}\n}}\n";
        let v: Value = serde_json::from_str(raw).or_else(|_| serde_json::from_str(&strip_line_comments(raw))).unwrap();
        assert_eq!(from_mcp_servers_map(&v["mcpServers"], "Cursor").len(), 1);
    }

    #[test]
    fn name_sanitizing() {
        assert_eq!(sanitize_name("My Server!"), "my-server");
        assert_eq!(sanitize_name("  Weird__Name  "), "weird__name");
        assert_eq!(sanitize_name("///"), "");
    }
}
