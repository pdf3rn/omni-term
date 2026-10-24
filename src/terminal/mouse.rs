//! Codificación de eventos de ratón para aplicaciones que los piden (vim, tmux, htop, less...).
//! Protocolos X10/normal, UTF-8 (1005) y SGR (1006), según los modos activos del terminal.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Press(Button),
    Release(Button),
    /// Movimiento, con el botón pulsado si lo hay.
    Motion(Option<Button>),
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// ¿Quiere la aplicación recibir eventos de ratón?
pub fn is_reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// Bytes a enviar para un evento en la celda (`col`, `row`), contadas desde 0.
/// `None` si el modo activo no pide ese evento o no se puede codificar.
pub fn encode(kind: Kind, col: usize, row: usize, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    if !is_reporting(mode) {
        return None;
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);

    let (mut code, release) = match kind {
        Kind::Press(button) => (button.code(), false),
        // El protocolo antiguo no dice qué botón se soltó; SGR sí.
        Kind::Release(button) => (if sgr { button.code() } else { 3 }, true),
        Kind::Motion(held) => {
            let allowed = mode.contains(TermMode::MOUSE_MOTION) || (mode.contains(TermMode::MOUSE_DRAG) && held.is_some());
            if !allowed {
                return None;
            }
            (32 + held.map_or(3, Button::code), false)
        }
        Kind::WheelUp => (64, false),
        Kind::WheelDown => (65, false),
    };
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }

    let (x, y) = (col as u32 + 1, row as u32 + 1);
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }

    let mut out = b"\x1b[M".to_vec();
    if mode.contains(TermMode::UTF8_MOUSE) {
        for value in [code, x, y] {
            let c = char::from_u32(value + 32)?;
            let mut buffer = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        }
    } else {
        for value in [code, x, y] {
            out.push(u8::try_from(value + 32).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_MODS: Mods = Mods { shift: false, alt: false, ctrl: false };
    const CLICK: TermMode = TermMode::MOUSE_REPORT_CLICK;

    fn sgr(base: TermMode) -> TermMode {
        base | TermMode::SGR_MOUSE
    }

    #[test]
    fn nothing_is_sent_unless_the_app_asked_for_the_mouse() {
        assert_eq!(encode(Kind::Press(Button::Left), 0, 0, NO_MODS, TermMode::empty()), None);
        assert_eq!(encode(Kind::WheelUp, 0, 0, NO_MODS, TermMode::SGR_MOUSE), None);
    }

    #[test]
    fn sgr_press_release_and_wheel() {
        let mode = sgr(CLICK);
        assert_eq!(encode(Kind::Press(Button::Left), 0, 0, NO_MODS, mode), Some(b"\x1b[<0;1;1M".to_vec()));
        assert_eq!(encode(Kind::Release(Button::Left), 9, 4, NO_MODS, mode), Some(b"\x1b[<0;10;5m".to_vec()));
        assert_eq!(encode(Kind::Press(Button::Right), 2, 2, NO_MODS, mode), Some(b"\x1b[<2;3;3M".to_vec()));
        assert_eq!(encode(Kind::WheelUp, 4, 2, NO_MODS, mode), Some(b"\x1b[<64;5;3M".to_vec()));
        assert_eq!(encode(Kind::WheelDown, 4, 2, NO_MODS, mode), Some(b"\x1b[<65;5;3M".to_vec()));
    }

    #[test]
    fn modifiers_add_to_the_button_code() {
        let mode = sgr(CLICK);
        let mods = Mods { shift: true, alt: true, ctrl: true };
        assert_eq!(encode(Kind::Press(Button::Left), 0, 0, mods, mode), Some(b"\x1b[<28;1;1M".to_vec()));
        let ctrl = Mods { ctrl: true, ..NO_MODS };
        assert_eq!(encode(Kind::Press(Button::Left), 0, 0, ctrl, mode), Some(b"\x1b[<16;1;1M".to_vec()));
    }

    #[test]
    fn motion_follows_the_requested_mode() {
        let click_only = sgr(CLICK);
        assert_eq!(encode(Kind::Motion(Some(Button::Left)), 1, 1, NO_MODS, click_only), None);

        let drag = sgr(TermMode::MOUSE_DRAG);
        assert_eq!(encode(Kind::Motion(Some(Button::Left)), 1, 1, NO_MODS, drag), Some(b"\x1b[<32;2;2M".to_vec()));
        assert_eq!(encode(Kind::Motion(None), 1, 1, NO_MODS, drag), None);

        let all = sgr(TermMode::MOUSE_MOTION);
        assert_eq!(encode(Kind::Motion(None), 1, 1, NO_MODS, all), Some(b"\x1b[<35;2;2M".to_vec()));
    }

    #[test]
    fn legacy_encoding_uses_offset_bytes_and_a_generic_release() {
        assert_eq!(encode(Kind::Press(Button::Left), 9, 4, NO_MODS, CLICK), Some(vec![0x1b, b'[', b'M', 32, 42, 37]));
        assert_eq!(encode(Kind::Release(Button::Right), 9, 4, NO_MODS, CLICK), Some(vec![0x1b, b'[', b'M', 35, 42, 37]));
    }

    #[test]
    fn legacy_encoding_cannot_reach_far_columns_but_utf8_can() {
        assert_eq!(encode(Kind::Press(Button::Left), 300, 0, NO_MODS, CLICK), None);
        let utf8 = CLICK | TermMode::UTF8_MOUSE;
        let bytes = encode(Kind::Press(Button::Left), 300, 0, NO_MODS, utf8).unwrap();
        assert_eq!(String::from_utf8(bytes[3..].to_vec()).unwrap(), format!("{}{}{}", ' ', char::from_u32(301 + 32).unwrap(), '!'));
    }
}
