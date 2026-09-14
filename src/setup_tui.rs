use crate::config::{Config, Provider, ProviderKind};
use crate::theme::*;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::io;

enum Screen {
    List,
    Editor(usize), // index into cfg.providers; usize::MAX = new
    Models(usize),
}

struct SetupApp {
    cfg: Config,
    screen: Screen,
    sel: usize,
    field: usize,
    status: String,
    ctx_buf: String,
}

const FIELDS: [&str; 5] = ["name", "kind ←/→", "base_url ⏎fetch", "api_key", "ctx tokens"];

impl SetupApp {
    fn cur(&mut self) -> &mut Provider {
        let i = match self.screen {
            Screen::Editor(i) => i,
            _ => 0,
        };
        &mut self.cfg.providers[i]
    }

    fn open_editor(&mut self, i: usize) {
        self.ctx_buf = self.cfg.providers[i]
            .context_window
            .map(|c| c.to_string())
            .unwrap_or_default();
        self.screen = Screen::Editor(i);
        self.field = 0;
    }

    fn commit_ctx(&mut self) {
        if let Screen::Editor(i) = self.screen {
            if let Ok(v) = self.ctx_buf.trim().parse::<usize>() {
                if v >= 1024 {
                    self.cfg.providers[i].context_window = Some(v);
                }
            }
        }
    }
}

pub fn run_setup(cfg: &Config) -> Result<Config> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = SetupApp {
        cfg: cfg.clone(),
        screen: Screen::List,
        sel: 0,
        field: 0,
        status: String::new(),
        ctx_buf: String::new(),
    };
    let result: Result<()> = loop {
        terminal.draw(|f| draw(&mut app, f))?;
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match app.screen {
                Screen::List => match k.code {
                    KeyCode::Esc | KeyCode::Char('q') => break Ok(()),
                    KeyCode::Up => app.sel = app.sel.saturating_sub(1),
                    KeyCode::Down => {
                        if !app.cfg.providers.is_empty() {
                            app.sel = (app.sel + 1).min(app.cfg.providers.len() - 1);
                        }
                    }
                    KeyCode::Enter => {
                        if !app.cfg.providers.is_empty() {
                            let sel = app.sel;
                            app.open_editor(sel);
                        }
                    }
                    KeyCode::Char('a') | KeyCode::Char('n') => {
                        app.cfg.providers.push(Provider {
                            name: String::new(),
                            kind: ProviderKind::Openai,
                            base_url: "https://api.openai.com/v1".into(),
                            api_key: "".into(),
                            models: vec![],
                            default_model: None,
                            context_window: None,
                        });
                        app.sel = app.cfg.providers.len() - 1;
                        app.open_editor(app.sel);
                        app.status = "give this provider a name →".into();
                    }
                    KeyCode::Char('s') => {
                        if let Some(p) = app.cfg.providers.get(app.sel) {
                            if p.name.trim().is_empty() {
                                app.status = "name it first (enter to edit)".into();
                            } else {
                                app.cfg.active_provider = Some(p.name.clone());
                                app.status = format!("active → {}", p.name);
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        if !app.cfg.providers.is_empty() {
                            app.cfg.providers.remove(app.sel);
                            app.sel = app.sel.min(app.cfg.providers.len().saturating_sub(1));
                            app.status = "deleted".into();
                        }
                    }
                    _ => {}
                },
                Screen::Editor(_) => match k.code {
                    KeyCode::Esc => {
                        app.commit_ctx();
                        app.screen = Screen::List;
                    }
                    KeyCode::Tab | KeyCode::Down => app.field = (app.field + 1) % FIELDS.len(),
                    KeyCode::BackTab | KeyCode::Up => app.field = (app.field + FIELDS.len() - 1) % FIELDS.len(),
                    KeyCode::Left | KeyCode::Right if app.field == 1 => {
                        let p = app.cur();
                        p.kind = match p.kind {
                            ProviderKind::Openai => ProviderKind::Anthropic,
                            ProviderKind::Anthropic => ProviderKind::Openai,
                        };
                    }
                    KeyCode::Char('f') if k.modifiers.contains(KeyModifiers::CONTROL) || app.field == 99 => {}
                    KeyCode::Enter if app.field == 2 => {
                        // fetch models
                        let idx = match app.screen {
                            Screen::Editor(i) => i,
                            _ => 0,
                        };
                        let p = app.cfg.providers[idx].clone();
                        app.status = "fetching models…".into();
                        terminal.draw(|f| draw(&mut app, f))?;
                        let t0 = std::time::Instant::now();
                        match crate::providers::fetch_models(&p) {
                            Ok(models) => {
                                app.status = format!("{} models found · endpoint answered in {:.1}s", models.len(), t0.elapsed().as_secs_f64());
                                app.cfg.providers[idx].models = models;
                                app.screen = Screen::Models(idx);
                                app.sel = 0;
                            }
                            Err(e) => app.status = format!("fetch failed after {:.1}s: {e}", t0.elapsed().as_secs_f64()),
                        }
                    }
                    KeyCode::Char('t') if !matches!(app.field, 0 | 2 | 3) => {
                        // ping the default model: measures time-to-first-token,
                        // which for slow endpoints is dominated by model load.
                        let idx = match app.screen {
                            Screen::Editor(i) => i,
                            _ => 0,
                        };
                        let p = app.cfg.providers[idx].clone();
                        let model = p.default_model.clone().or_else(|| p.models.first().cloned());
                        match model {
                            None => app.status = "no model selected — press m to pick one".into(),
                            Some(m) => {
                                app.status = format!("pinging “{m}” — measuring time to first token (up to 2 min)…");
                                terminal.draw(|f| draw(&mut app, f))?;
                                let rcfg = crate::providers::RequestCfg {
                                    first_byte: std::time::Duration::from_secs(120),
                                    idle: std::time::Duration::from_secs(60),
                                    retries: 0,
                                    ..Default::default()
                                };
                                let abort = std::sync::atomic::AtomicBool::new(false);
                                let t0 = std::time::Instant::now();
                                let first: std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>> =
                                    std::sync::Arc::new(std::sync::Mutex::new(None));
                                let req = vec![crate::providers::ChatMsg::user("Reply with the single word: pong")];
                                let res = {
                                    let f1 = first.clone();
                                    let f2 = first.clone();
                                    let mut ctx = crate::providers::StreamCtx {
                                        on_text: &mut move |_| {
                                            let mut g = f1.lock().unwrap();
                                            if g.is_none() {
                                                *g = Some(std::time::Instant::now());
                                            }
                                        },
                                        on_reasoning: &mut move |_| {
                                            if f2.lock().unwrap().is_none() {
                                                *f2.lock().unwrap() = Some(std::time::Instant::now());
                                            }
                                        },
                                        on_note: &mut |_| {},
                                        abort: &abort,
                                    };
                                    crate::providers::chat(&p, &m, &req, &[], &rcfg, &mut ctx)
                                };
                                let secs = t0.elapsed().as_secs_f64();
                                match res {
                                    Ok(r) => {
                                        let ttft = first.lock().unwrap()
                                            .map(|f| f.duration_since(t0).as_secs_f64())
                                            .unwrap_or(secs);
                                        app.status = format!("✓ “{m}” alive · first token {ttft:.1}s · {} chars out · total {secs:.1}s", r.text.chars().count());
                                    }
                                    Err(e) => app.status = format!("ping failed after {secs:.1}s: {e}"),
                                }
                            }
                        }
                    }
                    KeyCode::Char('m') if !matches!(app.field, 0 | 2 | 3) => {
                        let idx = match app.screen {
                            Screen::Editor(i) => i,
                            _ => 0,
                        };
                        if !app.cfg.providers[idx].models.is_empty() {
                            app.screen = Screen::Models(idx);
                            app.sel = 0;
                        } else {
                            app.status = "no models cached — set base_url and press Enter there".into();
                        }
                    }
                    KeyCode::Char(c) if app.field == 4 && c.is_ascii_digit() => {
                        if app.ctx_buf.chars().count() < 8 {
                            app.ctx_buf.push(c);
                        }
                    }
                    KeyCode::Char(c) if (app.field == 0 || app.field == 2 || app.field == 3) && !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        let f = app.field;
                        let idx = match app.screen { Screen::Editor(i) => i, _ => 0 };
                        let p = &mut app.cfg.providers[idx];
                        let s = match f {
                            0 => &mut p.name,
                            2 => &mut p.base_url,
                            _ => &mut p.api_key,
                        };
                        s.push(c);
                    }
                    KeyCode::Backspace if app.field == 4 => {
                        app.ctx_buf.pop();
                    }
                    KeyCode::Backspace if app.field == 0 || app.field == 2 || app.field == 3 => {
                        let f = app.field;
                        let idx = match app.screen { Screen::Editor(i) => i, _ => 0 };
                        let p = &mut app.cfg.providers[idx];
                        let s = match f {
                            0 => &mut p.name,
                            2 => &mut p.base_url,
                            _ => &mut p.api_key,
                        };
                        s.pop();
                    }
                    _ => {}
                },
                Screen::Models(idx) => {
                    let n = app.cfg.providers[idx].models.len();
                    match k.code {
                        KeyCode::Esc => {
                            app.screen = Screen::Editor(idx);
                        }
                        KeyCode::Up => app.sel = app.sel.saturating_sub(1),
                        KeyCode::Down if n > 0 => app.sel = (app.sel + 1).min(n - 1),
                        KeyCode::Char(' ') => {
                            // toggle default marker = just set default model
                            let m = app.cfg.providers[idx].models[app.sel].clone();
                            app.cfg.providers[idx].default_model = Some(m);
                            app.status = "default model set".into();
                        }
                        KeyCode::Enter => {
                            let m = app.cfg.providers[idx].models[app.sel].clone();
                            let p = &mut app.cfg.providers[idx];
                            if !p.models.contains(&m) {
                                p.models.push(m.clone());
                            }
                            p.default_model = Some(m);
                            app.screen = Screen::Editor(idx);
                        }
                        KeyCode::Char('d') => {
                            let m = app.cfg.providers[idx].models[app.sel].clone();
                            let p = &mut app.cfg.providers[idx];
                            p.models.retain(|x| x != &m);
                            if p.default_model.as_ref() == Some(&m) {
                                p.default_model = p.models.first().cloned();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    };
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    result?;
    app.cfg.normalize();
    app.cfg.dedup_names();
    app.cfg.save()?;
    Ok(app.cfg)
}

fn draw(app: &mut SetupApp, f: &mut Frame) {
    let area = f.area();
    f.buffer_mut().set_style(area, ratatui::style::Style::default().bg(OMARCHY.bg));
    match app.screen {
        Screen::List => draw_list(app, f, area),
        Screen::Editor(_) => draw_editor(app, f, area),
        Screen::Models(i) => draw_models(app, f, area, i),
    }
}

/// Always-visible key bar: [KEY] action · [KEY] action …
fn keybar(f: &mut Frame, area: Rect, pairs: &[(&str, &str)]) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, d)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().bg(Color::Rgb(0x1c, 0x1c, 0x26))));
        }
        spans.push(Span::styled(format!(" {k} "), Style::default().fg(Color::Rgb(0x14, 0x10, 0x20)).bg(OMARCHY.accent).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {d} "), Style::default().fg(OMARCHY.text).bg(Color::Rgb(0x1c, 0x1c, 0x26))));
    }
    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(0x1c, 0x1c, 0x26)));
    f.render_widget(Clear, area);
    f.render_widget(bar, area);
}

fn centered(r: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(r.width.saturating_sub(2));
    let h = h.min(r.height.saturating_sub(2));
    Rect {
        x: r.x + (r.width.saturating_sub(w)) / 2,
        y: r.y + (r.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn draw_list(app: &SetupApp, f: &mut Frame, area: Rect) {
    let block = Layout::vertical([Constraint::Length(3), Constraint::Min(5), Constraint::Length(2)]).split(area);
    let title = Paragraph::new(vec![Line::from(vec![
        Span::styled(" 🐧 penguin command ", Style::default().fg(OMARCHY.accent).add_modifier(Modifier::BOLD)),
        Span::styled("  providers", Style::default().fg(OMARCHY.muted)),
    ])])
    .style(Style::default().bg(OMARCHY.bg));
    f.render_widget(title, block[0]);

    let items: Vec<ListItem> = app
        .cfg
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let active = app.cfg.active_provider.as_deref() == Some(p.name.as_str());
            ListItem::new(Line::from(vec![
                Span::styled(
                    if active { " ● " } else { " ○ " },
                    Style::default().fg(if active { OMARCHY.success } else { OMARCHY.muted }),
                ),
                Span::styled(format!("{:<20}", p.name), Style::default().fg(OMARCHY.text).add_modifier(if i == app.sel { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(format!("{:<18}", p.kind.label()), Style::default().fg(OMARCHY.accent2)),
                Span::styled(&p.base_url, Style::default().fg(OMARCHY.muted)),
            ]))
        })
        .collect();
    let list = List::new(items).highlight_style(Style::default().bg(Color::Rgb(0x23, 0x23, 0x30)))
        .highlight_symbol("▸ ")
        .block(Block::bordered().border_type(ratatui::widgets::BorderType::Rounded).border_style(Style::default().fg(OMARCHY.panel_line)));
    f.render_widget(list, block[1]);

    keybar(
        f,
        block[2],
        &[("↑↓","move"),("enter","edit"),("a","add"),("s","set active"),("d","delete"),("q","quit & save")],
    );
    if !app.status.is_empty() {
        let p = Paragraph::new(Span::styled(app.status.clone(), Style::default().fg(OMARCHY.success)));
        f.render_widget(p, Rect { x: area.x + 2, y: area.height.saturating_sub(3), width: area.width, height: 1 });
    }
}

fn draw_editor(app: &SetupApp, f: &mut Frame, area: Rect) {
    let p = match &app.screen {
        Screen::Editor(i) => &app.cfg.providers[*i],
        _ => return,
    };
    let box_area = centered(area, 72, 13);
    f.render_widget(Clear, box_area);
    let block = Block::bordered().border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(OMARCHY.agent_line))
        .title(Span::styled(" provider ", Style::default().fg(OMARCHY.accent)));
    let inner_area = block.inner(box_area);
    f.render_widget(block, box_area);
    let inner = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner_area);
    let cursor = |i: usize, s: String| if app.field == i { format!("{s}▏") } else { s };
    let vals = [
        cursor(0, p.name.clone()),
        p.kind.label().to_string(),
        cursor(2, p.base_url.clone()),
        mask(&p.api_key),
        cursor(4, app.ctx_buf.clone()),
    ];
    for (i, label) in FIELDS.iter().enumerate() {
        let selected = i == app.field;
        let line = Line::from(vec![
            Span::styled(format!(" {:>15} ", label), Style::default().fg(if selected { OMARCHY.accent } else { OMARCHY.muted }).add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled(vals[i].clone(), Style::default().fg(OMARCHY.text).bg(if selected { Color::Rgb(0x23, 0x23, 0x30) } else { OMARCHY.panel })),
        ]);
        f.render_widget(Paragraph::new(line), inner[i]);
    }
    keybar(
        f,
        inner[6],
        &[("tab","next field"),("enter@url","fetch models"),("m","models"),("t","ping model (latency)"),("esc","back (saved)")],
    );
    if !app.status.is_empty() {
        let s = Paragraph::new(Span::styled(app.status.clone(), Style::default().fg(OMARCHY.warn)));
        f.render_widget(s, inner[5]);
    }
}

fn draw_models(app: &SetupApp, f: &mut Frame, area: Rect, idx: usize) {
    let p = &app.cfg.providers[idx];
    let box_area = centered(area, 72, (p.models.len() as u16 + 4).min(area.height.saturating_sub(2)));
    f.render_widget(Clear, box_area);
    let inner = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(box_area);
    f.render_widget(
        Paragraph::new(Span::styled(format!(" models for {} — enter: select  d: remove", p.name), Style::default().fg(OMARCHY.accent))),
        inner[0],
    );
    let items: Vec<ListItem> = p
        .models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let def = p.default_model.as_deref() == Some(m.as_str());
            ListItem::new(Line::from(vec![
                Span::styled(if def { " ★ " } else { "   " }, Style::default().fg(OMARCHY.warn)),
                Span::styled(m.clone(), Style::default().fg(OMARCHY.text).add_modifier(if i == app.sel { Modifier::BOLD } else { Modifier::empty() })),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▸ ")
        .highlight_style(Style::default().bg(Color::Rgb(0x23, 0x23, 0x30)));
    f.render_widget(list, inner[1]);
    keybar(f, inner[2], &[("↑↓","move"),("enter","use as default"),("d","remove"),("esc","back")]);
    let block = Block::bordered().border_type(ratatui::widgets::BorderType::Rounded).border_style(Style::default().fg(OMARCHY.panel_line));
    f.render_widget(block, box_area);
}

fn mask(k: &str) -> String {
    if k.is_empty() {
        "(none)".into()
    } else if k.starts_with('$') {
        k.to_string()
    } else {
        format!("••••{}", k.chars().rev().take(3).collect::<String>().chars().rev().collect::<String>())
    }
}

use ratatui::style::{Color, Modifier, Style};
