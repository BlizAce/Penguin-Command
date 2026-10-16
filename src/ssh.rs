//! SSH ride-along — when the user's shell pane holds an active
//! `ssh user@host` session, the agent can attach to that same computer:
//! commands and file tools then run there, in the folder the terminal is
//! sitting in. The channel is a fresh non-interactive `ssh` per call with
//! BatchMode (key auth only — no password prompts can leak into the TUI);
//! if that doesn't authenticate, attach fails cleanly and says so.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The host + folder the agent is currently riding along in.
#[derive(Clone, Debug)]
pub struct Remote {
    pub host: String,
    pub path: String,
}

impl Remote {
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn ssh_cmd(host: &str) -> Command {
    let mut c = Command::new("ssh");
    c.args([
        "-o", "BatchMode=yes",
        "-o", "ConnectTimeout=6",
        "-o", "ServerAliveInterval=15",
        "-o", "ClearAllForwardings=yes",
        host,
    ]);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c
}

/// Run `remote_cmd` on `host`, returning (exit code, stdout+stderr).
fn ssh_run(host: &str, remote_cmd: &str, timeout_secs: u64) -> Result<(i32, String)> {
    let mut child = ssh_cmd(host)
        .arg(remote_cmd)
        .spawn()
        .context("spawning ssh failed — is openssh installed?")?;
    // close stdin immediately: BatchMode never needs it
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                bail!("ssh timed out after {timeout_secs}s — is {host} reachable?");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let mut out = String::new();
    child.stdout.take().map(|mut s| s.read_to_string(&mut out));
    let mut err = String::new();
    child.stderr.take().map(|mut s| s.read_to_string(&mut err));
    if !err.trim().is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&err);
    }
    Ok((status.code().unwrap_or(-1), out))
}

/// Verify key auth works and resolve the starting folder. `path` = Some(p)
/// requires it to exist remotely; None defaults to the login directory.
pub fn probe(host: &str, path: Option<&str>) -> Result<Remote> {
    let remote_cmd = match path {
        Some(p) => format!("cd {} 2>/dev/null && pwd", sh_quote(p)),
        None => "pwd".into(),
    };
    let (code, out) = ssh_run(host, &remote_cmd, 15)?;
    if code != 0 {
        let hint = if out.to_lowercase().contains("permission denied") || out.to_lowercase().contains("host key") {
            " (BatchMode auth failed — set up ssh key access to this host first)"
        } else if path.is_some() {
            " (folder not found on the remote host)"
        } else {
            ""
        };
        bail!(
            "ssh probe failed (exit {code}):{} {}",
            hint,
            out.trim().lines().next().unwrap_or("")
        );
    }
    let path = out.trim().lines().last().unwrap_or("/").to_string();
    Ok(Remote { host: host.to_string(), path })
}

/// Run a shell command inside the ride-along folder. Mirrors run_shell's
/// shape: stdout+stderr merged, `[exit N]` tail on failure.
pub fn run(rm: &Remote, cmd: &str, timeout_secs: u64, abort: &AtomicBool) -> Result<String> {
    if abort.load(Ordering::Relaxed) {
        bail!("aborted");
    }
    let full = format!(
        "cd {} 2>/dev/null || {{ echo 'penguin: remote folder is gone: {}'; exit 1; }}; {}",
        sh_quote(&rm.path),
        sh_quote(&rm.path),
        cmd
    );
    let (code, out) = ssh_run(&rm.host, &full, timeout_secs)?;
    let mut text = out;
    if code != 0 {
        text.push_str(&format!("\n[exit {code}]"));
    }
    Ok(text)
}

pub fn read_file(rm: &Remote, path: &str) -> Result<String> {
    let (code, out) = ssh_run(&rm.host, &format!("cat -- {}", sh_quote(path)), 30)?;
    if code != 0 {
        bail!("read failed (exit {code}): {}", out.trim());
    }
    Ok(out)
}

/// Write a whole file remotely: mkdir the parent, stream base64 through
/// stdin so any content bytes survive quoting untouched.
pub fn write_file(rm: &Remote, path: &str, content: &str) -> Result<()> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(content);
    let parent = path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_else(|| ".".into());
    let (code, out) = {
        let mut child = ssh_cmd(&rm.host)
            .arg(format!(
                "mkdir -p -- {} 2>/dev/null; base64 -d > {} && test -s {} -o -z \"$(cat {})\"",
                sh_quote(&parent),
                sh_quote(path),
                sh_quote(path),
                sh_quote(path)
            ))
            .spawn()
            .context("spawning ssh failed")?;
        child.stdin.take().unwrap().write_all(b64.as_bytes())?;
        let status = child.wait_with_output()?;
        (status.status.code().unwrap_or(-1), String::from_utf8_lossy(&status.stderr).to_string())
    };
    if code != 0 {
        bail!("write failed (exit {code}): {}", out.trim());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Detection: find the `ssh user@host` client running under the shell pane.
// ---------------------------------------------------------------------------

/// Walk descendants of `root_pid`; return the target (user@host or host) of
/// the first live ssh client found among them.
pub fn detect_target(root_pid: u32) -> Option<String> {
    let mut queue = vec![root_pid];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = queue.pop() {
        for child in children_of(pid) {
            if !seen.insert(child) {
                continue;
            }
            if let Some(t) = ssh_target_of(child) {
                return Some(t);
            }
            queue.push(child);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn children_of(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(p): Option<u32> = name.to_str().and_then(|s| s.parse().ok()) else { continue };
        let Ok(stat) = std::fs::read(format!("/proc/{p}/stat")) else { continue };
        // ppid is the 4th field, after "comm" which may contain spaces in parens
        let Some(close) = stat.iter().rposition(|&b| b == b')') else { continue };
        let fields: Vec<&[u8]> = stat[close + 2..].split(|&b| b == b' ').collect();
        let ppid = fields
            .get(1)
            .and_then(|s| std::str::from_utf8(s).ok())
            .and_then(|s| s.parse::<u32>().ok());
        if ppid == Some(pid) {
            out.push(p);
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn ssh_target_of(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<&str> = raw.split(|&b| b == 0).map(|s| std::str::from_utf8(s).unwrap_or("")).filter(|s| !s.is_empty()).collect();
    parse_ssh_args(&args)
}

#[cfg(not(target_os = "linux"))]
fn children_of(pid: u32) -> Vec<u32> {
    let out = Command::new("ps").args(["-axo", "pid=,ppid="]).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let p: u32 = it.next()?.parse().ok()?;
            let pp: u32 = it.next()?.parse().ok()?;
            (pp == pid).then_some(p)
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn ssh_target_of(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "args="]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let args: Vec<&str> = shlexish(&line);
    parse_ssh_args(&args)
}

/// Minimal argv split for `ps args=` output (handles simple quoting).
#[cfg(not(target_os = "linux"))]
fn shlexish(line: &str) -> Vec<&str> {
    // ps already splits on spaces; quoted args are rare in ssh invocations —
    // a plain split is good enough for detection purposes.
    line.split_whitespace().collect()
}

/// Given argv like ["ssh", "-p", "2222", "user@host", "ls"], return the first
/// non-flag operand: the target. Skips known value-taking options.
fn parse_ssh_args(args: &[&str]) -> Option<String> {
    let exe = args.first()?;
    if exe.rsplit(['/', '\\']).next()? != "ssh" {
        return None;
    }
    let value_opts = ['b', 'c', 'D', 'F', 'i', 'I', 'J', 'L', 'l', 'o', 'O', 'p', 'Q', 'q', 'R', 'S', 'W', 'w'];
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        if *a == "--" {
            return it.next().map(|s| s.to_string());
        }
        if let Some(f) = a.strip_prefix('-') {
            let flag = f.chars().next()?;
            if value_opts.contains(&flag) && f.len() == 1 {
                it.next(); // skip the option's value
            }
            continue;
        }
        // first operand is the target (a bare host or user@host)
        return Some(a.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_flagged_targets() {
        assert_eq!(parse_ssh_args(&["ssh", "server"]).as_deref(), Some("server"));
        assert_eq!(parse_ssh_args(&["ssh", "user@example.org"]).as_deref(), Some("user@example.org"));
        assert_eq!(parse_ssh_args(&["ssh", "-p", "2222", "-i", "~/.ssh/k", "u@h"].as_ref()).as_deref(), Some("u@h"));
        assert_eq!(parse_ssh_args(&["/usr/bin/ssh", "-o", "X=1", "host", "ls"].as_ref()).as_deref(), Some("host"));
        // not an ssh client at all
        assert_eq!(parse_ssh_args(&["bash", "-lc", "x"]), None);
    }

    #[test]
    fn quotes_shell_words() {
        assert_eq!(sh_quote("/home/u/it's"), r#"'/home/u/it'\''s'"#);
    }
}
