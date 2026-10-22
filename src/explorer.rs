//! Pestaña de explorador de archivos: carpeta local a la izquierda y SFTP a la derecha.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontFamily, FontId, Id, Pos2, ProgressBar, Rect,
    RichText, ScrollArea, Sense, TextEdit, UiBuilder, Vec2,
};

use crate::cursor::HandExt;
use crate::icons;
use crate::dialogs::{error_dialog, shell, Action, ErrorAction, ErrorInfo, Spec};
use crate::remotefs::{self, RemoteFs};
use crate::sftp::{self, Entry, SortKey};
use crate::ssh::{Credentials, UiEvent};
use crate::store::Connection;
use crate::theme;
use crate::transfer::{self, Conflict, Resolution, Transfer, TransferEvent, TransferState};

const ROW_HEIGHT: f32 = 26.0;
const MIDDLE_WIDTH: f32 = 60.0;
const COLLAPSED_TRANSFERS: f32 = 34.0;
/// Por debajo de este ancho solo se ve un panel a la vez.
const NARROW_WIDTH: f32 = 640.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

enum Link {
    Connecting,
    Ready(Arc<dyn RemoteFs>),
    Failed(String),
}

enum Message {
    Connected { home: String },
    Listed { path: String, result: Result<Vec<Entry>, String> },
    Done { result: Result<(), String> },
}

#[derive(Default)]
struct PaneState {
    path: String,
    path_edit: String,
    editing_path: bool,
    focus_path: bool,
    history: Vec<String>,
    entries: Vec<Entry>,
    selected: BTreeSet<String>,
    anchor: Option<usize>,
    error: Option<String>,
    loading: bool,
    show_hidden: bool,
    sort: Option<(SortKey, bool)>,
}

impl PaneState {
    fn visible(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| self.show_hidden || !e.name.starts_with('.')).collect()
    }

    fn set_path(&mut self, path: String) {
        self.path_edit = path.clone();
        self.path = path;
        self.selected.clear();
        self.anchor = None;
    }

    fn resort(&mut self) {
        let (key, ascending) = self.sort.unwrap_or((SortKey::Name, true));
        sftp::sort_by(&mut self.entries, key, ascending);
    }

    fn selected_entries(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| self.selected.contains(&e.name)).collect()
    }
}

enum PaneAction {
    Open(String),
    Up,
    Go(String),
    Refresh,
    NewFolder,
    Rename,
    Delete,
    Permissions,
    PickFolder,
    Sort(SortKey),
    CopyAcross,
    Back,
    SelectAll,
    EditPath,
    OpenSelected,
    CopyPath,
    Activate,
}

enum Dialog {
    NewFolder { side: Side, name: String },
    Rename { side: Side, old: String, new: String },
    ConfirmDelete { side: Side, names: Vec<String> },
    Permissions { name: String, mode: u32 },
    Conflict { conflict: Conflict, apply_all: bool },
    Message(String),
    Error(ErrorInfo),
}

pub struct Explorer {
    pub connection: Connection,
    credentials: Credentials,
    runtime: tokio::runtime::Handle,
    wake: Arc<dyn Fn() + Send + Sync>,
    ui_events: Sender<UiEvent>,
    known_hosts: PathBuf,
    link: Arc<Mutex<Link>>,
    inbox: Arc<Mutex<VecDeque<Message>>>,
    local: PaneState,
    remote: PaneState,
    saved_remote: Option<String>,
    transfers: Vec<Transfer>,
    transfer_events: Sender<TransferEvent>,
    transfer_inbox: Receiver<TransferEvent>,
    folder_picks: Arc<Mutex<Vec<PathBuf>>>,
    dialog: Option<Dialog>,
    active_side: Side,
    transfers_open: bool,
    /// Panel visible cuando el espacio es poco.
    narrow_side: Side,
}

impl Explorer {
    pub fn new(
        connection: Connection,
        credentials: Credentials,
        runtime: tokio::runtime::Handle,
        wake: Arc<dyn Fn() + Send + Sync>,
        ui_events: Sender<UiEvent>,
        known_hosts: PathBuf,
        paths: (Option<String>, Option<String>),
    ) -> Self {
        let (transfer_events, transfer_inbox) = mpsc::channel();
        let mut explorer = Self {
            connection,
            credentials,
            runtime,
            wake,
            ui_events,
            known_hosts,
            link: Arc::new(Mutex::new(Link::Connecting)),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            local: PaneState::default(),
            remote: PaneState::default(),
            saved_remote: paths.1,
            transfers: Vec::new(),
            transfer_events,
            transfer_inbox,
            folder_picks: Arc::new(Mutex::new(Vec::new())),
            dialog: None,
            active_side: Side::Local,
            transfers_open: true,
            narrow_side: Side::Local,
        };
        let start = paths
            .0
            .filter(|p| Path::new(p).is_dir())
            .or_else(|| std::env::home_dir().map(|h| h.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".into());
        explorer.local.set_path(start);
        explorer.refresh_local();
        explorer.connect();
        explorer
    }

    /// Rutas actuales (local y remota) para restaurar la pestaña.
    pub fn paths(&self) -> (String, String) {
        (self.local.path.clone(), self.remote.path.clone())
    }

    pub fn running_transfers(&self) -> usize {
        self.transfers.iter().filter(|t| t.state() == TransferState::Running).count()
    }

    pub fn label(&self) -> String {
        format!("Archivos · {}", self.connection.name)
    }

    fn connect(&mut self) {
        if self.saved_remote.is_none() && !self.remote.path.is_empty() {
            self.saved_remote = Some(self.remote.path.clone());
        }
        *self.link.lock().unwrap() = Link::Connecting;
        let (connection, credentials) = (self.connection.clone(), self.credentials.clone());
        let (events, wake, known_hosts) = (self.ui_events.clone(), self.wake.clone(), self.known_hosts.clone());
        let (link, inbox) = (self.link.clone(), self.inbox.clone());
        self.runtime.spawn(async move {
            match remotefs::connect(&connection, &credentials, events, wake.clone(), known_hosts).await {
                Ok(client) => {
                    let home = client.home().await;
                    *link.lock().unwrap() = Link::Ready(client);
                    inbox.lock().unwrap().push_back(Message::Connected { home });
                }
                Err(error) => *link.lock().unwrap() = Link::Failed(format!("No se pudo conectar: {error:#}")),
            }
            wake();
        });
    }

    fn session(&self) -> Option<Arc<dyn RemoteFs>> {
        match &*self.link.lock().unwrap() {
            Link::Ready(client) => Some(client.clone()),
            _ => None,
        }
    }

    fn refresh_local(&mut self) {
        match list_local(Path::new(&self.local.path)) {
            Ok(entries) => {
                self.local.entries = entries;
                self.local.resort();
                self.local.error = None;
            }
            Err(error) => {
                self.local.entries.clear();
                self.local.error = Some(error.to_string());
            }
        }
        let names: BTreeSet<_> = self.local.entries.iter().map(|e| e.name.clone()).collect();
        self.local.selected.retain(|n| names.contains(n));
    }

    fn refresh_remote(&mut self) {
        let Some(session) = self.session() else { return };
        self.remote.loading = true;
        let (path, inbox, wake) = (self.remote.path.clone(), self.inbox.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let result = session.list(&path).await.map_err(|e| format!("{e:#}"));
            inbox.lock().unwrap().push_back(Message::Listed { path, result });
            wake();
        });
    }

    fn remote_task<F, Fut>(&mut self, work: F)
    where
        F: FnOnce(Arc<dyn RemoteFs>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let Some(session) = self.session() else { return };
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let result = work(session).await.map_err(|e| format!("{e:#}"));
            inbox.lock().unwrap().push_back(Message::Done { result });
            wake();
        });
    }

    fn navigate(&mut self, side: Side, path: String) {
        let state = if side == Side::Local { &mut self.local } else { &mut self.remote };
        if path != state.path && !state.path.is_empty() {
            state.history.push(state.path.clone());
        }
        self.go(side, path);
    }

    fn go(&mut self, side: Side, path: String) {
        match side {
            Side::Local => {
                self.local.set_path(path);
                self.refresh_local();
            }
            Side::Remote => {
                self.remote.set_path(path);
                self.refresh_remote();
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let messages: Vec<Message> = self.inbox.lock().unwrap().drain(..).collect();
        for message in messages {
            match message {
                Message::Connected { home } => {
                    let start = self.saved_remote.take().unwrap_or(home);
                    self.remote.set_path(start);
                    self.refresh_remote();
                }
                Message::Listed { path, result } => {
                    if path == self.remote.path {
                        self.remote.loading = false;
                        match result {
                            Ok(entries) => {
                                self.remote.entries = entries;
                                self.remote.resort();
                                self.remote.error = None;
                                let names: BTreeSet<_> = self.remote.entries.iter().map(|e| e.name.clone()).collect();
                                self.remote.selected.retain(|n| names.contains(n));
                            }
                            Err(error) => {
                                self.remote.entries.clear();
                                self.remote.error = Some(error);
                            }
                        }
                    }
                }
                Message::Done { result } => {
                    if let Err(error) = result {
                        self.dialog = Some(Dialog::Error(ErrorInfo { title: "No se pudo completar la operación".into(), detail: error, retry: None }));
                    }
                    self.refresh_remote();
                }
            }
        }

        while let Ok(TransferEvent::Conflict(conflict)) = self.transfer_inbox.try_recv() {
            self.dialog = Some(Dialog::Conflict { conflict, apply_all: false });
        }

        let picks: Vec<PathBuf> = self.folder_picks.lock().unwrap().drain(..).collect();
        if let Some(path) = picks.into_iter().last() {
            self.navigate(Side::Local, path.to_string_lossy().into_owned());
        }

        let finished_now = self.transfers.iter().any(|t| t.state() != TransferState::Running);
        if self.transfers.iter().any(|t| t.state() == TransferState::Running) {
            ctx.request_repaint_after(Duration::from_millis(150));
        } else if finished_now {
            // Tras terminar, se refresca el panel de destino una sola vez.
            self.refresh_after_transfers();
        }
    }

    fn refresh_after_transfers(&mut self) {
        let (uploaded, downloaded) = (
            self.transfers.iter().any(|t| t.upload && t.state() == TransferState::Done),
            self.transfers.iter().any(|t| !t.upload && t.state() == TransferState::Done),
        );
        self.transfers.retain(|t| !matches!(t.state(), TransferState::Done));
        if uploaded {
            self.refresh_remote();
        }
        if downloaded {
            self.refresh_local();
        }
    }

    fn upload_selected(&mut self) {
        let Some(session) = self.session() else { return };
        let sources: Vec<PathBuf> =
            self.local.selected_entries().iter().map(|e| Path::new(&self.local.path).join(&e.name)).collect();
        if sources.is_empty() || self.remote.path.is_empty() {
            return;
        }
        self.start_upload(session, sources);
    }

    fn start_upload(&mut self, session: Arc<dyn RemoteFs>, sources: Vec<PathBuf>) {
        let transfer = transfer::upload(
            &self.runtime,
            session,
            sources,
            self.remote.path.clone(),
            self.transfer_events.clone(),
            self.wake.clone(),
        );
        self.transfers.push(transfer);
        self.transfers_open = true;
    }

    fn download_selected(&mut self) {
        let Some(session) = self.session() else { return };
        let sources: Vec<(String, bool)> = self
            .remote
            .selected_entries()
            .iter()
            .map(|e| (sftp::join(&self.remote.path, &e.name), e.is_dir))
            .collect();
        if sources.is_empty() {
            return;
        }
        let transfer = transfer::download(
            &self.runtime,
            session,
            sources,
            PathBuf::from(&self.local.path),
            self.transfer_events.clone(),
            self.wake.clone(),
        );
        self.transfers.push(transfer);
        self.transfers_open = true;
    }

    fn copy_from(&mut self, source: Side) {
        match source {
            Side::Local => self.upload_selected(),
            Side::Remote => self.download_selected(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, area: Rect) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        // Sin eventos de entrada no hay fotogramas: se revisa cada segundo si la conexión sigue viva.
        ctx.request_repaint_after(Duration::from_secs(1));
        {
            let mut link = self.link.lock().unwrap();
            if matches!(&*link, Link::Ready(client) if client.is_closed()) {
                *link = Link::Failed("Se perdió la conexión con el servidor.".into());
            }
        }

        let failure = match &*self.link.lock().unwrap() {
            Link::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let connecting = matches!(&*self.link.lock().unwrap(), Link::Connecting);

        let transfers_height = if self.transfers.is_empty() {
            0.0
        } else if !self.transfers_open {
            COLLAPSED_TRANSFERS
        } else {
            (self.transfers.len() as f32 * 30.0 + 40.0).min(190.0)
        };
        let narrow = area.width() < NARROW_WIDTH;
        let mut panes_area = Rect::from_min_max(area.min, Pos2::new(area.max.x, area.max.y - transfers_height));
        let (local_rect, middle_rect, remote_rect);
        if narrow {
            let bar = Rect::from_min_size(panes_area.min, Vec2::new(panes_area.width(), 34.0));
            panes_area.min.y += 38.0;
            self.narrow_selector(ui, bar);
            local_rect = panes_area;
            remote_rect = panes_area;
            middle_rect = Rect::from_min_size(panes_area.min, Vec2::ZERO);
        } else {
            let pane_width = ((panes_area.width() - MIDDLE_WIDTH) / 2.0).max(120.0);
            local_rect = Rect::from_min_size(panes_area.min, Vec2::new(pane_width, panes_area.height()));
            middle_rect = Rect::from_min_size(
                Pos2::new(local_rect.max.x, panes_area.min.y),
                Vec2::new(MIDDLE_WIDTH, panes_area.height()),
            );
            remote_rect = Rect::from_min_max(Pos2::new(middle_rect.max.x, panes_area.min.y), panes_area.max);
        }

        let mut actions: Vec<(Side, PaneAction)> = Vec::new();
        let mut dropped_on: Option<(Side, Side)> = None;

        for (side, rect) in [(Side::Local, local_rect), (Side::Remote, remote_rect)] {
            if narrow && side != self.narrow_side {
                continue;
            }
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(Vec2::new(4.0, 0.0))));
            let title = match side {
                _ if narrow => String::new(),
                Side::Local => "Este equipo".to_string(),
                Side::Remote => self.connection.name.clone(),
            };
            let can_delete = side == Side::Remote;
            let can_chmod = side == Side::Remote && self.session().is_some_and(|fs| fs.supports_permissions());
            let (state, status) = match side {
                Side::Local => (&mut self.local, None),
                Side::Remote => {
                    let status = if let Some(reason) = &failure {
                        Some(reason.clone())
                    } else if connecting {
                        Some("Conectando…".to_string())
                    } else {
                        None
                    };
                    (&mut self.remote, status)
                }
            };
            let pane_actions = pane_ui(&mut child, side, &title, state, status.as_deref(), can_delete, can_chmod);
            actions.extend(pane_actions.into_iter().map(|a| (side, a)));

            let zone = ui.interact(rect, Id::NULL.with(("explorer-drop", matches!(side, Side::Remote))), Sense::hover());
            if let Some(payload) = zone.dnd_hover_payload::<Side>() {
                if *payload != side {
                    ui.painter().rect_stroke(
                        rect.shrink(2.0),
                        CornerRadius::same(theme::radius::CONTROL),
                        egui::Stroke::new(theme::stroke::THICK, theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            if let Some(payload) = zone.dnd_release_payload::<Side>() {
                if *payload != side {
                    dropped_on = Some((*payload, side));
                }
            }
        }

        if failure.is_some() && (!narrow || self.narrow_side == Side::Remote) {
            let mut retry = false;
            egui::Area::new(Id::NULL.with("explorer-retry"))
                .pivot(Align2::CENTER_CENTER)
                .fixed_pos(Pos2::new(remote_rect.center().x, remote_rect.center().y))
                .show(&ctx, |ui| {
                    retry = crate::ui_kit::button(ui, "Reintentar", crate::ui_kit::Variant::Primary).clicked();
                });
            if retry {
                self.connect();
            }
        }

        if !narrow {
            let mut middle = ui.new_child(UiBuilder::new().max_rect(middle_rect.shrink(6.0)));
            middle.add_space((middle_rect.height() / 2.0 - 56.0).max(40.0));
            middle.spacing_mut().item_spacing.y = 8.0;
            for (glyph, tip, from) in [
                (icons::ARROW_RIGHT, "Subir: copia la selección local al directorio remoto actual", Side::Local),
                (icons::ARROW_LEFT, "Bajar: copia la selección remota al directorio local actual", Side::Remote),
            ] {
                let clicked = middle.vertical_centered(|ui| crate::ui_kit::IconButton::new(glyph).size(theme::size::ICON_LG).show(ui).on_hover_text(tip).clicked()).inner;
                if clicked {
                    self.copy_from(from);
                }
            }
        }

        if let Some((from, _to)) = dropped_on {
            self.copy_from(from);
        }

        // Archivos soltados desde el gestor del sistema sobre el panel remoto: se suben.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            let over_remote = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| remote_rect.contains(p));
            if over_remote {
                if let Some(session) = self.session() {
                    self.start_upload(session, dropped);
                }
            }
        }

        if self.dialog.is_none() && !ctx.egui_wants_keyboard_input() {
            let side = self.active_side;
            let keys = ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                let mut found = Vec::new();
                if i.consume_key(Modifiers::NONE, Key::F2) {
                    found.push(PaneAction::Rename);
                }
                if i.consume_key(Modifiers::NONE, Key::Delete) {
                    found.push(PaneAction::Delete);
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    found.push(PaneAction::OpenSelected);
                }
                if i.consume_key(Modifiers::NONE, Key::Backspace) {
                    found.push(PaneAction::Up);
                }
                if i.consume_key(Modifiers::NONE, Key::F5) {
                    found.push(PaneAction::Refresh);
                }
                if i.consume_key(Modifiers::CTRL, Key::A) {
                    found.push(PaneAction::SelectAll);
                }
                if i.consume_key(Modifiers::CTRL, Key::L) {
                    found.push(PaneAction::EditPath);
                }
                found
            });
            actions.extend(keys.into_iter().map(|a| (side, a)));
        }

        for (side, action) in actions {
            self.apply(&ctx, side, action);
        }

        if transfers_height > 0.0 {
            let rect = Rect::from_min_max(Pos2::new(area.min.x, area.max.y - transfers_height), area.max);
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(Vec2::new(4.0, 6.0))));
            self.transfers_ui(&mut child);
        }

        self.dialog_ui(&ctx);
    }

    /// Selector Local/Remoto de la vista estrecha, con el botón de copiar hacia el otro lado.
    fn narrow_selector(&mut self, ui: &mut egui::Ui, bar: Rect) {
        let mut child = ui.new_child(UiBuilder::new().max_rect(bar.shrink2(Vec2::new(4.0, 0.0))));
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.spacing_mut().button_padding.x = 0.0;
            for (side, label) in [(Side::Local, "Este equipo".to_string()), (Side::Remote, self.connection.name.clone())] {
                let selected = self.narrow_side == side;
                let color = if selected { theme::text() } else { theme::subtext() };
                let font = FontId::new(theme::size::BODY, if selected { crate::ui::strong_family() } else { FontFamily::Proportional });
                let response = crate::ui_kit::button_with(ui, Button::new(RichText::new(label).font(font).color(color)), crate::ui_kit::Variant::Ghost);
                if selected {
                    let y = response.rect.max.y + 1.0;
                    ui.painter().line_segment([Pos2::new(response.rect.min.x, y), Pos2::new(response.rect.max.x, y)], egui::Stroke::new(theme::stroke::THICK, theme::accent()));
                }
                if response.clicked() {
                    self.narrow_side = side;
                    self.active_side = side;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (glyph, tip, from) = match self.narrow_side {
                    Side::Local => (icons::ARROW_RIGHT, "Subir la selección al directorio remoto actual", Side::Local),
                    Side::Remote => (icons::ARROW_LEFT, "Bajar la selección al directorio local actual", Side::Remote),
                };
                if tool_button(ui, glyph, tip, true, false) {
                    self.copy_from(from);
                }
            });
        });
    }

    fn apply(&mut self, ctx: &egui::Context, side: Side, action: PaneAction) {
        let state = match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        };
        match action {
            PaneAction::Open(name) => {
                let path = match side {
                    Side::Local => Path::new(&state.path).join(&name).to_string_lossy().into_owned(),
                    Side::Remote => sftp::join(&state.path, &name),
                };
                self.navigate(side, path);
            }
            PaneAction::Up => {
                let path = match side {
                    Side::Local => Path::new(&state.path)
                        .parent()
                        .map_or(state.path.clone(), |p| p.to_string_lossy().into_owned()),
                    Side::Remote => sftp::parent(&state.path),
                };
                self.navigate(side, path);
            }
            PaneAction::Go(path) => self.navigate(side, path),
            PaneAction::Back => {
                let pane = if side == Side::Local { &mut self.local } else { &mut self.remote };
                if let Some(previous) = pane.history.pop() {
                    self.go(side, previous);
                }
            }
            PaneAction::SelectAll => {
                let pane = if side == Side::Local { &mut self.local } else { &mut self.remote };
                pane.selected = pane.visible().iter().map(|e| e.name.clone()).collect();
            }
            PaneAction::EditPath => {
                let pane = if side == Side::Local { &mut self.local } else { &mut self.remote };
                pane.path_edit = pane.path.clone();
                pane.editing_path = true;
                pane.focus_path = true;
            }
            PaneAction::OpenSelected => {
                if let [entry] = state.selected_entries().as_slice() {
                    if entry.is_dir {
                        let name = entry.name.clone();
                        self.apply(ctx, side, PaneAction::Open(name));
                    }
                }
            }
            PaneAction::CopyPath => {
                let paths: Vec<String> = state
                    .selected_entries()
                    .iter()
                    .map(|e| match side {
                        Side::Local => Path::new(&state.path).join(&e.name).to_string_lossy().into_owned(),
                        Side::Remote => sftp::join(&state.path, &e.name),
                    })
                    .collect();
                if !paths.is_empty() {
                    ctx.copy_text(paths.join("\n"));
                }
            }
            PaneAction::Activate => self.active_side = side,
            PaneAction::Refresh => match side {
                Side::Local => self.refresh_local(),
                Side::Remote => self.refresh_remote(),
            },
            PaneAction::NewFolder => self.dialog = Some(Dialog::NewFolder { side, name: String::new() }),
            PaneAction::Rename => {
                if let Some(entry) = state.selected_entries().first() {
                    self.dialog = Some(Dialog::Rename { side, old: entry.name.clone(), new: entry.name.clone() });
                }
            }
            PaneAction::Delete => {
                let names: Vec<String> = state.selected.iter().cloned().collect();
                if !names.is_empty() {
                    self.dialog = Some(Dialog::ConfirmDelete { side, names });
                }
            }
            PaneAction::Permissions => {
                if let Some(entry) = state.selected_entries().first() {
                    self.dialog =
                        Some(Dialog::Permissions { name: entry.name.clone(), mode: entry.permissions.unwrap_or(0o644) });
                }
            }
            PaneAction::Sort(key) => {
                let pane = match side {
                    Side::Local => &mut self.local,
                    Side::Remote => &mut self.remote,
                };
                pane.sort = match pane.sort {
                    Some((current, ascending)) if current == key => Some((key, !ascending)),
                    _ => Some((key, true)),
                };
                pane.resort();
            }
            PaneAction::CopyAcross => self.copy_from(side),
            PaneAction::PickFolder => {
                let picks = self.folder_picks.clone();
                let (wake, start) = (self.wake.clone(), self.local.path.clone());
                self.runtime.spawn_blocking(move || {
                    if let Some(folder) = rfd::FileDialog::new().set_directory(start).pick_folder() {
                        picks.lock().unwrap().push(folder);
                        wake();
                    }
                });
            }
        }
    }

    fn transfers_ui(&mut self, ui: &mut egui::Ui) {
        let running = self.transfers.iter().filter(|t| t.state() == TransferState::Running).count();
        ui.horizontal(|ui| {
            let caret = if self.transfers_open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
            let title = if running > 0 { format!("{caret}  Transferencias · {running} en curso") } else { format!("{caret}  Transferencias") };
            let header = crate::ui_kit::button_with(ui, Button::new(RichText::new(title).small().color(theme::subtext())), crate::ui_kit::Variant::Ghost);
            if header.on_hover_text("Mostrar u ocultar la lista").hand().clicked() {
                self.transfers_open = !self.transfers_open;
            }
            if crate::ui_kit::button_with(ui, Button::new(RichText::new("Quitar terminadas").small()), crate::ui_kit::Variant::Ghost).clicked() {
                self.transfers.retain(|t| t.state() == TransferState::Running);
            }
        });
        if !self.transfers_open {
            return;
        }
        ScrollArea::vertical().id_salt("explorer-transfers").show(ui, |ui| {
            for transfer in &self.transfers {
                ui.horizontal(|ui| {
                    let arrow = if transfer.upload { icons::ARROW_RIGHT } else { icons::ARROW_LEFT };
                    ui.add_sized([200.0, 22.0], egui::Label::new(format!("{arrow}  {}", transfer.label)).truncate());
                    let (done, total) = transfer.progress();
                    let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                    let state = transfer.state();
                    let text = match &state {
                        TransferState::Running => match transfer.rate() {
                            Some((speed, left)) => format!("{} / {}  ·  {}/s  ·  {}", human_size(done), human_size(total), human_size(speed as u64), format_duration(left)),
                            None => format!("{} / {}", human_size(done), human_size(total)),
                        },
                        TransferState::Done => "Completada".to_string(),
                        TransferState::Cancelled => "Cancelada".to_string(),
                        TransferState::Failed(reason) => format!("Error: {reason}"),
                    };
                    ui.add(ProgressBar::new(if state == TransferState::Done { 1.0 } else { fraction }).desired_width(180.0));
                    ui.label(RichText::new(text).small().color(theme::subtext()));
                    if state == TransferState::Running && crate::ui_kit::button_with(ui, Button::new(icons::X), crate::ui_kit::Variant::Ghost).on_hover_text("Cancelar").clicked() {
                        transfer.cancel();
                    }
                });
            }
        });
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        enum Outcome {
            None,
            Accept,
            Cancel,
            Resolve(Resolution, bool),
        }
        let accept_or_cancel = |action: Action| match action {
            Action::Accept => Outcome::Accept,
            Action::Cancel => Outcome::Cancel,
            _ => Outcome::None,
        };
        let outcome = match &mut dialog {
            Dialog::NewFolder { name, .. } => accept_or_cancel(shell(ctx, Spec::new("Nueva carpeta").width(380.0).accept("Crear"), |ui| {
                let field = ui.add(TextEdit::singleline(name).desired_width(f32::INFINITY).hint_text("nombre"));
                field.request_focus();
                field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })),
            Dialog::Rename { new, .. } => accept_or_cancel(shell(ctx, Spec::new("Renombrar").width(380.0).accept("Renombrar"), |ui| {
                let field = ui.add(TextEdit::singleline(new).desired_width(f32::INFINITY));
                field.request_focus();
                field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })),
            Dialog::ConfirmDelete { side, names } => {
                let local = *side == Side::Local;
                let what = if names.len() == 1 { format!("\"{}\"", names[0]) } else { format!("{} elementos", names.len()) };
                let spec = Spec::new(if local { "Mover a la papelera" } else { "Eliminar en el servidor" })
                    .width(380.0)
                    .accept(if local { "Mover" } else { "Eliminar" });
                let spec = if local { spec } else { spec.danger() };
                accept_or_cancel(shell(ctx, spec, |ui| {
                    ui.label(if local {
                        format!("¿Mover {what} a la papelera?")
                    } else {
                        format!("¿Eliminar {what}? Las carpetas se borran con todo su contenido y no se puede deshacer.")
                    });
                    false
                }))
            }
            Dialog::Permissions { name, mode } => {
                let spec = Spec::new("Permisos").width(380.0).context(name.as_str()).accept("Aplicar");
                accept_or_cancel(shell(ctx, spec, |ui| {
                    egui::Grid::new("permission-grid").num_columns(4).spacing([18.0, 6.0]).show(ui, |ui| {
                        ui.label("");
                        for header in ["Propietario", "Grupo", "Otros"] {
                            ui.label(RichText::new(header).small().color(theme::subtext()));
                        }
                        ui.end_row();
                        for (label, bit) in [("Leer", 0o4), ("Escribir", 0o2), ("Ejecutar", 0o1)] {
                            ui.label(label);
                            for shift in [6, 3, 0] {
                                let mask = bit << shift;
                                let mut on = *mode & mask != 0;
                                if crate::ui_kit::switch(ui, &mut on).changed() {
                                    *mode = if on { *mode | mask } else { *mode & !mask };
                                }
                            }
                            ui.end_row();
                        }
                    });
                    ui.label(
                        RichText::new(format!("{:04o}  {}", *mode & 0o7777, sftp::format_mode(*mode)))
                            .monospace()
                            .color(theme::subtext()),
                    );
                    false
                }))
            }
            Dialog::Conflict { conflict, apply_all } => {
                // Reemplazar es la acción destructiva: botón rojo y nunca con Enter.
                let spec = Spec::new("El destino ya existe")
                    .width(380.0)
                    .accept("Reemplazar")
                    .danger()
                    .extra("Omitir")
                    .extra("Renombrar");
                let name = conflict.name.clone();
                match shell(ctx, spec, |ui| {
                    ui.label(format!("\"{name}\" ya existe. ¿Qué hacemos?"));
                    crate::ui_kit::switch_labeled(ui, apply_all, "Aplicar a todos los conflictos de esta transferencia");
                    false
                }) {
                    Action::Accept => Outcome::Resolve(Resolution::Replace, *apply_all),
                    Action::Extra(0) => Outcome::Resolve(Resolution::Skip, *apply_all),
                    Action::Extra(_) => Outcome::Resolve(Resolution::Rename, *apply_all),
                    Action::Cancel => Outcome::Cancel,
                    Action::None => Outcome::None,
                }
            }
            Dialog::Error(info) => match error_dialog(ctx, info) {
                ErrorAction::Close => Outcome::Cancel,
                _ => Outcome::None,
            },
            Dialog::Message(text) => accept_or_cancel(shell(
                ctx,
                Spec::new("Aviso").width(380.0).cancel("Aceptar").backdrop_closes(),
                |ui| {
                    ui.label(text.as_str());
                    false
                },
            )),
        };

        match (outcome, dialog) {
            (Outcome::None, dialog) => self.dialog = Some(dialog),
            (Outcome::Resolve(resolution, all), Dialog::Conflict { conflict, .. }) => {
                let _ = conflict.reply.send((resolution, all));
            }
            (Outcome::Cancel, Dialog::Conflict { conflict, .. }) => {
                let _ = conflict.reply.send((Resolution::Skip, false));
            }
            (Outcome::Accept, Dialog::NewFolder { side, name }) => match valid_name(&name) {
                None => self.dialog = Some(Dialog::Message("El nombre no puede estar vacío ni contener /".into())),
                Some(name) => match side {
                    Side::Local => {
                        if let Err(error) = std::fs::create_dir(Path::new(&self.local.path).join(&name)) {
                            self.dialog = Some(Dialog::Error(ErrorInfo { title: "No se pudo crear la carpeta".into(), detail: error.to_string(), retry: None }));
                        }
                        self.refresh_local();
                    }
                    Side::Remote => {
                        let path = sftp::join(&self.remote.path, &name);
                        self.remote_task(move |session| async move { session.make_dir(&path).await });
                    }
                },
            },
            (Outcome::Accept, Dialog::Rename { side, old, new }) => match valid_name(&new) {
                None => self.dialog = Some(Dialog::Message("El nombre no puede estar vacío ni contener /".into())),
                Some(_) if new == old => {}
                Some(new) => match side {
                    Side::Local => {
                        let base = PathBuf::from(&self.local.path);
                        if let Err(error) = std::fs::rename(base.join(&old), base.join(&new)) {
                            self.dialog = Some(Dialog::Error(ErrorInfo { title: "No se pudo renombrar".into(), detail: error.to_string(), retry: None }));
                        }
                        self.refresh_local();
                    }
                    Side::Remote => {
                        let (from, to) = (sftp::join(&self.remote.path, &old), sftp::join(&self.remote.path, &new));
                        self.remote_task(move |session| async move { session.rename(&from, &to).await });
                    }
                },
            },
            (Outcome::Accept, Dialog::ConfirmDelete { side: Side::Remote, names }) => {
                let paths: Vec<String> = names.iter().map(|n| sftp::join(&self.remote.path, n)).collect();
                self.remote_task(move |session| async move {
                    for path in paths {
                        session.remove(&path).await?;
                    }
                    Ok(())
                });
            }
            (Outcome::Accept, Dialog::ConfirmDelete { side: Side::Local, names }) => {
                let base = PathBuf::from(&self.local.path);
                let paths: Vec<PathBuf> = names.iter().map(|n| base.join(n)).collect();
                if let Err(error) = trash::delete_all(paths) {
                    self.dialog = Some(Dialog::Error(ErrorInfo { title: "No se pudo mover a la papelera".into(), detail: error.to_string(), retry: None }));
                }
                self.refresh_local();
            }
            (Outcome::Accept, Dialog::Permissions { name, mode }) => {
                let path = sftp::join(&self.remote.path, &name);
                self.remote_task(move |session| async move { session.set_permissions(&path, mode).await });
            }
            _ => {}
        }
    }
}

/// Un nombre de archivo válido: sin vacíos ni separadores de ruta.
fn valid_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != "..").then(|| name.to_string())
}

fn list_local(path: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(path)? {
        let Ok(item) = item else { continue };
        let Ok(metadata) = std::fs::metadata(item.path()) else { continue };
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            permissions: None,
        });
    }
    sftp::sort_entries(&mut entries);
    Ok(entries)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn format_time(seconds: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(seconds as i64, 0)
        .single()
        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string())
}

/// Botón de la barra de herramientas con su explicación; devuelve `true` al pulsarlo (si está habilitado).
fn tool_button(ui: &mut egui::Ui, glyph: &'static str, tip: &str, enabled: bool, active: bool) -> bool {
    crate::ui_kit::IconButton::new(glyph).enabled(enabled).active(active).show(ui).on_hover_text(tip).clicked()
}

/// Tramos de una ruta con la ruta completa de cada uno.
fn segments(side: Side, path: &str) -> Vec<(String, String)> {
    match side {
        Side::Local => {
            let mut accumulated = PathBuf::new();
            let mut out = Vec::new();
            for component in Path::new(path).components() {
                accumulated.push(component);
                let label = match component {
                    std::path::Component::RootDir => "/".to_string(),
                    other => other.as_os_str().to_string_lossy().into_owned(),
                };
                out.push((label, accumulated.to_string_lossy().into_owned()));
            }
            out
        }
        Side::Remote => {
            let mut out = vec![("/".to_string(), "/".to_string())];
            let mut accumulated = String::new();
            for part in path.split('/').filter(|p| !p.is_empty()) {
                accumulated.push('/');
                accumulated.push_str(part);
                out.push((part.to_string(), accumulated.clone()));
            }
            out
        }
    }
}

/// Barra de ruta: tramos clicables; un doble clic (o `Ctrl+L`) la convierte en campo de texto.
fn breadcrumb(ui: &mut egui::Ui, side: Side, state: &mut PaneState, actions: &mut Vec<PaneAction>) {
    let (bar, background) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    ui.painter().rect_filled(bar, CornerRadius::same(theme::radius::CONTROL), theme::mantle());

    if state.editing_path {
        let mut child = ui.new_child(UiBuilder::new().max_rect(bar.shrink2(Vec2::new(3.0, 2.0))));
        let field = child.add(TextEdit::singleline(&mut state.path_edit).desired_width(f32::INFINITY).frame(egui::Frame::NONE).align(Align2::LEFT_CENTER));
        if std::mem::take(&mut state.focus_path) {
            field.request_focus();
        }
        let escape = child.input(|i| i.key_pressed(egui::Key::Escape));
        if field.lost_focus() && child.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.push(PaneAction::Go(state.path_edit.trim().to_string()));
            state.editing_path = false;
        } else if escape || field.lost_focus() {
            state.path_edit = state.path.clone();
            state.editing_path = false;
        }
        return;
    }

    if background.on_hover_text("Doble clic para escribir una ruta (Ctrl+L)").hand().double_clicked() {
        actions.push(PaneAction::EditPath);
    }

    let crumbs = segments(side, &state.path);
    let font = FontId::new(theme::size::LABEL, FontFamily::Proportional);
    let width_of = |ui: &egui::Ui, text: &str| ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::PLACEHOLDER).size().x + 14.0;
    let widths: Vec<f32> = crumbs.iter().map(|(label, _)| width_of(ui, label)).collect();
    let separator = 14.0;
    // Si no cabe todo, se empieza por el final y los tramos anteriores quedan tras «…».
    let budget = bar.width() - 16.0 - 30.0;
    let mut start = crumbs.len();
    let mut used = 0.0;
    while start > 0 {
        let next = widths[start - 1] + if start < crumbs.len() { separator } else { 0.0 };
        if used + next > budget && start < crumbs.len() {
            break;
        }
        used += next;
        start -= 1;
    }
    let mut x = bar.min.x + 8.0;
    let y = bar.center().y;
    let mut draw = |ui: &mut egui::Ui, x: f32, index: usize, label: &str, target: String, width: f32, last: bool| -> f32 {
        let rect = Rect::from_min_size(Pos2::new(x, bar.min.y + 3.0), Vec2::new(width, bar.height() - 6.0));
        let id = ui.scope_id().with(("crumb", side == Side::Remote, index));
        let response = ui.interact(rect, id, Sense::click()).hand();
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::SMALL), theme::surface());
        }
        let color = if last || response.hovered() { theme::text() } else { theme::subtext() };
        let family = if last { crate::ui::strong_family() } else { FontFamily::Proportional };
        ui.painter().text(Pos2::new(rect.center().x, y), Align2::CENTER_CENTER, label, FontId::new(theme::size::LABEL, family), color);
        if response.clicked() {
            actions.push(PaneAction::Go(target));
        }
        x + width
    };
    if start > 0 {
        let parent = crumbs[start - 1].1.clone();
        x = draw(ui, x, usize::MAX, "…", parent, 26.0, false);
    }
    for index in start..crumbs.len() {
        if index > start || start > 0 {
            ui.painter().text(Pos2::new(x + separator / 2.0, y), Align2::CENTER_CENTER, icons::CARET_RIGHT, FontId::proportional(theme::size::CAPTION), theme::subtext());
            x += separator;
        }
        let (label, target) = &crumbs[index];
        x = draw(ui, x, index, label, target.clone(), widths[index], index == crumbs.len() - 1);
    }
}

fn pane_ui(
    ui: &mut egui::Ui,
    side: Side,
    title: &str,
    state: &mut PaneState,
    status: Option<&str>,
    can_delete: bool,
    can_chmod: bool,
) -> Vec<PaneAction> {
    let show_permissions = side == Side::Remote && ui.available_width() > 440.0;
    let mut actions = Vec::new();
    if !title.is_empty() {
        ui.label(RichText::new(title).font(FontId::new(theme::size::BODY, crate::ui::strong_family())));
        ui.add_space(theme::space::XXS);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let one = state.selected.len() == 1;
        if tool_button(ui, icons::ARROW_LEFT, "Atrás", !state.history.is_empty(), false) {
            actions.push(PaneAction::Back);
        }
        if tool_button(ui, icons::ARROW_UP, "Subir un nivel (Retroceso)", true, false) {
            actions.push(PaneAction::Up);
        }
        if tool_button(ui, icons::REFRESH, "Actualizar (F5)", true, false) {
            actions.push(PaneAction::Refresh);
        }
        ui.add_space(theme::space::M);
        if tool_button(ui, icons::FOLDER_PLUS, "Nueva carpeta", true, false) {
            actions.push(PaneAction::NewFolder);
        }
        if tool_button(ui, icons::PENCIL, "Renombrar (F2)", one, false) {
            actions.push(PaneAction::Rename);
        }
        if side == Side::Remote && tool_button(ui, icons::LOCK, if can_chmod { "Permisos" } else { "Este protocolo no admite permisos" }, one && can_chmod, false) {
            actions.push(PaneAction::Permissions);
        }
        let delete_tip = if can_delete { "Eliminar (Supr)" } else { "Mover a la papelera (Supr)" };
        if tool_button(ui, icons::TRASH, delete_tip, !state.selected.is_empty(), false) {
            actions.push(PaneAction::Delete);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let glyph = if state.show_hidden { icons::EYE } else { icons::EYE_OFF };
            let tip = if state.show_hidden { "Ocultar los archivos ocultos" } else { "Mostrar los archivos ocultos" };
            if tool_button(ui, glyph, tip, true, state.show_hidden) {
                state.show_hidden = !state.show_hidden;
            }
            if side == Side::Local && tool_button(ui, icons::FOLDER, "Elegir otra carpeta…", true, false) {
                actions.push(PaneAction::PickFolder);
            }
        });
    });
    ui.add_space(theme::space::XXS);
    breadcrumb(ui, side, state, &mut actions);
    ui.add_space(theme::space::S);

    let (head, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let columns = [
        (SortKey::Name, "Nombre", Rect::from_min_max(head.min, Pos2::new(head.max.x - 200.0, head.max.y)), Align2::LEFT_CENTER, head.min.x + 8.0),
        (SortKey::Size, "Tamaño", Rect::from_min_max(Pos2::new(head.max.x - 200.0, head.min.y), Pos2::new(head.max.x - 110.0, head.max.y)), Align2::RIGHT_CENTER, head.max.x - 150.0),
        (SortKey::Modified, "Modificado", Rect::from_min_max(Pos2::new(head.max.x - 110.0, head.min.y), head.max), Align2::RIGHT_CENTER, head.max.x - 8.0),
    ];
    let current_sort = state.sort.unwrap_or((SortKey::Name, true));
    for (key, label, zone, align, x) in columns {
        let id = ui.scope_id().with(("sort-column", side == Side::Remote, key as u8));
        let response = ui.interact(zone, id, Sense::click()).hand();
        let active = current_sort.0 == key;
        let arrow = match (active, current_sort.1) {
            (true, true) => format!(" {}", icons::CARET_UP),
            (true, false) => format!(" {}", icons::CARET_DOWN),
            _ => String::new(),
        };
        let color = if active || response.hovered() { theme::text() } else { theme::subtext() };
        ui.painter().text(Pos2::new(x, head.center().y), align, format!("{label}{arrow}"), FontId::new(theme::size::SMALL, FontFamily::Proportional), color);
        if response.on_hover_text("Ordenar por esta columna").clicked() {
            actions.push(PaneAction::Sort(key));
        }
    }

    let list_height = (ui.available_height() - 4.0).max(60.0);
    let (list_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), list_height), Sense::hover());
    ui.painter().rect_filled(list_rect, CornerRadius::same(theme::radius::CONTROL), theme::mantle());
    let mut list_ui = ui.new_child(UiBuilder::new().max_rect(list_rect.shrink(2.0)));

    if let Some(message) = status.or(state.error.as_deref()) {
        list_ui.add_space(theme::space::M);
        list_ui.label(RichText::new(message).color(if state.error.is_some() { theme::red() } else { theme::subtext() }));
    } else if state.loading && state.entries.is_empty() {
        list_ui.add_space(theme::space::M);
        list_ui.label(RichText::new("Cargando…").color(theme::subtext()));
    } else {
        let visible_names: Vec<String> = state.visible().iter().map(|e| e.name.clone()).collect();
        ScrollArea::vertical().id_salt(("explorer-list", side == Side::Remote)).auto_shrink([false, false]).show(&mut list_ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let visible: Vec<Entry> = state.visible().into_iter().cloned().collect();
            if visible.is_empty() {
                ui.add_space(theme::space::M);
                ui.label(RichText::new("Carpeta vacía").color(theme::subtext()));
            }
            for (index, entry) in visible.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::row(ROW_HEIGHT)), Sense::click_and_drag());
                let selected = state.selected.contains(&entry.name);
                if selected {
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::SMALL), theme::accent().gamma_multiply(0.28));
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::SMALL), theme::surface());
                }
                let painter = ui.painter().with_clip_rect(rect);
                let (glyph, glyph_color) = if entry.is_dir { (icons::FOLDER_CLOSED, theme::text()) } else { (icons::FILE, theme::subtext()) };
                painter.text(Pos2::new(rect.min.x + 16.0, rect.center().y), Align2::CENTER_CENTER, glyph, FontId::proportional(theme::size::ICON), glyph_color);
                let font = FontId::new(theme::size::BODY, if entry.is_dir { crate::ui::strong_family() } else { FontFamily::Proportional });
                let name_right = rect.max.x - if show_permissions { 340.0 } else { 230.0 };
                let name = crate::ui_kit::fit_text(&painter, &entry.name, &font, (name_right - rect.min.x - 32.0).max(24.0));
                painter.text(Pos2::new(rect.min.x + 32.0, rect.center().y), Align2::LEFT_CENTER, name, font, theme::text());
                if let (true, Some(mode)) = (show_permissions, entry.permissions) {
                    painter.text(
                        Pos2::new(rect.max.x - 250.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        sftp::format_mode(mode),
                        FontId::new(theme::size::SMALL, FontFamily::Monospace),
                        theme::subtext(),
                    );
                }
                let size = if entry.is_dir { String::new() } else { human_size(entry.size) };
                painter.text(
                    Pos2::new(rect.max.x - 150.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    size,
                    FontId::new(theme::size::SMALL, FontFamily::Proportional),
                    theme::subtext(),
                );
                if let Some(modified) = entry.modified {
                    painter.text(
                        Pos2::new(rect.max.x - 8.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        format_time(modified),
                        FontId::new(theme::size::SMALL, FontFamily::Proportional),
                        theme::subtext(),
                    );
                }

                if response.clicked() || response.drag_started() || response.secondary_clicked() {
                    actions.push(PaneAction::Activate);
                }
                if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    if modifiers.shift && state.anchor.is_some() {
                        let anchor = state.anchor.unwrap_or(index).min(visible.len() - 1);
                        let (from, to) = (anchor.min(index), anchor.max(index));
                        state.selected = visible_names[from..=to].iter().cloned().collect();
                    } else if modifiers.command || modifiers.ctrl {
                        if !state.selected.remove(&entry.name) {
                            state.selected.insert(entry.name.clone());
                        }
                        state.anchor = Some(index);
                    } else {
                        state.selected.clear();
                        state.selected.insert(entry.name.clone());
                        state.anchor = Some(index);
                    }
                }
                if response.secondary_clicked() && !state.selected.contains(&entry.name) {
                    state.selected.clear();
                    state.selected.insert(entry.name.clone());
                    state.anchor = Some(index);
                }
                if response.double_clicked() && entry.is_dir {
                    actions.push(PaneAction::Open(entry.name.clone()));
                }
                response.context_menu(|ui| {
                    if entry.is_dir && ui.button("Abrir").clicked() {
                        actions.push(PaneAction::Open(entry.name.clone()));
                        ui.close();
                    }
                    let (label, hint) = match side {
                        Side::Local => ("Subir al servidor", "→"),
                        Side::Remote => ("Bajar a este equipo", "←"),
                    };
                    if ui.button(format!("{label}  {hint}")).clicked() {
                        actions.push(PaneAction::CopyAcross);
                        ui.close();
                    }
                    if ui.button("Copiar ruta").clicked() {
                        actions.push(PaneAction::CopyPath);
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(state.selected.len() == 1, Button::new("Renombrar  (F2)")).clicked() {
                        actions.push(PaneAction::Rename);
                        ui.close();
                    }
                    if side == Side::Remote && ui.add_enabled(state.selected.len() == 1 && can_chmod, Button::new("Permisos")).clicked() {
                        actions.push(PaneAction::Permissions);
                        ui.close();
                    }
                    let delete = if can_delete { "Eliminar  (Supr)" } else { "Mover a la papelera  (Supr)" };
                    if ui.button(delete).clicked() {
                        actions.push(PaneAction::Delete);
                        ui.close();
                    }
                });
                if response.drag_started() {
                    if !state.selected.contains(&entry.name) {
                        state.selected.clear();
                        state.selected.insert(entry.name.clone());
                    }
                    response.dnd_set_drag_payload(side);
                }
            }
        });
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumbs_cover_every_level() {
        let remote = segments(Side::Remote, "/var/log/nginx");
        let labels: Vec<_> = remote.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["/", "var", "log", "nginx"]);
        assert_eq!(remote[2].1, "/var/log");
        assert_eq!(segments(Side::Remote, "/").len(), 1);
        let local = segments(Side::Local, "/home/user");
        // Los tramos locales usan el separador del sistema: se comparan como rutas, no como texto.
        assert_eq!(Path::new(&local.last().unwrap().1), Path::new("/home/user"));
        assert_eq!(Path::new(&local[0].1), Path::new("/"));
    }

    #[test]
    fn durations_read_as_clock_time() {
        assert_eq!(format_duration(42), "0:42");
        assert_eq!(format_duration(754), "12:34");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn hidden_files_are_filtered_unless_requested() {
        let entry = |name: &str| Entry { name: name.into(), is_dir: false, size: 0, modified: None, permissions: None };
        let mut pane = PaneState { entries: vec![entry(".bashrc"), entry("notas.txt")], ..Default::default() };
        assert_eq!(pane.visible().len(), 1);
        pane.show_hidden = true;
        assert_eq!(pane.visible().len(), 2);
    }
}
