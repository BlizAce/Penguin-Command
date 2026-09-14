pub mod input;

use std::collections::VecDeque;
use std::path::PathBuf;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TColor {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: TColor,
    pub bg: TColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub invisible: bool,
    pub strike: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: TColor::Default,
            bg: TColor::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            reverse: false,
            invisible: false,
            strike: false,
        }
    }
}

pub type Pen = Cell;

fn pen_default() -> Pen {
    Cell::default()
}

struct Screen {
    rows: usize,
    cols: usize,
    grid: Vec<Vec<Cell>>,
    cur: (usize, usize), // x may reach cols via pending wrap; stored clamped
    saved: Option<(usize, usize, Pen)>,
    top: usize,
    bottom: usize,
}

impl Screen {
    fn new(rows: usize, cols: usize) -> Self {
        Screen {
            rows: rows.max(1),
            cols: cols.max(1),
            grid: vec![vec![Cell::default(); cols.max(1)]; rows.max(1)],
            cur: (0, 0),
            saved: None,
            top: 0,
            bottom: rows.saturating_sub(1).max(1),
        }
    }

    fn blank(&self) -> Vec<Cell> {
        vec![Cell::default(); self.cols]
    }

    fn fit(s: &mut Screen, rows: usize, cols: usize, mut dropped: Option<&mut Vec<Vec<Cell>>>) {
        s.cols = cols;
        for line in s.grid.iter_mut() {
            line.resize(cols, Cell::default());
        }
        let old_rows = s.rows;
        if rows < old_rows {
            for _ in 0..old_rows - rows {
                let line = s.grid.remove(0);
                if let Some(d) = dropped.as_mut() {
                    d.push(line);
                }
            }
        } else if rows > old_rows {
            for _ in old_rows..rows {
                let blank = vec![Cell::default(); cols];
                s.grid.push(blank);
            }
        }
        s.rows = rows;
        s.top = 0;
        s.bottom = rows - 1;
        s.cur.0 = s.cur.0.min(cols.saturating_sub(1));
        s.cur.1 = s.cur.1.min(rows.saturating_sub(1));
    }
}

pub struct Term {
    main: Screen,
    alt: Option<Screen>,
    history: VecDeque<Vec<Cell>>,
    hist_cap: usize,
    pen: Pen,
    pub autowrap: bool,
    pending_wrap: bool,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
    pub app_cursor_keys: bool,
    pub title: String,
    pub cwd: Option<PathBuf>,
    /// Bytes the emulator must send back to the app (DSR / DA / color replies).
    pub pending_output: Vec<u8>,
    orig_mode: bool, // ORIGIN mode
}

/// Parser + Term pair (vte's advance() needs to borrow both).
pub struct Emulator {
    pub parser: vte::Parser,
    pub term: Term,
}

impl Emulator {
    pub fn new(rows: u16, cols: u16) -> Emulator {
        Emulator { parser: vte::Parser::new(), term: Term::new(rows, cols) }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }
}

impl Term {
    pub fn new(rows: u16, cols: u16) -> Term {
        let rows = rows.max(4) as usize;
        let cols = cols.max(8) as usize;
        Term {
            main: Screen::new(rows, cols),
            alt: None,
            history: VecDeque::new(),
            hist_cap: 5000,
            pen: pen_default(),
            autowrap: true,
            pending_wrap: false,
            cursor_visible: true,
            bracketed_paste: false,
            app_cursor_keys: false,
            title: String::new(),
            cwd: None,
            pending_output: Vec::new(),
            orig_mode: false,
        }
    }

    fn cur_screen(&mut self) -> &mut Screen {
        self.alt.as_mut().unwrap_or(&mut self.main)
    }

    pub fn screen_rows(&self) -> usize {
        self.alt.as_ref().map_or(self.main.rows, |a| a.rows)
    }
    pub fn screen_cols(&self) -> usize {
        self.alt.as_ref().map_or(self.main.cols, |a| a.cols)
    }

    pub fn cursor(&self) -> (usize, usize, bool) {
        let s = self.alt.as_ref().unwrap_or(&self.main);
        (s.cur.0.min(s.cols - 1), s.cur.1, self.cursor_visible)
    }

    pub fn screen_lines(&self) -> &Vec<Vec<Cell>> {
        &self.alt.as_ref().unwrap_or(&self.main).grid
    }

    pub fn history_len(&self) -> usize {
        if self.alt.is_some() {
            0
        } else {
            self.history.len()
        }
    }

    pub fn history_line(&self, i: usize) -> Option<&Vec<Cell>> {
        self.history.get(i)
    }

    #[allow(dead_code)]
    pub fn total_view_rows(&self) -> usize {
        self.history_len() + self.screen_rows()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(4) as usize;
        let cols = cols.max(8) as usize;
        let mut dropped: Vec<Vec<Cell>> = Vec::new();
        Screen::fit(&mut self.main, rows, cols, Some(&mut dropped));
        if let Some(a) = self.alt.as_mut() {
            Screen::fit(a, rows, cols, None);
        }
        for line in dropped {
            self.push_history(line);
        }
    }

    fn push_history(&mut self, line: Vec<Cell>) {
        if self.history.len() >= self.hist_cap {
            self.history.pop_front();
        }
        self.history.push_back(line);
    }

    // ---- vte helpers ----

    fn scroll_up_region(&mut self, n: usize) {
        let to_history = self.alt.is_none() && self.main.top == 0 && self.main.bottom + 1 == self.main.rows;
        let mut removed: Vec<Vec<Cell>> = Vec::new();
        {
            let s = self.cur_screen();
            if s.top > s.bottom {
                return;
            }
            for _ in 0..n.min(s.bottom - s.top + 1) {
                removed.push(s.grid.remove(s.top));
                let at = (s.bottom).min(s.grid.len());
                s.grid.insert(at, s.blank());
            }
        }
        if to_history {
            for line in removed {
                self.push_history(line);
            }
        }
    }

    fn scroll_down_region(&mut self, n: usize) {
        let s = self.cur_screen();
        if s.top > s.bottom {
            return;
        }
        for _ in 0..n.min(s.bottom - s.top + 1) {
            s.grid.remove(s.bottom);
            s.grid.insert(s.top, s.blank());
        }
    }

    fn linefeed(&mut self) {
        let s = self.cur_screen();
        if s.cur.1 == s.bottom {
            self.scroll_up_region(1);
        } else if s.cur.1 < self.screen_rows() - 1 {
            self.cur_screen().cur.1 += 1;
        }
    }

    fn carriage_return(&mut self) {
        self.cur_screen().cur.0 = 0;
        self.pending_wrap = false;
    }

    fn backspace(&mut self) {
        if self.pending_wrap {
            self.pending_wrap = false;
            return;
        }
        let s = self.cur_screen();
        s.cur.0 = s.cur.0.saturating_sub(1);
    }

    fn tab(&mut self) {
        let s = self.cur_screen();
        let next = ((s.cur.0 / 8) + 1) * 8;
        s.cur.0 = next.min(s.cols - 1);
        self.pending_wrap = false;
    }

    fn put_char(&mut self, c: char) {
        if self.pending_wrap && self.autowrap {
            self.carriage_return();
            self.linefeed();
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if w == 0 {
            return;
        }
        let cell = Cell { ch: c, ..self.pen };
        let blank = Cell { ch: ' ', ..self.pen };
        let (x, y) = {
            let s = self.cur_screen();
            (s.cur.0, s.cur.1)
        };
        let cols = self.screen_cols();
        {
            let s = self.cur_screen();
            if x < cols {
                s.grid[y][x] = cell;
                if w == 2 && x + 1 < cols {
                    s.grid[y][x + 1] = blank;
                }
            }
        }
        let s = self.cur_screen();
        if x + w >= cols {
            s.cur.0 = cols - 1;
            self.pending_wrap = true;
        } else {
            s.cur.0 = x + w;
        }
    }

    fn move_to(&mut self, row: usize, col: usize) {
        let (rows, cols) = (self.screen_rows(), self.screen_cols());
        let om = self.orig_mode;
        let s = self.cur_screen();
        let top = if om { s.top } else { 0 };
        s.cur.1 = (top + row).min(rows - 1);
        s.cur.0 = col.min(cols - 1);
        self.pending_wrap = false;
    }

    fn erase_cells(&mut self, from: usize, to: usize) {
        let blank = Cell { ch: ' ', bg: self.pen.bg, ..Default::default() };
        let y = self.cur_screen().cur.1;
        let s = self.cur_screen();
        let cols = s.cols;
        for x in from.min(cols)..to.min(cols) {
            s.grid[y][x] = blank;
        }
    }

    fn apply_sgr(&mut self, p: &[u16]) {
        let mut i = 0;
        if p.is_empty() {
            self.pen = pen_default();
            return;
        }
        while i < p.len() {
            let v = p[i];
            match v {
                0 => self.pen = pen_default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                3 => self.pen.italic = true,
                4 => self.pen.underline = true,
                5 | 6 => {}
                7 => self.pen.reverse = true,
                8 => self.pen.invisible = true,
                9 => self.pen.strike = true,
                21 | 22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                27 => self.pen.reverse = false,
                28 => self.pen.invisible = false,
                29 => self.pen.strike = false,
                30..=37 => self.pen.fg = TColor::Idx((v - 30) as u8),
                38 => {
                    if let Some(c) = parse_ext_color(p, &mut i) {
                        self.pen.fg = c;
                    }
                }
                39 => self.pen.fg = TColor::Default,
                40..=47 => self.pen.bg = TColor::Idx((v - 40) as u8),
                48 => {
                    if let Some(c) = parse_ext_color(p, &mut i) {
                        self.pen.bg = c;
                    }
                }
                49 => self.pen.bg = TColor::Default,
                90..=97 => self.pen.fg = TColor::Idx((v - 90 + 8) as u8),
                100..=107 => self.pen.bg = TColor::Idx((v - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
    }

    fn set_mode(&mut self, private: bool, set: bool, modes: &[u16]) {
        for m in modes {
            match (*m, private) {
                (7, true) => self.autowrap = set,
                (25, true) => self.cursor_visible = set,
                (2004, true) => self.bracketed_paste = set,
                (1, true) => self.app_cursor_keys = set,
                (1049 | 1047 | 47, true) => {
                    if set && self.alt.is_none() {
                        let (r, c) = (self.main.rows, self.main.cols);
                        self.main.saved = Some((self.main.cur.0, self.main.cur.1, self.pen));
                        self.alt = Some(Screen::new(r, c));
                    } else if !set && self.alt.is_some() {
                        self.alt = None;
                        if let Some((x, y, p)) = self.main.saved.take() {
                            self.main.cur = (x, y);
                            self.pen = p;
                        }
                    }
                }
                (6, true) => {
                    self.orig_mode = set;
                    self.move_to(0, 0);
                }
                _ => {}
            }
        }
    }
}

fn parse_ext_color(p: &[u16], i: &mut usize) -> Option<TColor> {
    let mode = *p.get(*i + 1)?;
    if mode == 5 {
        let idx = *p.get(*i + 2)?;
        *i += 2;
        Some(TColor::Idx(idx as u8))
    } else if mode == 2 {
        let r = *p.get(*i + 2)? as u8;
        let g = *p.get(*i + 3)? as u8;
        let b = *p.get(*i + 4)? as u8;
        *i += 4;
        Some(TColor::Rgb(r, g, b))
    } else {
        None
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl vte::Perform for Term {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => self.backspace(),
            0x09 => self.tab(),
            0x0a | 0x0b | 0x0c => self.linefeed(),
            0x0d => self.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        let p: Vec<u16> = params.iter().map(|sub| sub.first().copied().unwrap_or(0)).collect();
        let private = intermediates.first() == Some(&b'?');
        let n = |i: usize| p.get(i).copied().unwrap_or(1).max(1) as usize;

        match action {
            'A' => {
                let up = n(0);
                let om = self.orig_mode;
                let s = self.cur_screen();
                let top = if om { 0 } else { s.top };
                s.cur.1 = if s.cur.1 == top { top } else { (s.cur.1.saturating_sub(up)).max(top) };
                self.pending_wrap = false;
            }
            'B' => {
                let s = self.cur_screen();
                s.cur.1 = (s.cur.1 + n(0)).min(s.bottom);
                self.pending_wrap = false;
            }
            'C' => {
                let cols = self.screen_cols();
                let s = self.cur_screen();
                s.cur.0 = (s.cur.0 + n(0)).min(cols - 1);
                self.pending_wrap = false;
            }
            'D' => {
                let s = self.cur_screen();
                s.cur.0 = s.cur.0.saturating_sub(n(0));
                self.pending_wrap = false;
            }
            'E' => {
                self.carriage_return();
                for _ in 0..n(0) {
                    self.linefeed();
                }
            }
            'F' => {
                self.carriage_return();
                let s = self.cur_screen();
                s.cur.1 = s.cur.1.saturating_sub(n(0));
            }
            'G' | '`' => {
                let col = p.first().copied().unwrap_or(1).saturating_sub(1) as usize;
                let cols = self.screen_cols();
                self.cur_screen().cur.0 = col.min(cols - 1);
                self.pending_wrap = false;
            }
            'd' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1) as usize;
                let rows = self.screen_rows();
                self.cur_screen().cur.1 = row.min(rows - 1);
            }
            'H' | 'f' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1) as usize;
                let col = p.get(1).copied().unwrap_or(1).saturating_sub(1) as usize;
                self.move_to(row, col);
            }
            'J' => {
                let mode = p.first().copied().unwrap_or(0);
                let (x, y) = self.cur_screen().cur;
                match mode {
                    0 => {
                        self.erase_cells(x, self.screen_cols());
                        let blank = Cell { ch: ' ', bg: self.pen.bg, ..Default::default() };
                        for row in y + 1..self.screen_rows() {
                            let s = self.cur_screen();
                            s.grid[row] = vec![blank; s.cols];
                        }
                    }
                    1 => {
                        let blank = Cell { ch: ' ', bg: self.pen.bg, ..Default::default() };
                        for row in 0..y {
                            let s = self.cur_screen();
                            s.grid[row] = vec![blank; s.cols];
                        }
                        self.erase_cells(0, x + 1);
                    }
                    2 | 3 => {
                        let blank = Cell { ch: ' ', bg: self.pen.bg, ..Default::default() };
                        let rows = self.screen_rows();
                        for row in 0..rows {
                            let s = self.cur_screen();
                            s.grid[row] = vec![blank; s.cols];
                        }
                        if mode == 3 {
                            self.history.clear();
                        }
                    }
                    _ => {}
                }
            }
            'K' => {
                let mode = p.first().copied().unwrap_or(0);
                let x = self.cur_screen().cur.0;
                match mode {
                    0 => self.erase_cells(x, self.screen_cols()),
                    1 => self.erase_cells(0, x + 1),
                    2 => self.erase_cells(0, self.screen_cols()),
                    _ => {}
                }
            }
            'L' => {
                let s = self.cur_screen();
                if s.cur.1 >= s.top && s.cur.1 <= s.bottom {
                    for _ in 0..n(0) {
                        s.grid.remove(s.bottom);
                        s.grid.insert(s.cur.1, s.blank());
                    }
                }
            }
            'M' => {
                let s = self.cur_screen();
                if s.cur.1 >= s.top && s.cur.1 <= s.bottom {
                    for _ in 0..n(0) {
                        s.grid.remove(s.cur.1);
                        s.grid.insert(s.bottom, s.blank());
                    }
                }
            }
            'P' => {
                let y = self.cur_screen().cur.1;
                let x = self.cur_screen().cur.0;
                let s = self.cur_screen();
                let row = &mut s.grid[y];
                for _ in 0..n(0) {
                    if x < row.len() {
                        row.remove(x);
                    }
                    row.push(Cell::default());
                }
            }
            '@' => {
                let blank = Cell { ch: ' ', bg: self.pen.bg, ..Default::default() };
                let y = self.cur_screen().cur.1;
                let x = self.cur_screen().cur.0;
                let s = self.cur_screen();
                let row = &mut s.grid[y];
                for _ in 0..n(0) {
                    if x < row.len() {
                        row.insert(x, blank);
                    }
                    row.truncate(s.cols);
                }
            }
            'X' => {
                let x = self.cur_screen().cur.0;
                self.erase_cells(x, x + n(0));
            }
            'S' => self.scroll_up_region(n(0)),
            'T' => self.scroll_down_region(n(0)),
            'r' => {
                let top = p.first().copied().unwrap_or(1).saturating_sub(1) as usize;
                let bottom = p.get(1).copied().unwrap_or(self.screen_rows() as u16).saturating_sub(1) as usize;
                let rows = self.screen_rows();
                let om = self.orig_mode;
                let s = self.cur_screen();
                s.top = top.min(rows - 1);
                s.bottom = bottom.min(rows - 1);
                if om {
                    s.cur = (0, s.top);
                } else {
                    s.cur = (0, 0);
                }
            }
            'm' => self.apply_sgr(&p),
            'h' => {
                let modes: Vec<u16> = p.to_vec();
                self.set_mode(private, true, &modes);
            }
            'l' => {
                let modes: Vec<u16> = p.to_vec();
                self.set_mode(private, false, &modes);
            }
            's' => {
                let pen = self.pen;
                let s = self.cur_screen();
                s.saved = Some((s.cur.0, s.cur.1, pen));
            }
            'u' => {
                let saved = self.cur_screen().saved;
                if let Some((x, y, pen)) = saved {
                    let s = self.cur_screen();
                    s.cur = (x.min(s.cols - 1), y.min(s.rows - 1));
                    self.pen = pen;
                }
            }
            'n' => {
                if !private && p.first().copied().unwrap_or(0) == 6 {
                    let (x, y) = self.cur_screen().cur;
                    self.pending_output.extend(format!("\x1b[{};{}R", y + 1, x + 1).into_bytes());
                }
            }
            'c' => {
                if !private {
                    self.pending_output.extend(b"\x1b[?62;9c");
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates.first()) {
            (b'7', _) => {
                let pen = self.pen;
                let s = self.cur_screen();
                s.saved = Some((s.cur.0, s.cur.1, pen));
            }
            (b'8', _) => {
                let saved = self.cur_screen().saved;
                if let Some((x, y, pen)) = saved {
                    let s = self.cur_screen();
                    s.cur = (x.min(s.cols - 1), y.min(s.rows - 1));
                    self.pen = pen;
                }
            }
            (b'D', _) => self.linefeed(),
            (b'E', _) => {
                self.carriage_return();
                self.linefeed();
            }
            (b'M', _) => {
                let s = self.cur_screen();
                if s.cur.1 == s.top {
                    self.scroll_down_region(1);
                } else {
                    s.cur.1 = s.cur.1.saturating_sub(1);
                }
            }
            (b'c', _) => {
                let (r, c) = (self.main.rows, self.main.cols);
                self.main = Screen::new(r, c);
                self.alt = None;
                self.pen = pen_default();
                self.autowrap = true;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        let key = params[0];
        if (key == b"0" || key == b"2") && params.len() > 1 {
            self.title = String::from_utf8_lossy(params[1]).into_owned();
        } else if (key == b"10" || key == b"11") && params.get(1).map(|v| *v == b"?").unwrap_or(false) {
            // reply with our palette: fg #cdd6f4, bg #101014 (XGetPixel 4-hex form)
            let rgb = if key == b"10" { "cccc/dddd/ffff" } else { "1010/1010/1414" };
            self.pending_output.extend(format!("\x1b]{};rgb:{rgb}\x1b\\", String::from_utf8_lossy(key)).into_bytes());
        } else if key == b"7" && params.len() > 1 {
            let raw = String::from_utf8_lossy(params[1]).into_owned();
            if let Some(rest) = raw.strip_prefix("file://") {
                // file://host/path -> /path
                let path = match rest.find('/') {
                    Some(i) => &rest[i..],
                    None => return,
                };
                self.cwd = Some(PathBuf::from(percent_decode(path)));
            }
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(cells: &[Cell]) -> String {
        cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn basic_write_and_newline() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"hello\r\nworld");
        let t = &emu.term;
        assert_eq!(line_text(&t.screen_lines()[0]), "hello");
        assert_eq!(line_text(&t.screen_lines()[1]), "world");
    }

    #[test]
    fn sgr_and_erase() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[31mred\x1b[0m normal\r\n");
        let t = &emu.term;
        assert_eq!(line_text(&t.screen_lines()[0]), "red normal");
        assert!(matches!(t.screen_lines()[0][0].fg, TColor::Idx(1)));
        drop(t);
        emu.feed(b"\x1b[H\x1b[2J");
        assert_eq!(line_text(&emu.term.screen_lines()[0]), "");
    }

    #[test]
    fn scroll_into_history() {
        let mut emu = Emulator::new(4, 20);
        for i in 0..8 {
            emu.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(emu.term.history_len() > 0);
    }

    #[test]
    fn alt_screen_switch() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"main line\r\n\x1b[?1049halt screen");
        assert_eq!(line_text(&emu.term.screen_lines()[0]), "alt screen");
        emu.feed(b"\x1b[?1049l");
        assert_eq!(line_text(&emu.term.screen_lines()[0]), "main line");
    }

    #[test]
    fn osc7_cwd() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b]7;file://host/tmp/foo\x07");
        assert_eq!(emu.term.cwd.as_deref().unwrap().to_str().unwrap(), "/tmp/foo");
    }
}
