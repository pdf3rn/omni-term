//! Temas de color del terminal: fondo, texto, cursor, selección y los 16 colores ANSI. Son independientes del tema
//! claro u oscuro de la interfaz; «Interfaz» es la excepción y toma sus colores de ella.

use std::sync::atomic::{AtomicU8, Ordering};

pub struct TermTheme {
    /// Clave que se guarda en los ajustes: cambiarla dejaría sin su elección a quien ya la tenga guardada.
    pub key: &'static str,
    pub name: &'static str,
    /// `false`: fondo, texto y cursor son los de la interfaz (los tres valores siguientes no se usan) y el tema solo
    /// aporta su paleta de 16 colores.
    pub own_surface: bool,
    pub bg: u32,
    pub fg: u32,
    pub cursor: u32,
    /// Fondo del texto seleccionado; sin él se mezcla el acento con el fondo.
    pub selection: Option<u32>,
    pub ansi: [u32; 16],
}

/// Posición de «Interfaz» en `THEMES`: usa los colores de la interfaz en vez de los suyos.
pub const INTERFACE: u8 = 1;

/// GNOME va primero: es el tema por defecto. Los demás siguen las paletas publicadas de cada proyecto; «Claude»
/// es propio, inspirado en los colores de marca de Anthropic, no un tema oficial.
pub const THEMES: [TermTheme; 12] = [
    TermTheme {
        key: "gnome",
        name: "GNOME",
        // Solo la paleta de GNOME: el fondo y el texto son los de la interfaz, como antes de que hubiera más temas.
        own_surface: false,
        bg: 0,
        fg: 0,
        cursor: 0,
        selection: None,
        ansi: [
            0x171421, 0xc01c28, 0x26a269, 0xa2734c, 0x12488b, 0xa347ba, 0x2aa1b3, 0xd0cfcc,
            0x5e5c64, 0xf66151, 0x33da7a, 0xe9ad0c, 0x2a7bde, 0xc061cb, 0x33c7de, 0xffffff,
        ],
    },
    // Los colores de la interfaz; la clave «pastel» es la de antes de que existieran más temas.
    TermTheme { key: "pastel", name: "Interfaz", own_surface: false, bg: 0, fg: 0, cursor: 0, selection: None, ansi: [0; 16] },
    TermTheme {
        key: "tango",
        name: "Tango",
        own_surface: true,
        bg: 0x2e3436,
        fg: 0xd3d7cf,
        cursor: 0xd3d7cf,
        selection: None,
        ansi: [
            0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
            0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
        ],
    },
    TermTheme {
        key: "solarized-dark",
        name: "Solarized Dark",
        own_surface: true,
        bg: 0x002b36,
        fg: 0x839496,
        cursor: 0x93a1a1,
        selection: Some(0x073642),
        // El «negro brillante» canónico (#002b36) es el fondo: aquí es #586e75 para que el texto atenuado se lea.
        ansi: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x586e75, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    },
    TermTheme {
        key: "solarized-light",
        name: "Solarized Light",
        own_surface: true,
        bg: 0xfdf6e3,
        fg: 0x657b83,
        cursor: 0x586e75,
        selection: Some(0xeee8d5),
        ansi: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    },
    TermTheme {
        key: "dracula",
        name: "Dracula",
        own_surface: true,
        bg: 0x282a36,
        fg: 0xf8f8f2,
        cursor: 0xf8f8f2,
        selection: Some(0x44475a),
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    },
    TermTheme {
        key: "nord",
        name: "Nord",
        own_surface: true,
        bg: 0x2e3440,
        fg: 0xd8dee9,
        cursor: 0xd8dee9,
        selection: Some(0x434c5e),
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    TermTheme {
        key: "one-dark",
        name: "One Dark",
        own_surface: true,
        bg: 0x282c34,
        fg: 0xabb2bf,
        cursor: 0x528bff,
        selection: Some(0x3e4451),
        ansi: [
            0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
        ],
    },
    TermTheme {
        key: "gruvbox-dark",
        name: "Gruvbox Dark",
        own_surface: true,
        bg: 0x282828,
        fg: 0xebdbb2,
        cursor: 0xebdbb2,
        selection: Some(0x504945),
        ansi: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    },
    TermTheme {
        key: "tokyo-night",
        name: "Tokyo Night",
        own_surface: true,
        bg: 0x1a1b26,
        fg: 0xc0caf5,
        cursor: 0xc0caf5,
        selection: Some(0x33467c),
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    TermTheme {
        key: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        own_surface: true,
        bg: 0x1e1e2e,
        fg: 0xcdd6f4,
        cursor: 0xf5e0dc,
        selection: Some(0x45475a),
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    },
    TermTheme {
        key: "claude",
        name: "Claude",
        own_surface: true,
        bg: 0x141413,
        fg: 0xe8e6dc,
        cursor: 0xd97757,
        selection: Some(0x3d3d3a),
        ansi: [
            0x3d3d3a, 0xd4654a, 0x788c5d, 0xe0b060, 0x6a9bcc, 0xb08aaf, 0x7fb2a8, 0xb0aea5,
            0x6b6a63, 0xe8825f, 0x93a876, 0xecc27a, 0x8bb4dc, 0xc9a3c8, 0x9fcbc0, 0xfaf9f5,
        ],
    },
];

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(index: u8) {
    CURRENT.store(index.min(THEMES.len() as u8 - 1), Ordering::Relaxed);
}

/// El tema activo, o `None` si es «Interfaz» (los colores salen de la interfaz).
pub fn custom() -> Option<&'static TermTheme> {
    let index = CURRENT.load(Ordering::Relaxed);
    (index != INTERFACE).then(|| &THEMES[index as usize])
}

pub fn index_of_key(key: &str) -> Option<u8> {
    THEMES.iter().position(|t| t.key == key).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_gnome_is_the_first() {
        for (i, a) in THEMES.iter().enumerate() {
            assert!(THEMES.iter().skip(i + 1).all(|b| b.key != a.key), "clave repetida: {}", a.key);
        }
        assert_eq!(THEMES[0].key, "gnome");
        assert_eq!(THEMES[INTERFACE as usize].key, "pastel", "la clave guardada de la paleta de la interfaz");
    }
}
