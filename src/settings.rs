//! Ajustes del usuario, guardados en la tabla `meta` de la base de datos.

use crate::store::Store;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorChoice {
    Block,
    Beam,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentChoice {
    Blue,
    Gray,
    Green,
    Mauve,
}

impl AccentChoice {
    pub const ALL: [(AccentChoice, &'static str); 4] = [
        (AccentChoice::Blue, "Azul"),
        (AccentChoice::Gray, "Gris"),
        (AccentChoice::Green, "Verde"),
        (AccentChoice::Mauve, "Malva"),
    ];

    pub fn index(self) -> u8 {
        self as u8
    }

    fn key(self) -> &'static str {
        match self {
            AccentChoice::Blue => "blue",
            AccentChoice::Gray => "gray",
            AccentChoice::Green => "green",
            AccentChoice::Mauve => "mauve",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().map(|(choice, _)| *choice).find(|c| c.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub accent: AccentChoice,
    /// Barra de menú siempre visible; si no, `Alt` la muestra un momento.
    pub menu_bar: bool,
    /// Preguntar antes de cerrar pestañas (o la ventana) con algo en ejecución.
    pub confirm_close: bool,
    /// Filas de las listas más bajas.
    pub compact: bool,
    /// Segundos de actividad en una pestaña en segundo plano a partir de los cuales se avisa; 0 lo apaga.
    pub notify_after: u32,
    /// Fuente del terminal elegida por el usuario (archivo TTF/OTF); sin ella, JetBrains Mono.
    pub font_path: Option<String>,
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub cursor: CursorChoice,
    pub blink: bool,
    /// Líneas de historial de las sesiones nuevas.
    pub scrollback: usize,
}

pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 32.0;
pub const DEFAULT_FONT_SIZE: f32 = 15.0;
pub const MIN_SCROLLBACK: usize = 100;
pub const MAX_SCROLLBACK: usize = 200_000;

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: AccentChoice::Blue,
            menu_bar: true,
            confirm_close: true,
            compact: false,
            notify_after: 15,
            font_path: None,
            theme: ThemeChoice::Dark,
            font_size: DEFAULT_FONT_SIZE,
            cursor: CursorChoice::Block,
            blink: false,
            scrollback: 10_000,
        }
    }
}

fn text(store: &Store, key: &str) -> Option<String> {
    store.meta(key).ok().flatten().and_then(|bytes| String::from_utf8(bytes).ok())
}

impl Settings {
    pub fn load(store: &Store) -> Self {
        let defaults = Self::default();
        Self {
            accent: text(store, "setting.accent").and_then(|k| AccentChoice::from_key(&k)).unwrap_or(defaults.accent),
            menu_bar: text(store, "setting.menu_bar").map_or(defaults.menu_bar, |t| t == "1"),
            confirm_close: text(store, "setting.confirm_close").map_or(defaults.confirm_close, |t| t == "1"),
            compact: text(store, "setting.compact").map_or(defaults.compact, |t| t == "1"),
            notify_after: text(store, "setting.notify_after")
                .and_then(|t| t.parse::<u32>().ok())
                .map_or(defaults.notify_after, |n| n.min(3600)),
            font_path: text(store, "setting.font_path").filter(|p| !p.is_empty()),
            theme: match text(store, "setting.theme").as_deref() {
                Some("system") => ThemeChoice::System,
                Some("light") => ThemeChoice::Light,
                Some("dark") => ThemeChoice::Dark,
                _ => defaults.theme,
            },
            font_size: text(store, "font_size")
                .and_then(|t| t.parse::<f32>().ok())
                .filter(|size| size.is_finite())
                .map_or(defaults.font_size, |size| size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)),
            cursor: match text(store, "setting.cursor").as_deref() {
                Some("beam") => CursorChoice::Beam,
                Some("underline") => CursorChoice::Underline,
                Some("block") => CursorChoice::Block,
                _ => defaults.cursor,
            },
            blink: text(store, "setting.blink").map_or(defaults.blink, |t| t == "1"),
            scrollback: text(store, "setting.scrollback")
                .and_then(|t| t.parse::<usize>().ok())
                .map_or(defaults.scrollback, |n| n.clamp(MIN_SCROLLBACK, MAX_SCROLLBACK)),
        }
    }

    pub fn save(&self, store: &Store) -> anyhow::Result<()> {
        let theme = match self.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        };
        let cursor = match self.cursor {
            CursorChoice::Block => "block",
            CursorChoice::Beam => "beam",
            CursorChoice::Underline => "underline",
        };
        store.set_meta("setting.accent", self.accent.key().as_bytes())?;
        store.set_meta("setting.menu_bar", if self.menu_bar { b"1" } else { b"0" })?;
        store.set_meta("setting.confirm_close", if self.confirm_close { b"1" } else { b"0" })?;
        store.set_meta("setting.compact", if self.compact { b"1" } else { b"0" })?;
        store.set_meta("setting.notify_after", self.notify_after.to_string().as_bytes())?;
        store.set_meta("setting.font_path", self.font_path.as_deref().unwrap_or("").as_bytes())?;
        store.set_meta("setting.theme", theme.as_bytes())?;
        store.set_meta("font_size", self.font_size.to_string().as_bytes())?;
        store.set_meta("setting.cursor", cursor.as_bytes())?;
        store.set_meta("setting.blink", if self.blink { b"1" } else { b"0" })?;
        store.set_meta("setting.scrollback", self.scrollback.to_string().as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_database_gives_the_defaults() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(Settings::load(&store), Settings::default());
    }

    #[test]
    fn settings_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let settings = Settings {
            accent: AccentChoice::Mauve,
            menu_bar: false,
            confirm_close: false,
            compact: true,
            notify_after: 45,
            font_path: Some("/usr/share/fonts/mono.ttf".into()),
            theme: ThemeChoice::Light,
            font_size: 18.0,
            cursor: CursorChoice::Beam,
            blink: true,
            scrollback: 50_000,
        };
        settings.save(&store).unwrap();
        assert_eq!(Settings::load(&store), settings);
    }

    #[test]
    fn invalid_stored_values_are_clamped_or_ignored() {
        let store = Store::open_in_memory().unwrap();
        store.set_meta("font_size", b"500").unwrap();
        store.set_meta("setting.scrollback", b"3").unwrap();
        store.set_meta("setting.theme", b"neon").unwrap();
        store.set_meta("setting.cursor", b"???").unwrap();
        let loaded = Settings::load(&store);
        assert_eq!(loaded.font_size, MAX_FONT_SIZE);
        assert_eq!(loaded.scrollback, MIN_SCROLLBACK);
        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert_eq!(loaded.cursor, CursorChoice::Block);
        store.set_meta("font_size", b"NaN").unwrap();
        assert_eq!(Settings::load(&store).font_size, DEFAULT_FONT_SIZE);
    }
}
