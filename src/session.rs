use crate::providers::ChatMsg;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalState {
    pub text: String,
    /// Steps from update_plan: (label, status).
    #[serde(default)]
    pub plan: Vec<(String, String)>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub updated_ms: u64,
    pub title: String,
    pub workspace: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<ChatMsg>,
    /// The last /goal worked on in this session, if any. `#[serde(default)]`
    /// keeps sessions saved before this field existed loadable.
    #[serde(default)]
    pub goal: Option<GoalState>,
}

impl Session {
    pub fn new(
        id: String,
        env: &crate::agent::AgentEnv,
        messages: Vec<ChatMsg>,
        goal: Option<GoalState>,
    ) -> Session {
        let title = messages
            .iter()
            .find(|m| m.role == crate::providers::Role::User)
            .map(|m| crate::providers::truncate(&m.text, 72).to_string())
            .unwrap_or_else(|| "new session".into());
        Session {
            id,
            updated_ms: now_ms(),
            title,
            workspace: env.workspace.display().to_string(),
            provider: env.provider.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
            model: env.model.clone(),
            messages,
            goal,
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Civil date/time from unix millis (no chrono needed).
pub fn fmt_ts(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

pub fn new_id() -> String {
    format!("{}", now_ms())
}

fn dir() -> PathBuf {
    crate::config::Config::dir().join("sessions")
}

fn path_for(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

pub fn save(s: &Session) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(path_for(&s.id), serde_json::to_string_pretty(s)?)?;
    Ok(())
}

pub fn load(id: &str) -> Option<Session> {
    let raw = std::fs::read_to_string(path_for(id)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// All sessions, newest first.
pub fn list() -> Vec<Session> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&p) {
                if let Ok(s) = serde_json::from_str::<Session>(&raw) {
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    out
}

/// Newest session without parsing every file: ids are ms timestamps, so the
/// numerically-largest filename is the newest; fall back to a full scan only
/// if that file is unreadable.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_sessions_without_goal_still_load() {
        let old = r#"{"id":"1","updated_ms":2,"title":"t","workspace":"/w","provider":"p","model":"m","messages":[]}"#;
        let s: Session = serde_json::from_str(old).unwrap();
        assert!(s.goal.is_none());
    }

    #[test]
    fn goal_state_roundtrips() {
        let s = Session {
            id: "2".into(),
            updated_ms: 3,
            title: "t".into(),
            workspace: "/w".into(),
            provider: "p".into(),
            model: "m".into(),
            messages: vec![ChatMsg::user("hi")],
            goal: Some(GoalState {
                text: "ship it".into(),
                plan: vec![("step one".into(), "active".into()), ("step two".into(), "pending".into())],
                verified: false,
            }),
        };
        let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let g = back.goal.unwrap();
        assert_eq!(g.text, "ship it");
        assert_eq!(g.plan.len(), 2);
        assert_eq!(g.plan[0].1, "active");
        assert!(!g.verified);
    }
}

pub fn latest() -> Option<Session> {
    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let id = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok());
            files.push((id.unwrap_or(0), p));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, p) in files {
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(s) = serde_json::from_str::<Session>(&raw) {
                return Some(s);
            }
        }
    }
    None
}
