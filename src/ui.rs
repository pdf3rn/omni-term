use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use eframe::egui::{
    self, Align2, CornerRadius, Event, EventFilter, FontData, FontDefinitions, FontFamily,
    FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, ViewportCommand,
};

use crate::term::Session;
use crate::theme;

const PADDING: f32 = 12.0;
const FONT_SIZE: f32 = 15.0;

fn bold_family() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

pub struct App {
    session: Session,
    window_title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        install_fonts(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let session = Session::spawn(80, 24, move || ctx.request_repaint())?;
        Ok(Self { session, window_title: String::new() })
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.session.has_exited() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        let title = self.session.title.lock().unwrap().clone();
        let title = if title.is_empty() { "omniterm".to_string() } else { title };
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }

        let full = ui.max_rect();
        ui.painter().rect_filled(full, CornerRadius::ZERO, theme::BASE);
        self.terminal(ui, full.shrink(PADDING));
    }
}

impl App {
    fn terminal(&mut self, ui: &mut egui::Ui, area: Rect) {
        let font = FontId::new(FONT_SIZE, FontFamily::Monospace);
        let bold = FontId::new(FONT_SIZE, bold_family());
        let (cell_w, cell_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));

        let cols = (area.width() / cell_w).floor().max(1.0) as usize;
        let rows = (area.height() / cell_h).floor().max(1.0) as usize;
        self.session.resize(cols, rows);

        let id = ui.scope_id().with("terminal");
        let response = ui.interact(area, id, Sense::click_and_drag());
        ui.memory_mut(|m| {
            m.request_focus(id);
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

        self.handle_scroll(ui, cell_h);
        self.handle_input(ui);
        self.paint_grid(ui, area, &font, &bold, cell_w, cell_h);
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
        let mut out: Vec<u8> = Vec::new();
        let mut alt_held = false;

        for event in &events {
            match event {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    alt_held |= modifiers.alt;
                    if modifiers.ctrl && modifiers.shift && *key == Key::C {
                        self.copy_selection(ui);
                    } else if modifiers.ctrl && modifiers.shift && *key == Key::V {
                        if let Some(text) = clipboard_text() {
                            out.extend(paste_bytes(&text, mode));
                        }
                    } else if let Some(bytes) = key_bytes(*key, *modifiers, mode) {
                        out.extend(bytes);
                    }
                }
                Event::Text(text) if !alt_held && !ui.input(|i| i.modifiers.ctrl) => {
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
        let painter = ui.painter_at(area.expand(PADDING));
        let term = self.session.term.lock().unwrap();
        let content = term.renderable_content();
        let offset = content.display_offset;
        let cursor = content.cursor;
        let cursor_visible = offset == 0 && cursor.shape != CursorShape::Hidden;

        for indexed in content.display_iter {
            let Some(vp) = point_to_viewport(offset, indexed.point) else { continue };
            let cell = &indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let origin = area.min + Vec2::new(vp.column.0 as f32 * cell_w, vp.line as f32 * cell_h);
            let width = if cell.flags.contains(Flags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };
            let rect = Rect::from_min_size(origin, Vec2::new(width, cell_h));

            let (mut fg, mut bg) = (theme::resolve(cell.fg), theme::resolve(cell.bg));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
            let at_cursor = cursor_visible && indexed.point == cursor.point;

            let paints_bg = bg != theme::BASE || cell.flags.contains(Flags::INVERSE);
            if at_cursor {
                painter.rect_filled(rect, CornerRadius::ZERO, theme::CURSOR);
                fg = theme::BASE;
            } else if selected {
                painter.rect_filled(rect, CornerRadius::ZERO, theme::SELECTION);
            } else if paints_bg {
                painter.rect_filled(rect, CornerRadius::ZERO, bg);
            }

            if cell.c != ' ' && cell.c != '\0' && !cell.flags.contains(Flags::HIDDEN) {
                let face = if cell.flags.contains(Flags::BOLD) { bold } else { font };
                painter.text(origin, Align2::LEFT_TOP, cell.c, face.clone(), fg);
            }
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                let y = rect.max.y - 2.0;
                painter.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], Stroke::new(1.0, fg));
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                let y = rect.center().y;
                painter.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], Stroke::new(1.0, fg));
            }
        }

        if cursor_visible && matches!(cursor.shape, CursorShape::HollowBlock) {
            if let Some(vp) = point_to_viewport(offset, cursor.point) {
                let origin = area.min + Vec2::new(vp.column.0 as f32 * cell_w, vp.line as f32 * cell_h);
                painter.rect_stroke(
                    Rect::from_min_size(origin, Vec2::new(cell_w, cell_h)),
                    CornerRadius::ZERO,
                    Stroke::new(1.0, theme::CURSOR),
                    StrokeKind::Inside,
                );
            }
        }
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

fn key_bytes(key: Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let csi_or_ss3 = |final_byte: char| {
        if app_cursor { format!("\x1bO{final_byte}") } else { format!("\x1b[{final_byte}") }
    };

    let sequence = match key {
        Key::Enter => "\r".to_string(),
        Key::Backspace => "\x7f".to_string(),
        Key::Tab if mods.shift => "\x1b[Z".to_string(),
        Key::Tab => "\t".to_string(),
        Key::Escape => "\x1b".to_string(),
        Key::ArrowUp => csi_or_ss3('A'),
        Key::ArrowDown => csi_or_ss3('B'),
        Key::ArrowRight => csi_or_ss3('C'),
        Key::ArrowLeft => csi_or_ss3('D'),
        Key::Home => csi_or_ss3('H'),
        Key::End => csi_or_ss3('F'),
        Key::PageUp => "\x1b[5~".to_string(),
        Key::PageDown => "\x1b[6~".to_string(),
        Key::Delete => "\x1b[3~".to_string(),
        Key::Insert => "\x1b[2~".to_string(),
        Key::F1 => "\x1bOP".to_string(),
        Key::F2 => "\x1bOQ".to_string(),
        Key::F3 => "\x1bOR".to_string(),
        Key::F4 => "\x1bOS".to_string(),
        Key::F5 => "\x1b[15~".to_string(),
        Key::F6 => "\x1b[17~".to_string(),
        Key::F7 => "\x1b[18~".to_string(),
        Key::F8 => "\x1b[19~".to_string(),
        Key::F9 => "\x1b[20~".to_string(),
        Key::F10 => "\x1b[21~".to_string(),
        Key::F11 => "\x1b[23~".to_string(),
        Key::F12 => "\x1b[24~".to_string(),
        Key::Space if mods.ctrl => "\0".to_string(),
        _ => {
            let name = key.name();
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            if !c.is_ascii_alphanumeric() {
                return None;
            }
            if mods.ctrl && c.is_ascii_alphabetic() {
                let byte = (c.to_ascii_lowercase() as u8 - b'a') + 1;
                let prefix = if mods.alt { "\x1b" } else { "" };
                return Some([prefix.as_bytes(), &[byte]].concat());
            }
            if mods.alt && !mods.ctrl {
                let c = if mods.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() };
                return Some(format!("\x1b{c}").into_bytes());
            }
            return None;
        }
    };
    Some(sequence.into_bytes())
}
