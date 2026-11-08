//! Gestor de contraseñas: escribir una contraseña guardada en el terminal (Ctrl+Alt+P), por ejemplo
//! cuando `sudo` la pide, y darlas de alta, editarlas y borrarlas desde Ajustes.

use crate::i18n::t;
use super::*;
use crate::data::secrets::password_account;
use crate::terminal::password_prompt::looks_like_password_prompt;
use crate::ui::dialogs::{PasswordRequest, TypeRequest};
use crate::ui::palette::PasswordSource;

impl App {
    pub(super) fn refresh_passwords(&mut self) {
        self.passwords = self.store.lock().unwrap().list_passwords().unwrap_or_default();
    }

    /// Abre el selector de contraseñas para el panel con foco de la pestaña activa.
    pub(super) fn open_password_picker(&mut self) {
        let target = self.tabs.get(self.active).and_then(|tab| match &tab.body {
            Body::Terminal { focused, .. } if tab.pane().is_some_and(|p| !p.session.is_disconnected()) => Some((tab.id, *focused)),
            _ => None,
        });
        match target {
            Some((tab, pane)) => self.palette = Some(Palette::passwords(tab, pane)),
            None => self.notify(t("passwords-abre-un-terminal-para-escribir")),
        }
    }

    /// Elementos del selector: primero la contraseña de la conexión de la pestaña, si la tiene; luego las
    /// del gestor y, al final, la acción de gestionarlas.
    pub(super) fn password_items(&self, tab: u64) -> Vec<palette::Item> {
        let mut items = Vec::new();
        let connection = match self.tabs.iter().find(|t| t.id == tab).map(|t| t.kind) {
            Some(TabKind::Connection(id)) => self.connections.iter().find(|c| c.id == Some(id) && c.auth == Auth::Password),
            _ => None,
        };
        if let Some(connection) = connection {
            items.push(palette::Item {
                title: crate::tf!("passwords-password-of", "name" => connection.name.to_string()),
                detail: connection.target(),
                kind: t("commands-conexion"),
                target: Target::Password(PasswordSource::Connection(connection.id.unwrap_or_default())),
            });
        }
        for password in &self.passwords {
            items.push(palette::Item {
                title: password.name.clone(),
                detail: password.user.clone(),
                kind: t("passwords-guardada"),
                target: Target::Password(PasswordSource::Saved(password.id.unwrap_or_default())),
            });
        }
        let manage = if self.passwords.is_empty() { t("passwords-anadir-una-contrasena") } else { t("palette-gestionar-contrasenas") };
        items.push(palette::Item { title: manage.into(), detail: String::new(), kind: t("commands-accion"), target: Target::Command(Command::Passwords) });
        items
    }

    fn password_label(&self, source: PasswordSource) -> String {
        match source {
            PasswordSource::Connection(id) => {
                self.connections.iter().find(|c| c.id == Some(id)).map_or_else(|| t("passwords-la-conexion").into(), |c| format!("«{}»", c.name))
            }
            PasswordSource::Saved(id) => self.passwords.iter().find(|p| p.id == Some(id)).map_or_else(|| t("connecting-la-contrasena").into(), |p| format!("«{}»", p.name)),
        }
    }

    /// Escribe la contraseña en el panel pedido. Si el terminal no parece estar pidiendo una contraseña,
    /// pregunta antes: en un shell quedaría a la vista y en el historial.
    pub(super) fn type_password(&mut self, request: TypeRequest) {
        let Some(prompt_line) = self.find_pane(request.tab, request.pane).map(Pane::text_before_cursor) else {
            self.notify(t("passwords-el-panel-donde-ibas-a"));
            return;
        };
        if !request.checked && !looks_like_password_prompt(&prompt_line) {
            self.dialog = Some(Dialog::ConfirmTypePassword(dialogs::TypePasswordConfirm { request, label: self.password_label(request.source), line: prompt_line }));
            return;
        }
        let request = TypeRequest { checked: true, ..request };
        let secret = match request.source {
            PasswordSource::Connection(id) => match self.session_secrets.get(&(id, SecretKind::Password)) {
                Some(secret) => Ok(Some(secret.clone())),
                None => self.secrets.get(&account_for(id, SecretKind::Password)),
            },
            PasswordSource::Saved(id) => self.secrets.get(&password_account(id)),
        };
        match secret {
            Ok(Some(secret)) => {
                if let Some(pane) = self.find_pane(request.tab, request.pane) {
                    pane.type_secret(&secret, request.enter);
                }
            }
            Ok(None) => {
                let label = self.password_label(request.source);
                self.notify(crate::tf!("passwords-none-saved", "what" => label));
            }
            Err(SecretError::Locked) => self.ask_unlock(Pending::TypePassword(request)),
            Err(error) => self.dialog = Some(Dialog::failure(t("passwords-no-se-pudo-leer-la"), error)),
        }
    }

    fn find_pane(&self, tab: u64, pane: u64) -> Option<&Pane> {
        match &self.tabs.iter().find(|t| t.id == tab)?.body {
            Body::Terminal { panes, .. } => panes.get(pane).filter(|p| !p.session.is_disconnected()),
            _ => None,
        }
    }

    /// Atiende lo pedido en la sección de contraseñas de Ajustes.
    pub(super) fn handle_password_request(&mut self, request: PasswordRequest) {
        match request {
            PasswordRequest::New => self.dialog = Some(Dialog::Password(dialogs::PasswordForm::new())),
            PasswordRequest::Edit(password) => self.dialog = Some(Dialog::Password(dialogs::PasswordForm::from_saved(&password))),
            PasswordRequest::Delete(password) => {
                let Some(id) = password.id else { return };
                if let Err(error) = self.store.lock().unwrap().delete_password(id) {
                    self.dialog = Some(Dialog::failure(t("passwords-no-se-pudo-eliminar-la"), format!("{error:#}")));
                    return;
                }
                self.refresh_passwords();
                let text = crate::tf!("passwords-deleted", "name" => password.name.to_string());
                // El secreto se borra cuando el aviso caduca sin deshacer.
                self.push_toast(text, Some(Undo::Password(password)), Some(Cleanup::Password(id)));
            }
        }
    }

    /// Guarda el formulario de una contraseña: el nombre en la base de datos y el secreto en el almacén.
    pub(super) fn save_password_form(&mut self, mut form: dialogs::PasswordForm) {
        let password = match form.to_saved() {
            Ok(password) => password,
            Err(message) => {
                form.error = Some(message);
                self.dialog = Some(Dialog::Password(form));
                return;
            }
        };
        let saved = self.store.lock().unwrap().save_password(&password);
        match saved {
            Err(error) => {
                form.error = Some(crate::tf!("common-save-failed", "error" => format!("{error:#}")));
                self.dialog = Some(Dialog::Password(form));
            }
            Ok(id) => {
                self.refresh_passwords();
                if !form.secret.is_empty() {
                    self.save_secret(password_account(id), std::mem::take(&mut form.secret));
                }
            }
        }
    }
}
