//! Comandos de la app y paleta de comandos.

use crate::i18n::t;
use super::*;

impl App {
    pub(super) fn palette_items(&self) -> Vec<palette::Item> {
        let mut items = Vec::new();
        for connection in &self.connections {
            items.push(palette::Item {
                title: connection.name.clone(),
                detail: connection.target(),
                kind: t("commands-conexion"),
                target: Target::Connect(connection.clone()),
            });
        }
        for snippet in &self.snippets {
            let first = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            items.push(palette::Item {
                title: snippet.title.clone(),
                detail: first.chars().take(60).collect(),
                kind: t("commands-snippet"),
                target: Target::Snippet(snippet.clone()),
            });
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            items.push(palette::Item { title: tab.title(), detail: String::new(), kind: t("commands-pestana"), target: Target::Tab(index) });
        }
        for (command, label) in palette::COMMANDS {
            items.push(palette::Item { title: t(label).to_string(), detail: String::new(), kind: t("commands-accion"), target: Target::Command(command) });
        }
        items
    }

    pub(super) fn run_palette(&mut self, target: Target) {
        match target {
            Target::Connect(connection) => self.begin_open(connection, None, Purpose::Terminal),
            Target::Snippet(snippet) => self.insert_snippet(&snippet, false),
            Target::Tab(index) => self.active = index.min(self.tabs.len().saturating_sub(1)),
            Target::Command(command) => self.run_command(command),
            // Solo aparecen en el selector de contraseñas, que las atiende aparte.
            Target::Password(_) => {}
        }
    }

    pub(super) fn run_command(&mut self, command: Command) {
        match command {
            Command::NewTab => self.open_queue.push_back(SavedTab::plain(TabKind::Local)),
            Command::NewConnection => self.dialog = Some(Dialog::Edit(ConnForm::new())),
            Command::ImportConfig => self.import_ssh_config(),
            Command::Settings => self.open_settings(dialogs::SettingsTab::General),
            Command::KnownHosts => self.open_settings(dialogs::SettingsTab::Hosts),
            Command::Containers => self.open_containers(),
            Command::TypePassword => self.open_password_picker(),
            Command::Passwords => self.open_settings(dialogs::SettingsTab::Passwords),
            Command::Shortcuts => self.open_settings(dialogs::SettingsTab::Atajos),
            Command::About => self.dialog = Some(Dialog::About),
            Command::Welcome => self.open_welcome(),
            Command::CheckUpdates => self.check_updates_now(),
            Command::Palette => self.palette = Some(Palette::default()),
            Command::Fullscreen => {
                let on = self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                self.ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!on));
            }
            Command::SplitRight => self.split_active(Dir::Horizontal),
            Command::SplitDown => self.split_active(Dir::Vertical),
            Command::TogglePaneZoom => self.toggle_pane_zoom(),
            Command::EqualizePanes => {
                if let Some(Body::Terminal { panes, .. }) = self.tabs.get_mut(self.active).map(|t| &mut t.body) {
                    panes.equalize();
                }
            }
            Command::CloseTab => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.id) {
                    self.request_close(CloseAction::Tab(id));
                }
            }
            Command::Quit => self.request_close(CloseAction::Window),
            Command::ToggleSidebar => self.set_sidebar_collapsed(!self.sidebar_collapsed),
            Command::ThemeDark | Command::ThemeLight | Command::ThemeSystem => {
                self.settings.theme = match command {
                    Command::ThemeDark => ThemeChoice::Dark,
                    Command::ThemeLight => ThemeChoice::Light,
                    _ => ThemeChoice::System,
                };
                let _ = self.settings.save(&self.store.lock().unwrap());
            }
            Command::Copy => {
                if let Some(pane) = self.tabs.get(self.active).and_then(|t| t.pane()) {
                    pane.copy_selection_to(&self.ctx);
                }
            }
            Command::Paste => {
                if let Some(pane) = self.tabs.get_mut(self.active).and_then(|t| t.pane_mut()) {
                    pane.paste_clipboard();
                }
            }
            Command::SelectAll => {
                if let Some(pane) = self.tabs.get(self.active).and_then(|t| t.pane()) {
                    pane.select_all();
                }
            }
            Command::Search => {
                if let Some(pane) = self.tabs.get_mut(self.active).and_then(|t| t.pane_mut()) {
                    pane.toggle_search();
                }
            }
            Command::SaveSnippet => {
                let selected = self.tabs.get(self.active).and_then(|t| t.pane()).and_then(|p| p.selection_text());
                match selected {
                    Some(text) => self.dialog = Some(Dialog::Snippet(SnippetForm::new(text))),
                    None => self.notify(t("commands-selecciona-texto-en-el-terminal")),
                }
            }
            Command::ZoomIn => self.set_font_size(self.settings.font_size + 1.0),
            Command::ZoomOut => self.set_font_size(self.settings.font_size - 1.0),
            Command::ZoomReset => self.set_font_size(DEFAULT_FONT_SIZE),
            Command::NextTab | Command::PrevTab => {
                if !self.tabs.is_empty() {
                    let step = if command == Command::NextTab { 1 } else { -1 };
                    self.active = (self.active as i32 + step).rem_euclid(self.tabs.len() as i32) as usize;
                }
            }
            Command::Files => {
                if let Some(TabKind::Connection(id)) = self.tabs.get(self.active).map(|t| t.kind) {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.begin_open(connection, None, Purpose::Explorer(None, None));
                    }
                }
            }
        }
    }

    pub(super) fn global_shortcuts(&mut self, ctx: &egui::Context) {
        // Con Ctrl solo (B, K) chocarían con tmux y readline; por eso Ctrl+Shift (Cmd en macOS).
        let app = shortcuts::app();
        let (toggle_sidebar, search_sidebar, new_connection, close) =
            ctx.input_mut(|i| (i.consume_key(app, Key::B), i.consume_key(app, Key::K), i.consume_key(app, Key::N), i.consume_key(app, Key::W)));
        if new_connection && self.dialog.is_none() {
            self.run_command(Command::NewConnection);
        }
        // Aquí y no en el panel: cierra también pestañas que no son de terminal (Ajustes, explorador) y
        // funciona con la búsqueda abierta o con el foco en otro campo.
        if close && self.dialog.is_none() && self.palette.is_none() {
            self.request_close_focused_pane();
        }
        if toggle_sidebar {
            self.set_sidebar_collapsed(!self.sidebar_collapsed);
        }
        if ctx.input_mut(|i| i.consume_key(app, Key::P)) && self.dialog.is_none() {
            self.palette = if self.palette.is_some() { None } else { Some(Palette::default()) };
        }
        // Gestor de contraseñas: escribir una guardada en el terminal (por ejemplo, cuando sudo la pide).
        // Con distribuciones de teclado donde Ctrl+Alt equivale a AltGr, la P también puede llegar como texto:
        // se descarta para que no se escriba en el terminal.
        let password_key = ctx.input_mut(|i| {
            let pressed = i.consume_key(shortcuts::password(), Key::P);
            if pressed {
                i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
            }
            pressed
        });
        if password_key && self.dialog.is_none() {
            if matches!(self.palette.as_ref().map(|p| p.mode), Some(palette::Mode::Passwords { .. })) {
                self.palette = None;
            } else {
                self.open_password_picker();
            }
        }
        if ctx.input_mut(|i| i.consume_key(shortcuts::primary(), Key::Comma)) {
            self.open_settings(dialogs::SettingsTab::General);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F11)) {
            self.run_command(Command::Fullscreen);
        }
        if ctx.input_mut(|i| i.consume_key(app, Key::T)) && self.dialog.is_none() {
            self.open_plus_menu = true;
        }
        // Alt+1…9 (Cmd en macOS) salta a esa pestaña (la 9, a la última); Ctrl+Tab y Ctrl+Shift+Tab recorren.
        let (jump, step) = ctx.input_mut(|i| {
            let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
            let jump = digits.iter().position(|k| i.consume_key(shortcuts::tab_digit(), *k));
            if jump.is_some() {
                // El dígito también llega como texto: no debe escribirse en el terminal.
                i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
            }
            let step = if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
                -1
            } else if i.consume_key(Modifiers::CTRL, Key::Tab) {
                1
            } else {
                0
            };
            (jump, step)
        });
        if !self.tabs.is_empty() {
            let last = self.tabs.len() - 1;
            if let Some(n) = jump {
                self.active = if n == 8 { last } else { n.min(last) };
            }
            if step != 0 {
                self.active = (self.active as i32 + step).rem_euclid(self.tabs.len() as i32) as usize;
            }
        }
        if search_sidebar {
            self.set_sidebar_collapsed(false);
            self.focus_search = true;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.close_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_close(CloseAction::Window);
        }
    }
}
