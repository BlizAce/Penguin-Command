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
// Idle screensaver (in the spirit of omarchy-screensaver: a big block-ASCII
// wordmark run through random terminal text effects, plus the classic penguin
// scenes, auto-cycled by an inverse-frequency picker; any key exits).
// ---------------------------------------------------------------------------

pub const SCENE_COUNT: usize = 3;
pub const EFFECT_COUNT: usize = 6;
/// Pool entries: 0..SCENE_COUNT are penguin scenes, the rest are wordmark
/// effects (see `render_screensaver`).
pub const ENTRY_COUNT: usize = SCENE_COUNT + EFFECT_COUNT;

pub fn entry_name(entry: usize) -> &'static str {
    match entry % ENTRY_COUNT {
        0 => "blizzard stroll",
        1 => "aurora ridge",
        2 => "belly slide",
        3 => "matrix rain",
        4 => "scramble decode",
        5 => "glitch",
        6 => "wave sway",
        7 => "fire glow",
        _ => "shimmer sweep",
    }
}

/// How long each entry holds the screen before the picker rotates, in ticks
/// (~30fps while screensaving). Scramble gets the longest window: its reveal
/// is the show.
pub fn entry_duration(entry: usize) -> u32 {
    const DURATIONS: [u32; ENTRY_COUNT] = [540, 600, 420, 600, 540, 360, 540, 540, 480];
    DURATIONS[entry % ENTRY_COUNT]
}

/// Omarchy's inverse-frequency picker: each pick bumps that entry's count so
/// it becomes less likely next time, and the last entry is never chosen twice
/// in a row. Distribution evens out naturally over long idles.
pub struct EffectPicker {
    counts: [u32; ENTRY_COUNT],
    last: Option<usize>,
    rng: u64,
}

impl EffectPicker {
    pub fn new(seed: u64) -> Self {
        Self { counts: [0; ENTRY_COUNT], last: None, rng: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn pick(&mut self) -> usize {
        let pool: Vec<usize> = (0..ENTRY_COUNT).filter(|e| Some(*e) != self.last).collect();
        let total_w: u32 = pool.iter().map(|e| 1000 / (self.counts[*e] + 1)).sum();
        let mut ticket = (self.next_u64() % total_w as u64) as u32;
        let mut chosen = *pool.last().unwrap();
        for &e in &pool {
            let w = 1000 / (self.counts[e] + 1);
            if ticket < w {
                chosen = e;
                break;
            }
            ticket -= w;
        }
        self.counts[chosen] += 1;
        self.last = Some(chosen);
        chosen
    }
}

// ---------------------------------------------------------------------------
// Wordmark: "PENGUIN" in chunky half-block pixels, rasterized once per layout
// into a mask the text effects can paint against.
// ---------------------------------------------------------------------------

const FONT5X7: [(char, [&str; 7]); 6] = [
    ('P', ["11110", "10001", "10001", "11110", "10000", "10000", "10000"]),
    ('E', ["11111", "10000", "10000", "11110", "10000", "10000", "11111"]),
    ('N', ["10001", "11001", "10101", "10011", "10001", "10001", "10001"]),
    ('G', ["01110", "10001", "10000", "10111", "10001", "10001", "01110"]),
    ('U', ["10001", "10001", "10001", "10001", "10001", "10001", "01110"]),
    ('I', ["01110", "00100", "00100", "00100", "00100", "00100", "01110"]),
];

const WORD: &str = "PENGUIN";

/// Centered placement of the wordmark inside `area`, with a per-pixel mask.
struct Wordmark {
    ox: u16,
    oy: u16,
    w: u16,
    h: u16,
    /// rows[y][x] = pixel set; row-major, `w` × `h`.
    rows: Vec<Vec<bool>>,
}

impl Wordmark {
    fn layout(area: Rect) -> Self {
        let scale: usize = if area.width >= 100 && area.height >= 20 { 2 } else { 1 };
        let gw = 5; // glyph cell width
        let gh = 7;
        let total_w = WORD.chars().count() * (gw + 1) - 1;
        let w = (total_w * scale) as u16;
        let h = (gh * scale) as u16;
        let ox = area.x + area.width.saturating_sub(w) / 2;
        let oy = area.y + area.height.saturating_sub(h) / 2;
        let mut rows = vec![vec![false; w as usize]; h as usize];
        for (gi, ch) in WORD.chars().enumerate() {
            let glyph = FONT5X7.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g).unwrap_or(FONT5X7[0].1);
            for (py, row) in glyph.iter().enumerate() {
                for (px, bit) in row.chars().enumerate() {
                    if bit != '1' {
                        continue;
                    }
                    let bx = gi * (gw + 1) + px;
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = bx * scale + sx;
                            let y = py * scale + sy;
                            if x < w as usize && y < h as usize {
                                rows[y][x] = true;
                            }
                        }
                    }
                }
            }
        }
        Self { ox, oy, w, h, rows }
    }

    #[inline]
    fn inside(&self, lx: i32, ly: i32) -> bool {
        lx >= 0 && ly >= 0 && (lx as usize) < self.w as usize && (ly as usize) < self.h as usize
    }

    #[inline]
    fn at(&self, lx: usize, ly: usize) -> bool {
        self.rows[ly][lx]
    }

    /// Paint a character at wordmark-local coordinates.
    #[inline]
    fn put_local(&self, f: &mut Frame, area: Rect, lx: i32, ly: i32, c: char, s: Style) {
        if !self.inside(lx, ly) {
            return;
        }
        let x = self.ox as i32 + lx;
        let y = self.oy as i32 + ly;
        if x < 0 || y < 0 {
            return;
        }
        put(f, area, x as u16, y as u16, c, s);
    }

    fn for_each_pixel(&self, mut cb: impl FnMut(usize, usize)) {
        for y in 0..self.h as usize {
            for x in 0..self.w as usize {
                if self.rows[y][x] {
                    cb(x, y);
                }
            }
        }
    }
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

/// A colourful aurora band across the top of the sky — painted behind every
/// outdoor penguin scene. Green↔teal ribbons cross-fade into violet, drifting
/// with time; `dim` scales brightness for stormier scenes.
fn aurora_sky(f: &mut Frame, area: Rect, tick: u32, dim: f64) {
    let band = ((area.height as f64 * 0.34) as u16).clamp(4, 12);
    for y in 0..band {
        for x in 0..area.width {
            let v = ((x as f64 * 0.11 + (y as f64) * 0.35 + tick as f64 * 0.07).sin()
                + (x as f64 * 0.05 - tick as f64 * 0.045).sin())
                / 2.0;
            if v <= 0.1 {
                continue;
            }
            let t = ((v - 0.1) / 0.9).min(1.0);
            let s1 = 0.5 + 0.5 * (x as f64 * 0.02 + tick as f64 * 0.018).sin();
            let s2 = 0.5 + 0.5 * (x as f64 * 0.007 - tick as f64 * 0.011).sin();
            let r0 = mix(0x4d, 0x57, s1);
            let g0 = mix(0xf2, 0xc6, s1);
            let b0 = mix(0x8c, 0xe8, s1);
            let r = mix(r0, 0xb4, s2 * 0.75);
            let g = mix(g0, 0x8c, s2 * 0.75);
            let b = mix(b0, 0xf5, s2 * 0.75);
            let ch = if v > 0.8 { '▓' } else if v > 0.5 { '▒' } else { '░' };
            put(
                f,
                area,
                x,
                y + area.y,
                ch,
                Style::default().fg(Color::Rgb(mix(0x12, r, t * dim), mix(0x12, g, t * dim), mix(0x16, b, t * dim))),
            );
        }
    }
}

/// A jagged lightning bolt strikes every `cycle` ticks: a flash-dim-flash
/// strobe over three frames. Deterministic per strike so the bolt holds its
/// shape while it flashes, then walks to a new position next strike.
fn lightning(f: &mut Frame, area: Rect, tick: u32, ground_y: u16, cycle: u32) {
    let idx = tick / cycle;
    let phase = tick % cycle;
    if phase > 2 {
        return;
    }
    let h = hash(idx, 717);
    let span = (area.width / 2).max(1) as u32;
    let mut x = area.x + area.width / 8 + ((h >> 4) % span) as u16;
    let bright = match phase {
        0 => 1.0,
        1 => 0.35,
        _ => 0.7,
    };
    let style = Style::default()
        .fg(Color::Rgb(mix(0x12, 0xf5, bright), mix(0x14, 0xf8, bright), mix(0x20, 0xff, bright)))
        .add_modifier(Modifier::BOLD);
    let mut y = area.y;
    while y < ground_y.saturating_sub(1) {
        put(f, area, x, y, '│', style);
        let d = hash(idx ^ 0x5EED, y as u32);
        match d % 4 {
            0 => x = x.saturating_add(1),
            1 => x = x.saturating_sub(1),
            _ => {}
        }
        if d % 23 == 7 && y + 2 < ground_y {
            let dir: i32 = if d % 46 == 7 { 1 } else { -1 };
            for k in 1..=2i32 {
                let fx = x as i32 + dir * k;
                if fx >= area.x as i32 && fx < (area.x + area.width) as i32 {
                    put(f, area, fx as u16, y - 1, '─', style);
                }
            }
        }
        y += 1;
    }
}

/// Scene 0 — blizzard stroll: three penguins waddle across a snowfield under
/// an aurora, with lightning cracking through the storm.
fn scene_stroll(f: &mut Frame, area: Rect, tick: u32) {
    aurora_sky(f, area, tick, 0.65);
    let ground_y = (area.y + area.height).saturating_sub(2);
    lightning(f, area, tick, ground_y, 47);
    snow(f, area, tick, 4);
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
    aurora_sky(f, area, tick, 1.0);
    for i in 0..24u32 {
        let h = hash(i, 13);
        let x = (h % area.width.max(1) as u32) as u16;
        let y = ((h >> 8) % band.saturating_mul(2).max(1) as u32) as u16;
        if (tick / 4 + i) % (6 + h % 7) < 4 {
            put(f, area, x, area.y + y, '✦', Style::default().fg(Color::Rgb(0xba, 0xc2, 0xfe)).add_modifier(Modifier::DIM));
        }
    }
    let ground_y = (area.y + area.height).saturating_sub(2);
    lightning(f, area, tick, ground_y, 97);
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
    aurora_sky(f, area, tick, 0.8);
    snow(f, area, tick, 9);
    let ground_y = (area.y + area.height).saturating_sub(3);
    lightning(f, area, tick, ground_y, 63);
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

// ---------------------------------------------------------------------------
// Wordmark text effects — each paints the PENGUIN mask differently.
// ---------------------------------------------------------------------------

const MARK_GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);

fn dim_to(c: (u8, u8, u8), t: f64) -> Color {
    // blend a color toward the near-black background by (1 - t)
    Color::Rgb(mix(0x10, c.0, t), mix(0x10, c.1, t), mix(0x14, c.2, t))
}

/// Effect 3 — matrix rain: streams of katakana fall down every column with a
/// burning white-green head, glyphs mutating as they drop, trails fading to
/// dark; the wordmark blazes solid wherever a stream crosses it.
fn fx_matrix_rain(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    // faint base so the mark is always readable
    w.for_each_pixel(|x, y| {
        w.put_local(f, area, x as i32, y as i32, '█', Style::default().fg(dim_to((0x9e, 0xce, 0x6a), 0.18)));
    });
    for col in 0..area.width {
        let h = hash(col as u32, 4242);
        if h % 4 == 3 {
            continue; // sparse columns look more like rain
        }
        let speed = 1 + (h >> 8) % 4;
        let len = 9 + (h >> 12) % 10;
        let span = area.height as u32 + len * 2;
        let head = ((tick * speed) / 2 + (h >> 4)) % span;
        for t in 0..len {
            let y = head as i32 - t as i32;
            if y < 0 || y >= area.height as i32 {
                continue;
            }
            let yy = area.y + y as u16;
            // glyph mutates slowly down the trail, fast right under the head
            let gi = hash(col as u32, (y as u32).max(0) * 7 + if t < 3 { tick / 2 } else { tick / 6 });
            let glyph = if gi % 9 == 0 {
                char::from(b'0' + ((gi >> 4) % 10) as u8)
            } else {
                char::from_u32(0x30A1 + (gi >> 3) % 54).unwrap_or('・')
            };
            let f_t = t as f64 / len as f64;
            let intensity = if t == 0 { 1.0 } else { (1.0 - f_t).powi(2) * 0.9 };
            let lx = col as i32 - w.ox as i32;
            let ly = yy as i32 - w.oy as i32;
            let on_mark = w.inside(lx, ly) && w.at(lx.max(0) as usize, ly.max(0) as usize);
            let (fg, ch) = if t == 0 {
                (dim_to((0xe8, 0xff, 0xf0), 1.0), glyph) // burning-white head
            } else if on_mark {
                (dim_to((0x9e, 0xff, 0xb0), intensity * 1.6), '█') // mark blazes through
            } else {
                (dim_to((0x4f, 0xc7, 0x6a), intensity * 0.8), glyph)
            };
            put(f, area, col, yy, ch, Style::default().fg(fg));
        }
    }
}

/// Effect 4 — scramble decode: the mark materializes out of TV static, one
/// pixel at a time in a left-to-right reveal wave with a bright scanline.
fn fx_scramble(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    const NOISE: [char; 14] = ['▒', '░', '~', '!', '<', '>', '^', '*', '+', '#', '%', '&', '■', '□'];
    const REVEAL_SPAN: u32 = 300;
    let settle = |lx: usize, ly: usize| -> u32 {
        (hash(lx as u32, ly as u32 ^ 0xC0DE) % REVEAL_SPAN) + (lx as u32 * REVEAL_SPAN / (w.w.max(1) as u32)) / 2
    };
    w.for_each_pixel(|x, y| {
        let th = settle(x, y);
        if tick >= th {
            w.put_local(f, area, x as i32, y as i32, '█', Style::default().fg(MARK_GREEN).add_modifier(Modifier::BOLD));
        } else {
            let n = hash(x as u32, (tick / 2) ^ (y as u32 * 31));
            let near = th - tick < 4;
            w.put_local(
                f,
                area,
                x as i32,
                y as i32,
                NOISE[(n >> 4) as usize % NOISE.len()],
                Style::default().fg(if near { Color::Rgb(0xf5, 0xc9, 0xe1) } else { Color::Rgb(0x58, 0x5b, 0x70) }),
            );
        }
    });
    // stray static around the mark keeps the screen alive mid-reveal
    if tick < REVEAL_SPAN + 40 {
        for i in 0..30u32 {
            let h = hash(i, tick / 3);
            let x = area.x + (h % area.width.max(1) as u32) as u16;
            let y = area.y + ((h >> 9) % area.height.max(1) as u32) as u16;
            put(f, area, x, y, NOISE[(h >> 4) as usize % NOISE.len()], Style::default().fg(Color::Rgb(0x3a, 0x40, 0x58)));
        }
    }
}

/// Effect 5 — glitch: solid mark with horizontal slice displacement and
/// red/blue chromatic-split tears on random rows.
fn fx_glitch(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    let frame = tick / 2;
    w.for_each_pixel(|x, y| {
        w.put_local(f, area, x as i32, y as i32, '█', Style::default().fg(MARK_GREEN));
    });
    for row in 0..w.h as usize {
        let g = hash(row as u32, frame);
        if g % 5 != 0 {
            continue;
        }
        let shift = (((g >> 8) % 5) as i32).saturating_sub(2);
        let shift = if shift == 0 { 3 } else { shift };
        for x in 0..w.w as usize {
            if !w.at(x, row) {
                continue;
            }
            let split = hash(x as u32, frame ^ (row as u32 + 1));
            let col = if (x as u32 + frame) % 2 == 0 { Color::Rgb(0xf3, 0x8b, 0xa8) } else { Color::Rgb(0x89, 0xb4, 0xfa) };
            w.put_local(f, area, x as i32 + shift, row as i32, '▓', Style::default().fg(col));
            if split % 7 == 0 {
                w.put_local(f, area, x as i32 - shift, row as i32, '▒', Style::default().fg(dim_to((0xba, 0x9b, 0xfe), 0.5)));
            }
        }
    }
}

/// Effect 6 — wave sway: the mark sways on a travelling sine; crests bleach
/// toward white, troughs sink into dim blue.
fn fx_wave(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    let t = tick as f64 * 0.16;
    for x in 0..w.w as usize {
        let phase = (x as f64 * 0.32 + t).sin();
        let off = (phase * 2.0).round() as i32;
        let crest = (phase + 1.0) / 2.0; // 0..1
        for y in 0..w.h as usize {
            if !w.at(x, y) {
                continue;
            }
            let edge = if y == 0 || y + 1 == w.h as usize || !w.at(x, y - 1) || !w.at(x, (y + 1).min(w.h as usize - 1)) { 1.0 } else { 0.72 };
            let fg = Color::Rgb(
                mix(0x89, 0xd8, crest * edge),
                mix(0xb4, 0xf5, crest * edge),
                mix(0xfa, 0xb2, crest * edge),
            );
            w.put_local(f, area, x as i32, y as i32 + off, '█', Style::default().fg(fg));
        }
    }
}

/// Effect 7 — fire glow: flames lick down from the top of each pixel column;
/// unlit pixels smoulder dark red so the mark never disappears.
fn fx_fire(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    for x in 0..w.w as usize {
        let flick = (hash(x as u32, tick / 2) % 100) as f64 / 100.0;
        let flame = 3.0 + 2.5 * (x as f64 * 0.55 + tick as f64 * 0.22).sin() + flick * 2.5;
        for y in 0..w.h as usize {
            if !w.at(x, y) {
                continue;
            }
            let from_top = (w.h as f64 - 1.0 - y as f64).max(0.0);
            if from_top < flame {
                let t = (1.0 - from_top / flame.max(0.5)).min(1.0); // 1 at the top edge
                // yellow base → orange → red tips
                let r = mix(0xf3, 0xf9, t);
                let g = mix(0x8b, 0xe2, t * t);
                let b = mix(0xa8, 0xaf, t * t);
                w.put_local(f, area, x as i32, y as i32, '█', Style::default().fg(Color::Rgb(r, g, b)));
            } else {
                w.put_local(f, area, x as i32, y as i32, '▓', Style::default().fg(dim_to((0xf3, 0x8b, 0xa8), 0.35)));
            }
        }
    }
}

/// Effect 8 — shimmer sweep: diagonal pastel ramp across the mark plus a
/// white specular band travelling left to right.
fn fx_shimmer(f: &mut Frame, area: Rect, tick: u32, w: &Wordmark) {
    let band = ((tick as f64 / 1.6) % (w.w as f64 + 10.0)) as i32 - 5;
    w.for_each_pixel(|x, y| {
        let in_band = ((x as i32 - band).abs()) < 2;
        let fg = if in_band { Color::Rgb(0xf5, 0xf8, 0xff) } else { shimmer_color(tick / 2, x + y) };
        w.put_local(f, area, x as i32, y as i32, '█', Style::default().fg(fg).add_modifier(Modifier::BOLD));
    });
}

pub fn render_screensaver(entry: usize, tick: u32, f: &mut Frame, area: Rect) {
    let wordmark_effect = entry % ENTRY_COUNT >= SCENE_COUNT;
    match entry % ENTRY_COUNT {
        1 => scene_aurora(f, area, tick),
        2 => scene_slide(f, area, tick),
        0 => scene_stroll(f, area, tick),
        e => {
            let w = Wordmark::layout(area);
            match e {
                3 => fx_matrix_rain(f, area, tick, &w),
                4 => fx_scramble(f, area, tick, &w),
                5 => fx_glitch(f, area, tick, &w),
                6 => fx_wave(f, area, tick, &w),
                7 => fx_fire(f, area, tick, &w),
                _ => fx_shimmer(f, area, tick, &w),
            }
        }
    }
    draw_caption(f, area, tick, !wordmark_effect, entry_name(entry % ENTRY_COUNT));
}

fn draw_caption(f: &mut Frame, area: Rect, tick: u32, show_title: bool, label: &str) {
    if area.height < 8 || area.width < 40 {
        return;
    }
    if show_title {
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
    }
    // channel bug: current effect name, bottom-left, unobtrusive
    let bug = format!("· {label} ·");
    put_line(f, area, area.x + 2, (area.y + area.height).saturating_sub(1), &bug, Style::default().fg(Color::Rgb(0x4a, 0x50, 0x68)).add_modifier(Modifier::ITALIC));
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
            for entry in 0..ENTRY_COUNT {
                for tick in [0u32, 7, 42, 999] {
                    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                    let _ = terminal.draw(|f| render_screensaver(entry, tick, f, f.area()));
                    let buf = terminal.backend().buffer().clone();
                    let nonblank = buf.content().iter().filter(|c| !c.symbol().trim().is_empty()).count();
                    if w >= 40 && h >= 12 {
                        assert!(nonblank > 30, "entry {entry} at {w}x{h} t{tick} looks empty ({nonblank})");
                    }
                }
            }
        }
    }

    #[test]
    fn wordmark_mask_has_the_right_shape() {
        let w = Wordmark::layout(Rect::new(0, 0, 120, 34)); // scale 2
        assert_eq!(w.h, 14);
        assert_eq!(w.w, 82);
        // every letter has at least one pixel and the mark is centered
        for gi in 0..WORD.chars().count() {
            let lo = gi * 6 * 2;
            let hi = (lo + 5 * 2).min(w.w as usize);
            let any = (0..w.h as usize).any(|y| w.rows[y][lo..hi].iter().any(|p| *p));
            assert!(any, "letter {gi} of the wordmark is empty");
        }
        assert_eq!(w.ox, (120 - 82) / 2);
        let small = Wordmark::layout(Rect::new(0, 0, 60, 15)); // scale 1
        assert_eq!(small.w, 41);
        assert_eq!(small.h, 7);
    }

    #[test]
    fn scramble_reveals_the_full_mark() {
        let w = Wordmark::layout(Rect::new(0, 0, 80, 24));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let _ = terminal.draw(|f| fx_scramble(f, f.area(), 600, &w));
        let buf = terminal.backend().buffer().clone();
        let painted = (0..w.h as usize).filter(|y| (0..w.w as usize).any(|x| {
            let cell = &buf[(w.ox + x as u16, w.oy + *y as u16)];
            w.rows[*y][x] && cell.symbol() == "█"
        })).count();
        assert_eq!(painted, w.h as usize, "reveal incomplete: only {painted} rows show solid blocks");
    }

    #[test]
    fn picker_never_repeats_and_stays_balanced() {
        let mut picker = EffectPicker::new(0xBADC0FFE);
        let mut counts = [0u32; ENTRY_COUNT];
        let mut last = usize::MAX;
        for _ in 0..900 {
            let e = picker.pick();
            assert_ne!(e, last, "picker chose the same entry twice in a row");
            counts[e] += 1;
            last = e;
        }
        let min = *counts.iter().min().unwrap();
        let max = *counts.iter().max().unwrap();
        // inverse-frequency weighting keeps long-run distribution even-ish
        assert!(max - min <= min / 2 + 3, "picker skewed: {counts:?}");
    }

    #[test]
    fn every_effect_has_a_name_and_duration() {
        for e in 0..ENTRY_COUNT {
            assert!(!entry_name(e).is_empty());
            assert!(entry_duration(e) >= 300 && entry_duration(e) <= 900);
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
