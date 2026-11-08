//! Menú nativo de macOS: la barra superior del sistema, con los mismos comandos que la barra integrada.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::data::store::Connection;
use crate::i18n::{t, Lang};
use crate::ui::palette::Command;

/// Lo que hace falta para que una entrada esté activa.
#[derive(Clone, Copy)]
enum Need {
    Always,
    Tabs,
    Terminal,
    Selection,
    Several,
    Remote,
}

/// Estado de la ventana que decide qué entradas están activas.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Enabled {
    pub tabs: bool,
    pub terminal: bool,
    pub selection: bool,
    pub several: bool,
    pub connection: bool,
}

impl Enabled {
    fn allows(self, need: Need) -> bool {
        match need {
            Need::Always => true,
            Need::Tabs => self.tabs,
            Need::Terminal => self.terminal,
            Need::Selection => self.selection,
            Need::Several => self.several,
            Need::Remote => self.connection,
        }
    }
}

/// Una fila de un menú: una entrada con su comando o un separador.
enum Row {
    Item(&'static str, Command, Need),
    Separator,
    Recents,
}

use Need::*;
use Row::{Item, Separator};

const APP: &[Row] = &[
    Item("dialogs-acerca-de-omni-term", Command::About, Always),
    Item("native-menu-ajustes", Command::Settings, Always),
    Separator,
    Item("native-menu-salir", Command::Quit, Always),
];

const FILE: &[Row] = &[
    Item("native-menu-nueva-pestana-local", Command::NewTab, Always),
    Item("native-menu-nueva-conexion", Command::NewConnection, Always),
    Item("menu-importar-ssh-config", Command::ImportConfig, Always),
    Separator,
    Item("native-menu-cerrar-pestana", Command::CloseTab, Tabs),
    Separator,
    Item("settings-hosts-conocidos", Command::KnownHosts, Always),
];

const EDIT: &[Row] = &[
    Item("pane-copiar", Command::Copy, Selection),
    Item("pane-pegar", Command::Paste, Terminal),
    Item("pane-escribir-contrasena", Command::TypePassword, Terminal),
    Item("pane-seleccionar-todo", Command::SelectAll, Terminal),
    Separator,
    Item("settings-buscar-en-la-salida", Command::Search, Terminal),
    Item("pane-guardar-seleccion-como-snippet", Command::SaveSnippet, Selection),
    Separator,
    Item("native-menu-contrasenas-guardadas", Command::Passwords, Always),
];

const VIEW: &[Row] = &[
    Item("native-menu-barra-lateral", Command::ToggleSidebar, Always),
    Item("settings-pantalla-completa", Command::Fullscreen, Always),
    Separator,
    Item("pane-dividir-a-la-derecha", Command::SplitRight, Terminal),
    Item("pane-dividir-hacia-abajo", Command::SplitDown, Terminal),
    Item("settings-ampliar-o-restaurar-el-panel", Command::TogglePaneZoom, Terminal),
    Item("native-menu-igualar-paneles", Command::EqualizePanes, Terminal),
    Separator,
    Item("native-menu-aumentar-la-letra", Command::ZoomIn, Always),
    Item("native-menu-reducir-la-letra", Command::ZoomOut, Always),
    Item("native-menu-restablecer-la-letra", Command::ZoomReset, Always),
    Separator,
    Item("native-menu-tema-oscuro", Command::ThemeDark, Always),
    Item("native-menu-tema-claro", Command::ThemeLight, Always),
    Item("native-menu-tema-del-sistema", Command::ThemeSystem, Always),
];

const GO: &[Row] = &[
    Item("settings-paleta-de-comandos", Command::Palette, Always),
    Separator,
    Item("native-menu-pestana-siguiente", Command::NextTab, Several),
    Item("native-menu-pestana-anterior", Command::PrevTab, Several),
    Item("native-menu-explorador-de-archivos-de-esta", Command::Files, Remote),
    Item("conn-edit-contenedores", Command::Containers, Always),
    Separator,
    Row::Recents,
];

const HELP: &[Row] = &[Item("welcome-menu", Command::Welcome, Always), Item("settings-atajos-de-teclado", Command::Shortcuts, Always)];

/// Menú nativo de la aplicación. Vive en el hilo principal, junto a la ventana.
pub struct NativeMenu {
    _menu: Menu,
    entries: Vec<(MenuItem, Command, Need)>,
    recents: Submenu,
    recent_items: HashMap<MenuId, Connection>,
    recent_list: Vec<Connection>,
    enabled: Enabled,
    events: Receiver<MenuId>,
    lang: Lang,
}

impl NativeMenu {
    pub fn new(ctx: egui::Context) -> muda::Result<Self> {
        let (tx, events) = mpsc::channel();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = tx.send(event.id);
            ctx.request_repaint();
        }));

        let mut native = Self {
            _menu: Menu::new(),
            entries: Vec::new(),
            recents: Submenu::new("", false),
            recent_items: HashMap::new(),
            recent_list: Vec::new(),
            enabled: Enabled::default(),
            events,
            lang: crate::i18n::current(),
        };
        native.build()?;
        Ok(native)
    }

    /// Crea el menú con los textos del idioma actual; se repite cuando el idioma cambia.
    fn build(&mut self) -> muda::Result<()> {
        let menu = Menu::new();
        self.entries.clear();
        self.recents = Submenu::new(t("native-menu-conexiones-recientes"), false);
        self.recent_items.clear();
        self.recent_list.clear();
        self.lang = crate::i18n::current();
        for (title, rows) in [("omni-term", APP), (t("native-menu-archivo"), FILE), (t("settings-editar"), EDIT), (t("native-menu-ver"), VIEW), (t("native-menu-ir"), GO), (t("native-menu-ayuda"), HELP)] {
            let submenu = Submenu::new(title, true);
            self.fill(&submenu, rows)?;
            menu.append(&submenu)?;
        }
        menu.init_for_nsapp();
        self._menu = menu;
        Ok(())
    }

    fn fill(&mut self, submenu: &Submenu, rows: &[Row]) -> muda::Result<()> {
        for row in rows {
            match row {
                Item(label, command, need) => {
                    let item = MenuItem::new(t(label), true, None);
                    submenu.append(&item)?;
                    self.entries.push((item, *command, *need));
                }
                Separator => submenu.append(&PredefinedMenuItem::separator())?,
                Row::Recents => submenu.append(&self.recents)?,
            }
        }
        Ok(())
    }

    /// Activa o desactiva las entradas y rehace las conexiones recientes cuando cambian.
    pub fn sync(&mut self, enabled: Enabled, recents: &[Connection]) -> muda::Result<()> {
        // Con otro idioma el menú se rehace entero, así que hay que volver a aplicar el estado.
        let rebuilt = self.lang != crate::i18n::current();
        if rebuilt {
            self.build()?;
        }
        if rebuilt || enabled != self.enabled {
            for (item, _, need) in &self.entries {
                item.set_enabled(enabled.allows(*need));
            }
            self.enabled = enabled;
        }
        if rebuilt || recents != self.recent_list.as_slice() {
            while self.recents.remove_at(0).is_some() {}
            self.recent_items.clear();
            for connection in recents {
                let item = MenuItem::new(format!("{}   {}", connection.name, connection.target()), true, None);
                self.recents.append(&item)?;
                self.recent_items.insert(item.id().clone(), connection.clone());
            }
            self.recents.set_enabled(!recents.is_empty());
            self.recent_list = recents.to_vec();
        }
        Ok(())
    }

    /// Comando o conexión elegidos desde el menú nativo desde la última vez.
    pub fn poll(&mut self) -> (Option<Command>, Option<Connection>) {
        let (mut command, mut connection) = (None, None);
        while let Ok(id) = self.events.try_recv() {
            if let Some(recent) = self.recent_items.get(&id) {
                connection = Some(recent.clone());
            } else if let Some((_, chosen, _)) = self.entries.iter().find(|(item, _, _)| item.id() == &id) {
                command = Some(*chosen);
            }
        }
        (command, connection)
    }
}
