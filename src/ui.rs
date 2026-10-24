pub mod cursor;
pub mod dialogs;
pub mod fonts;
pub mod icons;
pub mod kit;
pub mod palette;
pub mod theme;

mod appearance;
mod closing;
mod commands;
mod connecting;
mod dialog_flow;
mod empty;
mod library;
mod secrets_flow;
mod sessions;
mod sidebar;
mod startup;
mod tabs;
mod toasts;
mod window;

pub use self::appearance::strong_family;
use self::appearance::{install_fonts, install_visuals};
use self::empty::EmptyAction;
use self::sidebar::{group_environment, PINNED_LABEL};
use self::tabs::{plus_menu, tab, tunnel_line, Body, PlusChoice, Tab, TabAction};
use self::toasts::{Cleanup, Toast, Undo};

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Key, Modifiers, Panel, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Vec2, ViewportCommand,
};
use zeroize::Zeroizing;

use crate::containers::view::{self, ContainersView};
use self::cursor::HandExt;
use crate::containers::docker;
use self::dialogs::{CloseAction, CloseConfirm, ConnForm, Dialog, ErrorInfo, Outcome, PaneTarget, Pending, Purpose, Retry, SecretPrompt, SnippetForm, TestState, UnlockPrompt, VarsPrompt};
use crate::terminal::layout::{Dir, Tree};
use crate::files::explorer::Explorer;
use crate::connect::known_hosts;
use self::palette::{Command, Palette, Target};
use crate::terminal::pane::{self, bold_family, Pane, PaneRequest, PADDING};
use crate::data::settings::{CursorChoice, Settings, StartupChoice, ThemeChoice, DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE};
use crate::data::secrets::{account_for, KeyringStore, MasterKeyStore, MigrateFrom, SecretError, SecretKind, SecretStore, Secrets};
use crate::connect::ssh::{self, Credentials, JumpHost, TunnelState, Tunnels, UiEvent};
use crate::data::store::{self, Auth, Connection, Forward, Protocol, SavedTab, Snippet, Store, TabKind};
use crate::connect::telnet;
use crate::terminal::term::{self, Session, TermOptions};

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
const ROW_HEIGHT: f32 = 46.0;

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

/// Separación entre paneles divididos; también aloja la barra de desplazamiento.
const SPLIT_GAP: f32 = 16.0;

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
    /// Hay que preguntar si se restaura la sesión anterior antes de abrir nada.
    restore_prompt: bool,
    /// Migración de secretos entre almacenes que se hace en este arranque.
    pending_migration: Option<MigrateFrom>,
    /// Clave maestra usada como origen o destino de una migración cuando el almacén activo es el llavero.
    migration_master: Option<Arc<MasterKeyStore>>,
    /// Si el llavero del sistema funciona; se sondea una sola vez cuando hace falta.
    keyring_probe: Option<bool>,
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
        let pending_migration = Secrets::pending_migration(&store.lock().unwrap());
        let keyring_probe = match (&secrets, Secrets::is_master_forced(&store.lock().unwrap())) {
            (Secrets::Keyring(_), _) => Some(true),
            (Secrets::Master(_), false) => Some(false),
            (Secrets::Master(_), true) => None,
        };
        let recent: Vec<(i64, u64)> = match store.lock().unwrap().meta("recent_connections") {
            Ok(Some(bytes)) => parse_recent(&String::from_utf8_lossy(&bytes)),
            _ => Vec::new(),
        };
        let pinned: Vec<i64> = match store.lock().unwrap().meta("pinned_connections") {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).split(',').filter_map(|id| id.trim().parse().ok()).collect(),
            _ => Vec::new(),
        };
        let settings = Settings::load(&store.lock().unwrap());
        let font_error = install_fonts(&cc.egui_ctx, settings.font_family.as_deref());
        theme::set_accent(settings.accent.index());
        theme::set_compact(settings.compact);
        theme::set_bold_bright(settings.bold_bright);
        theme::set_ansi_scheme(settings.ansi.index());
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
        let has_session = open_queue.iter().any(|s| !matches!(s.kind, TabKind::Local));
        let restore_prompt = has_session && settings.on_start == StartupChoice::Ask && std::env::var_os(RESTARTED_ENV).is_none();
        if has_session && settings.on_start == StartupChoice::Fresh {
            open_queue.clear();
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
            restore_prompt,
            pending_migration,
            migration_master: None,
            keyring_probe,
            restore_active: Some(active),
            dialog: None,
            suspended: None,
            saved_layout: (Vec::new(), 0),
            toasts: Vec::new(),
            next_toast: 0,
        };
        app.ssh_config_hosts = pending_ssh_config_hosts(&app.connections);
        app.loaded_font = app.settings.font_family.clone();
        if let Some(problem) = font_error {
            app.settings.font_family = None;
            app.loaded_font = None;
            app.notify(problem);
        }
        if let Some(notice) = startup_notice {
            app.notify(notice);
        }
        Ok(app)
    }

    fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let ctx = self.ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

}

/// Estado vacío de una lista: título, explicación y una acción principal (y otra opcional).
fn empty_state(ui: &mut egui::Ui, title: &str, body: &str, primary: &str, secondary: Option<&str>) -> (bool, bool) {
    ui.add_space(theme::space::GROUP);
    ui.label(RichText::new(title).font(FontId::new(theme::size::HEADING, strong_family())).color(theme::text()));
    ui.add_space(theme::space::XXS);
    ui.label(RichText::new(body).small().color(theme::subtext()));
    ui.add_space(theme::space::XL);
    let width = ui.available_width();
    let create = crate::ui::kit::button_with(ui, Button::new(primary).min_size(Vec2::new(width, 32.0)), crate::ui::kit::Variant::Primary).clicked();
    let mut other = false;
    if let Some(label) = secondary {
        ui.add_space(theme::space::XS);
        let button = Button::new(RichText::new(label).color(theme::text())).min_size(Vec2::new(width, 32.0));
        other = crate::ui::kit::button_with(ui, button, crate::ui::kit::Variant::Secondary).clicked();
    }
    (create, other)
}

/// Cuántos hosts de `~/.ssh/config` aún no están guardados.
fn pending_ssh_config_hosts(saved: &[Connection]) -> usize {
    let path = std::env::home_dir().unwrap_or_default().join(".ssh").join("config");
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let user = std::env::var("USER").unwrap_or_default();
    crate::connect::sshconfig::parse(&text, &user).iter().filter(|h| !saved.iter().any(|c| c.name == h.connection.name)).count()
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

/// Hasta que pasa este tiempo desde el arranque no se abren pestañas remotas restauradas.
fn clipboard_event() -> egui::Event {
    egui::Event::Paste(arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()).unwrap_or_default())
}

/// Variable que marca un reinicio pedido por la propia app: la sesión se restaura sin preguntar.
const RESTARTED_ENV: &str = "OMNITERM_RESTARTED";

const RESTORE_PROMPT_DELAY: Duration = Duration::from_millis(1000);

impl eframe::App for App {
    /// Con los atajos de portapapeles de eframe apagados (para que Ctrl+C/V lleguen a la terminal),
    /// los campos de texto los reciben aquí.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !ctx.text_edit_focused() {
            return;
        }
        let mut extra = Vec::new();
        for event in &raw_input.events {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
            match key {
                Key::V if modifiers.command => extra.push(clipboard_event()),
                Key::Insert if modifiers.shift && !modifiers.command => extra.push(clipboard_event()),
                Key::C | Key::Insert if modifiers.command && !modifiers.shift => extra.push(egui::Event::Copy),
                Key::X if modifiers.command => extra.push(egui::Event::Cut),
                Key::Delete if modifiers.shift && !modifiers.command => extra.push(egui::Event::Cut),
                _ => {}
            }
        }
        extra.retain(|e| !matches!(e, egui::Event::Paste(text) if text.is_empty()));
        raw_input.events.extend(extra);
    }

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
        self.window_frame(&ctx);
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
                    let toggle = crate::ui::kit::icon_button(ui, icons::SIDEBAR, theme::size::ICON_LG, theme::subtext());
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
                    ui.add_space(theme::space::S);
                    let plus = crate::ui::kit::icon_button(ui, icons::PLUS, theme::size::ICON, theme::text());
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
                        let list = crate::ui::kit::icon_button(ui, icons::CARET_DOWN, theme::size::ICON_SM, theme::subtext());
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
                                let split_menu = ui.menu_button(RichText::new(icons::SPLIT).size(theme::size::ICON_LG).color(theme::subtext()), |ui| {
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
                            if self.active_has_explorer() && crate::ui::kit::icon_button(ui, icons::FOLDER, theme::size::ICON_LG, theme::subtext()).on_hover_text("Abrir el explorador de archivos de esta conexión").clicked() {
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
                                    if crate::ui::kit::switch_labeled(ui, &mut dtr, "DTR").clicked() {
                                        session.control(term::Control::Dtr(dtr));
                                    }
                                    let mut rts = status.rts;
                                    if crate::ui::kit::switch_labeled(ui, &mut rts, "RTS").clicked() {
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
        if (!self.open_queue.is_empty() || self.restore_prompt || self.pending_migration.is_some()) && self.window_boot.elapsed() < RESTORE_PROMPT_DELAY {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.tabs.len() != tabs_before {
            ctx.request_repaint();
        }

        let finished_restoring = self.restore_active.is_none();
        if self.tabs.is_empty() && finished_restoring && self.open_queue.is_empty() {
            if self.window_title != "omni-term" {
                ctx.send_viewport_cmd(ViewportCommand::Title("omni-term".to_string()));
                self.window_title = "omni-term".to_string();
            }
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
                        ui.painter().rect_filled(line, CornerRadius::same(theme::radius::TINY), color);
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
                                CornerRadius::same(theme::radius::SMALL),
                                egui::Stroke::new(theme::stroke::MEDIUM, identity.unwrap_or_else(theme::accent).gamma_multiply(0.7)),
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
                Some(view::Request::Terminal(endpoint, container)) => self.open_container_terminal(endpoint, &container),
                Some(view::Request::Confirm(endpoint, container, action)) => {
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
                                clicked = crate::ui::kit::button_with(ui, button, crate::ui::kit::Variant::Primary).clicked();
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
