pub mod tools;

use crate::agent::tools::{ToolOutcome};
use crate::config::{Config, Provider};
use crate::providers::{self, AssistantMsg, ChatMsg, RequestCfg, Role, StreamCtx};
use crate::safety::{Gate, Outcome, Verdict};
use crate::session::{GoalState, Session};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
#[allow(dead_code)]
pub enum AgentEvent {
    TextDelta(String),
    /// Chain-of-thought from thinking models (Qwen3). Ephemeral: never saved.
    ReasoningDelta(String),
    TextDone,
    ToolStarted { id: String, name: String, args: String },
    ToolFinished { id: String, ok: bool, output: String },
    Plan(Vec<(String, String)>),
    NeedPermission { title: String, detail: String, rule_key: String, tx: Sender<Outcome> },
    /// A child command is waiting for a password: the UI must prompt (hidden)
    /// and send it back — Some(secret) to submit, None to cancel.
    NeedSecret { prompt: String, tx: Sender<Option<String>> },
    Info(String),
    ErrorText(String),
    GoalFinished { summary: String, evidence: String },
    /// Estimated context usage after the latest request.
    Context { used: usize, window: usize },
    Done,
}

#[derive(Debug)]
pub enum AgentCommand {
    Prompt { text: String, goal: bool },
    /// Resume an unfinished goal with the standard continuation nudge, plus
    /// optional extra guidance from the user (/continue [note]).
    ContinueGoal { note: Option<String> },
    Resume { messages: Vec<ChatMsg>, goal: Option<GoalState> },
    Compact,
    Abort,
    Quit,
}

/// How a turn was started: plain chat, a fresh goal, or resuming the open one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnMode {
    Normal,
    Goal,
    Continue,
}

#[derive(Clone)]
pub struct AgentEnv {
    pub provider: Option<crate::config::Provider>,
    pub model: String,
    pub workspace: std::path::PathBuf,
    pub shell_cwd: Option<std::path::PathBuf>,
    pub session_id: String,
    /// Thinking depth for reasoning models; None = model's own default.
    /// Seeded from config at startup and changed live via /effort.
    pub reasoning_effort: Option<String>,
    /// YOLO mode: safety-gate Ask verdicts are auto-approved (hard-blocks and
    /// sudo password prompts remain). Toggled live with /yolo or --yolo.
    pub yolo: bool,
}

#[allow(dead_code)]
pub struct AgentHandle {
    pub tx: Sender<AgentCommand>,
    pub abort: Arc<AtomicBool>,
    pub busy: Arc<AtomicBool>,
}

fn system_prompt(env: &AgentEnv) -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows (use PowerShell syntax)",
        "macos" => "macOS",
        o => o,
    };
    format!(
        "You are Penguin, a system assistant embedded in the user's terminal (\
         'penguin command'). Your primary role is operating the computer from the \
         shell: navigating, locating files, reading/editing files, permissions, \
         hardware and OS management. Building/coding projects is secondary.\n\n\
        Environment: {os}. Workspace: {}. The user's interactive shell may be in a \
        different directory: {}.\n\n\
        Rules:\n\
        - Commands run in a hidden non-interactive shell inside the workspace. \
        Interactive programs (vim, top, ssh prompts) will not work there.\n\
        - Elevated access works: just prefix a command with sudo — penguin intercepts the \
        password prompt and asks the user for it in a secure hidden dialog. Never ask the \
        user to type or paste their password into the chat.\n\
        - You may READ anything on the machine. Writes/deletes/permission-changes \
        outside the workspace require explicit user approval via a dialog; if denied, \
        do NOT retry with a different spelling — explain and ask the user instead.\n\
        - Catastrophic commands (rm -rf /, raw device writes, mkfs, fork bombs) are \
        hard-blocked by policy; never attempt them.\n\
        - Prefer read_file before edit_file; keep edits precise.\n\
        - When given a GOAL: first call update_plan with concrete steps, execute them \
        one at a time updating statuses, and finish ONLY via finish_goal after running \
        an actual verification command whose output proves success. If verification \
        fails, keep working.\n\
        - Skills: you have a personal skill library that persists across sessions. \
        {}\
        When a saved skill matches the task, call load_skill(name) and follow it. \
        After completing a GOAL — or any non-trivial multi-step procedure you had to \
        figure out — call save_skill with the proven recipe so future sessions reuse \
        it.\n\
        - Be concise in prose; you are in a terminal.",
        env.workspace.display(),
        env.shell_cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "unknown".into()),
        crate::skills::prompt_block(),
    )
}

pub fn spawn_agent(env: Arc<Mutex<AgentEnv>>, cfg: Config) -> (AgentHandle, Receiver<AgentEvent>) {
    let (ev_tx, ev_rx) = channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = channel::<AgentCommand>();
    let abort = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    let handle = AgentHandle { tx: cmd_tx, abort: abort.clone(), busy: busy.clone() };

    std::thread::spawn(move || {
        let mut messages: Vec<ChatMsg> = Vec::new();
        let mut goal_state: Option<GoalState> = None;
        agent_thread(env, cfg, cmd_rx, ev_tx, abort, busy, &mut messages, &mut goal_state);
    });
    (handle, ev_rx)
}

/// Rough token estimate (~4 chars/token) over the whole request payload.
pub fn estimate_tokens(msgs: &[ChatMsg]) -> usize {
    let mut chars = 0usize;
    for m in msgs {
        chars += m.text.chars().count();
        for tc in &m.tool_calls {
            chars += tc.name.chars().count() + tc.args.to_string().chars().count();
        }
    }
    chars / 4 + msgs.len() * 3 + 8
}

fn render_for_summary(msgs: &[ChatMsg]) -> String {
    let mut out = String::new();
    for m in msgs {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool-result",
        };
        if !m.text.is_empty() {
            out.push_str(&format!("[{role}] {}\n", tools::middle_truncate(&m.text, 1500)));
        }
        for tc in &m.tool_calls {
            out.push_str(&format!("[{role} → {}] {}\n", tc.name, tools::middle_truncate(&tc.args.to_string(), 500)));
        }
    }
    out
}

fn trim_old_tools(messages: &mut Vec<ChatMsg>, keep_last: usize) -> usize {
    let cut = messages.len().saturating_sub(keep_last);
    let mut trimmed = 0;
    for i in 1..cut {
        if messages[i].role == Role::Tool && messages[i].text.chars().count() > 300 {
            let l = messages[i].text.chars().count();
            messages[i].text = format!("[{l}-byte tool output elided by context compaction]");
            trimmed += 1;
        }
    }
    trimmed
}

/// Summarize old turns into one message + elide stale tool outputs.
/// Returns true if anything was reclaimed. Never breaks tool-call pairing:
/// the split point is always a User-role boundary.
fn compact_messages(
    provider: &Provider,
    model: &str,
    messages: &mut Vec<ChatMsg>,
    ev_tx: &Sender<AgentEvent>,
    abort: &AtomicBool,
    rcfg: &RequestCfg,
) -> bool {
    let before = estimate_tokens(messages);
    if messages.len() < 8 || abort.load(Ordering::Relaxed) {
        return false;
    }
    let mut split = None;
    for i in (1..=(messages.len() - 6)).rev() {
        if messages[i].role == Role::User {
            split = Some(i);
            break;
        }
    }
    if let Some(i) = split {
        let body = tools::middle_truncate(&render_for_summary(&messages[1..i]), 48_000);
        let req = vec![
            ChatMsg::system(
                "You compress conversation transcripts for an AI terminal agent. Write a dense \
                 summary preserving: the user's goal(s), key facts learned, files/paths touched, \
                 important command outcomes, decisions, errors hit, and remaining work. \
                 Prose + short bullets. No preamble.",
            ),
            ChatMsg::user(format!("Summarize so the agent can continue with full context:\n\n{body}")),
        ];
        let mut summary = String::new();
        let mut ctx = StreamCtx {
            on_text: &mut |t| summary.push_str(t),
            on_reasoning: &mut |_| {},
            on_note: &mut |n| {
                let _ = ev_tx.send(AgentEvent::Info(providers::truncate(n, 160).to_string()));
            },
            abort,
        };
        match providers::chat(provider, model, &req, &[], rcfg, &mut ctx) {
            Ok(resp) if !resp.text.trim().is_empty() => {
                messages.drain(1..i);
                messages.insert(
                    1,
                    ChatMsg::user(format!(
                        "[Context summary of the earlier conversation — live session continues below]\n\n{}",
                        resp.text.trim()
                    )),
                );
            }
            Ok(_) => {}
            Err(e) => {
                let _ = ev_tx.send(AgentEvent::Info(format!("⊂ summarizer unavailable ({}) — eliding stale tool output instead", providers::truncate(&e.to_string(), 120))));
            }
        }
    }
    trim_old_tools(messages, 12);
    let after = estimate_tokens(messages);
    if after >= before {
        return false;
    }
    let _ = ev_tx.send(AgentEvent::Info(format!("⊂ context compacted: ~{before} → ~{after} tokens")));
    true
}

fn save_session(env: &AgentEnv, messages: &[ChatMsg], goal: &Option<GoalState>) {
    let kept: Vec<ChatMsg> = messages.iter().filter(|m| m.role != Role::System).cloned().collect();
    if kept.is_empty() {
        return;
    }
    let s = Session::new(env.session_id.clone(), env, kept, goal.clone());
    let _ = crate::session::save(&s);
}

fn agent_thread(
    env: Arc<Mutex<AgentEnv>>,
    mut cfg: Config,
    cmd_rx: Receiver<AgentCommand>,
    ev_tx: Sender<AgentEvent>,
    abort: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    messages: &mut Vec<ChatMsg>,
    goal_state: &mut Option<GoalState>,
) {
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AgentCommand::Quit => break,
            AgentCommand::Abort => {
                abort.store(true, Ordering::Relaxed);
            }
            AgentCommand::Resume { messages: loaded, goal } => {
                // A fresh command must not inherit a stale Esc from the last turn.
                abort.store(false, Ordering::Relaxed);
                *messages = loaded;
                *goal_state = goal;
                let _ = ev_tx.send(AgentEvent::Info(format!(
                    "session resumed · {} messages · context ~{} tokens",
                    messages.len(),
                    estimate_tokens(messages)
                )));
                let env_guard = env.lock().unwrap().clone();
                let window = env_guard.provider.as_ref().and_then(|p| p.context_window).unwrap_or(32_768);
                let _ = ev_tx.send(AgentEvent::Context { used: estimate_tokens(messages), window });
            }
            AgentCommand::Compact => {
                abort.store(false, Ordering::Relaxed);
                let env_guard = env.lock().unwrap().clone();
                let mut rcfg = RequestCfg::from_agent(&cfg.agent);
                rcfg.reasoning_effort = env_guard.reasoning_effort.clone();
                match env_guard.provider.clone() {
                    Some(p) => {
                        if compact_messages(&p, &env_guard.model, messages, &ev_tx, &abort, &rcfg) {
                            save_session(&env_guard, messages, goal_state);
                        } else {
                            let _ = ev_tx.send(AgentEvent::Info("context is small — nothing to compact".into()));
                        }
                        let window = p.context_window.unwrap_or(32_768);
                        let _ = ev_tx.send(AgentEvent::Context { used: estimate_tokens(messages), window });
                    }
                    None => {
                        let _ = ev_tx.send(AgentEvent::ErrorText("no provider configured".into()));
                    }
                }
            }
            AgentCommand::Prompt { text, goal } => run_turn(
                &env,
                &mut cfg,
                messages,
                goal_state,
                &ev_tx,
                &abort,
                &busy,
                text,
                if goal { TurnMode::Goal } else { TurnMode::Normal },
            ),
            AgentCommand::ContinueGoal { note } => run_turn(
                &env,
                &mut cfg,
                messages,
                goal_state,
                &ev_tx,
                &abort,
                &busy,
                note.unwrap_or_default(),
                TurnMode::Continue,
            ),
        }
    }
}

/// Nudge that keeps a stalled goal moving; reused by auto-continuation and /continue.
const CONTINUE_NUDGE: &str = "Continue working toward the GOAL now. Take the next concrete action with \
 your tools. Do not stop until you have run a verification command proving success and called finish_goal. \
 If you are truly blocked, explain exactly what blocks you.";

/// One full agent turn: chat → tool calls → repeat, with goal auto-continuation.
fn run_turn(
    env: &Arc<Mutex<AgentEnv>>,
    cfg: &mut Config,
    messages: &mut Vec<ChatMsg>,
    goal_state: &mut Option<GoalState>,
    ev_tx: &Sender<AgentEvent>,
    abort: &AtomicBool,
    busy: &AtomicBool,
    text: String,
    mode: TurnMode,
) {
    abort.store(false, Ordering::Relaxed);
    let env_guard = env.lock().unwrap().clone();
    let provider = match env_guard.provider.clone() {
        Some(p) => p,
        None => {
            let _ = ev_tx.send(AgentEvent::ErrorText(
                "No provider configured. Run /providers to set one up.".into(),
            ));
            let _ = ev_tx.send(AgentEvent::Done);
            return;
        }
    };
    if mode == TurnMode::Continue && goal_state.as_ref().filter(|g| !g.verified).is_none() {
        let _ = ev_tx.send(AgentEvent::ErrorText(
            "no unfinished goal to continue — start one with /goal <text>".into(),
        ));
        let _ = ev_tx.send(AgentEvent::Done);
        return;
    }
    busy.store(true, Ordering::Relaxed);
    if messages.is_empty() || messages[0].role != Role::System {
        messages.insert(0, ChatMsg::system(system_prompt(&env_guard)));
    } else {
        messages[0] = ChatMsg::system(system_prompt(&env_guard));
    }
    match mode {
        TurnMode::Normal => messages.push(ChatMsg::user(text)),
        TurnMode::Goal => {
            messages.push(ChatMsg::user(format!(
                "GOAL (autonomous mode — plan, execute, verify): {text}"
            )));
            *goal_state = Some(GoalState { text, plan: Vec::new(), verified: false });
        }
        TurnMode::Continue => {
            let note = text.trim();
            let nudge = if note.is_empty() {
                CONTINUE_NUDGE.to_string()
            } else {
                format!("{CONTINUE_NUDGE}\n\nAdditional guidance from the user: {note}")
            };
            push_user(messages, &nudge);
        }
    }

    let mut gate = Gate::new(env_guard.workspace.clone(), cfg.agent.auto_approve_workspace, cfg.rules.clone());
    // Ask the UI for a password (hidden input) when a child command prompts for one.
    let ask_secret = |prompt: &str| -> Option<String> {
        let (tx, rx) = channel::<Option<String>>();
        if ev_tx.send(AgentEvent::NeedSecret { prompt: prompt.to_string(), tx }).is_err() {
            return None;
        }
        rx.recv().ok().flatten()
    };
    let specs = tools::tool_specs();
    let max_iter = cfg.agent.max_iterations.max(4);
    let max_cont = cfg.agent.goal_continuations;
    let mut rcfg = RequestCfg::from_agent(&cfg.agent);
    // Live /effort overrides the static config for this turn.
    rcfg.reasoning_effort = env_guard.reasoning_effort.clone();
    let window = provider.context_window.unwrap_or(32_768).max(1024);
    let compact_at = cfg.agent.compact_at_percent as usize;
    let mut goal_open = matches!(mode, TurnMode::Goal | TurnMode::Continue);
    let mut turn_err = None;
    let mut continuations = 0usize;
    // Consecutive replies with no text, no tool calls and nothing
    // actionable — thinking models cut off at the token limit can
    // produce these endlessly; bail out instead of spinning.
    let mut empty_streak = 0usize;
    // Consecutive replies where thinking ate the whole budget
    // (reasoning streamed but no answer); drives the escalating
    // auto-recovery below before finally giving up.
    let mut starve_streak = 0usize;

    loop {
        for _iter in 0..max_iter {
            if abort.load(Ordering::Relaxed) {
                break;
            }
            let est = estimate_tokens(messages);
            let _ = ev_tx.send(AgentEvent::Context { used: est, window });
            // Messages are always request-shaped here (no pending
            // tool calls), so this is the safe point to checkpoint
            // the session: a crash or timeout loses at most one step.
            save_session(&env_guard, messages, goal_state);
            if est * 100 >= window * compact_at {
                if compact_messages(&provider, &env_guard.model, messages, &ev_tx, &abort, &rcfg) {
                    save_session(&env_guard, messages, goal_state);
                    let _ = ev_tx.send(AgentEvent::Context { used: estimate_tokens(messages), window });
                }
            }
            let model = env_guard.model.clone();
            let mut ctx = StreamCtx {
                on_text: &mut |t: &str| {
                    let _ = ev_tx.send(AgentEvent::TextDelta(t.to_string()));
                },
                on_reasoning: &mut |t: &str| {
                    let _ = ev_tx.send(AgentEvent::ReasoningDelta(t.to_string()));
                },
                on_note: &mut |n: &str| {
                    let _ = ev_tx.send(AgentEvent::Info(n.to_string()));
                },
                abort: &abort,
            };
            match providers::chat(&provider, &model, messages, &specs, &rcfg, &mut ctx) {
                Ok(resp) => {
                    let _ = ev_tx.send(AgentEvent::TextDone);
                    let totally_empty = resp.text.trim().is_empty()
                        && resp.tool_calls.is_empty()
                        && resp.dropped_tools.is_empty();
                    if !totally_empty {
                        messages.push(assistant_msg(&resp));
                    }
                    if !resp.dropped_tools.is_empty() {
                        let names = resp.dropped_tools.join(", ");
                        let _ = ev_tx.send(AgentEvent::Info(format!(
                            "⚠ dropped truncated tool call(s): {names} — never executed with partial arguments"
                        )));
                        messages.push(ChatMsg::user(format!(
                            "Your tool call(s) [{names}] were cut off at the token limit and were NOT \
                             executed. Retry with smaller/simpler arguments (e.g. build large files with \
                             several sequential edit_file calls)."
                        )));
                    }
                    if totally_empty && (resp.reasoning_chars > 0 || resp.truncated) {
                        // Thinking starvation: the model reasoned until the
                        // completion budget ran out and never answered.
                        // Escalate automatically instead of erroring out.
                        starve_streak += 1;
                        empty_streak = 0;
                        match starve_streak {
                            1 => {
                                rcfg.max_tokens = (rcfg.max_tokens * 2).min(window as u32);
                                push_user(messages, STARVED_NUDGE);
                                let _ = ev_tx.send(AgentEvent::Info(format!(
                                    "⚠ thinking exhausted the output budget — retrying with a {} token budget",
                                    rcfg.max_tokens
                                )));
                            }
                            2 => {
                                rcfg.reasoning_effort = step_down_effort(rcfg.reasoning_effort.as_deref());
                                push_user(messages, STARVED_NUDGE);
                                let _ = ev_tx.send(AgentEvent::Info(format!(
                                    "⚠ starved again — retrying with thinking depth {} (your /effort setting unchanged)",
                                    rcfg.reasoning_effort.as_deref().unwrap_or("server default")
                                )));
                            }
                            _ => {
                                turn_err = Some(format!(
                                    "model spent its whole budget on thinking and returned nothing usable \
                                     ({starve_streak}× in a row) — try /effort off, raise agent.max_tokens, \
                                     or set agent.max_reasoning_tokens to cap thinking"
                                ));
                                break;
                            }
                        }
                        continue;
                    }
                    if totally_empty {
                        empty_streak += 1;
                    } else {
                        empty_streak = 0;
                        starve_streak = 0;
                    }
                    if empty_streak >= 2 {
                        turn_err = Some(
                            "model returned nothing usable twice in a row — it may be burning its token \
                             budget on thinking (try agent.max_tokens up, or /model to switch)".into(),
                        );
                        break;
                    }
                    if resp.tool_calls.is_empty() {
                        if resp.truncated && !abort.load(Ordering::Relaxed) {
                            // answer cut off mid-sentence: let it
                            // continue once before giving up
                            messages.push(ChatMsg::user(
                                "Your reply was cut off at the token limit. Continue exactly where \
                                 you left off, in smaller pieces.".to_string(),
                            ));
                            continue;
                        }
                        break;
                    }
                    let mut finish = None;
                    // Index of the first call a mid-batch Esc left
                    // unanswered; those get synthetic tool results
                    // so the next request never carries dangling calls.
                    let mut cancelled_from = resp.tool_calls.len();
                    for (ci, call) in resp.tool_calls.iter().enumerate() {
                        if abort.load(Ordering::Relaxed) {
                            cancelled_from = ci;
                            break;
                        }
                        let _ = ev_tx.send(AgentEvent::ToolStarted {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            args: providers::truncate(&call.args.to_string(), 300).to_string(),
                        });

                        // safety gate for side-effecting tools
                        let allowed = gate_tool(&mut gate, cfg, call, &env_guard, &ev_tx);
                        let outcome = if allowed {
                            match tools::execute(&call.name, &call.args, &env_guard.workspace, &ask_secret, &abort) {
                                Ok(o) => Some(o),
                                Err(e) => {
                                    let _ = ev_tx.send(AgentEvent::ToolFinished {
                                        id: call.id.clone(),
                                        ok: false,
                                        output: format!("error: {e}"),
                                    });
                                    messages.push(ChatMsg::tool_result(&call.id, format!("error: {e}")));
                                    None
                                }
                            }
                        } else {
                            let msg = "Permission denied by user or policy. Do not retry this exact action; ask the user or choose a safer approach.";
                            let _ = ev_tx.send(AgentEvent::ToolFinished {
                                id: call.id.clone(),
                                ok: false,
                                output: msg.into(),
                            });
                            messages.push(ChatMsg::tool_result(&call.id, msg));
                            None
                        };

                        match outcome {
                            Some(ToolOutcome::Feed(text)) => {
                                let _ = ev_tx.send(AgentEvent::ToolFinished {
                                    id: call.id.clone(),
                                    ok: true,
                                    output: text.clone(),
                                });
                                messages.push(ChatMsg::tool_result(&call.id, text));
                            }
                            Some(ToolOutcome::Plan(steps)) => {
                                let _ = ev_tx.send(AgentEvent::ToolFinished {
                                    id: call.id.clone(),
                                    ok: true,
                                    output: format!("{} steps", steps.len()),
                                });
                                if let Some(g) = goal_state.as_mut() {
                                    g.plan = steps.clone();
                                }
                                let _ = ev_tx.send(AgentEvent::Plan(steps));
                                messages.push(ChatMsg::tool_result(&call.id, "plan published"));
                            }
                            Some(ToolOutcome::GoalFinished { summary, evidence }) => {
                                let _ = ev_tx.send(AgentEvent::ToolFinished {
                                    id: call.id.clone(),
                                    ok: true,
                                    output: summary.clone(),
                                });
                                if let Some(g) = goal_state.as_mut() {
                                    g.verified = true;
                                    g.plan.iter_mut().for_each(|(_, s)| *s = "done".into());
                                }
                                finish = Some((summary, evidence));
                                messages.push(ChatMsg::tool_result(&call.id, "goal accepted"));
                            }
                            None => {}
                        }
                    }
                    if cancelled_from < resp.tool_calls.len() {
                        for call in &resp.tool_calls[cancelled_from..] {
                            let msg = "[cancelled by user before execution]";
                            messages.push(ChatMsg::tool_result(&call.id, msg));
                        }
                    }
                    if let Some((summary, evidence)) = finish {
                        goal_open = false;
                        let _ = ev_tx.send(AgentEvent::GoalFinished { summary, evidence });
                        break;
                    }
                }
                Err(e) => {
                    turn_err = Some(format!("{e}"));
                    break;
                }
            }
        }
        if abort.load(Ordering::Relaxed) {
            let _ = ev_tx.send(AgentEvent::Info("(aborted)".into()));
        }
        if !goal_open || turn_err.is_some() || abort.load(Ordering::Relaxed) {
            break;
        }
        if continuations >= max_cont {
            break;
        }
        continuations += 1;
        let _ = ev_tx.send(AgentEvent::Info(format!(
            "↻ goal not verified yet — auto-continuing ({continuations}/{max_cont})"
        )));
        push_user(messages, CONTINUE_NUDGE);
    }
    if goal_open && turn_err.is_none() {
        let _ = ev_tx.send(AgentEvent::Info(
"Goal still unverified after continuation attempts — send /continue to keep going \
 (optionally with guidance, e.g. /continue try the docker route instead).".into(),
        ));
    }
    if let Some(e) = turn_err {
        let _ = ev_tx.send(AgentEvent::ErrorText(e));
    }
    save_session(&env_guard, messages, goal_state);
    let _ = ev_tx.send(AgentEvent::Context { used: estimate_tokens(messages), window });
    busy.store(false, Ordering::Relaxed);
    let _ = ev_tx.send(AgentEvent::Done);
}

fn assistant_msg(resp: &AssistantMsg) -> ChatMsg {
    ChatMsg { role: Role::Assistant, text: resp.text.clone(), tool_calls: resp.tool_calls.clone(), tool_call_id: None }
}

/// Appends to the trailing user message instead of stacking two consecutive
/// user turns (Anthropic rejects non-alternating roles).
fn push_user(messages: &mut Vec<ChatMsg>, text: &str) {
    match messages.last_mut() {
        Some(m) if m.role == Role::User => {
            m.text.push_str("\n\n");
            m.text.push_str(text);
        }
        _ => messages.push(ChatMsg::user(text.to_string())),
    }
}

/// Sent when a thinking model burns its whole completion budget on reasoning
/// and produces neither an answer nor a tool call.
const STARVED_NUDGE: &str = "You produced only internal reasoning and hit the output limit before \
                             answering. Do NOT think any further. Reply NOW with ONLY your final \
                             answer (or your next tool call), as briefly as possible.";

/// One notch shallower for a single retry; keeps the user's /effort setting.
/// None means the server default (deep on Qwen3.x) → cut it in half.
fn step_down_effort(cur: Option<&str>) -> Option<String> {
    let order = crate::config::REASONING_EFFORTS; // low · medium · high · xhigh
    match cur {
        None => Some("medium".into()),
        Some(c) => match order.iter().position(|e| *e == c) {
            None | Some(0) => Some(c.to_string()),
            Some(i) => Some(order[i - 1].to_string()),
        },
    }
}

/// Returns true if the call may run.
fn gate_tool(
    gate: &mut Gate,
    cfg: &mut Config,
    call: &crate::providers::ToolCall,
    env: &AgentEnv,
    ev_tx: &Sender<AgentEvent>,
) -> bool {
    let verdict = match call.name.as_str() {
        "run_command" => {
            let cmd = call.args["command"].as_str().unwrap_or("");
            gate.check_command(cmd, &env.workspace)
        }
        "write_file" | "edit_file" => {
            let p = crate::safety::resolve(&env.workspace, call.args["path"].as_str().unwrap_or(""));
            gate.check_file_op(&p, true)
        }
        "set_permissions" => {
            let p = crate::safety::resolve(&env.workspace, call.args["path"].as_str().unwrap_or(""));
            gate.check_file_op(&p, true)
        }
        _ => Verdict::Allow, // read-only tools + plan/goal/skills
    };
    match verdict {
        Verdict::Allow => true,
        Verdict::HardDeny(msg) => {
            let _ = ev_tx.send(AgentEvent::Info(format!("⛔ blocked: {msg}")));
            false
        }
        Verdict::Ask(a) if env.yolo => {
            let _ = ev_tx.send(AgentEvent::Info(format!("⚠ yolo: auto-approved — {}", a.title)));
            true
        }
        Verdict::Ask(a) => {
            let (tx, rx) = channel::<Outcome>();
            let rule_key = a.rule_key.clone();
            let _ = ev_tx.send(AgentEvent::NeedPermission { title: a.title, detail: a.detail, rule_key: rule_key.clone(), tx });
            match rx.recv() {
                Ok(outcome) => {
                    gate.record(&outcome, &rule_key);
                    cfg.rules = gate.rules_mut().clone();
                    if outcome == Outcome::AlwaysAllow || outcome == Outcome::NeverAllow {
                        let mut c = Config::load().unwrap_or_else(|_| cfg.clone());
                        for r in &cfg.rules {
                            if !c.rules.iter().any(|x| x.pattern == r.pattern) {
                                c.rules.push(r.clone());
                            }
                        }
                        let _ = c.save();
                    }
                    matches!(outcome, Outcome::AllowOnce | Outcome::AlwaysAllow)
                }
                Err(_) => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;
    use std::path::PathBuf;

    fn call(name: &str, args: serde_json::Value) -> crate::providers::ToolCall {
        crate::providers::ToolCall { id: "c1".into(), name: name.into(), args }
    }

    fn env_with(yolo: bool) -> AgentEnv {
        AgentEnv {
            provider: None,
            model: String::new(),
            workspace: PathBuf::from("/w"),
            shell_cwd: None,
            session_id: "test".into(),
            reasoning_effort: None,
            yolo,
        }
    }

    #[test]
    fn effort_steps_down_only_shallow() {
        assert_eq!(step_down_effort(None).as_deref(), Some("medium"));
        assert_eq!(step_down_effort(Some("xhigh")).as_deref(), Some("high"));
        assert_eq!(step_down_effort(Some("high")).as_deref(), Some("medium"));
        // already shallowest: stays, never climbs back to the server default
        assert_eq!(step_down_effort(Some("low")).as_deref(), Some("low"));
    }

    #[test]
    fn push_user_merges_consecutive_user_turns() {
        let mut msgs = vec![ChatMsg::user("first"), ChatMsg::tool_result("c1", "out")];
        push_user(&mut msgs, "second");
        assert_eq!(msgs.len(), 3);
        push_user(&mut msgs, "third");
        assert_eq!(msgs.len(), 3);
        assert!(msgs[2].text.contains("second"));
        assert!(msgs[2].text.contains("third"));
    }

    #[test]
    fn yolo_auto_approves_asks_but_not_hard_denies() {
        let (tx, _rx) = channel::<AgentEvent>();
        let mut cfg = Config::default();

        let risky = call("run_command", json!({"command": "sudo systemctl restart nginx"}));
        assert!(gate_tool(&mut Gate::new("/w".into(), true, vec![]), &mut cfg, &risky, &env_with(true), &tx));

        let outside = call("write_file", json!({"path": "/etc/hosts"}));
        assert!(gate_tool(&mut Gate::new("/w".into(), false, vec![]), &mut cfg, &outside, &env_with(true), &tx));

        let cataclysm = call("run_command", json!({"command": "rm -rf /"}));
        assert!(!gate_tool(&mut Gate::new("/w".into(), true, vec![]), &mut cfg, &cataclysm, &env_with(true), &tx));
    }
}
