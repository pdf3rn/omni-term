use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Panel,
    RichText, ViewportCommand,
};
use zeroize::Zeroizing;

use crate::dialogs::{self, ConnForm, Dialog, Outcome, Pending, SecretPrompt, UnlockPrompt};
use crate::pane::{bold_family, Pane, PADDING};
use crate::secrets::{account_for, SecretError, SecretKind, SecretStore, Secrets};
use crate::ssh::{self, Credentials, HostKeyRequest, UiEvent};
use crate::store::{Auth, Connection, Store, TabKind};
use crate::term::Session;
use crate::theme;

const SIDEBAR_WIDTH: f32 = 230.0;
const TAB_BAR_HEIGHT: f32 = 38.0;

struct Tab {
    kind: TabKind,
    label: String,
    pane: Pane,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    debug_keys: bool,
    window_title: String,
    ctx: egui::Context,
    runtime: tokio::runtime::Runtime,
    store: Arc<Mutex<Store>>,
    secrets: Secrets,
    connections: Vec<Connection>,
    events_tx: Sender<UiEvent>,
    events_rx: Receiver<UiEvent>,
    host_keys: VecDeque<HostKeyRequest>,
    open_queue: VecDeque<TabKind>,
    restore_active: Option<usize>,
    dialog: Option<Dialog>,
    saved_layout: (Vec<TabKind>, usize),
    notice: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, debug_keys: bool) -> anyhow::Result<Self> {
        install_fonts(&cc.egui_ctx);
        install_visuals(&cc.egui_ctx);

        let mut notice = None;
        let store = match Store::default_path().and_then(|path| Store::open(&path)) {
            Ok(store) => store,
            Err(error) => {
                notice = Some(format!("Sin base de datos persistente: {error:#}"));
                Store::open_in_memory()?
            }
        };
        let (layout, active) = store.load_layout().unwrap_or_default();
        let connections = store.list_connections().unwrap_or_default();
        let store = Arc::new(Mutex::new(store));
        let secrets = Secrets::select(store.clone());
        let (events_tx, events_rx) = mpsc::channel();

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;

        let mut open_queue: VecDeque<TabKind> = layout.into();
        if open_queue.is_empty() {
            open_queue.push_back(TabKind::Local);
        }

        Ok(Self {
            tabs: Vec::new(),
            active: 0,
            debug_keys,
            window_title: String::new(),
            ctx: cc.egui_ctx.clone(),
            runtime,
            store,
            secrets,
            connections,
            events_tx,
            events_rx,
            host_keys: VecDeque::new(),
            open_queue,
            restore_active: Some(active),
            dialog: None,
            saved_layout: (Vec::new(), 0),
            notice,
        })
    }

    fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let ctx = self.ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    fn open_local(&mut self) -> anyhow::Result<()> {
        let wake = self.wake();
        let session = Session::start(80, 24, move || wake(), |feed| crate::local::spawn(80, 24, feed))?;
        self.tabs.push(Tab { kind: TabKind::Local, label: "Terminal local".into(), pane: Pane::new(session, self.debug_keys) });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    fn open_ssh(&mut self, connection: Connection, credentials: Credentials) -> anyhow::Result<()> {
        let Some(id) = connection.id else { anyhow::bail!("la conexión no está guardada") };
        let wake = self.wake();
        let wake_for_ssh = wake.clone();
        let handle = self.runtime.handle().clone();
        let events = self.events_tx.clone();
        let label = connection.name.clone();
        let session = Session::start(80, 24, move || wake(), move |feed| {
            Ok(ssh::spawn(&handle, connection, credentials, (80, 24), feed, events, wake_for_ssh, ssh::default_known_hosts()))
        })?;
        self.tabs.push(Tab { kind: TabKind::Connection(id), label, pane: Pane::new(session, self.debug_keys) });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Reúne los secretos necesarios (pidiéndolos si hace falta) y abre la pestaña SSH.
    fn begin_open(&mut self, connection: Connection) {
        let kind = match connection.auth {
            Auth::Password => Some(SecretKind::Password),
            Auth::Key => match connection.key_path.as_deref().map(ssh::key_is_encrypted) {
                Some(Ok(true)) => Some(SecretKind::Passphrase),
                Some(Err(error)) => {
                    self.dialog = Some(Dialog::Message(format!("{error:#}")));
                    return;
                }
                _ => None,
            },
            Auth::Agent => None,
        };
        let Some(kind) = kind else {
            self.finish_open(connection, Credentials::default());
            return;
        };

        let account = account_for(connection.id.unwrap_or_default(), kind);
        match self.secrets.get(&account) {
            Ok(Some(secret)) => self.finish_open(connection, credentials(kind, secret)),
            Ok(None) => {
                self.dialog = Some(Dialog::Secret(SecretPrompt { connection, kind, input: String::new(), save: true }));
            }
            Err(SecretError::Locked) => self.ask_unlock(Pending::Open(connection)),
            Err(error) => self.dialog = Some(Dialog::Message(format!("Almacén de secretos: {error}"))),
        }
    }

    fn finish_open(&mut self, connection: Connection, credentials: Credentials) {
        if let Err(error) = self.open_ssh(connection, credentials) {
            self.dialog = Some(Dialog::Message(format!("No se pudo abrir la conexión: {error:#}")));
        }
    }

    fn ask_unlock(&mut self, then: Pending) {
        let creating = self.secrets.master().is_some_and(|m| !m.is_initialized().unwrap_or(false));
        self.dialog = Some(Dialog::Unlock(UnlockPrompt {
            creating,
            input: String::new(),
            confirm: String::new(),
            error: None,
            then,
        }));
    }

    fn save_secret(&mut self, account: String, secret: Zeroizing<String>) {
        match self.secrets.set(&account, &secret) {
            Ok(()) => {}
            Err(SecretError::Locked) => self.ask_unlock(Pending::Save { account, secret }),
            Err(error) => self.dialog = Some(Dialog::Message(format!("No se pudo guardar el secreto: {error}"))),
        }
    }

    fn refresh_connections(&mut self) {
        self.connections = self.store.lock().unwrap().list_connections().unwrap_or_default();
    }

    fn close_tab(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if index < self.active {
            self.active -= 1;
        }
    }

    fn import_ssh_config(&mut self) {
        let path = std::env::home_dir().unwrap_or_default().join(".ssh").join("config");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.dialog = Some(Dialog::Message(format!("No se pudo leer {}: {error}", path.display())));
                return;
            }
        };
        let user = std::env::var("USER").unwrap_or_default();
        let mut imported = 0;
        {
            let store = self.store.lock().unwrap();
            for connection in crate::sshconfig::parse(&text, &user) {
                if self.connections.iter().any(|c| c.name == connection.name) {
                    continue;
                }
                if store.save_connection(&connection).is_ok() {
                    imported += 1;
                }
            }
        }
        self.refresh_connections();
        self.notice = Some(format!("Importadas {imported} conexiones de ~/.ssh/config"));
    }

    fn drive_pending(&mut self) {
        while let Ok(UiEvent::HostKey(request)) = self.events_rx.try_recv() {
            self.host_keys.push_back(request);
        }

        let exited: Vec<usize> =
            (0..self.tabs.len()).rev().filter(|&i| self.tabs[i].pane.session.has_exited()).collect();
        for index in exited {
            self.close_tab(index);
        }

        if self.dialog.is_some() {
            return;
        }
        if let Some(request) = self.host_keys.pop_front() {
            self.dialog = Some(Dialog::HostKey(request));
        } else if let Some(kind) = self.open_queue.pop_front() {
            match kind {
                TabKind::Local => {
                    if let Err(error) = self.open_local() {
                        self.dialog = Some(Dialog::Message(format!("No se pudo abrir el terminal local: {error:#}")));
                    }
                }
                TabKind::Connection(id) => {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.begin_open(connection);
                    }
                }
            }
        } else if let Some(active) = self.restore_active.take() {
            if self.tabs.is_empty() {
                let _ = self.open_local();
            }
            self.active = active.min(self.tabs.len().saturating_sub(1));
        }
    }

    fn persist_layout(&mut self) {
        if self.restore_active.is_some() {
            return;
        }
        let layout = (self.tabs.iter().map(|t| t.kind).collect::<Vec<_>>(), self.active);
        if layout != self.saved_layout {
            if let Err(error) = self.store.lock().unwrap().save_layout(&layout.0, layout.1) {
                eprintln!("no se pudo guardar la sesión: {error:#}");
            }
            self.saved_layout = layout;
        }
    }

    fn handle_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let forced = Secrets::is_master_forced(&self.store.lock().unwrap());
        let outcome = dialogs::show(ctx, &mut dialog, forced);

        match (outcome, dialog) {
            (Outcome::None, dialog) => self.dialog = Some(dialog),
            (Outcome::Cancel, Dialog::Secret(_) | Dialog::Unlock(_) | Dialog::Edit(_) | Dialog::ConfirmDelete { .. } | Dialog::ConfirmReset(_) | Dialog::Message(_)) => {}
            (Outcome::Cancel, dialog) => self.dialog = Some(dialog),
            (Outcome::Trust(accepted), Dialog::HostKey(request)) => {
                let _ = request.reply.send(accepted);
            }
            (Outcome::Save, Dialog::Edit(mut form)) => match form.to_connection() {
                Err(message) => {
                    form.error = Some(message);
                    self.dialog = Some(Dialog::Edit(form));
                }
                Ok(connection) => {
                    let saved = self.store.lock().unwrap().save_connection(&connection);
                    match saved {
                        Err(error) => {
                            form.error = Some(format!("{error:#}"));
                            self.dialog = Some(Dialog::Edit(form));
                        }
                        Ok(id) => {
                            self.refresh_connections();
                            if !form.secret.is_empty() && connection.auth != Auth::Agent {
                                let kind = if connection.auth == Auth::Password { SecretKind::Password } else { SecretKind::Passphrase };
                                self.save_secret(account_for(id, kind), Zeroizing::new(std::mem::take(&mut form.secret)));
                            }
                        }
                    }
                }
            },
            (Outcome::Delete, Dialog::ConfirmDelete { id, .. }) => {
                let _ = self.store.lock().unwrap().delete_connection(id);
                let _ = self.secrets.delete(&account_for(id, SecretKind::Password));
                let _ = self.secrets.delete(&account_for(id, SecretKind::Passphrase));
                self.refresh_connections();
            }
            (Outcome::Delete, Dialog::ConfirmReset(then)) => {
                if let Some(master) = self.secrets.master() {
                    if let Err(error) = master.reset() {
                        self.dialog = Some(Dialog::Message(format!("No se pudo borrar: {error:#}")));
                        return;
                    }
                }
                self.ask_unlock(then);
            }
            (Outcome::ResetMasterKey, Dialog::Unlock(prompt)) => self.dialog = Some(Dialog::ConfirmReset(prompt.then)),
            (Outcome::Submit, Dialog::Secret(prompt)) => {
                let secret = Zeroizing::new(prompt.input);
                let account = account_for(prompt.connection.id.unwrap_or_default(), prompt.kind);
                self.finish_open(prompt.connection, credentials(prompt.kind, secret.clone()));
                if prompt.save && !secret.is_empty() {
                    self.save_secret(account, secret);
                }
            }
            (Outcome::Submit, Dialog::Unlock(mut prompt)) => {
                let Some(master) = self.secrets.master().cloned() else { return };
                if prompt.input.is_empty() {
                    prompt.error = Some("Escribe la clave maestra".into());
                } else if prompt.creating && prompt.input != prompt.confirm {
                    prompt.error = Some("Las claves no coinciden".into());
                } else {
                    match master.unlock(&prompt.input) {
                        Ok(()) => {
                            match prompt.then {
                                Pending::Open(connection) => self.begin_open(connection),
                                Pending::Save { account, secret } => self.save_secret(account, secret),
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

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let mut open_local = false;
        let mut new_connection = false;
        let mut import = false;
        let mut connect: Option<Connection> = None;
        let mut edit: Option<Connection> = None;
        let mut delete: Option<Connection> = None;

        Panel::left("sidebar")
            .default_size(SIDEBAR_WIDTH)
            .min_size(170.0)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                ui.label(RichText::new("OMNITERM").small().color(theme::SUBTEXT));
                ui.add_space(8.0);
                let full = egui::vec2(ui.available_width(), 28.0);
                if ui.add(Button::new("+ Terminal local").corner_radius(6).min_size(full)).clicked() {
                    open_local = true;
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("CONEXIONES").small().color(theme::SUBTEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::new("+").frame(false)).on_hover_text("Nueva conexión").clicked() {
                            new_connection = true;
                        }
                    });
                });
                ui.add_space(4.0);
                if self.connections.is_empty() {
                    ui.label(RichText::new("Sin conexiones guardadas").color(theme::SUBTEXT));
                }
                for connection in &self.connections {
                    let label = format!("{}\n{}@{}", connection.name, connection.user, connection.host);
                    let response = ui.add(
                        Button::new(RichText::new(label).color(theme::TEXT))
                            .fill(Color32::TRANSPARENT)
                            .corner_radius(6)
                            .min_size(egui::vec2(ui.available_width(), 40.0)),
                    );
                    if response.double_clicked() {
                        connect = Some(connection.clone());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Conectar").clicked() {
                            connect = Some(connection.clone());
                            ui.close();
                        }
                        if ui.button("Editar").clicked() {
                            edit = Some(connection.clone());
                            ui.close();
                        }
                        if ui.button("Eliminar").clicked() {
                            delete = Some(connection.clone());
                            ui.close();
                        }
                    });
                }
                ui.add_space(12.0);
                if ui.add(Button::new("Importar ~/.ssh/config").corner_radius(6)).clicked() {
                    import = true;
                }
                if let Some(notice) = &self.notice {
                    ui.add_space(8.0);
                    ui.label(RichText::new(notice).small().color(theme::SUBTEXT));
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let mut forced = Secrets::is_master_forced(&self.store.lock().unwrap());
                    if ui.checkbox(&mut forced, "Forzar clave maestra").changed() {
                        let result = Secrets::set_master_forced(&self.store.lock().unwrap(), forced);
                        self.notice = Some(match result {
                            Ok(()) => "Reinicia omniterm para aplicar el cambio de almacén".into(),
                            Err(error) => format!("No se pudo guardar el ajuste: {error:#}"),
                        });
                    }
                    ui.label(RichText::new(format!("Secretos: {}", self.secrets.label())).small().color(theme::SUBTEXT));
                });
            });

        if open_local {
            self.open_queue.push_back(TabKind::Local);
        }
        if new_connection {
            self.dialog = Some(Dialog::Edit(ConnForm::new()));
        }
        if import {
            self.import_ssh_config();
        }
        if let Some(connection) = connect {
            self.begin_open(connection);
        }
        if let Some(connection) = edit {
            self.dialog = Some(Dialog::Edit(ConnForm::from_connection(&connection)));
        }
        if let Some(connection) = delete {
            self.dialog = connection.id.map(|id| Dialog::ConfirmDelete { id, name: connection.name });
        }
    }
}

fn credentials(kind: SecretKind, secret: Zeroizing<String>) -> Credentials {
    match kind {
        SecretKind::Password => Credentials { password: Some(secret), key_passphrase: None },
        SecretKind::Passphrase => Credentials { password: None, key_passphrase: Some(secret) },
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "jb-regular".into(),
        FontData::from_static(include_bytes!("../assets/JetBrainsMono-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "jb-bold".into(),
        FontData::from_static(include_bytes!("../assets/JetBrainsMono-Bold.ttf")).into(),
    );
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jb-regular".into());
    fonts.families.insert(bold_family(), vec!["jb-bold".into()]);
    ctx.set_fonts(fonts);
}

fn install_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = theme::BASE;
    visuals.window_fill = theme::BASE;
    visuals.override_text_color = Some(theme::TEXT);
    visuals.widgets.inactive.weak_bg_fill = theme::SURFACE;
    visuals.widgets.hovered.weak_bg_fill = theme::SELECTION;
    visuals.widgets.active.weak_bg_fill = theme::SELECTION;
    ctx.set_visuals(visuals);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drive_pending();

        let finished_restoring = self.restore_active.is_none();
        if self.tabs.is_empty() && finished_restoring && self.open_queue.is_empty() && self.dialog.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        self.sidebar(ui);

        let mut activate = None;
        let mut close = None;
        let mut new_local = false;
        Panel::top("tabs")
            .exact_size(TAB_BAR_HEIGHT)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin::symmetric(8, 5)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let selected = i == self.active;
                        let color = if selected { theme::TEXT } else { theme::SUBTEXT };
                        let fill = if selected { theme::BASE } else { Color32::TRANSPARENT };
                        let title = tab.pane.title();
                        let label = if title.is_empty() { tab.label.clone() } else { title };
                        if ui.add(Button::new(RichText::new(label).color(color)).fill(fill).corner_radius(6)).clicked() {
                            activate = Some(i);
                        }
                        if ui.add(Button::new("×").frame(false)).clicked() {
                            close = Some(i);
                        }
                        ui.add_space(4.0);
                    }
                    if ui.add(Button::new("+").frame(false)).clicked() {
                        new_local = true;
                    }
                });
            });

        if let Some(i) = activate {
            self.active = i;
        }
        if new_local {
            self.open_queue.push_back(TabKind::Local);
        }
        if let Some(i) = close {
            self.close_tab(i);
        }

        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, CornerRadius::ZERO, theme::BASE);
        if !self.tabs.is_empty() {
            let active = self.active.min(self.tabs.len() - 1);
            let input_enabled = self.dialog.is_none() && self.host_keys.is_empty();
            self.tabs[active].pane.show(ui, area.shrink(PADDING), input_enabled);

            let title = self.tabs[active].pane.title();
            let title = if title.is_empty() { "omniterm".to_string() } else { title };
            if title != self.window_title {
                ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
                self.window_title = title;
            }
        }

        self.handle_dialog(&ctx);
        self.persist_layout();
    }
}
