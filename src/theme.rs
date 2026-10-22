use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
    ansi: [(u8, u8, u8); 16],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn triple(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Radios de esquina de toda la interfaz.
pub mod radius {
    /// Botones, campos, filas y menús desplegables.
    pub const CONTROL: u8 = 6;
    /// Tarjetas, píldoras y menús.
    pub const CARD: u8 = 8;
    /// Ventanas y diálogos.
    pub const DIALOG: u8 = 10;
    /// Fichas pequeñas, teclas y fondos de fila.
    pub const SMALL: u8 = 4;
    /// Marcas finas: barras de identidad, miniaturas.
    pub const TINY: u8 = 2;
}

/// Escala de espaciado.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 6.0;
    pub const M: f32 = 8.0;
    pub const L: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const XXL: f32 = 24.0;
    /// Entre filas de un mismo grupo de un formulario.
    pub const ROW: f32 = 8.0;
    /// Entre grupos de un formulario.
    pub const GROUP: f32 = 18.0;
}

/// Alturas fijas.
pub mod height {
    /// Campos de texto y selectores de los formularios.
    pub const FIELD: f32 = 28.0;
}

/// Tamaños de letra e iconos.
pub mod size {
    pub const CAPTION: f32 = 11.5;
    pub const SMALL: f32 = 12.0;
    /// Títulos de sección de tarjetas y listas.
    pub const SECTION: f32 = 12.5;
    pub const LABEL: f32 = 13.0;
    pub const BODY: f32 = 14.0;
    pub const HEADING: f32 = 16.0;
    /// Título de las pantallas (por ejemplo, «No hay pestañas abiertas»).
    pub const TITLE: f32 = 20.0;
    /// Estilo de título por defecto de egui.
    pub const DISPLAY: f32 = 18.0;
    pub const ICON_SM: f32 = 14.0;
    pub const ICON: f32 = 16.0;
    pub const ICON_LG: f32 = 17.0;
    pub const ICON_XL: f32 = 22.0;
}

/// Grosor de los bordes.
pub mod stroke {
    pub const HAIRLINE: f32 = 1.0;
    pub const MEDIUM: f32 = 1.5;
    pub const THICK: f32 = 2.0;
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
    ansi: [
        triple(0x454545), triple(0xf38ba8), triple(0xa6e3a1), triple(0xf9e2af),
        triple(0x89b4fa), triple(0xf5c2e7), triple(0x94e2d5), triple(0xbababa),
        triple(0x575757), triple(0xf38ba8), triple(0xa6e3a1), triple(0xf9e2af),
        triple(0x89b4fa), triple(0xf5c2e7), triple(0x94e2d5), triple(0xa6a6a6),
    ],
};

static LIGHT: Palette = Palette {
    base: rgb(0xf5f5f5),
    mantle: rgb(0xebebeb),
    text: rgb(0x2e2e2e),
    subtext: rgb(0x555555),
    surface: rgb(0xd6d6d6),
    selection: rgb(0xb0b0b0),
    cursor: rgb(0xdc8a78),
    red: rgb(0xd20f39),
    ansi: [
        triple(0x5a5a5a), triple(0xd20f39), triple(0x40a02b), triple(0xdf8e1d),
        triple(0x1e66f5), triple(0xea76cb), triple(0x179299), triple(0xb0b0b0),
        triple(0x6e6e6e), triple(0xd20f39), triple(0x40a02b), triple(0xdf8e1d),
        triple(0x1e66f5), triple(0xea76cb), triple(0x179299), triple(0xbdbdbd),
    ],
};

static USE_LIGHT: AtomicBool = AtomicBool::new(false);
static ACCENT: AtomicU8 = AtomicU8::new(0);
static COMPACT: AtomicBool = AtomicBool::new(false);
static BOLD_BRIGHT: AtomicBool = AtomicBool::new(false);
static ANSI_SCHEME: AtomicU8 = AtomicU8::new(0);

/// Colores de acento elegibles: azul, gris, verde y malva (oscuro, claro).
const ACCENTS: [(u32, u32); 4] = [(0x89b4fa, 0x1e66f5), (0xb9bbc6, 0x5c5c5c), (0xa6e3a1, 0x2d7a1c), (0xcba6f7, 0x8839ef)];

pub fn set_accent(index: u8) {
    ACCENT.store(index.min(ACCENTS.len() as u8 - 1), Ordering::Relaxed);
}

pub fn accent_index() -> u8 {
    ACCENT.load(Ordering::Relaxed)
}

pub fn set_compact(compact: bool) {
    COMPACT.store(compact, Ordering::Relaxed);
}

pub fn is_compact() -> bool {
    COMPACT.load(Ordering::Relaxed)
}

/// Altura de una fila de lista: la cómoda tal cual, la compacta algo más baja.
pub fn row(height: f32) -> f32 {
    if is_compact() { (height * 0.86).round() } else { height }
}

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
/// Sombra de las ventanas (`strong`) y de los menús desplegables.
pub fn shadow(strong: bool) -> Color32 {
    Color32::from_black_alpha(if strong { 110 } else { 90 })
}

pub fn cursor() -> Color32 {
    palette().cursor
}
pub fn red() -> Color32 {
    palette().red
}
pub fn accent() -> Color32 {
    let (dark, light) = ACCENTS[accent_index() as usize];
    rgb(if is_light() { light } else { dark })
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

/// Paletas ANSI alternativas, iguales en tema claro y oscuro (la 0, «Catppuccin», sale del tema).
const ANSI_SCHEMES: [[(u8, u8, u8); 16]; 2] = [
    // GNOME
    [
        triple(0x171421), triple(0xc01c28), triple(0x26a269), triple(0xa2734c),
        triple(0x12488b), triple(0xa347ba), triple(0x2aa1b3), triple(0xd0cfcc),
        triple(0x5e5c64), triple(0xf66151), triple(0x33da7a), triple(0xe9ad0c),
        triple(0x2a7bde), triple(0xc061cb), triple(0x33c7de), triple(0xffffff),
    ],
    // Tango
    [
        triple(0x2e3436), triple(0xcc0000), triple(0x4e9a06), triple(0xc4a000),
        triple(0x3465a4), triple(0x75507b), triple(0x06989a), triple(0xd3d7cf),
        triple(0x555753), triple(0xef2929), triple(0x8ae234), triple(0xfce94f),
        triple(0x729fcf), triple(0xad7fa8), triple(0x34e2e2), triple(0xeeeeec),
    ],
];

/// Elige la paleta de los 16 colores ANSI: 0 Catppuccin (la del tema), 1 GNOME, 2 Tango.
pub fn set_ansi_scheme(index: u8) {
    ANSI_SCHEME.store(index, Ordering::Relaxed);
}

fn ansi(index: usize) -> Color32 {
    let (r, g, b) = match ANSI_SCHEME.load(Ordering::Relaxed) {
        scheme @ 1..=2 => ANSI_SCHEMES[scheme as usize - 1][index],
        _ => palette().ansi[index],
    };
    Color32::from_rgb(r, g, b)
}

/// La negrita aclara los colores ANSI 0-7 (compatibilidad con terminales antiguos).
pub fn set_bold_bright(on: bool) {
    BOLD_BRIGHT.store(on, Ordering::Relaxed);
}

pub fn dim(color: Color32) -> Color32 {
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

/// Color de una celda; en negrita, los 8 ANSI básicos pasan a su versión brillante si está activado.
pub fn resolve_styled(color: Color, bold: bool) -> Color32 {
    if bold && BOLD_BRIGHT.load(Ordering::Relaxed) {
        match color {
            Color::Named(named) if (named as usize) < 8 => return ansi(named as usize + 8),
            Color::Indexed(i) if i < 8 => return ansi(i as usize + 8),
            _ => {}
        }
    }
    resolve(color)
}

/// Color que se responde a una consulta OSC 4/10/11/12 del programa: 0-255 de la paleta, 256 texto, 257 fondo, 258 cursor.
pub fn query_color(index: usize) -> Rgb {
    let color = match index {
        0..=255 => indexed(index as u8),
        256 => text(),
        257 => base(),
        _ => cursor(),
    };
    Rgb { r: color.r(), g: color.g(), b: color.b() }
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
    fn every_accent_reads_against_the_base_and_takes_dark_text() {
        for light in [false, true] {
            set_light(light);
            for index in 0..ACCENTS.len() as u8 {
                set_accent(index);
                assert!(contrast(accent(), base()) >= 3.0, "acento {index} sobre el fondo (claro: {light})");
                assert!(contrast(base(), accent()) >= 3.0, "texto del botón sobre el acento {index} (claro: {light})");
            }
        }
        set_light(false);
        set_accent(0);
    }

    #[test]
    fn bold_makes_basic_colours_bright_only_when_enabled() {
        use alacritty_terminal::vte::ansi::NamedColor;
        let red = Color::Named(NamedColor::Red);
        set_bold_bright(false);
        assert_eq!(resolve_styled(red, true), resolve(red));
        set_bold_bright(true);
        assert_eq!(resolve_styled(red, true), ansi(9));
        assert_eq!(resolve_styled(Color::Indexed(1), true), ansi(9));
        assert_eq!(resolve_styled(red, false), resolve(red));
        assert_eq!(resolve_styled(Color::Indexed(200), true), resolve(Color::Indexed(200)));
        set_bold_bright(false);
    }

    #[test]
    fn the_ansi_scheme_changes_the_sixteen_colours() {
        set_ansi_scheme(0);
        let pastel = ansi(3);
        set_ansi_scheme(1);
        assert_eq!(ansi(3), Color32::from_rgb(0xa2, 0x73, 0x4c));
        set_ansi_scheme(2);
        assert_eq!(ansi(4), Color32::from_rgb(0x34, 0x65, 0xa4));
        set_ansi_scheme(0);
        assert_eq!(ansi(3), pastel);
    }

    #[test]
    fn colour_queries_answer_with_the_theme_colours() {
        set_light(false);
        let background = query_color(257);
        assert_eq!((background.r, background.g, background.b), (base().r(), base().g(), base().b()));
        let foreground = query_color(256);
        assert_eq!((foreground.r, foreground.g, foreground.b), (text().r(), text().g(), text().b()));
        let green = query_color(2);
        assert_eq!((green.r, green.g, green.b), (ansi(2).r(), ansi(2).g(), ansi(2).b()));
        let cube = query_color(196);
        assert_eq!((cube.r, cube.g, cube.b), (255, 0, 0));
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
