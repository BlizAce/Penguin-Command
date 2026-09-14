use super::{App, Entry, Mode};
use crate::theme::*;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, Paragraph},
    Frame,
};

pub fn char_index(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

fn shimmer(tick: u32) -> Color {
    const STEPS: [Color; 8] = [
        OMARCHY.accent,
        Color::Rgb(0xd3, 0xb1, 0xf9),
        Color::Rgb(0xdc, 0xbc, 0xf7),
        OMARCHY.accent2,
        Color::Rgb(0xdc, 0xbc, 0xf7),
        Color::Rgb(0xd3, 0xb1, 0xf9),
        OMARCHY.accent,
        Color::Rgb(0xc0, 0x9d, 0xf6),
    ];
    STEPS[((tick / 2) as usize) % STEPS.len()]
}

fn spinner(tick: u32) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[((tick / 2) as usize) % FRAMES.len()]
}

/// Char-safe word wrap (byte slicing here once panicked on multibyte words).
fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out: Vec<String> = Vec::new();
    for para in s.split('\n') {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut llen = 0usize;
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let mut w: Vec<char> = word.chars().collect();
            if llen > 0 && llen + 1 + w.len().min(width) > width {
                out.push(std::mem::take(&mut line));
                llen = 0;
            }
            if llen > 0 {
                line.push(' ');
                llen += 1;
            }
            while w.len() > width.saturating_sub(llen) && llen < width {
                let take = (width - llen).min(w.len());
                line.extend(&w[..take]);
                llen += take;
                w = w[take..].to_vec();
                if !w.is_empty() {
                    out.push(std::mem::take(&mut line));
                    llen = 0;
                }
            }
            if !w.is_empty() {
                line.extend(&w[..]);
                llen += w.len();
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{composer_cursor, composer_height, composer_wrap, wrap};

    #[test]
    fn wrap_basic() {
        assert_eq!(wrap("hello world", 10), vec!["hello", "world"]);
        assert_eq!(wrap("hi there friend", 8), vec!["hi there", "friend"]);
    }

    #[test]
    fn composer_wraps_hard_at_width_and_keeps_newlines() {
        let w = composer_wrap("abcdef", 4);
        assert_eq!(w, vec!["abcd".to_string(), "ef".to_string()]);
        let w = composer_wrap("ab\ncd", 10);
        assert_eq!(w, vec!["ab".to_string(), "cd".to_string()]);
        assert_eq!(composer_wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn composer_cursor_tracks_wrapped_rows_and_columns() {
        assert_eq!(composer_cursor(5, "abcdef", 4), (1, 1));
        assert_eq!(composer_cursor(2, "ab\ncd", 10), (0, 2));
        assert_eq!(composer_cursor(3, "ab\ncd", 10), (1, 0));
        // cursor parked just past a full row needs its own row
        assert_eq!(composer_cursor(8, "abcdefgh", 4), (2, 0));
        assert_eq!(composer_height(8, "abcdefgh", 4), 3);
        assert_eq!(composer_height(3, "abc", 4), 1);
        assert_eq!(composer_height(0, "", 4), 1);
    }

    #[test]
    fn wrap_long_and_multibyte_words() {
        assert_eq!(wrap("aaaaaaaaaaaaaa", 10), vec!["aaaaaaaaaa", "aaaa"]);
        // multibyte words must not panic nor split mid-char
        let w = wrap("🐧🐧🐧🐧🐧🐧 🦆x", 6);
        assert!(w.iter().all(|l| l.chars().count() <= 6));
    }
}

pub fn render(app: &mut App, f: &mut Frame) {
    let area = f.area();
    f.buffer_mut().set_style(area, Style::default().bg(OMARCHY.bg));

    if app.screensaving {
        crate::penguin::render_screensaver(app.saver_scene, app.tick, f, area);
        return;
    }

    let top = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(area);
    render_status(app, f, top[0]);

    if app.mode == Mode::Agent {
        // unified window: the agent owns the whole viewport below the status bar
        let sidebar_w: u16 = if area.width >= 108 { 32 } else { 0 };
        let body = if sidebar_w > 0 {
            let cols = Layout::horizontal([Constraint::Min(40), Constraint::Length(sidebar_w)]).split(top[1]);
            render_sidebar(app, f, cols[1]);
            cols[0]
        } else {
            top[1]
        };
        render_agent_view(app, f, body, sidebar_w == 0);
    } else {
        render_shell(app, f, top[1]);
    }

    if let Some(m) = &app.permission {
        let (sel, title, detail) = (m.sel, m.title.clone(), m.detail.clone());
        render_permission(f, area, sel, &title, &detail);
    }

    if let Some(m) = &app.secret {
        let (prompt, typed) = (m.prompt.clone(), m.buf.chars().count());
        render_secret(f, area, &prompt, typed);
    }
}

fn toggle_hint(agent: bool) -> String {
    if agent {
        "⎵ ctrl-space → shell".into()
    } else {
        "⎵ ctrl-space → agent".into()
    }
}

fn render_status(app: &App, f: &mut Frame, area: Rect) {
    let agent = app.mode == Mode::Agent;
    let yolo = app.env.lock().map(|e| e.yolo).unwrap_or(false);
    let mut spans = vec![
        Span::styled(" 🐧 PC ", Style::default().fg(OMARCHY.bg).bg(shimmer(app.tick)).add_modifier(Modifier::BOLD)),
        Span::styled("  ", Style::default()),
        if agent {
            Span::styled(" ✦ AGENT MODE ", Style::default().fg(Color::Rgb(0x14, 0x10, 0x20)).bg(OMARCHY.agent_line).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(" ⌨ SHELL ", Style::default().fg(OMARCHY.muted).bg(Color::Rgb(0x1c, 0x1c, 0x26)))
        },
        Span::styled("  ", Style::default()),
    ];
    if yolo {
        spans.push(Span::styled(" ⚠ YOLO ", Style::default().fg(Color::White).bg(OMARCHY.danger).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("  ", Style::default()));
    }
    spans.push(Span::styled(short_path(&workspace_of(app)), Style::default().fg(if agent { OMARCHY.accent } else { OMARCHY.muted })));
    if app.scroll > 0 {
        spans.push(Span::styled(format!("  ↑{} ", app.scroll), Style::default().fg(OMARCHY.warn)));
    }
    if app.ctx_window > 0 && app.ctx_used > 0 {
        let pct = app.ctx_used * 100 / app.ctx_window;
        let c = if pct >= 85 { OMARCHY.danger } else if pct >= 70 { OMARCHY.warn } else { OMARCHY.success };
        spans.push(Span::styled(format!(" ctx {pct}% "), Style::default().fg(c)));
    }
    let busy_info = if app.busy {
        let secs = app.busy_since.elapsed().as_secs();
        let tps = match app.first_token_at {
            Some(t) => {
                let d = t.elapsed().as_secs_f64();
                if d > 1.0 { format!(" · {} tok/s", (app.recv_chars as f64 / 4.0 / d) as u32) } else { String::new() }
            }
            None => String::new(),
        };
        format!("{} ⏱{secs}s{tps} ", spinner(app.tick))
    } else {
        String::new()
    };
    let right = format!(
        "{}   {} {}",
        toggle_hint(agent),
        busy_info,
        model_of(app),
    );
    spans.push(Span::styled(format!("{:>width$}", right, width = (area.width as usize).saturating_sub(display_width(&spans)).max(1)), Style::default().fg(OMARCHY.muted)));
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(OMARCHY.bg)), area);
}

fn display_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.width()).sum()
}

fn workspace_of(app: &App) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let p = &app.workspace;
    match p.strip_prefix(&home) {
        Ok(rel) if !rel.as_os_str().is_empty() => format!("~/{}", rel.display()),
        _ => p.display().to_string(),
    }
}

fn short_path(p: &str) -> String {
    if p.chars().count() > 42 {
        let tail: String = p.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    } else {
        p.to_string()
    }
}

fn model_of(app: &App) -> String {
    let e = app.env.lock().unwrap();
    let effort = e.reasoning_effort.clone();
    match (&e.provider, e.model.is_empty()) {
        (Some(p), false) => {
            let badge = effort.map(|x| format!(" ·e:{x}")).unwrap_or_default();
            format!("◆ {}·{}{badge}", p.name, crate::providers::truncate(&e.model, 24))
        }
        _ => "◆ no model — /providers".into(),
    }
}

fn tcolor(c: crate::term::TColor, bold: bool, default: Color) -> Color {
    match c {
        crate::term::TColor::Default => default,
        crate::term::TColor::Idx(i) => xterm2rgb(if bold && i < 8 { i + 8 } else { i }),
        crate::term::TColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn render_shell(app: &App, f: &mut Frame, area: Rect) {
    let g = app.emu.lock().unwrap();
    let term = &g.term;
    let rows = area.height as usize;
    let cols = area.width as usize;
    let screen = term.screen_lines();
    let hist_len = term.history_len();

    let total = hist_len + screen.len();
    let end = total.saturating_sub(app.scroll);
    let start = end.saturating_sub(rows);

    let mut out_lines: Vec<Line> = Vec::with_capacity(rows);
    for i in start..end {
        let cells: &[crate::term::Cell] = if i < hist_len {
            term.history_line(i).map(|v| v.as_slice()).unwrap_or(&[])
        } else {
            screen[i - hist_len].as_slice()
        };
        let mut spans: Vec<Span> = Vec::new();
        let mut cur = Style::default().bg(OMARCHY.bg).fg(OMARCHY.text);
        let mut buf = String::new();
        for cell in cells.iter().take(cols) {
            let mut st = Style::default()
                .fg(tcolor(cell.fg, cell.bold, OMARCHY.text))
                .bg(tcolor(cell.bg, false, OMARCHY.bg));
            if cell.reverse {
                st = st
                    .fg(tcolor(cell.bg, false, OMARCHY.bg))
                    .bg(tcolor(cell.fg, cell.bold, OMARCHY.text));
            }
            if cell.bold {
                st = st.add_modifier(Modifier::BOLD);
            }
            if cell.italic {
                st = st.add_modifier(Modifier::ITALIC);
            }
            if cell.underline {
                st = st.add_modifier(Modifier::UNDERLINED);
            }
            if cell.dim {
                st = st.add_modifier(Modifier::DIM);
            }
            if st != cur {
                spans.push(Span::styled(std::mem::take(&mut buf), cur));
                cur = st;
            }
            buf.push(if cell.invisible { ' ' } else { cell.ch });
        }
        spans.push(Span::styled(buf, cur));
        out_lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(out_lines), area);

    if app.mode == Mode::Shell && app.permission.is_none() {
        let (cx, cy, vis) = term.cursor();
        if vis && cy < rows && cx < cols && app.scroll == 0 {
            let cell = f.buffer_mut()[(area.x + cx as u16, area.y + cy as u16)].clone();
            let st = cell.style();
            let fg = st.fg.unwrap_or(OMARCHY.bg);
            let bg = if st.bg == Some(OMARCHY.bg) || st.bg.is_none() {
                OMARCHY.accent
            } else {
                st.bg.unwrap_or(OMARCHY.accent)
            };
            f.buffer_mut()[(area.x + cx as u16, area.y + cy as u16)].set_style(Style::default().fg(bg).bg(fg));
        }
    }
}

fn render_agent_view(app: &mut App, f: &mut Frame, area: Rect, inline_plan: bool) {
    // no border: the agent output is dumped straight into the main console
    let inner = area;

    let show_plan = inline_plan && !app.plan.is_empty() && area.width >= 96;
    let (tr_area, plan_area) = if show_plan {
        let c = Layout::horizontal([Constraint::Min(40), Constraint::Length(34)]).split(inner);
        (c[0], Some(c[1]))
    } else {
        (inner, None)
    };

    // transcript + optional slash suggestions + composer + key legend
    let typing_slash = !app.busy && app.composer.starts_with('/');
    let comp_w = (tr_area.width as usize).saturating_sub(4).max(1);
    let comp_cap = (tr_area.height as usize).saturating_sub(if typing_slash { 3 } else { 2 }).max(1);
    let comp_h = composer_height(app.comp_cur, &app.composer, comp_w).min(comp_cap) as u16;
    let mut constraints = vec![Constraint::Min(1)];
    if typing_slash {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(comp_h));
    constraints.push(Constraint::Length(1));
    let chunks = Layout::vertical(constraints).split(tr_area);
    render_transcript(app, f, chunks[0]);
    let mut idx = 1;
    if typing_slash {
        render_suggestions(app, f, chunks[idx]);
        idx += 1;
    }
    render_composer(app, f, chunks[idx]);
    render_keylegend(f, chunks[idx + 1], app.busy, app.queued.len());

    if let Some(pa) = plan_area {
        render_plan(app, f, pa);
    }
}

const SLASH_COMMANDS: [(&str, &str); 17] = [
    ("/goal", "autonomous: plan → execute → verify"),
    ("/continue", "resume the unfinished goal · /continue <note>"),
    ("/new", "start a fresh session"),
    ("/sessions", "list saved sessions"),
    ("/resume", "[id] resume a session (latest)"),
    ("/skills", "list the agent's saved skills"),
    ("/compact", "compact context now"),
    ("/retry", "resend the last prompt"),
    ("/effort", "cycle reasoning depth · /effort l|m|h|x"),
    ("/context", "show context usage"),
    ("/providers", "provider setup wizard"),
    ("/model", "[name] show or switch model"),
    ("/workspace", "<dir> set agent workspace"),
    ("/screensaver", "wake the penguins (idle screen)"),
    ("/clear", "clear transcript"),
    ("/help", "all commands"),
    ("/quit", "exit penguin"),
];

fn render_suggestions(app: &App, f: &mut Frame, area: Rect) {
    let q = app.composer.to_lowercase();
    let matches: Vec<(&str, &str)> = SLASH_COMMANDS.iter().copied().filter(|(c, _)| c.starts_with(&q)).collect();
    if matches.is_empty() {
        return;
    }
    let mut spans = vec![Span::styled(" ⌨ ", Style::default().fg(OMARCHY.accent))];
    for (i, (cmd, desc)) in matches.iter().take(5).enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default()));
        }
        spans.push(Span::styled(format!("{cmd} "), Style::default().fg(OMARCHY.accent2).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(*desc, Style::default().fg(OMARCHY.muted)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(0x16, 0x16, 0x1d))), area);
}

fn render_keylegend(f: &mut Frame, area: Rect, busy: bool, queued: usize) {
    let text = if busy {
        if queued > 0 {
            &format!("enter queue · ⏳ {queued} waiting · esc stop agent · ctrl-space → shell")
        } else {
            "enter queue · esc stop agent · ctrl-space → shell"
        }
    } else {
        "enter send · shift/alt+enter newline · ↑↓ history · tab tool output · /help · ctrl-space → shell"
    };
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(OMARCHY.muted).add_modifier(Modifier::ITALIC))),
        area,
    );
}

fn render_sidebar(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if app.active_goal.is_some() { OMARCHY.agent_line } else { OMARCHY.panel_line }))
        .title(Line::from(vec![
            Span::styled(" 🎯 goals ", Style::default().fg(OMARCHY.accent).add_modifier(Modifier::BOLD)),
            if app.goal_verified {
                Span::styled(" ✓ ", Style::default().fg(OMARCHY.success).add_modifier(Modifier::BOLD))
            } else if app.busy && app.active_goal.is_some() {
                Span::styled(spinner(app.tick), Style::default().fg(OMARCHY.warn))
            } else {
                Span::raw("")
            },
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    match &app.active_goal {
        Some(goal) => {
            for (i, l) in wrap(goal, inner.width.saturating_sub(2) as usize).into_iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(if i == 0 { "◉ " } else { "  " }, Style::default().fg(OMARCHY.accent2).add_modifier(Modifier::BOLD)),
                    Span::styled(l, Style::default().fg(OMARCHY.text).add_modifier(Modifier::BOLD)),
                ]));
            }
            lines.push(Line::from(""));
            if app.plan.is_empty() {
                lines.push(Line::from(Span::styled(
                    if app.busy { "planning steps…" } else { "awaiting plan" },
                    Style::default().fg(OMARCHY.muted).add_modifier(Modifier::ITALIC),
                )));
            }
            for (label, status) in &app.plan {
                let (icon, c) = match status.as_str() {
                    "done" => ("✓", OMARCHY.success),
                    "active" => (spinner(app.tick), OMARCHY.accent),
                    "failed" => ("✗", OMARCHY.danger),
                    _ => ("○", OMARCHY.muted),
                };
                let w = wrap(label, inner.width.saturating_sub(4) as usize);
                for (i, l) in w.into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(if i == 0 { format!("{icon} ") } else { "  ".into() }, Style::default().fg(c)),
                        Span::styled(l, Style::default().fg(OMARCHY.text)),
                    ]));
                }
            }
            if app.goal_verified {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " ★ goal verified ",
                    Style::default().fg(OMARCHY.bg).bg(OMARCHY.success).add_modifier(Modifier::BOLD),
                )));
            }
        }
        None => {
            lines.push(Line::from(Span::styled(" no active goal", Style::default().fg(OMARCHY.muted).add_modifier(Modifier::ITALIC))));
            lines.push(Line::from(""));
            for l in [
                "give the agent a task:",
                "",
                "  /goal <what to achieve>",
                "",
                "it plans the steps, runs",
                "them one by one here,",
                "and must verify before",
                "declaring success.",
            ] {
                lines.push(Line::from(Span::styled(l, Style::default().fg(OMARCHY.muted))));
            }
        }
    }
    let h = inner.height as usize;
    let visible: Vec<Line> = if lines.len() > h {
        lines[lines.len() - h..].to_vec()
    } else {
        lines
    };
    f.render_widget(Paragraph::new(visible), inner);
}

fn transcript_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    for e in &app.transcript {
        match e {
            Entry::Art(lines) => {
                for l in lines {
                    out.push(Line::from(Span::styled(l.clone(), Style::default().fg(OMARCHY.accent).add_modifier(Modifier::BOLD))));
                }
            }
            Entry::User(t) => {
                for (i, l) in wrap(t, width - 4).into_iter().enumerate() {
                    out.push(Line::from(vec![
                        Span::styled(if i == 0 { "❮ " } else { "  " }, Style::default().fg(OMARCHY.accent2).add_modifier(Modifier::BOLD)),
                        Span::styled(l, Style::default().fg(OMARCHY.text)),
                    ]));
                }
            }
            Entry::Assistant(t) => {
                for (i, l) in wrap(t, width - 4).into_iter().enumerate() {
                    out.push(Line::from(vec![
                        Span::styled(if i == 0 { "✦ " } else { "  " }, Style::default().fg(OMARCHY.accent).add_modifier(Modifier::BOLD)),
                        Span::styled(l, Style::default().fg(OMARCHY.text)),
                    ]));
                }
            }
            Entry::Tool { name, args, status, output, started } => {
                let running = status == "running";
                let (icon, ic) = if running {
                    (spinner(crate_tick(app)), OMARCHY.warn)
                } else if status == "done" {
                    ("✓", OMARCHY.success)
                } else {
                    ("✗", OMARCHY.danger)
                };
                let mut spans = vec![
                    Span::styled(format!("{icon} "), Style::default().fg(ic).add_modifier(Modifier::BOLD)),
                    Span::styled(name.clone(), Style::default().fg(OMARCHY.accent)),
                    Span::styled(crate::providers::truncate(args, width.saturating_sub(name.len() + 16)).to_string(), Style::default().fg(OMARCHY.muted)),
                ];
                if running {
                    spans.push(Span::styled(format!(" ⏱{}s", started.elapsed().as_secs()), Style::default().fg(OMARCHY.warn)));
                }
                out.push(Line::from(spans));
                if let Some(o) = output {
                    let cap = if app.expand_tools { 100_000 } else { 12 };
                    for l in wrap(o, width - 4).into_iter().take(cap) {
                        out.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(OMARCHY.panel_line)),
                            Span::styled(l, Style::default().fg(OMARCHY.muted)),
                        ]));
                    }
                }
            }
            Entry::Info(t) => {
                for l in wrap(t, width - 4) {
                    out.push(Line::from(vec![
                        Span::styled(" · ", Style::default().fg(OMARCHY.muted)),
                        Span::styled(l, Style::default().fg(OMARCHY.muted).add_modifier(Modifier::ITALIC)),
                    ]));
                }
            }
            Entry::Error(t) => {
                for l in wrap(t, width - 4) {
                    out.push(Line::from(vec![
                        Span::styled(" ⚠ ", Style::default().fg(OMARCHY.danger)),
                        Span::styled(l, Style::default().fg(OMARCHY.danger)),
                    ]));
                }
            }
            Entry::Goal { summary, evidence } => {
                out.push(Line::from(Span::styled(" ★ goal verified ", Style::default().fg(OMARCHY.bg).bg(OMARCHY.success).add_modifier(Modifier::BOLD))));
                for l in wrap(summary, width - 4) {
                    out.push(Line::from(vec![Span::styled("   ", Style::default()), Span::styled(l, Style::default().fg(OMARCHY.text))]));
                }
                for l in wrap(evidence, width - 6).into_iter().take(8) {
                    out.push(Line::from(vec![
                        Span::styled("   │ ", Style::default().fg(OMARCHY.success)),
                        Span::styled(l, Style::default().fg(OMARCHY.muted)),
                    ]));
                }
            }
        }
    }
    if !app.streaming.is_empty() {
        for (i, l) in wrap(&app.streaming, width - 4).into_iter().enumerate() {
            out.push(Line::from(vec![
                Span::styled(if i == 0 { "✦ " } else { "  " }, Style::default().fg(shimmer(app.tick)).add_modifier(Modifier::BOLD)),
                Span::styled(l, Style::default().fg(OMARCHY.text)),
            ]));
        }
    } else if app.busy {
        // Thinking models can chew silently for minutes before the first
        // answer token: show live chain-of-thought (tail) + an elapsed timer
        // so a slow endpoint never looks frozen.
        let secs = app.busy_since.elapsed().as_secs();
        let tail: Vec<String> = {
            let all: Vec<&str> = app.reasoning.lines().collect();
            all.iter().rev().take(3).rev().map(|l| l.trim_end().to_string()).collect()
        };
        if !tail.is_empty() {
            out.push(Line::from(Span::styled(
                format!(" ✻ reasoning · {secs}s"),
                Style::default().fg(OMARCHY.accent2).add_modifier(Modifier::ITALIC),
            )));
            for l in tail {
                out.push(Line::from(vec![
                    Span::styled("  │ ", Style::default().fg(OMARCHY.panel_line)),
                    Span::styled(crate::providers::truncate(&l, width - 6).to_string(), Style::default().fg(OMARCHY.muted).add_modifier(Modifier::ITALIC)),
                ]));
            }
        } else {
            out.push(Line::from(Span::styled(
                format!(" {} thinking… {secs}s {}", spinner(app.tick), crate::penguin::thinking_glyph(app.tick)),
                Style::default().fg(shimmer(app.tick)).add_modifier(Modifier::BOLD),
            )));
        }
    }
    out
}

fn empty_state_art(width: usize, tick: u32) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for a in crate::penguin::idle_frame(tick) {
        lines.push(Line::from(Span::styled(a, Style::default().fg(shimmer(tick)).add_modifier(Modifier::BOLD))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        crate::providers::truncate("agent mode — ask me anything · /goal <text> for autonomous work · /help", width)
            .to_string(),
        Style::default().fg(OMARCHY.muted),
    )));
    lines
}

fn crate_tick(app: &App) -> u32 {
    app.tick
}

fn render_transcript(app: &App, f: &mut Frame, area: Rect) {
    let width = area.width.max(8) as usize;
    let mut all = transcript_lines(app, width);
    let centered = all.is_empty();
    if centered {
        all = empty_state_art(width, app.tick);
    }
    let h = area.height as usize;
    let end = all.len().saturating_sub(app.agent_scroll);
    let start = end.saturating_sub(h);
    let visible = all[start..end].to_vec();
    let para = Paragraph::new(visible).alignment(if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    });
    f.render_widget(para, area);
}

fn render_composer(app: &mut App, f: &mut Frame, area: Rect) {
    let width = (area.width as usize).saturating_sub(4).max(1);
    let mut out: Vec<Line> = Vec::new();
    for (i, l) in composer_wrap(&app.composer, width).into_iter().enumerate() {
        let mut spans = vec![Span::styled(
            if i == 0 { "✻ ❯ ".to_string() } else { "    ".to_string() },
            Style::default().fg(shimmer(app.tick)).add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::styled(l, Style::default().fg(OMARCHY.text)));
        out.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(out), area);
    // real cursor at input position (clamped into the composer box)
    let (vr, vc) = composer_cursor(app.comp_cur, &app.composer, width);
    let x = area.x + 4 + vc as u16;
    let y = area.y + (vr as u16).min(area.height.saturating_sub(1));
    if x < area.x + area.width {
        f.set_cursor_position((x, y));
    }
}

/// Hard-wrap the composer text into visual lines: real newlines break lines,
/// long paragraphs wrap every `width` columns.
pub(crate) fn composer_wrap(text: &str, width: usize) -> Vec<String> {
    let w = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let chars: Vec<char> = para.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
        } else {
            for chunk in chars.chunks(w) {
                out.push(chunk.iter().collect());
            }
        }
    }
    out
}

/// (visual row, column) of the char offset `cur` within the wrapped composer.
pub(crate) fn composer_cursor(cur: usize, text: &str, width: usize) -> (usize, usize) {
    let w = width.max(1);
    let mut rem = cur.min(text.chars().count());
    let mut vline = 0usize;
    for para in text.split('\n') {
        let plen = para.chars().count();
        if rem <= plen {
            return (vline + rem / w, rem % w);
        }
        rem -= plen + 1;
        vline += (plen + w - 1) / w;
    }
    (vline, 0)
}

/// Number of rows the composer needs, including a trailing row when the
/// cursor sits just past a full-width line.
pub(crate) fn composer_height(cur: usize, text: &str, width: usize) -> usize {
    let w = width.max(1);
    let mut h = 0usize;
    for para in text.split('\n') {
        let plen = para.chars().count();
        h += ((plen + w - 1) / w).max(1);
    }
    if composer_cursor(cur, text, w).0 >= h {
        h += 1;
    }
    h.max(1)
}

fn render_plan(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(OMARCHY.panel_line))
        .title(Span::styled(" plan ", Style::default().fg(OMARCHY.accent)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let items: Vec<ListItem> = app
        .plan
        .iter()
        .map(|(label, status)| {
            let (icon, c) = match status.as_str() {
                "done" => ("●", OMARCHY.success),
                "active" => (spinner(app.tick), OMARCHY.accent),
                "failed" => ("✗", OMARCHY.danger),
                _ => ("○", OMARCHY.muted),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(c)),
                Span::styled(label.clone(), Style::default().fg(OMARCHY.text)),
            ]))
        })
        .collect();
    f.render_widget(List::new(items), inner);
}

fn render_permission(f: &mut Frame, area: Rect, sel: usize, title: &str, detail: &str) {
    let width = 76.min(area.width.saturating_sub(4));
    let detail_w = width as usize - 8;
    let detail_lines = wrap(detail, detail_w);
    let height = ((detail_lines.len() + 8) as u16).min(area.height.saturating_sub(2));
    let box_area = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, box_area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(OMARCHY.danger))
        .title(Span::styled(" ⚠ permission required ", Style::default().fg(OMARCHY.danger).add_modifier(Modifier::BOLD)))
        .style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x1c)));
    let inner = block.inner(box_area);
    f.render_widget(block, box_area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length((detail_lines.len() as u16 + 2).min(inner.height.saturating_sub(5))),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(Paragraph::new(Span::styled(title.to_string(), Style::default().fg(OMARCHY.warn).add_modifier(Modifier::BOLD))), chunks[0]);
    let detail_para = Paragraph::new(detail_lines.iter().map(|l| Line::from(l.clone())).collect::<Vec<_>>())
        .block(Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(OMARCHY.panel_line)))
        .style(Style::default().fg(OMARCHY.text));
    f.render_widget(detail_para, chunks[1]);

    let options = [
        ("1", "Allow once"),
        ("2", "Always allow (remember rule)"),
        ("3", "Never allow (remember rule)"),
    ];
    for (i, (key, label)) in options.iter().enumerate() {
        let selected = i == sel;
        let line = Line::from(vec![
            Span::styled(
                format!(" {} {} ", key, label),
                if selected {
                    Style::default().fg(Color::Rgb(0x14, 0x10, 0x20)).bg(shimmer(0)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(OMARCHY.text)
                },
            ),
        ]);
        let r = Rect { x: chunks[2].x + 1, y: chunks[2].y + i as u16, width: chunks[2].width, height: 1 };
        f.render_widget(Paragraph::new(line), r);
    }
    f.render_widget(
        Paragraph::new(Span::styled("↑↓ select · enter confirm · y/a/n shortcuts · esc = deny once", Style::default().fg(OMARCHY.muted))),
        chunks[3],
    );
}

/// Modal for entering a password a child command (sudo) is waiting on.
/// Only the typed length is ever rendered — never the characters themselves.
fn render_secret(f: &mut Frame, area: Rect, prompt: &str, typed: usize) {
    let width = 68.min(area.width.saturating_sub(4));
    let height = 9.min(area.height.saturating_sub(2));
    if width < 20 || height < 7 {
        return;
    }
    let box_area = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, box_area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(OMARCHY.agent_line))
        .title(Span::styled(" 🔑 password required ", Style::default().fg(OMARCHY.accent).add_modifier(Modifier::BOLD)))
        .style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x1c)));
    let inner = block.inner(box_area);
    f.render_widget(block, box_area);

    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Length(3), Constraint::Min(1)]).split(inner);
    let prompt_lines: Vec<Line> = wrap(prompt, inner.width.saturating_sub(2) as usize)
        .into_iter()
        .take(2)
        .map(|l| Line::from(Span::styled(l, Style::default().fg(OMARCHY.warn).add_modifier(Modifier::BOLD))))
        .collect();
    f.render_widget(Paragraph::new(prompt_lines), chunks[0]);

    let max_dots = inner.width.saturating_sub(6) as usize;
    let masked: String = "•".repeat(typed.min(max_dots));
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {masked}▌"),
            Style::default().fg(OMARCHY.text),
        )))
        .block(Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(OMARCHY.panel_line))),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            "enter submit · esc cancel — input hidden, never stored or shown",
            Style::default().fg(OMARCHY.muted),
        )),
        chunks[2],
    );
}
