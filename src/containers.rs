//! Pestaña «Contenedores»: lista de Docker o Podman con iniciar, detener, reiniciar y abrir un terminal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Align2, CornerRadius, FontFamily, FontId, Layout, Rect, RichText, Sense, TextEdit, Ui, UiBuilder, Vec2};
use tokio::runtime::Handle;

use crate::cursor::HandExt;
use crate::docker::{self, Action, Client, Container, Endpoint};
use crate::theme;

/// Cada cuánto se refresca la lista mientras la pestaña está a la vista.
const REFRESH_EVERY: Duration = Duration::from_secs(3);
/// Si la pestaña no se dibuja durante este tiempo, se deja de consultar al motor.
const STALE_AFTER: Duration = Duration::from_secs(6);
const ROW_HEIGHT: f32 = 58.0;

enum Glyph {
    Text(&'static str),
    /// Cuadrado relleno: el icono de «parar» se dibuja a mano.
    Stop,
}

/// Botón cuadrado con un icono y su explicación al pasar el ratón.
fn icon_button(ui: &mut Ui, enabled: bool, tooltip: &str, glyph: Glyph) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.0), if enabled { Sense::click() } else { Sense::hover() });
    let response = if enabled { response.hand().on_hover_text(tooltip) } else { response.on_hover_text(tooltip) };
    let radius = CornerRadius::same(6);
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, radius, theme::selection());
    }
    ui.painter().rect_stroke(rect, radius, egui::Stroke::new(1.0, theme::selection()), egui::StrokeKind::Inside);
    let ink = if enabled { theme::text() } else { theme::subtext().gamma_multiply(0.5) };
    match glyph {
        Glyph::Text(text) => {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(16.0), ink);
        }
        Glyph::Stop => {
            ui.painter().rect_filled(Rect::from_center_size(rect.center(), Vec2::splat(11.0)), CornerRadius::same(2), ink);
        }
    }
    response
}

#[derive(Default)]
struct Snapshot {
    loaded: bool,
    containers: Vec<Container>,
    error: Option<String>,
}

/// Lo que la vista le pide a la aplicación.
pub enum Request {
    Terminal(Endpoint, Container),
    /// Detener y reiniciar piden confirmación.
    Confirm(Endpoint, Container, Action),
}

pub struct ContainersView {
    endpoints: Vec<Endpoint>,
    selected: usize,
    current: Arc<Mutex<Option<Endpoint>>>,
    snapshot: Arc<Mutex<Snapshot>>,
    last_seen: Arc<Mutex<Instant>>,
    refresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    busy: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<String>>>,
    runtime: Handle,
    wake: Arc<dyn Fn() + Send + Sync>,
    search: String,
    show_stopped: bool,
}

impl Drop for ContainersView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl ContainersView {
    pub fn new(runtime: Handle, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let endpoints = docker::detect_endpoints();
        let view = Self {
            current: Arc::new(Mutex::new(endpoints.first().cloned())),
            endpoints,
            selected: 0,
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            last_seen: Arc::new(Mutex::new(Instant::now())),
            refresh: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            runtime,
            wake,
            search: String::new(),
            show_stopped: true,
        };
        view.start_polling();
        view
    }

    pub fn label(&self) -> String {
        "Contenedores".into()
    }

    /// Avisos de acciones terminadas (o fallidas) para mostrarlos como notificación.
    pub fn take_messages(&self) -> Vec<String> {
        std::mem::take(&mut *self.messages.lock().unwrap())
    }

    fn start_polling(&self) {
        let (current, snapshot, last_seen, refresh, stop, wake) =
            (self.current.clone(), self.snapshot.clone(), self.last_seen.clone(), self.refresh.clone(), self.stop.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let mut last_fetch: Option<Instant> = None;
            while !stop.load(Ordering::SeqCst) {
                let forced = refresh.swap(false, Ordering::SeqCst);
                let visible = last_seen.lock().unwrap().elapsed() < STALE_AFTER;
                let due = last_fetch.is_none_or(|t| t.elapsed() >= REFRESH_EVERY);
                if forced || (visible && due) {
                    let endpoint = current.lock().unwrap().clone();
                    if let Some(endpoint) = endpoint {
                        let result = Client::new(endpoint.clone()).list().await;
                        // Si se cambió de motor mientras tanto, el resultado ya no vale.
                        if current.lock().unwrap().as_ref() == Some(&endpoint) {
                            let mut snapshot = snapshot.lock().unwrap();
                            snapshot.loaded = true;
                            match result {
                                Ok(list) => {
                                    snapshot.containers = list;
                                    snapshot.error = None;
                                }
                                Err(error) => snapshot.error = Some(format!("{error:#}")),
                            }
                        }
                        last_fetch = Some(Instant::now());
                        wake();
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    /// Ejecuta una acción en segundo plano y avisa del resultado.
    pub fn act(&self, endpoint: Endpoint, container: &Container, action: Action) {
        let (id, name) = (container.id.clone(), container.name.clone());
        self.busy.lock().unwrap().push(id.clone());
        let (busy, messages, refresh, wake) = (self.busy.clone(), self.messages.clone(), self.refresh.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let result = Client::new(endpoint).act(&id, action).await;
            let message = match (&result, action) {
                (Ok(()), Action::Start) => format!("«{name}» iniciado"),
                (Ok(()), Action::Stop) => format!("«{name}» detenido"),
                (Ok(()), Action::Restart) => format!("«{name}» reiniciado"),
                (Err(error), _) => format!("No se pudo con «{name}»: {error}"),
            };
            busy.lock().unwrap().retain(|b| *b != id);
            messages.lock().unwrap().push(message);
            refresh.store(true, Ordering::SeqCst);
            wake();
        });
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        *self.current.lock().unwrap() = self.endpoints.get(index).cloned();
        *self.snapshot.lock().unwrap() = Snapshot::default();
        self.refresh.store(true, Ordering::SeqCst);
    }

    fn redetect(&mut self) {
        self.endpoints = docker::detect_endpoints();
        self.select(0);
    }

    pub fn show(&mut self, ui: &mut Ui, area: Rect) -> Option<Request> {
        *self.last_seen.lock().unwrap() = Instant::now();
        let mut request = None;
        let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

        self.header(ui);
        ui.add_space(4.0);

        if self.endpoints.is_empty() {
            self.empty(ui);
            return None;
        }
        let (loaded, error, containers) = {
            let snapshot = self.snapshot.lock().unwrap();
            (snapshot.loaded, snapshot.error.clone(), snapshot.containers.clone())
        };
        if let Some(error) = error {
            ui.add_space(12.0);
            ui.label(RichText::new(error).color(theme::red()));
            if ui.button("Reintentar").hand().clicked() {
                self.refresh.store(true, Ordering::SeqCst);
            }
            return None;
        }
        if !loaded {
            ui.add_space(12.0);
            ui.label(RichText::new("Cargando contenedores…").color(theme::subtext()));
            return None;
        }

        let needle = self.search.trim().to_lowercase();
        let mut shown: Vec<&Container> = containers
            .iter()
            .filter(|c| self.show_stopped || c.is_running())
            .filter(|c| needle.is_empty() || c.name.to_lowercase().contains(&needle) || c.image.to_lowercase().contains(&needle))
            .collect();
        shown.sort_by(|a, b| b.is_running().cmp(&a.is_running()).then_with(|| a.name.cmp(&b.name)));

        if shown.is_empty() {
            ui.add_space(12.0);
            let text = if containers.is_empty() { "No hay contenedores." } else { "Ningún contenedor coincide." };
            ui.label(RichText::new(text).color(theme::subtext()));
            return None;
        }
        let endpoint = self.endpoints[self.selected.min(self.endpoints.len() - 1)].clone();
        let busy = self.busy.lock().unwrap().clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for container in shown {
                if let Some(asked) = self.row(ui, container, &endpoint, busy.contains(&container.id)) {
                    request = Some(asked);
                }
            }
        });
        request
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Contenedores").font(FontId::new(16.0, crate::ui::strong_family())).color(theme::text()));
            if self.endpoints.len() > 1 {
                ui.add_space(8.0);
                let mut chosen = None;
                for (index, endpoint) in self.endpoints.iter().enumerate() {
                    if ui.selectable_label(index == self.selected, endpoint.engine.name()).hand().clicked() && index != self.selected {
                        chosen = Some(index);
                    }
                }
                if let Some(index) = chosen {
                    self.select(index);
                }
            } else if let Some(endpoint) = self.endpoints.first() {
                ui.label(RichText::new(endpoint.engine.name()).color(theme::subtext()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(crate::icons::REFRESH).frame(false)).on_hover_text("Actualizar ahora").hand().clicked() {
                    self.refresh.store(true, Ordering::SeqCst);
                }
                ui.checkbox(&mut self.show_stopped, "Mostrar apagados").hand();
                ui.scope(|ui| {
                    ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0, theme::surface());
                    ui.add(TextEdit::singleline(&mut self.search).desired_width(180.0).hint_text("Buscar").margin(Vec2::new(8.0, 4.0)));
                });
            });
        });
    }

    fn empty(&mut self, ui: &mut Ui) {
        ui.add_space(24.0);
        ui.label(RichText::new("No se encontró Docker ni Podman").font(FontId::new(16.0, crate::ui::strong_family())).color(theme::text()));
        ui.label(RichText::new("Inicia el motor y vuelve a buscar. Se buscan sus sockets en /var/run/docker.sock, en DOCKER_HOST y en el directorio de ejecución del usuario.").color(theme::subtext()));
        if cfg!(not(unix)) {
            ui.label(RichText::new("En Windows todavía no está soportado (usa una tubería con nombre).").color(theme::subtext()));
        }
        ui.add_space(4.0);
        if ui.button("Buscar de nuevo").hand().clicked() {
            self.redetect();
        }
    }

    fn row(&self, ui: &mut Ui, container: &Container, endpoint: &Endpoint, working: bool) -> Option<Request> {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::hover());
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), theme::surface());
        }
        let mut request = None;

        let mut buttons = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(Vec2::new(10.0, 0.0))).layout(Layout::right_to_left(Align::Center)));
        buttons.spacing_mut().item_spacing.x = 6.0;
        if container.is_running() {
            if icon_button(&mut buttons, !working, "Detener", Glyph::Stop).clicked() {
                request = Some(Request::Confirm(endpoint.clone(), container.clone(), Action::Stop));
            }
            if icon_button(&mut buttons, !working, "Reiniciar", Glyph::Text(crate::icons::REFRESH)).clicked() {
                request = Some(Request::Confirm(endpoint.clone(), container.clone(), Action::Restart));
            }
            if icon_button(&mut buttons, !working, "Abrir un terminal", Glyph::Text(crate::icons::TERMINAL)).clicked() {
                request = Some(Request::Terminal(endpoint.clone(), container.clone()));
            }
        } else {
            if icon_button(&mut buttons, !working, "Iniciar", Glyph::Text(crate::icons::PLAY)).clicked() {
                self.act(endpoint.clone(), container, Action::Start);
            }
            icon_button(&mut buttons, false, "Inicia el contenedor para abrir un terminal", Glyph::Text(crate::icons::TERMINAL));
        }
        let text_right = buttons.min_rect().min.x - 12.0;

        let painter = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, egui::pos2(text_right, rect.max.y)));
        let dot = if container.is_running() {
            theme::identity(3)
        } else if container.failed() {
            theme::red()
        } else {
            theme::subtext()
        };
        painter.circle_filled(egui::pos2(rect.min.x + 16.0, rect.min.y + 20.0), 5.0, dot);
        painter.text(egui::pos2(rect.min.x + 32.0, rect.min.y + 20.0), Align2::LEFT_CENTER, &container.name, FontId::new(14.0, crate::ui::strong_family()), theme::text());
        let status = if working { "Procesando…".to_string() } else { container.status.clone() };
        let mut detail = vec![container.image.clone(), status];
        if !container.ports.is_empty() {
            detail.push(container.ports.clone());
        }
        painter.text(egui::pos2(rect.min.x + 32.0, rect.min.y + 40.0), Align2::LEFT_CENTER, detail.join("  ·  "), FontId::new(12.0, FontFamily::Proportional), theme::subtext());
        request
    }
}
