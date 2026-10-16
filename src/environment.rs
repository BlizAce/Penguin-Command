//! The environment profile: a small living document of machine-specific facts
//! penguin discovers while working — hardware quirks, installed toolchains and
//! their real locations/versions, network constraints, user preferences. It is
//! injected (tail-truncated) into every system prompt so knowledge gained in
//! one session survives into the next instead of dying with the transcript.
//!
//! The file itself lives at `~/.config/penguin/environment.md` and stays plain
//! markdown — the user can read and edit it by hand at any time.

use anyhow::Result;
use std::path::PathBuf;

pub const FILE_NAME: &str = "environment.md";
/// Beyond this the agent must tidy (replace) instead of appending forever.
const MAX_BYTES: usize = 16_384;
/// How much of the profile rides along in the system prompt.
const PROMPT_TAIL_CHARS: usize = 2_000;

pub fn path() -> PathBuf {
    crate::config::Config::dir().join(FILE_NAME)
}

pub fn load_raw() -> String {
    std::fs::read_to_string(path()).unwrap_or_default()
}

/// Normalize a line for duplicate comparison: drop a leading list bullet and
/// the trailing "(date)" stamp add_note appends, then case/period-fold. So
/// "- GPU is an RTX 4090 (2026-…)" and "gpu is an rtx 4090." are one fact.
fn norm(s: &str) -> String {
    let mut t = s.trim();
    t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).unwrap_or(t).trim();
    if t.ends_with(')') {
        // strip a single trailing "(…)" group (the date stamp) when balanced.
        if let Some(i) = t.rfind(" (") {
            if t[i + 1..].matches('(').count() == 1 {
                t = t[..i].trim_end();
            }
        }
    }
    t.trim_end_matches('.').to_lowercase()
}

/// True when two notes mean the same thing.
fn same_note(a: &str, b: &str) -> bool {
    norm(a) == norm(b)
}

/// Append one bullet to the profile. Duplicate facts (the usual accident of
/// rediscovering something already known) are dropped, not doubled.
pub fn add_note(fact: &str) -> Result<AddOutcome> {
    let fact = fact.trim();
    if fact.is_empty() {
        return Ok(AddOutcome::Empty);
    }
    let mut body = load_raw();
    if body.lines().any(|l| same_note(l, fact)) {
        return Ok(AddOutcome::Duplicate);
    }
    if body.len() + fact.len() > MAX_BYTES {
        return Ok(AddOutcome::TooBig);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    let date = crate::session::fmt_ts(crate::session::now_ms());
    body.push_str(&format!("- {fact} ({date})\n"));
    std::fs::create_dir_all(crate::config::Config::dir()).ok();
    std::fs::write(path(), &body)?;
    Ok(AddOutcome::Added)
}

pub enum AddOutcome {
    Added,
    Duplicate,
    TooBig,
    Empty,
}

/// Replace the whole profile (tidy-up path). The previous content is kept as
/// `environment.md.bak` and returned so nothing is lost silently.
pub fn replace(new_body: &str) -> Result<String> {
    let old = load_raw();
    if !old.is_empty() {
        let _ = std::fs::write(path().with_extension("md.bak"), &old);
    }
    std::fs::create_dir_all(crate::config::Config::dir()).ok();
    std::fs::write(path(), new_body)?;
    Ok(old)
}

/// The profile block for the system prompt: tail-truncated (newest notes are
/// at the bottom of the file), with a pointer to the full document.
pub fn prompt_block() -> String {
    let body = load_raw();
    if body.trim().is_empty() {
        return String::new();
    }
    let chars: Vec<char> = body.chars().collect();
    let shown: String = if chars.len() > PROMPT_TAIL_CHARS {
        format!("…(older notes elided)…\n{}", chars[chars.len() - PROMPT_TAIL_CHARS..].iter().collect::<String>())
    } else {
        body.clone()
    };
    format!(
        "\nEnvironment profile (machine facts learned in past sessions — verify before trusting blindly; \
         update it via save_env_note when something changes):\n{shown}\n\
         Full file: {} — keep it current.\n",
        path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_detection_ignores_case_and_punctuation() {
        assert!(same_note("GPU is an RTX 4090", "gpu is an rtx 4090."));
        assert!(same_note("- nvm installed via script", "nvm installed via script"));
        assert!(!same_note("python 3.12 default", "python 3.11 in venv"));
    }

    #[test]
    fn prompt_block_empty_when_no_profile() {
        // In unit tests the real config dir may or may not exist; an empty or
        // missing profile must yield an empty block, never a panic.
        let _ = prompt_block();
    }

    #[test]
    fn add_note_appends_dedupes_and_replace_backs_up() {
        crate::config::with_temp_config_dir("env", || {
            assert!(matches!(add_note("GPU is an RTX 4090").unwrap(), AddOutcome::Added));
            // Case/punctuation-insensitive duplicate is dropped, not doubled.
            assert!(matches!(add_note("gpu is an rtx 4090.").unwrap(), AddOutcome::Duplicate));
            assert!(matches!(add_note("").unwrap(), AddOutcome::Empty));
            let body = load_raw();
            assert_eq!(body.matches("4090").count(), 1);

            // A second, distinct fact is appended.
            assert!(matches!(add_note("python3.12 is the system default").unwrap(), AddOutcome::Added));
            assert!(load_raw().contains("system default"));

            // replace_all backs up the old content and swaps it wholesale.
            let old = replace("# Machine\n- curated profile\n").unwrap();
            assert!(old.contains("4090"));
            assert_eq!(load_raw(), "# Machine\n- curated profile\n");
            assert!(std::fs::read_to_string(path().with_extension("md.bak")).unwrap().contains("4090"));

            // prompt_block now carries the new content.
            assert!(prompt_block().contains("curated profile"));
        });
    }
}
