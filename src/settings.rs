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

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
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
