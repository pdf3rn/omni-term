use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::vte::ansi::{Color, Rgb};
use eframe::egui::Color32;

/// Colores de la interfaz y del terminal: Catppuccin Mocha (oscuro) y Latte (claro).
pub struct Palette {
    base: Color32,
    mantle: Color32,
    text: Color32,
    subtext: Color32,
    surface: Color32,
    selection: Color32,
    cursor: Color32,
    red: Color32,
    accent: Color32,
    ansi: [(u8, u8, u8); 16],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn triple(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

static DARK: Palette = Palette {
    base: rgb(0x1e1e1e),
    mantle: rgb(0x181818),
    text: rgb(0xd4d4d4),
    subtext: rgb(0xa6a6a6),
    surface: rgb(0x303030),
    selection: rgb(0x575757),
    cursor: rgb(0xf5e0dc),
    red: rgb(0xf38ba8),
    accent: rgb(0x89b4fa),
    ansi: [
        triple(0x454545), triple(0xf38ba8), triple(0xa6e3a1), triple(0xf9e2af),
        triple(0x89b4fa), triple(0xf5c2e7), triple(0x94e2d5), triple(0xbababa),
        triple(0x575757), triple(0xf38ba8), triple(0xa6e3a1), triple(0xf9e2af),
        triple(0x89b4fa), triple(0xf5c2e7), triple(0x94e2d5), triple(0xa6a6a6),
    ],
};

static LIGHT: Palette = Palette {
    base: rgb(0xeff1f5),
    mantle: rgb(0xe6e9ef),
    text: rgb(0x4c4f69),
    subtext: rgb(0x555870),
    surface: rgb(0xccd0da),
    selection: rgb(0xacb0be),
    cursor: rgb(0xdc8a78),
    red: rgb(0xd20f39),
    accent: rgb(0x1e66f5),
    ansi: [
        triple(0x5c5f77), triple(0xd20f39), triple(0x40a02b), triple(0xdf8e1d),
        triple(0x1e66f5), triple(0xea76cb), triple(0x179299), triple(0xacb0be),
        triple(0x6c6f85), triple(0xd20f39), triple(0x40a02b), triple(0xdf8e1d),
        triple(0x1e66f5), triple(0xea76cb), triple(0x179299), triple(0xbcc0cc),
    ],
};

static USE_LIGHT: AtomicBool = AtomicBool::new(false);

pub fn set_light(light: bool) {
    USE_LIGHT.store(light, Ordering::Relaxed);
}

pub fn is_light() -> bool {
    USE_LIGHT.load(Ordering::Relaxed)
}

fn palette() -> &'static Palette {
    if is_light() { &LIGHT } else { &DARK }
}

pub fn base() -> Color32 {
    palette().base
}
pub fn mantle() -> Color32 {
    palette().mantle
}
pub fn text() -> Color32 {
    palette().text
}
pub fn subtext() -> Color32 {
    palette().subtext
}
pub fn surface() -> Color32 {
    palette().surface
}
pub fn selection() -> Color32 {
    palette().selection
}
pub fn cursor() -> Color32 {
    palette().cursor
}
pub fn red() -> Color32 {
    palette().red
}
pub fn accent() -> Color32 {
    palette().accent
}

/// Colores con los que se marca una conexión (rojo, naranja, amarillo, verde, azul, violeta).
pub const IDENTITY_NAMES: [&str; 6] = ["Rojo", "Naranja", "Amarillo", "Verde", "Azul", "Violeta"];

/// Color de identidad `index` (0..6), tomado de la paleta activa para que se lea en ambos temas.
pub fn identity(index: u8) -> Color32 {
    // En el tema claro los tonos pastel no alcanzan 3:1 sobre el fondo; se usan versiones más oscuras.
    const LIGHT_SET: [u32; 6] = [0xd20f39, 0xc2500a, 0xa66a00, 0x2d7a1c, 0x1e66f5, 0x8839ef];
    const DARK_SET: [u32; 6] = [0xf38ba8, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x89b4fa, 0xcba6f7];
    let set = if is_light() { &LIGHT_SET } else { &DARK_SET };
    rgb(set[(index as usize).min(5)])
}

fn ansi(index: usize) -> Color32 {
    let (r, g, b) = palette().ansi[index];
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
            256 | 267 => text(),
            257 => base(),
            258 => cursor(),
            i @ 259..=266 => dim(ansi(i - 259)),
            _ => dim(text()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let channel = |c: u8| {
            let v = c as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_is_readable_on_every_surface_in_both_themes() {
        for palette in [&DARK, &LIGHT] {
            for background in [palette.base, palette.mantle, palette.surface] {
                assert!(contrast(palette.text, background) >= 4.5, "texto principal");
                assert!(contrast(palette.subtext, background) >= 4.5, "texto secundario");
            }
        }
    }

    #[test]
    fn identity_colours_stand_out_from_the_backgrounds() {
        for light in [false, true] {
            set_light(light);
            for index in 0..6 {
                assert!(contrast(identity(index), mantle()) >= 3.0, "identidad {index} (claro: {light})");
            }
        }
        set_light(false);
    }
}
