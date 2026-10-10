pub mod render;

use crate::agent::{AgentCommand, AgentEnv, AgentEvent};
use crate::config::Config;
use crate::providers::Role;
use crate::safety::Outcome;
use crate::session;
use crate::term::input::{encode_key, encode_paste, parse_chord};
use crate::term::Emulator;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use portable_pty::{native_pty_system, MasterPty, PtySize};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(PartialEq, Clone, Copy)]
pub enum Mode {
    Shell,
    Agent,
}

#[derive(Clone)]
pub enum Entry {
    Art(Vec<String>),
    User(String),
    Assistant(String),
    Tool { name: String, args: String, status: String, output: Option<String>, started: std::time::Instant },
    Info(String),
    Error(String),
    Goal { summary: String, evidence: String },
    /// Unified diff of an agent file edit, rendered inline automatically.
    Diff { path: String, added: usize, removed: usize, text: String },
    /// An ask_user exchange: the question and how the user answered
    /// (Some = picked or typed answer, None = dismissed).
    Question { question: String, answer: Option<String> },
}

pub struct PermModal {
    pub title: String,
    pub detail: String,
    pub tx: Sender<Outcome>,
    pub sel: usize,
}

/// Hidden password entry for child commands (e.g. sudo). The buffer is only
/// ever moved into the waiting tool — never rendered or logged.
pub struct SecretModal {
    pub prompt: String,
    pub buf: String,
    pub tx: Sender<Option<String>>,
}

/// Selectable question from the agent's ask_user tool. The last list entry is
/// always "type your own answer"; selecting it switches to free-text editing.
pub struct QuestionModal {
    pub question: String,
    pub options: Vec<String>,
    pub tx: Sender<Option<String>>,
    /// 0..options.len() = an option; == options.len() = the custom-answer row.
    pub sel: usize,
    /// True while typing a custom answer (keys edit `buf` instead of the list).
    pub typing: bool,
    pub buf: String,
}

pub struct Options {
    pub shell: Option<String>,
    pub workspace: Option<PathBuf>,
    pub prompt: Option<String>,
    pub cont_last: bool,
    pub yolo: bool,
}

pub struct App {
    pub cfg: Config,
    pub emu: Arc<Mutex<Emulator>>,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    _master: Box<dyn MasterPty + Send>,
    exit_rx: Receiver<u32>,
    pub mode: Mode,
    toggle: (KeyCode, KeyModifiers),
    agent_tx: Option<Sender<AgentCommand>>,
    agent_abort: std::sync::Arc<AtomicBool>,
    agent_ev: Option<Receiver<AgentEvent>>,
    pub env: Arc<Mutex<AgentEnv>>,
    pub workspace: PathBuf,
    pub transcript: Vec<Entry>,
    pub streaming: String,
    /// Live chain-of-thought from thinking models; ephemeral, never saved.
    pub reasoning: String,
    pub plan: Vec<(String, String)>,
    pub composer: String,
    pub comp_cur: usize,
    history: Vec<String>,
    hist_pos: usize,
    pub permission: Option<PermModal>,
    pub secret: Option<SecretModal>,
    pub question: Option<QuestionModal>,
    pub scroll: usize,
    pub agent_scroll: usize,
    pub tick: u32,
    pub busy: bool,
    pub running: bool,
    welcomed: bool,
    pub active_goal: Option<String>,
    pub goal_verified: bool,
    pub ctx_used: usize,
    pub ctx_window: usize,
    pub screensaving: bool,
    pub saver_scene: usize,
    last_input: std::time::Instant,
    /// When the current busy turn began (for the elapsed timer).
    pub busy_since: std::time::Instant,
    /// First answer token of the current turn; drives the tok/s meter.
    pub first_token_at: Option<std::time::Instant>,
    pub recv_chars: usize,
    /// Tool currently executing — shown live in the status bar while busy.
    pub activity: Option<String>,
    /// Tool calls performed this turn; reported by the end-of-turn summary.
    pub turn_tools: usize,
    /// Last submitted prompt, for /retry after a failed turn.
    last_prompt: Option<(String, bool)>,
    /// Instructions typed while the agent is mid-run. Shared with the agent
    /// thread, which injects them at iteration boundaries; whatever is left
    /// when a turn ends (aborts, plain chat) flushes one-per-turn from here.
    pub queued: crate::agent::InstructionQueue,
    /// Tab toggles full tool output vs the compact preview.
    pub expand_tools: bool,
}

impl App {
    fn send_prompt(&mut self, text: String, goal: bool) {
        if let Some(tx) = &self.agent_tx {
            self.transcript.push(Entry::User(text.clone()));
            self.streaming.clear();
            self.reasoning.clear();
            // Keep the plan visible while an unverified goal is still open —
            // ordinary follow-ups must not blank the sidebar mid-goal.
            if goal || self.active_goal.is_none() || self.goal_verified {
                self.plan.clear();
            }
            if goal {
                self.active_goal = Some(text.clone());
                self.goal_verified = false;
            }
            self.busy = true;
            self.agent_scroll = 0;
            self.busy_since = std::time::Instant::now();
            self.first_token_at = None;
            self.recv_chars = 0;
            self.activity = None;
            self.turn_tools = 0;
            self.last_prompt = Some((text.clone(), goal));
            let _ = tx.send(AgentCommand::Prompt { text, goal });
        } else {
            self.transcript.push(Entry::Error("agent unavailable".into()));
        }
    }

    /// /continue [note] — resume the unfinished goal without losing its plan.
    fn send_continue(&mut self, note: String) {
        if self.busy {
            self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
        } else if self.active_goal.is_none() || self.goal_verified {
            self.transcript.push(Entry::Error(
                "no unfinished goal to continue — start one with /goal <what to achieve>".into(),
            ));
        } else if let Some(tx) = &self.agent_tx {
            self.streaming.clear();
            self.reasoning.clear();
            self.busy = true;
            self.agent_scroll = 0;
            self.busy_since = std::time::Instant::now();
            self.first_token_at = None;
            self.recv_chars = 0;
            self.activity = None;
            self.turn_tools = 0;
            let label = if note.is_empty() {
                "↻ continuing goal…".to_string()
            } else {
                format!("↻ continuing goal — your guidance: {note}")
            };
            self.transcript.push(Entry::Info(label));
            let _ = tx.send(AgentCommand::ContinueGoal { note: (!note.is_empty()).then_some(note) });
        } else {
            self.transcript.push(Entry::Error("agent unavailable".into()));
        }
    }

    fn handle_slash(&mut self, line: &str) -> Result<()> {
        let mut it = line[1..].splitn(2, ' ');
        let cmd = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("").trim();
        match cmd {
            "" => {}
            "help" => self.transcript.push(Entry::Info(
                "/goal <text> autonomous plan+execute+verify · /continue [note] resume the unfinished goal · \
                 /queue <text> add instructions mid-run (/queue lists, /queue clear) · \
                 /limit off|on remove/restore run-length caps · \
                 /search <q> web search (sandboxed page reader) · \
                 /new fresh session · /sessions list · /resume [id] · /skills list saved skills · \
                 /compact compact context now · /retry resend last prompt · /effort cycle reasoning depth · \
                 /context usage · /yolo bypass approvals (dangerous) · /providers setup UI · /model [name] · \
                 /workspace <dir> · /screensaver · /clear · /quit".into(),
            )),
            "quit" | "exit" => self.running = false,
            "clear" => {
                self.transcript.clear();
                self.plan.clear();
                self.active_goal = None;
                self.goal_verified = false;
            }
            "goal" => {
                if rest.is_empty() {
                    self.transcript.push(Entry::Error("usage: /goal <what to achieve>".into()));
                } else if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                } else {
                    self.send_prompt(rest.to_string(), true);
                }
            }
            "continue" | "cont" => self.send_continue(rest.to_string()),
            "queue" => {
                if rest.eq_ignore_ascii_case("clear") {
                    let n = self.queued.lock().unwrap().len();
                    self.queued.lock().unwrap().clear();
                    self.transcript.push(Entry::Info(format!("🗑 cleared {n} queued instruction(s)")));
                } else if rest.is_empty() {
                    let q = self.queued.lock().unwrap();
                    if q.is_empty() {
                        self.transcript.push(Entry::Info(
                            "queue is empty — /queue <text> hands the agent an extra instruction mid-run".into(),
                        ));
                    } else {
                        for (i, t) in q.iter().enumerate() {
                            self.transcript.push(Entry::Info(format!("⏳ {}: {}", i + 1, crate::providers::truncate(t, 100))));
                        }
                        self.transcript.push(Entry::Info("/queue clear drops everything".into()));
                    }
                } else if self.busy {
                    self.enqueue_instruction(rest.to_string());
                } else {
                    // Idle: nothing to wait for — run it right away.
                    self.send_prompt(rest.to_string(), false);
                }
            }
            "search" => {
                if rest.is_empty() {
                    self.transcript.push(Entry::Error("usage: /search <what to look up>".into()));
                } else if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                } else {
                    // Sugar for the research flow: search → pick hits → read in
                    // the sandbox → report. Keeps the work (and any injection)
                    // inside the agent's tool loop instead of the UI thread.
                    self.send_prompt(format!(
                        "Look this up on the web: {rest}\nUse web_search, choose the most relevant \
                         hits, read the best 1–3 with web_read, and summarize what you learned with \
                         source URLs."
                    ), false);
                }
            }
            "limit" => {
                let next = match rest.to_lowercase().as_str() {
                    "" | "toggle" => !self.env.lock().unwrap().unlimited_run,
                    "off" | "unlimited" => true,
                    "on" | "restore" => false,
                    _ => {
                        self.transcript.push(Entry::Error(
                            "usage: /limit off|on — off lifts the iteration & continuation caps so runs \
                             never pause for /continue (esc still stops)".into(),
                        ));
                        return Ok(());
                    }
                };
                self.env.lock().unwrap().unlimited_run = next;
                self.cfg.agent.unlimited_run = next;
                let _ = self.cfg.save();
                if next {
                    self.transcript.push(Entry::Info(
                        "⚡ UNLIMITED RUN — iteration & continuation caps lifted; goals auto-continue until \
                         verified. A ∞ badge marks it in the status bar. Esc still stops, safety gates still \
                         apply. /limit on to restore.".into(),
                    ));
                } else {
                    self.transcript.push(Entry::Info(format!(
                        "✦ run limits restored — max_iterations {}, goal continuations {}",
                        self.cfg.agent.max_iterations.max(4),
                        self.cfg.agent.goal_continuations
                    )));
                }
            }
            "providers" | "provider" => self.setup_interactive()?,
            "new" => {
                if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                    return Ok(());
                }
                let id = session::new_id();
                self.env.lock().unwrap().session_id = id.clone();
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentCommand::Resume { messages: vec![], goal: None });
                }
                self.transcript.clear();
                self.plan.clear();
                self.active_goal = None;
                self.goal_verified = false;
                self.ctx_used = 0;
                self.streaming.clear();
                self.queued.lock().unwrap().clear();
                self.transcript.push(Entry::Info(format!("✦ new session · {id}")));
            }
            "sessions" | "history" => {
                let list = session::list();
                if list.is_empty() {
                    self.transcript.push(Entry::Info("no saved sessions yet".into()));
                } else {
                    for s in list.iter().take(10) {
                        self.transcript.push(Entry::Info(format!(
                            "{} · {} · {} · {} msgs · “{}”",
                            s.id,
                            session::fmt_ts(s.updated_ms),
                            if s.model.is_empty() { "—" } else { &s.model },
                            s.messages.len(),
                            s.title,
                        )));
                    }
                    self.transcript.push(Entry::Info("/resume <id> — no id resumes the latest".into()));
                }
            }
            "resume" => {
                if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                    return Ok(());
                }
                let found = if rest.is_empty() { session::latest() } else { session::load(rest) };
                match found {
                    Some(s) => self.resume_session(s),
                    None => self.transcript.push(Entry::Error(format!(
                        "no session to resume{}",
                        if rest.is_empty() { " (none saved)".into() } else { format!(": {rest}") }
                    ))),
                }
            }
            "skills" => {
                let skills = crate::skills::list();
                if skills.is_empty() {
                    self.transcript.push(Entry::Info("skill library is empty — the agent fills it after finishing goals".into()));
                } else {
                    for sk in skills {
                        self.transcript.push(Entry::Info(format!("◆ {}: {}", sk.name, sk.description)));
                    }
                }
            }
            "compact" => {
                if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                } else if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentCommand::Compact);
                    self.transcript.push(Entry::Info("compacting context…".into()));
                }
            }
            "retry" => {
                if self.busy {
                    self.transcript.push(Entry::Info("agent is still working — esc to stop it first".into()));
                } else if let Some((text, goal)) = self.last_prompt.clone() {
                    self.send_prompt(text, goal);
                } else {
                    self.transcript.push(Entry::Error("nothing to retry yet".into()));
                }
            }
            "context" | "ctx" => {
                let pct = if self.ctx_window > 0 { self.ctx_used * 100 / self.ctx_window } else { 0 };
                self.transcript.push(Entry::Info(format!(
                    "context: ~{} / ~{} tokens ({pct}%) · auto-compact at {}%",
                    self.ctx_used, self.ctx_window, self.cfg.agent.compact_at_percent
                )));
            }
            "effort" => {
                let mut e = self.env.lock().unwrap();
                let next = if rest.is_empty() {
                    // bare /effort cycles low → medium → high → xhigh → low…
                    let cur = e.reasoning_effort.clone().unwrap_or_else(|| "xhigh".into());
                    let order = crate::config::REASONING_EFFORTS;
                    let idx = order.iter().position(|x| *x == cur).unwrap_or(order.len() - 1);
                    Some(order[(idx + 1) % order.len()].to_string())
                } else if matches!(rest.to_lowercase().as_str(), "off" | "none") {
                    None
                } else {
                    match crate::config::normalize_effort(rest) {
                        Some(level) => Some(level),
                        None => {
                            drop(e);
                            self.transcript.push(Entry::Error(format!(
                                "unknown effort “{rest}” — use l · m · h · x (or /effort alone to cycle, /effort off to omit)"
                            )));
                            return Ok(());
                        }
                    }
                };
                e.reasoning_effort = next.clone();
                drop(e);
                self.cfg.agent.reasoning_effort = next.clone().or_else(|| Some("off".into()));
                let _ = self.cfg.save();
                self.transcript.push(Entry::Info(format!(
                    "✦ reasoning effort → {}{}",
                    next.unwrap_or_else(|| "off (server decides)".into()),
                    if rest.is_empty() { " — /effort again for the next level" } else { "" }
                )));
            }
            "yolo" => {
                let on = match rest.to_lowercase().as_str() {
                    "" | "on" => true,
                    "off" => false,
                    _ => {
                        self.transcript.push(Entry::Error("usage: /yolo [on|off]".into()));
                        return Ok(());
                    }
                };
                self.env.lock().unwrap().yolo = on;
                self.cfg.agent.yolo = on;
                if on {
                    self.transcript.push(Entry::Info(
                        "⚠ YOLO ON — approvals are bypassed for this session. Catastrophic hard-blocks \
                         and sudo password prompts still apply. /yolo off to restore them.".into(),
                    ));
                } else {
                    self.transcript.push(Entry::Info("✦ yolo off — approval dialogs restored".into()));
                }
            }
            "model" => {
                let mut e = self.env.lock().unwrap();
                if rest.is_empty() {
                    let list = e.provider.as_ref().map(|p| p.models.join(", ")).unwrap_or_default();
                    drop(e);
                    self.transcript.push(Entry::Info(format!("models: {}", if list.is_empty() { "(none fetched — /providers)" } else { &list })));
                } else if let Some(p) = e.provider.as_mut() {
                    p.models.push(rest.to_string());
                    p.models.sort();
                    p.models.dedup();
                    p.default_model = Some(rest.to_string());
                    let updated = p.clone();
                    e.model = rest.to_string();
                    drop(e);
                    // Persist so the switch survives a restart, like /effort.
                    if let Some(pc) = self.cfg.providers.iter_mut().find(|x| x.name == updated.name) {
                        *pc = updated;
                        let _ = self.cfg.save();
                    }
                    self.transcript.push(Entry::Info(format!("model → {rest} (saved)")));
                } else {
                    drop(e);
                    self.transcript.push(Entry::Error("no provider configured — /providers".into()));
                }
            }
            "workspace" | "ws" => {
                if rest.is_empty() {
                    self.transcript.push(Entry::Info(format!("workspace: {}", self.workspace.display())));
                } else {
                    let p = crate::safety::resolve(&self.workspace, rest);
                    if p.is_dir() {
                        let label = format!("workspace → {}", p.display());
                        self.workspace = p.clone();
                        self.env.lock().unwrap().workspace = p;
                        self.transcript.push(Entry::Info(label));
                    } else {
                        self.transcript.push(Entry::Error(format!("not a directory: {}", p.display())));
                    }
                }
            }
            "screensaver" | "saver" => self.force_screensave(),
            other => self.transcript.push(Entry::Error(format!("unknown command /{other} — try /help"))),
        }
        Ok(())
    }

    fn resume_session(&mut self, s: session::Session) {
        let n = s.messages.len();
        self.env.lock().unwrap().session_id = s.id.clone();
        self.transcript.clear();
        self.streaming.clear();
        self.queued.lock().unwrap().clear();
        // Restore the goal + plan exactly as saved; sessions written before
        // those fields existed fall back to scanning for the GOAL header.
        match &s.goal {
            Some(g) => {
                self.active_goal = Some(g.text.clone());
                self.plan = g.plan.clone();
                self.goal_verified = g.verified;
            }
            None => {
                self.plan.clear();
                self.goal_verified = false;
                self.active_goal = s.messages.iter().rev().find_map(|m| {
                    (m.role == Role::User)
                        .then(|| m.text.strip_prefix("GOAL (autonomous mode — plan, execute, verify): ").map(|g| g.to_string()))
                        .flatten()
                });
            }
        }
        for m in &s.messages {
            match m.role {
                Role::User => self.transcript.push(Entry::User(m.text.clone())),
                Role::Assistant if !m.text.trim().is_empty() => {
                    self.transcript.push(Entry::Assistant(m.text.clone()));
                }
                _ => {}
            }
        }
        let goal = s.goal.clone();
        if let Some(tx) = &self.agent_tx {
            let _ = tx.send(AgentCommand::Resume { messages: s.messages, goal });
        }
        self.transcript.push(Entry::Info(format!("↺ resumed session {} · {n} messages", s.id)));
        if self.active_goal.is_some() && !self.goal_verified {
            self.transcript.push(Entry::Info("goal unfinished — /continue picks it back up with its plan".into()));
        }
    }

    fn setup_interactive(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen, Show)?;
        let new_cfg = crate::setup_tui::run_setup(&self.cfg)?;
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        self.apply_config(new_cfg);
        self.transcript.push(Entry::Info("provider config saved".into()));
        Ok(())
    }

    pub fn apply_config(&mut self, cfg: Config) {
        let mut e = self.env.lock().unwrap();
        if let Some(p) = cfg.active() {
            e.provider = Some(p.clone());
            if p.default_model.is_some() {
                e.model = p.default_model.clone().unwrap_or_default();
            }
        } else {
            e.provider = None;
            e.model.clear();
        }
        drop(e);
        self.ctx_window = provider_window(&cfg);
        self.cfg = cfg;
    }

    fn drain_agent_events(&mut self) {
        let mut events = Vec::new();
        let mut dead = false;
        if let Some(rx) = &self.agent_ev {
            loop {
                match rx.try_recv() {
                    Ok(ev) => events.push(ev),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                }
            }
        }
        for ev in events {
            match ev {
                AgentEvent::TextDelta(t) => {
                    self.recv_chars += t.chars().count();
                    if self.first_token_at.is_none() {
                        self.first_token_at = Some(std::time::Instant::now());
                    }
                    self.streaming.push_str(&t);
                }
                AgentEvent::ReasoningDelta(t) => self.reasoning.push_str(&t),
                AgentEvent::TextDone => {
                    if !self.streaming.is_empty() {
                        let t = std::mem::take(&mut self.streaming);
                        self.transcript.push(Entry::Assistant(t));
                    }
                }
                AgentEvent::ToolStarted { name, args, .. } => {
                    self.streaming.clear();
                    self.reasoning.clear();
                    self.activity = Some(name.clone());
                    self.transcript.push(Entry::Tool {
                        name,
                        args,
                        status: "running".into(),
                        output: None,
                        started: std::time::Instant::now(),
                    });
                }
                AgentEvent::ToolFinished { ok, output, .. } => {
                    self.turn_tools += 1;
                    self.activity = None;
                    if let Some(Entry::Tool { status, output: slot, .. }) =
                        self.transcript.iter_mut().rev().find(|e| matches!(e, Entry::Tool { status, .. } if status == "running"))
                    {
                        *status = if ok { "done".into() } else { "failed".into() };
                        *slot = Some(short_output(&output));
                    }
                }
                AgentEvent::Diff { path, added, removed, text } => {
                    self.transcript.push(Entry::Diff { path, added, removed, text });
                }
                AgentEvent::Plan(steps) => self.plan = steps,
                AgentEvent::NeedPermission { title, detail, tx, .. } => {
                    self.permission = Some(PermModal { title, detail, tx, sel: 0 });
                }
                AgentEvent::NeedSecret { prompt, tx } => {
                    if let Some(old) = self.secret.take() {
                        let _ = old.tx.send(None);
                    }
                    self.secret = Some(SecretModal { prompt, buf: String::new(), tx });
                }
                AgentEvent::NeedQuestion { question, options, tx } => {
                    if let Some(old) = self.question.take() {
                        let _ = old.tx.send(None);
                    }
                    self.question = Some(QuestionModal { question, options, tx, sel: 0, typing: false, buf: String::new() });
                }
                AgentEvent::Info(s) => self.transcript.push(Entry::Info(s)),
                AgentEvent::ErrorText(s) => self.transcript.push(Entry::Error(s)),
                AgentEvent::GoalFinished { summary, evidence } => {
                    self.goal_verified = true;
                    self.plan.iter_mut().for_each(|(_, s)| *s = "done".into());
                    self.transcript.push(Entry::Art(crate::penguin::victory_lines()));
                    self.transcript.push(Entry::Goal { summary, evidence });
                }
                AgentEvent::Context { used, window } => {
                    self.ctx_used = used;
                    self.ctx_window = window;
                }
                AgentEvent::Done => {
                    self.busy = false;
                    self.reasoning.clear();
                    self.activity = None;
                    for e in &mut self.transcript {
                        if let Entry::Tool { status, .. } = e {
                            if status == "running" {
                                *status = "done".into();
                            }
                        }
                    }
                    // Turn summary: a visible closing beat so the screen never
                    // just silently stops moving.
                    let secs = self.busy_since.elapsed().as_secs();
                    let toks = self.recv_chars / 4;
                    self.transcript.push(Entry::Info(format!(
                        "✻ turn done · {secs}s · {} tool call{} · ~{} tok out",
                        self.turn_tools,
                        if self.turn_tools == 1 { "" } else { "s" },
                        toks
                    )));
                    // give a fresh idle window after the agent finishes working
                    self.last_input = std::time::Instant::now();
                    // Anything still queued (typed after the agent's last
                    // mid-run drain, or left by an abort) goes out now.
                    let next = {
                        let mut q = self.queued.lock().unwrap();
                        if q.is_empty() { None } else { Some(q.remove(0)) }
                    };
                    if let Some(next) = next {
                        self.transcript.push(Entry::Info("⏳ sending queued prompt…".into()));
                        self.send_prompt(next, false);
                    }
                }
            }
        }
        if dead && self.busy {
            self.busy = false;
            self.streaming.clear();
            for e in &mut self.transcript {
                if let Entry::Tool { status, .. } = e {
                    if status == "running" {
                        *status = "failed".into();
                    }
                }
            }
            self.transcript.push(Entry::Error("agent thread stopped unexpectedly — send again to retry".into()));
        }
    }

    fn answer_permission(&mut self, outcome: Outcome) {
        if let Some(m) = self.permission.take() {
            let label = outcome.label().to_lowercase();
            let _ = m.tx.send(outcome);
            self.transcript.push(Entry::Info(format!("permission: {label}")));
        }
    }

    /// Resolve the open ask_user modal: Some(answer) was picked/typed,
    /// None means the user dismissed it.
    fn answer_question(&mut self, answer: Option<String>) {
        if let Some(m) = self.question.take() {
            let _ = m.tx.send(answer.clone());
            self.transcript.push(Entry::Question { question: m.question, answer });
        }
    }

    pub fn wake(&mut self) {
        self.screensaving = false;
        self.last_input = std::time::Instant::now();
    }

    /// Omarchy-style idle screensaver: after [ui] idle_secs without input,
    /// animated penguins take over the viewport until any key is pressed.
    fn maybe_screensave(&mut self) {
        if self.screensaving || self.busy || self.permission.is_some() || self.secret.is_some() || self.question.is_some() {
            return;
        }
        let idle = self.cfg.ui.idle_secs.unwrap_or(150);
        if idle > 0 && self.last_input.elapsed() >= std::time::Duration::from_secs(idle) {
            self.saver_scene = crate::penguin::pick_scene(self.tick as u64 ^ (std::process::id() as u64) << 17);
            self.screensaving = true;
        }
    }

    pub fn force_screensave(&mut self) {
        self.saver_scene = crate::penguin::pick_scene(self.tick as u64 ^ 0x5EED);
        self.screensaving = true;
    }

    /// Unified window: the agent shares the full viewport with the shell,
    /// so the PTY always keeps every row below the status bar.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let shell_rows = rows.saturating_sub(1).max(3);
        let _ = self._master.resize(PtySize { rows: shell_rows, cols, pixel_width: 0, pixel_height: 0 });
        self.emu.lock().unwrap().term.resize(shell_rows, cols);
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode == Mode::Agent && !self.welcomed {
            self.welcomed = true;
            self.transcript.push(Entry::Art(crate::penguin::welcome_lines()));
        }
        if mode == Mode::Agent {
            let ws = self.emu.lock().unwrap().term.cwd.clone();
            if let Some(mut w) = ws {
                while !w.is_dir() {
                    if !w.pop() {
                        break;
                    }
                }
                if w.is_dir() {
                    self.workspace = w;
                }
            }
            self.env.lock().unwrap().workspace = self.workspace.clone();
            self.transcript.push(Entry::Info(format!("agent mode · workspace: {}", self.workspace.display())));
        }
    }

    fn composer_submit(&mut self) -> Result<()> {
        let text = self.composer.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        // Basic slash autocomplete: a unique prefix completes and sends; an
        // ambiguous one fills the common prefix so typing can continue.
        if let Some(comp) = render::complete_slash(&text) {
            match comp {
                render::SlashCompletion::Send(full) => {
                    self.history.push(full.clone());
                    self.hist_pos = self.history.len();
                    self.composer.clear();
                    self.comp_cur = 0;
                    return self.handle_slash(&full);
                }
                render::SlashCompletion::Fill(prefix) => {
                    self.composer = prefix;
                    self.comp_cur = self.composer.chars().count();
                    return Ok(());
                }
            }
        }
        self.history.push(text.clone());
        self.hist_pos = self.history.len();
        self.composer.clear();
        self.comp_cur = 0;
        if text.starts_with('/') {
            self.handle_slash(&text)?;
        } else if self.busy {
            // "enter queue" from the key legend: the agent injects it at its
            // next step; leftovers (abort, plain chat) flush on Done.
            let n = {
                let mut q = self.queued.lock().unwrap();
                q.push(text);
                q.len()
            };
            self.transcript.push(Entry::Info(format!("⏳ queued ({n}) — injected at the agent's next step")));
        } else {
            self.send_prompt(text, false);
        }
        Ok(())
    }

    fn enqueue_instruction(&mut self, text: String) {
        let n = {
            let mut q = self.queued.lock().unwrap();
            q.push(text);
            q.len()
        };
        self.transcript.push(Entry::Info(format!(
            "⏳ queued ({n}) — injected at the agent's next step"
        )));
    }
}

fn provider_window(cfg: &Config) -> usize {
    cfg.active().and_then(|p| p.context_window).unwrap_or(32_768)
}

fn short_output(o: &str) -> String {
    let lines: Vec<&str> = o.lines().collect();
    if lines.len() <= 6 {
        o.to_string()
    } else {
        format!("{}\n… {} more lines\n{}", lines[..3].join("\n"), lines.len() - 5, lines[lines.len() - 2..].join("\n"))
    }
}

pub fn run(opts: Options) -> Result<()> {
    let mut cfg = Config::load().unwrap_or_default();
    cfg.normalize();
    if opts.yolo {
        cfg.agent.yolo = true;
    }

    let pty_system = native_pty_system();
    let (init_cols, init_rows) = crossterm::terminal::size()?;
    let shell_rows = init_rows.saturating_sub(1).max(4);
    let pair = pty_system.openpty(PtySize { rows: shell_rows, cols: init_cols, pixel_width: 0, pixel_height: 0 })?;

    let cmd = crate::integration::build_shell_command(opts.shell.as_deref())?;
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer()?));

    let emu = Arc::new(Mutex::new(Emulator::new(shell_rows, init_cols)));
    let emu2 = emu.clone();
    let writer2 = writer.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let reply = {
                        let mut g = emu2.lock().unwrap();
                        g.feed(&buf[..n]);
                        std::mem::take(&mut g.term.pending_output)
                    };
                    if !reply.is_empty() {
                        let _ = writer2.lock().unwrap().write_all(&reply);
                    }
                }
            }
        }
    });

    let mut killer = child.clone_killer();
    let (exit_tx, exit_rx) = channel::<u32>();
    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.map(|s| s.exit_code()).unwrap_or(0);
        let _ = exit_tx.send(code);
    });

    let workspace = opts.workspace.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider = cfg.active().cloned();
    let model = provider.as_ref().and_then(|p| p.default_model.clone()).unwrap_or_default();
    let env = Arc::new(Mutex::new(AgentEnv {
        provider,
        model,
        workspace: workspace.clone(),
        shell_cwd: std::env::current_dir().ok(),
        session_id: session::new_id(),
        reasoning_effort: cfg.agent.reasoning_effort(),
        yolo: cfg.agent.yolo,
        unlimited_run: cfg.agent.unlimited_run,
    }));
    let queue: crate::agent::InstructionQueue = Arc::new(Mutex::new(Vec::new()));
    let (handle, agent_ev) = crate::agent::spawn_agent(env.clone(), cfg.clone(), queue.clone());

    let mut app = App {
        cfg: cfg.clone(),
        emu,
        writer,
        _master: pair.master,
        exit_rx,
        mode: Mode::Shell,
        toggle: parse_chord(&cfg.ui.toggle_key),
        agent_tx: Some(handle.tx.clone()),
        agent_abort: handle.abort,
        agent_ev: Some(agent_ev),
        env,
        workspace,
        transcript: Vec::new(),
        streaming: String::new(),
        reasoning: String::new(),
        plan: Vec::new(),
        composer: String::new(),
        comp_cur: 0,
        history: Vec::new(),
        hist_pos: 0,
        permission: None,
        secret: None,
        question: None,
        scroll: 0,
        agent_scroll: 0,
        tick: 0,
        busy: false,
        running: true,
        welcomed: false,
        active_goal: None,
        goal_verified: false,
        ctx_used: 0,
        ctx_window: provider_window(&cfg),
        screensaving: false,
        saver_scene: crate::penguin::pick_scene(std::process::id() as u64),
        last_input: std::time::Instant::now(),
        busy_since: std::time::Instant::now(),
        first_token_at: None,
        recv_chars: 0,
        activity: None,
        turn_tools: 0,
        last_prompt: None,
        queued: queue,
        expand_tools: false,
    };

    if opts.cont_last {
        match session::latest() {
            Some(s) => app.resume_session(s),
            None => app.transcript.push(Entry::Info("no previous session to continue".into())),
        }
    }

    if let Some(p) = opts.prompt {
        app.mode = Mode::Agent;
        app.welcomed = true;
        app.send_prompt(p, false);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;
    // Mouse capture: wheel scrolls the agent transcript / shell scrollback.
    // (The embedded PTY never receives mouse bytes either way — inner apps
    // were never wired for it — so nothing regresses.)
    let _ = execute!(stdout, EnableMouseCapture);
    // Bracketed paste on OUR stdin: without it a multi-line paste arrives as
    // a torrent of individual keystrokes and every newline inside fires its
    // own Enter (each pasted line becomes a separate prompt/command). With it
    // the terminal delivers one Event::Paste for the whole block.
    let _ = execute!(stdout, EnableBracketedPaste);
    // kitty keyboard protocol (disambiguate flag): lets us tell Shift+Enter
    // from plain Enter. Terminals without support ignore this sequence entirely.
    let _ = stdout.write_all(b"\x1b[>1u");
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = main_loop(&mut app, &mut terminal, &mut killer);

    disable_raw_mode()?;
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x1b[<u");
    let _ = execute!(stdout, DisableMouseCapture, DisableBracketedPaste);
    execute!(stdout, Show, LeaveAlternateScreen)?;
    let _ = killer.kill();
    if let Some(tx) = &app.agent_tx {
        let _ = tx.send(AgentCommand::Quit);
    }
    result
}

fn main_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    killer: &mut Box<dyn portable_pty::ChildKiller + Send + Sync>,
) -> Result<()> {
    while app.running {
        if app.exit_rx.try_recv().is_ok() {
            break;
        }
        app.drain_agent_events();
        app.tick = app.tick.wrapping_add(1);
        app.maybe_screensave();
        terminal.draw(|f| render::render(app, f))?;

        let timeout = std::time::Duration::from_millis(if app.busy { 30 } else { 60 });
        if !event::poll(timeout)? {
            continue;
        }
        // Process every event already in the queue before drawing again: a
        // large paste or key burst used to cost one full redraw per event and
        // could wedge the UI for minutes.
        let mut quit = false;
        loop {
            if handle_event(app, event::read()?, killer)? {
                quit = true;
                break;
            }
            if !event::poll(std::time::Duration::ZERO)? {
                break;
            }
        }
        if quit {
            break;
        }
    }
    Ok(())
}

/// One input event. Returns true when penguin should exit (Ctrl+Q).
fn handle_event(
    app: &mut App,
    ev: Event,
    killer: &mut Box<dyn portable_pty::ChildKiller + Send + Sync>,
) -> Result<bool> {
    match ev {
        Event::Resize(cols, rows) => {
            app.wake();
            app.resize(rows, cols);
        }
        Event::Mouse(m) => {
            use crossterm::event::MouseEventKind;
            // Wheel scrolls the agent transcript / shell scrollback.
            let delta = match m.kind {
                MouseEventKind::ScrollUp => 3i64,
                MouseEventKind::ScrollDown => -3,
                _ => 0,
            };
            if delta != 0 {
                if app.mode == Mode::Agent {
                    app.agent_scroll = (app.agent_scroll as i64 + delta).max(0) as usize;
                } else {
                    let max = app.emu.lock().unwrap().term.history_len();
                    app.scroll = ((app.scroll as i64 + delta).max(0) as usize).min(max);
                }
            }
            app.wake();
        }
        Event::Paste(text) => {
            if app.screensaving {
                app.wake();
                return Ok(false);
            }
            app.wake();
            if app.secret.is_some() || app.permission.is_some() || app.question.is_some() {
                // A modal owns the keyboard: never smuggle pasted text into
                // the shell (or composer) behind a pending dialog.
                return Ok(false);
            }
            if app.mode == Mode::Shell {
                let bracketed = app.emu.lock().unwrap().term.bracketed_paste;
                let _ = app.writer.lock().unwrap().write_all(&encode_paste(&text, bracketed));
            } else {
                // Multi-line pastes land in the composer as one block: nothing
                // is submitted until the user presses Enter themselves.
                let (clean, dropped) = sanitize_paste(&text);
                if !clean.is_empty() {
                    app.composer.insert_str(app.comp_cur, &clean);
                    app.comp_cur += clean.chars().count();
                }
                if dropped > 0 {
                    app.transcript.push(Entry::Info(format!(
                        "⚠ paste capped at {} bytes ({dropped} chars dropped) — huge content belongs in a file the agent can read",
                        MAX_PASTE_BYTES
                    )));
                }
            }
        }
        Event::Key(k) => {
            if k.kind == KeyEventKind::Release {
                return Ok(false);
            }
            if k.modifiers.contains(KeyModifiers::CONTROL)
                && k.code == KeyCode::Char('q')
                && app.permission.is_none()
                && app.secret.is_none()
            {
                let _ = killer.kill();
                return Ok(true);
            }
            if app.screensaving {
                app.wake();
                return Ok(false);
            }
            // any keystroke counts as activity — without this the
            // screensaver interrupts people who type for a living
            app.wake();
            if app.secret.is_some() {
                handle_secret_key(app, &k);
                return Ok(false);
            }
            if app.question.is_some() {
                handle_question_key(app, &k);
                return Ok(false);
            }
            if app.permission.is_some() {
                handle_permission_key(app, &k);
                return Ok(false);
            }
            if k.code == app.toggle.0 && k.modifiers == app.toggle.1 {
                let (cols, rows) = crossterm::terminal::size()?;
                let new_mode = if app.mode == Mode::Shell { Mode::Agent } else { Mode::Shell };
                app.set_mode(new_mode);
                app.resize(rows, cols);
                return Ok(false);
            }
            match app.mode {
                Mode::Shell => handle_shell_key(app, &k),
                Mode::Agent => handle_agent_key(app, &k)?,
            }
        }
        _ => {}
    }
    Ok(false)
}

/// Largest paste accepted into the agent composer. Beyond this the tail is
/// dropped with a visible notice — megabytes in a one-line prompt box make
/// the UI (and the model prompt) useless; big content belongs in a file.
pub const MAX_PASTE_BYTES: usize = 65_536;

/// Make pasted text safe and sane for the composer: CRLF → LF, tabs to
/// spaces, control characters stripped (ESC sequences in particular must
/// never ride through the transcript into the prompt), tail capped.
/// Returns the clean text plus how many source chars were dropped.
fn sanitize_paste(text: &str) -> (String, usize) {
    let total = text.chars().count();
    let mut out = String::with_capacity(text.len().min(MAX_PASTE_BYTES + 16));
    let mut dropped = 0usize;
    let mut seen = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        seen += 1;
        if out.len() >= MAX_PASTE_BYTES {
            // current char and everything after it never make it in
            dropped += total - seen + 1;
            break;
        }
        match c {
            // a CR directly followed by LF is one newline, produced by the LF
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => dropped += 1,
            c => out.push(c),
        }
    }
    (out, dropped)
}

/// Hidden password entry: chars are buffered (never echoed), enter submits to
/// the waiting tool, esc cancels. The secret is never shown or logged.
fn handle_secret_key(app: &mut App, k: &event::KeyEvent) {
    match k.code {
        KeyCode::Char(c) if !c.is_control() && (k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT) => {
            if let Some(m) = app.secret.as_mut() {
                m.buf.push(c);
            }
        }
        KeyCode::Backspace => {
            if let Some(m) = app.secret.as_mut() {
                m.buf.pop();
            }
        }
        KeyCode::Enter => {
            if let Some(m) = app.secret.take() {
                let _ = m.tx.send(Some(m.buf));
                app.transcript.push(Entry::Info("🔑 password sent · hidden".into()));
            }
        }
        KeyCode::Esc => {
            if let Some(m) = app.secret.take() {
                let _ = m.tx.send(None);
                app.transcript.push(Entry::Info("password prompt cancelled".into()));
            }
        }
        _ => {}
    }
}

/// Selectable question from the agent's ask_user tool: ↑↓/1-9 pick an option,
/// the last row (or `c`) switches to typing a custom answer; enter submits,
/// esc dismisses (the agent is told it got no answer).
fn handle_question_key(app: &mut App, k: &event::KeyEvent) {
    let typing = app.question.as_ref().map(|m| m.typing).unwrap_or(false);
    if typing {
        match k.code {
            KeyCode::Char(c) if !c.is_control() => {
                if let Some(m) = app.question.as_mut() {
                    m.buf.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = app.question.as_mut() {
                    m.buf.pop();
                }
            }
            KeyCode::Enter => {
                let ans = app.question.as_ref().map(|m| m.buf.trim().to_string()).unwrap_or_default();
                if ans.is_empty() {
                    // Empty submit: fall back to the option list, don't answer nothing.
                    if let Some(m) = app.question.as_mut() {
                        m.typing = false;
                    }
                } else {
                    app.answer_question(Some(ans));
                }
            }
            KeyCode::Esc => {
                if let Some(m) = app.question.as_mut() {
                    m.typing = false;
                }
            }
            _ => {}
        }
        return;
    }
    let n = app.question.as_ref().map(|m| m.options.len()).unwrap_or(0);
    match k.code {
        KeyCode::Up => {
            if let Some(m) = app.question.as_mut() {
                m.sel = if m.sel == 0 { n } else { m.sel - 1 };
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if let Some(m) = app.question.as_mut() {
                m.sel = (m.sel + 1) % (n + 1);
            }
        }
        KeyCode::Enter => {
            let (sel, option) = match app.question.as_ref() {
                Some(m) => (m.sel, m.options.get(m.sel).cloned()),
                None => return,
            };
            if sel >= n {
                if let Some(m) = app.question.as_mut() {
                    m.typing = true;
                }
            } else if let Some(opt) = option {
                app.answer_question(Some(opt));
            }
        }
        KeyCode::Char('c') => {
            if let Some(m) = app.question.as_mut() {
                m.typing = true;
            }
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let option = app.question.as_ref().and_then(|m| m.options.get(c as usize - '1' as usize).cloned());
            if let Some(opt) = option {
                app.answer_question(Some(opt));
            }
        }
        KeyCode::Esc => app.answer_question(None),
        _ => {}
    }
}

fn handle_permission_key(app: &mut App, k: &event::KeyEvent) {
    match k.code {
        KeyCode::Up | KeyCode::Down => {
            if let Some(m) = app.permission.as_mut() {
                m.sel = (m.sel + 1) % 3;
            }
        }
        KeyCode::Enter => {
            let sel = app.permission.as_ref().map(|m| m.sel).unwrap_or(0);
            let outcome = match sel {
                0 => Outcome::AllowOnce,
                1 => Outcome::AlwaysAllow,
                _ => Outcome::NeverAllow,
            };
            app.answer_permission(outcome);
        }
        KeyCode::Char('y') | KeyCode::Char('1') => app.answer_permission(Outcome::AllowOnce),
        KeyCode::Char('a') | KeyCode::Char('2') => app.answer_permission(Outcome::AlwaysAllow),
        KeyCode::Char('n') | KeyCode::Char('3') => app.answer_permission(Outcome::NeverAllow),
        KeyCode::Esc => app.answer_permission(Outcome::DenyOnce),
        _ => {}
    }
}

fn handle_shell_key(app: &mut App, k: &event::KeyEvent) {
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::PageUp if shift => {
            let max = app.emu.lock().unwrap().term.history_len();
            app.scroll = (app.scroll + 10).min(max);
        }
        KeyCode::PageDown if shift => app.scroll = app.scroll.saturating_sub(10),
        _ => {
            app.scroll = 0;
            let (ack, bp) = {
                let g = app.emu.lock().unwrap();
                (g.term.app_cursor_keys, g.term.bracketed_paste)
            };
            if let Some(bytes) = encode_key(k, ack, bp) {
                let _ = app.writer.lock().unwrap().write_all(&bytes);
            }
        }
    }
}

fn insert_char(app: &mut App, c: char) {
    app.composer.insert(app.comp_cur, c);
    app.comp_cur += 1;
}

fn handle_agent_key(app: &mut App, k: &event::KeyEvent) -> Result<()> {
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => insert_char(app, c),
        KeyCode::Enter if alt || shift => insert_char(app, '\n'),
        KeyCode::Enter => app.composer_submit()?,
        KeyCode::Backspace => {
            if app.comp_cur > 0 {
                app.comp_cur -= 1;
                app.composer.remove(crate::app::render::char_index(&app.composer, app.comp_cur));
            }
        }
        KeyCode::Delete => {
            let n = app.composer.chars().count();
            if app.comp_cur < n {
                app.composer.remove(crate::app::render::char_index(&app.composer, app.comp_cur));
            }
        }
        KeyCode::Left => app.comp_cur = app.comp_cur.saturating_sub(1),
        KeyCode::Right => app.comp_cur = (app.comp_cur + 1).min(app.composer.chars().count()),
        KeyCode::Home if k.modifiers.contains(KeyModifiers::CONTROL) => app.agent_scroll = usize::MAX / 4,
        KeyCode::End if k.modifiers.contains(KeyModifiers::CONTROL) => app.agent_scroll = 0,
        KeyCode::Home => app.comp_cur = 0,
        KeyCode::End => app.comp_cur = app.composer.chars().count(),
        KeyCode::PageUp => app.agent_scroll += if shift { 2 } else { 10 },
        KeyCode::PageDown => app.agent_scroll = app.agent_scroll.saturating_sub(if shift { 2 } else { 10 }),
        KeyCode::Tab => app.expand_tools = !app.expand_tools,
        KeyCode::Up => {
            if !app.history.is_empty() && app.hist_pos > 0 {
                app.hist_pos -= 1;
                app.composer = app.history[app.hist_pos].clone();
                app.comp_cur = app.composer.chars().count();
            }
        }
        KeyCode::Down => {
            if app.hist_pos + 1 < app.history.len() {
                app.hist_pos += 1;
                app.composer = app.history[app.hist_pos].clone();
            } else {
                app.hist_pos = app.history.len();
                app.composer.clear();
            }
            app.comp_cur = app.composer.chars().count();
        }
        KeyCode::Esc if app.busy => {
            app.agent_abort.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(tx) = &app.agent_tx {
                let _ = tx.send(AgentCommand::Abort);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_paste;

    #[test]
    fn paste_normalizes_newlines_and_tabs() {
        let (clean, dropped) = sanitize_paste("a\r\nb\rc\td");
        assert_eq!(clean, "a\nb\nc    d");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn paste_strips_escape_and_control_sequences() {
        let (clean, dropped) = sanitize_paste("ok\x1b]0;pwned\x07line\x00end");
        assert_eq!(clean, "ok]0;pwnedlineend");
        assert!(dropped > 0);
    }

    #[test]
    fn paste_is_capped_with_visible_drop_count() {
        let big = "x".repeat(super::MAX_PASTE_BYTES + 1234);
        let (clean, dropped) = sanitize_paste(&big);
        assert_eq!(clean.len(), super::MAX_PASTE_BYTES);
        assert_eq!(dropped, 1234);
    }
}
