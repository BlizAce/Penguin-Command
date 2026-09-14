use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a crossterm key event back into terminal input bytes for the PTY.
pub fn encode_key(ev: &KeyEvent, app_cursor_keys: bool, bracketed_paste: bool) -> Option<Vec<u8>> {
    if ev.kind == crossterm::event::KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    let mut out: Vec<u8> = Vec::new();
    if alt {
        out.push(0x1b);
    }

    fn csi(out: &mut Vec<u8>, params: &str, modif: u8, final_: char) {
        if modif > 1 {
            out.extend(format!("\x1b[{};{}{}", params, modif, final_).into_bytes());
        } else {
            out.extend(format!("\x1b[{}{}", params, final_).into_bytes());
        }
    }

    let modif = 1 + shift as u8 + (alt as u8) * 2 + (ctrl as u8) * 4;
    // when alt already emitted ESC prefix, xterm puts modifier in seq too; keep both
    match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = match c {
                    ' ' => Some(0u8),
                    '@' | '2' => Some(0),
                    'a'..='z' => Some((c as u8) - b'a' + 1),
                    'A'..='Z' => Some((c as u8) - b'A' + 1),
                    '[' | '3' => Some(27),
                    '\\' | '4' => Some(28),
                    ']' | '5' => Some(29),
                    '^' | '6' => Some(30),
                    '_' | '7' => Some(31),
                    '/' | '8' => Some(127),
                    _ => None,
                };
                match b {
                    Some(v) => out.push(v),
                    None => return None,
                }
            } else {
                let mut buf = [0u8; 4];
                out.extend(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Backspace => out.push(if ctrl { 8 } else { 127 }),
        KeyCode::Tab => {
            if shift {
                out.extend(b"\x1b[Z");
            } else {
                out.push(b'\t');
            }
        }
        KeyCode::Esc => {
            out.clear();
            out.push(0x1b);
        }
        KeyCode::Up => {
            if app_cursor_keys {
                csi(&mut out, "1", modif.saturating_sub(1).max(if modif > 1 { modif } else { 0 }), 'A');
                if modif <= 1 {
                    out.clear();
                    if alt {
                        out.push(0x1b);
                    }
                    out.extend(b"\x1bOA");
                }
            } else {
                csi(&mut out, "", modif, 'A');
            }
        }
        KeyCode::Down => {
            if app_cursor_keys && modif <= 1 {
                out.clear();
                if alt {
                    out.push(0x1b);
                }
                out.extend(b"\x1bOB");
            } else {
                csi(&mut out, "", modif, 'B');
            }
        }
        KeyCode::Right => {
            if app_cursor_keys && modif <= 1 {
                out.clear();
                if alt {
                    out.push(0x1b);
                }
                out.extend(b"\x1bOC");
            } else {
                csi(&mut out, "", modif, 'C');
            }
        }
        KeyCode::Left => {
            if app_cursor_keys && modif <= 1 {
                out.clear();
                if alt {
                    out.push(0x1b);
                }
                out.extend(b"\x1bOD");
            } else {
                csi(&mut out, "", modif, 'D');
            }
        }
        KeyCode::Home => {
            if app_cursor_keys && modif <= 1 {
                out.clear();
                if alt {
                    out.push(0x1b);
                }
                out.extend(b"\x1bOH");
            } else {
                csi(&mut out, "1", modif, 'H');
            }
        }
        KeyCode::End => {
            if app_cursor_keys && modif <= 1 {
                out.clear();
                if alt {
                    out.push(0x1b);
                }
                out.extend(b"\x1bOF");
            } else {
                csi(&mut out, "4", modif, 'H');
            }
        }
        KeyCode::PageUp => csi(&mut out, "5", modif, '~'),
        KeyCode::PageDown => csi(&mut out, "6", modif, '~'),
        KeyCode::Insert => csi(&mut out, "2", modif, '~'),
        KeyCode::Delete => csi(&mut out, "3", modif, '~'),
        KeyCode::F(n) => {
            let code = match n {
                1 => "\x1bOP".to_string(),
                2 => "\x1bOQ".to_string(),
                3 => "\x1bOR".to_string(),
                4 => "\x1bOS".to_string(),
                5..=10 => format!("\x1b[{}~", 11 + n - 5),
                11..=12 => format!("\x1b[{}~", 23 + n - 11),
                _ => return None,
            };
            out.extend(code.into_bytes());
        }
        _ => return None,
    }
    let _ = bracketed_paste;
    Some(out)
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend(text.as_bytes());
        v.extend(b"\x1b[201~");
        v
    } else {
        text.as_bytes().to_vec()
    }
}

/// Parse a chord string like "ctrl-space", "ctrl-t", "alt-i".
pub fn parse_chord(s: &str) -> (KeyCode, KeyModifiers) {
    let mut mods = KeyModifiers::empty();
    let parts: Vec<&str> = s.split('-').collect();
    let last = parts.last().copied().unwrap_or("space");
    for p in &parts[..parts.len().saturating_sub(1)] {
        match *p {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "meta" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => {}
        }
    }
    let key = match last {
        "space" => KeyCode::Char(' '),
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" => KeyCode::Esc,
        s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        _ => KeyCode::Char(' '),
    };
    (key, mods)
}
