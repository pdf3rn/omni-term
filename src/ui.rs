use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Key, Modifiers, Panel, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Vec2, ViewportCommand,
};
use zeroize::Zeroizing;

use crate::containers::{self, ContainersView};
use crate::cursor::HandExt;
use crate::docker;
use crate::dialogs::{self, CloseAction, CloseConfirm, ConnForm, Dialog, ErrorInfo, Outcome, PaneTarget, Pending, Purpose, Retry, SecretPrompt, SnippetForm, TestState, UnlockPrompt, VarsPrompt};
use crate::layout::{Dir, Tree};
use crate::explorer::Explorer;
use crate::icons;
use crate::known_hosts;
use crate::palette::{self, Command, Palette, Target};
use crate::pane::{self, bold_family, Pane, PaneRequest, PADDING};
use crate::settings::{CursorChoice, Settings, ThemeChoice, DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE};
use crate::secrets::{account_for, SecretError, SecretKind, SecretStore, Secrets};
use crate::ssh::{self, Credentials, JumpHost, TunnelState, Tunnels, UiEvent};
use crate::store::{self, Auth, Connection, Forward, Protocol, SavedTab, Snippet, Store, TabKind};
use crate::telnet;
use crate::term::{self, Session, TermOptions};
use crate::theme;

const SIDEBAR_WIDTH: f32 = 240.0;
const SIDEBAR_MIN: f32 = 200.0;
const SIDEBAR_MAX: f32 = 360.0;
/// Grupos con encabezado a partir de este número de conexiones.
const GROUP_THRESHOLD: usize = 5;
/// Ancho mínimo de una pestaña antes de que la tira empiece a desplazarse.
const MIN_TAB_WIDTH: f32 = 110.0;
/// Por debajo de este ancho de ventana la barra lateral se reduce a una tira de iconos.
const NARROW_WINDOW: f32 = 760.0;
const STRIP_WIDTH: f32 = 54.0;
const TAB_BAR_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 34.0;
const CONTROL_RADIUS: u8 = 6;
const ROW_HEIGHT: f32 = 46.0;

/// Cómo deshacer lo que mostró un aviso.
enum Undo {
    Connection { connection: Connection, forwards: Vec<Forward>, dependents: Vec<i64> },
    Snippet(Snippet),
    /// Entorno y color anteriores de una conexión movida de grupo.
    Environment { id: i64, env: Option<String>, color: Option<u8> },
    /// Líneas de `known_hosts` borradas: posición y texto.
    KnownHosts(Vec<(usize, String)>),
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
    Containers(Box<ContainersView>),
    Settings(dialogs::SettingsTab),
}

struct Tab {
    id: u64,
    kind: TabKind,
    label: String,
    /// Nombre puesto por el usuario; manda sobre el título del terminal.
    custom: Option<String>,
    /// Hubo actividad en segundo plano que el usuario aún no ha visto.
    attention: bool,
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
    /// Qué está haciendo la pestaña que se perdería al cerrarla, si algo.
    fn busy_reason(&self) -> Option<String> {
        match &self.body {
            Body::Containers(_) | Body::Settings(_) => None,
            Body::Explorer(explorer) => {
                let n = explorer.running_transfers();
                (n > 0).then(|| if n == 1 { "1 transferencia en curso".to_string() } else { format!("{n} transferencias en curso") })
            }
            Body::Terminal { panes, .. } => {
                let mut busy = false;
                panes.for_each(&mut |_, pane| {
                    let session = &pane.session;
                    if session.has_exited() || session.is_disconnected() {
                        return;
                    }
                    busy |= match session.is_busy() {
                        Some(busy) => busy,
                        // SSH: no hay forma de saberlo; salida reciente se toma como «ocupado».
                        None => session.activity.lock().unwrap().recent(Duration::from_secs(3)),
                    };
                });
                busy.then(|| "proceso en ejecución".to_string())
            }
        }
    }

    fn pane_mut(&mut self) -> Option<&mut Pane> {
        match &mut self.body {
            Body::Terminal { panes, focused, .. } => panes.get_mut(*focused),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    /// El panel con foco de una pestaña de terminal.
    fn pane(&self) -> Option<&Pane> {
        match &self.body {
            Body::Terminal { panes, focused, .. } => panes.get(*focused),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    fn tunnels(&self) -> Option<Tunnels> {
        match &self.body {
            Body::Terminal { tunnels, .. } => tunnels.clone(),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    /// Título mostrado: el del terminal con foco si lo ha fijado, y si no la etiqueta de la pestaña.
    fn title(&self) -> String {
        if let Some(custom) = &self.custom {
            return custom.clone();
        }
        match &self.body {
            Body::Terminal { .. } => {
                let title = self.pane().map(Pane::title).unwrap_or_default();
                if title.is_empty() { self.label.clone() } else { title }
            }
            Body::Explorer(explorer) => explorer.label(),
            Body::Containers(view) => view.label(),
            Body::Settings(_) => "Ajustes".to_string(),
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
    window_maximized: bool,
    window_pos: (i32, i32),
    window_restored: bool,
    window_boot: std::time::Instant,
    grid: (usize, usize),
    ctx: egui::Context,
    runtime: tokio::runtime::Runtime,
    store: Arc<Mutex<Store>>,
    secrets: Secrets,
    connections: Vec<Connection>,
    forwards: HashMap<i64, Vec<Forward>>,
    snippets: Vec<Snippet>,
    sidebar_view: SidebarView,
    /// Búsqueda de la barra lateral; filtra conexiones o snippets según la vista.
    sidebar_query: String,
    sidebar_collapsed: bool,
    /// Pestaña que se está renombrando: índice, texto y si falta darle el foco.
    renaming: Option<(usize, String, bool)>,
    palette: Option<Palette>,
    close_confirmed: bool,
    known_hosts: dialogs::KnownHostsView,
    /// Hosts de `~/.ssh/config` que aún no están guardados.
    ssh_config_hosts: usize,
    loaded_font: Option<String>,
    /// Ids de conexión, de la usada más recientemente a la más antigua.
    recent: Vec<(i64, u64)>,
    pinned: Vec<i64>,
    /// El foco está en la lista del sidebar (flechas, Enter, F2, Supr).
    sidebar_nav: bool,
    nav_index: usize,
    sidebar_rect: Rect,
    sidebar_epoch: u32,
    plus_selected: usize,
    open_plus_menu: bool,
    /// Última pestaña a la que se desplazó la tira de pestañas.
    tab_strip_seen: u64,
    /// La barra de menú oculta se muestra de paso (tras pulsar Alt).
    menu_peek: bool,
    /// Alt pulsado: `true` si ya se pulsó otra tecla mientras tanto.
    alt_state: Option<bool>,
    sidebar_width: f32,
    focus_search: bool,
    /// Grupos de entorno plegados (en minúsculas).
    collapsed_groups: HashSet<String>,
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
        let store_meta_flag = matches!(store.lock().unwrap().meta("window_maximized"), Ok(Some(v)) if v == b"1");
        let secrets = Secrets::select(store.clone());
        let recent: Vec<(i64, u64)> = match store.lock().unwrap().meta("recent_connections") {
            Ok(Some(bytes)) => parse_recent(&String::from_utf8_lossy(&bytes)),
            _ => Vec::new(),
        };
        let pinned: Vec<i64> = match store.lock().unwrap().meta("pinned_connections") {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).split(',').filter_map(|id| id.trim().parse().ok()).collect(),
            _ => Vec::new(),
        };
        let settings = Settings::load(&store.lock().unwrap());
        let font_error = install_fonts(&cc.egui_ctx, settings.font_path.as_deref());
        theme::set_accent(settings.accent.index());
        theme::set_compact(settings.compact);
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
        let (sidebar_collapsed, sidebar_width, collapsed_groups) = {
            let store = store.lock().unwrap();
            let collapsed = matches!(store.meta("sidebar_collapsed"), Ok(Some(v)) if v == b"1");
            let width = match store.meta("sidebar_width") {
                Ok(Some(v)) => String::from_utf8_lossy(&v).parse::<f32>().unwrap_or(SIDEBAR_WIDTH),
                _ => SIDEBAR_WIDTH,
            };
            let groups = match store.meta("collapsed_groups") {
                Ok(Some(v)) => String::from_utf8_lossy(&v).lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
                _ => HashSet::new(),
            };
            (collapsed, width.clamp(SIDEBAR_MIN, SIDEBAR_MAX), groups)
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
            window_maximized: matches!(store_meta_flag, true),
            window_pos: (i32::MIN, i32::MIN),
            window_restored: false,
            window_boot: std::time::Instant::now(),
            grid: (80, 24),
            ctx: cc.egui_ctx.clone(),
            runtime,
            store,
            secrets,
            connections,
            forwards,
            snippets,
            sidebar_view,
            sidebar_query: String::new(),
            sidebar_collapsed,
            renaming: None,
            palette: None,
            close_confirmed: false,
            known_hosts: Default::default(),
            ssh_config_hosts: 0,
            loaded_font: None,
            recent,
            pinned,
            sidebar_nav: false,
            nav_index: 0,
            sidebar_rect: Rect::NOTHING,
            sidebar_epoch: 0,
            plus_selected: 0,
            open_plus_menu: false,
            tab_strip_seen: 0,
            menu_peek: false,
            alt_state: None,
            sidebar_width,
            focus_search: false,
            collapsed_groups,
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
        app.ssh_config_hosts = pending_ssh_config_hosts(&app.connections);
        app.loaded_font = app.settings.font_path.clone();
        if let Some(problem) = font_error {
            app.settings.font_path = None;
            app.loaded_font = None;
            app.notify(problem);
        }
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

    fn telnet_session(&mut self, connection: Connection) -> anyhow::Result<Session> {
        let wake = self.wake();
        let handle = self.runtime.handle().clone();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        Session::start(cols, rows, options, move || wake(), move |feed| Ok(telnet::spawn(&handle, connection, (cols, rows), feed)))
    }

    fn adb_session(&mut self, connection: Connection) -> anyhow::Result<Session> {
        let wake = self.wake();
        let handle = self.runtime.handle().clone();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        Session::start(cols, rows, options, move || wake(), move |feed| Ok(crate::adb::spawn(&handle, connection, (cols, rows), feed)))
    }

    fn serial_session(&mut self, connection: Connection) -> anyhow::Result<Session> {
        let wake = self.wake();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        Session::start(cols, rows, options, move || wake(), move |feed| Ok(crate::serial::spawn(&connection, feed)))
    }

    /// ¿La pestaña activa es una conexión con explorador de archivos (SSH o un dispositivo Android)?
    fn active_has_explorer(&self) -> bool {
        match self.tabs.get(self.active).map(|t| t.kind) {
            Some(TabKind::Connection(id)) => self.connections.iter().any(|c| c.id == Some(id) && (c.is_ssh() || c.is_adb())),
            _ => false,
        }
    }

    /// Sesión de terminal de una conexión: SSH, Telnet, puerto serie o Android.
    fn connection_session(&mut self, connection: Connection, credentials: Credentials, tunnels: Tunnels) -> anyhow::Result<Session> {
        if connection.is_serial() {
            self.serial_session(connection)
        } else if connection.is_adb() {
            self.adb_session(connection)
        } else if connection.is_telnet() {
            self.telnet_session(connection)
        } else {
            self.ssh_session(connection, credentials, tunnels)
        }
    }

    fn open_local(&mut self) -> anyhow::Result<()> {
        let session = self.local_session()?;
        let id = self.take_tab_id();
        let body = Body::terminal(Pane::new(session, self.debug_keys, 1), None);
        self.tabs.push(Tab { id, kind: TabKind::Local, label: "Terminal local".into(), custom: None, attention: false, body });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Abre la pestaña de ajustes en la sección pedida, o vuelve a ella si ya está abierta.
    fn open_settings(&mut self, section: dialogs::SettingsTab) {
        self.known_hosts.loaded = false;
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Settings) {
            self.tabs[index].body = Body::Settings(section);
            self.active = index;
            return;
        }
        let id = self.take_tab_id();
        self.tabs.push(Tab { id, kind: TabKind::Settings, label: "Ajustes".into(), custom: None, attention: false, body: Body::Settings(section) });
        self.active = self.tabs.len() - 1;
    }

    /// Abre el administrador de contenedores, o vuelve a él si ya hay uno abierto.
    fn open_containers(&mut self) {
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Containers) {
            self.active = index;
            return;
        }
        let view = ContainersView::new(self.runtime.handle().clone(), self.wake());
        let id = self.take_tab_id();
        self.tabs.push(Tab { id, kind: TabKind::Containers, label: "Contenedores".into(), custom: None, attention: false, body: Body::Containers(Box::new(view)) });
        self.active = self.tabs.len() - 1;
    }

    /// Terminal dentro de un contenedor, en una pestaña nueva.
    fn open_container_terminal(&mut self, endpoint: docker::Endpoint, container: &docker::Container) {
        let wake = self.wake();
        let handle = self.runtime.handle().clone();
        let (cols, rows) = self.grid;
        let options = self.term_options();
        let (id, name) = (container.id.clone(), container.name.clone());
        let started = Session::start(cols, rows, options, move || wake(), move |feed| Ok(docker::spawn(&handle, endpoint, id, name.clone(), (cols, rows), feed)));
        match started {
            Ok(session) => {
                let tab_id = self.take_tab_id();
                let body = Body::terminal(Pane::new(session, self.debug_keys, 1), None);
                self.tabs.push(Tab { id: tab_id, kind: TabKind::Container, label: container.name.clone(), custom: None, attention: false, body });
                self.active = self.tabs.len() - 1;
            }
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo abrir el terminal", format!("{error:#}"))),
        }
    }

    /// Ejecuta una acción confirmada sobre un contenedor con el administrador abierto.
    fn container_action(&mut self, confirm: dialogs::ContainerConfirm) {
        let view = self.tabs.iter().find_map(|t| match &t.body {
            Body::Containers(view) => Some(view),
            _ => None,
        });
        if let Some(view) = view {
            view.act(confirm.endpoint, &confirm.container, confirm.action);
        }
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
        let session = self.connection_session(connection, credentials, tunnels.clone())?;
        let tab_id = slot.unwrap_or_else(|| self.take_tab_id());
        let body = Body::terminal(Pane::new(session, self.debug_keys, 1), Some(tunnels));
        let mut tab = Tab { id: tab_id, kind: TabKind::Connection(id), label, custom: None, attention: false, body };
        match slot.and_then(|slot| self.tabs.iter().position(|t| t.id == slot)) {
            Some(index) => {
                tab.custom = self.tabs[index].custom.take();
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
        let session = self.connection_session(connection, credentials, ssh::new_tunnels(&[]))?;
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
            TabKind::Explorer(_) | TabKind::Containers | TabKind::Container | TabKind::Settings => {}
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
        self.tabs.push(Tab { id: tab_id, kind: TabKind::Explorer(id), label, custom: None, attention: false, body: Body::Explorer(Box::new(explorer)) });
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
        // Las conexiones de archivos no tienen terminal: abrirlas es abrir su explorador.
        let purpose = if connection.is_files() && matches!(purpose, Purpose::Terminal) { Purpose::Explorer(None, None) } else { purpose };
        if connection.is_serial() {
            if matches!(purpose, Purpose::Explorer(..)) {
                self.notify("Un puerto serie no tiene explorador de archivos");
            } else {
                self.finish_open(connection, Credentials::default(), slot, purpose);
            }
            return;
        }
        if let Some(label) = connection.insecure_protocol() {
            let quiet = match connection.protocol {
                Protocol::Telnet(options) => options.quiet,
                Protocol::Files(store::FilesKind::Ftp(options)) => options.quiet,
                _ => true,
            };
            let reconnecting = slot.is_some() || matches!(purpose, Purpose::Pane { target: PaneTarget::Replace(_), .. });
            if !quiet && !reconnecting {
                self.dialog = Some(Dialog::TelnetWarning(dialogs::TelnetWarning { connection, slot, purpose, dont_ask: false, label }));
                return;
            }
        }
        self.continue_open(connection, slot, purpose);
    }

    /// Pide lo que falte (contraseñas) y abre la conexión.
    fn continue_open(&mut self, connection: Connection, slot: Option<u64>, purpose: Purpose) {
        if connection.is_telnet() {
            // Telnet no tiene credenciales guardadas ni explorador de archivos.
            if matches!(purpose, Purpose::Explorer(..)) {
                self.notify("Telnet no tiene explorador de archivos");
            } else {
                self.finish_open(connection, Credentials::default(), slot, purpose);
            }
            return;
        }
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

    /// Anota la conexión como la usada más recientemente (solo aperturas nuevas, no reconexiones).
    fn touch_recent(&mut self, id: i64) {
        self.recent.retain(|(r, _)| *r != id);
        self.recent.insert(0, (id, now_secs()));
        self.recent.truncate(20);
        let text = self.recent.iter().map(|(id, at)| format!("{id}:{at}")).collect::<Vec<_>>().join(",");
        let _ = self.store.lock().unwrap().set_meta("recent_connections", text.as_bytes());
    }

    fn finish_open(&mut self, connection: Connection, credentials: Credentials, slot: Option<u64>, purpose: Purpose) {
        if slot.is_none() && !matches!(purpose, Purpose::Pane { .. }) {
            if let Some(id) = connection.id {
                self.touch_recent(id);
            }
        }
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
        drop(store);
        self.ssh_config_hosts = pending_ssh_config_hosts(&self.connections);
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
            TabKind::Local | TabKind::Containers | TabKind::Container | TabKind::Settings => None,
        }
    }

    fn environment_for(&self, kind: TabKind) -> Option<String> {
        match kind {
            TabKind::Connection(id) | TabKind::Explorer(id) => {
                self.connections.iter().find(|c| c.id == Some(id)).and_then(|c| c.env.clone())
            }
            TabKind::Local | TabKind::Containers | TabKind::Container | TabKind::Settings => None,
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

    fn toggle_pin(&mut self, id: i64) {
        if let Some(position) = self.pinned.iter().position(|p| *p == id) {
            self.pinned.remove(position);
        } else {
            self.pinned.push(id);
        }
        let text = self.pinned.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let _ = self.store.lock().unwrap().set_meta("pinned_connections", text.as_bytes());
    }

    /// Cambia el entorno (y el color) de una conexión al soltarla sobre otro grupo, con deshacer.
    fn move_to_group(&mut self, id: i64, label: &str) {
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
    fn forget_known_hosts(&mut self, entries: Vec<known_hosts::KnownHost>, reconnect: Option<(String, u16)>) {
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
    fn reconnect_to(&mut self, host: &str, port: u16) {
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
                                if toast.undo.is_some() && ui.add(Button::new(RichText::new("Deshacer").color(theme::accent())).frame_when_inactive(false)).clicked() {
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
        if self.settings.font_path != self.loaded_font {
            if let Some(problem) = install_fonts(&self.ctx, self.settings.font_path.as_deref()) {
                self.settings.font_path = None;
                self.notify(problem);
            }
            self.loaded_font = self.settings.font_path.clone();
        }
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
        let (accent, compact) = (self.settings.accent.index(), self.settings.compact);
        if light != theme::is_light() || accent != theme::accent_index() || compact != theme::is_compact() {
            theme::set_light(light);
            theme::set_accent(accent);
            theme::set_compact(compact);
            install_visuals(ctx);
        }
    }

    /// Recuerda tamaño, posición y si estaba maximizada, para la próxima vez. El tamaño y la posición
    /// guardados son los de la ventana normal, no los de maximizada ni pantalla completa.
    fn remember_window_size(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized, fullscreen) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.maximized.unwrap_or(false), v.fullscreen.unwrap_or(false))
        });
        // Al arrancar, el gestor de ventanas aún está aplicando lo guardado: no se anota nada todavía.
        if fullscreen || self.window_boot.elapsed() < Duration::from_secs(2) {
            return;
        }
        if maximized != self.window_maximized {
            self.window_maximized = maximized;
            let _ = self.store.lock().unwrap().set_meta("window_maximized", if maximized { b"1" } else { b"0" });
        }
        if maximized || self.window_saved_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        if let Some(rect) = inner {
            let size = (rect.width().round() as u32, rect.height().round() as u32);
            if size != self.window_saved {
                self.window_saved = size;
                self.window_saved_at = std::time::Instant::now();
                let _ = self.store.lock().unwrap().set_meta("window_size", format!("{}x{}", size.0, size.1).as_bytes());
            }
        }
        if let Some(rect) = outer {
            let pos = (rect.min.x.round() as i32, rect.min.y.round() as i32);
            if pos != self.window_pos {
                self.window_pos = pos;
                self.window_saved_at = std::time::Instant::now();
                let _ = self.store.lock().unwrap().set_meta("window_pos", format!("{},{}", pos.0, pos.1).as_bytes());
            }
        }
    }

    /// Devuelve la ventana a donde estaba, solo si esa posición cae en el monitor actual.
    fn restore_window_position(&mut self, ctx: &egui::Context) {
        // Algunos gestores ignoran maximizar si llega pronto: se repite hasta que la ventana lo confirma.
        if self.window_maximized && self.window_boot.elapsed() < Duration::from_secs(2) {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if !maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        if self.window_restored {
            return;
        }
        let (monitor, maximized) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().maximized.unwrap_or(false)));
        let Some(monitor) = monitor else { return };
        self.window_restored = true;
        if self.window_maximized {
            if !maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            }
            return;
        }
        if maximized {
            return;
        }
        let saved = match self.store.lock().unwrap().meta("window_pos") {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            _ => return,
        };
        let size = ctx.input(|i| i.viewport().inner_rect).map_or(egui::vec2(960.0, 600.0), |r| r.size());
        if let Some(pos) = parse_window_position(&saved, monitor, size) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
    }

    fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (size - self.settings.font_size).abs() > f32::EPSILON {
            self.settings.font_size = size;
            let _ = self.settings.save(&self.store.lock().unwrap());
        }
    }

    /// Pestañas afectadas por una acción de cierre, y entre ellas las que tienen algo en marcha.
    fn close_busy_lines(&self, action: CloseAction) -> Vec<String> {
        let affected: Vec<usize> = match action {
            CloseAction::Tab(id) => self.tabs.iter().position(|t| t.id == id).into_iter().collect(),
            CloseAction::Others(id) => (0..self.tabs.len()).filter(|i| self.tabs[*i].id != id).collect(),
            CloseAction::Right(id) => match self.tabs.iter().position(|t| t.id == id) {
                Some(position) => ((position + 1)..self.tabs.len()).collect(),
                None => Vec::new(),
            },
            CloseAction::Window => (0..self.tabs.len()).collect(),
        };
        affected
            .into_iter()
            .filter_map(|i| self.tabs[i].busy_reason().map(|reason| format!("«{}»: {reason}", self.tabs[i].title())))
            .collect()
    }

    /// Cierra directamente o, si hay algo en ejecución y no se ha desactivado, pide confirmación.
    fn request_close(&mut self, action: CloseAction) {
        let lines = if self.settings.confirm_close { self.close_busy_lines(action) } else { Vec::new() };
        if lines.is_empty() {
            self.perform_close(action);
        } else if self.dialog.is_none() {
            self.dialog = Some(Dialog::ConfirmClose(CloseConfirm { action, lines, dont_ask: false }));
        }
    }

    fn perform_close(&mut self, action: CloseAction) {
        match action {
            CloseAction::Tab(id) => {
                if let Some(position) = self.tabs.iter().position(|t| t.id == id) {
                    self.close_tab(position);
                }
            }
            CloseAction::Others(id) => {
                for index in (0..self.tabs.len()).rev() {
                    if self.tabs[index].id != id {
                        self.close_tab(index);
                    }
                }
                self.active = 0;
            }
            CloseAction::Right(id) => {
                if let Some(position) = self.tabs.iter().position(|t| t.id == id) {
                    for index in ((position + 1)..self.tabs.len()).rev() {
                        self.close_tab(index);
                    }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                }
            }
            CloseAction::Window => {
                self.close_confirmed = true;
                self.ctx.send_viewport_cmd(ViewportCommand::Close);
            }
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

    /// Pantalla sin pestañas: qué se puede hacer a continuación. Devuelve `true` si se pide un terminal local.
    fn empty_state(&self, ui: &mut egui::Ui, area: Rect) -> Option<EmptyAction> {
        let mut action = None;
        let width = 480.0_f32.min(area.width());
        // Centrado si hay sitio; en ventanas bajas empieza arriba y se desplaza.
        let top = if area.height() > 640.0 { area.center().y - 290.0 } else { area.min.y + 8.0 };
        let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
            Pos2::new(area.center().x - width / 2.0, top),
            Pos2::new(area.center().x + width / 2.0, area.max.y),
        )));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut outer, |child| {
        child.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        child.label(RichText::new("No hay pestañas abiertas").font(FontId::new(20.0, strong_family())));
        child.label(RichText::new("Abre un terminal o conéctate a un servidor guardado.").color(theme::subtext()));
        child.add_space(4.0);
        child.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            // Outline: el acento marca la acción principal en el borde y el texto, sin relleno.
            if outline_button(ui, RichText::new("Nuevo terminal local").size(13.0).color(theme::accent()), true).clicked() {
                action = Some(EmptyAction::Local);
            }
            if outline_button(ui, RichText::new("Nueva conexión").size(13.0), false).clicked() {
                action = Some(EmptyAction::NewConnection);
            }
            if outline_button(ui, RichText::new("Contenedores").size(13.0), false).clicked() {
                action = Some(EmptyAction::Containers);
            }
        });

        child.add_space(12.0);
        let recents: Vec<Connection> = self.recent_connections().into_iter().take(4).collect();
        if recents.is_empty() {
            child.label(RichText::new("Aún no hay conexiones guardadas.").color(theme::subtext()));
            if outline_button(child, RichText::new("Importar de ~/.ssh/config").size(13.0), false).clicked() {
                action = Some(EmptyAction::Import);
            }
        } else {
            child.label(RichText::new("Recientes").font(FontId::new(12.5, strong_family())).color(theme::subtext()));
            Frame::new().fill(theme::mantle()).stroke(Stroke::new(1.0, theme::surface())).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(6, 4)).show(child, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(ui.available_width());
                for connection in &recents {
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
                    let response = response.hand();
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::surface());
                    }
                    let glyph = if connection.is_files() {
                        icons::FOLDER
                    } else if connection.is_adb() {
                        icons::DOTS
                    } else {
                        icons::TERMINAL
                    };
                    ui.painter().text(Pos2::new(rect.min.x + 18.0, rect.center().y), Align2::CENTER_CENTER, glyph, FontId::proportional(17.0), theme::subtext());
                    let clip = ui.painter().with_clip_rect(rect);
                    let name_font = FontId::new(14.0, strong_family());
                    let name = fit_text(&clip, &connection.name, &name_font, rect.width() - 48.0);
                    clip.text(Pos2::new(rect.min.x + 38.0, rect.min.y + 7.0), Align2::LEFT_TOP, name, name_font, theme::text());
                    let detail_font = FontId::new(12.0, FontFamily::Proportional);
                    let detail = fit_text(&clip, &format!("{}  ·  {}", connection.kind_label(), connection.destination()), &detail_font, rect.width() - 48.0);
                    clip.text(Pos2::new(rect.min.x + 38.0, rect.min.y + 25.0), Align2::LEFT_TOP, detail, detail_font, theme::subtext());
                    if response.on_hover_text("Conectar").clicked() {
                        action = Some(EmptyAction::Connect(connection.clone()));
                    }
                }
            });
        }

        child.add_space(12.0);
        child.label(RichText::new("Atajos").font(FontId::new(12.5, strong_family())).color(theme::subtext()));
        for (keys, what) in [
            ("Ctrl+Shift+P", "Paleta de comandos"),
            ("Ctrl+Shift+D / E", "Dividir el panel a la derecha / hacia abajo"),
            ("Ctrl+Shift+B", "Mostrar u ocultar la barra lateral"),
            ("Ctrl+,", "Abrir los ajustes"),
        ] {
            child.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(210.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(210.0);
                    for (n, combo) in keys.split(" / ").enumerate() {
                        if n > 0 {
                            ui.label(RichText::new("/").color(theme::subtext()));
                        }
                        dialogs::keycap(ui, combo);
                    }
                });
                ui.label(RichText::new(what).color(theme::subtext()));
            });
        }
        if child.add(Button::new(RichText::new("Ver todos los atajos").color(theme::accent())).frame(false)).clicked() {
            action = Some(EmptyAction::Shortcuts);
        }
        });
        action
    }

    /// Hasta 5 conexiones para el menú del «+»: las últimas usadas y, si faltan, las demás por nombre.
    fn recent_connections(&self) -> Vec<Connection> {
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

    fn palette_items(&self) -> Vec<palette::Item> {
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

    fn run_palette(&mut self, target: Target) {
        match target {
            Target::Connect(connection) => self.begin_open(connection, None, Purpose::Terminal),
            Target::Snippet(snippet) => self.insert_snippet(&snippet, false),
            Target::Tab(index) => self.active = index.min(self.tabs.len().saturating_sub(1)),
            Target::Command(command) => self.run_command(command),
        }
    }

    fn run_command(&mut self, command: Command) {
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

    /// Barra de menú superior. Devuelve el comando elegido o la conexión reciente que se pidió abrir.
    fn menu_bar(&mut self, ui: &mut egui::Ui) -> (Option<Command>, Option<Connection>) {
        if !self.settings.menu_bar && !self.menu_peek {
            return (None, None);
        }
        let tab = self.tabs.get(self.active);
        let pane = tab.and_then(|t| t.pane());
        let has_terminal = pane.is_some();
        let has_selection = pane.is_some_and(|p| p.selection_text().is_some());
        let on_connection = matches!(tab.map(|t| t.kind), Some(TabKind::Connection(_)));
        let several = self.tabs.len() > 1;
        let recents = self.recent_connections();
        let mut chosen: Option<Command> = None;
        let mut connect: Option<Connection> = None;
        Panel::top("menubar")
            .exact_size(30.0)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin::symmetric(8, 3)))
            .show(ui, |ui| {
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                let mut item = |ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool, command: Command| {
                    let mut button = Button::new(label);
                    if !shortcut.is_empty() {
                        button = button.shortcut_text(shortcut);
                    }
                    if ui.add_enabled(enabled, button).clicked() {
                        chosen = Some(command);
                        ui.close();
                    }
                };
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("Archivo", |ui| {
                        ui.set_min_width(300.0);
                        item(ui, "Nueva pestaña local", "Ctrl+Shift+T", true, Command::NewTab);
                        item(ui, "Nueva conexión…", "", true, Command::NewConnection);
                        item(ui, "Importar de ~/.ssh/config", "", true, Command::ImportConfig);
                        ui.separator();
                        item(ui, "Cerrar pestaña", "", !self.tabs.is_empty(), Command::CloseTab);
                        item(ui, "Ajustes", "Ctrl+,", true, Command::Settings);
                        item(ui, "Hosts conocidos", "", true, Command::KnownHosts);
                        ui.separator();
                        item(ui, "Salir", "", true, Command::Quit);
                    });
                    ui.menu_button("Editar", |ui| {
                        ui.set_min_width(300.0);
                        item(ui, "Copiar", "Ctrl+Shift+C", has_selection, Command::Copy);
                        item(ui, "Pegar", "Ctrl+Shift+V", has_terminal, Command::Paste);
                        item(ui, "Seleccionar todo", "", has_terminal, Command::SelectAll);
                        ui.separator();
                        item(ui, "Buscar en la salida", "Ctrl+Shift+F", has_terminal, Command::Search);
                        item(ui, "Guardar selección como snippet", "", has_selection, Command::SaveSnippet);
                    });
                    ui.menu_button("Ver", |ui| {
                        ui.set_min_width(300.0);
                        item(ui, "Barra lateral", "Ctrl+Shift+B", true, Command::ToggleSidebar);
                        item(ui, "Pantalla completa", "F11", true, Command::Fullscreen);
                        ui.separator();
                        item(ui, "Dividir a la derecha", "Ctrl+Shift+D", has_terminal, Command::SplitRight);
                        item(ui, "Dividir hacia abajo", "Ctrl+Shift+E", has_terminal, Command::SplitDown);
                        ui.separator();
                        item(ui, "Aumentar la letra", "Ctrl+=", true, Command::ZoomIn);
                        item(ui, "Reducir la letra", "Ctrl+−", true, Command::ZoomOut);
                        item(ui, "Restablecer la letra", "Ctrl+0", true, Command::ZoomReset);
                        ui.separator();
                        item(ui, "Tema oscuro", "", true, Command::ThemeDark);
                        item(ui, "Tema claro", "", true, Command::ThemeLight);
                        item(ui, "Tema del sistema", "", true, Command::ThemeSystem);
                    });
                    ui.menu_button("Ir", |ui| {
                        ui.set_min_width(300.0);
                        item(ui, "Paleta de comandos", "Ctrl+Shift+P", true, Command::Palette);
                        ui.separator();
                        item(ui, "Pestaña siguiente", "Ctrl+Tab", several, Command::NextTab);
                        item(ui, "Pestaña anterior", "Ctrl+Mayús+Tab", several, Command::PrevTab);
                        item(ui, "Explorador de archivos de esta conexión", "", on_connection, Command::Files);
                        item(ui, "Contenedores", "", true, Command::Containers);
                        ui.separator();
                        ui.add_enabled_ui(!recents.is_empty(), |ui| {
                            ui.menu_button("Conexiones recientes", |ui| {
                                for connection in &recents {
                                    let label = format!("{}   {}", connection.name, connection.target());
                                    if ui.button(label).clicked() {
                                        connect = Some(connection.clone());
                                        ui.close();
                                    }
                                }
                            });
                        });
                    });
                    ui.menu_button("Ayuda", |ui| {
                        ui.set_min_width(260.0);
                        item(ui, "Atajos de teclado", "", true, Command::Shortcuts);
                        item(ui, "Acerca de omni-term", "", true, Command::About);
                    });
                });
            });
        (chosen, connect)
    }

    /// Marca las pestañas en segundo plano cuyo trabajo terminó y, con la ventana sin foco, avisa al sistema.
    fn watch_activity(&mut self, ctx: &egui::Context) {
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

    /// Barra lateral reducida a iconos: terminal local, una insignia por conexión, buscar, nueva y ajustes.
    fn sidebar_strip(&mut self, ui: &mut egui::Ui, connect: &mut Option<Connection>, open_settings: &mut bool, new_connection: &mut bool) {
        let mut palette = false;
        Panel::left("sidebar-strip")
            .exact_size(STRIP_WIDTH)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin::symmetric(9, 12)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let size = Vec2::splat(36.0);
                let icon = |ui: &mut egui::Ui, glyph: &str, tip: &str| -> bool {
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                    let response = response.hand();
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::surface());
                    }
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(18.0), theme::text());
                    response.on_hover_text(tip).clicked()
                };
                if icon(ui, icons::SEARCH, "Buscar (Ctrl+Shift+P)") {
                    palette = true;
                }
                ui.add_space(4.0);
                for connection in &self.connections {
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                    let response = response.hand();
                    let color = connection.color.map(theme::identity);
                    let fill = if response.hovered() { theme::selection() } else { theme::surface() };
                    ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), fill);
                    if let Some(color) = color {
                        let bar = Rect::from_min_size(Pos2::new(rect.min.x - 6.0, rect.min.y + 7.0), Vec2::new(3.0, size.y - 14.0));
                        ui.painter().rect_filled(bar, CornerRadius::same(2), color);
                    }
                    let initial = connection.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, initial, FontId::new(15.0, strong_family()), theme::text());
                    let tip = format!("{}  ({})", connection.name, connection.target());
                    if response.on_hover_text(tip).clicked() {
                        *connect = Some(connection.clone());
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if icon(ui, icons::GEAR, "Ajustes") {
                        *open_settings = true;
                    }
                    if icon(ui, icons::PLUS, "Nueva conexión") {
                        *new_connection = true;
                    }
                });
            });
        if palette && self.dialog.is_none() {
            self.palette = Some(Palette::default());
        }
    }

    /// Alt suelto (pulsado y soltado sin otra tecla) muestra u oculta la barra de menú oculta.
    fn track_alt(&mut self, ctx: &egui::Context) {
        let (alt, typed, escape, clicked_below) = ctx.input(|i| {
            let typed = i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_)));
            let escape = i.key_pressed(Key::Escape);
            let below = i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| p.y > 40.0);
            (i.modifiers.alt, typed, escape, below)
        });
        match (alt, self.alt_state) {
            (true, None) => self.alt_state = Some(false),
            (true, Some(_)) if typed => self.alt_state = Some(true),
            (false, Some(false)) if !self.settings.menu_bar => {
                self.menu_peek = !self.menu_peek;
                self.alt_state = None;
            }
            (false, _) => self.alt_state = None,
            _ => {}
        }
        if self.menu_peek && (escape || clicked_below) {
            self.menu_peek = false;
        }
    }

    fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        if self.sidebar_collapsed != collapsed {
            self.sidebar_collapsed = collapsed;
            let _ = self.store.lock().unwrap().set_meta("sidebar_collapsed", if collapsed { b"1" } else { b"0" });
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

    /// Dibuja la página de ajustes y aplica lo que el usuario haya cambiado, al instante.
    fn settings_page(&mut self, ui: &mut egui::Ui, area: Rect, mut section: dialogs::SettingsTab) -> dialogs::SettingsTab {
        let mut forced = Secrets::is_master_forced(&self.store.lock().unwrap());
        let (forced_before, settings_before) = (forced, self.settings.clone());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
        dialogs::settings_page(&mut child, &mut section, &mut self.settings, &mut forced, &mut self.known_hosts, &self.connections);
        if let Some(entry) = self.known_hosts.delete.take() {
            self.forget_known_hosts(vec![entry], None);
        }
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
        section
    }

    fn handle_dialog(&mut self, ctx: &egui::Context) {
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
            (Outcome::Cancel, Dialog::Secret(_) | Dialog::ConfirmContainer(_) | Dialog::Unlock(_) | Dialog::Edit(_) | Dialog::Error(_) | Dialog::HostKeyChanged(_) | Dialog::About | Dialog::ConfirmClose(_) | Dialog::TelnetWarning(_) | Dialog::Snippet(_) | Dialog::SnippetVars(_) | Dialog::ConfirmReset(_)) => {}
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
                let Some(master) = self.secrets.master().cloned() else { return };
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
        let mut row_action: Option<(RowAction, Connection)> = None;
        let mut toggle_group: Option<String> = None;
        let searching = !self.sidebar_query.trim().is_empty();
        let headed = self.connections.len() >= GROUP_THRESHOLD || !self.pinned.is_empty();
        let groups = sidebar_groups(&self.connections, &self.sidebar_query, &self.pinned);
        let now = now_secs();
        let mut open_tab: Option<usize> = None;
        let mut drop_move: Option<(i64, String)> = None;
        let mut nav_moved = false;
        let folded_in = |app: &App, label: &str| headed && !searching && app.collapsed_groups.contains(&label.to_lowercase());
        let mut nav_rows: Vec<NavRow> = Vec::new();
        for (label, indices) in &groups {
            if headed {
                nav_rows.push(NavRow::Header(label.to_lowercase()));
            }
            if !folded_in(self, label) {
                nav_rows.extend(indices.iter().map(|i| NavRow::Conn(*i)));
            }
        }
        // Clic fuera del sidebar: la lista deja de tener el foco del teclado.
        if self.sidebar_nav {
            let outside = self.ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !self.sidebar_rect.contains(p)));
            if outside || self.dialog.is_some() || self.palette.is_some() || self.sidebar_view != SidebarView::Connections || nav_rows.is_empty() {
                self.sidebar_nav = false;
            }
        }
        if self.sidebar_nav {
            self.nav_index = self.nav_index.min(nav_rows.len() - 1);
            let (down, up, left, right, enter, f2, delete_key, escape) = self.ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                    i.consume_key(Modifiers::NONE, Key::ArrowRight),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::F2),
                    i.consume_key(Modifiers::NONE, Key::Delete),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if down {
                self.nav_index = (self.nav_index + 1) % nav_rows.len();
                nav_moved = true;
            }
            if up {
                self.nav_index = (self.nav_index + nav_rows.len() - 1) % nav_rows.len();
                nav_moved = true;
            }
            if escape {
                self.sidebar_nav = false;
            }
            match &nav_rows[self.nav_index] {
                NavRow::Header(key) => {
                    let folded = self.collapsed_groups.contains(key);
                    if enter || (left && !folded) || (right && folded) {
                        toggle_group = Some(key.clone());
                    }
                }
                NavRow::Conn(index) => {
                    let connection = self.connections[*index].clone();
                    if left && headed {
                        if let Some(header) = nav_rows[..self.nav_index].iter().rposition(|r| matches!(r, NavRow::Header(_))) {
                            self.nav_index = header;
                            nav_moved = true;
                        }
                    } else if enter {
                        row_action = Some((RowAction::Connect, connection));
                    } else if f2 {
                        row_action = Some((RowAction::Edit, connection));
                    } else if delete_key {
                        row_action = Some((RowAction::Delete, connection));
                    }
                }
            }
        }
        let keyring_fallback = self.secrets.master().is_some() && !Secrets::is_master_forced(&self.store.lock().unwrap());
        let narrow_window = ctx_width(&self.ctx) < NARROW_WINDOW;
        if !self.sidebar_collapsed && narrow_window {
            self.sidebar_strip(ui, &mut connect, &mut open_settings, &mut new_connection);
        } else if !self.sidebar_collapsed {
            let panel = Panel::left(egui::Id::NULL.with(("sidebar", self.sidebar_epoch)))
                .default_size(self.sidebar_width)
                .size_range(SIDEBAR_MIN..=SIDEBAR_MAX)
                .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin::same(14)))
                .show(ui, |ui| {
                    let connections_view = self.sidebar_view == SidebarView::Connections;
                    let search_id = ui.make_persistent_id("sidebar_search");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.sidebar_query)
                            .id(search_id)
                            .hint_text(if connections_view { "Buscar conexiones" } else { "Buscar snippets" })
                            .margin(Margin { left: 30, right: 8, top: 5, bottom: 5 })
                            .desired_width(f32::INFINITY),
                    );
                    ui.painter().text(
                        Pos2::new(search.rect.min.x + 15.0, search.rect.center().y),
                        Align2::CENTER_CENTER,
                        icons::SEARCH,
                        FontId::proportional(15.0),
                        theme::subtext(),
                    );
                    if self.focus_search {
                        search.request_focus();
                        self.focus_search = false;
                    }
                    if search.has_focus() && connections_view && !nav_rows.is_empty() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                        self.sidebar_nav = true;
                        self.nav_index = 0;
                        search.surrender_focus();
                    }
                    if search.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                        self.sidebar_query.clear();
                        search.surrender_focus();
                    }
                    ui.add_space(8.0);

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().button_padding.x = 0.0;
                        ui.spacing_mut().item_spacing.x = 18.0;
                        for (view, label) in [(SidebarView::Connections, "Conexiones"), (SidebarView::Snippets, "Snippets")] {
                            let selected = self.sidebar_view == view;
                            let color = if selected { theme::text() } else { theme::subtext() };
                            let font = FontId::new(13.0, if selected { strong_family() } else { FontFamily::Proportional });
                            let button = Button::new(RichText::new(label).font(font).color(color)).frame_when_inactive(false);
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
                    ui.add_space(8.0);

                    if connections_view && self.tabs.len() >= 3 {
                        ui.label(RichText::new("Abiertas").small().color(theme::subtext()));
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for (index, tab) in self.tabs.iter().enumerate().take(6) {
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                            let response = response.hand();
                            let active = index == self.active;
                            if active {
                                ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), theme::selection());
                            } else if response.hovered() {
                                ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), theme::surface());
                            }
                            let glyph = match (&tab.body, tab.kind) {
                                (Body::Explorer(_), _) => icons::FOLDER,
                                (Body::Settings(_), _) => icons::GEAR,
                                (_, TabKind::Local) => icons::TERMINAL,
                                _ => icons::ARROW_RIGHT,
                            };
                            ui.painter().text(Pos2::new(rect.min.x + 8.0, rect.center().y), Align2::CENTER_CENTER, glyph, FontId::proportional(14.0), theme::subtext());
                            let font = FontId::new(13.0, if active { strong_family() } else { FontFamily::Proportional });
                            ui.painter().with_clip_rect(rect.shrink2(Vec2::new(0.0, 0.0)).with_max_x(rect.max.x - 18.0)).text(
                                Pos2::new(rect.min.x + 22.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                tab.title(),
                                font,
                                theme::text(),
                            );
                            let lost = tab.pane().is_some_and(|p| p.session.is_disconnected());
                            if lost || tab.attention {
                                let color = if lost { theme::red() } else { theme::accent() };
                                ui.painter().circle_filled(Pos2::new(rect.max.x - 6.0, rect.center().y), 3.5, color);
                            }
                            if response.on_hover_text(if lost { "Sesión caída" } else if tab.attention { "Actividad terminada" } else { "Ir a esta pestaña" }).clicked() {
                                open_tab = Some(index);
                            }
                        }
                        if self.tabs.len() > 6 {
                            ui.label(RichText::new(format!("+{} más en la barra de pestañas", self.tabs.len() - 6)).small().color(theme::subtext()));
                        }
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.y = 8.0;
                    }
                    let list_empty = if connections_view { self.connections.is_empty() } else { self.snippets.is_empty() };
                    if !list_empty {
                    ui.horizontal(|ui| {
                        let title = if connections_view { "Guardadas" } else { "Guardados" };
                        ui.label(RichText::new(title).small().color(theme::subtext()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        if connections_view {
                                if compact_icon(ui, icons::PLUS, 16.0, theme::text()).on_hover_text("Nueva conexión").clicked() {
                                    new_connection = true;
                                }
                            } else if compact_icon(ui, icons::PLUS, 16.0, theme::text()).on_hover_text("Nuevo snippet").clicked() {
                                new_snippet = true;
                            }
                        });
                    });
                    ui.add_space(-2.0);
                    }

                    if connections_view {
                        if self.connections.is_empty() {
                            let secondary = (self.ssh_config_hosts > 0).then(|| format!("Importar {} de ~/.ssh/config", self.ssh_config_hosts));
                            let (create, import_clicked) = empty_state(
                                ui,
                                "Aún no hay conexiones",
                                "Guarda los servidores a los que te conectas y ábrelos con un doble clic.",
                                "Nueva conexión",
                                secondary.as_deref(),
                            );
                            new_connection |= create;
                            import |= import_clicked;
                        } else if groups.is_empty() {
                            ui.label(RichText::new("Ninguna conexión coincide con la búsqueda.").color(theme::subtext()));
                        }
                        ui.spacing_mut().item_spacing.y = 2.0;
                        egui::ScrollArea::vertical().id_salt("connections").auto_shrink([false, false]).max_height(ui.available_height() - 64.0).show(ui, |ui| {
                        let mut row_pos = 0usize;
                        for (label, indices) in &groups {
                            let key = label.to_lowercase();
                            let folded = headed && !searching && self.collapsed_groups.contains(&key);
                            if headed {
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
                                if response.dnd_hover_payload::<i64>().is_some() {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), Stroke::new(1.5, theme::accent()), egui::StrokeKind::Inside);
                                }
                                if let Some(payload) = response.dnd_release_payload::<i64>() {
                                    drop_move = Some((*payload, label.clone()));
                                }
                                if self.sidebar_nav && row_pos == self.nav_index {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), Stroke::new(1.5, theme::text()), egui::StrokeKind::Inside);
                                    if nav_moved {
                                        ui.scroll_to_rect(rect, None);
                                    }
                                }
                                row_pos += 1;
                                let color = if response.hovered() { theme::text() } else { theme::subtext() };
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 6.0, rect.center().y),
                                    Align2::CENTER_CENTER,
                                    if folded { icons::CARET_RIGHT } else { icons::CARET_DOWN },
                                    FontId::proportional(12.0),
                                    color,
                                );
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 18.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    format!("{label}  {}", indices.len()),
                                    FontId::new(12.0, strong_family()),
                                    color,
                                );
                                if response.hand().on_hover_text(if folded { "Mostrar el grupo" } else { "Plegar el grupo" }).clicked() && !searching {
                                    toggle_group = Some(key.clone());
                                }
                            }
                            if folded {
                                continue;
                            }
                            for &index in indices {
                                let connection = &self.connections[index];
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
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::row(ROW_HEIGHT)), Sense::click_and_drag());
                                let over = ui.rect_contains_pointer(rect);
                                let this_pos = row_pos;
                                row_pos += 1;
                                let connection_id = connection.id.unwrap_or_default();
                                if response.drag_started() {
                                    response.dnd_set_drag_payload(connection_id);
                                }
                                if response.dragged() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                                    if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                                        let ghost = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::NULL.with("drag-ghost")));
                                        let galley = ghost.layout_no_wrap(connection.name.clone(), FontId::new(13.0, strong_family()), theme::text());
                                        let box_rect = Rect::from_min_size(pointer + Vec2::new(14.0, 8.0), galley.size() + Vec2::new(16.0, 10.0));
                                        ghost.rect_filled(box_rect, CornerRadius::same(CONTROL_RADIUS), theme::selection());
                                        ghost.galley(box_rect.min + Vec2::new(8.0, 5.0), galley, theme::text());
                                    }
                                }
                                if response.dnd_hover_payload::<i64>().is_some_and(|p| *p != connection_id) {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), Stroke::new(1.5, theme::accent()), egui::StrokeKind::Inside);
                                }
                                if let Some(payload) = response.dnd_release_payload::<i64>() {
                                    if *payload != connection_id {
                                        drop_move = Some((*payload, label.clone()));
                                    }
                                }
                                if self.sidebar_nav && this_pos == self.nav_index {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), Stroke::new(1.5, theme::text()), egui::StrokeKind::Inside);
                                    if nav_moved {
                                        ui.scroll_to_rect(rect, None);
                                    }
                                }
                                if response.clicked() {
                                    self.sidebar_nav = true;
                                    self.nav_index = this_pos;
                                }
                                let is_pinned = connection.id.is_some_and(|id| self.pinned.contains(&id));
                                let ago = self.recent.iter().find(|(id, _)| Some(*id) == connection.id).map(|(_, at)| relative_time(*at, now)).unwrap_or_default();
                                if response.hovered() || over {
                                    ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(CONTROL_RADIUS), theme::surface());
                                }
                                if let Some(color) = identity {
                                    // Marca de identidad: informa de qué servidor es, no decora.
                                    let bar = Rect::from_min_size(Pos2::new(rect.min.x - 8.0, rect.min.y + 7.0), Vec2::new(3.0, theme::row(ROW_HEIGHT) - 14.0));
                                    ui.painter().rect_filled(bar, CornerRadius::same(2), color);
                                }
                                // Acciones al pasar el ratón: sustituyen a la hora, que vuelve a verse sin ratón encima.
                                let more_rect = Rect::from_center_size(Pos2::new(rect.max.x - 12.0, rect.center().y), Vec2::splat(24.0));
                                let edit_rect = Rect::from_center_size(Pos2::new(rect.max.x - 38.0, rect.center().y), Vec2::splat(24.0));
                                let more = ui.interact(more_rect, response.id.with("more"), Sense::click()).on_hover_text("Más acciones").hand();
                                let pencil = ui.interact(edit_rect, response.id.with("edit"), Sense::click()).on_hover_text("Editar").hand();
                                let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more));
                                let actions = over || menu_open;

                                // Icono de tipo con la insignia del estado de la sesión en su esquina.
                                let icon_center = Pos2::new(rect.min.x + 9.0, rect.center().y);
                                let kind_icon = if connection.is_files() {
                                    icons::FOLDER
                                } else if connection.is_adb() {
                                    icons::DOTS
                                } else {
                                    icons::TERMINAL
                                };
                                ui.painter().text(icon_center, Align2::CENTER_CENTER, kind_icon, FontId::proportional(17.0), theme::subtext());
                                let badge = match state {
                                    LinkState::Open => Some(theme::accent()),
                                    LinkState::Lost => Some(theme::red()),
                                    LinkState::Closed => None,
                                };
                                if let Some(color) = badge {
                                    let at = icon_center + Vec2::new(8.0, 8.0);
                                    ui.painter().circle_filled(at, 5.5, if over { theme::surface() } else { theme::base() });
                                    ui.painter().circle_filled(at, 3.5, color);
                                }

                                let text_x = rect.min.x + 28.0;
                                let ago_font = FontId::new(11.5, FontFamily::Proportional);
                                let ago_width = if ago.is_empty() || actions { 0.0 } else { ui.painter().layout_no_wrap(ago.clone(), ago_font.clone(), Color32::PLACEHOLDER).size().x + 10.0 };
                                let right_limit = if actions { rect.max.x - 56.0 } else { rect.max.x - 4.0 };
                                let first_line = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(right_limit - ago_width, rect.max.y)));
                                let name_font = FontId::new(14.0, strong_family());
                                let pill_font = FontId::new(11.0, strong_family());
                                let env = connection.env.as_ref().filter(|_| !headed || searching);
                                let pill_size = env.map(|env| first_line.layout_no_wrap(env.clone(), pill_font.clone(), Color32::PLACEHOLDER).size() + Vec2::new(10.0, 3.0));
                                let lock_width = if connection.insecure_protocol().is_some() { 18.0 } else { 0.0 };
                                // El nombre cede sitio al candado y a la pastilla, que siempre se ven.
                                let room = right_limit - ago_width - text_x - lock_width - pill_size.map_or(0.0, |s| s.x + 8.0);
                                let name = fit_text(&first_line, &connection.name, &name_font, room);
                                let name_width = first_line.layout_no_wrap(name.clone(), name_font.clone(), Color32::PLACEHOLDER).size().x;
                                first_line.text(Pos2::new(text_x, rect.min.y + 9.0), Align2::LEFT_TOP, name, name_font, theme::text());
                                let mut after_name = text_x + name_width + 8.0;
                                if lock_width > 0.0 {
                                    first_line.text(Pos2::new(after_name + 5.0, rect.min.y + 19.0), Align2::CENTER_CENTER, icons::LOCK_OPEN, FontId::proportional(13.0), theme::red());
                                    after_name += lock_width;
                                }
                                if let (Some(env), Some(size)) = (env, pill_size) {
                                    // Pastilla con el color de identidad: dice en qué entorno estás.
                                    let tint = identity.unwrap_or_else(theme::subtext);
                                    let pill = Rect::from_min_size(Pos2::new(after_name, rect.min.y + 11.0), size);
                                    first_line.rect_filled(pill, CornerRadius::same(8), tint.gamma_multiply(0.2));
                                    first_line.text(pill.center(), Align2::CENTER_CENTER, env, pill_font, tint);
                                }
                                if ago_width > 0.0 {
                                    ui.painter().text(Pos2::new(rect.max.x - 4.0, rect.min.y + 11.0), Align2::RIGHT_TOP, &ago, ago_font, theme::subtext());
                                }
                                let second_line = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(right_limit, rect.max.y)));
                                let detail_font = FontId::new(12.0, FontFamily::Proportional);
                                let detail = fit_text(&second_line, &format!("{}  ·  {}", connection.kind_label(), connection.destination()), &detail_font, right_limit - text_x);
                                second_line.text(Pos2::new(text_x, rect.min.y + 27.0), Align2::LEFT_TOP, detail, detail_font, theme::subtext());
                                let response = response.hand().on_hover_text(match state {
                                    LinkState::Closed => "Doble clic para conectar",
                                    LinkState::Open => "Sesión abierta. Doble clic para otra pestaña",
                                    LinkState::Lost => "Sesión caída: usa Reconectar en su pestaña",
                                });
                                if response.double_clicked() {
                                    self.sidebar_nav = false;
                                    connect = Some(connection.clone());
                                }
                                response.context_menu(|ui| connection_menu(ui, connection, is_pinned, &mut row_action));

                                if actions {
                                    for (button, rect) in [(&pencil, edit_rect), (&more, more_rect)] {
                                        if button.hovered() || (menu_open && rect == more_rect) {
                                            ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::selection());
                                        }
                                    }
                                    for (glyph, rect, size) in [(icons::PENCIL, edit_rect, 16.0), (icons::DOTS, more_rect, 20.0)] {
                                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(size), theme::text());
                                    }
                                }
                                if pencil.clicked() {
                                    row_action = Some((RowAction::Edit, connection.clone()));
                                }
                                egui::Popup::menu(&more).show(|ui| connection_menu(ui, connection, is_pinned, &mut row_action));

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
                        }
                        });
                    } else {
                        if !self.snippets.is_empty() {
                            if outline_add(ui, Button::new(RichText::new("Guardar selección").small().color(theme::subtext())), false).clicked() {
                                snippet_from_selection = true;
                            }
                            ui.add_space(4.0);
                        }

                        let query = self.sidebar_query.clone();
                        let visible: Vec<&Snippet> = self.snippets.iter().filter(|s| s.matches(&query)).collect();
                        if self.snippets.is_empty() {
                            let (create, _) = empty_state(
                                ui,
                                "Aún no hay snippets",
                                "Guarda los comandos que repites y pégalos en el terminal con un clic. También puedes seleccionar texto en el terminal y guardarlo desde el menú del clic derecho.",
                                "Nuevo snippet",
                                None,
                            );
                            new_snippet |= create;
                        } else if visible.is_empty() {
                            ui.label(RichText::new("Ningún snippet coincide con la búsqueda.").color(theme::subtext()));
                        }
                        ui.spacing_mut().item_spacing.y = 2.0;
                        egui::ScrollArea::vertical().id_salt("snippets").auto_shrink([false, false]).max_height(ui.available_height() - 64.0).show(ui, |ui| {
                        let mut tags: Vec<(String, Vec<&Snippet>)> = Vec::new();
                        for snippet in visible {
                            let label = if snippet.tag.trim().is_empty() { "Sin etiqueta".to_string() } else { snippet.tag.trim().to_string() };
                            match tags.iter_mut().find(|(l, _)| l.to_lowercase() == label.to_lowercase()) {
                                Some((_, members)) => members.push(snippet),
                                None => tags.push((label, vec![snippet])),
                            }
                        }
                        tags.sort_by_key(|(label, _)| (label == "Sin etiqueta", label.to_lowercase()));
                        let grouped = tags.len() > 1 || tags.first().is_some_and(|(l, _)| l != "Sin etiqueta");
                        for (label, members) in &tags {
                            let key = format!("snip:{}", label.to_lowercase());
                            let folded = grouped && !searching && self.collapsed_groups.contains(&key);
                            if grouped {
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
                                let color = if response.hovered() { theme::text() } else { theme::subtext() };
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 6.0, rect.center().y),
                                    Align2::CENTER_CENTER,
                                    if folded { icons::CARET_RIGHT } else { icons::CARET_DOWN },
                                    FontId::proportional(12.0),
                                    color,
                                );
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 18.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    format!("{label}  {}", members.len()),
                                    FontId::new(12.0, strong_family()),
                                    color,
                                );
                                if response.hand().on_hover_text(if folded { "Mostrar el grupo" } else { "Plegar el grupo" }).clicked() && !searching {
                                    toggle_group = Some(key.clone());
                                }
                            }
                            if folded {
                                continue;
                            }
                            for snippet in members {
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::row(ROW_HEIGHT)), Sense::click());
                                let over = ui.rect_contains_pointer(rect);
                                if response.hovered() || over {
                                    ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::surface());
                                }
                                let left = rect.min.x + 10.0;
                                let painter = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)).with_max_x(rect.max.x - if over { 34.0 } else { 4.0 }));
                                painter.text(Pos2::new(left, rect.min.y + 9.0), Align2::LEFT_TOP, &snippet.title, FontId::new(14.0, strong_family()), theme::text());
                                let preview = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                                let preview: String = preview.chars().take(80).collect();
                                painter.text(Pos2::new(left, rect.min.y + 27.0), Align2::LEFT_TOP, preview, FontId::new(12.0, FontFamily::Monospace), theme::subtext());

                                let run_rect = Rect::from_center_size(Pos2::new(rect.max.x - 18.0, rect.center().y), Vec2::splat(26.0));
                                let run = ui.interact(run_rect, response.id.with("run"), Sense::click()).hand();
                                if over {
                                    if run.hovered() {
                                        ui.painter().rect_filled(run_rect, CornerRadius::same(CONTROL_RADIUS), theme::selection());
                                    }
                                    ui.painter().text(run_rect.center(), Align2::CENTER_CENTER, icons::PLAY, FontId::proportional(15.0), theme::text());
                                }
                                let run = run.on_hover_text("Insertar y ejecutar");
                                let run_clicked = run.clicked();
                                if run_clicked {
                                    insert = Some((Snippet::clone(snippet), true));
                                }
                                let response = response.hand().on_hover_ui(|ui| {
                                    ui.set_max_width(420.0);
                                    let body: String = snippet.body.chars().take(600).collect();
                                    ui.label(RichText::new(body).monospace().color(theme::text()));
                                    ui.label(RichText::new("Clic para insertar en el terminal activo. Clic derecho: más opciones").small().color(theme::subtext()));
                                });
                                if response.clicked() && !run_clicked {
                                    insert = Some((Snippet::clone(snippet), false));
                                }
                                response.context_menu(|ui| {
                                    if ui.button("Insertar").clicked() {
                                        insert = Some((Snippet::clone(snippet), false));
                                        ui.close();
                                    }
                                    if ui.button("Insertar y ejecutar").clicked() {
                                        insert = Some((Snippet::clone(snippet), true));
                                        ui.close();
                                    }
                                    if ui.button("Copiar").clicked() {
                                        copy = Some(snippet.body.clone());
                                        ui.close();
                                    }
                                    if ui.button("Editar").clicked() {
                                        edit_snippet = Some(Snippet::clone(snippet));
                                        ui.close();
                                    }
                                    if ui.button("Eliminar").clicked() {
                                        delete_snippet = Some(Snippet::clone(snippet));
                                        ui.close();
                                    }
                                });
                            }
                        }
                        });
                    }

                    // Pie: solo avisos; el estado normal de los secretos no hace falta mostrarlo.
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                        if outline_add(ui, Button::new(format!("{}  Ajustes", icons::GEAR)), false).clicked() {
                            open_settings = true;
                        }
                        if keyring_fallback {
                            ui.label(RichText::new("Sin keyring: las contraseñas usan la clave maestra").small().color(theme::subtext()));
                        }
                    });
                });
            self.sidebar_rect = panel.response.rect;
            // Asa del ancho: se resalta al pasar el ratón y un doble clic restablece el ancho por defecto.
            let edge = Rect::from_min_max(
                Pos2::new(panel.response.rect.max.x - 3.0, panel.response.rect.min.y),
                Pos2::new(panel.response.rect.max.x + 3.0, panel.response.rect.max.y),
            );
            if ui.rect_contains_pointer(edge) {
                ui.painter().rect_filled(edge, CornerRadius::ZERO, theme::accent().gamma_multiply(0.7));
                if ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
                    self.sidebar_epoch += 1;
                    self.sidebar_width = SIDEBAR_WIDTH;
                    let _ = self.store.lock().unwrap().set_meta("sidebar_width", SIDEBAR_WIDTH.to_string().as_bytes());
                }
            }
            let width = panel.response.rect.width();
            if (width - self.sidebar_width).abs() > 0.5 && !ui.input(|i| i.pointer.any_down()) {
                self.sidebar_width = width;
                let _ = self.store.lock().unwrap().set_meta("sidebar_width", width.round().to_string().as_bytes());
            }
        }

        if let Some(index) = open_tab {
            self.active = index.min(self.tabs.len().saturating_sub(1));
        }
        if let Some((id, label)) = drop_move {
            self.move_to_group(id, &label);
        }
        if let Some(key) = toggle_group {
            if !self.collapsed_groups.remove(&key) {
                self.collapsed_groups.insert(key);
            }
            let mut keys: Vec<&str> = self.collapsed_groups.iter().map(String::as_str).collect();
            keys.sort_unstable();
            let _ = self.store.lock().unwrap().set_meta("collapsed_groups", keys.join("\n").as_bytes());
        }
        if let Some((action, connection)) = row_action {
            match action {
                RowAction::Connect => {
                    self.sidebar_nav = false;
                    connect = Some(connection)
                }
                RowAction::Files => {
                    self.sidebar_nav = false;
                    files = Some(connection)
                }
                RowAction::Pin => {
                    if let Some(id) = connection.id {
                        self.toggle_pin(id);
                    }
                }
                RowAction::Edit => edit = Some(connection),
                RowAction::Delete => delete = Some(connection),
                RowAction::Duplicate => {
                    let forwards = self.forwards.get(&connection.id.unwrap_or_default()).map_or(&[][..], |f| f.as_slice());
                    let mut form = ConnForm::from_connection(&connection, forwards);
                    form.id = None;
                    form.name = format!("{} (copia)", connection.name);
                    self.dialog = Some(Dialog::Edit(form));
                }
            }
        }

        if open_settings {
            self.open_settings(dialogs::SettingsTab::General);
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

enum RowAction {
    Connect,
    Files,
    Edit,
    Duplicate,
    Pin,
    Delete,
}

/// Una fila navegable de la lista: encabezado de grupo (clave) o conexión (índice).
enum NavRow {
    Header(String),
    Conn(usize),
}

const PINNED_LABEL: &str = "Fijadas";

/// Entorno y color que corresponden a un grupo del sidebar; `None` en «Sin entorno».
fn group_environment(label: &str, members: &[&Connection]) -> Option<(String, Option<u8>)> {
    match label.to_lowercase().as_str() {
        "sin entorno" => None,
        "producción" => Some(("producción".into(), Some(0))),
        "staging" => Some(("staging".into(), Some(1))),
        "desarrollo" => Some(("desarrollo".into(), Some(3))),
        "personal" => Some(("personal".into(), Some(4))),
        _ => members.first().map(|c| (c.env.clone().unwrap_or_else(|| label.to_string()), c.color)),
    }
}

fn connection_menu(ui: &mut egui::Ui, connection: &Connection, pinned: bool, out: &mut Option<(RowAction, Connection)>) {
    for (label, action) in [
        ("Conectar", RowAction::Connect),
        ("Explorador de archivos", RowAction::Files),
        ("Editar", RowAction::Edit),
        ("Duplicar", RowAction::Duplicate),
        (if pinned { "Desfijar" } else { "Fijar arriba" }, RowAction::Pin),
        ("Eliminar", RowAction::Delete),
    ] {
        if ui.button(label).clicked() {
            *out = Some((action, connection.clone()));
            ui.close();
        }
    }
}

/// Conexiones que cumplen la búsqueda, agrupadas por entorno (índices en `connections`).
/// Orden: entornos conocidos, otros por nombre y al final las que no tienen entorno.
fn sidebar_groups(connections: &[Connection], query: &str, pinned: &[i64]) -> Vec<(String, Vec<usize>)> {
    let query = query.trim().to_lowercase();
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, c) in connections.iter().enumerate() {
        let env = c.env.as_deref().map(str::trim).unwrap_or("");
        let haystack = format!("{} {} {} {}", c.name, c.host, c.user, env).to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }
        let label = if c.id.is_some_and(|id| pinned.contains(&id)) {
            PINNED_LABEL.to_string()
        } else if env.is_empty() {
            "Sin entorno".to_string()
        } else {
            let mut chars = env.chars();
            chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        };
        match groups.iter_mut().find(|(l, _)| l.to_lowercase() == label.to_lowercase()) {
            Some((_, members)) => members.push(index),
            None => groups.push((label, vec![index])),
        }
    }
    let rank = |label: &str| match label.to_lowercase().as_str() {
        "fijadas" => (0, String::new()),
        "producción" => (1, String::new()),
        "staging" => (2, String::new()),
        "desarrollo" => (3, String::new()),
        "personal" => (4, String::new()),
        "sin entorno" => (6, String::new()),
        other => (5, other.to_string()),
    };
    groups.sort_by_key(|(label, _)| rank(label));
    groups
}

/// `x,y` guardado, si deja a la vista al menos 50 px de la ventana dentro del monitor.
fn parse_window_position(text: &str, monitor: Vec2, size: Vec2) -> Option<Pos2> {
    let (x, y) = text.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    let margin = 50.0;
    let visible = x + size.x > margin && x < monitor.x - margin && y >= 0.0 && y < monitor.y - margin;
    visible.then(|| Pos2::new(x, y))
}

/// Estado vacío de una lista: título, explicación y una acción principal (y otra opcional).
fn empty_state(ui: &mut egui::Ui, title: &str, body: &str, primary: &str, secondary: Option<&str>) -> (bool, bool) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).font(FontId::new(15.0, strong_family())).color(theme::text()));
    ui.add_space(2.0);
    ui.label(RichText::new(body).small().color(theme::subtext()));
    ui.add_space(14.0);
    let width = ui.available_width();
    let create = solid_add(ui, Button::new(RichText::new(primary).color(theme::base())).min_size(Vec2::new(width, 32.0)), theme::accent()).clicked();
    let mut other = false;
    if let Some(label) = secondary {
        ui.add_space(4.0);
        let button = Button::new(RichText::new(label).color(theme::text())).min_size(Vec2::new(width, 32.0));
        other = outline_add(ui, button, false).clicked();
    }
    (create, other)
}

/// Cuántos hosts de `~/.ssh/config` aún no están guardados.
fn pending_ssh_config_hosts(saved: &[Connection]) -> usize {
    let path = std::env::home_dir().unwrap_or_default().join(".ssh").join("config");
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let user = std::env::var("USER").unwrap_or_default();
    crate::sshconfig::parse(&text, &user).iter().filter(|h| !saved.iter().any(|c| c.name == h.connection.name)).count()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `id:segundos,id:segundos…`; también acepta el formato antiguo, solo ids.
fn parse_recent(text: &str) -> Vec<(i64, u64)> {
    text.split(',')
        .filter_map(|part| {
            let mut pieces = part.trim().splitn(2, ':');
            let id = pieces.next()?.parse().ok()?;
            Some((id, pieces.next().and_then(|t| t.parse().ok()).unwrap_or(0)))
        })
        .collect()
}

/// «ahora», «hace 5 min», «hace 3 h», «ayer», «hace 4 d»; vacío si no se sabe cuándo fue.
fn relative_time(then: u64, now: u64) -> String {
    if then == 0 || then > now {
        return String::new();
    }
    let seconds = now - then;
    match seconds {
        0..=59 => "ahora".to_string(),
        60..=3599 => format!("hace {} min", seconds / 60),
        3600..=86_399 => format!("hace {} h", seconds / 3600),
        86_400..=172_799 => "ayer".to_string(),
        _ => format!("hace {} d", seconds / 86_400),
    }
}

fn ctx_width(ctx: &egui::Context) -> f32 {
    ctx.content_rect().width()
}

enum PlusChoice {
    Local,
    Connect(Connection),
    Files(Connection),
    Containers,
    All,
}

/// Botón con borde y sin relleno que reacciona al ratón: en reposo es transparente y al pasar
/// por encima se tiñe (con el acento si es el principal). `fill()` en `Button` anularía ese efecto.
fn outline_button(ui: &mut egui::Ui, text: RichText, accent: bool) -> egui::Response {
    outline_add(ui, Button::new(text), accent)
}

/// Botón de icono compacto (26 px) con fondo solo al pasar el ratón; los botones de egui
/// heredan la altura mínima de los controles y quedan demasiado grandes para un icono suelto.
fn compact_icon(ui: &mut egui::Ui, glyph: &str, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    let response = response.hand();
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::selection());
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(size), color);
    response
}

/// Botón de acción principal con relleno que se aclara al pasar el ratón (un `fill()` explícito lo impediría).
pub(crate) fn solid_add(ui: &mut egui::Ui, button: Button<'_>, color: Color32) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = color;
        widgets.hovered.weak_bg_fill = color.lerp_to_gamma(Color32::WHITE, 0.18);
        widgets.active.weak_bg_fill = color.lerp_to_gamma(Color32::WHITE, 0.32);
        ui.add(button).hand()
    })
    .inner
}

/// Igual que `outline_button` para un botón ya construido (con su tamaño mínimo, por ejemplo).
pub(crate) fn outline_add(ui: &mut egui::Ui, button: Button<'_>, accent: bool) -> egui::Response {
    ui.scope(|ui| {
        let (rest, hover) = if accent { (theme::accent(), theme::accent()) } else { (theme::selection(), theme::subtext()) };
        let width = if accent { 1.5 } else { 1.0 };
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = Stroke::new(width, rest);
        widgets.hovered.bg_stroke = Stroke::new(width, hover);
        widgets.active.bg_stroke = Stroke::new(width, hover);
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.hovered.weak_bg_fill = if accent { theme::accent().gamma_multiply(0.18) } else { theme::surface() };
        widgets.active.weak_bg_fill = if accent { theme::accent().gamma_multiply(0.3) } else { theme::selection() };
        ui.add(button).hand()
    })
    .inner
}

/// Qué se pidió en la pantalla sin pestañas.
enum EmptyAction {
    Local,
    NewConnection,
    Containers,
    Import,
    Shortcuts,
    Connect(Connection),
}

/// Recorta `text` con «…» para que quepa en `width` píxeles.
fn fit_text(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
    let measure = |t: &str| painter.layout_no_wrap(t.to_string(), font.clone(), Color32::PLACEHOLDER).size().x;
    if measure(text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>().trim_end().to_string() + "…";
        if measure(&candidate) <= width { low = mid } else { high = mid - 1 }
    }
    chars[..low].iter().collect::<String>().trim_end().to_string() + "…"
}

/// Contenido del menú del «+»: terminal local primero, luego conexiones recientes y «Ver todas».
fn plus_menu(ui: &mut egui::Ui, recents: &[Connection], selected: &mut usize, choice: &mut Option<PlusChoice>) {
    ui.set_width(310.0);
    let rows = recents.len() + 3;
    let (down, up, enter) = ui.ctx().input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    if down {
        *selected = (*selected + 1) % rows;
    }
    if up {
        *selected = (*selected + rows - 1) % rows;
    }
    *selected = (*selected).min(rows - 1);
    let mut picked: Option<usize> = enter.then_some(*selected);
    let mut files_for: Option<usize> = None;
    ui.spacing_mut().item_spacing.y = 2.0;

    let row = |ui: &mut egui::Ui, index: usize, selected: &mut usize| -> (Rect, bool) {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
        let response = response.hand();
        if response.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
            *selected = index;
        }
        if *selected == index {
            ui.painter().rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), theme::selection());
        }
        (rect, response.clicked())
    };

    let (rect, clicked) = row(ui, 0, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::TERMINAL, FontId::proportional(18.0), theme::text());
    ui.painter().text(rect.min + Vec2::new(42.0, 13.0), Align2::LEFT_CENTER, "Terminal local", FontId::new(14.0, strong_family()), theme::text());
    ui.painter().text(rect.min + Vec2::new(42.0, 29.0), Align2::LEFT_CENTER, "Este equipo", FontId::new(12.0, FontFamily::Proportional), theme::subtext());
    if clicked {
        picked = Some(0);
    }

    if !recents.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Recientes").small().color(theme::subtext()));
    }
    for (offset, connection) in recents.iter().enumerate() {
        let index = offset + 1;
        let (rect, clicked) = row(ui, index, selected);
        let badge = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::splat(28.0));
        let fill = connection.color.map_or_else(theme::surface, theme::identity);
        let ink = if connection.color.is_some() { theme::base() } else { theme::text() };
        ui.painter().rect_filled(badge, CornerRadius::same(6), fill);
        let initial = connection.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, initial, FontId::new(14.0, strong_family()), ink);
        ui.painter().text(rect.min + Vec2::new(44.0, 13.0), Align2::LEFT_CENTER, &connection.name, FontId::new(14.0, strong_family()), theme::text());
        ui.painter().text(
            rect.min + Vec2::new(44.0, 29.0),
            Align2::LEFT_CENTER,
            connection.target(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::subtext(),
        );
        let files = Rect::from_center_size(Pos2::new(rect.max.x - 20.0, rect.center().y), Vec2::splat(28.0));
        let files_response = ui.interact(files, ui.auto_id_with(("plus-files", index)), Sense::click()).hand();
        if files_response.hovered() {
            ui.painter().rect_filled(files, CornerRadius::same(CONTROL_RADIUS), theme::surface());
        }
        ui.painter().text(files.center(), Align2::CENTER_CENTER, icons::FOLDER, FontId::proportional(16.0), theme::subtext());
        if files_response.on_hover_text("Abrir el explorador de archivos").clicked() {
            files_for = Some(index);
        } else if clicked {
            picked = Some(index);
        }
    }

    ui.add_space(4.0);
    let containers_index = rows - 2;
    let (rect, clicked) = row(ui, containers_index, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::DOTS, FontId::proportional(16.0), theme::subtext());
    ui.painter().text(rect.min + Vec2::new(42.0, 20.0), Align2::LEFT_CENTER, "Contenedores", FontId::new(14.0, FontFamily::Proportional), theme::text());
    if clicked {
        picked = Some(containers_index);
    }
    let index = rows - 1;
    let (rect, clicked) = row(ui, index, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::SEARCH, FontId::proportional(16.0), theme::subtext());
    ui.painter().text(rect.min + Vec2::new(42.0, 20.0), Align2::LEFT_CENTER, "Ver todas…", FontId::new(14.0, FontFamily::Proportional), theme::text());
    ui.painter().text(rect.max - Vec2::new(10.0, 20.0), Align2::RIGHT_CENTER, "Ctrl+Shift+P", FontId::new(12.0, FontFamily::Proportional), theme::subtext());
    if clicked {
        picked = Some(index);
    }

    let result = match (files_for, picked) {
        (Some(index), _) => recents.get(index - 1).cloned().map(PlusChoice::Files),
        (None, Some(0)) => Some(PlusChoice::Local),
        (None, Some(index)) if index == rows - 2 => Some(PlusChoice::Containers),
        (None, Some(index)) if index == rows - 1 => Some(PlusChoice::All),
        (None, Some(index)) => recents.get(index - 1).cloned().map(PlusChoice::Connect),
        (None, None) => None,
    };
    if result.is_some() {
        *choice = result;
        ui.close();
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

/// Instala las fuentes; con `custom` (archivo TTF/OTF) el terminal usa esa en lugar de JetBrains Mono.
/// Si el archivo no se puede usar, se queda JetBrains Mono y se devuelve el motivo.
fn install_fonts(ctx: &egui::Context, custom: Option<&str>) -> Option<String> {
    let mut problem = None;
    let custom_bytes: Option<&'static [u8]> = custom.and_then(|path| match std::fs::read(path) {
        Ok(bytes) if ab_glyph::FontRef::try_from_slice(&bytes).is_ok() => Some(&*Box::leak(bytes.into_boxed_slice())),
        Ok(_) => {
            problem = Some(format!("{path} no es una fuente TTF u OTF válida"));
            None
        }
        Err(error) => {
            problem = Some(format!("No se pudo leer {path}: {error}"));
            None
        }
    });
    let mut fonts = FontDefinitions::default();
    let load = |name: &str, bytes: &'static [u8], fonts: &mut FontDefinitions| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    load("jb-regular", custom_bytes.unwrap_or(include_bytes!("../assets/JetBrainsMono-Regular.ttf")), &mut fonts);
    load("jb-bold", custom_bytes.unwrap_or(include_bytes!("../assets/JetBrainsMono-Bold.ttf")), &mut fonts);
    // La interfaz usa una sans neutra con buena cobertura de acentos; el terminal, JetBrains Mono.
    load("ui-regular", include_bytes!("../assets/NotoSans-Regular.ttf"), &mut fonts);
    load("ui-semibold", include_bytes!("../assets/NotoSans-SemiBold.ttf"), &mut fonts);
    load("icons", include_bytes!("../assets/Phosphor.ttf"), &mut fonts);
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jb-regular".into());
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui-regular".into());
    fonts.families.insert(bold_family(), vec!["jb-bold".into()]);
    fonts.families.insert(strong_family(), vec!["ui-semibold".into(), "ui-regular".into()]);
    // Los iconos van de último recurso: cualquier texto puede llevar uno sin cambiar de familia.
    for family in [FontFamily::Proportional, strong_family()] {
        fonts.families.entry(family).or_default().push("icons".into());
    }
    ctx.set_fonts(fonts);
    problem
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
    // Barras finas que solo aparecen al acercar el ratón o al desplazar.
    style.spacing.scroll = egui::style::ScrollStyle::floating();
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
#[derive(Clone, Copy)]
enum TabAction {
    Rename,
    Duplicate,
    CloseOthers,
    CloseRight,
    Close,
}

struct TabOutcome {
    clicked: bool,
    closed: bool,
    dragged: bool,
    rename: bool,
    rect: Rect,
}

fn tab(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    index: usize,
    identity: Option<Color32>,
    attention: bool,
    insecure: bool,
    count: usize,
    can_duplicate: bool,
    can_rename: bool,
    max_width: f32,
    menu: &mut Option<(usize, TabAction)>,
) -> TabOutcome {
    let font = FontId::new(14.0, if selected { strong_family() } else { FontFamily::Proportional });
    // Un candado abierto rojo avisa de las conexiones sin cifrar (Telnet).
    let lock = if insecure { 16.0 } else { 0.0 };
    let chrome = 12.0 + 8.0 + 20.0 + 8.0 + lock;
    // Hasta 24 caracteres de ancho natural; si no caben todas las pestañas, el texto se corta con «…».
    let natural: String = label.chars().take(24).collect();
    let mut job = egui::text::LayoutJob::simple_singleline(natural, font, Color32::PLACEHOLDER);
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - chrome).max(24.0));
    let galley = ui.painter().layout_job(job);
    let size = Vec2::new(galley.size().x + chrome, TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let response = response.hand();

    // Por geometría: con la X encima, `response.hovered()` se apagaría y la X desaparecería a fotogramas alternos.
    let hovered = response.hovered() || ui.rect_contains_pointer(rect);
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
    if insecure {
        ui.painter().text(Pos2::new(rect.min.x + 18.0, rect.center().y + 1.0), Align2::CENTER_CENTER, icons::LOCK_OPEN, FontId::proportional(13.0), theme::red());
    }
    ui.painter().galley(Pos2::new(rect.min.x + 12.0 + lock, rect.center().y - galley.size().y / 2.0 + 1.0), galley, color);

    let close_rect = Rect::from_center_size(Pos2::new(rect.max.x - 18.0, rect.center().y + 1.0), Vec2::splat(20.0));
    if attention && !selected && !hovered {
        ui.painter().circle_filled(close_rect.center(), 3.5, theme::accent());
    }
    let mut close_clicked = false;
    if selected || hovered {
        let close = ui.interact(close_rect, ui.auto_id_with(("tab-close", index)), Sense::click()).hand();
        if close.hovered() {
            ui.painter().rect_filled(close_rect, CornerRadius::same(4), theme::selection());
        }
        ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, icons::X, FontId::proportional(13.0), color);
        close_clicked = close.on_hover_text("Cerrar pestaña").clicked();
    }
    response.context_menu(|ui| {
        for (label, action, enabled) in [
            ("Renombrar", TabAction::Rename, can_rename),
            ("Duplicar", TabAction::Duplicate, can_duplicate),
            ("Cerrar otras", TabAction::CloseOthers, count > 1),
            ("Cerrar a la derecha", TabAction::CloseRight, index + 1 < count),
            ("Cerrar", TabAction::Close, true),
        ] {
            if ui.add_enabled(enabled, Button::new(label)).clicked() {
                *menu = Some((index, action));
                ui.close();
            }
        }
    });
    TabOutcome {
        clicked: response.clicked() && !close_clicked,
        closed: close_clicked,
        dragged: response.dragged() && !close_clicked,
        rename: can_rename && response.double_clicked() && !close_clicked,
        rect,
    }
}

/// Hasta que pasa este tiempo desde el arranque no se abren pestañas remotas restauradas.
const RESTORE_PROMPT_DELAY: Duration = Duration::from_millis(1000);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_theme(&ctx);
        self.watch_activity(&ctx);
        self.restore_window_position(&ctx);
        self.remember_window_size(&ctx);

        // Con Ctrl solo (B, K) chocarían con tmux y readline.
        let (toggle_sidebar, search_sidebar) = ctx.input_mut(|i| {
            let both = Modifiers::CTRL | Modifiers::SHIFT;
            (i.consume_key(both, Key::B), i.consume_key(both, Key::K))
        });
        if toggle_sidebar {
            self.set_sidebar_collapsed(!self.sidebar_collapsed);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::P)) && self.dialog.is_none() {
            self.palette = if self.palette.is_some() { None } else { Some(Palette::default()) };
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Comma)) {
            self.open_settings(dialogs::SettingsTab::General);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F11)) {
            self.run_command(Command::Fullscreen);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::T)) && self.dialog.is_none() {
            self.open_plus_menu = true;
        }
        // Alt+1…9 salta a esa pestaña (Alt+9, a la última); Ctrl+Tab y Ctrl+Mayús+Tab recorren.
        let (jump, step) = ctx.input_mut(|i| {
            let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
            let jump = digits.iter().position(|k| i.consume_key(Modifiers::ALT, *k));
            if jump.is_some() {
                // El dígito también llega como texto: no debe escribirse en el terminal.
                i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
            }
            let step = if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
                -1
            } else if i.consume_key(Modifiers::CTRL, Key::Tab) {
                1
            } else {
                0
            };
            (jump, step)
        });
        if !self.tabs.is_empty() {
            let last = self.tabs.len() - 1;
            if let Some(n) = jump {
                self.active = if n == 8 { last } else { n.min(last) };
            }
            if step != 0 {
                self.active = (self.active as i32 + step).rem_euclid(self.tabs.len() as i32) as usize;
            }
        }
        if search_sidebar {
            self.set_sidebar_collapsed(false);
            self.focus_search = true;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.close_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_close(CloseAction::Window);
        }
        self.track_alt(&ctx);
        let (menu_command, menu_connection) = self.menu_bar(ui);
        if menu_command.is_some() || menu_connection.is_some() {
            self.menu_peek = false;
        }
        if let Some(command) = menu_command {
            if command == Command::Palette {
                self.palette = Some(Palette::default());
            } else {
                self.run_command(command);
            }
        }
        if let Some(connection) = menu_connection {
            self.begin_open(connection, None, Purpose::Terminal);
        }
        self.sidebar(ui);

        let mut activate = None;
        let mut close = None;
        let mut toggle_sidebar_button = false;
        let mut files_tab: Option<i64> = None;
        let mut split: Option<Dir> = None;
        let mut close_pane = false;
        let tab_labels: Vec<(String, Option<Color32>, bool, bool)> = self
            .tabs
            .iter()
            .map(|t| {
                let insecure = matches!(t.kind, TabKind::Connection(id) if self.connections.iter().any(|c| c.id == Some(id) && c.insecure_protocol().is_some()));
                (t.title(), self.identity_for(t.kind), t.attention, insecure)
            })
            .collect();
        let mut renaming = self.renaming.take();
        let mut rename_done: Option<(usize, Option<String>)> = None;
        let mut tab_menu: Option<(usize, TabAction)> = None;
        let mut tab_rects: Vec<Rect> = Vec::new();
        let mut overflowing = false;
        let mut strip_scrolled_to: Option<u64> = None;
        let plus_recents = self.recent_connections();
        let mut plus_selected = self.plus_selected;
        let mut plus_choice: Option<PlusChoice> = None;
        let mut open_plus_menu = std::mem::take(&mut self.open_plus_menu);
        let mut dragging: Option<usize> = None;
        Panel::top("tabs")
            .exact_size(TAB_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 0 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let hint = if self.sidebar_collapsed { "Mostrar la barra lateral (Ctrl+Shift+B)" } else { "Ocultar la barra lateral (Ctrl+Shift+B)" };
                    let toggle = compact_icon(ui, icons::SIDEBAR, 17.0, theme::subtext());
                    if toggle.on_hover_text(hint).clicked() {
                        toggle_sidebar_button = true;
                    }
                    // Reserva sitio para «+», el menú de pestañas y las acciones de la derecha.
                    let strip_width = (ui.available_width() - 150.0).max(120.0);
                    let tab_cap = (strip_width / tab_labels.len().max(1) as f32).max(MIN_TAB_WIDTH);
                    overflowing = tab_labels.len() as f32 * (MIN_TAB_WIDTH + 2.0) > strip_width;
                    egui::ScrollArea::horizontal()
                        .id_salt("tab-strip")
                        .max_width(strip_width)
                        .auto_shrink([true, false])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                    // La rueda del ratón sobre las pestañas las desplaza de lado.
                    if ui.rect_contains_pointer(ui.clip_rect()) {
                        let delta = ui.input(|i| i.smooth_scroll_delta.y);
                        if delta != 0.0 {
                            ui.scroll_with_delta(Vec2::new(delta, 0.0));
                        }
                    }
                    ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, (title, identity, attention, insecure)) in tab_labels.iter().enumerate() {
                        if let Some((index, text, focus)) = renaming.as_mut().filter(|(index, _, _)| *index == i) {
                            let _ = index;
                            let field = ui.add_sized([150.0, TAB_HEIGHT - 8.0], egui::TextEdit::singleline(text).hint_text("Nombre de la pestaña"));
                            if std::mem::take(focus) {
                                field.request_focus();
                            }
                            // Se consumen para que el terminal, que recupera el foco, no reciba Enter ni Esc.
                            let enter = field.has_focus() && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
                            let escape = field.has_focus() && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
                            if escape {
                                rename_done = Some((i, None));
                            } else if enter || field.lost_focus() {
                                rename_done = Some((i, Some(text.trim().to_string())));
                            }
                            tab_rects.push(field.rect);
                            continue;
                        }
                        let can_duplicate = !matches!(self.tabs[i].kind, TabKind::Explorer(_) | TabKind::Settings | TabKind::Containers);
                        let can_rename = self.tabs[i].kind != TabKind::Settings;
                        let outcome = tab(ui, title, i == self.active, i, *identity, *attention, *insecure, self.tabs.len(), can_duplicate, can_rename, tab_cap, &mut tab_menu);
                        tab_rects.push(outcome.rect);
                        if i == self.active && self.tabs.get(i).is_some_and(|t| t.id != self.tab_strip_seen) {
                            ui.scroll_to_rect(outcome.rect.expand2(Vec2::new(24.0, 0.0)), None);
                            strip_scrolled_to = self.tabs.get(i).map(|t| t.id);
                        }
                        if outcome.clicked {
                            activate = Some(i);
                        }
                        if outcome.closed {
                            close = Some(i);
                        }
                        if outcome.dragged {
                            dragging = Some(i);
                        }
                        if outcome.rename {
                            tab_menu = Some((i, TabAction::Rename));
                        }
                    }
                    });
                        });
                    ui.add_space(6.0);
                    let plus = compact_icon(ui, icons::PLUS, 16.0, theme::text());
                    let plus = plus.on_hover_text("Nueva pestaña (Ctrl+Shift+T)");
                    let popup_id = egui::Popup::default_response_id(&plus);
                    if std::mem::take(&mut open_plus_menu) {
                        egui::Popup::open_id(ui.ctx(), popup_id);
                    }
                    if !egui::Popup::is_id_open(ui.ctx(), popup_id) {
                        plus_selected = 0;
                    }
                    egui::Popup::menu(&plus).width(330.0).show(|ui| {
                        plus_menu(ui, &plus_recents, &mut plus_selected, &mut plus_choice);
                    });
                    if overflowing {
                        let list = compact_icon(ui, icons::CARET_DOWN, 14.0, theme::subtext());
                        let list = list.on_hover_text("Todas las pestañas");
                        egui::Popup::menu(&list).width(260.0).show(|ui| {
                            for (i, (title, _, _, _)) in tab_labels.iter().enumerate() {
                                let shown: String = title.chars().take(32).collect();
                                if ui.selectable_label(i == self.active, shown).clicked() {
                                    activate = Some(i);
                                    ui.close();
                                }
                            }
                        });
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if self.tabs.get(self.active).is_some_and(|t| matches!(t.body, Body::Terminal { .. })) {
                            ui.scope(|ui| {
                                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                                let split_menu = ui.menu_button(RichText::new(icons::SPLIT).size(17.0).color(theme::subtext()), |ui| {
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
                                split_menu.response.on_hover_text("Dividir el panel");
                            });
                        }
                        if let Some(TabKind::Connection(id)) = self.tabs.get(self.active).map(|t| t.kind) {
                            if self.active_has_explorer() && compact_icon(ui, icons::FOLDER, 17.0, theme::subtext()).on_hover_text("Abrir el explorador de archivos de esta conexión").clicked() {
                                files_tab = Some(id);
                            }
                        }
                        if let Some(session) = self.tabs.get(self.active).and_then(|t| t.pane()).map(|p| &p.session).filter(|s| s.serial_status().is_some()) {
                            if let Some(status) = session.serial_status() {
                                let menu = ui.menu_button(RichText::new(format!("{} baudios", status.baud)).small().color(theme::subtext()), |ui| {
                                    ui.menu_button("Velocidad", |ui| {
                                        for rate in store::BAUD_RATES {
                                            if ui.selectable_label(rate == status.baud, rate.to_string()).clicked() {
                                                session.control(term::Control::Baud(rate));
                                                ui.close();
                                            }
                                        }
                                    });
                                    if ui.button("Enviar break").clicked() {
                                        session.control(term::Control::Break);
                                        ui.close();
                                    }
                                    ui.separator();
                                    let mut dtr = status.dtr;
                                    if ui.checkbox(&mut dtr, "DTR").clicked() {
                                        session.control(term::Control::Dtr(dtr));
                                    }
                                    let mut rts = status.rts;
                                    if ui.checkbox(&mut rts, "RTS").clicked() {
                                        session.control(term::Control::Rts(rts));
                                    }
                                });
                                menu.response.on_hover_text("Ajustes del puerto serie");
                            }
                        }
                    });
                });
            });

        if let Some(id) = strip_scrolled_to {
            self.tab_strip_seen = id;
        }
        self.renaming = renaming;
        self.plus_selected = plus_selected;
        match plus_choice {
            Some(PlusChoice::Local) => self.open_queue.push_back(SavedTab::plain(TabKind::Local)),
            Some(PlusChoice::Connect(connection)) => self.begin_open(connection, None, Purpose::Terminal),
            Some(PlusChoice::Files(connection)) => self.begin_open(connection, None, Purpose::Explorer(None, None)),
            Some(PlusChoice::Containers) => self.open_containers(),
            Some(PlusChoice::All) => self.palette = Some(Palette::default()),
            None => {}
        }
        if let Some((i, name)) = rename_done {
            self.renaming = None;
            if let (Some(tab), Some(name)) = (self.tabs.get_mut(i), name) {
                tab.custom = if name.is_empty() { None } else { Some(name) };
            }
        }
        if let Some(from) = dragging {
            let pointer = ctx.input(|i| i.pointer.interact_pos());
            let target = pointer.and_then(|p| tab_rects.iter().position(|r| r.x_range().contains(p.x)));
            if let Some(target) = target.filter(|t| *t != from && *t < self.tabs.len() && from < self.tabs.len()) {
                let moved = self.tabs.remove(from);
                self.tabs.insert(target, moved);
                self.active = target;
            }
        }
        if let Some((i, action)) = tab_menu {
            match action {
                TabAction::Rename => {
                    if let Some(tab) = self.tabs.get(i) {
                        self.renaming = Some((i, tab.title(), true));
                    }
                }
                TabAction::Duplicate => match self.tabs.get(i).map(|t| t.kind) {
                    Some(TabKind::Local) => self.open_queue.push_back(SavedTab::plain(TabKind::Local)),
                    Some(TabKind::Connection(id)) => {
                        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(id)).cloned() {
                            self.begin_open(connection, None, Purpose::Terminal);
                        }
                    }
                    _ => {}
                },
                TabAction::CloseOthers => {
                    if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                        self.request_close(CloseAction::Others(id));
                    }
                }
                TabAction::CloseRight => {
                    if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                        self.request_close(CloseAction::Right(id));
                    }
                }
                TabAction::Close => close = Some(i),
            }
        }
        if toggle_sidebar_button {
            self.set_sidebar_collapsed(!self.sidebar_collapsed);
        }
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
        if let Some(i) = close {
            if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                self.request_close(CloseAction::Tab(id));
            }
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
        if !self.open_queue.is_empty() && self.window_boot.elapsed() < RESTORE_PROMPT_DELAY {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.tabs.len() != tabs_before {
            ctx.request_repaint();
        }

        let finished_restoring = self.restore_active.is_none();
        if self.tabs.is_empty() && finished_restoring && self.open_queue.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Title("omni-term".to_string()));
            match self.empty_state(ui, text_area) {
                Some(EmptyAction::Local) => self.open_queue.push_back(SavedTab::plain(TabKind::Local)),
                Some(EmptyAction::NewConnection) => self.run_command(Command::NewConnection),
                Some(EmptyAction::Containers) => self.open_containers(),
                Some(EmptyAction::Import) => self.run_command(Command::ImportConfig),
                Some(EmptyAction::Shortcuts) => self.open_settings(dialogs::SettingsTab::Atajos),
                Some(EmptyAction::Connect(connection)) => self.begin_open(connection, None, Purpose::Terminal),
                None => {}
            }
        }

        let mut reconnect = None;
        if !self.tabs.is_empty() {
            let active = self.active.min(self.tabs.len() - 1);
            let input_enabled = self.dialog.is_none() && self.pending_ui.is_empty() && self.palette.is_none() && !self.sidebar_nav;
            let font_size = self.settings.font_size;
            let mut disconnected = false;
            let mut container_request = None;
            let mut messages: Vec<String> = Vec::new();
            let mut request = None;
            let mut snippet_text: Option<String> = None;
            if let Body::Settings(section) = self.tabs[active].body {
                let section = self.settings_page(ui, area.shrink2(Vec2::new(24.0, 16.0)), section);
                self.tabs[active].body = Body::Settings(section);
            }
            match &mut self.tabs[active].body {
                Body::Settings(_) => {}
                Body::Terminal { panes, focused, .. } => {
                    let gap = if panes.len() > 1 { SPLIT_GAP } else { 0.0 };
                    let (leaves, handles) = panes.layout(text_area, gap);

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

                    let pressed = ctx.input(|i| (i.pointer.primary_pressed() || i.pointer.button_pressed(egui::PointerButton::Secondary)).then(|| i.pointer.interact_pos()).flatten());
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
                        snippet_text = pane.take_snippet();
                    }
                }
                Body::Explorer(explorer) => explorer.show(ui, area.shrink2(Vec2::new(8.0, 8.0))),
                Body::Containers(view) => {
                    container_request = view.show(ui, area.shrink2(Vec2::new(16.0, 12.0)));
                    messages = view.take_messages();
                }
            }
            for message in messages {
                self.notify(message);
            }
            match container_request {
                Some(containers::Request::Terminal(endpoint, container)) => self.open_container_terminal(endpoint, &container),
                Some(containers::Request::Confirm(endpoint, container, action)) => {
                    self.dialog = Some(Dialog::ConfirmContainer(dialogs::ContainerConfirm { endpoint, container, action }));
                }
                None => {}
            }
            if let Some(text) = snippet_text {
                self.dialog = Some(Dialog::Snippet(SnippetForm::new(text)));
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

            if disconnected && self.tabs[active].kind != TabKind::Container {
                let mut clicked = false;
                egui::Area::new(egui::Id::NULL.with("reconnect"))
                    .pivot(Align2::CENTER_BOTTOM)
                    .fixed_pos(Pos2::new(area.center().x, area.max.y - 24.0))
                    .show(&ctx, |ui| {
                        Frame::popup(ui.style()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Sesión desconectada");
                                let button = Button::new(RichText::new("Reconectar").color(theme::base()));
                                clicked = solid_add(ui, button, theme::accent()).clicked();
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

        if let Some(mut palette) = self.palette.take() {
            let items = self.palette_items();
            match palette.show(&ctx, &items) {
                palette::Outcome::Open => self.palette = Some(palette),
                palette::Outcome::Close => {}
                palette::Outcome::Run(index) => {
                    if let Some(item) = items.into_iter().nth(index) {
                        self.run_palette(item.target);
                    }
                }
            }
        }
        self.handle_dialog(&ctx);
        self.toasts_ui(&ctx);
        self.persist_layout();
    }
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;

    #[test]
    fn saved_window_position_is_used_only_when_it_is_on_screen() {
        let (monitor, size) = (Vec2::new(1920.0, 1080.0), Vec2::new(960.0, 600.0));
        assert_eq!(parse_window_position("100,80", monitor, size), Some(Pos2::new(100.0, 80.0)));
        assert_eq!(parse_window_position("-950,40", monitor, size), None);
        assert_eq!(parse_window_position("3000,40", monitor, size), None);
        assert_eq!(parse_window_position("100,-20", monitor, size), None);
        assert_eq!(parse_window_position("basura", monitor, size), None);
    }

    fn connection(id: i64, name: &str, env: Option<&str>) -> Connection {
        Connection {
            id: Some(id),
            protocol: crate::store::Protocol::Ssh,
            name: name.into(),
            host: format!("{name}.test"),
            port: 22,
            user: "u".into(),
            auth: Auth::Agent,
            key_path: None,
            init_command: None,
            jump: None,
            color: None,
            env: env.map(str::to_string),
        }
    }

    #[test]
    fn pinned_connections_form_the_first_group_and_leave_their_environment() {
        let all = vec![connection(1, "web", Some("producción")), connection(2, "db", Some("producción")), connection(3, "nas", None)];
        let groups = sidebar_groups(&all, "", &[2]);
        let labels: Vec<&str> = groups.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Fijadas", "Producción", "Sin entorno"]);
        assert_eq!(groups[0].1, vec![1]);
        assert_eq!(groups[1].1, vec![0]);
    }

    #[test]
    fn dropping_on_a_group_maps_to_its_environment() {
        let custom = connection(4, "lab", Some("pre-prod"));
        assert_eq!(group_environment("Staging", &[]), Some(("staging".into(), Some(1))));
        assert_eq!(group_environment("Sin entorno", &[]), None);
        assert_eq!(group_environment("Pre-prod", &[&custom]), Some(("pre-prod".into(), None)));
    }

    #[test]
    fn last_use_reads_as_relative_time() {
        let now = 1_000_000;
        assert_eq!(relative_time(0, now), "");
        assert_eq!(relative_time(now - 20, now), "ahora");
        assert_eq!(relative_time(now - 5 * 60, now), "hace 5 min");
        assert_eq!(relative_time(now - 3 * 3600, now), "hace 3 h");
        assert_eq!(relative_time(now - 30 * 3600, now), "ayer");
        assert_eq!(relative_time(now - 4 * 86_400, now), "hace 4 d");
    }

    #[test]
    fn recent_connections_parse_both_formats() {
        assert_eq!(parse_recent("3:1700000000,1:1699999999"), vec![(3, 1_700_000_000), (1, 1_699_999_999)]);
        assert_eq!(parse_recent("2,5"), vec![(2, 0), (5, 0)]);
        assert!(parse_recent("").is_empty());
    }
}
