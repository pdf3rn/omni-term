use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use alacritty_terminal::vte::ansi::{Color, Rgb};
use eframe::egui::{Color32, Stroke};

use crate::data::term_theme::{self, TermTheme};

/// Colores de la interfaz (grises neutros) y del terminal (paleta ANSI basada en Catppuccin Mocha y Latte).
pub struct Palette {
    base: Color32,
    mantle: Color32,
    sunken: Color32,
    text: Color32,
    subtext: Color32,
    surface: Color32,
    selection: Color32,
    hover: Color32,
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
    pub const CAPTION: f32 = 12.0;
    pub const SMALL: f32 = 12.0;
    /// Títulos de sección de tarjetas y listas.
    pub const SECTION: f32 = 13.0;
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

/// Contorno de lo que tiene el foco del teclado: acento translúcido y fino, el mismo en campos, botones y filas.
pub fn focus_stroke() -> Stroke {
    Stroke::new(stroke::HAIRLINE, accent().gamma_multiply(0.6))
}

/// Grosor de los bordes.
pub mod stroke {
    pub const HAIRLINE: f32 = 1.0;
    /// Contornos de foco y de soltar. Entero, para que a escala 1x no caiga entre píxeles y se vea borroso.
    pub const MEDIUM: f32 = 2.0;
    pub const THICK: f32 = 2.0;
}

static DARK: Palette = Palette {
    base: rgb(0x1e1e1e),
    mantle: rgb(0x181818),
    sunken: rgb(0x181818),
    text: rgb(0xd4d4d4),
    subtext: rgb(0xa6a6a6),
    surface: rgb(0x303030),
    selection: rgb(0x575757),
    hover: rgb(0x3c3c3c),
    cursor: rgb(0xf5e0dc),
    red: rgb(0xf38ba8),
    // Los brillantes (8-15) son versiones más claras de los normales: la negrita brillante se nota.
    ansi: [
        triple(0x5a5a5a), triple(0xf38ba8), triple(0xa6e3a1), triple(0xf9e2af),
        triple(0x89b4fa), triple(0xf5c2e7), triple(0x94e2d5), triple(0xbababa),
        triple(0x7f7f7f), triple(0xf7aec2), triple(0xc1ebbd), triple(0xfbebc7),
        triple(0xaccafc), triple(0xf8d4ee), triple(0xb4ebe2), triple(0xe6e6e6),
    ],
};

static LIGHT: Palette = Palette {
    base: rgb(0xf5f5f5),
    mantle: rgb(0xebebeb),
    sunken: rgb(0xdcdcdc),
    text: rgb(0x2e2e2e),
    subtext: rgb(0x555555),
    surface: rgb(0xd6d6d6),
    selection: rgb(0xb0b0b0),
    hover: rgb(0xd0d0d0),
    cursor: rgb(0xdc8a78),
    red: rgb(0xd20f39),
    // Todos los colores de texto llegan a 3:1 sobre el fondo; el «blanco» es un gris medio para que se lea.
    ansi: [
        triple(0x3c3c3c), triple(0xd20f39), triple(0x40a02b), triple(0xb06d00),
        triple(0x1e66f5), triple(0xb8369c), triple(0x137f86), triple(0x767676),
        triple(0x5c5c5c), triple(0xe64553), triple(0x2f8a1c), triple(0xc27a00),
        triple(0x4a83f7), triple(0xc94cad), triple(0x1a9aa1), triple(0x8a8a8a),
    ],
};

static USE_LIGHT: AtomicBool = AtomicBool::new(false);
static ACCENT: AtomicU8 = AtomicU8::new(0);
static COMPACT: AtomicBool = AtomicBool::new(false);
static BOLD_BRIGHT: AtomicBool = AtomicBool::new(false);

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
/// Fondo más hundido que el de los diálogos, para paneles laterales dentro de ellos.
pub fn sunken() -> Color32 {
    palette().sunken
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
/// Fondo de un control pulsado o seleccionado.
pub fn selection() -> Color32 {
    palette().selection
}
/// Fondo de un control con el ratón encima: más suave que `selection`, que queda para el pulsado.
pub fn hover() -> Color32 {
    palette().hover
}
/// Rojo del botón de cerrar la ventana al pasar el ratón: lleva una X blanca en los dos temas.
pub fn close_hover() -> Color32 {
    rgb(0xc42b1c)
}
/// Verde de estado («conectado»): no depende del acento elegido.
pub fn success() -> Color32 {
    identity(3)
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

/// Elige el tema de color del terminal (posición en `term_theme::THEMES`).
pub fn set_term_theme(index: u8) {
    term_theme::set(index);
}

fn from_hex(hex: u32) -> Color32 {
    rgb(hex)
}

fn ansi(index: usize) -> Color32 {
    match term_theme::custom() {
        Some(theme) => from_hex(theme.ansi[index]),
        None => {
            let (r, g, b) = palette().ansi[index];
            Color32::from_rgb(r, g, b)
        }
    }
}

/// Fondo del terminal.
pub fn term_bg() -> Color32 {
    term_theme::custom().map_or_else(base, |t| from_hex(t.bg))
}

/// Color del texto del terminal.
pub fn term_fg() -> Color32 {
    term_theme::custom().map_or_else(text, |t| from_hex(t.fg))
}

/// Color del cursor del terminal.
pub fn term_cursor() -> Color32 {
    term_theme::custom().map_or_else(cursor, |t| from_hex(t.cursor))
}

/// Fondo del texto seleccionado en el terminal: el del tema, o el acento mezclado con el fondo (opaco, para que
/// cualquier color de texto siga leyéndose encima).
pub fn term_selection() -> Color32 {
    match term_theme::custom() {
        Some(TermTheme { selection: Some(hex), .. }) => from_hex(*hex),
        _ => {
            let bg = term_bg();
            let light_bg = bg.r() as u32 + bg.g() as u32 + bg.b() as u32 > 3 * 140;
            bg.lerp_to_gamma(accent(), if light_bg { 0.22 } else { 0.3 })
        }
    }
}

/// Fondo, texto y los seis colores de la vista previa (rojo, verde, amarillo, azul, magenta, cian) del tema `index`.
pub fn term_theme_preview(index: u8) -> (Color32, Color32, [Color32; 6]) {
    if index == term_theme::INTERFACE {
        let ansi = palette().ansi;
        let pick = |i: usize| Color32::from_rgb(ansi[i].0, ansi[i].1, ansi[i].2);
        return (base(), text(), [pick(1), pick(2), pick(3), pick(4), pick(5), pick(6)]);
    }
    let theme = &term_theme::THEMES[index as usize];
    let pick = |i: usize| from_hex(theme.ansi[i]);
    (from_hex(theme.bg), from_hex(theme.fg), [pick(1), pick(2), pick(3), pick(4), pick(5), pick(6)])
}

/// La negrita aclara los colores ANSI 0-7 (compatibilidad con terminales antiguos).
pub fn set_bold_bright(on: bool) {
    BOLD_BRIGHT.store(on, Ordering::Relaxed);
}

/// Texto atenuado (SGR 2): se acerca al fondo, así baja el contraste en los dos temas.
pub fn dim(color: Color32) -> Color32 {
    color.lerp_to_gamma(term_bg(), 0.4)
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
        256 => term_fg(),
        257 => term_bg(),
        _ => term_cursor(),
    };
    Rgb { r: color.r(), g: color.g(), b: color.b() }
}

pub fn resolve(color: Color) -> Color32 {
    match color {
        Color::Spec(Rgb { r, g, b }) => Color32::from_rgb(r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Named(named) => match named as usize {
            i @ 0..=15 => ansi(i),
            256 | 267 => term_fg(),
            257 => term_bg(),
            258 => term_cursor(),
            i @ 259..=266 => dim(ansi(i - 259)),
            _ => dim(term_fg()),
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
    fn hover_keeps_text_readable() {
        for palette in [&DARK, &LIGHT] {
            assert!(contrast(palette.text, palette.hover) >= 4.5, "texto sobre el hover");
            assert!(contrast(palette.subtext, palette.hover) >= 4.5, "texto secundario sobre el hover");
            assert_ne!(palette.hover, palette.surface, "el hover se distingue del relleno de los botones");
            assert_ne!(palette.hover, palette.selection, "el hover se distingue del pulsado");
        }
    }

    #[test]
    fn ansi_text_colours_read_on_the_background() {
        for palette in [&DARK, &LIGHT] {
            let base = palette.base;
            // 1-7 y 9-15 se usan como color de texto; 0 y 8 (negros) suelen ser fondos.
            for index in (1..8).chain(9..16) {
                let (r, g, b) = palette.ansi[index];
                assert!(contrast(Color32::from_rgb(r, g, b), base) >= 3.0, "ANSI {index}");
            }
            let (r, g, b) = palette.ansi[0];
            assert!(contrast(Color32::from_rgb(r, g, b), base) >= 1.8, "ANSI 0 casi invisible");
        }
    }

    #[test]
    fn bright_colours_differ_from_the_normal_ones() {
        for palette in [&DARK, &LIGHT] {
            for index in 0..8 {
                assert_ne!(palette.ansi[index], palette.ansi[index + 8], "ANSI {index} y {}", index + 8);
            }
        }
    }

    #[test]
    fn selected_text_stays_readable() {
        term_theme::set(term_theme::INTERFACE);
        for light in [false, true] {
            set_light(light);
            for accent in 0..ACCENTS.len() as u8 {
                set_accent(accent);
                assert!(contrast(text(), term_selection()) >= 4.5, "texto sobre la selección (acento {accent}, claro: {light})");
                for index in [1usize, 2, 4, 7] {
                    let (r, g, b) = palette().ansi[index];
                    assert!(contrast(Color32::from_rgb(r, g, b), term_selection()) >= 2.0, "ANSI {index} seleccionado (claro: {light})");
                }
            }
        }
        set_light(false);
        set_accent(0);
        term_theme::set(0);
    }

    #[test]
    fn dim_text_has_less_contrast_in_both_themes() {
        term_theme::set(term_theme::INTERFACE);
        for light in [false, true] {
            set_light(light);
            assert!(contrast(dim(text()), base()) < contrast(text(), base()), "claro: {light}");
            assert!(contrast(dim(text()), base()) >= 3.0, "claro: {light}");
        }
        set_light(false);
        term_theme::set(0);
    }

    #[test]
    fn every_terminal_theme_is_readable() {
        for (index, theme) in term_theme::THEMES.iter().enumerate() {
            if index as u8 == term_theme::INTERFACE {
                continue;
            }
            let (bg, fg) = (rgb(theme.bg), rgb(theme.fg));
            // Solarized Light es así de suave (4,1:1); el resto supera 7:1.
            assert!(contrast(fg, bg) >= 4.0, "{}: texto sobre el fondo", theme.name);
            assert!(contrast(rgb(theme.cursor), bg) >= 3.0, "{}: cursor sobre el fondo", theme.name);
            if let Some(selection) = theme.selection {
                assert!(contrast(fg, rgb(selection)) >= 3.0, "{}: texto sobre la selección", theme.name);
            }
            // Los seis colores de texto (rojo a cian, normales y brillantes); los negros y los blancos suelen hacer de fondo.
            for ansi in (1..7).chain(9..15) {
                assert!(contrast(rgb(theme.ansi[ansi]), bg) >= 1.8, "{}: ANSI {ansi} sobre el fondo", theme.name);
            }
        }
    }

    #[test]
    fn white_reads_on_the_window_close_red() {
        assert!(contrast(Color32::WHITE, close_hover()) >= 4.5);
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
    fn the_terminal_theme_changes_the_sixteen_colours() {
        set_term_theme(term_theme::INTERFACE);
        let interface = ansi(3);
        set_term_theme(0);
        assert_eq!(ansi(3), Color32::from_rgb(0xa2, 0x73, 0x4c));
        set_term_theme(2);
        assert_eq!(ansi(4), Color32::from_rgb(0x34, 0x65, 0xa4));
        set_term_theme(term_theme::INTERFACE);
        assert_eq!(ansi(3), interface);
        set_term_theme(0);
    }

    #[test]
    fn the_terminal_theme_sets_background_text_and_cursor() {
        set_term_theme(0);
        assert_eq!(term_bg(), Color32::from_rgb(0x17, 0x14, 0x21));
        assert_eq!(term_fg(), Color32::from_rgb(0xd0, 0xcf, 0xcc));
        assert_eq!(resolve(Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background)), term_bg());
        set_term_theme(term_theme::INTERFACE);
        assert_eq!(term_bg(), base());
        assert_eq!(term_fg(), text());
        set_term_theme(0);
    }

    #[test]
    fn colour_queries_answer_with_the_theme_colours() {
        set_term_theme(term_theme::INTERFACE);
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
