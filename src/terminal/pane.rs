use alacritty_terminal::grid::{Dimensions, Scroll};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, EventFilter, FontFamily, FontId, Key, Pos2, Rect, Sense,
    Stroke, StrokeKind, Vec2,
};
use alacritty_terminal::Term;

use crate::ui::cursor::HandExt;
use crate::terminal::keys;
use crate::terminal::mouse::{self, Button as MouseButton, Kind as MouseKind, Mods as MouseMods};
use crate::terminal::search;
use crate::terminal::term::{Proxy, Session};
use crate::ui::theme;

pub const PADDING: f32 = 12.0;

/// Cada cuánto se desplaza la vista al arrastrar una selección fuera del terminal, y cuántas líneas como máximo.
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(40);
const AUTOSCROLL_MAX_LINES: i32 = 6;

pub fn bold_family() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

pub fn italic_family() -> FontFamily {
    FontFamily::Name("mono-italic".into())
}

pub fn bold_italic_family() -> FontFamily {
    FontFamily::Name("mono-bold-italic".into())
}

/// Dibuja el subrayado de una celda según su estilo: simple, doble, ondulado, punteado o discontinuo.
fn draw_underline(painter: &egui::Painter, rect: Rect, flags: Flags, color: Color32, ppp: f32) {
    let px = 1.0 / ppp;
    let base = rect.max.y - px;
    let line = |y: f32| painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y + px)), CornerRadius::ZERO, color);
    if flags.contains(Flags::UNDERCURL) {
        let steps = 12;
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                Pos2::new(rect.min.x + t * rect.width(), base - 1.5 * px - 1.5 * px * (t * std::f32::consts::TAU).sin())
            })
            .collect();
        let width = 1.2 * px;
        painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        line(base);
        line(base - 2.0 * px);
    } else if flags.intersects(Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) {
        let (on, off) = if flags.contains(Flags::DOTTED_UNDERLINE) { (2.0, 2.0) } else { (5.0, 3.0) };
        let mut x = rect.min.x;
        while x < rect.max.x {
            let end = (x + on * px).min(rect.max.x);
            painter.rect_filled(Rect::from_min_max(Pos2::new(x, base), Pos2::new(end, base + px)), CornerRadius::ZERO, color);
            x += (on + off) * px;
        }
    } else {
        line(base);
    }
}

/// Tamaño de celda en puntos, alineado a píxeles físicos.
fn cell_size(ui: &egui::Ui, font_size: f32) -> (f32, f32) {
    let font = FontId::new(font_size, FontFamily::Monospace);
    let ppp = ui.ctx().pixels_per_point();
    let (glyph_w, row_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    ((glyph_w * ppp).round().max(1.0) / ppp, (row_h * ppp).ceil().max(1.0) / ppp)
}

/// Columnas y filas que caben en `area`.
pub fn grid_for(ui: &egui::Ui, area: Rect, font_size: f32) -> (usize, usize) {
    let (cell_w, cell_h) = cell_size(ui, font_size);
    ((area.width() / cell_w).floor().max(1.0) as usize, (area.height() / cell_h).floor().max(1.0) as usize)
}

struct LineMatch {
    line: i32,
    start: usize,
    end: usize,
}

struct SearchState {
    query: String,
    searched: String,
    matches: Vec<LineMatch>,
    current: usize,
    last_run: Instant,
    focus: bool,
}

struct LinkHit {
    line: i32,
    start: usize,
    end: usize,
    url: String,
}

/// Texto de una fila y, para cada carácter, la columna donde empieza.
fn line_text(term: &Term<Proxy>, line: i32) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut columns = Vec::new();
    let row = &term.grid()[Line(line)];
    for column in 0..term.columns() {
        let cell = &row[Column(column)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        text.push(if cell.c == '\0' { ' ' } else { cell.c });
        columns.push(column);
    }
    (text, columns)
}

/// Solo se abren enlaces web o de correo.
fn is_openable(url: &str) -> bool {
    ["http://", "https://", "mailto:"].iter().any(|scheme| url.starts_with(scheme))
}

/// Acciones que el usuario pide con atajos sobre un panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneRequest {
    SplitRight,
    SplitDown,
    Close,
}

/// Vista de un terminal: pinta la rejilla de una sesión y le envía la entrada.
pub struct Pane {
    id: u64,
    request: Option<PaneRequest>,
    pub session: Session,
    debug_keys: bool,
    input_enabled: bool,
    held: Option<MouseButton>,
    last_cell: Option<(usize, usize)>,
    hover_cell: (usize, usize),
    window_focused: bool,
    search: Option<SearchState>,
    link: Option<LinkHit>,
    epoch: Instant,
    /// Pulsaciones seguidas del botón izquierdo (1 simple, 2 palabra, 3 línea) y cuándo y dónde fue la última.
    click_count: u8,
    last_press: Option<(Instant, Pos2)>,
    /// Enlace bajo el puntero cuando se abrió el menú contextual.
    menu_link: Option<String>,
    /// Selección en curso con el ratón, para copiarla a la selección primaria al soltar.
    selecting: bool,
    /// Punto del último clic simple: desde ahí se extiende la selección con Mayús+clic.
    anchor: Option<(Point, Side)>,
    /// El clic en curso se pulsó con Mayús (extiende la selección en vez de limpiarla).
    extending: bool,
    /// Último desplazamiento automático al arrastrar fuera del terminal.
    last_autoscroll: Option<Instant>,
    snippet_request: Option<String>,
    /// Pegado con saltos de línea a la espera de que la app decida si pide confirmación.
    paste_request: Option<String>,
}

impl Pane {
    pub fn new(session: Session, debug_keys: bool, id: u64) -> Self {
        Self { id, request: None, session, debug_keys, input_enabled: true, held: None, last_cell: None, hover_cell: (0, 0), window_focused: true, search: None, link: None, epoch: Instant::now(), click_count: 0, last_press: None, menu_link: None, selecting: false, anchor: None, extending: false, last_autoscroll: None, snippet_request: None, paste_request: None }
    }

    /// Escribe `text` como un pegado (con pegado entre corchetes si la aplicación lo pidió).
    /// Con `run`, añade Enter para ejecutarlo.
    pub fn insert_text(&self, text: &str, run: bool) {
        let mode = *self.session.term.lock().unwrap().mode();
        let mut bytes = paste_bytes(text.trim_end_matches(['\n', '\r']), mode);
        if run {
            bytes.push(b'\r');
        }
        self.session.term.lock().unwrap().scroll_display(Scroll::Bottom);
        self.session.write(&bytes);
    }

    /// Texto seleccionado con el ratón, si lo hay.
    pub fn selection_text(&self) -> Option<String> {
        self.session.term.lock().unwrap().selection_to_string().filter(|t| !t.is_empty())
    }

    pub fn copy_selection_to(&self, ctx: &egui::Context) {
        if let Some(text) = self.selection_text() {
            ctx.copy_text(text);
        }
    }

    pub fn paste_clipboard(&mut self) {
        if let Some(text) = clipboard_text() {
            self.paste(text);
        }
    }

    /// Pega `text`; si tiene saltos de línea lo deja en `paste_request` para que la app pueda confirmarlo.
    fn paste(&mut self, text: String) {
        if text.contains(['\n', '\r']) {
            self.paste_request = Some(text);
        } else {
            self.write_paste(&text);
        }
    }

    /// Escribe `text` como pegado, sin más comprobaciones.
    pub fn write_paste(&self, text: &str) {
        let mode = *self.session.term.lock().unwrap().mode();
        self.session.term.lock().unwrap().scroll_display(Scroll::Bottom);
        self.session.write(&paste_bytes(text, mode));
    }

    pub fn take_paste(&mut self) -> Option<String> {
        self.paste_request.take()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn select_all(&self) {
        let mut term = self.session.term.lock().unwrap();
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
    }

    /// Texto que el usuario pidió guardar como snippet desde el menú contextual.
    pub fn take_snippet(&mut self) -> Option<String> {
        self.snippet_request.take()
    }

    pub fn take_request(&mut self) -> Option<PaneRequest> {
        self.request.take()
    }

    pub fn title(&self) -> String {
        self.session.title.lock().unwrap().clone()
    }

    /// `input_enabled` es falso mientras hay un diálogo abierto, para no robarle el foco.
    pub fn show(&mut self, ui: &mut egui::Ui, area: Rect, input_enabled: bool, font_size: f32) {
        self.input_enabled = input_enabled;
        self.terminal(ui, area, font_size);
    }
}

impl Pane {
    fn terminal(&mut self, ui: &mut egui::Ui, area: Rect, font_size: f32) {
        let font = FontId::new(font_size, FontFamily::Monospace);
        let bold = FontId::new(font_size, bold_family());
        let italic = FontId::new(font_size, italic_family());
        let bold_italic = FontId::new(font_size, bold_italic_family());
        let ppp = ui.ctx().pixels_per_point();
        let (cell_w, cell_h) = cell_size(ui, font_size);
        let area = Rect::from_min_size(snap(area.min, ppp), area.size());

        let cols = (area.width() / cell_w).floor().max(1.0) as usize;
        let rows = (area.height() / cell_h).floor().max(1.0) as usize;
        self.session.resize(cols, rows);

        let id = ui.scope_id().with(("terminal", self.id));
        let response = ui.interact(area, id, Sense::click_and_drag());
        // No se roba el foco a otros campos de texto (búsqueda del sidebar, renombrar pestaña).
        if self.input_enabled && self.search.is_none() && ui.memory(|m| m.focused().is_none_or(|f| f == id)) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            );
        });
        // Cursor de texto solo sobre el terminal; fuera, cada elemento pone el suyo.
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        let to_point = |pos: Pos2, display_offset: usize| {
            let rel = pos - area.min;
            let col = (rel.x / cell_w).floor().clamp(0.0, cols as f32 - 1.0);
            let row = (rel.y / cell_h).floor().clamp(0.0, rows as f32 - 1.0);
            let side = if (rel.x / cell_w).fract() < 0.5 { Side::Left } else { Side::Right };
            let point = viewport_to_point(display_offset, Point::new(row as usize, Column(col as usize)));
            (point, side)
        };

        let mode = *self.session.term.lock().unwrap().mode();
        let shift = ui.input(|i| i.modifiers.shift);
        // Con Mayús se salta el reporte y se puede seleccionar texto aunque la aplicación use el ratón.
        let reporting = self.input_enabled && mouse::is_reporting(mode) && !shift;
        self.send_focus_events(ui, mode);
        if reporting {
            self.report_mouse(ui, area, (cell_w, cell_h), (cols, rows), mode);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        } else {
            self.held = None;
        }

        if !reporting {
            let mut term = self.session.term.lock().unwrap();
            let offset = term.grid().display_offset();
            let press = ui.input(|i| i.pointer.primary_pressed()).then(|| ui.input(|i| i.pointer.interact_pos())).flatten().filter(|p| area.contains(*p) && response.hovered());
            if let Some(pos) = press {
                let repeated = self.last_press.is_some_and(|(at, from)| at.elapsed() < Duration::from_millis(450) && (pos - from).length() < 6.0);
                self.click_count = if repeated { self.click_count % 3 + 1 } else { 1 };
                self.last_press = Some((Instant::now(), pos));
                let kind = match self.click_count {
                    2 => Some(SelectionType::Semantic),
                    3 => Some(SelectionType::Lines),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let (point, side) = to_point(pos, offset);
                    term.selection = Some(Selection::new(kind, point, side));
                } else {
                    // Mayús+clic extiende la selección hasta el punto pulsado, aunque se haya hecho scroll entre medias.
                    let (point, side) = to_point(pos, offset);
                    if shift {
                        if let Some(selection) = term.selection.as_mut() {
                            selection.update(point, side);
                        } else if let Some((anchor, anchor_side)) = self.anchor {
                            let mut selection = Selection::new(SelectionType::Simple, anchor, anchor_side);
                            selection.update(point, side);
                            term.selection = Some(selection);
                        }
                    } else {
                        self.anchor = Some((point, side));
                    }
                }
                self.extending = shift && self.click_count == 1;
            }
            if ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) && response.hovered() {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let (point, side) = to_point(pos, offset);
                    let inside = term.selection.as_ref().and_then(|s| s.to_range(&term)).is_some_and(|range| range.contains(point));
                    if !inside {
                        let blank = matches!(term.grid()[point.line][point.column].c, ' ' | '\0');
                        term.selection = (!blank).then(|| Selection::new(SelectionType::Semantic, point, side));
                    }
                }
            }
            if press.is_some() {
                self.selecting = true;
            }
            let multi_click = self.click_count >= 2;
            if let Some(pos) = response.interact_pointer_pos() {
                if response.drag_started_by(egui::PointerButton::Primary) && !multi_click {
                    if !(self.extending && term.selection.is_some()) {
                        // `drag_started` llega cuando el ratón ya se movió unos píxeles: el inicio es donde se pulsó.
                        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                        let (point, side) = to_point(origin, offset);
                        self.anchor = Some((point, side));
                        let mut selection = Selection::new(SelectionType::Simple, point, side);
                        let (now, now_side) = to_point(pos, offset);
                        selection.update(now, now_side);
                        term.selection = Some(selection);
                    }
                } else if response.dragged_by(egui::PointerButton::Primary) {
                    // Arrastrando por encima o por debajo del terminal, la vista se desplaza y la selección la sigue.
                    let beyond = (area.min.y - pos.y).max(pos.y - area.max.y);
                    let mut offset = offset;
                    if beyond > 0.0 {
                        ui.ctx().request_repaint_after(AUTOSCROLL_INTERVAL);
                        if self.last_autoscroll.is_none_or(|at| at.elapsed() >= AUTOSCROLL_INTERVAL) {
                            let lines = ((beyond / cell_h).ceil() as i32).clamp(1, AUTOSCROLL_MAX_LINES);
                            term.scroll_display(Scroll::Delta(if pos.y < area.min.y { lines } else { -lines }));
                            self.last_autoscroll = Some(Instant::now());
                            offset = term.grid().display_offset();
                        }
                    }
                    let (point, side) = to_point(pos, offset);
                    if let Some(selection) = term.selection.as_mut() {
                        selection.update(point, side);
                    }
                }
            }
            if response.clicked() && !multi_click && !self.extending {
                term.selection = None;
            }
            if self.selecting && ui.input(|i| i.pointer.primary_released()) {
                self.selecting = false;
                if let Some(text) = term.selection_to_string().filter(|t| !t.is_empty()) {
                    set_primary_selection(&text);
                }
            }
            if response.hovered() && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Middle)) {
                drop(term);
                self.paste_primary();
            }
        }

        if !reporting {
            if response.secondary_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.menu_link = self.link_at(pos, area, (cell_w, cell_h), (cols, rows)).map(|l| l.url);
                }
            }
            let selection = self.selection_text();
            let alt_screen = mode.contains(TermMode::ALT_SCREEN);
            response.context_menu(|ui| self.terminal_menu(ui, selection, alt_screen));
        }

        self.update_link(ui, area, (cell_w, cell_h), (cols, rows), reporting);
        if let Some(link) = &self.link {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if response.clicked() && is_openable(&link.url) {
                let _ = open::that_detached(&link.url);
            }
        }

        let searching = self.search.is_some();
        if self.input_enabled {
            self.handle_scroll(ui, cell_h, reporting);
            let typing_elsewhere = ui.memory(|m| m.focused().is_some_and(|f| f != id));
            if !searching && !typing_elsewhere {
                self.handle_input(ui);
            }
        }
        // Solo en Windows: lo necesitan Win+V y Win+. En Linux, activar el IME hace que fcitx5/ibus intercepten el
        // teclado y se pierdan eventos de soltar tecla (la tecla se queda repitiendo en equipos lentos).
        if cfg!(windows) && self.input_enabled && ui.memory(|m| m.has_focus(id)) {
            self.publish_ime(ui, area, cell_w, cell_h);
        }
        if self.window_focused && self.session.term.lock().unwrap().cursor_style().blinking {
            ui.ctx().request_repaint_after(Duration::from_millis(265));
        }
        self.refresh_search();
        let highlights = self.highlights(rows);
        self.paint_grid(ui, area, &font, [&bold, &italic, &bold_italic], cell_w, cell_h, &highlights);
        self.scrollbar(ui, area, rows);
        self.search_bar(ui, area);
    }

    /// Barra de scroll sobre el historial; solo aparece si lo hay. Se arrastra con el ratón.
    fn scrollbar(&mut self, ui: &mut egui::Ui, area: Rect, rows: usize) {
        let (history, offset) = {
            let term = self.session.term.lock().unwrap();
            (term.grid().history_size(), term.grid().display_offset())
        };
        if history == 0 {
            return;
        }
        let track = Rect::from_min_max(Pos2::new(area.max.x + 3.0, area.min.y), Pos2::new(area.max.x + 9.0, area.max.y));
        let thumb_len = (track.height() * rows as f32 / (history + rows) as f32).clamp(28.0, track.height());
        let travel = (track.height() - thumb_len).max(1.0);
        let thumb_top = track.min.y + travel * (1.0 - offset as f32 / history as f32);

        let hit = track.expand2(Vec2::new(4.0, 0.0));
        let response = ui.interact(hit, ui.scope_id().with(("scrollbar", self.id)), Sense::click_and_drag()).grab();
        if let Some(pos) = response.interact_pointer_pos() {
            if response.dragged() || response.is_pointer_button_down_on() {
                let ratio = ((pos.y - track.min.y - thumb_len / 2.0) / travel).clamp(0.0, 1.0);
                let target = ((1.0 - ratio) * history as f32).round() as i32;
                self.session.term.lock().unwrap().scroll_display(Scroll::Delta(target - offset as i32));
            }
        }
        let active = response.hovered() || response.dragged();
        let color = if active { theme::subtext() } else { theme::selection() };
        let thumb = Rect::from_min_size(Pos2::new(track.min.x, thumb_top), Vec2::new(track.width(), thumb_len));
        ui.painter().rect_filled(thumb, CornerRadius::same(theme::radius::TINY), color);
    }

    /// Abre la búsqueda, ya con `query` escrita.
    fn open_search(&mut self, query: String) {
        self.search = Some(SearchState {
            query,
            searched: String::new(),
            matches: Vec::new(),
            current: 0,
            last_run: Instant::now(),
            focus: true,
        });
    }

    /// Pega la selección primaria (botón central del ratón, solo Linux).
    fn paste_primary(&mut self) {
        if let Some(text) = primary_text() {
            self.paste(text);
        }
    }

    /// Menú del clic derecho: las entradas que no aplican no se muestran.
    fn terminal_menu(&mut self, ui: &mut egui::Ui, selection: Option<String>, alt_screen: bool) {
        ui.set_min_width(260.0);
        let entry = |ui: &mut egui::Ui, label: &str, shortcut: &str| -> bool {
            let mut button = egui::Button::new(label);
            if !shortcut.is_empty() {
                button = button.shortcut_text(shortcut);
            }
            ui.add(button).clicked()
        };
        if let Some(url) = self.menu_link.clone() {
            if entry(ui, "Abrir enlace", "") {
                if is_openable(&url) {
                    let _ = open::that_detached(&url);
                }
                ui.close();
            }
            if entry(ui, "Copiar enlace", "") {
                ui.ctx().copy_text(url);
                ui.close();
            }
            ui.separator();
        }
        if let Some(text) = &selection {
            if entry(ui, "Copiar", "Ctrl+Shift+C") {
                ui.ctx().copy_text(text.clone());
                ui.close();
            }
        }
        if entry(ui, "Pegar", "Ctrl+Shift+V") {
            self.paste_clipboard();
            ui.close();
        }
        if entry(ui, "Seleccionar todo", "") {
            self.select_all();
            ui.close();
        }
        if let Some(text) = &selection {
            let first_line = text.lines().next().unwrap_or("").trim();
            let shown: String = first_line.chars().take(24).collect();
            let ellipsis = if first_line.chars().count() > 24 { "…" } else { "" };
            if entry(ui, &format!("Buscar «{shown}{ellipsis}»"), "") {
                self.open_search(first_line.to_string());
                ui.close();
            }
            if entry(ui, "Guardar selección como snippet", "") {
                self.snippet_request = Some(text.clone());
                ui.close();
            }
        }
        ui.separator();
        if entry(ui, "Dividir a la derecha", "Ctrl+Shift+D") {
            self.request = Some(PaneRequest::SplitRight);
            ui.close();
        }
        if entry(ui, "Dividir hacia abajo", "Ctrl+Shift+E") {
            self.request = Some(PaneRequest::SplitDown);
            ui.close();
        }
        // Ctrl+L limpia la pantalla en un shell; una aplicación a pantalla completa podría tomarlo de otra forma.
        if !alt_screen {
            ui.separator();
            if entry(ui, "Limpiar pantalla", "") {
                self.session.write(b"\x0c");
                ui.close();
            }
        }
    }

    pub fn toggle_search(&mut self) {
        self.search = match self.search {
            Some(_) => None,
            None => Some(SearchState {
                query: String::new(),
                searched: String::new(),
                matches: Vec::new(),
                current: 0,
                last_run: Instant::now(),
                focus: true,
            }),
        };
    }

    /// Recalcula las coincidencias cuando cambia la búsqueda y, mientras haya salida nueva, cada poco.
    fn refresh_search(&mut self) {
        let Some(state) = &self.search else { return };
        if state.query == state.searched && state.last_run.elapsed() < Duration::from_millis(800) {
            return;
        }
        let query = state.query.clone();
        let changed = query != state.searched;
        let mut matches = Vec::new();
        if !query.is_empty() {
            let term = self.session.term.lock().unwrap();
            let (top, bottom) = (term.topmost_line().0, term.bottommost_line().0);
            'lines: for line in top..=bottom {
                let (text, columns) = line_text(&term, line);
                for (start, end) in search::find(&text, &query) {
                    matches.push(LineMatch { line, start: columns[start], end: columns[end - 1] + 1 });
                    if matches.len() >= 5000 {
                        break 'lines;
                    }
                }
            }
        }
        let Some(state) = &mut self.search else { return };
        state.searched = query;
        state.last_run = Instant::now();
        state.current = if changed { matches.len().saturating_sub(1) } else { state.current.min(matches.len().saturating_sub(1)) };
        let target = matches.get(state.current).map(|m| m.line);
        state.matches = matches;
        if changed {
            if let Some(line) = target {
                self.reveal(line);
            }
        }
    }

    /// Desplaza el historial para que `line` quede visible.
    fn reveal(&mut self, line: i32) {
        let mut term = self.session.term.lock().unwrap();
        let rows = term.screen_lines() as i32;
        let history = term.history_size() as i32;
        let offset = term.grid().display_offset() as i32;
        let (top, bottom) = (-offset, -offset + rows - 1);
        if line < top || line > bottom {
            let wanted = (-line + rows / 2).clamp(0, history);
            term.scroll_display(Scroll::Delta(wanted - offset));
        }
    }

    fn step_search(&mut self, forward: bool) {
        let Some(state) = &mut self.search else { return };
        if state.matches.is_empty() {
            return;
        }
        let count = state.matches.len();
        state.current = if forward { (state.current + 1) % count } else { (state.current + count - 1) % count };
        let line = state.matches[state.current].line;
        self.reveal(line);
    }

    /// Coincidencias visibles por línea: columnas inicial y final, y si es la actual.
    fn highlights(&self, rows: usize) -> HashMap<i32, Vec<(usize, usize, bool)>> {
        let mut map: HashMap<i32, Vec<(usize, usize, bool)>> = HashMap::new();
        let Some(state) = &self.search else { return map };
        let offset = self.session.term.lock().unwrap().grid().display_offset() as i32;
        let (top, bottom) = (-offset, -offset + rows as i32 - 1);
        for (index, found) in state.matches.iter().enumerate() {
            if found.line >= top && found.line <= bottom {
                map.entry(found.line).or_default().push((found.start, found.end, index == state.current));
            }
        }
        map
    }

    fn search_bar(&mut self, ui: &egui::Ui, area: Rect) {
        let Some(state) = &mut self.search else { return };
        let mut close = false;
        let mut step: Option<bool> = None;
        egui::Area::new(egui::Id::NULL.with(("search-bar", self.id)))
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(Pos2::new(area.max.x - 6.0, area.min.y + 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let field = ui.add(egui::TextEdit::singleline(&mut state.query).hint_text("Buscar").desired_width(190.0));
                        if state.focus {
                            field.request_focus();
                            state.focus = false;
                        }
                        let count = if state.query.is_empty() {
                            String::new()
                        } else if state.matches.is_empty() {
                            "Sin resultados".to_string()
                        } else {
                            format!("{} de {}", state.current + 1, state.matches.len())
                        };
                        ui.label(egui::RichText::new(count).small().color(theme::subtext()));
                        let small = |ui: &mut egui::Ui, text: &str| crate::ui::kit::button_with(ui, egui::Button::new(egui::RichText::new(text).small()), crate::ui::kit::Variant::Ghost);
                        if small(ui, "Anterior").clicked() {
                            step = Some(false);
                        }
                        if small(ui, "Siguiente").clicked() {
                            step = Some(true);
                        }
                        if small(ui, "Cerrar").clicked() {
                            close = true;
                        }
                        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            step = Some(!ui.input(|i| i.modifiers.shift));
                            field.request_focus();
                        }
                    });
                });
            });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        if let Some(forward) = step {
            self.step_search(forward);
        }
        if close {
            self.search = None;
        }
    }

    /// Enlace bajo el puntero (con Ctrl pulsado): hipervínculo OSC 8 o una URL en el texto.
    fn update_link(&mut self, ui: &egui::Ui, area: Rect, cell: (f32, f32), grid: (usize, usize), reporting: bool) {
        let (ctrl, pos) = ui.input(|i| (i.modifiers.ctrl, i.pointer.hover_pos()));
        self.link = pos.filter(|p| ctrl && !reporting && area.contains(*p)).and_then(|p| self.link_at(p, area, cell, grid));
    }

    /// Enlace (hipervínculo OSC 8 o URL en el texto) bajo `pos`.
    fn link_at(&self, pos: Pos2, area: Rect, (cell_w, cell_h): (f32, f32), (cols, rows): (usize, usize)) -> Option<LinkHit> {
        let col = ((pos.x - area.min.x) / cell_w).floor().clamp(0.0, cols as f32 - 1.0) as usize;
        let row = ((pos.y - area.min.y) / cell_h).floor().clamp(0.0, rows as f32 - 1.0) as usize;
        let term = self.session.term.lock().unwrap();
        let line = row as i32 - term.grid().display_offset() as i32;

        let hyperlink = term.grid()[Line(line)][Column(col)].hyperlink().map(|h| h.uri().to_string());
        if let Some(url) = hyperlink {
            let same = |c: usize| term.grid()[Line(line)][Column(c)].hyperlink().is_some_and(|h| h.uri() == url);
            let (mut start, mut end) = (col, col + 1);
            while start > 0 && same(start - 1) {
                start -= 1;
            }
            while end < cols && same(end) {
                end += 1;
            }
            return Some(LinkHit { line, start, end, url });
        }

        let (text, columns) = line_text(&term, line);
        for (start, end, url) in search::find_urls(&text) {
            let (first, last) = (columns[start], columns[end - 1] + 1);
            if (first..last).contains(&col) {
                return Some(LinkHit { line, start: first, end: last, url });
            }
        }
        None
    }

    /// Avisa a la aplicación cuando la ventana gana o pierde el foco (modo 1004).
    fn send_focus_events(&mut self, ui: &egui::Ui, mode: TermMode) {
        let focused = ui.input(|i| i.focused);
        if focused != self.window_focused {
            self.window_focused = focused;
            if mode.contains(TermMode::FOCUS_IN_OUT) {
                self.session.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
            }
        }
    }

    /// Envía a la aplicación los pulsados, soltados y movimientos del ratón sobre el terminal.
    fn report_mouse(&mut self, ui: &egui::Ui, area: Rect, (cell_w, cell_h): (f32, f32), (cols, rows): (usize, usize), mode: TermMode) {
        let cell_at = |pos: Pos2| {
            let col = ((pos.x - area.min.x) / cell_w).floor().clamp(0.0, cols as f32 - 1.0) as usize;
            let row = ((pos.y - area.min.y) / cell_h).floor().clamp(0.0, rows as f32 - 1.0) as usize;
            (col, row)
        };
        let mods = mouse_mods(ui);
        let events = ui.input(|i| i.events.clone());
        let mut out: Vec<u8> = Vec::new();
        for event in events {
            match event {
                Event::PointerButton { pos, button, pressed, .. } => {
                    let Some(button) = mouse_button(button) else { continue };
                    let inside = area.contains(pos);
                    let (col, row) = cell_at(pos);
                    self.hover_cell = (col, row);
                    if pressed && inside {
                        self.held = Some(button);
                        self.last_cell = Some((col, row));
                        out.extend(mouse::encode(MouseKind::Press(button), col, row, mods, mode).unwrap_or_default());
                    } else if !pressed && self.held == Some(button) {
                        self.held = None;
                        out.extend(mouse::encode(MouseKind::Release(button), col, row, mods, mode).unwrap_or_default());
                    }
                }
                Event::PointerMoved(pos) => {
                    if !area.contains(pos) && self.held.is_none() {
                        continue;
                    }
                    let cell = cell_at(pos);
                    self.hover_cell = cell;
                    if self.last_cell != Some(cell) {
                        self.last_cell = Some(cell);
                        out.extend(mouse::encode(MouseKind::Motion(self.held), cell.0, cell.1, mods, mode).unwrap_or_default());
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            self.session.write(&out);
        }
    }

    fn handle_scroll(&mut self, ui: &egui::Ui, cell_h: f32, reporting: bool) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        let lines = (dy / cell_h).round() as i32;
        if lines == 0 {
            return;
        }
        if reporting {
            let mode = *self.session.term.lock().unwrap().mode();
            let (col, row) = self.hover_cell;
            let kind = if lines > 0 { MouseKind::WheelUp } else { MouseKind::WheelDown };
            if let Some(bytes) = mouse::encode(kind, col, row, mouse_mods(ui), mode) {
                for _ in 0..lines.unsigned_abs() {
                    self.session.write(&bytes);
                }
            }
            return;
        }
        let mut term = self.session.term.lock().unwrap();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            drop(term);
            for _ in 0..lines.unsigned_abs() {
                self.session.write(seq);
            }
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
    }

    fn handle_input(&mut self, ui: &egui::Ui) {
        let events = ui.input(|i| i.events.clone());
        let mode = *self.session.term.lock().unwrap().mode();
        let ctrl_held = ui.input(|i| i.modifiers.ctrl);
        let mut out: Vec<u8> = Vec::new();
        let mut pasted = None;

        for event in &events {
            match event {
                Event::Key { key, pressed, modifiers, repeat, .. } => {
                    // En Windows Ctrl+V pega, como en Windows Terminal: el historial del portapapeles (Win+V)
                    // pega simulando esa tecla.
                    if cfg!(windows) && *key == Key::V && modifiers.ctrl && !modifiers.shift && !modifiers.alt {
                        if *pressed {
                            pasted = clipboard_text();
                        }
                        continue;
                    }
                    let reserved = modifiers.ctrl && modifiers.shift && matches!(key, Key::C | Key::V | Key::F | Key::D | Key::E | Key::W);
                    if reserved {
                        if *pressed && matches!(key, Key::D | Key::E | Key::W) {
                            self.request = Some(match key {
                                Key::D => PaneRequest::SplitRight,
                                Key::E => PaneRequest::SplitDown,
                                _ => PaneRequest::Close,
                            });
                        } else if *pressed && *key == Key::F {
                            self.toggle_search();
                        } else if *pressed && *key == Key::C {
                            self.copy_selection(ui);
                        } else if *pressed && *key == Key::V {
                            pasted = clipboard_text();
                        }
                        continue;
                    }
                    let press = keys::Press { key: *key, mods: *modifiers, pressed: *pressed, repeat: *repeat };
                    let bytes = keys::encode(&press, mode);
                    if self.debug_keys {
                        eprintln!("key {key:?} {modifiers:?} pressed={pressed} repeat={repeat} -> {bytes:?}");
                    }
                    out.extend(bytes.unwrap_or_default());
                }
                Event::Text(text) if !ctrl_held && !keys::reports_all_keys(mode) => {
                    if self.debug_keys {
                        eprintln!("text {text:?}");
                    }
                    out.extend(text.as_bytes());
                }
                // Texto confirmado por el IME o insertado desde un panel del sistema; con saltos de línea es un pegado.
                Event::Ime(egui::ImeEvent::Commit(text)) if !text.is_empty() => {
                    if self.debug_keys {
                        eprintln!("ime {text:?}");
                    }
                    if text.contains(['\n', '\r']) {
                        pasted = Some(text.clone());
                    } else {
                        out.extend(text.as_bytes());
                    }
                }
                _ => {}
            }
        }

        if !out.is_empty() {
            let mut term = self.session.term.lock().unwrap();
            term.scroll_display(Scroll::Bottom);
            drop(term);
            self.session.write(&out);
        }
        if let Some(text) = pasted {
            self.paste(text);
        }
    }

    /// Activa el IME mientras el terminal tiene el foco. Sin él, Windows no puede llevar el teclado a sus
    /// paneles (historial del portapapeles con Win+V, emojis con Win+.), y los métodos de entrada
    /// colocan su ventana junto al cursor.
    fn publish_ime(&self, ui: &egui::Ui, area: Rect, cell_w: f32, cell_h: f32) {
        let cursor = {
            let term = self.session.term.lock().unwrap();
            point_to_viewport(term.grid().display_offset(), term.grid().cursor.point)
        };
        let cursor_rect = cursor.map_or(area, |vp| {
            Rect::from_min_size(area.min + Vec2::new(vp.column.0 as f32 * cell_w, vp.line as f32 * cell_h), Vec2::new(cell_w, cell_h))
        });
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect: area, cursor_rect, should_interrupt_composition: false });
        });
    }

    fn copy_selection(&self, ui: &egui::Ui) {
        let text = self.session.term.lock().unwrap().selection_to_string();
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            ui.ctx().copy_text(text);
        }
    }

    fn paint_grid(
        &self,
        ui: &egui::Ui,
        area: Rect,
        font: &FontId,
        faces: [&FontId; 3],
        cell_w: f32,
        cell_h: f32,
        highlights: &HashMap<i32, Vec<(usize, usize, bool)>>,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(area.expand(PADDING));
        let term = self.session.term.lock().unwrap();
        let content = term.renderable_content();
        let offset = content.display_offset;
        let cursor = content.cursor;
        // Sin foco, el cursor se dibuja hueco; con parpadeo, se apaga cada medio segundo.
        let shape = match cursor.shape {
            CursorShape::Hidden => CursorShape::Hidden,
            shape if self.window_focused && self.input_enabled => shape,
            _ => CursorShape::HollowBlock,
        };
        let blink_on = !(term.cursor_style().blinking && self.window_focused && self.input_enabled) || self.epoch.elapsed().as_millis() / 530 % 2 == 0;
        let cursor_visible = offset == 0 && shape != CursorShape::Hidden && blink_on;

        let cell_rect = |line: usize, col: usize, cols: usize| {
            let x0 = area.min.x + col as f32 * cell_w;
            let y0 = area.min.y + line as f32 * cell_h;
            snap_rect(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + cols as f32 * cell_w, y0 + cell_h)),
                ppp,
            )
        };

        struct Glyph {
            line: usize,
            col: usize,
            wide: bool,
            c: char,
            fg: Color32,
            bg: Color32,
            flags: Flags,
            underline: Color32,
        }

        let mut glyphs: Vec<Glyph> = Vec::new();
        // Fondos contiguos de la misma línea y color se pintan como un solo rectángulo.
        let mut runs: Vec<(usize, usize, usize, Color32)> = Vec::new();

        for indexed in content.display_iter {
            let Some(vp) = point_to_viewport(offset, indexed.point) else { continue };
            let cell = &indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let wide = cell.flags.contains(Flags::WIDE_CHAR);

            let (mut fg, mut bg) = (theme::resolve_styled(cell.fg, cell.flags.contains(Flags::BOLD)), theme::resolve(cell.bg));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = theme::dim(fg);
            }
            let underline = cell.underline_color().map(theme::resolve);
            let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
            let at_cursor = cursor_visible && shape == CursorShape::Block && indexed.point == cursor.point;

            let found = highlights.get(&indexed.point.line.0).and_then(|ranges| {
                ranges.iter().find(|(start, end, _)| (*start..*end).contains(&indexed.point.column.0)).map(|r| r.2)
            });

            let fill = if at_cursor {
                fg = theme::base();
                Some(theme::cursor())
            } else if let Some(current) = found {
                if current {
                    fg = theme::base();
                    Some(theme::accent())
                } else {
                    Some(theme::accent().gamma_multiply(0.35))
                }
            } else if selected {
                Some(theme::selection())
            } else if bg != theme::base() {
                Some(bg)
            } else {
                None
            };
            let shown_bg = fill.unwrap_or(theme::base());

            if let Some(color) = fill {
                let span = if wide { 2 } else { 1 };
                match runs.last_mut() {
                    Some((line, _, end, c)) if *line == vp.line && *end == vp.column.0 && *c == color => {
                        *end += span;
                    }
                    _ => runs.push((vp.line, vp.column.0, vp.column.0 + span, color)),
                }
            }

            let visible = cell.c != ' ' && cell.c != '\0' && !cell.flags.contains(Flags::HIDDEN);
            if visible || cell.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT) {
                glyphs.push(Glyph {
                    line: vp.line,
                    col: vp.column.0,
                    wide,
                    c: if visible { cell.c } else { ' ' },
                    fg,
                    bg: shown_bg,
                    flags: cell.flags,
                    underline: underline.unwrap_or(fg),
                });
            }
        }

        for (line, start, end, color) in runs {
            let rect = cell_rect(line, start, end - start);
            painter.rect_filled(rect, CornerRadius::ZERO, color);
        }

        for g in &glyphs {
            let rect = cell_rect(g.line, g.col, if g.wide { 2 } else { 1 });
            if g.c != ' ' && !draw_cell_graphic(&painter, rect, g.c, g.fg, g.bg, ppp) {
                let face = match (g.flags.contains(Flags::BOLD), g.flags.contains(Flags::ITALIC)) {
                    (true, true) => faces[2],
                    (true, false) => faces[0],
                    (false, true) => faces[1],
                    (false, false) => &font,
                };
                painter.text(rect.min, Align2::LEFT_TOP, g.c, face.clone(), g.fg);
            }
            if g.flags.intersects(Flags::ALL_UNDERLINES) {
                draw_underline(&painter, rect, g.flags, g.underline, ppp);
            }
            if g.flags.contains(Flags::STRIKEOUT) {
                let y = snap_f(rect.center().y, ppp);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y + 1.0 / ppp)),
                    CornerRadius::ZERO,
                    g.fg,
                );
            }
        }

        if let Some(link) = &self.link {
            let viewport_line = link.line + offset as i32;
            if viewport_line >= 0 {
                let rect = cell_rect(viewport_line as usize, link.start, link.end - link.start);
                let y = rect.max.y - 1.0 / ppp;
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(rect.min.x, y - 1.0 / ppp), Pos2::new(rect.max.x, y + 1.0 / ppp)),
                    CornerRadius::ZERO,
                    theme::accent(),
                );
            }
        }

        if cursor_visible && matches!(shape, CursorShape::Underline | CursorShape::Beam) {
            if let Some(vp) = point_to_viewport(offset, cursor.point) {
                let rect = cell_rect(vp.line, vp.column.0, 1);
                let thickness = (2.0 / ppp).max(rect.height() * 0.1);
                let bar = if shape == CursorShape::Underline {
                    Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - thickness), rect.max)
                } else {
                    Rect::from_min_max(rect.min, Pos2::new(rect.min.x + thickness, rect.max.y))
                };
                painter.rect_filled(bar, CornerRadius::ZERO, theme::cursor());
            }
        }

        if cursor_visible && matches!(shape, CursorShape::HollowBlock) {
            if let Some(vp) = point_to_viewport(offset, cursor.point) {
                painter.rect_stroke(
                    cell_rect(vp.line, vp.column.0, 1),
                    CornerRadius::ZERO,
                    Stroke::new(theme::stroke::HAIRLINE, theme::cursor()),
                    StrokeKind::Inside,
                );
            }
        }
    }
}

fn snap_f(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

fn snap(p: Pos2, ppp: f32) -> Pos2 {
    Pos2::new(snap_f(p.x, ppp), snap_f(p.y, ppp))
}

fn snap_rect(r: Rect, ppp: f32) -> Rect {
    Rect::from_min_max(snap(r.min, ppp), snap(r.max, ppp))
}

/// Dibuja bloques (U+2580..259F) y líneas de caja (U+2500..257F) como geometría exacta,
/// para que llenen la celda sin huecos. Devuelve `false` si el carácter debe usar la fuente.
fn draw_cell_graphic(painter: &egui::Painter, rect: Rect, c: char, fg: Color32, bg: Color32, ppp: f32) -> bool {
    let fill = |x0: f32, y0: f32, x1: f32, y1: f32, color: Color32| {
        let r = Rect::from_min_max(
            Pos2::new(rect.min.x + rect.width() * x0, rect.min.y + rect.height() * y0),
            Pos2::new(rect.min.x + rect.width() * x1, rect.min.y + rect.height() * y1),
        );
        painter.rect_filled(snap_rect(r, ppp), CornerRadius::ZERO, color);
    };

    let code = c as u32;
    match code {
        0x2580 => fill(0.0, 0.0, 1.0, 0.5, fg),
        0x2581..=0x2587 => fill(0.0, 1.0 - (code - 0x2580) as f32 / 8.0, 1.0, 1.0, fg),
        0x2588 => fill(0.0, 0.0, 1.0, 1.0, fg),
        0x2589..=0x258F => fill(0.0, 0.0, (0x2590 - code) as f32 / 8.0, 1.0, fg),
        0x2590 => fill(0.5, 0.0, 1.0, 1.0, fg),
        0x2591..=0x2593 => {
            let t = (code - 0x2590) as f32 / 4.0;
            fill(0.0, 0.0, 1.0, 1.0, Color32::lerp_to_gamma(&bg, fg, t));
        }
        0x2594 => fill(0.0, 0.0, 1.0, 0.125, fg),
        0x2595 => fill(0.875, 0.0, 1.0, 1.0, fg),
        0x2596..=0x259F => {
            // Bits: superior izquierda, superior derecha, inferior izquierda, inferior derecha.
            let quadrants: [u8; 10] = [0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111];
            let mask = quadrants[(code - 0x2596) as usize];
            for (bit, (x, y)) in [(0b1000, (0.0, 0.0)), (0b0100, (0.5, 0.0)), (0b0010, (0.0, 0.5)), (0b0001, (0.5, 0.5))] {
                if mask & bit != 0 {
                    fill(x, y, x + 0.5, y + 0.5, fg);
                }
            }
        }
        _ => {
            let Some((l, r, u, d)) = box_segments(c) else { return false };
            let light = (rect.height() * ppp / 14.0).round().max(1.0) / ppp;
            let weight = |w: u8| if w == 2 { light * 2.0 } else { light };
            let center = rect.center();
            let v_half = weight(u.max(d)) / 2.0;
            let h_half = weight(l.max(r)) / 2.0;
            let seg = |x0: f32, y0: f32, x1: f32, y1: f32| {
                painter.rect_filled(
                    snap_rect(Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)), ppp),
                    CornerRadius::ZERO,
                    fg,
                );
            };
            if l > 0 {
                seg(rect.min.x, center.y - weight(l) / 2.0, center.x + v_half, center.y + weight(l) / 2.0);
            }
            if r > 0 {
                seg(center.x - v_half, center.y - weight(r) / 2.0, rect.max.x, center.y + weight(r) / 2.0);
            }
            if u > 0 {
                seg(center.x - weight(u) / 2.0, rect.min.y, center.x + weight(u) / 2.0, center.y + h_half);
            }
            if d > 0 {
                seg(center.x - weight(d) / 2.0, center.y - h_half, center.x + weight(d) / 2.0, rect.max.y);
            }
        }
    }
    true
}

/// Segmentos (izquierda, derecha, arriba, abajo) con grosor 0 = ninguno, 1 = fino, 2 = grueso.
fn box_segments(c: char) -> Option<(u8, u8, u8, u8)> {
    const TABLE: [(u8, u8, u8, u8); 76] = [
        (1, 1, 0, 0), (2, 2, 0, 0), (0, 0, 1, 1), (0, 0, 2, 2),
        (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0),
        (0, 1, 0, 1), (0, 2, 0, 1), (0, 1, 0, 2), (0, 2, 0, 2),
        (1, 0, 0, 1), (2, 0, 0, 1), (1, 0, 0, 2), (2, 0, 0, 2),
        (0, 1, 1, 0), (0, 2, 1, 0), (0, 1, 2, 0), (0, 2, 2, 0),
        (1, 0, 1, 0), (2, 0, 1, 0), (1, 0, 2, 0), (2, 0, 2, 0),
        (0, 1, 1, 1), (0, 2, 1, 1), (0, 1, 2, 1), (0, 1, 1, 2), (0, 1, 2, 2), (0, 2, 2, 1), (0, 2, 1, 2), (0, 2, 2, 2),
        (1, 0, 1, 1), (2, 0, 1, 1), (1, 0, 2, 1), (1, 0, 1, 2), (1, 0, 2, 2), (2, 0, 2, 1), (2, 0, 1, 2), (2, 0, 2, 2),
        (1, 1, 0, 1), (2, 1, 0, 1), (1, 2, 0, 1), (2, 2, 0, 1), (1, 1, 0, 2), (2, 1, 0, 2), (1, 2, 0, 2), (2, 2, 0, 2),
        (1, 1, 1, 0), (2, 1, 1, 0), (1, 2, 1, 0), (2, 2, 1, 0), (1, 1, 2, 0), (2, 1, 2, 0), (1, 2, 2, 0), (2, 2, 2, 0),
        (1, 1, 1, 1), (2, 1, 1, 1), (1, 2, 1, 1), (2, 2, 1, 1), (1, 1, 2, 1), (1, 1, 1, 2), (1, 1, 2, 2),
        (2, 1, 2, 1), (1, 2, 2, 1), (2, 1, 1, 2), (1, 2, 1, 2), (2, 2, 2, 1), (2, 2, 1, 2), (2, 1, 2, 2), (1, 2, 2, 2), (2, 2, 2, 2),
    ];
    let code = c as u32;
    match code {
        0x2500..=0x2503 | 0x250C..=0x254B => Some(TABLE[(code - 0x2500) as usize]),
        0x256D..=0x2570 => Some(match code {
            0x256D => (0, 1, 0, 1),
            0x256E => (1, 0, 0, 1),
            0x256F => (1, 0, 1, 0),
            _ => (0, 1, 1, 0),
        }),
        0x2574 => Some((1, 0, 0, 0)),
        0x2575 => Some((0, 0, 1, 0)),
        0x2576 => Some((0, 1, 0, 0)),
        0x2577 => Some((0, 0, 0, 1)),
        0x2578 => Some((2, 0, 0, 0)),
        0x2579 => Some((0, 0, 2, 0)),
        0x257A => Some((0, 2, 0, 0)),
        0x257B => Some((0, 0, 0, 2)),
        0x257C => Some((1, 2, 0, 0)),
        0x257D => Some((0, 0, 1, 2)),
        0x257E => Some((2, 1, 0, 0)),
        0x257F => Some((0, 0, 2, 1)),
        _ => None,
    }
}

fn mouse_button(button: egui::PointerButton) -> Option<MouseButton> {
    match button {
        egui::PointerButton::Primary => Some(MouseButton::Left),
        egui::PointerButton::Middle => Some(MouseButton::Middle),
        egui::PointerButton::Secondary => Some(MouseButton::Right),
        _ => None,
    }
}

fn mouse_mods(ui: &egui::Ui) -> MouseMods {
    ui.input(|i| MouseMods { shift: i.modifiers.shift, alt: i.modifiers.alt, ctrl: i.modifiers.ctrl })
}

/// Copia a la selección primaria de X11/Wayland, para pegarla con el botón central.
#[cfg(target_os = "linux")]
fn set_primary_selection(text: &str) {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text.to_string());
    }
}

#[cfg(not(target_os = "linux"))]
fn set_primary_selection(_text: &str) {}

#[cfg(target_os = "linux")]
fn primary_text() -> Option<String> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    arboard::Clipboard::new().ok()?.get().clipboard(LinuxClipboardKind::Primary).text().ok().filter(|t| !t.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn primary_text() -> Option<String> {
    None
}

fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok().filter(|t| !t.is_empty())
}

/// Los saltos de línea se envían como `\r`, que es lo que manda Enter: un `\r\n` de Windows llegaría como dos.
fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        [b"\x1b[200~", text.replace('\x1b', "").as_bytes(), b"\x1b[201~"].concat()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::{box_segments, paste_bytes};
    use alacritty_terminal::term::TermMode;

    #[test]
    fn paste_sends_each_line_break_once() {
        assert_eq!(paste_bytes("a\r\nb\nc", TermMode::empty()), b"a\rb\rc");
        assert_eq!(paste_bytes("a\r\nb", TermMode::BRACKETED_PASTE), b"\x1b[200~a\rb\x1b[201~");
    }

    #[test]
    fn box_table_matches_unicode_order() {
        assert_eq!(box_segments('─'), Some((1, 1, 0, 0)));
        assert_eq!(box_segments('┃'), Some((0, 0, 2, 2)));
        assert_eq!(box_segments('┌'), Some((0, 1, 0, 1)));
        assert_eq!(box_segments('┓'), Some((2, 0, 0, 2)));
        assert_eq!(box_segments('├'), Some((0, 1, 1, 1)));
        assert_eq!(box_segments('┬'), Some((1, 1, 0, 1)));
        assert_eq!(box_segments('┼'), Some((1, 1, 1, 1)));
        assert_eq!(box_segments('╊'), Some((1, 2, 2, 2)));
        assert_eq!(box_segments('╋'), Some((2, 2, 2, 2)));
        assert_eq!(box_segments('┄'), None);
        assert_eq!(box_segments('═'), None);
    }
}
