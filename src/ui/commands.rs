//! Comandos de la app y paleta de comandos.

use super::*;

impl App {
    pub(super) fn palette_items(&self) -> Vec<palette::Item> {
        let mut items = Vec::new();
        for connection in &self.connections {
            items.push(palette::Item {
                title: connection.name.clone(),
                detail: connection.target(),
                kind: "Conexión",
                target: Target::Connect(connection.clone()),
            });
        }
        for snippet in &self.snippets {
            let first = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            items.push(palette::Item {
                title: snippet.title.clone(),
                detail: first.chars().take(60).collect(),
                kind: "Snippet",
                target: Target::Snippet(snippet.clone()),
            });
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            items.push(palette::Item { title: tab.title(), detail: String::new(), kind: "Pestaña", target: Target::Tab(index) });
        }
        for (command, label) in palette::COMMANDS {
            items.push(palette::Item { title: label.to_string(), detail: String::new(), kind: "Acción", target: Target::Command(command) });
        }
        items
    }

    pub(super) fn run_palette(&mut self, target: Target) {
        match target {
            Target::Connect(connection) => self.begin_open(connection, None, Purpose::Terminal),
            Target::Snippet(snippet) => self.insert_snippet(&snippet, false),
            Target::Tab(index) => self.active = index.min(self.tabs.len().saturating_sub(1)),
            Target::Command(command) => self.run_command(command),
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
            Command::Shortcuts => self.open_settings(dialogs::SettingsTab::Atajos),
            Command::About => self.dialog = Some(Dialog::About),
            Command::Palette => self.palette = Some(Palette::default()),
            Command::Fullscreen => {
                let on = self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                self.ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!on));
            }
            Command::SplitRight => self.split_active(Dir::Horizontal),
            Command::SplitDown => self.split_active(Dir::Vertical),
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
                if let Some(pane) = self.tabs.get(self.active).and_then(|t| t.pane()) {
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
                    None => self.notify("Selecciona texto en el terminal para guardarlo como snippet"),
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
}
