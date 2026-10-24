//! La ventana: barra de título integrada, bordes, tamaño y posición.

use super::*;

impl App {
    /// Recuerda tamaño, posición y si estaba maximizada, para la próxima vez. El tamaño y la posición
    /// guardados son los de la ventana normal, no los de maximizada ni pantalla completa.
    pub(super) fn remember_window_size(&mut self, ctx: &egui::Context) {
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
    pub(super) fn restore_window_position(&mut self, ctx: &egui::Context) {
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

    /// Bordes redimensionables y marco fino de la ventana sin decoraciones.
    pub(super) fn window_frame(&self, ctx: &egui::Context) {
        if self.settings.system_titlebar || ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::NULL.with("window-border")));
        painter.rect_stroke(rect, 0.0, Stroke::new(theme::stroke::HAIRLINE, theme::surface()), egui::StrokeKind::Inside);
        let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
        use egui::viewport::ResizeDirection as D;
        let (left, right) = (p.x - rect.min.x < RESIZE_BORDER, rect.max.x - p.x < RESIZE_BORDER);
        let (top, bottom) = (p.y - rect.min.y < RESIZE_BORDER, rect.max.y - p.y < RESIZE_BORDER);
        let (direction, cursor) = match (left, right, top, bottom) {
            (true, _, true, _) => (D::NorthWest, egui::CursorIcon::ResizeNwSe),
            (_, true, true, _) => (D::NorthEast, egui::CursorIcon::ResizeNeSw),
            (true, _, _, true) => (D::SouthWest, egui::CursorIcon::ResizeNeSw),
            (_, true, _, true) => (D::SouthEast, egui::CursorIcon::ResizeNwSe),
            (true, ..) => (D::West, egui::CursorIcon::ResizeHorizontal),
            (_, true, ..) => (D::East, egui::CursorIcon::ResizeHorizontal),
            (_, _, true, _) => (D::North, egui::CursorIcon::ResizeVertical),
            (_, _, _, true) => (D::South, egui::CursorIcon::ResizeVertical),
            _ => return,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }

    /// Alt suelto (pulsado y soltado sin otra tecla) muestra u oculta la barra de menú oculta.
    pub(super) fn track_alt(&mut self, ctx: &egui::Context) {
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

    /// Barra de menú superior. Devuelve el comando elegido o la conexión reciente que se pidió abrir.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) -> (Option<Command>, Option<Connection>) {
        let custom = !self.settings.system_titlebar;
        if !custom && !self.settings.menu_bar && !self.menu_peek {
            return (None, None);
        }
        let (menu_in_bar, bar_height) = (self.settings.titlebar_menu, if self.settings.titlebar_compact { 30.0 } else { 36.0 });
        let title = if self.window_title.is_empty() { "omni-term".to_string() } else { self.window_title.clone() };
        let ctx = self.ctx.clone();
        let mut open_settings = false;
        let tab = self.tabs.get(self.active);
        let pane = tab.and_then(|t| t.pane());
        let has_terminal = pane.is_some();
        let has_selection = pane.is_some_and(|p| p.selection_text().is_some());
        let on_connection = matches!(tab.map(|t| t.kind), Some(TabKind::Connection(_)));
        let several = self.tabs.len() > 1;
        let recents = self.recent_connections();
        let mut chosen: Option<Command> = None;
        let mut connect: Option<Connection> = None;
        let (panel_height, margin) = if custom { (bar_height, Margin { left: 10, right: 0, top: 0, bottom: 0 }) } else { (30.0, Margin::symmetric(8, 3)) };
        Panel::top("menubar")
            .exact_size(panel_height)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(theme::mantle()).inner_margin(margin))
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
                let mut menus = |ui: &mut egui::Ui| {
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
                };
                if !custom {
                    egui::MenuBar::new().ui(ui, |ui| menus(ui));
                } else {
                    let bar = ui.max_rect();
                    // Se registra antes que el resto: los menús y botones, añadidos después, tienen prioridad sobre la zona de arrastre.
                    let response = ui.interact(bar, egui::Id::NULL.with("titlebar-drag"), Sense::click_and_drag());
                    ui.horizontal_centered(|ui| {
                        ui.add(egui::Image::new(&crate::ui::dialogs::app_logo(ui.ctx())).fit_to_exact_size(Vec2::splat(16.0)));
                        ui.add_space(theme::space::XS);
                        egui::MenuBar::new().ui(ui, |ui| {
                            ui.spacing_mut().button_padding.x = theme::space::M;
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.style_mut().override_font_id = Some(FontId::new(theme::size::SMALL, FontFamily::Proportional));
                            ui.visuals_mut().widgets.inactive.fg_stroke.color = theme::subtext();
                            if menu_in_bar {
                                menus(ui);
                            } else {
                                ui.menu_button(icons::DOTS, |ui| menus(ui));
                            }
                        });
                    });
                    let controls = 3.0 * WINDOW_BUTTON_WIDTH;
                    let title_area = Rect::from_min_max(Pos2::new(bar.min.x + 300.0, bar.min.y), Pos2::new(bar.max.x - controls - 36.0, bar.max.y));
                    ui.painter().with_clip_rect(title_area).text(
                        Pos2::new(bar.center().x, bar.center().y),
                        egui::Align2::CENTER_CENTER,
                        &title,
                        FontId::new(theme::size::LABEL, FontFamily::Proportional),
                        theme::subtext(),
                    );
                    let gear_slot = Rect::from_center_size(Pos2::new(bar.max.x - controls - 18.0, bar.center().y), Vec2::splat(1.0));
                    if crate::ui::kit::IconButton::new(icons::GEAR).size(theme::size::ICON).show_at(ui, gear_slot, "titlebar-settings").on_hover_text("Ajustes (Ctrl+,)").clicked() {
                        open_settings = true;
                    }
                    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    if response.double_clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                    } else if response.drag_started_by(egui::PointerButton::Primary) {
                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    }
                    for (index, kind) in [WindowButton::Minimize, if maximized { WindowButton::Restore } else { WindowButton::Maximize }, WindowButton::Close].into_iter().enumerate() {
                        let rect = Rect::from_min_size(Pos2::new(bar.max.x - controls + index as f32 * WINDOW_BUTTON_WIDTH, bar.min.y), Vec2::new(WINDOW_BUTTON_WIDTH, bar.height()));
                        if window_button(ui, rect, kind).clicked() {
                            ctx.send_viewport_cmd(match kind {
                                WindowButton::Minimize => ViewportCommand::Minimized(true),
                                WindowButton::Maximize | WindowButton::Restore => ViewportCommand::Maximized(!maximized),
                                WindowButton::Close => ViewportCommand::Close,
                            });
                        }
                    }
                }
            });
        if open_settings {
            chosen = Some(Command::Settings);
        }
        (chosen, connect)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Botón de ventana de la barra integrada, dibujado con trazos para que no dependa de ninguna fuente.
pub(super) fn window_button(ui: &mut egui::Ui, rect: Rect, kind: WindowButton) -> egui::Response {
    let id = egui::Id::NULL.with(("window-button", kind as u8));
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let closing = kind == WindowButton::Close;
    if response.hovered() {
        let fill = if closing { theme::red() } else { theme::selection() };
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    let ink = if closing && response.hovered() { Color32::WHITE } else { theme::text() };
    let stroke = Stroke::new(theme::stroke::HAIRLINE, ink);
    let c = rect.center();
    let h = 5.0;
    let painter = ui.painter();
    match kind {
        WindowButton::Minimize => {
            painter.line_segment([Pos2::new(c.x - h, c.y), Pos2::new(c.x + h, c.y)], stroke);
        }
        WindowButton::Maximize => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * h)), 1.0, stroke, egui::StrokeKind::Inside);
        }
        WindowButton::Restore => {
            let front = Rect::from_center_size(Pos2::new(c.x - 1.0, c.y + 1.0), Vec2::splat(2.0 * h - 2.0));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
            painter.line_segment([Pos2::new(front.min.x + 2.0, front.min.y - 2.0), Pos2::new(front.max.x + 2.0, front.min.y - 2.0)], stroke);
            painter.line_segment([Pos2::new(front.max.x + 2.0, front.min.y - 2.0), Pos2::new(front.max.x + 2.0, front.max.y - 2.0)], stroke);
        }
        WindowButton::Close => {
            painter.line_segment([Pos2::new(c.x - h, c.y - h), Pos2::new(c.x + h, c.y + h)], stroke);
            painter.line_segment([Pos2::new(c.x - h, c.y + h), Pos2::new(c.x + h, c.y - h)], stroke);
        }
    }
    response
}

/// `x,y` guardado, si deja a la vista al menos 50 px de la ventana dentro del monitor.
pub(super) fn parse_window_position(text: &str, monitor: Vec2, size: Vec2) -> Option<Pos2> {
    let (x, y) = text.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    let margin = 50.0;
    let visible = x + size.x > margin && x < monitor.x - margin && y >= 0.0 && y < monitor.y - margin;
    visible.then(|| Pos2::new(x, y))
}

pub(super) const WINDOW_BUTTON_WIDTH: f32 = 46.0;

/// Grosor de los bordes de la ventana sin decoraciones donde se puede redimensionar.
pub(super) const RESIZE_BORDER: f32 = 5.0;
