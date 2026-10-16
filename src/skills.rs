use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One archived version of a skill's content, kept when an update overwrites
/// the previous recipe. Newest first, capped by HISTORY_CAP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRev {
    pub saved_ms: u64,
    pub description: String,
    pub instructions: String,
}

/// How many past versions a skill keeps when it is updated.
const HISTORY_CAP: usize = 5;

/// A reusable procedure the agent saves for future sessions.
///
/// Metadata fields carry `#[serde(default)]` so skills written before they
/// existed still load untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// First save (unix ms).
    #[serde(default)]
    pub created_ms: u64,
    /// Last content change (unix ms).
    #[serde(default)]
    pub updated_ms: u64,
    /// Last time load_skill pulled this into a session (unix ms).
    #[serde(default)]
    pub last_used_ms: u64,
    /// Total loads across all sessions — drives prompt ordering.
    #[serde(default)]
    pub use_count: u32,
    /// Archived content from previous saves (newest first).
    #[serde(default)]
    pub history: Vec<SkillRev>,
}

impl Skill {
    /// Fresh metadata for a brand-new skill.
    fn stamp_new(&mut self) {
        let now = crate::session::now_ms();
        if self.created_ms == 0 {
            self.created_ms = now;
        }
        self.updated_ms = now;
    }
}


/// Construct a skill with fresh metadata (used for bundled seeds and tools).
pub fn new_skill(name: &str, description: &str, instructions: &str) -> Skill {
    let mut sk = Skill {
        name: name.into(),
        description: description.into(),
        instructions: instructions.into(),
        created_ms: 0,
        updated_ms: 0,
        last_used_ms: 0,
        use_count: 0,
        history: vec![],
    };
    sk.stamp_new();
    sk
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

/// Save a skill. Overwriting an existing one is an *update*: the previous
/// description+instructions are archived into `history` (capped) so nothing
/// proven ever silently disappears, and created_ms is preserved.
pub fn save(s: &Skill) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    let mut s = s.clone();
    if let Some(prev) = load(&s.name) {
        s.created_ms = prev.created_ms;
        s.use_count = prev.use_count;
        s.last_used_ms = prev.last_used_ms;
        // Only archive when the content actually changed.
        if prev.instructions != s.instructions || prev.description != s.description {
            // Keep any history the caller carried, add the version being
            // replaced on top, newest-first, capped.
            let mut hist = std::mem::take(&mut s.history);
            hist.insert(0, SkillRev { saved_ms: prev.updated_ms, description: prev.description, instructions: prev.instructions });
            hist.sort_by_key(|r| std::cmp::Reverse(r.saved_ms));
            hist.truncate(HISTORY_CAP);
            s.history = hist;
        } else {
            s.history = prev.history;
        }
    }
    s.stamp_new();
    std::fs::write(path_for(&s.name), toml::to_string_pretty(&s)?)?;
    Ok(())
}

/// Merge-style update: overwrite only the fields that are Some. Returns an
/// error string in Err when the skill does not exist (callers feed that back
/// to the model so it creates instead).
pub fn update(name: &str, description: Option<&str>, instructions: Option<&str>) -> Result<bool> {
    let mut sk = match load(name) {
        Some(s) => s,
        None => return Ok(false),
    };
    if let Some(d) = description {
        if !d.trim().is_empty() {
            sk.description = d.trim().to_string();
        }
    }
    if let Some(i) = instructions {
        if !i.trim().is_empty() {
            sk.instructions = i.to_string();
        }
    }
    save(&sk)?;
    Ok(true)
}

pub fn load(name: &str) -> Option<Skill> {
    let raw = std::fs::read_to_string(path_for(name)).ok()?;
    toml::from_str(&raw).ok()
}

/// Record that a skill was loaded into a session (usage + recency bookkeeping).
/// `updated_ms` deliberately does not move: this is usage, not a content change.
pub fn touch(name: &str) {
    if let Some(mut sk) = load(name) {
        sk.use_count += 1;
        sk.last_used_ms = crate::session::now_ms();
        if let Ok(txt) = toml::to_string_pretty(&sk) {
            let _ = std::fs::write(path_for(name), txt);
        }
    }
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

/// Case-insensitive substring search across name, description and
/// instructions. Every query word must appear somewhere (AND), so
/// "weasyprint table" narrows instead of widening.
pub fn search(query: &str) -> Vec<Skill> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(|w| w.to_string()).collect();
    if words.is_empty() {
        return list();
    }
    list()
        .into_iter()
        .filter(|s| {
            let hay = format!("{}\n{}\n{}", s.name, s.description, s.instructions).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect()
}

/// Built-in skills seeded into the library on first run (user edits win —
/// an existing skill with the same name is never overwritten).
fn bundled() -> Vec<Skill> {
    let setup_mcp = new_skill(
        "setup-mcp",
        "Connect penguin to MCP (Model Context Protocol) servers: install, configure, verify, troubleshoot",
        r#"# Setting up an MCP server for penguin

MCP servers give the agent external tools (filesystem scopes, APIs, browsers,
databases…). Penguin speaks MCP over stdio (spawned commands) and Streamable
HTTP (hosted endpoints).

## Preferred path — slash commands (live, verified)
1. Find out what the user wants to reach (files in a dir? GitHub? a browser?)
   and which runtime they have: `run_command` → `command -v npx uvx node python3`.
2. Add it (the `--` separates name from the launch line; URLs work too):
   `/mcp add <name> -- npx -y @modelcontextprotocol/server-filesystem /some/dir`
   `/mcp add weather -- uvx mcp-weather`
   `/mcp add hosted -- https://example.com/mcp`
   Penguin probes the connection immediately and reports the tool count.
3. Verify: `/mcp` shows status; `/mcp tools <name>` lists exposed tools
   (they reach the model as `mcp__<name>__<tool>`).
4. Safety: calls ask the user for approval by default; per-tool "always allow"
   is remembered, and `/mcp trust <name>` skips prompts entirely — only do
   that when the user explicitly asks.

## Canonical servers (npx / uvx)
- filesystem: `npx -y @modelcontextprotocol/server-filesystem <dir…>`
- fetch:      `uvx mcp-server-fetch`
- git:        `uvx mcp-server-git`  (or npx @modelcontextprotocol/server-git)
- memory:     `npx -y @modelcontextprotocol/server-memory`
Anything on npm or PyPI with "mcp" in the name follows the same pattern.

## Config file (alternative, survives restarts too — slash commands write it)
~/.config/penguin/config.toml:
    [[mcp.servers]]
    name = "filesystem"
    command = "npx"
    args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/projects"]
    env = { GITHUB_TOKEN = "$GITHUB_TOKEN" }   # $VAR expands from environment
    enabled = true
    trust = false
    timeout_secs = 60
For HTTP servers use `url = "https://…"` (+ optional `headers`) instead of
command/args.

## Importing from other agents
`/mcp import` scans Claude Desktop, Claude Code, opencode, Codex CLI, Gemini
CLI and Cursor configs; add picks with `/mcp import 1,3` or `--all`.

## Troubleshooting
- "spawning … failed" → command missing: install node (for npx) or uv (for
  uvx), or use an absolute path.
- Handshake timeout → run the exact launch line via run_command interactively;
  MCP servers must print only JSON-RPC on stdout — debug output must go to
  stderr. Raise timeout_secs for slow cold starts (npx downloads once).
- Server died mid-session → `/mcp reconnect <name>`; check its stderr tail in
  the error message.
- Tools missing from a turn → server shows `✗ failed` in /mcp; fix + reconnect.
- A tool call hangs → it hits timeout_secs and reports; Esc aborts the turn."#,
    );

    let code_craft = new_skill(
        "code-craft",
        "Engineering standards for any coding task: SOLID, language idioms, docs, self-review, tests, git — load before writing or changing code",
        r#"# Code craft standards

Follow these on every coding task unless the project's own conventions (README,
CONTRIBUTING, surrounding code) say otherwise. Project convention wins.

## Design
- SOLID where it applies: single responsibility per module/class, open for
  extension via traits/interfaces rather than editing switch-chains, Liskov-safe
  subtypes, small client-specific interfaces over god-objects, depend on
  abstractions at system boundaries. Don't over-abstract a 50-line script —
  SOLID is for growth, not ceremony.
- Match the language's idioms and current best practice (rustfmt/clippy for
  Rust; PEP8/ruff/black + type hints for Python; ESLint/prettier + modern ES
  for JS/TS; gofmt/go vet for Go; clang-format/-tidy for C++). Check which
  tooling the repo already configures and use THAT.
- No dead code, no commented-out blocks left behind, no TODO without an issue.

## Documentation & comments
- Doc-comment every public function/type/struct (what + params + returns;
  why over what for non-obvious logic). Private helpers: comment intent only
  where the code doesn't speak for itself.
- Update README / docs files whenever behavior, setup or interfaces change.

## Self-review after writing (do this unprompted, before declaring done)
1. Re-read every file you changed end-to-end (read_file) — do not trust memory.
2. Check: names accurate? error paths handled? edge cases (empty input,
   unicode, huge files)? no leftover debug prints? diff does exactly the task,
   nothing else?
3. Run the project's static checks: build + linter/formatter check + type
   checker if configured. Fix everything they report.

## Testing (never wait to be asked)
- After implementing, run whatever tests exist (`cargo test`, `pytest`,
  `npm test`, ctest…). All must pass.
- No tests for what you built? Write at least smoke tests covering the happy
  path plus 2-3 edge cases; unit-test pure logic directly. Put them where the
  project puts tests (tests/ dir, *_test.go, tests/*.py …).
- For anything with a GUI/web UI: verify it actually renders — screenshot +
  programmatic checks (non-blank frame, expected colors/text/layout via PIL or
  playwright DOM assertions), or view_screenshot when the provider has vision.
  A build that compiles but shows nothing is a failure.

## Git (when the workspace is a repo)
- Check `git status` before starting; commit coherent units with why-focused
  messages (see write-why-focused-commit-message skill). Never rewrite history,
  never force-push, never touch git config or credentials without being asked."#,
    );

    let research_method = new_skill(
        "research-method",
        "Scientific loop for experiments and investigations: hypothesis → experiment → result → why → document; search patterns that isolate variables",
        r#"# Research method

The loop, always in this order, always documented as you go (a running notes
file in the workspace beats memory — yours dies with the context window):

## 1. Theory / hypothesis
- State it in writing BEFORE running anything: "If X, then Y, because Z."
- Name what will count as confirming vs refuting it (a number, a threshold).

## 2. Experiment
- Change ONE variable at a time. A baseline run with nothing changed is part
  of the experiment, not wasted time.
- Keep everything else fixed and recorded: seeds, versions, configs, hardware
  state. Log exact commands so any result is reproducible.
- For searches/investigations: vary one filter at a time (query terms, date
  range, source), then widen deliberately — note what each widening costs
  and gains. Also check what can be ADDED, not just changed: new sources,
  new metrics, new angles that were absent from the first pass.

## 3. Result
- Capture raw numbers to files first (CSV/JSON), analyze second. Never
  transcribe numbers by hand into prose from memory.
- Sanity-check: sample size? outliers? did the thing you think you changed
  actually change (verify via logs/hashes)?

## 4. Why
- Explain the result mechanically before believing it. A number without a
  mechanism is an anecdote. If the result contradicts the hypothesis, that IS
  the finding — chase it instead of rescuing the theory.
- Cross-check surprising results with one independent method before writing
  them up.

## 5. Document (always) + new theories
- Append to the notes file after every experiment: hypothesis, exact method,
  raw numbers, verdict, why, next question.
- Every finding spawns the next theory — write it down as the header of the
  next loop.
- For major findings use the report-deliverables skill (paper + PDF + raw
  data in the workspace)."#,
    );

    let report_deliverables = new_skill(
        "report-deliverables",
        "Write up a major finding: MD paper with methods+numbers, PDF with graphs for key data, raw numbers files — all in the workspace",
        r#"# Report deliverables for major findings

When an investigation produces a result worth keeping, deliver three things
into the working folder (not just chat text — chat scrolls away):

## 1. The paper — `<topic>-report.md`
Sections, in order:
- **Abstract** — 3-5 sentences: question, method, headline numbers, so what.
- **Hypothesis & background** — what was predicted and why it matters.
- **Method** — exact steps another agent could rerun blind: commands, seeds,
  versions, hardware, sample sizes, what was varied vs held fixed.
- **Results** — hard numbers in tables (every table also exists as CSV).
  Quote measured values with units and uncertainty; never round away the
  finding.
- **Analysis / why** — mechanism behind the numbers; confounds considered.
- **Limitations & next theories** — honest weaknesses + the specific follow-up
  experiments each weakness suggests.
- **Appendix** — raw command logs, full data listings or file pointers.

## 2. Graphs → PDF
- matplotlib (or equivalent) for every key dataset: label axes+units, one
  takeaway per figure, captions that stand alone without the text.
- Render MD→PDF with weasyprint (pip install if missing; check the
  weasyprint-wide-table-pdf-layout / weasyprint-figure-legibility-layout
  skills for known layout traps). Verify the PDF opens and pages are not
  blank/overflowed before claiming it done.

## 3. Raw numbers
- Every number in the paper must trace to a file: CSV/JSON in a `data/`
  subfolder, referenced from the tables ("source: data/run1.csv").
- Include the generation command in a header comment or a DATA_NOTES.md.

## Final check before finish_goal
`ls` the deliverables, re-open the MD and PDF (page count, figure presence),
and paste the key numbers from the raw files next to the paper's claims —
the verification evidence is that they match."#,
    );

    vec![setup_mcp, code_craft, research_method, report_deliverables]
}

/// Seed bundled skills (call once at startup). User edits win: an existing
/// skill with a matching name is never overwritten.
pub fn ensure_bundled() {
    for sk in bundled() {
        if load(&sk.name).is_none() {
            let _ = save(&sk);
        }
    }
}

/// One-line-per-skill summary for the system prompt, ordered by proven value
/// (most used first, then most recently updated) so the strongest recipes sit
/// at the top as the library grows.
pub fn prompt_block() -> String {
    let mut skills = list();
    skills.sort_by(|a, b| {
        b.use_count
            .cmp(&a.use_count)
            .then(b.updated_ms.cmp(&a.updated_ms))
            .then(a.name.cmp(&b.name))
    });
    if skills.is_empty() {
        return "No skills saved yet.".into();
    }
    let mut s = String::from("Saved skills (reusable procedures from past sessions):\n");
    for sk in &skills {
        s.push_str(&format!("- {}: {}\n", sk.name, sk.description));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrips_and_legacy_files_load() {
        // Legacy files (no metadata keys) must still load with defaults.
        let legacy = "name = \"old\"\ndescription = \"d\"\ninstructions = \"i\"\n";
        let s: Skill = toml::from_str(legacy).unwrap();
        assert_eq!(s.use_count, 0);
        assert!(s.history.is_empty());
    }

    #[test]
    fn save_update_archives_previous_and_search_finds_by_body() {
        crate::config::with_temp_config_dir("skills", || {
            save(&new_skill("pdf-layout", "one line", "original steps")).unwrap();
            let first = load("pdf-layout").unwrap();
            assert!(first.created_ms > 0 && first.updated_ms > 0);

            // Update the body: the old recipe must survive in history.
            update("pdf-layout", Some("sharper"), Some("better steps")).unwrap();
            let after = load("pdf-layout").unwrap();
            assert_eq!(after.description, "sharper");
            assert_eq!(after.instructions, "better steps");
            assert_eq!(after.history.len(), 1);
            assert_eq!(after.history[0].instructions, "original steps");
            // created_ms is preserved across an update.
            assert_eq!(after.created_ms, first.created_ms);

            // search() matches on body text, not just the name.
            let hits = search("better steps");
            assert!(hits.iter().any(|s| s.name == "pdf-layout"));
            // AND semantics: a word that isn't present yields nothing.
            assert!(search("better zzznotpresent").is_empty());

            // touch() bumps usage without touching updated_ms.
            let before_touch = load("pdf-layout").unwrap();
            touch("pdf-layout");
            let touched = load("pdf-layout").unwrap();
            assert_eq!(touched.use_count, before_touch.use_count + 1);
            assert_eq!(touched.updated_ms, before_touch.updated_ms);

            // update on a missing skill reports false (caller suggests save_skill).
            assert!(!update("nope-does-not-exist", Some("x"), None).unwrap());
        });
    }

    #[test]
    fn ensure_bundled_seeds_once_and_respects_user_edits() {
        crate::config::with_temp_config_dir("bundled", || {
            ensure_bundled();
            let names: Vec<String> = list().into_iter().map(|s| s.name).collect();
            for expected in ["setup-mcp", "code-craft", "research-method", "report-deliverables"] {
                assert!(names.iter().any(|n| n == expected), "missing bundled skill {expected}");
            }
            // A user edit must survive a re-seed.
            update("code-craft", Some("my own wording"), None).unwrap();
            ensure_bundled();
            assert_eq!(load("code-craft").unwrap().description, "my own wording");
        });
    }

    #[test]
    fn search_matches_across_fields_with_and_semantics() {
        // Pure function over the query logic — mirror of what search() applies.
        let words: Vec<String> = "weasy table".split_whitespace().map(|w| w.to_lowercase()).collect();
        let hay = "name weasyprint-wide-table-pdf-layout\ndesc x\ninstr y".to_lowercase();
        assert!(words.iter().all(|w| hay.contains(w)));
        let no = "table only-name".to_lowercase();
        assert!(!words.iter().all(|w| no.contains(w)));
    }

    #[test]
    fn sanitize_stays_safe() {
        assert_eq!(sanitize("  My Skill/Name! "), "my-skill-name");
        assert_eq!(sanitize("../etc/passwd"), "etc-passwd");
    }
}
