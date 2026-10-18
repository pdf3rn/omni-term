use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, EventFilter, FontFamily, FontId, Key, Pos2, Rect, Sense,
    Stroke, StrokeKind, Vec2,
};

use crate::keys;
use crate::term::Session;
use crate::theme;

pub const PADDING: f32 = 12.0;
pub const DEFAULT_FONT_SIZE: f32 = 15.0;
pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 32.0;

pub fn bold_family() -> FontFamily {
    FontFamily::Name("mono-bold".into())
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

/// Vista de un terminal: pinta la rejilla de una sesión y le envía la entrada.
pub struct Pane {
    pub session: Session,
    debug_keys: bool,
    input_enabled: bool,
}

impl Pane {
    pub fn new(session: Session, debug_keys: bool) -> Self {
        Self { session, debug_keys, input_enabled: true }
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
        let ppp = ui.ctx().pixels_per_point();
        let (cell_w, cell_h) = cell_size(ui, font_size);
        let area = Rect::from_min_size(snap(area.min, ppp), area.size());

        let cols = (area.width() / cell_w).floor().max(1.0) as usize;
        let rows = (area.height() / cell_h).floor().max(1.0) as usize;
        self.session.resize(cols, rows);

        let id = ui.scope_id().with("terminal");
        let response = ui.interact(area, id, Sense::click_and_drag());
        if self.input_enabled {
            ui.memory_mut(|m| m.request_focus(id));
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            );
        });
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);

        let to_point = |pos: Pos2, display_offset: usize| {
            let rel = pos - area.min;
            let col = (rel.x / cell_w).floor().clamp(0.0, cols as f32 - 1.0);
            let row = (rel.y / cell_h).floor().clamp(0.0, rows as f32 - 1.0);
            let side = if (rel.x / cell_w).fract() < 0.5 { Side::Left } else { Side::Right };
            let point = viewport_to_point(display_offset, Point::new(row as usize, Column(col as usize)));
            (point, side)
        };

        {
            let mut term = self.session.term.lock().unwrap();
            let offset = term.grid().display_offset();
            if let Some(pos) = response.interact_pointer_pos() {
                if response.drag_started_by(egui::PointerButton::Primary) {
                    let (point, side) = to_point(pos, offset);
                    term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                } else if response.dragged_by(egui::PointerButton::Primary) {
                    let (point, side) = to_point(pos, offset);
                    if let Some(selection) = term.selection.as_mut() {
                        selection.update(point, side);
                    }
                }
            }
            if response.clicked() {
                term.selection = None;
            }
        }

        if self.input_enabled {
            self.handle_scroll(ui, cell_h);
            self.handle_input(ui);
        }
        self.paint_grid(ui, area, &font, &bold, cell_w, cell_h);
        self.scrollbar(ui, area, rows);
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
        let response = ui.interact(hit, ui.scope_id().with("scrollbar"), Sense::click_and_drag());
        if let Some(pos) = response.interact_pointer_pos() {
            if response.dragged() || response.is_pointer_button_down_on() {
                let ratio = ((pos.y - track.min.y - thumb_len / 2.0) / travel).clamp(0.0, 1.0);
                let target = ((1.0 - ratio) * history as f32).round() as i32;
                self.session.term.lock().unwrap().scroll_display(Scroll::Delta(target - offset as i32));
            }
        }
        let active = response.hovered() || response.dragged();
        let color = if active { theme::SUBTEXT } else { theme::SELECTION };
        let thumb = Rect::from_min_size(Pos2::new(track.min.x, thumb_top), Vec2::new(track.width(), thumb_len));
        ui.painter().rect_filled(thumb, CornerRadius::same(3), color);
    }

    fn handle_scroll(&mut self, ui: &egui::Ui, cell_h: f32) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        let lines = (dy / cell_h).round() as i32;
        if lines == 0 {
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

        for event in &events {
            match event {
                Event::Key { key, pressed, modifiers, repeat, .. } => {
                    let reserved = modifiers.ctrl && modifiers.shift && matches!(key, Key::C | Key::V);
                    if reserved {
                        if *pressed && *key == Key::C {
                            self.copy_selection(ui);
                        } else if *pressed && *key == Key::V {
                            if let Some(text) = clipboard_text() {
                                out.extend(paste_bytes(&text, mode));
                            }
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
                _ => {}
            }
        }

        if !out.is_empty() {
            let mut term = self.session.term.lock().unwrap();
            term.scroll_display(Scroll::Bottom);
            drop(term);
            self.session.write(&out);
        }
    }

    fn copy_selection(&self, ui: &egui::Ui) {
        let text = self.session.term.lock().unwrap().selection_to_string();
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            ui.ctx().copy_text(text);
        }
    }

    fn paint_grid(&self, ui: &egui::Ui, area: Rect, font: &FontId, bold: &FontId, cell_w: f32, cell_h: f32) {
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(area.expand(PADDING));
        let term = self.session.term.lock().unwrap();
        let content = term.renderable_content();
        let offset = content.display_offset;
        let cursor = content.cursor;
        let cursor_visible = offset == 0 && cursor.shape != CursorShape::Hidden;

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

            let (mut fg, mut bg) = (theme::resolve(cell.fg), theme::resolve(cell.bg));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
            let at_cursor = cursor_visible && indexed.point == cursor.point;

            let fill = if at_cursor {
                fg = theme::BASE;
                Some(theme::CURSOR)
            } else if selected {
                Some(theme::SELECTION)
            } else if bg != theme::BASE {
                Some(bg)
            } else {
                None
            };
            let shown_bg = fill.unwrap_or(theme::BASE);

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
                let face = if g.flags.contains(Flags::BOLD) { bold } else { font };
                painter.text(rect.min, Align2::LEFT_TOP, g.c, face.clone(), g.fg);
            }
            if g.flags.intersects(Flags::ALL_UNDERLINES) {
                let y = rect.max.y - 1.0 / ppp;
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y + 1.0 / ppp)),
                    CornerRadius::ZERO,
                    g.fg,
                );
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

        if cursor_visible && matches!(cursor.shape, CursorShape::HollowBlock) {
            if let Some(vp) = point_to_viewport(offset, cursor.point) {
                painter.rect_stroke(
                    cell_rect(vp.line, vp.column.0, 1),
                    CornerRadius::ZERO,
                    Stroke::new(1.0, theme::CURSOR),
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

fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok().filter(|t| !t.is_empty())
}

fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        [b"\x1b[200~", text.replace('\x1b', "").as_bytes(), b"\x1b[201~"].concat()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::box_segments;

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
