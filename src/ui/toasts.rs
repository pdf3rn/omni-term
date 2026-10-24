//! Avisos temporales con «Deshacer».

use super::*;

impl App {
    /// Aviso breve, sin acción.
    pub(super) fn notify(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), None, None);
    }

    pub(super) fn push_toast(&mut self, text: String, undo: Option<Undo>, cleanup: Option<Cleanup>) {
        self.next_toast += 1;
        self.toasts.push(Toast { id: self.next_toast, text, undo, cleanup, born: std::time::Instant::now() });
        // Como mucho tres a la vez: el más antiguo caduca ya.
        while self.toasts.len() > 3 {
            let old = self.toasts.remove(0);
            self.finish_toast(old);
        }
    }

    /// Aplica la limpieza pendiente de un aviso que se cierra sin deshacer.
    pub(super) fn finish_toast(&mut self, toast: Toast) {
        if let Some(Cleanup::Secrets(id)) = &toast.cleanup {
            let _ = self.secrets.delete(&account_for(*id, SecretKind::Password));
            let _ = self.secrets.delete(&account_for(*id, SecretKind::Passphrase));
        }
    }

    /// Dibuja los avisos abajo a la derecha, con fundido de entrada y de salida, y procesa su caducidad.
    pub(super) fn toasts_ui(&mut self, ctx: &egui::Context) {
        let mut undone: Option<usize> = None;
        let mut dismissed: Option<usize> = None;
        let mut expired = Vec::new();
        for (index, toast) in self.toasts.iter().enumerate() {
            if toast.born.elapsed() >= TOAST_LIFETIME + TOAST_FADE {
                expired.push(index);
            }
        }
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Area::new(egui::Id::NULL.with("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for (index, toast) in self.toasts.iter().enumerate().rev() {
                    let fading_out = toast.born.elapsed() >= TOAST_LIFETIME;
                    let opacity = ctx.animate_value_with_time(egui::Id::NULL.with(("toast", toast.id)), if fading_out { 0.0 } else { 1.0 }, 0.15);
                    ui.scope(|ui| {
                        ui.set_opacity(opacity);
                        Frame::popup(ui.style()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(&toast.text);
                                if toast.undo.is_some() && crate::ui::kit::button_with(ui, Button::new(RichText::new("Deshacer").color(theme::accent())), crate::ui::kit::Variant::Ghost).clicked() {
                                    undone = Some(index);
                                }
                                if ui.add(Button::new(icons::X).frame(false)).on_hover_text("Cerrar").clicked() {
                                    dismissed = Some(index);
                                }
                            });
                        });
                    });
                }
            });

        if let Some(index) = undone {
            let toast = self.toasts.remove(index);
            if let Some(undo) = toast.undo {
                self.undo(undo);
            }
        } else if let Some(index) = dismissed {
            let toast = self.toasts.remove(index);
            self.finish_toast(toast);
        } else {
            for index in expired.into_iter().rev() {
                let toast = self.toasts.remove(index);
                self.finish_toast(toast);
            }
        }
    }

    pub(super) fn undo(&mut self, undo: Undo) {
        let result = {
            let mut store = self.store.lock().unwrap();
            match &undo {
                Undo::Connection { connection, forwards, dependents } => store
                    .restore_connection(connection)
                    .and_then(|_| store.replace_forwards(connection.id.unwrap_or_default(), forwards))
                    .and_then(|_| dependents.iter().try_for_each(|id| store.set_jump(*id, connection.id))),
                Undo::Snippet(snippet) => store.restore_snippet(snippet),
                Undo::KnownHosts(lines) => {
                    let path = ssh::default_known_hosts();
                    lines.iter().try_for_each(|(index, raw)| known_hosts::insert(&path, *index, raw)).map_err(Into::into)
                }
                Undo::Environment { id, env, color } => match self.connections.iter().find(|c| c.id == Some(*id)).cloned() {
                    Some(mut connection) => {
                        connection.env = env.clone();
                        connection.color = *color;
                        store.save_connection(&connection).map(|_| ())
                    }
                    None => Ok(()),
                },
            }
        };
        match result {
            Ok(()) => {
                self.known_hosts.loaded = false;
                self.refresh_connections()
            }
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo deshacer", format!("{error:#}"))),
        }
    }
}

/// Cómo deshacer lo que mostró un aviso.
pub(super) enum Undo {
    Connection { connection: Connection, forwards: Vec<Forward>, dependents: Vec<i64> },
    Snippet(Snippet),
    /// Entorno y color anteriores de una conexión movida de grupo.
    Environment { id: i64, env: Option<String>, color: Option<u8> },
    /// Líneas de `known_hosts` borradas: posición y texto.
    KnownHosts(Vec<(usize, String)>),
}

/// Limpieza que solo se hace si el aviso caduca sin deshacer.
pub(super) enum Cleanup {
    Secrets(i64),
}

pub(super) struct Toast {
    pub(super) id: u64,
    pub(super) text: String,
    pub(super) undo: Option<Undo>,
    pub(super) cleanup: Option<Cleanup>,
    pub(super) born: std::time::Instant,
}

pub(super) const TOAST_LIFETIME: Duration = Duration::from_secs(8);

pub(super) const TOAST_FADE: Duration = Duration::from_millis(150);
