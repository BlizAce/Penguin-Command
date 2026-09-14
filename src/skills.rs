use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A reusable procedure the agent saves for future sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub instructions: String,
}

fn dir() -> PathBuf {
    crate::config::Config::dir().join("skills")
}

/// Keep names filesystem-safe.
pub fn sanitize(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c.is_whitespace() || c == '.' || c == '/' {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn path_for(name: &str) -> PathBuf {
    dir().join(format!("{name}.toml"))
}

pub fn save(s: &Skill) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(path_for(&s.name), toml::to_string_pretty(s)?)?;
    Ok(())
}

pub fn load(name: &str) -> Option<Skill> {
    let raw = std::fs::read_to_string(path_for(name)).ok()?;
    toml::from_str(&raw).ok()
}

pub fn list() -> Vec<Skill> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&p) {
                if let Ok(s) = toml::from_str::<Skill>(&raw) {
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One-line-per-skill summary for the system prompt.
pub fn prompt_block() -> String {
    let skills = list();
    if skills.is_empty() {
        return "No skills saved yet.".into();
    }
    let mut s = String::from("Saved skills (reusable procedures from past sessions):\n");
    for sk in &skills {
        s.push_str(&format!("- {}: {}\n", sk.name, sk.description));
    }
    s
}
