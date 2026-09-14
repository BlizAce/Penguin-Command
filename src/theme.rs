use ratatui::style::Color;

pub struct Palette {
    pub bg: Color,
    pub panel: Color,
    pub panel_line: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent2: Color,
    pub agent_line: Color,
    pub success: Color,
    pub warn: Color,
    pub danger: Color,
}

pub const OMARCHY: Palette = Palette {
    bg: Color::Rgb(0x10, 0x10, 0x14),
    panel: Color::Rgb(0x16, 0x16, 0x1d),
    panel_line: Color::Rgb(0x2a, 0x2a, 0x35),
    text: Color::Rgb(0xcd, 0xd6, 0xf4),
    muted: Color::Rgb(0x6c, 0x70, 0x86),
    accent: Color::Rgb(0xcb, 0xa6, 0xf7),
    accent2: Color::Rgb(0xf5, 0xc9, 0xe1),
    agent_line: Color::Rgb(0xba, 0x9b, 0xfe),
    success: Color::Rgb(0xa6, 0xe3, 0xb1),
    warn: Color::Rgb(0xf9, 0xe2, 0xaf),
    danger: Color::Rgb(0xf3, 0x8b, 0xa8),
};

/// Map an xterm-256 index to RGB (standard cube).
pub fn xterm2rgb(idx: u8) -> Color {
    const BASE: [u8; 48] = [
        0x1e, 0x1e, 0x2e, // 0 black
        0xf3, 0x8b, 0xa8, // 1 red
        0xa6, 0xe3, 0xb1, // 2 green
        0xf9, 0xe2, 0xaf, // 3 yellow
        0x89, 0xb4, 0xfa, // 4 blue
        0xba, 0x9b, 0xfe, // 5 magenta
        0x94, 0xe2, 0xd8, // 6 cyan
        0xba, 0xc2, 0xfe, // 7 white
        0x58, 0x5b, 0x70, // 8 br black
        0xf3, 0x8b, 0xa8, // 9
        0xa6, 0xe3, 0xb1, // 10
        0xf9, 0xe2, 0xaf, // 11
        0x89, 0xb4, 0xfa, // 12
        0xc7, 0xa6, 0xf7, // 13
        0x94, 0xe2, 0xd8, // 14
        0xa5, 0xad, 0xff, // 15
    ];
    if idx < 16 {
        let i = idx as usize * 3;
        return Color::Rgb(BASE[i], BASE[i + 1], BASE[i + 2]);
    }
    if idx < 232 {
        let c = idx - 16;
        let r = c / 36;
        let g = (c % 36) / 6;
        let b = c % 6;
        let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Color::Rgb(conv(r as u8), conv(g as u8), conv(b as u8));
    }
    let v = 8 + (idx - 232) as u16 * 10;
    Color::Rgb(v as u8, v as u8, v as u8)
}
