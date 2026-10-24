//! Conexiones y snippets guardados: borrar, fijar, mover de grupo, importar y olvidar hosts.

use super::*;

impl App {
    pub(super) fn refresh_connections(&mut self) {
        let store = self.store.lock().unwrap();
        self.connections = store.list_connections().unwrap_or_default();
        self.forwards = store.all_forwards().unwrap_or_default();
        self.snippets = store.list_snippets().unwrap_or_default();
        drop(store);
        self.ssh_config_hosts = pending_ssh_config_hosts(&self.connections);
    }

    /// Inserta el snippet; si tiene variables, primero pide sus valores.
    pub(super) fn insert_snippet(&mut self, snippet: &Snippet, run: bool) {
        if crate::terminal::snippet_vars::variables(&snippet.body).is_empty() {
            self.insert_into_active(&snippet.body, run);
        } else {
            self.dialog = Some(Dialog::SnippetVars(VarsPrompt::new(snippet, run)));
        }
    }

    pub(super) fn insert_into_active(&mut self, body: &str, run: bool) {
        match self.tabs.get(self.active) {
            Some(tab) if tab.pane().is_some_and(|p| !p.session.is_disconnected()) => {
                if let Some(pane) = tab.pane() {
                    pane.insert_text(body, run);
                }
            }
            _ => self.notify("No hay un terminal activo donde insertar"),
        }
    }

    /// Color de identidad de la conexión a la que pertenece una pestaña, si lo tiene.
    pub(super) fn identity_for(&self, kind: TabKind) -> Option<Color32> {
        match kind {
            TabKind::Connection(id) | TabKind::Explorer(id) => self
                .connections
                .iter()
                .find(|c| c.id == Some(id))
                .and_then(|c| c.color)
                .map(theme::identity),
            TabKind::Local | TabKind::Containers | TabKind::Container | TabKind::Settings => None,
        }
    }

    pub(super) fn environment_for(&self, kind: TabKind) -> Option<String> {
        match kind {
            TabKind::Connection(id) | TabKind::Explorer(id) => {
                self.connections.iter().find(|c| c.id == Some(id)).and_then(|c| c.env.clone())
            }
            TabKind::Local | TabKind::Containers | TabKind::Container | TabKind::Settings => None,
        }
    }

    pub(super) fn delete_connection(&mut self, connection: Connection) {
        let Some(id) = connection.id else { return };
        let forwards = self.forwards.get(&id).cloned().unwrap_or_default();
        let dependents: Vec<i64> = self.connections.iter().filter(|c| c.jump == Some(id)).filter_map(|c| c.id).collect();
        if let Err(error) = self.store.lock().unwrap().delete_connection(id) {
            self.dialog = Some(Dialog::failure("No se pudo eliminar la conexión", format!("{error:#}")));
            return;
        }
        self.refresh_connections();
        let text = format!("«{}» eliminada", connection.name);
        self.push_toast(text, Some(Undo::Connection { connection, forwards, dependents }), Some(Cleanup::Secrets(id)));
    }

    pub(super) fn toggle_pin(&mut self, id: i64) {
        if let Some(position) = self.pinned.iter().position(|p| *p == id) {
            self.pinned.remove(position);
        } else {
            self.pinned.push(id);
        }
        let text = self.pinned.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let _ = self.store.lock().unwrap().set_meta("pinned_connections", text.as_bytes());
    }

    /// Cambia el entorno (y el color) de una conexión al soltarla sobre otro grupo, con deshacer.
    pub(super) fn move_to_group(&mut self, id: i64, label: &str) {
        if label == PINNED_LABEL {
            if !self.pinned.contains(&id) {
                self.toggle_pin(id);
            }
            return;
        }
        let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() else { return };
        let members: Vec<&Connection> = self
            .connections
            .iter()
            .filter(|c| c.id != Some(id) && c.env.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(label)))
            .collect();
        let target = group_environment(label, &members);
        let (env, color) = (target.as_ref().map(|(e, _)| e.clone()), target.as_ref().and_then(|(_, c)| *c));
        let was_pinned = self.pinned.contains(&id);
        if was_pinned {
            self.toggle_pin(id);
        }
        if connection.env == env && connection.color == color {
            return;
        }
        let mut moved = connection.clone();
        moved.env = env;
        moved.color = color;
        if let Err(error) = self.store.lock().unwrap().save_connection(&moved) {
            self.dialog = Some(Dialog::failure("No se pudo mover la conexión", format!("{error:#}")));
            return;
        }
        self.refresh_connections();
        let text = format!("«{}» movida a {label}", connection.name);
        self.push_toast(text, Some(Undo::Environment { id, env: connection.env, color: connection.color }), None);
    }

    /// Borra líneas de `known_hosts` con aviso y deshacer; con `reconnect`, vuelve a conectar al servidor.
    pub(super) fn forget_known_hosts(&mut self, entries: Vec<known_hosts::KnownHost>, reconnect: Option<(String, u16)>) {
        let path = ssh::default_known_hosts();
        let mut removed: Vec<(usize, String)> = Vec::new();
        // De la última línea a la primera, para que las posiciones guardadas sigan valiendo.
        let mut ordered = entries;
        ordered.sort_by_key(|e| std::cmp::Reverse(e.index));
        for entry in &ordered {
            match known_hosts::remove(&path, entry) {
                Ok(index) => removed.push((index, entry.raw.clone())),
                Err(error) => {
                    self.dialog = Some(Dialog::failure("No se pudo olvidar el host", format!("{error:#}")));
                    break;
                }
            }
        }
        self.known_hosts.loaded = false;
        if removed.is_empty() {
            return;
        }
        removed.reverse();
        let first = ordered.first().map_or_else(String::new, |e| e.display());
        let text = if removed.len() == 1 { format!("Huella de «{first}» olvidada") } else { format!("{} huellas olvidadas", removed.len()) };
        self.push_toast(text, Some(Undo::KnownHosts(removed)), None);
        if let Some((host, port)) = reconnect {
            self.reconnect_to(&host, port);
        }
    }

    /// Vuelve a conectar a `host:port`: la pestaña caída de esa conexión si la hay; si no, una nueva.
    pub(super) fn reconnect_to(&mut self, host: &str, port: u16) {
        let Some(connection) = self.connections.iter().find(|c| c.host.eq_ignore_ascii_case(host) && c.port == port).cloned() else { return };
        let dropped = self
            .tabs
            .iter()
            .position(|t| t.kind == TabKind::Connection(connection.id.unwrap_or_default()) && t.pane().is_some_and(|p| p.session.is_disconnected()));
        match dropped {
            Some(index) => self.reconnect(index),
            None => self.begin_open(connection, None, Purpose::Terminal),
        }
    }

    pub(super) fn delete_snippet(&mut self, snippet: Snippet) {
        let Some(id) = snippet.id else { return };
        if let Err(error) = self.store.lock().unwrap().delete_snippet(id) {
            self.dialog = Some(Dialog::failure("No se pudo eliminar el snippet", format!("{error:#}")));
            return;
        }
        self.refresh_connections();
        let text = format!("«{}» eliminado", snippet.title);
        self.push_toast(text, Some(Undo::Snippet(snippet)), None);
    }

    pub(super) fn import_ssh_config(&mut self) {
        let path = std::env::home_dir().unwrap_or_default().join(".ssh").join("config");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.dialog = Some(Dialog::failure("No se pudo leer ~/.ssh/config", format!("{}: {error}", path.display())));
                return;
            }
        };
        let user = std::env::var("USER").unwrap_or_default();
        let mut imported = 0;
        let mut jumps: Vec<(i64, String)> = Vec::new();
        {
            let store = self.store.lock().unwrap();
            for host in crate::connect::sshconfig::parse(&text, &user) {
                if self.connections.iter().any(|c| c.name == host.connection.name) {
                    continue;
                }
                if let Ok(id) = store.save_connection(&host.connection) {
                    imported += 1;
                    if let Some(alias) = host.jump_alias {
                        jumps.push((id, alias));
                    }
                }
            }
        }
        self.refresh_connections();
        // Con todos los hosts ya guardados, se resuelven los saltos por nombre.
        for (id, alias) in jumps {
            let jump = self.connections.iter().find(|c| c.name == alias).and_then(|c| c.id);
            let target = self.connections.iter().find(|c| c.id == Some(id)).cloned();
            if let (Some(jump), Some(mut target)) = (jump, target) {
                target.jump = Some(jump);
                let _ = self.store.lock().unwrap().save_connection(&target);
            }
        }
        self.refresh_connections();
        self.notify(format!("Importadas {imported} conexiones de ~/.ssh/config"));
    }

    /// Hasta 5 conexiones para el menú del «+»: las últimas usadas y, si faltan, las demás por nombre.
    pub(super) fn recent_connections(&self) -> Vec<Connection> {
        let mut list: Vec<Connection> = self
            .recent
            .iter()
            .filter_map(|(id, _)| self.connections.iter().find(|c| c.id == Some(*id)).cloned())
            .take(5)
            .collect();
        if list.len() < 5 {
            let mut rest: Vec<&Connection> = self.connections.iter().filter(|c| !list.iter().any(|l| l.id == c.id)).collect();
            rest.sort_by_key(|c| c.name.to_lowercase());
            list.extend(rest.into_iter().take(5 - list.len()).cloned());
        }
        list
    }
}
