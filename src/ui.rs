use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Key, Modifiers, Panel, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Vec2, ViewportCommand,
};
use zeroize::Zeroizing;

use crate::dialogs::{self, ConnForm, Dialog, Outcome, Pending, SecretPrompt, SnippetForm, UnlockPrompt, VarsPrompt};
use crate::pane::{self, bold_family, Pane, DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE, PADDING};
use crate::secrets::{account_for, SecretError, SecretKind, SecretStore, Secrets};
use crate::ssh::{self, Credentials, HostKeyRequest, TunnelState, Tunnels, UiEvent};
use crate::store::{Auth, Connection, Forward, Snippet, Store, TabKind};
use crate::term::Session;
use crate::theme;

const SIDEBAR_WIDTH: f32 = 230.0;
const TAB_BAR_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 34.0;
const CONTROL_RADIUS: u8 = 6;
const ROW_HEIGHT: f32 = 46.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Connections,
    Snippets,
}

struct Tab {
    id: u64,
    kind: TabKind,
    label: String,
    pane: Pane,
    tunnels: Option<Tunnels>,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    debug_keys: bool,
    window_title: String,
    next_tab_id: u64,
    font_size: f32,
    grid: (usize, usize),
    ctx: egui::Context,
    runtime: tokio::runtime::Runtime,
    store: Arc<Mutex<Store>>,
    secrets: Secrets,
    connections: Vec<Connection>,
    forwards: HashMap<i64, Vec<Forward>>,
    snippets: Vec<Snippet>,
    sidebar_view: SidebarView,
    snippet_query: String,
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
        let forwards = store.all_forwards().unwrap_or_default();
        let snippets = store.list_snippets().unwrap_or_default();
        let store = Arc::new(Mutex::new(store));
        let secrets = Secrets::select(store.clone());
        let font_size = store
            .lock()
            .unwrap()
            .meta("font_size")
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| text.parse::<f32>().ok())
            .map_or(DEFAULT_FONT_SIZE, |size| size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE));
        let sidebar_view = match store.lock().unwrap().meta("sidebar_view") {
            Ok(Some(value)) if value == b"snippets" => SidebarView::Snippets,
            _ => SidebarView::Connections,
        };
        cc.egui_ctx.options_mut(|options| options.zoom_with_keyboard = false);
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
            next_tab_id: 0,
            font_size,
            grid: (80, 24),
            ctx: cc.egui_ctx.clone(),
            runtime,
            store,
            secrets,
            connections,
            forwards,
            snippets,
            sidebar_view,
            snippet_query: String::new(),
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
        let (cols, rows) = self.grid;
        let session = Session::start(cols, rows, move || wake(), move |feed| crate::local::spawn(cols, rows, feed))?;
        let id = self.take_tab_id();
        self.tabs.push(Tab { id, kind: TabKind::Local, label: "Terminal local".into(), pane: Pane::new(session, self.debug_keys), tunnels: None });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    fn take_tab_id(&mut self) -> u64 {
        self.next_tab_id += 1;
        self.next_tab_id
    }

    /// Abre la sesión SSH en una pestaña nueva o, con `slot`, en lugar de una caída.
    fn open_ssh(&mut self, connection: Connection, credentials: Credentials, slot: Option<u64>) -> anyhow::Result<()> {
        let Some(id) = connection.id else { anyhow::bail!("la conexión no está guardada") };
        let wake = self.wake();
        let wake_for_ssh = wake.clone();
        let handle = self.runtime.handle().clone();
        let events = self.events_tx.clone();
        let label = connection.name.clone();
        let (cols, rows) = self.grid;
        let forwards = self.forwards.get(&id).cloned().unwrap_or_default();
        let tunnels = ssh::new_tunnels(&forwards);
        let tunnels_for_spawn = tunnels.clone();
        let session = Session::start(cols, rows, move || wake(), move |feed| {
            Ok(ssh::spawn(&handle, connection, credentials, (cols, rows), feed, events, wake_for_ssh, ssh::default_known_hosts(), tunnels_for_spawn))
        })?;
        let tab_id = slot.unwrap_or_else(|| self.take_tab_id());
        let tab = Tab { id: tab_id, kind: TabKind::Connection(id), label, pane: Pane::new(session, self.debug_keys), tunnels: Some(tunnels) };
        match slot.and_then(|slot| self.tabs.iter().position(|t| t.id == slot)) {
            Some(index) => {
                self.tabs[index] = tab;
                self.active = index;
            }
            None => {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
            }
        }
        Ok(())
    }

    /// Reúne los secretos necesarios (pidiéndolos si hace falta) y abre la pestaña SSH.
    fn begin_open(&mut self, connection: Connection, slot: Option<u64>) {
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
            self.finish_open(connection, Credentials::default(), slot);
            return;
        };

        let account = account_for(connection.id.unwrap_or_default(), kind);
        match self.secrets.get(&account) {
            Ok(Some(secret)) => self.finish_open(connection, credentials(kind, secret), slot),
            Ok(None) => {
                self.dialog = Some(Dialog::Secret(SecretPrompt { connection, slot, kind, input: String::new(), save: true }));
            }
            Err(SecretError::Locked) => self.ask_unlock(Pending::Open(connection, slot)),
            Err(error) => self.dialog = Some(Dialog::Message(format!("Almacén de secretos: {error}"))),
        }
    }

    fn finish_open(&mut self, connection: Connection, credentials: Credentials, slot: Option<u64>) {
        if let Err(error) = self.open_ssh(connection, credentials, slot) {
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
        let store = self.store.lock().unwrap();
        self.connections = store.list_connections().unwrap_or_default();
        self.forwards = store.all_forwards().unwrap_or_default();
        self.snippets = store.list_snippets().unwrap_or_default();
    }

    /// Inserta el snippet; si tiene variables, primero pide sus valores.
    fn insert_snippet(&mut self, snippet: &Snippet, run: bool) {
        if crate::snippet_vars::variables(&snippet.body).is_empty() {
            self.insert_into_active(&snippet.body, run);
        } else {
            self.dialog = Some(Dialog::SnippetVars(VarsPrompt::new(snippet, run)));
        }
    }

    fn insert_into_active(&mut self, body: &str, run: bool) {
        match self.tabs.get(self.active) {
            Some(tab) if !tab.pane.session.is_disconnected() => tab.pane.insert_text(body, run),
            _ => self.notice = Some("No hay un terminal activo donde insertar".into()),
        }
    }

    fn reconnect(&mut self, index: usize) {
        let (TabKind::Connection(id), tab_id) = (self.tabs[index].kind, self.tabs[index].id) else { return };
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
            self.begin_open(connection, Some(tab_id));
        }
    }

    fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (size - self.font_size).abs() > f32::EPSILON {
            self.font_size = size;
            let _ = self.store.lock().unwrap().set_meta("font_size", size.to_string().as_bytes());
        }
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
                        self.begin_open(connection, None);
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
            (Outcome::Cancel, Dialog::Secret(_) | Dialog::Unlock(_) | Dialog::Edit(_) | Dialog::Snippet(_) | Dialog::SnippetVars(_) | Dialog::ConfirmDelete { .. } | Dialog::ConfirmDeleteSnippet { .. } | Dialog::ConfirmReset(_) | Dialog::Message(_)) => {}
            (Outcome::Cancel, dialog) => self.dialog = Some(dialog),
            (Outcome::Trust(accepted), Dialog::HostKey(request)) => {
                let _ = request.reply.send(accepted);
            }
            (Outcome::Save, Dialog::Edit(mut form)) => match form.to_connection().and_then(|c| form.to_forwards().map(|f| (c, f))) {
                Err(message) => {
                    form.error = Some(message);
                    self.dialog = Some(Dialog::Edit(form));
                }
                Ok((connection, forwards)) => {
                    let saved = self.store.lock().unwrap().save_connection(&connection);
                    match saved {
                        Err(error) => {
                            form.error = Some(format!("{error:#}"));
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
            (Outcome::Delete, Dialog::ConfirmDeleteSnippet { id, .. }) => {
                let _ = self.store.lock().unwrap().delete_snippet(id);
                self.refresh_connections();
            }
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
                self.finish_open(prompt.connection, credentials(prompt.kind, secret.clone()), prompt.slot);
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
                                Pending::Open(connection, slot) => self.begin_open(connection, slot),
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
        let mut new_snippet = false;
        let mut snippet_from_selection = false;
        let mut insert: Option<(Snippet, bool)> = None;
        let mut copy: Option<String> = None;
        let mut edit_snippet: Option<Snippet> = None;
        let mut delete_snippet: Option<Snippet> = None;
        let open_ids: Vec<i64> = self
            .tabs
            .iter()
            .filter_map(|t| match t.kind {
                TabKind::Connection(id) => Some(id),
                TabKind::Local => None,
            })
            .collect();

        Panel::left("sidebar")
            .default_size(SIDEBAR_WIDTH)
            .min_size(180.0)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                let primary = Button::new(RichText::new("Terminal local").color(theme::BASE).font(FontId::new(14.0, strong_family())))
                    .fill(theme::ACCENT)
                    .min_size(Vec2::new(ui.available_width(), 34.0));
                if ui.add(primary).clicked() {
                    open_local = true;
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for (view, label) in [(SidebarView::Connections, "Conexiones"), (SidebarView::Snippets, "Snippets")] {
                        let selected = self.sidebar_view == view;
                        let color = if selected { theme::TEXT } else { theme::SUBTEXT };
                        let font = FontId::new(13.0, if selected { strong_family() } else { FontFamily::Proportional });
                        let button = Button::new(RichText::new(label).font(font).color(color)).fill(Color32::TRANSPARENT);
                        let response = ui.add(button);
                        if selected {
                            let y = response.rect.max.y + 1.0;
                            ui.painter().line_segment(
                                [Pos2::new(response.rect.min.x + 8.0, y), Pos2::new(response.rect.max.x - 8.0, y)],
                                Stroke::new(2.0, theme::ACCENT),
                            );
                        }
                        if response.clicked() && self.sidebar_view != view {
                            self.sidebar_view = view;
                            let value: &[u8] = if view == SidebarView::Snippets { b"snippets" } else { b"connections" };
                            let _ = self.store.lock().unwrap().set_meta("sidebar_view", value);
                        }
                    }
                });
                ui.add_space(4.0);

                if self.sidebar_view == SidebarView::Connections {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Guardadas").small().color(theme::SUBTEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let add = Button::new(RichText::new("+").size(16.0)).fill(Color32::TRANSPARENT).min_size(Vec2::splat(26.0));
                        if ui.add(add).on_hover_text("Nueva conexión").clicked() {
                            new_connection = true;
                        }
                    });
                });
                ui.add_space(-2.0);

                if self.connections.is_empty() {
                    ui.label(RichText::new("Aún no hay conexiones. Crea una con + o importa las de ~/.ssh/config.").color(theme::SUBTEXT));
                }
                ui.spacing_mut().item_spacing.y = 2.0;
                for connection in &self.connections {
                    let is_open = connection.id.is_some_and(|id| open_ids.contains(&id));
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::SURFACE);
                    }
                    let left = rect.min.x + 10.0;
                    let painter = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)));
                    painter.text(
                        Pos2::new(left, rect.min.y + 9.0),
                        Align2::LEFT_TOP,
                        &connection.name,
                        FontId::new(14.0, strong_family()),
                        theme::TEXT,
                    );
                    painter.text(
                        Pos2::new(left, rect.min.y + 27.0),
                        Align2::LEFT_TOP,
                        format!("{}@{}", connection.user, connection.host),
                        FontId::new(12.0, FontFamily::Proportional),
                        theme::SUBTEXT,
                    );
                    if is_open {
                        ui.painter().circle_filled(Pos2::new(rect.max.x - 14.0, rect.center().y), 3.5, theme::ACCENT);
                    }
                    let response = response.on_hover_text(if is_open { "Sesión abierta. Doble clic para otra pestaña" } else { "Doble clic para conectar" });
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

                    let live = connection
                        .id
                        .and_then(|id| self.tabs.iter().find(|t| t.kind == TabKind::Connection(id)))
                        .and_then(|t| t.tunnels.clone());
                    match live {
                        Some(tunnels) => {
                            for status in tunnels.lock().unwrap().iter() {
                                tunnel_line(ui, status);
                            }
                        }
                        None => {
                            let count = connection.id.and_then(|id| self.forwards.get(&id)).map_or(0, Vec::len);
                            if count > 0 {
                                let text = if count == 1 { "1 túnel".to_string() } else { format!("{count} túneles") };
                                ui.label(RichText::new(text).small().color(theme::SUBTEXT));
                            }
                        }
                    }
                }

                ui.add_space(8.0);
                let import_button = Button::new(RichText::new("Importar de ~/.ssh/config").small().color(theme::SUBTEXT))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, theme::SURFACE));
                if ui.add(import_button).clicked() {
                    import = true;
                }
                if let Some(notice) = &self.notice {
                    ui.label(RichText::new(notice).small().color(theme::SUBTEXT));
                }

                } else {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.snippet_query)
                                .hint_text("Buscar snippets")
                                .desired_width(150.0),
                        );
                        let add = Button::new(RichText::new("+").size(16.0)).fill(Color32::TRANSPARENT).min_size(Vec2::splat(26.0));
                        if ui.add(add).on_hover_text("Nuevo snippet").clicked() {
                            new_snippet = true;
                        }
                    });
                    if ui
                        .add(Button::new(RichText::new("Guardar selección").small().color(theme::SUBTEXT)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, theme::SURFACE)))
                        .clicked()
                    {
                        snippet_from_selection = true;
                    }
                    ui.add_space(4.0);

                    let query = self.snippet_query.clone();
                    let visible: Vec<&Snippet> = self.snippets.iter().filter(|s| s.matches(&query)).collect();
                    if self.snippets.is_empty() {
                        ui.label(RichText::new("Aún no hay snippets. Crea uno con + o guarda la selección del terminal.").color(theme::SUBTEXT));
                    } else if visible.is_empty() {
                        ui.label(RichText::new("Ningún snippet coincide con la búsqueda.").color(theme::SUBTEXT));
                    }
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for snippet in visible {
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::SURFACE);
                        }
                        let left = rect.min.x + 10.0;
                        let painter = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)));
                        painter.text(
                            Pos2::new(left, rect.min.y + 9.0),
                            Align2::LEFT_TOP,
                            &snippet.title,
                            FontId::new(14.0, strong_family()),
                            theme::TEXT,
                        );
                        let preview = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                        let preview: String = preview.chars().take(80).collect();
                        let preview = if snippet.tag.is_empty() { preview } else { format!("{}  ·  {}", snippet.tag, preview) };
                        painter.text(
                            Pos2::new(left, rect.min.y + 27.0),
                            Align2::LEFT_TOP,
                            preview,
                            FontId::new(12.0, FontFamily::Monospace),
                            theme::SUBTEXT,
                        );
                        let response = response.on_hover_text("Clic para insertar en el terminal activo. Clic derecho: más opciones");
                        if response.clicked() {
                            insert = Some((snippet.clone(), false));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Insertar").clicked() {
                                insert = Some((snippet.clone(), false));
                                ui.close();
                            }
                            if ui.button("Insertar y ejecutar").clicked() {
                                insert = Some((snippet.clone(), true));
                                ui.close();
                            }
                            if ui.button("Copiar").clicked() {
                                copy = Some(snippet.body.clone());
                                ui.close();
                            }
                            if ui.button("Editar").clicked() {
                                edit_snippet = Some(snippet.clone());
                                ui.close();
                            }
                            if ui.button("Eliminar").clicked() {
                                delete_snippet = Some(snippet.clone());
                                ui.close();
                            }
                        });
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let mut forced = Secrets::is_master_forced(&self.store.lock().unwrap());
                    if ui.checkbox(&mut forced, RichText::new("Forzar clave maestra").small()).changed() {
                        let result = Secrets::set_master_forced(&self.store.lock().unwrap(), forced);
                        self.notice = Some(match result {
                            Ok(()) => "Reinicia omni-term para aplicar el cambio de almacén".into(),
                            Err(error) => format!("No se pudo guardar el ajuste: {error:#}"),
                        });
                    }
                    ui.label(RichText::new(format!("Secretos: {}", self.secrets.label())).small().color(theme::SUBTEXT));
                });
            });

        if let Some((snippet, run)) = insert {
            self.insert_snippet(&snippet, run);
        }
        if let Some(body) = copy {
            self.ctx.copy_text(body);
        }
        if new_snippet {
            self.dialog = Some(Dialog::Snippet(SnippetForm::new(String::new())));
        }
        if snippet_from_selection {
            let selected = self.tabs.get(self.active).and_then(|t| t.pane.selection_text());
            match selected {
                Some(text) => self.dialog = Some(Dialog::Snippet(SnippetForm::new(text))),
                None => self.notice = Some("Selecciona texto en el terminal para guardarlo como snippet".into()),
            }
        }
        if let Some(snippet) = edit_snippet {
            self.dialog = Some(Dialog::Snippet(SnippetForm::from_snippet(&snippet)));
        }
        if let Some(snippet) = delete_snippet {
            self.dialog = snippet.id.map(|id| Dialog::ConfirmDeleteSnippet { id, title: snippet.title });
        }
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
            self.begin_open(connection, None);
        }
        if let Some(connection) = edit {
            self.dialog = Some(Dialog::Edit(ConnForm::from_connection(&connection, self.forwards.get(&connection.id.unwrap_or_default()).map_or(&[][..], |f| f.as_slice()))));
        }
        if let Some(connection) = delete {
            self.dialog = connection.id.map(|id| Dialog::ConfirmDelete { id, name: connection.name });
        }
    }
}

/// Una línea por túnel: punto de estado, resumen y número de conexiones atendidas.
fn tunnel_line(ui: &mut egui::Ui, status: &ssh::TunnelStatus) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let (color, tip) = match &status.state {
        TunnelState::Starting => (theme::SUBTEXT, "Iniciando".to_string()),
        TunnelState::Active => (theme::ACCENT, "Activo".to_string()),
        TunnelState::Failed(reason) => (theme::RED, reason.clone()),
        TunnelState::Stopped => (theme::SELECTION, "Detenido".to_string()),
    };
    ui.painter().circle_filled(Pos2::new(rect.min.x + 14.0, rect.center().y), 3.0, color);
    ui.painter().text(
        Pos2::new(rect.min.x + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        status.forward.summary(),
        FontId::new(12.0, FontFamily::Proportional),
        theme::SUBTEXT,
    );
    if status.served > 0 {
        ui.painter().text(
            Pos2::new(rect.max.x - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status.served.to_string(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::SUBTEXT,
        );
    }
    response.on_hover_text(tip);
}

fn credentials(kind: SecretKind, secret: Zeroizing<String>) -> Credentials {
    match kind {
        SecretKind::Password => Credentials { password: Some(secret), key_passphrase: None },
        SecretKind::Passphrase => Credentials { password: None, key_passphrase: Some(secret) },
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let load = |name: &str, bytes: &'static [u8], fonts: &mut FontDefinitions| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    load("jb-regular", include_bytes!("../assets/JetBrainsMono-Regular.ttf"), &mut fonts);
    load("jb-bold", include_bytes!("../assets/JetBrainsMono-Bold.ttf"), &mut fonts);
    // La interfaz usa una sans neutra con buena cobertura de acentos; el terminal, JetBrains Mono.
    load("ui-regular", include_bytes!("../assets/NotoSans-Regular.ttf"), &mut fonts);
    load("ui-semibold", include_bytes!("../assets/NotoSans-SemiBold.ttf"), &mut fonts);
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jb-regular".into());
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui-regular".into());
    fonts.families.insert(bold_family(), vec!["jb-bold".into()]);
    fonts.families.insert(strong_family(), vec!["ui-semibold".into(), "ui-regular".into()]);
    ctx.set_fonts(fonts);
}

pub fn strong_family() -> FontFamily {
    FontFamily::Name("ui-strong".into())
}

/// Escala de espaciado (4/8/12/16) y radios (controles 6, diálogos 10) de toda la interfaz.
fn install_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(18.0, strong_family())),
        (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;

    let radius = CornerRadius::same(CONTROL_RADIUS);
    let v = &mut style.visuals;
    v.panel_fill = theme::BASE;
    v.window_fill = theme::MANTLE;
    v.window_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, theme::SURFACE);
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(110) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
    v.menu_corner_radius = CornerRadius::same(8);
    v.override_text_color = Some(theme::TEXT);
    v.extreme_bg_color = theme::BASE;
    v.selection.bg_fill = theme::ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, theme::ACCENT);

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::SURFACE);
    for (widget, fill) in [
        (&mut v.widgets.inactive, theme::SURFACE),
        (&mut v.widgets.hovered, theme::SELECTION),
        (&mut v.widgets.active, theme::SELECTION),
    ] {
        widget.weak_bg_fill = fill;
        widget.bg_fill = fill;
        widget.bg_stroke = Stroke::NONE;
        widget.corner_radius = radius;
    }
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, theme::TEXT);
    ctx.set_global_style(style);
}

/// Pestaña con trazo de acento arriba cuando está activa; el cierre aparece al pasar el ratón.
fn tab(ui: &mut egui::Ui, label: &str, selected: bool, index: usize) -> (bool, bool) {
    let shown = if label.chars().count() > 24 {
        format!("{}…", label.chars().take(23).collect::<String>())
    } else {
        label.to_string()
    };
    let font = FontId::new(14.0, if selected { strong_family() } else { FontFamily::Proportional });
    let galley = ui.painter().layout_no_wrap(shown, font, Color32::WHITE);
    let size = Vec2::new(galley.size().x + 12.0 + 8.0 + 20.0 + 8.0, TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let hovered = response.hovered();
    let fill = if selected {
        theme::BASE
    } else if hovered {
        theme::SURFACE
    } else {
        Color32::TRANSPARENT
    };
    let corner = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
    ui.painter().rect_filled(rect, corner, fill);
    if selected {
        let line = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 2.0));
        ui.painter().rect_filled(line, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, theme::ACCENT);
    }

    let color = if selected { theme::TEXT } else { theme::SUBTEXT };
    ui.painter().galley(Pos2::new(rect.min.x + 12.0, rect.center().y - galley.size().y / 2.0 + 1.0), galley, color);

    let close_rect = Rect::from_center_size(Pos2::new(rect.max.x - 18.0, rect.center().y + 1.0), Vec2::splat(20.0));
    let mut close_clicked = false;
    if selected || hovered {
        let close = ui.interact(close_rect, ui.auto_id_with(("tab-close", index)), Sense::click());
        if close.hovered() {
            ui.painter().rect_filled(close_rect, CornerRadius::same(4), theme::SELECTION);
        }
        ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(15.0), color);
        close_clicked = close.on_hover_text("Cerrar pestaña").clicked();
    }
    (response.clicked() && !close_clicked, close_clicked)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.sidebar(ui);

        let mut activate = None;
        let mut close = None;
        let mut new_local = false;
        Panel::top("tabs")
            .exact_size(TAB_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 0 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, t) in self.tabs.iter().enumerate() {
                        let title = t.pane.title();
                        let label = if title.is_empty() { t.label.clone() } else { title };
                        let (clicked, closed) = tab(ui, &label, i == self.active, i);
                        if clicked {
                            activate = Some(i);
                        }
                        if closed {
                            close = Some(i);
                        }
                    }
                    ui.add_space(6.0);
                    let plus = ui.add(Button::new(RichText::new("+").size(16.0)).fill(Color32::TRANSPARENT).min_size(Vec2::splat(28.0)));
                    if plus.on_hover_text("Nueva pestaña").clicked() {
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
        let text_area = area.shrink(PADDING);

        // Zoom del terminal: Ctrl+= / Ctrl+- / Ctrl+0, reservados y no enviados a la aplicación.
        let zoom = ctx.input_mut(|i| {
            let mut next = self.font_size;
            if i.consume_key(Modifiers::CTRL, Key::Equals)
                || i.consume_key(Modifiers::CTRL, Key::Plus)
                || i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Equals)
            {
                next += 1.0;
            }
            if i.consume_key(Modifiers::CTRL, Key::Minus) {
                next -= 1.0;
            }
            if i.consume_key(Modifiers::CTRL, Key::Num0) {
                next = DEFAULT_FONT_SIZE;
            }
            next
        });
        self.set_font_size(zoom);

        // Con el tamaño real de la zona de terminal, las sesiones nuevas arrancan sin reajuste.
        self.grid = pane::grid_for(ui, text_area, self.font_size);
        let tabs_before = self.tabs.len();
        self.drive_pending();
        if self.tabs.len() != tabs_before {
            ctx.request_repaint();
        }

        let finished_restoring = self.restore_active.is_none();
        if self.tabs.is_empty() && finished_restoring && self.open_queue.is_empty() && self.dialog.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        let mut reconnect = None;
        if !self.tabs.is_empty() {
            let active = self.active.min(self.tabs.len() - 1);
            let input_enabled = self.dialog.is_none() && self.host_keys.is_empty();
            self.tabs[active].pane.show(ui, text_area, input_enabled, self.font_size);

            let title = self.tabs[active].pane.title();
            let title = if title.is_empty() { "omni-term".to_string() } else { title };
            if title != self.window_title {
                ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
                self.window_title = title;
            }

            if self.tabs[active].pane.session.is_disconnected() {
                let mut clicked = false;
                egui::Area::new(egui::Id::NULL.with("reconnect"))
                    .pivot(Align2::CENTER_BOTTOM)
                    .fixed_pos(Pos2::new(area.center().x, area.max.y - 24.0))
                    .show(&ctx, |ui| {
                        Frame::popup(ui.style()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Sesión desconectada");
                                let button = Button::new(RichText::new("Reconectar").color(theme::BASE)).fill(theme::ACCENT);
                                clicked = ui.add(button).clicked();
                            });
                        });
                    });
                let enter = input_enabled && ctx.input(|i| i.key_pressed(Key::Enter));
                if clicked || enter {
                    reconnect = Some(active);
                }
            }
        }
        if let Some(index) = reconnect {
            self.reconnect(index);
        }

        self.handle_dialog(&ctx);
        self.persist_layout();
    }
}
