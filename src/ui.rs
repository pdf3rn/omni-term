use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Key, Modifiers, Panel, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Vec2, ViewportCommand,
};
use zeroize::Zeroizing;

use crate::cursor::HandExt;
use crate::dialogs::{self, ConnForm, Dialog, ErrorInfo, Outcome, PaneTarget, Pending, Purpose, Retry, SecretPrompt, SnippetForm, TestState, UnlockPrompt, VarsPrompt};
use crate::layout::{Dir, Tree};
use crate::explorer::Explorer;
use crate::pane::{self, bold_family, Pane, PaneRequest, PADDING};
use crate::settings::{CursorChoice, Settings, ThemeChoice, DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE};
use crate::secrets::{account_for, SecretError, SecretKind, SecretStore, Secrets};
use crate::ssh::{self, Credentials, JumpHost, TunnelState, Tunnels, UiEvent};
use crate::store::{Auth, Connection, Forward, SavedTab, Snippet, Store, TabKind};
use crate::term::{Session, TermOptions};
use crate::theme;

const SIDEBAR_WIDTH: f32 = 230.0;
const TAB_BAR_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 34.0;
const CONTROL_RADIUS: u8 = 6;
const ROW_HEIGHT: f32 = 46.0;

/// Cómo deshacer lo que mostró un aviso.
enum Undo {
    Connection { connection: Connection, forwards: Vec<Forward>, dependents: Vec<i64> },
    Snippet(Snippet),
}

/// Limpieza que solo se hace si el aviso caduca sin deshacer.
enum Cleanup {
    Secrets(i64),
}

struct Toast {
    id: u64,
    text: String,
    undo: Option<Undo>,
    cleanup: Option<Cleanup>,
    born: std::time::Instant,
}

const TOAST_LIFETIME: Duration = Duration::from_secs(8);
const TOAST_FADE: Duration = Duration::from_millis(150);

/// Estado de las sesiones de una conexión, para el punto de la barra lateral.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LinkState {
    Closed,
    Open,
    Lost,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Connections,
    Snippets,
}

enum Body {
    Terminal {
        panes: Tree<Pane>,
        focused: u64,
        next_pane: u64,
        /// Túneles de la conexión de la pestaña (los paneles divididos no repiten los túneles).
        tunnels: Option<Tunnels>,
    },
    Explorer(Box<Explorer>),
}

struct Tab {
    id: u64,
    kind: TabKind,
    label: String,
    body: Body,
}

/// Separación entre paneles divididos; también aloja la barra de desplazamiento.
const SPLIT_GAP: f32 = 16.0;

impl Body {
    fn terminal(pane: Pane, tunnels: Option<Tunnels>) -> Self {
        let id = 1;
        Body::Terminal { panes: Tree::new(id, pane), focused: id, next_pane: id + 1, tunnels }
    }
}

impl Tab {
    /// El panel con foco de una pestaña de terminal.
    fn pane(&self) -> Option<&Pane> {
        match &self.body {
            Body::Terminal { panes, focused, .. } => panes.get(*focused),
            Body::Explorer(_) => None,
        }
    }

    fn tunnels(&self) -> Option<Tunnels> {
        match &self.body {
            Body::Terminal { tunnels, .. } => tunnels.clone(),
            Body::Explorer(_) => None,
        }
    }

    /// Título mostrado: el del terminal con foco si lo ha fijado, y si no la etiqueta de la pestaña.
    fn title(&self) -> String {
        match &self.body {
            Body::Terminal { .. } => {
                let title = self.pane().map(Pane::title).unwrap_or_default();
                if title.is_empty() { self.label.clone() } else { title }
            }
            Body::Explorer(explorer) => explorer.label(),
        }
    }
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    debug_keys: bool,
    window_title: String,
    next_tab_id: u64,
    settings: Settings,
    window_saved: (u32, u32),
    window_saved_at: std::time::Instant,
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
    pending_ui: VecDeque<UiEvent>,
    session_secrets: HashMap<(i64, SecretKind), Zeroizing<String>>,
    save_queue: VecDeque<(String, Zeroizing<String>)>,
    open_queue: VecDeque<SavedTab>,
    restore_active: Option<usize>,
    dialog: Option<Dialog>,
    /// Diálogo apartado mientras otro (huella, 2FA) pasa por delante.
    suspended: Option<Dialog>,
    saved_layout: (Vec<SavedTab>, usize),
    toasts: Vec<Toast>,
    next_toast: u64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, debug_keys: bool) -> anyhow::Result<Self> {
        install_fonts(&cc.egui_ctx);
        install_visuals(&cc.egui_ctx);

        let mut startup_notice: Option<String> = None;
        let store = match Store::default_path().and_then(|path| Store::open(&path)) {
            Ok(store) => store,
            Err(error) => {
                startup_notice = Some(format!("Sin base de datos persistente: {error:#}"));
                Store::open_in_memory()?
            }
        };
        let (layout, active) = store.load_layout().unwrap_or_default();
        let connections = store.list_connections().unwrap_or_default();
        let forwards = store.all_forwards().unwrap_or_default();
        let snippets = store.list_snippets().unwrap_or_default();
        let store = Arc::new(Mutex::new(store));
        let secrets = Secrets::select(store.clone());
        let settings = Settings::load(&store.lock().unwrap());
        let light = match settings.theme {
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
            ThemeChoice::System => cc.egui_ctx.system_theme() == Some(egui::Theme::Light),
        };
        theme::set_light(light);
        install_visuals(&cc.egui_ctx);
        let sidebar_view = match store.lock().unwrap().meta("sidebar_view") {
            Ok(Some(value)) if value == b"snippets" => SidebarView::Snippets,
            _ => SidebarView::Connections,
        };
        cc.egui_ctx.options_mut(|options| options.zoom_with_keyboard = false);
        let (events_tx, events_rx) = mpsc::channel();

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;

        let mut open_queue: VecDeque<SavedTab> = layout.into();
        if open_queue.is_empty() {
            open_queue.push_back(SavedTab::plain(TabKind::Local));
        }

        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            debug_keys,
            window_title: String::new(),
            next_tab_id: 0,
            settings,
            window_saved: (0, 0),
            window_saved_at: std::time::Instant::now(),
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
            pending_ui: VecDeque::new(),
            session_secrets: HashMap::new(),
            save_queue: VecDeque::new(),
            open_queue,
            restore_active: Some(active),
            dialog: None,
            suspended: None,
            saved_layout: (Vec::new(), 0),
            toasts: Vec::new(),
            next_toast: 0,
        };
        if let Some(notice) = startup_notice {
            app.notify(notice);
        }
        Ok(app)
    }

    /// Aviso breve, sin acción.
    fn notify(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), None, None);
    }

    fn push_toast(&mut self, text: String, undo: Option<Undo>, cleanup: Option<Cleanup>) {
        self.next_toast += 1;
        self.toasts.push(Toast { id: self.next_toast, text, undo, cleanup, born: std::time::Instant::now() });
        // Como mucho tres a la vez: el más antiguo caduca ya.
        while self.toasts.len() > 3 {
            let old = self.toasts.remove(0);
            self.finish_toast(old);
        }
    }

    /// Aplica la limpieza pendiente de un aviso que se cierra sin deshacer.
    fn finish_toast(&mut self, toast: Toast) {
        if let Some(Cleanup::Secrets(id)) = &toast.cleanup {
            let _ = self.secrets.delete(&account_for(*id, SecretKind::Password));
            let _ = self.secrets.delete(&account_for(*id, SecretKind::Passphrase));
        }
    }

    fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let ctx = self.ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    fn local_session(&mut self) -> anyhow::Result<Session> {
        let wake = self.wake();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        Session::start(cols, rows, options, move || wake(), move |feed| crate::local::spawn(cols, rows, feed))
    }

    fn ssh_session(&mut self, connection: Connection, credentials: Credentials, tunnels: Tunnels) -> anyhow::Result<Session> {
        let wake = self.wake();
        let wake_for_ssh = wake.clone();
        let handle = self.runtime.handle().clone();
        let events = self.events_tx.clone();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        Session::start(cols, rows, options, move || wake(), move |feed| {
            Ok(ssh::spawn(&handle, connection, credentials, (cols, rows), feed, events, wake_for_ssh, ssh::default_known_hosts(), tunnels))
        })
    }

    fn open_local(&mut self) -> anyhow::Result<()> {
        let session = self.local_session()?;
        let id = self.take_tab_id();
        let body = Body::terminal(Pane::new(session, self.debug_keys, 1), None);
        self.tabs.push(Tab { id, kind: TabKind::Local, label: "Terminal local".into(), body });
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
        let label = connection.name.clone();
        let forwards = self.forwards.get(&id).cloned().unwrap_or_default();
        let tunnels = ssh::new_tunnels(&forwards);
        let session = self.ssh_session(connection, credentials, tunnels.clone())?;
        let tab_id = slot.unwrap_or_else(|| self.take_tab_id());
        let body = Body::terminal(Pane::new(session, self.debug_keys, 1), Some(tunnels));
        let tab = Tab { id: tab_id, kind: TabKind::Connection(id), label, body };
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

    /// Añade o sustituye un panel SSH dentro de una pestaña de terminal.
    fn open_ssh_pane(&mut self, connection: Connection, credentials: Credentials, tab: u64, target: PaneTarget) -> anyhow::Result<()> {
        let session = self.ssh_session(connection, credentials, ssh::new_tunnels(&[]))?;
        self.place_pane(tab, target, session);
        Ok(())
    }

    fn place_pane(&mut self, tab: u64, target: PaneTarget, session: Session) {
        let debug_keys = self.debug_keys;
        let Some(Body::Terminal { panes, focused, next_pane, .. }) =
            self.tabs.iter_mut().find(|t| t.id == tab).map(|t| &mut t.body)
        else {
            return;
        };
        match target {
            PaneTarget::Split(dir) => {
                let id = *next_pane;
                *next_pane += 1;
                if panes.split(*focused, dir, id, Pane::new(session, debug_keys, id)) {
                    *focused = id;
                }
            }
            PaneTarget::Replace(id) => {
                if let Some(slot) = panes.get_mut(id) {
                    *slot = Pane::new(session, debug_keys, id);
                }
            }
        }
    }

    /// Divide el panel con foco de la pestaña activa con un terminal nuevo del mismo tipo.
    fn split_active(&mut self, dir: Dir) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let (tab_id, kind) = (tab.id, tab.kind);
        if !matches!(tab.body, Body::Terminal { .. }) {
            return;
        }
        match kind {
            TabKind::Local => match self.local_session() {
                Ok(session) => self.place_pane(tab_id, PaneTarget::Split(dir), session),
                Err(error) => self.dialog = Some(Dialog::failure("No se pudo abrir el terminal", format!("{error:#}"))),
            },
            TabKind::Connection(id) => {
                if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                    self.begin_open(connection, None, Purpose::Pane { tab: tab_id, target: PaneTarget::Split(dir) });
                }
            }
            TabKind::Explorer(_) => {}
        }
    }

    /// Cierra el panel con foco; si es el único, cierra la pestaña.
    fn close_focused_pane(&mut self) {
        let active = self.active;
        let Some(tab) = self.tabs.get_mut(active) else { return };
        match &mut tab.body {
            Body::Terminal { panes, focused, .. } if panes.len() > 1 => {
                let closing = *focused;
                if panes.remove(closing) {
                    *focused = panes.ids().first().copied().unwrap_or(1);
                }
            }
            _ => self.close_tab(active),
        }
    }

    /// Abre un explorador de archivos con su propia sesión SSH.
    fn open_explorer(&mut self, connection: Connection, credentials: Credentials, paths: (Option<String>, Option<String>)) -> anyhow::Result<()> {
        let Some(id) = connection.id else { anyhow::bail!("la conexión no está guardada") };
        let label = format!("Archivos · {}", connection.name);
        let explorer = Explorer::new(
            connection,
            credentials,
            self.runtime.handle().clone(),
            self.wake(),
            self.events_tx.clone(),
            ssh::default_known_hosts(),
            paths,
        );
        let tab_id = self.take_tab_id();
        self.tabs.push(Tab { id: tab_id, kind: TabKind::Explorer(id), label, body: Body::Explorer(Box::new(explorer)) });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Credenciales de una sola conexión: contraseña o passphrase si las necesita.
    fn own_credentials(&mut self, connection: &Connection) -> Result<Credentials, Step> {
        let kind = match connection.auth {
            Auth::Password => Some(SecretKind::Password),
            Auth::Key => match connection.key_path.as_deref().map(ssh::key_is_encrypted) {
                Some(Ok(true)) => Some(SecretKind::Passphrase),
                Some(Err(error)) => return Err(Step::Failed(format!("{error:#}"))),
                _ => None,
            },
            Auth::Agent | Auth::Interactive => None,
        };
        let Some(kind) = kind else { return Ok(Credentials::default()) };
        let id = connection.id.unwrap_or_default();
        if let Some(secret) = self.session_secrets.get(&(id, kind)) {
            return Ok(credentials_for(kind, secret.clone()));
        }
        match self.secrets.get(&account_for(id, kind)) {
            Ok(Some(secret)) => Ok(credentials_for(kind, secret)),
            Ok(None) => Err(Step::Prompt { asking: connection.clone(), kind }),
            Err(SecretError::Locked) => Err(Step::Unlock),
            Err(error) => Err(Step::Failed(format!("Almacén de secretos: {error}"))),
        }
    }

    /// Credenciales de la conexión y de toda su cadena de servidores de salto.
    fn gather_credentials(&mut self, connection: &Connection, depth: usize) -> Result<Credentials, Step> {
        let mut credentials = self.own_credentials(connection)?;
        if let Some(jump_id) = connection.jump {
            if depth >= 4 {
                return Err(Step::Failed("Demasiados saltos encadenados: revisa que no haya un ciclo".into()));
            }
            let Some(jump) = self.connections.iter().find(|c| c.id == Some(jump_id)).cloned() else {
                return Err(Step::Failed("El servidor de salto de esta conexión ya no existe".into()));
            };
            let jump_credentials = self.gather_credentials(&jump, depth + 1)?;
            credentials.jump = Some(Box::new(JumpHost { connection: jump, credentials: jump_credentials }));
        }
        Ok(credentials)
    }

    /// Credenciales para «Probar conexión»: el secreto escrito en el formulario o el ya guardado.
    /// Nunca pregunta: si falta algo, devuelve un mensaje que dice qué escribir.
    fn test_credentials(&mut self, connection: &Connection, typed: &str) -> Result<Credentials, String> {
        let needs = match connection.auth {
            Auth::Password => Some(SecretKind::Password),
            Auth::Key => match connection.key_path.as_deref().map(ssh::key_is_encrypted) {
                Some(Ok(true)) => Some(SecretKind::Passphrase),
                Some(Err(error)) => return Err(format!("{error:#}")),
                _ => None,
            },
            Auth::Agent | Auth::Interactive => None,
        };
        let mut credentials = Credentials::default();
        if let Some(kind) = needs {
            let what = if kind == SecretKind::Password { "la contraseña" } else { "la passphrase" };
            if !typed.is_empty() {
                credentials = credentials_for(kind, Zeroizing::new(typed.to_string()));
            } else if connection.id.is_some() {
                credentials = match self.own_credentials(connection) {
                    Ok(found) => found,
                    Err(Step::Prompt { .. }) => return Err(format!("Escribe {what} en el formulario para poder probar.")),
                    Err(Step::Unlock) => {
                        return Err("El almacén de secretos está bloqueado: ábrelo antes con una conexión que use contraseña guardada.".into())
                    }
                    Err(Step::Failed(message)) => return Err(message),
                };
            } else {
                return Err(format!("Escribe {what} en el formulario para poder probar."));
            }
        }
        if let Some(jump_id) = connection.jump {
            let Some(jump) = self.connections.iter().find(|c| c.id == Some(jump_id)).cloned() else {
                return Err("El servidor de salto ya no existe.".into());
            };
            match self.gather_credentials(&jump, 1) {
                Ok(found) => credentials.jump = Some(Box::new(JumpHost { connection: jump, credentials: found })),
                Err(Step::Prompt { asking, .. }) => {
                    return Err(format!("«{}» necesita su contraseña guardada para poder pasar por él.", asking.name))
                }
                Err(Step::Unlock) => return Err("El almacén de secretos está bloqueado.".into()),
                Err(Step::Failed(message)) => return Err(message),
            }
        }
        Ok(credentials)
    }

    /// Reúne los secretos necesarios (pidiéndolos si hace falta) y abre la pestaña.
    fn begin_open(&mut self, connection: Connection, slot: Option<u64>, purpose: Purpose) {
        match self.gather_credentials(&connection, 0) {
            Ok(credentials) => self.finish_open(connection, credentials, slot, purpose),
            Err(Step::Prompt { asking, kind }) => {
                self.dialog = Some(Dialog::Secret(SecretPrompt {
                    purpose,
                    connection,
                    asking,
                    slot,
                    kind,
                    input: String::new(),
                    save: true,
                    reveal: false,
                }));
            }
            Err(Step::Unlock) => self.ask_unlock(Pending::Open(connection, slot, purpose)),
            Err(Step::Failed(message)) => self.dialog = Some(Dialog::failure("No se pudo preparar la conexión", message)),
        }
    }

    fn finish_open(&mut self, connection: Connection, credentials: Credentials, slot: Option<u64>, purpose: Purpose) {
        let retry = Retry::Open(connection.clone(), slot, purpose.clone());
        let result = match purpose {
            Purpose::Terminal => self.open_ssh(connection, credentials, slot),
            Purpose::Explorer(local, remote) => self.open_explorer(connection, credentials, (local, remote)),
            Purpose::Pane { tab, target } => self.open_ssh_pane(connection, credentials, tab, target),
        };
        if let Err(error) = result {
            self.dialog = Some(Dialog::Error(ErrorInfo {
                title: "No se pudo abrir la conexión".into(),
                detail: format!("{error:#}"),
                retry: Some(retry),
            }));
        }
    }

    fn ask_unlock(&mut self, then: Pending) {
        let creating = self.secrets.master().is_some_and(|m| !m.is_initialized().unwrap_or(false));
        self.dialog = Some(Dialog::Unlock(UnlockPrompt {
            creating,
            reveal: false,
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
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo guardar el secreto", error)),
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
            Some(tab) if tab.pane().is_some_and(|p| !p.session.is_disconnected()) => {
                if let Some(pane) = tab.pane() {
                    pane.insert_text(body, run);
                }
            }
            _ => self.notify("No hay un terminal activo donde insertar"),
        }
    }

    fn reconnect(&mut self, index: usize) {
        let (TabKind::Connection(id), tab_id) = (self.tabs[index].kind, self.tabs[index].id) else { return };
        let split_focus = match &self.tabs[index].body {
            Body::Terminal { panes, focused, .. } if panes.len() > 1 => Some(*focused),
            _ => None,
        };
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
            match split_focus {
                // Con paneles divididos solo se reconecta el panel caído.
                Some(pane) => self.begin_open(connection, None, Purpose::Pane { tab: tab_id, target: PaneTarget::Replace(pane) }),
                None => self.begin_open(connection, Some(tab_id), Purpose::Terminal),
            }
        }
    }

    /// Color de identidad de la conexión a la que pertenece una pestaña, si lo tiene.
    fn identity_for(&self, kind: TabKind) -> Option<Color32> {
        match kind {
            TabKind::Connection(id) | TabKind::Explorer(id) => self
                .connections
                .iter()
                .find(|c| c.id == Some(id))
                .and_then(|c| c.color)
                .map(theme::identity),
            TabKind::Local => None,
        }
    }

    fn environment_for(&self, kind: TabKind) -> Option<String> {
        match kind {
            TabKind::Connection(id) | TabKind::Explorer(id) => {
                self.connections.iter().find(|c| c.id == Some(id)).and_then(|c| c.env.clone())
            }
            TabKind::Local => None,
        }
    }

    fn delete_connection(&mut self, connection: Connection) {
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

    fn delete_snippet(&mut self, snippet: Snippet) {
        let Some(id) = snippet.id else { return };
        if let Err(error) = self.store.lock().unwrap().delete_snippet(id) {
            self.dialog = Some(Dialog::failure("No se pudo eliminar el snippet", format!("{error:#}")));
            return;
        }
        self.refresh_connections();
        let text = format!("«{}» eliminado", snippet.title);
        self.push_toast(text, Some(Undo::Snippet(snippet)), None);
    }

    fn undo(&mut self, undo: Undo) {
        let result = {
            let mut store = self.store.lock().unwrap();
            match &undo {
                Undo::Connection { connection, forwards, dependents } => store
                    .restore_connection(connection)
                    .and_then(|_| store.replace_forwards(connection.id.unwrap_or_default(), forwards))
                    .and_then(|_| dependents.iter().try_for_each(|id| store.set_jump(*id, connection.id))),
                Undo::Snippet(snippet) => store.restore_snippet(snippet),
            }
        };
        match result {
            Ok(()) => self.refresh_connections(),
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo deshacer", format!("{error:#}"))),
        }
    }

    /// Dibuja los avisos abajo a la derecha, con fundido de entrada y de salida, y procesa su caducidad.
    fn toasts_ui(&mut self, ctx: &egui::Context) {
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
            .order(egui::Order::Foreground)
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
                                if toast.undo.is_some() && ui.add(Button::new(RichText::new("Deshacer").color(theme::accent())).fill(Color32::TRANSPARENT)).clicked() {
                                    undone = Some(index);
                                }
                                if ui.add(Button::new("×").frame(false)).on_hover_text("Cerrar").clicked() {
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

    fn term_options(&self) -> TermOptions {
        use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
        let shape = match self.settings.cursor {
            CursorChoice::Block => CursorShape::Block,
            CursorChoice::Beam => CursorShape::Beam,
            CursorChoice::Underline => CursorShape::Underline,
        };
        TermOptions { history: self.settings.scrollback, cursor: CursorStyle { shape, blinking: self.settings.blink } }
    }

    /// Guarda los ajustes y los aplica a las sesiones abiertas.
    fn apply_settings(&mut self) {
        let _ = self.settings.save(&self.store.lock().unwrap());
        let options = self.term_options();
        for tab in &self.tabs {
            if let Body::Terminal { panes, .. } = &tab.body {
                panes.for_each(&mut |_, pane| pane.session.set_options(options));
            }
        }
    }

    /// Cambia entre tema claro y oscuro según el ajuste (o el del sistema) y reinstala los estilos.
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let light = match self.settings.theme {
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
            ThemeChoice::System => ctx.system_theme() == Some(egui::Theme::Light),
        };
        if light != theme::is_light() {
            theme::set_light(light);
            install_visuals(ctx);
        }
    }

    /// Recuerda el tamaño de la ventana para la próxima vez (como mucho una vez por segundo).
    fn remember_window_size(&mut self, ctx: &egui::Context) {
        let (rect, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        let Some(rect) = rect else { return };
        let size = (rect.width().round() as u32, rect.height().round() as u32);
        if maximized || size == self.window_saved || self.window_saved_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.window_saved = size;
        self.window_saved_at = std::time::Instant::now();
        let _ = self.store.lock().unwrap().set_meta("window_size", format!("{}x{}", size.0, size.1).as_bytes());
    }

    fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (size - self.settings.font_size).abs() > f32::EPSILON {
            self.settings.font_size = size;
            let _ = self.settings.save(&self.store.lock().unwrap());
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
                self.dialog = Some(Dialog::failure("No se pudo leer ~/.ssh/config", format!("{}: {error}", path.display())));
                return;
            }
        };
        let user = std::env::var("USER").unwrap_or_default();
        let mut imported = 0;
        let mut jumps: Vec<(i64, String)> = Vec::new();
        {
            let store = self.store.lock().unwrap();
            for host in crate::sshconfig::parse(&text, &user) {
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

    fn drive_pending(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.pending_ui.push_back(event);
        }

        // Un panel cuyo proceso terminó se quita; si era el único de la pestaña, se cierra la pestaña.
        let mut closing_tabs = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Body::Terminal { panes, focused, .. } = &mut tab.body else { continue };
            let mut gone = Vec::new();
            panes.for_each(&mut |id, pane| {
                if pane.session.has_exited() {
                    gone.push(id);
                }
            });
            for id in gone {
                if panes.remove(id) {
                    if *focused == id {
                        *focused = panes.ids().first().copied().unwrap_or(1);
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
        } else if let Some(saved) = self.open_queue.pop_front() {
            match saved.kind {
                TabKind::Local => {
                    if let Err(error) = self.open_local() {
                        self.dialog = Some(Dialog::failure("No se pudo abrir el terminal local", format!("{error:#}")));
                    }
                }
                TabKind::Connection(id) => {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.begin_open(connection, None, Purpose::Terminal);
                    }
                }
                TabKind::Explorer(id) => {
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                        self.begin_open(connection, None, Purpose::Explorer(saved.local_path, saved.remote_path));
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
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .map(|t| match &t.body {
                Body::Explorer(explorer) => {
                    let (local, remote) = explorer.paths();
                    SavedTab { kind: t.kind, local_path: Some(local), remote_path: Some(remote).filter(|r| !r.is_empty()) }
                }
                Body::Terminal { .. } => SavedTab::plain(t.kind),
            })
            .collect();
        let layout = (tabs, self.active);
        if layout != self.saved_layout {
            if let Err(error) = self.store.lock().unwrap().save_layout(&layout.0, layout.1) {
                eprintln!("no se pudo guardar la sesión: {error:#}");
            }
            self.saved_layout = layout;
        }
    }

    fn handle_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut forced = Secrets::is_master_forced(&self.store.lock().unwrap());
        let (forced_before, settings_before) = (forced, self.settings.clone());
        let outcome = dialogs::show(ctx, &mut dialog, &mut forced, &self.connections, &mut self.settings);
        if self.settings != settings_before {
            self.apply_settings();
        }
        if forced != forced_before {
            let result = Secrets::set_master_forced(&self.store.lock().unwrap(), forced);
            match result {
                Ok(()) => self.notify("Reinicia omni-term para aplicar el cambio de almacén de secretos"),
                Err(error) => self.dialog = Some(Dialog::failure("No se pudo guardar el ajuste", format!("{error:#}"))),
            }
        }

        match (outcome, dialog) {
            (Outcome::None, dialog) => self.dialog = Some(dialog),
            (Outcome::Submit, Dialog::Error(ErrorInfo { retry: Some(Retry::Open(connection, slot, purpose)), .. })) => {
                self.begin_open(connection, slot, purpose);
            }
            (Outcome::Cancel, Dialog::Prompt(request, _)) => {
                let _ = request.reply.send(None);
            }
            (Outcome::Cancel, Dialog::Secret(_) | Dialog::Unlock(_) | Dialog::Edit(_) | Dialog::Error(_) | Dialog::HostKeyChanged(_) | Dialog::Settings | Dialog::Snippet(_) | Dialog::SnippetVars(_) | Dialog::ConfirmReset(_)) => {}
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
            (Outcome::Delete, Dialog::ConfirmReset(then)) => {
                if let Some(master) = self.secrets.master() {
                    if let Err(error) = master.reset() {
                        self.dialog = Some(Dialog::failure("No se pudieron borrar los secretos", format!("{error:#}")));
                        return;
                    }
                }
                self.ask_unlock(then);
            }
            (Outcome::ResetMasterKey, Dialog::Unlock(prompt)) => self.dialog = Some(Dialog::ConfirmReset(prompt.then)),
            (Outcome::Submit, Dialog::Secret(prompt)) => {
                let secret = Zeroizing::new(prompt.input);
                let id = prompt.asking.id.unwrap_or_default();
                self.session_secrets.insert((id, prompt.kind), secret.clone());
                if prompt.save && !secret.is_empty() {
                    self.save_queue.push_back((account_for(id, prompt.kind), secret));
                }
                self.begin_open(prompt.connection, prompt.slot, prompt.purpose);
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
                                Pending::Open(connection, slot, purpose) => self.begin_open(connection, slot, purpose),
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
        let mut files: Option<Connection> = None;
        let mut open_settings = false;
        let mut new_snippet = false;
        let mut snippet_from_selection = false;
        let mut insert: Option<(Snippet, bool)> = None;
        let mut copy: Option<String> = None;
        let mut edit_snippet: Option<Snippet> = None;
        let mut delete_snippet: Option<Snippet> = None;
        Panel::left("sidebar")
            .default_size(SIDEBAR_WIDTH)
            .min_size(180.0)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                ui.label(RichText::new("omni-term").font(FontId::new(15.0, strong_family())).color(theme::text()));
                ui.add_space(6.0);

                // El terminal local es un sitio más de la lista: misma fila, sin botón destacado.
                let shell = std::env::var("SHELL")
                    .ok()
                    .and_then(|s| s.rsplit('/').next().map(str::to_string))
                    .unwrap_or_else(|| "shell".into());
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                if response.hovered() {
                    ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), theme::surface());
                }
                {
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.text(Pos2::new(rect.min.x, rect.min.y + 9.0), Align2::LEFT_TOP, "Terminal local", FontId::new(14.0, strong_family()), theme::text());
                    painter.text(Pos2::new(rect.min.x, rect.min.y + 27.0), Align2::LEFT_TOP, format!("Este equipo · {shell}"), FontId::new(12.0, FontFamily::Proportional), theme::subtext());
                }
                if response.hand().on_hover_text("Abrir un terminal local en una pestaña nueva").clicked() {
                    open_local = true;
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding.x = 0.0;
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for (view, label) in [(SidebarView::Connections, "Conexiones"), (SidebarView::Snippets, "Snippets")] {
                        let selected = self.sidebar_view == view;
                        let color = if selected { theme::text() } else { theme::subtext() };
                        let font = FontId::new(13.0, if selected { strong_family() } else { FontFamily::Proportional });
                        let button = Button::new(RichText::new(label).font(font).color(color)).fill(Color32::TRANSPARENT);
                        let response = ui.add(button);
                        if selected {
                            let y = response.rect.max.y + 1.0;
                            ui.painter().line_segment(
                                [Pos2::new(response.rect.min.x, y), Pos2::new(response.rect.max.x, y)],
                                Stroke::new(2.0, theme::accent()),
                            );
                        }
                        if response.clicked() && self.sidebar_view != view {
                            self.sidebar_view = view;
                            let value: &[u8] = if view == SidebarView::Snippets { b"snippets" } else { b"connections" };
                            let _ = self.store.lock().unwrap().set_meta("sidebar_view", value);
                        }
                    }
                });
                ui.add_space(6.0);

                if self.sidebar_view == SidebarView::Connections {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Guardadas").small().color(theme::subtext()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let add = Button::new(RichText::new("+").size(16.0)).fill(Color32::TRANSPARENT).min_size(Vec2::splat(26.0));
                        if ui.add(add).on_hover_text("Nueva conexión").clicked() {
                            new_connection = true;
                        }
                    });
                });
                ui.add_space(-2.0);

                if self.connections.is_empty() {
                    ui.label(RichText::new("Aún no hay conexiones. Crea una con + o importa las de ~/.ssh/config.").color(theme::subtext()));
                }
                ui.spacing_mut().item_spacing.y = 2.0;
                for connection in &self.connections {
                    let state = connection.id.map_or(LinkState::Closed, |id| {
                        self.tabs
                            .iter()
                            .filter(|t| t.kind == TabKind::Connection(id))
                            .fold(LinkState::Closed, |acc, t| {
                                if t.pane().is_some_and(|p| p.session.is_disconnected()) {
                                    acc.max(LinkState::Lost)
                                } else {
                                    acc.max(LinkState::Open)
                                }
                            })
                    });
                    let identity = connection.color.map(theme::identity);
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), theme::surface());
                    }
                    if let Some(color) = identity {
                        // Marca de identidad: informa de qué servidor es, no decora.
                        let bar = Rect::from_min_size(Pos2::new(rect.min.x - 8.0, rect.min.y + 7.0), Vec2::new(3.0, ROW_HEIGHT - 14.0));
                        ui.painter().rect_filled(bar, CornerRadius::same(2), color);
                    }
                    let painter = ui.painter().with_clip_rect(rect);
                    let name_font = FontId::new(14.0, strong_family());
                    let name_width = painter.layout_no_wrap(connection.name.clone(), name_font.clone(), Color32::PLACEHOLDER).size().x;
                    painter.text(Pos2::new(rect.min.x, rect.min.y + 9.0), Align2::LEFT_TOP, &connection.name, name_font, theme::text());
                    if let Some(env) = &connection.env {
                        painter.text(
                            Pos2::new(rect.min.x + name_width + 8.0, rect.min.y + 11.0),
                            Align2::LEFT_TOP,
                            env,
                            FontId::new(12.0, strong_family()),
                            identity.unwrap_or_else(theme::subtext),
                        );
                    }
                    painter.text(
                        Pos2::new(rect.min.x, rect.min.y + 27.0),
                        Align2::LEFT_TOP,
                        format!("{}@{}", connection.user, connection.host),
                        FontId::new(12.0, FontFamily::Proportional),
                        theme::subtext(),
                    );
                    match state {
                        LinkState::Open => {
                            ui.painter().circle_filled(Pos2::new(rect.max.x - 6.0, rect.center().y), 3.5, theme::accent());
                        }
                        LinkState::Lost => {
                            ui.painter().circle_filled(Pos2::new(rect.max.x - 6.0, rect.center().y), 3.5, theme::red());
                        }
                        LinkState::Closed => {}
                    }
                    let response = response.hand().on_hover_text(match state {
                        LinkState::Closed => "Doble clic para conectar",
                        LinkState::Open => "Sesión abierta. Doble clic para otra pestaña",
                        LinkState::Lost => "Sesión caída: usa Reconectar en su pestaña",
                    });
                    if response.double_clicked() {
                        connect = Some(connection.clone());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Conectar").clicked() {
                            connect = Some(connection.clone());
                            ui.close();
                        }
                        if ui.button("Explorador de archivos").clicked() {
                            files = Some(connection.clone());
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
                        .and_then(|t| t.tunnels());
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
                                ui.label(RichText::new(text).small().color(theme::subtext()));
                            }
                        }
                    }
                    ui.add_space(4.0);
                }

                ui.add_space(8.0);
                let import_button = Button::new(RichText::new("Importar de ~/.ssh/config").small().color(theme::subtext()))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, theme::surface()));
                if ui.add(import_button).clicked() {
                    import = true;
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
                        .add(Button::new(RichText::new("Guardar selección").small().color(theme::subtext())).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, theme::surface())))
                        .clicked()
                    {
                        snippet_from_selection = true;
                    }
                    ui.add_space(4.0);

                    let query = self.snippet_query.clone();
                    let visible: Vec<&Snippet> = self.snippets.iter().filter(|s| s.matches(&query)).collect();
                    if self.snippets.is_empty() {
                        ui.label(RichText::new("Aún no hay snippets. Crea uno con + o guarda la selección del terminal.").color(theme::subtext()));
                    } else if visible.is_empty() {
                        ui.label(RichText::new("Ningún snippet coincide con la búsqueda.").color(theme::subtext()));
                    }
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for snippet in visible {
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::surface());
                        }
                        let left = rect.min.x + 10.0;
                        let painter = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)));
                        painter.text(
                            Pos2::new(left, rect.min.y + 9.0),
                            Align2::LEFT_TOP,
                            &snippet.title,
                            FontId::new(14.0, strong_family()),
                            theme::text(),
                        );
                        let preview = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                        let preview: String = preview.chars().take(80).collect();
                        let preview = if snippet.tag.is_empty() { preview } else { format!("{}  ·  {}", snippet.tag, preview) };
                        painter.text(
                            Pos2::new(left, rect.min.y + 27.0),
                            Align2::LEFT_TOP,
                            preview,
                            FontId::new(12.0, FontFamily::Monospace),
                            theme::subtext(),
                        );
                        let response = response.hand().on_hover_text("Clic para insertar en el terminal activo. Clic derecho: más opciones");
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
                    if ui.add(Button::new("Ajustes").fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, theme::surface()))).clicked() {
                        open_settings = true;
                    }
                    ui.label(RichText::new(format!("Secretos: {}", self.secrets.label())).small().color(theme::subtext()));
                });
            });

        if open_settings {
            self.dialog = Some(Dialog::Settings);
        }
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
            let selected = self.tabs.get(self.active).and_then(|t| t.pane()).and_then(|p| p.selection_text());
            match selected {
                Some(text) => self.dialog = Some(Dialog::Snippet(SnippetForm::new(text))),
                None => self.notify("Selecciona texto en el terminal para guardarlo como snippet"),
            }
        }
        if let Some(snippet) = edit_snippet {
            self.dialog = Some(Dialog::Snippet(SnippetForm::from_snippet(&snippet)));
        }
        if let Some(snippet) = delete_snippet {
            self.delete_snippet(snippet);
        }
        if open_local {
            self.open_queue.push_back(SavedTab::plain(TabKind::Local));
        }
        if new_connection {
            self.dialog = Some(Dialog::Edit(ConnForm::new()));
        }
        if import {
            self.import_ssh_config();
        }
        if let Some(connection) = connect {
            self.begin_open(connection, None, Purpose::Terminal);
        }
        if let Some(connection) = files {
            self.begin_open(connection, None, Purpose::Explorer(None, None));
        }
        if let Some(connection) = edit {
            self.dialog = Some(Dialog::Edit(ConnForm::from_connection(&connection, self.forwards.get(&connection.id.unwrap_or_default()).map_or(&[][..], |f| f.as_slice()))));
        }
        if let Some(connection) = delete {
            self.delete_connection(connection);
        }
    }
}

/// Una línea por túnel: punto de estado, resumen y número de conexiones atendidas.
fn tunnel_line(ui: &mut egui::Ui, status: &ssh::TunnelStatus) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let (color, tip) = match &status.state {
        TunnelState::Starting => (theme::subtext(), "Iniciando".to_string()),
        TunnelState::Active => (theme::accent(), "Activo".to_string()),
        TunnelState::Failed(reason) => (theme::red(), reason.clone()),
        TunnelState::Stopped => (theme::selection(), "Detenido".to_string()),
    };
    ui.painter().circle_filled(Pos2::new(rect.min.x + 14.0, rect.center().y), 3.0, color);
    ui.painter().text(
        Pos2::new(rect.min.x + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        status.forward.summary(),
        FontId::new(12.0, FontFamily::Proportional),
        theme::subtext(),
    );
    if status.served > 0 {
        ui.painter().text(
            Pos2::new(rect.max.x - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status.served.to_string(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::subtext(),
        );
    }
    response.on_hover_text(tip);
}

/// Qué falta para poder abrir una conexión.
enum Step {
    Prompt { asking: Connection, kind: SecretKind },
    Unlock,
    Failed(String),
}

fn credentials_for(kind: SecretKind, secret: Zeroizing<String>) -> Credentials {
    match kind {
        SecretKind::Password => Credentials { password: Some(secret), ..Default::default() },
        SecretKind::Passphrase => Credentials { key_passphrase: Some(secret), ..Default::default() },
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
    style.animation_time = 0.12;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;

    let radius = CornerRadius::same(CONTROL_RADIUS);
    let v = &mut style.visuals;
    v.panel_fill = theme::base();
    v.window_fill = theme::mantle();
    v.window_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, theme::surface());
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(110) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
    v.menu_corner_radius = CornerRadius::same(8);
    v.override_text_color = Some(theme::text());
    v.extreme_bg_color = theme::base();
    v.selection.bg_fill = theme::accent().gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, theme::accent());

    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::surface());
    for (widget, fill) in [
        (&mut v.widgets.inactive, theme::surface()),
        (&mut v.widgets.hovered, theme::selection()),
        (&mut v.widgets.active, theme::selection()),
    ] {
        widget.weak_bg_fill = fill;
        widget.bg_fill = fill;
        widget.bg_stroke = Stroke::NONE;
        widget.corner_radius = radius;
    }
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, theme::text());
    ctx.set_global_style(style);
}

/// Pestaña con trazo de acento arriba cuando está activa; el cierre aparece al pasar el ratón.
fn tab(ui: &mut egui::Ui, label: &str, selected: bool, index: usize, identity: Option<Color32>) -> (bool, bool) {
    let shown = if label.chars().count() > 24 {
        format!("{}…", label.chars().take(23).collect::<String>())
    } else {
        label.to_string()
    };
    let font = FontId::new(14.0, if selected { strong_family() } else { FontFamily::Proportional });
    let galley = ui.painter().layout_no_wrap(shown, font, Color32::PLACEHOLDER);
    let size = Vec2::new(galley.size().x + 12.0 + 8.0 + 20.0 + 8.0, TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.hand();

    let hovered = response.hovered();
    let fill = if selected {
        theme::base()
    } else if hovered {
        theme::surface()
    } else {
        Color32::TRANSPARENT
    };
    let corner = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
    ui.painter().rect_filled(rect, corner, fill);
    // La línea superior marca la pestaña activa; con identidad, usa el color del servidor y se ve siempre.
    let line_color = match (selected, identity) {
        (true, Some(color)) => Some(color),
        (true, None) => Some(theme::accent()),
        (false, Some(color)) => Some(color.gamma_multiply(0.55)),
        (false, None) => None,
    };
    if let Some(color) = line_color {
        let line = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 2.0));
        ui.painter().rect_filled(line, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, color);
    }

    let color = if selected { theme::text() } else { theme::subtext() };
    ui.painter().galley(Pos2::new(rect.min.x + 12.0, rect.center().y - galley.size().y / 2.0 + 1.0), galley, color);

    let close_rect = Rect::from_center_size(Pos2::new(rect.max.x - 18.0, rect.center().y + 1.0), Vec2::splat(20.0));
    let mut close_clicked = false;
    if selected || hovered {
        let close = ui.interact(close_rect, ui.auto_id_with(("tab-close", index)), Sense::click()).hand();
        if close.hovered() {
            ui.painter().rect_filled(close_rect, CornerRadius::same(4), theme::selection());
        }
        ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(15.0), color);
        close_clicked = close.on_hover_text("Cerrar pestaña").clicked();
    }
    (response.clicked() && !close_clicked, close_clicked)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_theme(&ctx);
        self.remember_window_size(&ctx);

        self.sidebar(ui);

        let mut activate = None;
        let mut close = None;
        let mut new_local = false;
        let mut files_tab: Option<i64> = None;
        let mut split: Option<Dir> = None;
        let mut close_pane = false;
        Panel::top("tabs")
            .exact_size(TAB_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 0 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, t) in self.tabs.iter().enumerate() {
                        let identity = self.identity_for(t.kind);
                        let (clicked, closed) = tab(ui, &t.title(), i == self.active, i, identity);
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

                    // Acciones de la pestaña activa, juntas y a la derecha, con el mismo estilo.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if self.tabs.get(self.active).is_some_and(|t| matches!(t.body, Body::Terminal { .. })) {
                            ui.scope(|ui| {
                                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                                ui.menu_button(RichText::new("Dividir").small(), |ui| {
                                    if ui.button("A la derecha  (Ctrl+Shift+D)").clicked() {
                                        split = Some(Dir::Horizontal);
                                        ui.close();
                                    }
                                    if ui.button("Hacia abajo  (Ctrl+Shift+E)").clicked() {
                                        split = Some(Dir::Vertical);
                                        ui.close();
                                    }
                                    if ui.button("Cerrar panel  (Ctrl+Shift+W)").clicked() {
                                        close_pane = true;
                                        ui.close();
                                    }
                                });
                            });
                        }
                        if let Some(TabKind::Connection(id)) = self.tabs.get(self.active).map(|t| t.kind) {
                            let files = Button::new(RichText::new("Archivos").small()).fill(Color32::TRANSPARENT);
                            if ui.add(files).on_hover_text("Abrir el explorador de archivos de esta conexión").clicked() {
                                files_tab = Some(id);
                            }
                        }
                    });
                });
            });

        if let Some(i) = activate {
            self.active = i;
        }
        if let Some(dir) = split {
            self.split_active(dir);
        }
        if close_pane {
            self.close_focused_pane();
        }
        if let Some(id) = files_tab {
            if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                self.begin_open(connection, None, Purpose::Explorer(None, None));
            }
        }
        if new_local {
            self.open_queue.push_back(SavedTab::plain(TabKind::Local));
        }
        if let Some(i) = close {
            self.close_tab(i);
        }

        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, CornerRadius::ZERO, theme::base());
        let text_area = area.shrink(PADDING);
        let active_kind = self.tabs.get(self.active).map(|t| t.kind);
        let identity = active_kind.and_then(|kind| self.identity_for(kind));
        if let Some(color) = identity {
            // Banda de identidad bajo las pestañas: recuerda en qué servidor estás escribiendo.
            ui.painter().rect_filled(Rect::from_min_size(area.min, Vec2::new(area.width(), 3.0)), CornerRadius::ZERO, color);
        }

        // Zoom del terminal: Ctrl+= / Ctrl+- / Ctrl+0, reservados y no enviados a la aplicación.
        let zoom = ctx.input_mut(|i| {
            let mut next = self.settings.font_size;
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
        self.grid = pane::grid_for(ui, text_area, self.settings.font_size);
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
            let input_enabled = self.dialog.is_none() && self.pending_ui.is_empty();
            let font_size = self.settings.font_size;
            let mut disconnected = false;
            let mut request = None;
            match &mut self.tabs[active].body {
                Body::Terminal { panes, focused, .. } => {
                    let gap = if panes.len() > 1 { SPLIT_GAP } else { 0.0 };
                    let (leaves, handles) = panes.layout(text_area, gap);

                    // Arrastrar las asas redimensiona las divisiones.
                    let mut ratios = Vec::new();
                    for (index, handle) in handles.iter().enumerate() {
                        let id = ui.scope_id().with(("split-handle", active, index));
                        let response = ui.interact(handle.rect, id, Sense::drag());
                        let icon = match handle.dir {
                            Dir::Horizontal => egui::CursorIcon::ResizeHorizontal,
                            Dir::Vertical => egui::CursorIcon::ResizeVertical,
                        };
                        if response.hovered() || response.dragged() {
                            ctx.set_cursor_icon(icon);
                        }
                        if let (true, Some(pointer)) = (response.dragged(), response.interact_pointer_pos()) {
                            ratios.push((handle.path.clone(), handle.ratio_at(pointer)));
                        }
                        let line = match handle.dir {
                            Dir::Horizontal => Rect::from_center_size(handle.rect.center(), Vec2::new(2.0, handle.rect.height())),
                            Dir::Vertical => Rect::from_center_size(handle.rect.center(), Vec2::new(handle.rect.width(), 2.0)),
                        };
                        let color = if response.hovered() || response.dragged() { theme::accent() } else { theme::surface() };
                        ui.painter().rect_filled(line, CornerRadius::same(1), color);
                    }
                    for (path, ratio) in ratios {
                        panes.set_ratio(&path, ratio);
                    }

                    // Un clic en un panel le da el foco.
                    let pressed = ctx.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten());
                    if let Some(position) = pressed {
                        if let Some((id, _)) = leaves.iter().find(|(_, rect)| rect.contains(position)) {
                            *focused = *id;
                        }
                    }

                    for (id, rect) in &leaves {
                        if let Some(pane) = panes.get_mut(*id) {
                            pane.show(ui, *rect, input_enabled && *id == *focused, font_size);
                        }
                    }
                    if leaves.len() > 1 {
                        if let Some((_, rect)) = leaves.iter().find(|(id, _)| id == focused) {
                            ui.painter().rect_stroke(
                                rect.expand(4.0),
                                CornerRadius::same(4),
                                egui::Stroke::new(1.5, identity.unwrap_or_else(theme::accent).gamma_multiply(0.7)),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                    if let Some(pane) = panes.get_mut(*focused) {
                        disconnected = pane.session.is_disconnected();
                        request = pane.take_request();
                    }
                }
                Body::Explorer(explorer) => explorer.show(ui, area.shrink2(Vec2::new(8.0, 8.0))),
            }
            match request {
                Some(PaneRequest::SplitRight) => self.split_active(Dir::Horizontal),
                Some(PaneRequest::SplitDown) => self.split_active(Dir::Vertical),
                Some(PaneRequest::Close) => self.close_focused_pane(),
                None => {}
            }

            let mut title = self.tabs[active].title();
            if let Some(env) = self.environment_for(self.tabs[active].kind) {
                title = format!("{title} · {env}");
            }
            let title = if title.is_empty() { "omni-term".to_string() } else { format!("{title} — omni-term") };
            if title != self.window_title {
                ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
                self.window_title = title;
            }

            if disconnected {
                let mut clicked = false;
                egui::Area::new(egui::Id::NULL.with("reconnect"))
                    .pivot(Align2::CENTER_BOTTOM)
                    .fixed_pos(Pos2::new(area.center().x, area.max.y - 24.0))
                    .show(&ctx, |ui| {
                        Frame::popup(ui.style()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Sesión desconectada");
                                let button = Button::new(RichText::new("Reconectar").color(theme::base())).fill(theme::accent());
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
        self.toasts_ui(&ctx);
        self.persist_layout();
    }
}
