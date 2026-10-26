//! Resultado de los diálogos y de la página de ajustes.

use super::*;

impl App {
    pub(super) fn handle_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let outcome = dialogs::show(ctx, &mut dialog, &self.connections);

        match (outcome, dialog) {
            (Outcome::None, dialog) => self.dialog = Some(dialog),
            (Outcome::Submit, Dialog::Error(ErrorInfo { retry: Some(Retry::Open(connection, slot, purpose)), .. })) => {
                self.continue_open(connection, slot, purpose);
            }
            (Outcome::Cancel, Dialog::Prompt(request, _)) => {
                let _ = request.reply.send(None);
            }
            (Outcome::Submit, Dialog::SecretsMode(prompt)) => self.apply_secrets_mode(prompt.to_master, true),
            (Outcome::Alternate, Dialog::SecretsMode(prompt)) => self.apply_secrets_mode(prompt.to_master, false),
            (Outcome::Submit, Dialog::Restart) => self.restart_app(),
            (Outcome::Cancel, Dialog::SecretsMode(_) | Dialog::Restart) => {}
            (Outcome::Cancel, Dialog::RestoreSession(prompt)) => self.answer_restore(false, prompt.remember),
            (Outcome::Submit, Dialog::RestoreSession(prompt)) => self.answer_restore(true, prompt.remember),
            (Outcome::Cancel, Dialog::Secret(_) | Dialog::ConfirmContainer(_) | Dialog::Unlock(_) | Dialog::Edit(_) | Dialog::Error(_) | Dialog::HostKeyChanged(_) | Dialog::About | Dialog::ConfirmClose(_) | Dialog::ConfirmPaste(_) | Dialog::TelnetWarning(_) | Dialog::Snippet(_) | Dialog::SnippetVars(_) | Dialog::ConfirmReset(_)) => {}
            (Outcome::Cancel, dialog) => self.dialog = Some(dialog),
            (Outcome::Submit, Dialog::Prompt(request, answers)) => {
                let _ = request.reply.send(Some(answers));
            }
            (Outcome::Trust(accepted), Dialog::HostKey(request)) => {
                let _ = request.reply.send(accepted);
            }
            (Outcome::Save, Dialog::Edit(mut form)) => match form.validate() {
                None => self.dialog = Some(Dialog::Edit(form)),
                Some((connection, forwards)) => {
                    let saved = self.store.lock().unwrap().save_connection(&connection);
                    match saved {
                        Err(error) => {
                            form.errors.general = Some(format!("No se pudo guardar: {error:#}"));
                            self.dialog = Some(Dialog::Edit(form));
                        }
                        Ok(id) => {
                            if let Err(error) = self.store.lock().unwrap().replace_forwards(id, &forwards) {
                                eprintln!("no se pudieron guardar los túneles: {error:#}");
                            }
                            self.refresh_connections();
                            if !form.secret.is_empty() && connection.auth != Auth::Agent {
                                let kind = if connection.auth == Auth::Password { SecretKind::Password } else { SecretKind::Passphrase };
                                self.save_secret(account_for(id, kind), Zeroizing::new(std::mem::take(&mut form.secret)));
                            }
                        }
                    }
                }
            },
            (Outcome::Test, Dialog::Edit(mut form)) => {
                if let Some((connection, _)) = form.validate() {
                    let state = match self.test_credentials(&connection, &form.secret) {
                        Err(message) => TestState::Failed(message),
                        Ok(credentials) => {
                            let status = Arc::new(Mutex::new(TestState::Running));
                            let task_status = status.clone();
                            let (events, wake) = (self.events_tx.clone(), self.wake());
                            self.runtime.spawn(async move {
                                let result =
                                    ssh::test_connection(&connection, &credentials, events, wake.clone(), ssh::default_known_hosts()).await;
                                *task_status.lock().unwrap() = match result {
                                    Ok(message) => TestState::Ok(message),
                                    Err(error) => TestState::Failed(format!("{error:#}")),
                                };
                                wake();
                            });
                            form.test = Some(status);
                            self.dialog = Some(Dialog::Edit(form));
                            return;
                        }
                    };
                    form.test = Some(Arc::new(Mutex::new(state)));
                }
                self.dialog = Some(Dialog::Edit(form));
            }
            (Outcome::Save, Dialog::Snippet(mut form)) => match form.to_snippet() {
                Err(message) => {
                    form.error = Some(message);
                    self.dialog = Some(Dialog::Snippet(form));
                }
                Ok(snippet) => {
                    let saved = self.store.lock().unwrap().save_snippet(&snippet);
                    match saved {
                        Ok(_) => self.refresh_connections(),
                        Err(error) => {
                            form.error = Some(format!("{error:#}"));
                            self.dialog = Some(Dialog::Snippet(form));
                        }
                    }
                }
            },
            (Outcome::Submit, Dialog::SnippetVars(prompt)) => {
                let text = prompt.rendered();
                self.insert_into_active(&text, prompt.run);
            }
            (Outcome::Submit, Dialog::TelnetWarning(mut warning)) => {
                if warning.dont_ask {
                    match &mut warning.connection.protocol {
                        Protocol::Telnet(options) => options.quiet = true,
                        Protocol::Files(store::FilesKind::Ftp(options)) => options.quiet = true,
                        _ => {}
                    }
                    let _ = self.store.lock().unwrap().save_connection(&warning.connection);
                    self.refresh_connections();
                }
                self.continue_open(warning.connection, warning.slot, warning.purpose);
            }
            (Outcome::Forget, Dialog::HostKeyChanged(change)) => {
                let entries = known_hosts::load(&ssh::default_known_hosts()).map(|all| known_hosts::entries_for(&all, &change.host, change.port)).unwrap_or_default();
                self.forget_known_hosts(entries, Some((change.host.clone(), change.port)));
            }
            (Outcome::Delete, Dialog::ConfirmClose(confirm)) => {
                if confirm.dont_ask {
                    self.settings.confirm_close = false;
                    let _ = self.settings.save(&self.store.lock().unwrap());
                }
                self.perform_close(confirm.action);
            }
            (Outcome::Submit, Dialog::ConfirmPaste(confirm)) => {
                if confirm.dont_ask {
                    self.settings.confirm_paste = false;
                    let _ = self.settings.save(&self.store.lock().unwrap());
                }
                self.write_paste(confirm.tab, confirm.pane, &confirm.text);
            }
            (Outcome::Delete, Dialog::ConfirmReset(then)) => {
                if let Some(master) = self.active_master() {
                    if let Err(error) = master.reset() {
                        self.dialog = Some(Dialog::failure("No se pudieron borrar los secretos", format!("{error:#}")));
                        return;
                    }
                }
                self.ask_unlock(then);
            }
            (Outcome::ResetMasterKey, Dialog::Unlock(prompt)) => self.dialog = Some(Dialog::ConfirmReset(prompt.then)),
            (Outcome::Submit, Dialog::ConfirmContainer(confirm)) => self.container_action(confirm),
            (Outcome::Submit, Dialog::Secret(prompt)) => {
                let secret = Zeroizing::new(prompt.input);
                let id = prompt.asking.id.unwrap_or_default();
                self.session_secrets.insert((id, prompt.kind), secret.clone());
                if prompt.save && !secret.is_empty() {
                    self.save_queue.push_back((account_for(id, prompt.kind), secret));
                }
                self.continue_open(prompt.connection, prompt.slot, prompt.purpose);
            }
            (Outcome::Submit, Dialog::Unlock(mut prompt)) => {
                let Some(master) = self.active_master() else { return };
                if prompt.input.is_empty() {
                    prompt.error = Some("Escribe la clave maestra".into());
                } else if prompt.creating && prompt.input != prompt.confirm {
                    prompt.error = Some("Las claves no coinciden".into());
                } else {
                    match master.unlock(&prompt.input) {
                        Ok(()) => {
                            match prompt.then {
                                Pending::Open(connection, slot, purpose) => self.continue_open(connection, slot, purpose),
                                Pending::Save { account, secret } => self.save_secret(account, secret),
                                Pending::Migrate(from) => self.run_migration(from),
                            }
                            return;
                        }
                        Err(SecretError::WrongPassword) => prompt.error = Some("Clave maestra incorrecta".into()),
                        Err(error) => prompt.error = Some(error.to_string()),
                    }
                }
                prompt.input.clear();
                prompt.confirm.clear();
                self.dialog = Some(Dialog::Unlock(prompt));
            }
            (_, dialog) => self.dialog = Some(dialog),
        }
    }

    pub(super) fn answer_restore(&mut self, restore: bool, remember: bool) {
        self.restore_prompt = false;
        if !restore {
            self.open_queue.clear();
        }
        if remember {
            self.settings.on_start = if restore { StartupChoice::Restore } else { StartupChoice::Fresh };
            let _ = self.settings.save(&self.store.lock().unwrap());
        }
    }

    /// Dibuja la página de ajustes y aplica lo que el usuario haya cambiado, al instante.
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui, area: Rect, mut section: dialogs::SettingsTab) -> dialogs::SettingsTab {
        let active_master = self.secrets.master().is_some();
        let forced = Secrets::is_master_forced(&self.store.lock().unwrap());
        let keyring_available = *self.keyring_probe.get_or_insert_with(KeyringStore::probe);
        let mut choice = dialogs::SecretsChoice { master: forced || active_master, keyring_available, active_master };
        let (master_before, settings_before) = (choice.master, self.settings.clone());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
        dialogs::settings_page(&mut child, &mut section, &mut self.settings, &mut choice, &mut self.known_hosts, &self.connections);
        if let Some(entry) = self.known_hosts.delete.take() {
            self.forget_known_hosts(vec![entry], None);
        }
        if self.settings != settings_before {
            self.apply_settings();
        }
        if choice.master != master_before && self.dialog.is_none() {
            let count = self.stored_secret_count();
            if choice.master == active_master || count == 0 {
                self.apply_secrets_mode(choice.master, false);
            } else {
                self.dialog = Some(Dialog::SecretsMode(dialogs::SecretsModePrompt { to_master: choice.master, count }));
            }
        }
        section
    }
}
