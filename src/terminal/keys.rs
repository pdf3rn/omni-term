//! Traduce teclas de egui a los bytes que espera la aplicación del terminal.
//!
//! Codificación legada (xterm) o protocolo kitty según los modos que la
//! aplicación haya activado en `alacritty_terminal`.

use alacritty_terminal::term::TermMode;
use eframe::egui::{Key, Modifiers};
use termwiz::input::{KeyCode as LegacyCode, KeyCodeEncodeModes, KeyboardEncoding, Modifiers as TwMods};

/// Tecla independiente de la librería de codificación.
#[derive(Clone, Copy)]
enum Code {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Function(u8),
}

impl Code {
    fn legacy(self) -> LegacyCode {
        match self {
            Code::Char('\t') => LegacyCode::Tab,
            Code::Char(c) => LegacyCode::Char(c),
            Code::Up => LegacyCode::UpArrow,
            Code::Down => LegacyCode::DownArrow,
            Code::Left => LegacyCode::LeftArrow,
            Code::Right => LegacyCode::RightArrow,
            Code::Home => LegacyCode::Home,
            Code::End => LegacyCode::End,
            Code::PageUp => LegacyCode::PageUp,
            Code::PageDown => LegacyCode::PageDown,
            Code::Insert => LegacyCode::Insert,
            Code::Function(n) => LegacyCode::Function(n),
        }
    }
}

#[derive(Clone, Copy)]
struct Kitty {
    event_types: bool,
    all_keys: bool,
}

pub struct Press {
    pub key: Key,
    pub mods: Modifiers,
    pub pressed: bool,
    pub repeat: bool,
}

/// `true` si el modo pide que hasta el texto normal viaje como secuencia de escape.
pub fn reports_all_keys(mode: TermMode) -> bool {
    mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC)
}

/// Bytes a enviar para esta pulsación. `None` si no hay nada que enviar por aquí
/// (por ejemplo, texto normal que ya llega como `Event::Text`).
pub fn encode(press: &Press, mode: TermMode) -> Option<Vec<u8>> {
    let kitty = kitty_mode(mode);
    let code = keycode(press.key, press.mods, kitty.is_some())?;

    let is_text_key = matches!(code, Code::Char(c) if c.is_ascii_graphic() || c == ' ');
    let modified = press.mods.ctrl || press.mods.alt;
    if is_text_key && !modified && !reports_all_keys(mode) {
        return None;
    }

    let bytes = match kitty {
        Some(kitty) => encode_kitty(code, press, kitty)?,
        None => {
            let modes = KeyCodeEncodeModes {
                encoding: KeyboardEncoding::Xterm,
                application_cursor_keys: mode.contains(TermMode::APP_CURSOR),
                newline_mode: false,
                modify_other_keys: None,
            };
            code.legacy().encode(termwiz_mods(press.mods), modes, press.pressed).ok()?
        }
    };
    (!bytes.is_empty()).then(|| bytes.into_bytes())
}

fn kitty_mode(mode: TermMode) -> Option<Kitty> {
    mode.contains(TermMode::DISAMBIGUATE_ESC_CODES).then_some(Kitty {
        event_types: mode.contains(TermMode::REPORT_EVENT_TYPES),
        all_keys: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
    })
}

/// Protocolo kitty (flags 1, 2 y 8). No cubre teclas alternativas ni texto asociado.
fn encode_kitty(code: Code, press: &Press, kitty: Kitty) -> Option<String> {
    if !press.pressed && !kitty.event_types {
        return None;
    }

    let mut mods = 1;
    if press.mods.shift {
        mods += 1;
    }
    if press.mods.alt {
        mods += 2;
    }
    if press.mods.ctrl {
        mods += 4;
    }
    let event = match (press.pressed, press.repeat) {
        (false, _) => 3,
        (true, true) => 2,
        (true, false) => 1,
    };
    let kitty_event = kitty.event_types && event != 1;
    let suffix = |lead: &str, end: char| {
        let mods_field = if mods == 1 && !kitty_event { String::new() } else { format!(";{mods}") };
        let event_field = if kitty_event { format!(":{event}") } else { String::new() };
        format!("\x1b[{lead}{mods_field}{event_field}{end}")
    };
    let csi_u = |codepoint: u32| suffix(&codepoint.to_string(), 'u');
    let tilde = |n: u32| suffix(&n.to_string(), '~');
    let letter = |c: char| {
        if mods == 1 && !kitty_event {
            format!("\x1b[{c}")
        } else {
            suffix("1", c)
        }
    };

    Some(match code {
        Code::Char(c @ ('\r' | '\t' | '\x08')) => {
            let codepoint = match c {
                '\r' => 13,
                '\t' => 9,
                _ => 127,
            };
            let legacy = mods == 1 && !kitty.all_keys;
            if legacy {
                if !press.pressed {
                    return None;
                }
                match c {
                    '\x08' => '\x7f'.to_string(),
                    c => c.to_string(),
                }
            } else {
                csi_u(codepoint)
            }
        }
        Code::Char('\x7f') => tilde(3),
        Code::Char(c) => csi_u(c as u32),
        Code::Up => letter('A'),
        Code::Down => letter('B'),
        Code::Right => letter('C'),
        Code::Left => letter('D'),
        Code::Home => letter('H'),
        Code::End => letter('F'),
        Code::Insert => tilde(2),
        Code::PageUp => tilde(5),
        Code::PageDown => tilde(6),
        Code::Function(n) => match n {
            1 => letter('P'),
            2 => letter('Q'),
            3 => letter('R'),
            4 => letter('S'),
            5 => tilde(15),
            6 => tilde(17),
            7 => tilde(18),
            8 => tilde(19),
            9 => tilde(20),
            10 => tilde(21),
            11 => tilde(23),
            12 => tilde(24),
            13..=20 => csi_u(57376 + (n as u32 - 13)),
            _ => return None,
        },
    })
}

fn termwiz_mods(mods: Modifiers) -> TwMods {
    let mut out = TwMods::NONE;
    if mods.shift {
        out |= TwMods::SHIFT;
    }
    if mods.alt {
        out |= TwMods::ALT;
    }
    if mods.ctrl {
        out |= TwMods::CTRL;
    }
    out
}

/// Con Ctrl+Shift, la tecla produce el símbolo de la capa superior (distribución US).
fn ctrl_shifted_symbol(key: Key) -> Option<char> {
    Some(match key {
        Key::Num2 => '@',
        Key::Num6 => '^',
        Key::Minus => '_',
        _ => return None,
    })
}

fn keycode(key: Key, mods: Modifiers, kitty: bool) -> Option<Code> {
    use Key::*;
    let special = match key {
        Enter => Code::Char('\r'),
        Tab => Code::Char('\t'),
        Backspace => Code::Char('\x08'),
        Escape => Code::Char('\x1b'),
        Delete => Code::Char('\x7f'),
        Space => Code::Char(' '),
        ArrowUp => Code::Up,
        ArrowDown => Code::Down,
        ArrowLeft => Code::Left,
        ArrowRight => Code::Right,
        Home => Code::Home,
        End => Code::End,
        PageUp => Code::PageUp,
        PageDown => Code::PageDown,
        Insert => Code::Insert,
        F1 => Code::Function(1),
        F2 => Code::Function(2),
        F3 => Code::Function(3),
        F4 => Code::Function(4),
        F5 => Code::Function(5),
        F6 => Code::Function(6),
        F7 => Code::Function(7),
        F8 => Code::Function(8),
        F9 => Code::Function(9),
        F10 => Code::Function(10),
        F11 => Code::Function(11),
        F12 => Code::Function(12),
        F13 => Code::Function(13),
        F14 => Code::Function(14),
        F15 => Code::Function(15),
        F16 => Code::Function(16),
        F17 => Code::Function(17),
        F18 => Code::Function(18),
        F19 => Code::Function(19),
        F20 => Code::Function(20),
        _ => return char_key(key, mods, kitty).map(Code::Char),
    };
    Some(special)
}

fn char_key(key: Key, mods: Modifiers, kitty: bool) -> Option<char> {
    if !kitty && mods.ctrl && mods.shift {
        if let Some(c) = ctrl_shifted_symbol(key) {
            return Some(c);
        }
    }
    use Key::*;
    Some(match key {
        Num0 => '0',
        Num1 => '1',
        Num2 => '2',
        Num3 => '3',
        Num4 => '4',
        Num5 => '5',
        Num6 => '6',
        Num7 => '7',
        Num8 => '8',
        Num9 => '9',
        Minus => '-',
        Equals => '=',
        Plus => '+',
        Comma => ',',
        Period => '.',
        Slash => '/',
        Backslash => '\\',
        Semicolon => ';',
        Colon => ':',
        Quote => '\'',
        Backtick => '`',
        OpenBracket => '[',
        CloseBracket => ']',
        OpenCurlyBracket => '{',
        CloseCurlyBracket => '}',
        Pipe => '|',
        Questionmark => '?',
        Exclamationmark => '!',
        _ => {
            let mut chars = key.name().chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => c.to_ascii_lowercase(),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;
    const CTRL: Modifiers = Modifiers { ctrl: true, ..Modifiers::NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };
    const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };
    const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };
    const CTRL_ALT: Modifiers = Modifiers { ctrl: true, alt: true, ..Modifiers::NONE };

    fn legacy(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
        encode(&Press { key, mods, pressed: true, repeat: false }, TermMode::empty())
    }

    fn with_mode(key: Key, mods: Modifiers, pressed: bool, mode: TermMode) -> Option<Vec<u8>> {
        encode(&Press { key, mods, pressed, repeat: false }, mode)
    }

    #[test]
    fn plain_text_is_left_to_text_events() {
        assert_eq!(legacy(Key::A, NONE), None);
        assert_eq!(legacy(Key::A, SHIFT), None);
        assert_eq!(legacy(Key::Num1, NONE), None);
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        assert_eq!(legacy(Key::X, CTRL), Some(vec![0x18]));
        assert_eq!(legacy(Key::C, CTRL), Some(vec![0x03]));
        assert_eq!(legacy(Key::O, CTRL), Some(vec![0x0f]));
        assert_eq!(legacy(Key::Z, CTRL), Some(vec![0x1a]));
    }

    #[test]
    fn ctrl_punctuation_used_by_nano_and_emacs() {
        assert_eq!(legacy(Key::Space, CTRL), Some(vec![0x00]));
        assert_eq!(legacy(Key::Backslash, CTRL), Some(vec![0x1c]));
        assert_eq!(legacy(Key::CloseBracket, CTRL), Some(vec![0x1d]));
        assert_eq!(legacy(Key::Slash, CTRL), Some(vec![0x1f]));
        assert_eq!(legacy(Key::Minus, CTRL_SHIFT), Some(vec![0x1f]));
        assert_eq!(legacy(Key::Num2, CTRL_SHIFT), Some(vec![0x00]));
        assert_eq!(legacy(Key::Num6, CTRL_SHIFT), Some(vec![0x1e]));
        assert_eq!(legacy(Key::Num6, CTRL), Some(vec![0x1e]));
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(legacy(Key::X, ALT), Some(b"\x1bx".to_vec()));
        assert_eq!(legacy(Key::Period, ALT), Some(b"\x1b.".to_vec()));
        assert_eq!(legacy(Key::Num3, ALT), Some(b"\x1b3".to_vec()));
        assert_eq!(legacy(Key::A, CTRL_ALT), Some(b"\x1b\x01".to_vec()));
    }

    #[test]
    fn special_keys() {
        assert_eq!(legacy(Key::Enter, NONE), Some(b"\r".to_vec()));
        assert_eq!(legacy(Key::Backspace, NONE), Some(vec![0x7f]));
        assert_eq!(legacy(Key::Tab, NONE), Some(b"\t".to_vec()));
        assert_eq!(legacy(Key::Tab, SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(legacy(Key::Escape, NONE), Some(b"\x1b".to_vec()));
        assert_eq!(legacy(Key::Delete, NONE), Some(b"\x1b[3~".to_vec()));
        assert_eq!(legacy(Key::PageUp, NONE), Some(b"\x1b[5~".to_vec()));
        assert_eq!(legacy(Key::F5, NONE), Some(b"\x1b[15~".to_vec()));
        assert_eq!(legacy(Key::F1, NONE), Some(b"\x1bOP".to_vec()));
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(legacy(Key::ArrowUp, NONE), Some(b"\x1b[A".to_vec()));
        assert_eq!(with_mode(Key::ArrowUp, NONE, true, TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));
        assert_eq!(legacy(Key::ArrowLeft, CTRL), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(legacy(Key::ArrowRight, ALT), Some(b"\x1b[1;3C".to_vec()));
    }

    #[test]
    fn releases_are_ignored_in_legacy_mode() {
        assert_eq!(with_mode(Key::X, CTRL, false, TermMode::empty()), None);
        assert_eq!(with_mode(Key::Enter, NONE, false, TermMode::empty()), None);
    }

    #[test]
    fn kitty_disambiguates_escape_and_ctrl_letters() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(with_mode(Key::Escape, NONE, true, mode), Some(b"\x1b[27u".to_vec()));
        assert_eq!(with_mode(Key::C, CTRL, true, mode), Some(b"\x1b[99;5u".to_vec()));
        assert_eq!(with_mode(Key::Enter, CTRL, true, mode), Some(b"\x1b[13;5u".to_vec()));
        assert_eq!(with_mode(Key::A, NONE, true, mode), None);
    }

    #[test]
    fn kitty_reports_release_events_when_requested() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(with_mode(Key::C, CTRL, false, mode), Some(b"\x1b[99;5:3u".to_vec()));
        let without = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(with_mode(Key::C, CTRL, false, without), None);
    }

    #[test]
    fn kitty_all_keys_as_escape_codes_covers_text() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert!(reports_all_keys(mode));
        assert_eq!(with_mode(Key::A, NONE, true, mode), Some(b"\x1b[97u".to_vec()));
        assert_eq!(with_mode(Key::A, SHIFT, true, mode), Some(b"\x1b[97;2u".to_vec()));
    }

    #[test]
    fn kitty_repeat_and_enter_rules() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        let repeat = Press { key: Key::C, mods: CTRL, pressed: true, repeat: true };
        assert_eq!(encode(&repeat, mode), Some(b"\x1b[99;5:2u".to_vec()));
        assert_eq!(with_mode(Key::Enter, NONE, true, mode), Some(b"\r".to_vec()));
        assert_eq!(with_mode(Key::Enter, NONE, false, mode), None);
        assert_eq!(with_mode(Key::Tab, SHIFT, true, mode), Some(b"\x1b[9;2u".to_vec()));
        assert_eq!(with_mode(Key::ArrowUp, NONE, true, mode), Some(b"\x1b[A".to_vec()));
        assert_eq!(with_mode(Key::F5, CTRL, true, mode), Some(b"\x1b[15;5~".to_vec()));
    }
}
