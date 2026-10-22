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

pub fn dir() -> PathBuf {
    crate::config::Config::dir().join("sessions")
}

fn path_for(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

/// Human-readable goal/plan tracker written alongside the session as
/// `sessions/<id>.progress.md` — survives compaction, greppable, and shows
/// exactly which steps were skipped rather than silently marked done.
pub fn write_progress(id: &str, goal: &GoalState) {
    let mut md = format!("# Goal\n\n{}\n\n## Plan\n\n", goal.text.trim());
    if goal.plan.is_empty() {
        md.push_str("_(no steps published yet)_\n");
    }
    for (label, status) in &goal.plan {
        let mark = match status.as_str() {
            "done" => "x",
            "failed" => "!",
            "skipped" => "-",
            _ => " ",
        };
        md.push_str(&format!("- [{mark}] {label}  `({status})`\n"));
    }
    md.push_str(&format!(
        "\n_verified: {} · updated: {}_\n",
        goal.verified,
        fmt_ts(now_ms())
    ));
    let _ = std::fs::create_dir_all(dir());
    let _ = std::fs::write(dir().join(format!("{id}.progress.md")), md);
}

/// Suffixes that belong to a session group (shared stem = shared session id).
const GROUP_SUFFIXES: [&str; 4] = [".log.jsonl", ".injection.log", ".progress.md", ".json"];

fn group_stem(name: &str) -> Option<&str> {
    GROUP_SUFFIXES.iter().find(|suf| name.ends_with(*suf)).map(|suf| &name[..name.len() - suf.len()])
}

/// Result of a [`prune`] pass.
#[derive(Debug, Clone, Copy)]
pub struct PruneReport {
    pub removed_groups: usize,
    pub freed_bytes: u64,
}

/// Enforce the disk budget over `sessions/`: when all session files (JSON +
/// injection/activity/progress sidecars) exceed `max_bytes`, whole groups are
/// deleted oldest-first (session id = creation ms) until it fits. The group
/// named by `exclude_id` — the live session — is never touched. Returns
/// `None` when nothing needed deleting.
pub fn prune(exclude_id: &str, max_bytes: u64) -> Option<PruneReport> {
    let d = dir();
    let rd = std::fs::read_dir(&d).ok()?;
    // (id, files, bytes) per group
    let mut groups: std::collections::BTreeMap<u64, (Vec<PathBuf>, u64)> = Default::default();
    let mut total = 0u64;
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str())?.to_string();
        let stem = group_stem(&name)?;
        let id: u64 = stem.parse().ok()?;
        let size = p.metadata().map(|m| m.len()).unwrap_or(0);
        total += size;
        let g = groups.entry(id).or_insert_with(|| (Vec::new(), 0));
        g.0.push(p);
        g.1 += size;
    }
    if total <= max_bytes || groups.is_empty() {
        return None;
    }
    let mut removed = 0usize;
    let mut freed = 0u64;
    for (id, (files, bytes)) in groups {
        if total <= max_bytes {
            break;
        }
        if id.to_string() == exclude_id {
            continue;
        }
        for f in files {
            let _ = std::fs::remove_file(&f);
        }
        removed += 1;
        freed += bytes;
        total -= bytes;
    }
    if removed == 0 {
        return None;
    }
    Some(PruneReport { removed_groups: removed, freed_bytes: freed })
}

/// Throttled prune: called from session saves; does a real pass at most once
/// every 5 minutes per process. Returns the report when something was deleted.
pub fn maybe_prune(exclude_id: &str, max_bytes: u64) -> Option<PruneReport> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 5 * 60 * 1000 {
        return None;
    }
    LAST.store(now, Ordering::Relaxed);
    prune(exclude_id, max_bytes)
}

/// Total bytes used by all session files (for the `/sessions` display).
pub fn total_bytes() -> u64 {
    let mut n = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            if let Ok(m) = e.path().metadata() {
                if m.is_file() {
                    n += m.len();
                }
            }
        }
    }
    n
}

/// Format a byte count human-readably (for info lines).
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn save(s: &Session) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(path_for(&s.id), serde_json::to_string_pretty(s)?)?;
    Ok(())
}

/// Append a timestamped security/injection event to `sessions/<id>.injection.log`.
/// Deliberately separate from the session JSON: compaction rewrites messages,
/// but this evidence must survive it. Best-effort — never fails a turn.
pub fn log_injection(session_id: &str, line: &str) {
    #[cfg(test)]
    {
        let _ = (session_id, line);
        return; // unit tests must not write into the user's config dir
    }
    #[allow(unreachable_code)]
    {
        use std::io::Write;
        let ms = now_ms();
        let path = dir().join(format!("{session_id}.injection.log"));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "[{}] {}", fmt_ts(ms), line);
        }
    }
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

    fn seed(id: &str, ext: &str, bytes: usize) {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(dir().join(format!("{id}{ext}")), vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn prune_deletes_oldest_groups_whole() {
        crate::config::with_temp_config_dir("prune", || {
            // three sessions; group 100 has sidecars too
            seed("100", ".json", 400);
            seed("100", ".injection.log", 100);
            seed("100", ".log.jsonl", 100);
            seed("200", ".json", 400);
            seed("300", ".json", 400);
            // over a 900-byte budget → must drop the oldest group (100, 600 B)
            let r = prune("300", 900).expect("prune should fire");
            assert_eq!(r.removed_groups, 1);
            assert_eq!(r.freed_bytes, 600);
            assert!(!dir().join("100.json").exists());
            assert!(!dir().join("100.injection.log").exists(), "sidecar must go with its group");
            assert!(!dir().join("100.log.jsonl").exists());
            assert!(dir().join("200.json").exists() && dir().join("300.json").exists());
            // under budget now → no-op
            assert!(prune("300", 900).is_none());
        });
    }

    #[test]
    fn prune_never_touches_the_live_session() {
        crate::config::with_temp_config_dir("prune-live", || {
            seed("100", ".json", 400);
            seed("200", ".json", 400);
            // budget below a single group: only the excluded one survives
            let r = prune("100", 100).expect("must prune");
            assert_eq!(r.removed_groups, 1);
            assert!(dir().join("100.json").exists(), "live session was deleted!");
            assert!(!dir().join("200.json").exists());
        });
    }

    #[test]
    fn progress_file_tracks_step_statuses() {
        crate::config::with_temp_config_dir("progress", || {
            write_progress(
                "42",
                &GoalState {
                    text: "ship the thing".into(),
                    plan: vec![
                        ("first".into(), "done".into()),
                        ("second".into(), "skipped".into()),
                        ("third".into(), "pending".into()),
                    ],
                    verified: true,
                },
            );
            let md = std::fs::read_to_string(dir().join("42.progress.md")).unwrap();
            assert!(md.contains("- [x] first"));
            assert!(md.contains("- [-] second"), "skipped must be visible, not silently done:\n{md}");
            assert!(md.contains("- [ ] third"));
        });
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
