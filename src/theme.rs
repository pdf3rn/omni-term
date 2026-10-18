use alacritty_terminal::vte::ansi::{Color, Rgb};
use eframe::egui::Color32;

pub const BASE: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x2e);
pub const MANTLE: Color32 = Color32::from_rgb(0x18, 0x18, 0x25);
pub const SUBTEXT: Color32 = Color32::from_rgb(0xa6, 0xad, 0xc8);
pub const SURFACE: Color32 = Color32::from_rgb(0x31, 0x32, 0x44);
pub const TEXT: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xf4);
pub const SELECTION: Color32 = Color32::from_rgb(0x58, 0x5b, 0x70);
pub const CURSOR: Color32 = Color32::from_rgb(0xf5, 0xe0, 0xdc);

const ANSI: [(u8, u8, u8); 16] = [
    (0x45, 0x47, 0x5a),
    (0xf3, 0x8b, 0xa8),
    (0xa6, 0xe3, 0xa1),
    (0xf9, 0xe2, 0xaf),
    (0x89, 0xb4, 0xfa),
    (0xf5, 0xc2, 0xe7),
    (0x94, 0xe2, 0xd5),
    (0xba, 0xc2, 0xde),
    (0x58, 0x5b, 0x70),
    (0xf3, 0x8b, 0xa8),
    (0xa6, 0xe3, 0xa1),
    (0xf9, 0xe2, 0xaf),
    (0x89, 0xb4, 0xfa),
    (0xf5, 0xc2, 0xe7),
    (0x94, 0xe2, 0xd5),
    (0xa6, 0xad, 0xc8),
];

fn ansi(index: usize) -> Color32 {
    let (r, g, b) = ANSI[index];
    Color32::from_rgb(r, g, b)
}

fn dim(color: Color32) -> Color32 {
    let scale = |c: u8| (c as f32 * 0.66) as u8;
    Color32::from_rgb(scale(color.r()), scale(color.g()), scale(color.b()))
}

fn indexed(index: u8) -> Color32 {
    match index {
        0..=15 => ansi(index as usize),
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

pub fn resolve(color: Color) -> Color32 {
    match color {
        Color::Spec(Rgb { r, g, b }) => Color32::from_rgb(r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Named(named) => match named as usize {
            i @ 0..=15 => ansi(i),
            256 | 267 => TEXT,
            257 => BASE,
            258 => CURSOR,
            i @ 259..=266 => dim(ansi(i - 259)),
            _ => dim(TEXT),
        },
    }
}
