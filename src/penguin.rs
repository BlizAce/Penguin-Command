//! Penguin art, animation frames, and the idle screensaver scenes.
//!
//! The original sprite (`@..@` over a wide `(----)` mouth) read as a frog:
//! eyes on top of a broad grin. These sprites use the classic penguin shape —
//! small round head with a tiny beak between the eyes, plump belly, side
//! flippers, and webbed feet.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

/// The classic upright penguin (11 × 7). Beak sits between the eyes (`o_o`),
/// flippers angle out at the waist, webbed feet at the base.
pub const CLASSIC: [&str; 7] = [
    "   .--.    ",
    "  |o_o |   ",
    "  |:_/ |   ",
    " //   \\ \\  ",
    "(|     | ) ",
    "/'\\_-_/`\\  ",
    "\\___)=(___/",
];

/// Same penguin with both flippers raised (waving / celebrating).
pub const CLASSIC_ARMS_UP: [&str; 7] = [
    "   .--.    ",
    "  |o_o |   ",
    "  |:_/ |   ",
    " \\\\   / /  ",
    "(|\\___/|)  ",
    "/'\\_-_/`\\  ",
    "\\___)=(___/",
];

fn swap_line3(s: &str, eyes: &str) -> String {
    s.replacen("o_o", eyes, 1)
}

/// Idle waddle cycle: sway left/right (shifted sprite), blink, and the
/// occasional happy squint. Returns one frame of 7 lines.
pub fn idle_frame(tick: u32) -> Vec<String> {
    // 8-frame cycle: walk · blink · walk · happy-squint
    let phase = (tick / 5) % 8;
    let shifted = phase % 2 == 1;
    let eyes = match phase {
        2 | 3 => "-_-",
        6 | 7 => ">_<",
        _ => "o_o",
    };
    CLASSIC
        .iter()
        .map(|l| {
            let l = swap_line3(l, eyes);
            if shifted {
                format!(" {l}")
            } else {
                l
            }
        })
        .collect()
}

/// Flipper-wave cycle for cheering penguins (goal verified, bystanders).
pub fn wave_frame(tick: u32) -> Vec<String> {
    let up = (tick / 4) % 2 == 0;
    let src = if up { &CLASSIC_ARMS_UP } else { &CLASSIC };
    src.iter().map(|l| l.to_string()).collect()
}

/// Belly-slide sprite: horizontal, beak forward (`>`), riding a spray of snow.
pub fn slide_frame(tick: u32) -> Vec<String> {
    match (tick / 6) % 2 {
        0 => vec![
            "             .--.       ".to_string(),
            "   ~~       |o_o \\      ".to_string(),
            "  ~~~  __.--|:_/ >      ".to_string(),
            " ~~~~~/______________    ".to_string(),
        ],
        _ => vec![
            "             .--.       ".to_string(),
            "    ~~      |o_o \\      ".to_string(),
            "   ~~~  __.--|:_/ >     ".to_string(),
            "~~~~~~/______________   ".to_string(),
        ],
    }
}

/// One-line penguin that waddles along a dotted track — used for the
/// "agent is working" indicator.
pub fn thinking_glyph(tick: u32) -> String {
    const TRACKS: [&str; 8] = [
        "(o_o)<···",
        "·(o_o)<··",
        "··(o_o)<·",
        "···(o_o)<",
        ">o_o)····",
        ">(-_-)···",
        "·>(o_o)·",
        "··>(o_o)",
    ];
    TRACKS[((tick / 3) as usize) % TRACKS.len()].to_string()
}

/// Welcome art for the agent transcript (sprite + greeting lines).
pub fn welcome_lines() -> Vec<String> {
    vec![
        "   .--.".into(),
        "  |o_o |      penguin command · agent online".into(),
        "  |:_/ |      ask me anything, or hand me a /goal".into(),
        " //   \\ \\     Ctrl+Space returns you to the shell".into(),
        "(|     | )".into(),
        "/'\\_-_/`\\".into(),
        "\\___)=(___/".into(),
    ]
}

/// Celebration art shown when a goal verifies.
pub fn victory_lines() -> Vec<String> {
    vec![
        "   .--.       goal verified!".into(),
        "  |^_^ |      ★ the penguin approves ★".into(),
        "  |:_/ |".into(),
        " \\\\   / /".into(),
        "(|\\___/|)".into(),
        "/'\\_-_/`\\".into(),
        "\\___)=(___/".into(),
    ]
}

// ---------------------------------------------------------------------------
// Idle screensaver (in the spirit of omarchy-screensaver: ASCII art cycling
// through "random effects", any key exits).
// ---------------------------------------------------------------------------

pub const SCENE_COUNT: usize = 3;

pub fn pick_scene(seed: u64) -> usize {
    (seed.wrapping_mul(6364136223846793005) >> 33) as usize % SCENE_COUNT
}

fn hash(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263);
    h = h.wrapping_mul(h ^ (h >> 13));
    h ^ (h >> 16)
}

fn put(f: &mut Frame, area: Rect, x: u16, y: u16, c: char, s: Style) {
    if x < area.x + area.width && y < area.y + area.height {
        let cell = &mut f.buffer_mut()[(x, y)];
        cell.set_char(c);
        cell.set_style(s);
    }
}

fn put_sprite(f: &mut Frame, area: Rect, x: i32, y: u16, sprite: &[String], s: Style) {
    for (r, line) in sprite.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let px = x + c as i32;
            if px < area.x as i32 {
                continue;
            }
            put(f, area, px as u16, y.saturating_add(r as u16), ch, s);
        }
    }
}

fn shimmer_color(tick: u32, offset: usize) -> Color {
    const STEPS: [Color; 8] = [
        Color::Rgb(0xcb, 0xa6, 0xf7),
        Color::Rgb(0xd3, 0xb1, 0xf9),
        Color::Rgb(0xf5, 0xc9, 0xe1),
        Color::Rgb(0xba, 0x9b, 0xfe),
        Color::Rgb(0x94, 0xe2, 0xd8),
        Color::Rgb(0x89, 0xb4, 0xfa),
        Color::Rgb(0xf9, 0xe2, 0xaf),
        Color::Rgb(0xc4, 0xb3, 0xf5),
    ];
    STEPS[((tick / 3 + offset as u32) as usize) % STEPS.len()]
}

fn snow(f: &mut Frame, area: Rect, tick: u32, density: u32) {
    const FLAKES: [char; 5] = ['·', '✦', '*', '◦', '∙'];
    for col in 0..area.width {
        let h = hash(col as u32, 7);
        if h % density != 0 {
            continue;
        }
        let speed = 1 + (h >> 8) % 3;
        let y = ((h >> 4) + tick * speed / 2) % area.height.max(1) as u32;
        let drift = (((tick * speed) / 6 + (h >> 12)) % 5) as i32 - 2;
        let x = col as i32 + drift;
        if x < 0 || x >= area.width as i32 {
            continue;
        }
        let dim = (h >> 20) % 3 == 0;
        put(
            f,
            area,
            x as u16,
            y as u16,
            FLAKES[(h >> 3) as usize % FLAKES.len()],
            Style::default().fg(if dim { Color::Rgb(0x4a, 0x50, 0x68) } else { Color::Rgb(0xcd, 0xd6, 0xf4) }),
        );
    }
}

/// Scene 0 — blizzard stroll: three penguins waddle across a snowfield.
fn scene_stroll(f: &mut Frame, area: Rect, tick: u32) {
    snow(f, area, tick, 4);
    let ground_y = (area.y + area.height).saturating_sub(2);
    for x in 0..area.width {
        let h = hash(x as u32, 99);
        put(f, area, x, ground_y, if h % 7 == 0 { '▂' } else { '▁' }, Style::default().fg(Color::Rgb(0x3a, 0x40, 0x58)));
    }
    let base = ground_y.saturating_sub(CLASSIC.len() as u16);
    for i in 0..3u32 {
        let speed = if i == 1 { 3 } else { 2 };
        let span = (area.width + 24).max(25) as u32;
        let x = ((tick / speed + i * (span / 3 + 4)) % span) as i32 - 12;
        let sprite: Vec<String> = idle_frame(tick + i * 3);
        put_sprite(f, area, x, base, &sprite, Style::default().fg(shimmer_color(tick, i as usize)).add_modifier(Modifier::BOLD));
    }
}

fn mix(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t).round().clamp(0.0, 255.0) as u8
}

/// Scene 1 — aurora ridge: shimmering sky, stars, penguins on the ice.
fn scene_aurora(f: &mut Frame, area: Rect, tick: u32) {
    let band = (area.height / 3).max(4);
    for y in 0..band {
        for x in 0..area.width {
            let v = ((x as f64 * 0.11 + (y as f64) * 0.35 + tick as f64 * 0.07).sin()
                + (x as f64 * 0.05 - tick as f64 * 0.045).sin())
                / 2.0;
            if v > 0.15 {
                let t = ((v - 0.15) / 0.85).min(1.0);
                let r = mix(0x6c, 0xcb, t);
                let g = mix(0xb4, 0xa6, t);
                let b = mix(0xfa, 0xf7, t);
                let ch = if v > 0.8 { '▓' } else if v > 0.5 { '▒' } else { '░' };
                put(f, area, x, y + area.y, ch, Style::default().fg(Color::Rgb(r, g, b)));
            }
        }
    }
    for i in 0..24u32 {
        let h = hash(i, 13);
        let x = (h % area.width.max(1) as u32) as u16;
        let y = ((h >> 8) % band.saturating_mul(2).max(1) as u32) as u16;
        if (tick / 4 + i) % (6 + h % 7) < 4 {
            put(f, area, x, area.y + y, '✦', Style::default().fg(Color::Rgb(0xba, 0xc2, 0xfe)).add_modifier(Modifier::DIM));
        }
    }
    let ground_y = (area.y + area.height).saturating_sub(2);
    for x in 0..area.width {
        let ridge = ((x as f64 * 0.18).sin() * 2.0).round() as i32;
        put(f, area, x, (ground_y as i32 + ridge).clamp(area.y as i32, ground_y as i32) as u16, '▄', Style::default().fg(Color::Rgb(0x58, 0x5b, 0x70)));
    }
    let base = ground_y.saturating_sub(CLASSIC.len() as u16 + 2);
    for i in 0..4u32 {
        let x = area.x as i32 + (area.width.saturating_sub(48)) as i32 / 2 + i as i32 * 11;
        let sprite = if i == 1 || i == 2 { wave_frame(tick + i * 2) } else { idle_frame(tick + i * 5) };
        put_sprite(f, area, x, base, &sprite, Style::default().fg(shimmer_color(tick, (i * 2) as usize)).add_modifier(Modifier::BOLD));
    }
}

/// Scene 2 — belly slide: one penguin surfs across, two cheer it on.
fn scene_slide(f: &mut Frame, area: Rect, tick: u32) {
    snow(f, area, tick, 9);
    let ground_y = (area.y + area.height).saturating_sub(3);
    for x in 0..area.width {
        put(f, area, x, ground_y, '▁', Style::default().fg(Color::Rgb(0x3a, 0x40, 0x58)));
    }
    let span = (area.width + 26).max(30) as u32;
    let x = ((tick / 2) % span) as i32 + area.x as i32 - 15;
    put_sprite(f, area, x, ground_y.saturating_sub(4), &slide_frame(tick), Style::default().fg(Color::Rgb(0x94, 0xe2, 0xd8)).add_modifier(Modifier::BOLD));
    let cheer_x = (area.x + area.width).saturating_sub(26) as i32;
    for i in 0..2u32 {
        put_sprite(
            f,
            area,
            cheer_x + i as i32 * 12,
            ground_y.saturating_sub(CLASSIC.len() as u16),
            &wave_frame(tick + i * 3),
            Style::default().fg(shimmer_color(tick, i as usize + 4)).add_modifier(Modifier::BOLD),
        );
    }
}

pub fn render_screensaver(scene: usize, tick: u32, f: &mut Frame, area: Rect) {
    match scene % SCENE_COUNT {
        1 => scene_aurora(f, area, tick),
        2 => scene_slide(f, area, tick),
        _ => scene_stroll(f, area, tick),
    }
    draw_caption(f, area, tick);
}

fn draw_caption(f: &mut Frame, area: Rect, tick: u32) {
    if area.height < 8 || area.width < 40 {
        return;
    }
    let title = "🐧 PENGUIN COMMAND 🐧";
    let y = area.y + area.height / 5;
    let mut x = area.x + area.width.saturating_sub(title.chars().count() as u16) / 2;
    for (i, ch) in title.chars().enumerate() {
        if x >= area.x + area.width {
            break;
        }
        put(f, area, x, y, ch, Style::default().fg(shimmer_color(tick, i)).add_modifier(Modifier::BOLD));
        x += 1;
    }
    let hint = "idle · any key to wake";
    let hx = area.x + area.width.saturating_sub(hint.chars().count() as u16) / 2;
    put_line(f, area, hx, (area.y + area.height).saturating_sub(1), hint, Style::default().fg(Color::Rgb(0x6c, 0x70, 0x86)).add_modifier(Modifier::ITALIC));
}

fn put_line(f: &mut Frame, area: Rect, x: u16, y: u16, s: &str, style: Style) {
    let mut cx = x;
    for ch in s.chars() {
        if cx >= area.x + area.width {
            break;
        }
        put(f, area, cx, y, ch, style);
        cx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn sprites_have_consistent_frames() {
        assert_eq!(CLASSIC.len(), 7);
        assert_eq!(CLASSIC_ARMS_UP.len(), 7);
        for t in [0u32, 5, 11, 40, 999] {
            assert_eq!(idle_frame(t).len(), 7);
            assert_eq!(wave_frame(t).len(), 7);
            assert_eq!(slide_frame(t).len(), 4);
        }
    }

    #[test]
    fn no_frog_left_in_sprites() {
        let all = CLASSIC.concat();
        assert!(!all.contains("@..@"));
        assert!(!all.contains("(----)"));
        // the penguin must have a beak between its eyes and webbed feet
        assert!(CLASSIC[1].contains("o_o"));
        assert!(CLASSIC[6].contains(")="));
    }

    #[test]
    fn scenes_render_on_many_terminals() {
        for (w, h) in [(120u16, 34u16), (80, 24), (40, 12), (20, 8), (5, 3)] {
            for scene in 0..SCENE_COUNT {
                for tick in [0u32, 7, 42, 999] {
                    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                    let _ = terminal.draw(|f| render_screensaver(scene, tick, f, f.area()));
                    let buf = terminal.backend().buffer().clone();
                    let nonblank = buf.content().iter().filter(|c| !c.symbol().trim().is_empty()).count();
                    if w >= 40 && h >= 12 {
                        assert!(nonblank > 30, "scene {scene} at {w}x{h} t{tick} looks empty ({nonblank})");
                    }
                }
            }
        }
    }

    #[test]
    fn stroll_scene_shows_a_penguin_face() {
        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        let _ = terminal.draw(|f| scene_stroll(f, f.area(), 60));
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol().chars().next().unwrap_or(' ')).collect();
        assert!(text.contains("o_o") || text.contains("-_-") || text.contains(">_<"), "no penguin face found in stroll scene");
    }

    #[test]
    fn print_frames_for_eyeballing() {
        // cargo test -- --nocapture to inspect the art
        println!("idle:");
        for l in idle_frame(0) {
            println!("[{l}]");
        }
        println!("idle shifted:");
        for l in idle_frame(5) {
            println!("[{l}]");
        }
        println!("arms up:");
        for l in &CLASSIC_ARMS_UP {
            println!("[{l}]");
        }
        println!("slide:");
        for l in slide_frame(0) {
            println!("[{l}]");
        }
        println!("thinking: {}", thinking_glyph(0));
    }
}
