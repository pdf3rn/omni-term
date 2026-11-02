//! Trabajo de cada fotograma: eventos pendientes, avisos de actividad y sesión guardada.

use super::*;

impl App {
    /// Marca las pestañas en segundo plano cuyo trabajo terminó y, con la ventana sin foco, avisa al sistema.
    pub(super) fn watch_activity(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let threshold = Duration::from_secs(u64::from(self.settings.notify_after));
        let enabled = self.settings.notify_after > 0;
        let mut finished: Vec<String> = Vec::new();
        let mut busy = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Body::Terminal { panes, .. } = &tab.body else { continue };
            let watching = index == self.active && focused;
            let (mut hit, mut running) = (false, false);
            panes.for_each(&mut |_, pane| {
                let mut activity = pane.session.activity.lock().unwrap();
                match activity.pending() {
                    Some(_) if watching || !enabled => activity.mark_reported(),
                    Some((length, quiet)) if quiet >= term::BURST_GAP => {
                        hit |= length >= threshold;
                        activity.mark_reported();
                    }
                    Some(_) => running = true,
                    None => {}
                }
            });
            if watching {
                tab.attention = false;
            } else if hit {
                tab.attention = true;
                finished.push(tab.title());
                ctx.request_repaint();
            }
            busy |= running;
        }
        if busy {
            ctx.request_repaint_after(term::BURST_GAP / 2);
        }
        if !focused {
            for title in finished {
                std::thread::spawn(move || {
                    let _ = notify_rust::Notification::new()
                        .summary("omni-term")
                        .body(&format!("Terminó la actividad en «{title}»"))
                        .show();
                });
            }
        }
    }

    pub(super) fn drive_pending(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.pending_ui.push_back(event);
        }

        // Un panel cuyo proceso terminó se quita; si era el único de la pestaña, se cierra la pestaña.
        let mut closing_tabs = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Body::Terminal { panes, focused, zoomed, .. } = &mut tab.body else { continue };
            let mut gone = Vec::new();
            panes.for_each(&mut |id, pane| {
                if pane.session.has_exited() {
                    gone.push(id);
                }
            });
            for id in gone {
                if let Some(heir) = panes.remove(id) {
                    if *focused == id {
                        *focused = heir;
                    }
                    if *zoomed == Some(id) {
                        *zoomed = None;
                    }
                } else {
                    closing_tabs.push(index);
                }
            }
        }
        for index in closing_tabs.into_iter().rev() {
            self.close_tab(index);
        }

        // El editor de conexión cede el paso a las preguntas del servidor (huella, 2FA) de «Probar conexión».
        if !self.pending_ui.is_empty() && matches!(self.dialog, Some(Dialog::Edit(_))) {
            self.suspended = self.dialog.take();
        }
        if self.dialog.is_some() {
            return;
        }
        if let Some(event) = self.pending_ui.pop_front() {
            self.dialog = Some(match event {
                UiEvent::HostKey(request) => Dialog::HostKey(request),
                UiEvent::HostKeyChanged(change) => Dialog::HostKeyChanged(change),
                UiEvent::Prompt(request) => Dialog::Prompt(request, Vec::new()),
            });
        } else if let Some(dialog) = self.suspended.take() {
            self.dialog = Some(dialog);
        } else if let Some((account, secret)) = self.save_queue.pop_front() {
            self.save_secret(account, secret);
        } else if self.pending_migration.is_some() && self.window_boot.elapsed() < RESTORE_PROMPT_DELAY {
            // Mismo motivo que abajo: nada de diálogos en los primeros fotogramas.
        } else if let Some(from) = self.pending_migration.take() {
            self.begin_migration(from);
        } else if self.restore_prompt && self.window_boot.elapsed() < RESTORE_PROMPT_DELAY {
            // Un diálogo en los primeros fotogramas deja a la ventana sin recibir eventos.
        } else if self.restore_prompt {
            let remote = self
                .open_queue
                .iter()
                .filter(|s| match s.kind {
                    TabKind::Connection(id) | TabKind::Explorer(id) => self.connections.iter().any(|c| c.id == Some(id) && !c.is_local()),
                    _ => false,
                })
                .count();
            self.dialog = Some(Dialog::RestoreSession(dialogs::RestorePrompt { tabs: self.open_queue.len(), remote, remember: false }));
        } else if self.window_boot.elapsed() < RESTORE_PROMPT_DELAY && self.open_queue.front().is_some_and(|s| !matches!(s.kind, TabKind::Local)) {
            // Un diálogo en los primeros fotogramas deja a la ventana sin recibir eventos de ratón ni teclado.
        } else if let Some(saved) = self.open_queue.pop_front() {
            match saved.kind {
                TabKind::Local => {
                    if let Err(error) = self.open_local() {
                        self.dialog = Some(Dialog::failure("No se pudo abrir el terminal local", format!("{error:#}")));
                    }
                }
                TabKind::Connection(id) => {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.continue_open(connection, None, Purpose::Terminal);
                    }
                }
                TabKind::Explorer(id) => {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.continue_open(connection, None, Purpose::Explorer(saved.local_path, saved.remote_path));
                    }
                }
                TabKind::Containers | TabKind::Container | TabKind::Settings => {}
            }
        } else if let Some(active) = self.restore_active.take() {
            self.active = active.min(self.tabs.len().saturating_sub(1));
        }
    }

    pub(super) fn persist_layout(&mut self) {
        if self.restore_active.is_some() {
            return;
        }
        // Los contenedores son efímeros: ni el administrador ni sus terminales se restauran.
        let persisted = |t: &Tab| !matches!(t.kind, TabKind::Containers | TabKind::Container | TabKind::Settings);
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .filter(|t| persisted(t))
            .map(|t| match &t.body {
                Body::Explorer(explorer) => {
                    let (local, remote) = explorer.paths();
                    SavedTab { kind: t.kind, local_path: Some(local), remote_path: Some(remote).filter(|r| !r.is_empty()) }
                }
                Body::Terminal { .. } | Body::Containers(_) | Body::Settings(_) => SavedTab::plain(t.kind),
            })
            .collect();
        let active = self.tabs.iter().take(self.active + 1).filter(|t| persisted(t)).count().saturating_sub(1);
        let layout = (tabs, active);
        if layout != self.saved_layout {
            if let Err(error) = self.store.lock().unwrap().save_layout(&layout.0, layout.1) {
                eprintln!("no se pudo guardar la sesión: {error:#}");
            }
            self.saved_layout = layout;
        }
    }
}
