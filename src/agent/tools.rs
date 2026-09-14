use crate::providers::{ToolSpec, truncate};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;
use walkdir::WalkDir;

pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "run_command",
            description: "Run a shell command in the workspace (non-interactive). Returns stdout+stderr. Use for system tasks, package queries, git builds, etc. If it needs root, just prefix with sudo — penguin prompts the user for their password securely (hidden input); never ask them to paste it.",
            schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The shell command line"},
                    "cwd": {"type": "string", "description": "Optional directory to run in (defaults to workspace)"},
                    "timeout_secs": {"type": "integer", "description": "Max runtime, default 120"}
                },
                "required": ["command"]
            }),
        },
        ToolSpec {
            name: "read_file",
            description: "Read a text file anywhere on the system. Returns numbered lines.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "description": "1-based start line"},
                    "limit": {"type": "integer", "description": "max lines, default 400"}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write_file",
            description: "Create or overwrite a file with the given content.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "edit_file",
            description: "Replace an exact string occurrence in a file. Read the file first.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "old_string": {"type": "string"}, "new_string": {"type": "string"}},
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolSpec {
            name: "list_dir",
            description: "List a directory (names, sizes, permissions).",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "find_files",
            description: "Find files by name glob or content regex under a root dir.",
            schema: json!({
                "type": "object",
                "properties": {
                    "root": {"type": "string"},
                    "name_glob": {"type": "string", "description": "e.g. *.rs (case-insensitive)"},
                    "content_regex": {"type": "string"}
                },
                "required": ["root"]
            }),
        },
        ToolSpec {
            name: "set_permissions",
            description: "Change file permissions (octal like 755) or owner user:group. May require approval if outside workspace.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "mode": {"type": "string"}, "owner": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "update_plan",
            description: "Publish or update the step plan for a goal. steps: array of {label, status} with status in pending|active|done|failed.",
            schema: json!({
                "type": "object",
                "properties": {"steps": {"type": "array", "items": {"type": "object", "properties": {"label": {"type": "string"}, "status": {"type": "string"}}, "required": ["label"]}}},
                "required": ["steps"]
            }),
        },
        ToolSpec {
            name: "finish_goal",
            description: "Declare the current goal finished. You MUST have run a verification command proving success and include its evidence.",
            schema: json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}, "verification_evidence": {"type": "string"}},
                "required": ["summary", "verification_evidence"]
            }),
        },
        ToolSpec {
            name: "save_skill",
            description: "Save a reusable procedure (skill) into penguin's own system for future sessions. Call this after completing a goal or solving a non-trivial problem whose steps others (you, later) would otherwise have to rediscover.",
            schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "short-kebab-case name"},
                    "description": {"type": "string", "description": "one line: when to use it"},
                    "instructions": {"type": "string", "description": "the proven procedure, step by step"}
                },
                "required": ["name", "description", "instructions"]
            }),
        },
        ToolSpec {
            name: "load_skill",
            description: "Load the full instructions of a saved skill by name.",
            schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        ToolSpec {
            name: "list_skills",
            description: "List all skills saved in penguin's skill library.",
            schema: json!({"type": "object", "properties": {}}),
        },
    ]
}

pub enum ToolOutcome {
    /// Result text fed back to the model.
    Feed(String),
    /// update_plan payload: Vec<(label,status)>
    Plan(Vec<(String, String)>),
    /// finish_goal called
    GoalFinished { summary: String, evidence: String },
}

fn shell_cmd(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", cmd]);
        c
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut c = Command::new(shell);
        c.arg("-lc").arg(cmd);
        c
    }
}

/// Rewrite bare `sudo` at a command position to `sudo -S`, so the password is
/// read from our piped stdin (fed by penguin's hidden prompt) instead of the
/// controlling tty — where it would race with the TUI's own key reader.
fn inject_sudo_stdin(cmd: &str) -> (bool, String) {
    let mut out = String::with_capacity(cmd.len() + 8);
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0usize;
    let mut quote: Option<char> = None;
    let mut at_cmdpos = true;
    let mut found = false;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                out.push(c);
                i += 1;
            }
            '\\' if i + 1 < chars.len() => {
                out.push(c);
                out.push(chars[i + 1]);
                i += 2;
            }
            _ if c.is_whitespace() => {
                out.push(c);
                i += 1;
            }
            ';' | '|' | '&' | '\n' | '(' => {
                out.push(c);
                at_cmdpos = true;
                i += 1;
            }
            ')' | '<' | '>' | '`' => {
                out.push(c);
                i += 1;
            }
            _ => {
                let start = i;
                while i < chars.len()
                    && !chars[i].is_whitespace()
                    && !matches!(chars[i], ';' | '|' | '&' | '(' | ')' | '<' | '>' | '`' | '\n' | '\'' | '"' | '\\')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if at_cmdpos && word == "sudo" {
                    out.push_str("sudo -S");
                    found = true;
                    at_cmdpos = false;
                } else {
                    out.push_str(&word);
                    // `FOO=bar sudo …` keeps the command position
                    if !word.contains('=') {
                        at_cmdpos = false;
                    }
                }
            }
        }
    }
    (found, out)
}

/// True when a captured stream tail ends on an authentication prompt.
fn password_prompt(tail: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)(password|passcode)[^:\n]{0,48}:\s*$").unwrap());
    re.is_match(tail.trim_end())
}

pub fn run_shell(
    cmd: &str,
    cwd: &PathBuf,
    timeout_secs: u64,
    ask_secret: &dyn Fn(&str) -> Option<String>,
    abort: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    let (sudo_hit, effective) = inject_sudo_stdin(cmd);
    let mut c = shell_cmd(&effective);
    c.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Detach from our controlling terminal: anything still reaching for
    // /dev/tty fails cleanly instead of stealing keystrokes from the TUI.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    let mut child_stdin = child.stdin.take();

    let (tx, rx) = std::sync::mpsc::channel::<(bool, Vec<u8>)>();
    let streams: [(bool, Box<dyn Read + Send>); 2] = [
        (true, Box::new(child.stderr.take().unwrap())),
        (false, Box::new(child.stdout.take().unwrap())),
    ];
    for (is_err, mut rd) in streams {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match rd.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send((is_err, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(tx);

    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let mut answered_err = 0usize;
    let mut prompts_left = 3usize;
    let mut cancelled = false;
    let mut aborted = false;

    while !cancelled {
        if abort.load(std::sync::atomic::Ordering::Relaxed) {
            aborted = true;
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(250))) {
            Ok((false, chunk)) => out.extend_from_slice(&chunk),
            // sudo -S always asks on stderr; scanning only stderr avoids
            // false positives from command output that merely mentions it.
            Ok((true, chunk)) => {
                err.extend_from_slice(&chunk);
                if sudo_hit && prompts_left > 0 && err.len() > answered_err {
                    let tail = String::from_utf8_lossy(&err[err.len().saturating_sub(240)..]).to_string();
                    if password_prompt(&tail) {
                        answered_err = err.len();
                        prompts_left -= 1;
                        let shown = tail
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("password:")
                            .trim()
                            .to_string();
                        match ask_secret(&shown) {
                            Some(secret) => {
                                if let Some(w) = child_stdin.as_mut() {
                                    let _ = w.write_all(secret.as_bytes());
                                    let _ = w.write_all(b"\n");
                                    let _ = w.flush();
                                }
                            }
                            None => cancelled = true,
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    if cancelled || aborted {
        child.kill().ok();
    }
    drop(child_stdin);
    let status = child.wait_timeout(deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(2)))?;
    while let Ok((is_err, chunk)) = rx.try_recv() {
        if is_err {
            err.extend_from_slice(&chunk);
        } else {
            out.extend_from_slice(&chunk);
        }
    }

    let mut body = format!(
        "{}{}",
        String::from_utf8_lossy(&out),
        if err.is_empty() { String::new() } else { format!("\n[stderr]\n{}", String::from_utf8_lossy(&err)) }
    );
    if cancelled {
        body.push_str("\n[password prompt cancelled by user — command killed]");
    } else if aborted {
        body.push_str("\n[killed — agent aborted]");
    }
    match status {
        Some(st) => {
            let code = st.code().unwrap_or(-1);
            Ok(format!("{}\n[exit {code}]", middle_truncate(&body, 9000)))
        }
        None => {
            child.kill().ok();
            Ok(format!("{}\n[error: timed out after {timeout_secs}s, killed]", middle_truncate(&body, 9000)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{inject_sudo_stdin, password_prompt, run_shell};

    #[test]
    fn sudo_gets_stdin_flag() {
        assert_eq!(inject_sudo_stdin("sudo systemctl restart nginx").1, "sudo -S systemctl restart nginx");
        assert!(inject_sudo_stdin("cat f | sudo tee g").1.contains("sudo -S tee"));
        assert!(inject_sudo_stdin("true && sudo reboot").1.contains("sudo -S reboot"));
        // not an argument, not inside quotes, not a path
        assert!(!inject_sudo_stdin("grep sudo file").0);
        assert!(!inject_sudo_stdin("echo 'sudo rm'").0);
        assert!(!inject_sudo_stdin("/usr/bin/sudo ls").0);
    }

    #[test]
    fn prompt_detection() {
        assert!(password_prompt("[sudo] password for rift:"));
        assert!(password_prompt("root's password: "));
        assert!(!password_prompt("total 42\ndrwxr-xr-x 2 rift rift 4096 Sep 10 02:00 ."));
    }

    #[test]
    fn sudo_password_roundtrip_and_cancel() {
        // A PATH containing only a fake `sudo` that prompts on stderr and reads
        // the password from stdin — exactly what run_shell must answer.
        let dir = std::env::temp_dir().join(format!("pc-fake-sudo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sudo = dir.join("sudo");
        std::fs::write(&sudo, "#!/bin/sh\necho '[sudo] password for tester:' >&2\nread pw\necho \"got:$pw\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", &dir);
        let cwd = std::env::current_dir().unwrap();

        let no_abort = std::sync::atomic::AtomicBool::new(false);
        let out = run_shell("sudo whatever", &cwd, 30, &|p| {
            assert!(p.contains("password"), "prompt was: {p}");
            Some("hunting42".into())
        }, &no_abort)
        .unwrap();
        assert!(out.contains("got:hunting42"), "output was: {out}");
        assert!(out.contains("[exit 0]"), "output was: {out}");

        // Cancelling at the prompt kills the command instead of hanging.
        let out = run_shell("sudo whatever", &cwd, 30, &|_p| None, &no_abort).unwrap();
        std::env::set_var("PATH", old_path);
        assert!(out.contains("password prompt cancelled"), "output was: {out}");
    }
}

pub(crate) fn middle_truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let head = n * 2 / 3;
    let tail = n / 3;
    let mut h = head;
    while !s.is_char_boundary(h) {
        h -= 1;
    }
    let mut t = s.len() - tail;
    while !s.is_char_boundary(t) {
        t += 1;
    }
    format!("{}\n… [{} bytes elided] …\n{}", &s[..h], t - h, &s[t..])
}

pub fn execute(
    name: &str,
    args: &Value,
    workspace: &PathBuf,
    ask_secret: &dyn Fn(&str) -> Option<String>,
    abort: &std::sync::atomic::AtomicBool,
) -> Result<ToolOutcome> {
    match name {
        "run_command" => {
            let cmd = args["command"].as_str().unwrap_or("");
            let cwd = match args.get("cwd").and_then(|c| c.as_str()) {
                Some(c) => crate::safety::resolve(workspace, c),
                None => workspace.clone(),
            };
            let to = args.get("timeout_secs").and_then(|t| t.as_u64()).unwrap_or(120).min(600);
            Ok(ToolOutcome::Feed(run_shell(cmd, &cwd, to, ask_secret, abort)?))
        }
        "read_file" => {
            let path = crate::safety::resolve(workspace, args["path"].as_str().unwrap_or(""));
            let content = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let offset = args.get("offset").and_then(|o| o.as_u64()).unwrap_or(1).max(1) as usize;
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(400) as usize;
            let mut out = String::new();
            for (i, line) in content.lines().enumerate() {
                let ln = i + 1;
                if ln < offset {
                    continue;
                }
                if out.lines().count() >= limit {
                    out.push_str(&format!("… ({} more lines)\n", content.lines().count() - ln + 1));
                    break;
                }
                out.push_str(&format!("{ln}| {line}\n"));
            }
            Ok(ToolOutcome::Feed(truncate(&out, 60_000)))
        }
        "write_file" => {
            let path = crate::safety::resolve(workspace, args["path"].as_str().unwrap_or(""));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, args["content"].as_str().unwrap_or(""))?;
            Ok(ToolOutcome::Feed(format!("wrote {} bytes to {}", args["content"].as_str().unwrap_or("").len(), path.display())))
        }
        "edit_file" => {
            let path = crate::safety::resolve(workspace, args["path"].as_str().unwrap_or(""));
            let old = args["old_string"].as_str().unwrap_or("");
            let new = args["new_string"].as_str().unwrap_or("");
            let content = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let count = content.matches(old).count();
            if count == 0 {
                return Ok(ToolOutcome::Feed("edit failed: old_string not found. Re-read the file.".into()));
            }
            if count > 1 {
                return Ok(ToolOutcome::Feed(format!("edit failed: old_string matches {count} times; make it unique.")));
            }
            std::fs::write(&path, content.replacen(old, new, 1))?;
            Ok(ToolOutcome::Feed(format!("edited {}", path.display())))
        }
        "list_dir" => {
            let path = crate::safety::resolve(workspace, args["path"].as_str().unwrap_or("."));
            let mut out = String::new();
            let mut entries: Vec<_> = std::fs::read_dir(&path)?.collect::<std::result::Result<Vec<_>, _>>()?;
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let md = e.metadata().ok();
                let name = e.file_name().to_string_lossy().into_owned();
                match md {
                    Some(m) if m.is_dir() => out.push_str(&format!("d  {name}/\n")),
                    Some(m) => out.push_str(&format!("-  {name}  ({} bytes)\n", m.len())),
                    None => out.push_str(&format!("?  {name}\n")),
                }
            }
            Ok(ToolOutcome::Feed(middle_truncate(&out, 20_000)))
        }
        "find_files" => {
            let root = crate::safety::resolve(workspace, args["root"].as_str().unwrap_or("."));
            let glob = args.get("name_glob").and_then(|g| g.as_str()).map(|g| g.to_lowercase());
            let cre = args.get("content_regex").and_then(|r| r.as_str()).map(|r| regex::RegexBuilder::new(r).case_insensitive(true).build()).transpose()?;
            let mut out = String::new();
            let mut hits = 0;
            'outer: for entry in WalkDir::new(&root).max_depth(12).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let p = entry.path();
                if let Some(g) = &glob {
                    let fname = entry.file_name().to_string_lossy().to_lowercase();
                    if !glob_match(g, &fname) {
                        continue;
                    }
                }
                if let Some(re) = &cre {
                    match std::fs::read_to_string(p) {
                        Ok(txt) => {
                            let mut shown = false;
                            for (i, line) in txt.lines().enumerate() {
                                if re.is_match(line) {
                                    out.push_str(&format!("{}:{}: {}\n", p.display(), i + 1, crate::providers::truncate(line, 200)));
                                    shown = true;
                                    hits += 1;
                                    if hits > 200 {
                                        break 'outer;
                                    }
                                }
                            }
                            if !shown && glob.is_none() {
                                continue;
                            }
                        }
                        Err(_) => continue,
                    }
                } else {
                    out.push_str(&format!("{}\n", p.display()));
                    hits += 1;
                    if hits > 500 {
                        break;
                    }
                }
            }
            if out.is_empty() {
                out = "no matches".into();
            }
            Ok(ToolOutcome::Feed(middle_truncate(&out, 20_000)))
        }
        "set_permissions" => {
            let path = crate::safety::resolve(workspace, args["path"].as_str().unwrap_or(""));
            let mut parts = Vec::new();
            if let Some(mode) = args.get("mode").and_then(|m| m.as_str()) {
                #[cfg(unix)]
                parts.push(format!("chmod {mode} '{}'", path.display()));
                #[cfg(windows)]
                parts.push(format!("icacls '{}' /grant Everyone:F", path.display()));
            }
            if let Some(owner) = args.get("owner").and_then(|o| o.as_str()) {
                #[cfg(unix)]
                parts.push(format!("chown '{owner}' '{}'", path.display()));
                #[cfg(windows)]
                parts.push(format!("takeown /f '{}'", path.display()));
            }
            if parts.is_empty() {
                return Ok(ToolOutcome::Feed("nothing to do: provide mode and/or owner".into()));
            }
            let joined = parts.join(" && ");
            Ok(ToolOutcome::Feed(run_shell(&joined, &path.parent().unwrap_or(workspace).to_path_buf(), 60, ask_secret, abort)?))
        }
        "update_plan" => {
            let mut steps = Vec::new();
            if let Some(arr) = args["steps"].as_array() {
                for s in arr {
                    let label = s["label"].as_str().unwrap_or("?").to_string();
                    let status = s["status"].as_str().unwrap_or("pending").to_string();
                    steps.push((label, status));
                }
            }
            Ok(ToolOutcome::Plan(steps))
        }
        "finish_goal" => Ok(ToolOutcome::GoalFinished {
            summary: args["summary"].as_str().unwrap_or("").to_string(),
            evidence: args["verification_evidence"].as_str().unwrap_or("").to_string(),
        }),
        "save_skill" => {
            let raw = args["name"].as_str().unwrap_or("");
            let name = crate::skills::sanitize(raw);
            if name.is_empty() {
                return Ok(ToolOutcome::Feed("skill name must contain letters/digits".into()));
            }
            let sk = crate::skills::Skill {
                name: name.clone(),
                description: args["description"].as_str().unwrap_or("").to_string(),
                instructions: args["instructions"].as_str().unwrap_or("").to_string(),
            };
            match crate::skills::save(&sk) {
                Ok(()) => Ok(ToolOutcome::Feed(format!("skill '{name}' saved to penguin's library — it is now available in every future session"))),
                Err(e) => Ok(ToolOutcome::Feed(format!("failed to save skill: {e}"))),
            }
        }
        "load_skill" => {
            let name = crate::skills::sanitize(args["name"].as_str().unwrap_or(""));
            match crate::skills::load(&name) {
                Some(sk) => Ok(ToolOutcome::Feed(format!("# skill: {}\n{}\n\n{}", sk.name, sk.description, sk.instructions))),
                None => {
                    let avail: Vec<String> = crate::skills::list().into_iter().map(|s| s.name).collect();
                    Ok(ToolOutcome::Feed(format!("no skill '{name}'. available: {}", if avail.is_empty() { "(none)".into() } else { avail.join(", ")})))
                }
            }
        }
        "list_skills" => {
            let skills = crate::skills::list();
            if skills.is_empty() {
                return Ok(ToolOutcome::Feed("skill library is empty".into()));
            }
            let out: Vec<String> = skills.iter().map(|s| format!("- {}: {}", s.name, s.description)).collect();
            Ok(ToolOutcome::Feed(out.join("\n")))
        }
        other => Ok(ToolOutcome::Feed(format!("unknown tool: {other}"))),
    }
}

/// Simple glob (* and ?) matcher.
pub fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => (0..=t.len()).any(|i| m(&p[1..], &t[i..])),
            '?' => !t.is_empty() && m(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
        }
    }
    m(&p, &t)
}
