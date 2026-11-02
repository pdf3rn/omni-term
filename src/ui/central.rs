//! Zona central: lo que muestra la pestaña activa (terminal, explorador, contenedores, ajustes) o la pantalla de inicio.

use super::*;

impl App {
    pub(super) fn central_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
            let mut pasted: Option<(u64, String)> = None;
            let mut close_pane: Option<u64> = None;
            if let Body::Settings(section) = self.tabs[active].body {
                let section = self.settings_page(ui, area.shrink2(Vec2::new(24.0, 16.0)), section);
                self.tabs[active].body = Body::Settings(section);
            }
            match &mut self.tabs[active].body {
                Body::Settings(_) => {}
                Body::Terminal { panes, focused, zoomed, .. } => {
                    if input_enabled && panes.len() > 1 {
                        pane_keys(ctx, panes, focused, zoomed);
                    }
                    if zoomed.is_some_and(|id| !panes.contains(id)) {
                        *zoomed = None;
                    }
                    let (leaves, handles) = match *zoomed {
                        Some(id) => (vec![(id, text_area)], Vec::new()),
                        None => panes.layout(text_area, if panes.len() > 1 { SPLIT_GAP } else { 0.0 }),
                    };

                    let mut ratios = Vec::new();
                    for (index, handle) in handles.iter().enumerate() {
                        let id = ui.scope_id().with(("split-handle", active, index));
                        let response = ui.interact(handle.rect, id, Sense::click_and_drag());
                        if response.double_clicked() {
                            ratios.push((handle.path.clone(), 0.5));
                        }
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
                    // Encima de los paneles: distintivo de ampliado y × para cerrar el panel bajo el puntero.
                    if panes.len() > 1 {
                        let pointer = ctx.pointer_hover_pos();
                        for (id, rect) in &leaves {
                            let search_open = panes.get(*id).is_some_and(Pane::search_open);
                            let corner = Rect::from_min_size(Pos2::new(rect.max.x - PANE_BUTTON, rect.min.y), Vec2::splat(PANE_BUTTON));
                            if zoomed.is_some() && !search_open {
                                let galley = ui.painter().layout_no_wrap("Ampliado".to_string(), FontId::proportional(theme::size::CAPTION), theme::subtext());
                                let badge = Rect::from_min_size(Pos2::new(corner.min.x - galley.size().x - theme::space::L * 2.0, rect.min.y + theme::space::XS), galley.size() + Vec2::new(theme::space::L, theme::space::XS) * 2.0);
                                ui.painter().rect_filled(badge, CornerRadius::same(theme::radius::SMALL), theme::surface());
                                ui.painter().galley(badge.min + Vec2::new(theme::space::L, theme::space::XS), galley, theme::subtext());
                            }
                            if !search_open && input_enabled && pointer.is_some_and(|p| rect.contains(p)) {
                                let close = crate::ui::kit::IconButton::new(icons::X).color(theme::subtext()).show_at(ui, corner, ("pane-close", active, *id));
                                if close.on_hover_text("Cerrar panel  (Ctrl+Shift+W)").clicked() {
                                    close_pane = Some(*id);
                                }
                            }
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
                        pasted = pane.take_paste().map(|text| (pane.id(), text));
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
            if let Some((pane, text)) = pasted {
                let tab = self.tabs[active].id;
                if !self.settings.confirm_paste {
                    self.write_paste(tab, pane, &text);
                } else if self.dialog.is_none() {
                    self.dialog = Some(Dialog::ConfirmPaste(dialogs::PasteConfirm { tab, pane, text, dont_ask: false }));
                }
            }
            if let Some(text) = snippet_text {
                self.dialog = Some(Dialog::Snippet(SnippetForm::new(text)));
            }
            match request {
                Some(PaneRequest::SplitRight) => self.split_active(Dir::Horizontal),
                Some(PaneRequest::SplitDown) => self.split_active(Dir::Vertical),
                Some(PaneRequest::Close) => self.request_close_focused_pane(),
                None => {}
            }
            if let Some(pane) = close_pane {
                let tab = self.tabs[active].id;
                self.request_close_pane(tab, pane);
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
    }
}

/// Lado del botón de cerrar de cada panel.
const PANE_BUTTON: f32 = 30.0;
/// Cuánto mueve el borde cada pulsación de Alt+Mayús+flecha.
const NUDGE_STEP: f32 = 0.05;

/// Atajos de paneles, reservados solo con la pestaña dividida (con un panel llegan a la aplicación):
/// Alt+flechas cambia de panel, Alt+Mayús+flechas mueve su borde y Ctrl+Mayús+Z amplía o restaura.
fn pane_keys(ctx: &egui::Context, panes: &mut Tree<Pane>, focused: &mut u64, zoomed: &mut Option<u64>) {
    const ARROWS: [(Key, Side); 4] = [(Key::ArrowLeft, Side::Left), (Key::ArrowRight, Side::Right), (Key::ArrowUp, Side::Up), (Key::ArrowDown, Side::Down)];
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z) {
            *zoomed = if zoomed.is_some() { None } else { Some(*focused) };
        }
        for (key, side) in ARROWS {
            // Primero con Mayús: `consume_key` ignora un Mayús de más, y Alt+flecha se lo llevaría.
            let nudges = i.count_and_consume_key(Modifiers::ALT | Modifiers::SHIFT, key);
            if nudges > 0 && zoomed.is_none() {
                for _ in 0..nudges {
                    panes.nudge(*focused, side, NUDGE_STEP);
                }
            }
            if i.consume_key(Modifiers::ALT, key) {
                *zoomed = None;
                if let Some(next) = panes.neighbor(*focused, side) {
                    *focused = next;
                }
            }
        }
    });
}
