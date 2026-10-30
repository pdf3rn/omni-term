//! Ajustes del usuario, guardados en la tabla `meta` de la base de datos.

use crate::data::store::Store;

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

/// Paleta de los 16 colores ANSI que usan los programas del terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsiChoice {
    Catppuccin,
    Gnome,
    Tango,
}

impl AnsiChoice {
    pub const ALL: [(AnsiChoice, &'static str); 3] = [(AnsiChoice::Catppuccin, "Catppuccin"), (AnsiChoice::Gnome, "GNOME"), (AnsiChoice::Tango, "Tango")];

    pub fn index(self) -> u8 {
        self as u8
    }

    fn key(self) -> &'static str {
        match self {
            // La clave se queda como «pastel» para no perder lo ya guardado.
            AnsiChoice::Catppuccin => "pastel",
            AnsiChoice::Gnome => "gnome",
            AnsiChoice::Tango => "tango",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().map(|(choice, _)| *choice).find(|c| c.key() == key)
    }
}

/// En macOS la barra de título la dibuja la app, con los botones de la ventana del sistema encima, y el menú es el nativo.
pub const NATIVE_CHROME: bool = cfg!(target_os = "macos");

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Qué hacer con las pestañas de la sesión anterior al arrancar.
    pub on_start: StartupChoice,
    /// Perfil de terminal local que abren el botón + y Ctrl+Shift+T; sin él, el shell del sistema.
    pub local_profile: Option<i64>,
    pub accent: AccentChoice,
    pub ansi: AnsiChoice,
    /// Barra de menú siempre visible; si no, `Alt` la muestra un momento.
    pub menu_bar: bool,
    /// Usa la barra de título del sistema en lugar de la integrada (se aplica al reiniciar).
    pub system_titlebar: bool,
    /// Con la barra integrada, el menú va en la barra; si no, tras un botón de hamburguesa.
    pub titlebar_menu: bool,
    /// Barra integrada más baja.
    pub titlebar_compact: bool,
    /// Preguntar antes de cerrar pestañas (o la ventana) con algo en ejecución.
    pub confirm_close: bool,
    /// Preguntar antes de pegar en el terminal texto con saltos de línea.
    pub confirm_paste: bool,
    /// Filas de las listas más bajas.
    pub compact: bool,
    /// Segundos de actividad en una pestaña en segundo plano a partir de los cuales se avisa; 0 lo apaga.
    pub notify_after: u32,
    /// Familia instalada elegida para el terminal; sin ella, JetBrains Mono (incluida).
    pub font_family: Option<String>,
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub cursor: CursorChoice,
    pub blink: bool,
    /// La negrita aclara los colores ANSI 0-7.
    pub bold_bright: bool,
    /// Líneas de historial de las sesiones nuevas.
    pub scrollback: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartupChoice {
    Ask,
    Restore,
    Fresh,
}

impl StartupChoice {
    fn key(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Restore => "restore",
            Self::Fresh => "fresh",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Ask, Self::Restore, Self::Fresh].into_iter().find(|c| c.key() == key)
    }
}

pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 32.0;
pub const DEFAULT_FONT_SIZE: f32 = 15.0;
pub const MIN_SCROLLBACK: usize = 100;
pub const MAX_SCROLLBACK: usize = 200_000;

impl Default for Settings {
    fn default() -> Self {
        Self {
            on_start: StartupChoice::Ask,
            local_profile: None,
            accent: AccentChoice::Blue,
            ansi: AnsiChoice::Catppuccin,
            menu_bar: true,
            system_titlebar: false,
            titlebar_menu: true,
            titlebar_compact: false,
            confirm_close: true,
            confirm_paste: true,
            compact: false,
            notify_after: 15,
            font_family: None,
            theme: ThemeChoice::Dark,
            font_size: DEFAULT_FONT_SIZE,
            cursor: CursorChoice::Block,
            blink: false,
            bold_bright: false,
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
            local_profile: text(store, "setting.local_profile").and_then(|t| t.parse::<i64>().ok()),
            on_start: text(store, "setting.on_start").and_then(|k| StartupChoice::from_key(&k)).unwrap_or(defaults.on_start),
            accent: text(store, "setting.accent").and_then(|k| AccentChoice::from_key(&k)).unwrap_or(defaults.accent),
            ansi: text(store, "setting.ansi").and_then(|k| AnsiChoice::from_key(&k)).unwrap_or(defaults.ansi),
            menu_bar: text(store, "setting.menu_bar").map_or(defaults.menu_bar, |t| t == "1"),
            system_titlebar: !NATIVE_CHROME && text(store, "setting.system_titlebar").map_or(defaults.system_titlebar, |t| t == "1"),
            titlebar_menu: text(store, "setting.titlebar_menu").map_or(defaults.titlebar_menu, |t| t == "1"),
            titlebar_compact: text(store, "setting.titlebar_compact").map_or(defaults.titlebar_compact, |t| t == "1"),
            confirm_close: text(store, "setting.confirm_close").map_or(defaults.confirm_close, |t| t == "1"),
            confirm_paste: text(store, "setting.confirm_paste").map_or(defaults.confirm_paste, |t| t == "1"),
            compact: text(store, "setting.compact").map_or(defaults.compact, |t| t == "1"),
            notify_after: text(store, "setting.notify_after")
                .and_then(|t| t.parse::<u32>().ok())
                .map_or(defaults.notify_after, |n| n.min(3600)),
            font_family: text(store, "setting.font_family").filter(|f| !f.is_empty()),
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
            bold_bright: text(store, "setting.bold_bright").map_or(defaults.bold_bright, |t| t == "1"),
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
        store.set_meta("setting.local_profile", self.local_profile.map(|id| id.to_string()).unwrap_or_default().as_bytes())?;
        store.set_meta("setting.on_start", self.on_start.key().as_bytes())?;
        store.set_meta("setting.accent", self.accent.key().as_bytes())?;
        store.set_meta("setting.ansi", self.ansi.key().as_bytes())?;
        store.set_meta("setting.menu_bar", if self.menu_bar { b"1" } else { b"0" })?;
        store.set_meta("setting.system_titlebar", if self.system_titlebar { b"1" } else { b"0" })?;
        store.set_meta("setting.titlebar_menu", if self.titlebar_menu { b"1" } else { b"0" })?;
        store.set_meta("setting.titlebar_compact", if self.titlebar_compact { b"1" } else { b"0" })?;
        store.set_meta("setting.confirm_close", if self.confirm_close { b"1" } else { b"0" })?;
        store.set_meta("setting.confirm_paste", if self.confirm_paste { b"1" } else { b"0" })?;
        store.set_meta("setting.compact", if self.compact { b"1" } else { b"0" })?;
        store.set_meta("setting.notify_after", self.notify_after.to_string().as_bytes())?;
        store.set_meta("setting.font_family", self.font_family.as_deref().unwrap_or("").as_bytes())?;
        store.set_meta("setting.theme", theme.as_bytes())?;
        store.set_meta("font_size", self.font_size.to_string().as_bytes())?;
        store.set_meta("setting.cursor", cursor.as_bytes())?;
        store.set_meta("setting.blink", if self.blink { b"1" } else { b"0" })?;
        store.set_meta("setting.bold_bright", if self.bold_bright { b"1" } else { b"0" })?;
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
            on_start: StartupChoice::Restore,
            local_profile: Some(7),
            accent: AccentChoice::Mauve,
            ansi: AnsiChoice::Gnome,
            menu_bar: false,
            system_titlebar: !NATIVE_CHROME,
            titlebar_menu: false,
            titlebar_compact: true,
            confirm_close: false,
            confirm_paste: false,
            compact: true,
            notify_after: 45,
            font_family: Some("Fira Code".into()),
            theme: ThemeChoice::Light,
            font_size: 18.0,
            cursor: CursorChoice::Beam,
            blink: true,
            bold_bright: true,
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
