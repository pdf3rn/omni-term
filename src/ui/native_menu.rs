//! Menú nativo de macOS: la barra superior del sistema, con los mismos comandos que la barra integrada.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::data::store::Connection;
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
    Item("Acerca de omni-term", Command::About, Always),
    Item("Ajustes…", Command::Settings, Always),
    Separator,
    Item("Salir", Command::Quit, Always),
];

const FILE: &[Row] = &[
    Item("Nueva pestaña local", Command::NewTab, Always),
    Item("Nueva conexión…", Command::NewConnection, Always),
    Item("Importar de ~/.ssh/config", Command::ImportConfig, Always),
    Separator,
    Item("Cerrar pestaña", Command::CloseTab, Tabs),
    Item("Hosts conocidos", Command::KnownHosts, Always),
];

const EDIT: &[Row] = &[
    Item("Copiar", Command::Copy, Selection),
    Item("Pegar", Command::Paste, Terminal),
    Item("Seleccionar todo", Command::SelectAll, Terminal),
    Separator,
    Item("Buscar en la salida", Command::Search, Terminal),
    Item("Guardar selección como snippet", Command::SaveSnippet, Selection),
];

const VIEW: &[Row] = &[
    Item("Barra lateral", Command::ToggleSidebar, Always),
    Item("Pantalla completa", Command::Fullscreen, Always),
    Separator,
    Item("Dividir a la derecha", Command::SplitRight, Terminal),
    Item("Dividir hacia abajo", Command::SplitDown, Terminal),
    Item("Ampliar o restaurar el panel", Command::TogglePaneZoom, Terminal),
    Item("Igualar paneles", Command::EqualizePanes, Terminal),
    Separator,
    Item("Aumentar la letra", Command::ZoomIn, Always),
    Item("Reducir la letra", Command::ZoomOut, Always),
    Item("Restablecer la letra", Command::ZoomReset, Always),
    Separator,
    Item("Tema oscuro", Command::ThemeDark, Always),
    Item("Tema claro", Command::ThemeLight, Always),
    Item("Tema del sistema", Command::ThemeSystem, Always),
];

const GO: &[Row] = &[
    Item("Paleta de comandos", Command::Palette, Always),
    Separator,
    Item("Pestaña siguiente", Command::NextTab, Several),
    Item("Pestaña anterior", Command::PrevTab, Several),
    Item("Explorador de archivos de esta conexión", Command::Files, Remote),
    Item("Contenedores", Command::Containers, Always),
    Separator,
    Row::Recents,
];

const HELP: &[Row] = &[Item("Atajos de teclado", Command::Shortcuts, Always)];

/// Menú nativo de la aplicación. Vive en el hilo principal, junto a la ventana.
pub struct NativeMenu {
    _menu: Menu,
    entries: Vec<(MenuItem, Command, Need)>,
    recents: Submenu,
    recent_items: HashMap<MenuId, Connection>,
    recent_list: Vec<Connection>,
    enabled: Enabled,
    events: Receiver<MenuId>,
}

impl NativeMenu {
    pub fn new(ctx: egui::Context) -> muda::Result<Self> {
        let (tx, events) = mpsc::channel();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = tx.send(event.id);
            ctx.request_repaint();
        }));

        let menu = Menu::new();
        let recents = Submenu::new("Conexiones recientes", false);
        let mut native = Self {
            _menu: menu.clone(),
            entries: Vec::new(),
            recents: recents.clone(),
            recent_items: HashMap::new(),
            recent_list: Vec::new(),
            enabled: Enabled::default(),
            events,
        };
        for (title, rows) in [("omni-term", APP), ("Archivo", FILE), ("Editar", EDIT), ("Ver", VIEW), ("Ir", GO), ("Ayuda", HELP)] {
            let submenu = Submenu::new(title, true);
            native.fill(&submenu, rows)?;
            menu.append(&submenu)?;
        }
        menu.init_for_nsapp();
        Ok(native)
    }

    fn fill(&mut self, submenu: &Submenu, rows: &[Row]) -> muda::Result<()> {
        for row in rows {
            match row {
                Item(label, command, need) => {
                    let item = MenuItem::new(label, true, None);
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
        if enabled != self.enabled {
            for (item, _, need) in &self.entries {
                item.set_enabled(enabled.allows(*need));
            }
            self.enabled = enabled;
        }
        if recents != self.recent_list.as_slice() {
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
