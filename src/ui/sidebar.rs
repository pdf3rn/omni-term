//! Barra lateral: conexiones, snippets, grupos y navegación.

use super::*;

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let system_bar = self.settings.system_titlebar;
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
                        FontId::proportional(theme::size::ICON),
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
                    ui.add_space(theme::space::M);

                    ui.add_space(theme::space::L);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().button_padding.x = 0.0;
                        ui.spacing_mut().item_spacing.x = 18.0;
                        for (view, label) in [(SidebarView::Connections, "Conexiones"), (SidebarView::Snippets, "Snippets")] {
                            let selected = self.sidebar_view == view;
                            let color = if selected { theme::text() } else { theme::subtext() };
                            let font = FontId::new(theme::size::LABEL, if selected { strong_family() } else { FontFamily::Proportional });
                            let button = Button::new(RichText::new(label).font(font).color(color));
                            let response = crate::ui::kit::button_with(ui, button, crate::ui::kit::Variant::Ghost);
                            if selected {
                                let y = response.rect.max.y + 1.0;
                                ui.painter().line_segment(
                                    [Pos2::new(response.rect.min.x, y), Pos2::new(response.rect.max.x, y)],
                                    Stroke::new(theme::stroke::THICK, theme::accent()),
                                );
                            }
                            if response.clicked() && self.sidebar_view != view {
                                self.sidebar_view = view;
                                let value: &[u8] = if view == SidebarView::Snippets { b"snippets" } else { b"connections" };
                                let _ = self.store.lock().unwrap().set_meta("sidebar_view", value);
                            }
                        }
                    });
                    ui.add_space(theme::space::M);

                    if connections_view && self.tabs.len() >= 3 {
                        ui.label(RichText::new("Abiertas").small().color(theme::subtext()));
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for (index, tab) in self.tabs.iter().enumerate().take(6) {
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                            let response = response.hand();
                            let active = index == self.active;
                            if active {
                                ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), theme::selection());
                            } else if response.hovered() {
                                ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), theme::surface());
                            }
                            let glyph = match (&tab.body, tab.kind) {
                                (Body::Explorer(_), _) => icons::FOLDER,
                                (Body::Settings(_), _) => icons::GEAR,
                                (_, TabKind::Local) => icons::TERMINAL,
                                _ => icons::ARROW_RIGHT,
                            };
                            ui.painter().text(Pos2::new(rect.min.x + 8.0, rect.center().y), Align2::CENTER_CENTER, glyph, FontId::proportional(theme::size::ICON_SM), theme::subtext());
                            let font = FontId::new(theme::size::LABEL, if active { strong_family() } else { FontFamily::Proportional });
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
                        ui.add_space(theme::space::M);
                        ui.spacing_mut().item_spacing.y = 8.0;
                    }
                    let list_empty = if connections_view { self.connections.is_empty() } else { self.snippets.is_empty() };
                    if !list_empty {
                    ui.horizontal(|ui| {
                        let title = if connections_view { "Guardadas" } else { "Guardados" };
                        ui.label(RichText::new(title).small().color(theme::subtext()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        if connections_view {
                                if crate::ui::kit::icon_button(ui, icons::PLUS, theme::size::ICON, theme::text()).on_hover_text("Nueva conexión").clicked() {
                                    new_connection = true;
                                }
                            } else if crate::ui::kit::icon_button(ui, icons::PLUS, theme::size::ICON, theme::text()).on_hover_text("Nuevo snippet").clicked() {
                                new_snippet = true;
                            }
                        });
                    });
                    ui.add_space(-theme::space::XXS);
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
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), Stroke::new(theme::stroke::MEDIUM, theme::accent()), egui::StrokeKind::Inside);
                                }
                                if let Some(payload) = response.dnd_release_payload::<i64>() {
                                    drop_move = Some((*payload, label.clone()));
                                }
                                if self.sidebar_nav && row_pos == self.nav_index {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), Stroke::new(theme::stroke::MEDIUM, theme::text()), egui::StrokeKind::Inside);
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
                                    FontId::proportional(theme::size::SMALL),
                                    color,
                                );
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 18.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    format!("{label}  {}", indices.len()),
                                    FontId::new(theme::size::SMALL, strong_family()),
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
                                        let galley = ghost.layout_no_wrap(connection.name.clone(), FontId::new(theme::size::LABEL, strong_family()), theme::text());
                                        let box_rect = Rect::from_min_size(pointer + Vec2::new(14.0, 8.0), galley.size() + Vec2::new(16.0, 10.0));
                                        ghost.rect_filled(box_rect, CornerRadius::same(theme::radius::CONTROL), theme::selection());
                                        ghost.galley(box_rect.min + Vec2::new(8.0, 5.0), galley, theme::text());
                                    }
                                }
                                if response.dnd_hover_payload::<i64>().is_some_and(|p| *p != connection_id) {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), Stroke::new(theme::stroke::MEDIUM, theme::accent()), egui::StrokeKind::Inside);
                                }
                                if let Some(payload) = response.dnd_release_payload::<i64>() {
                                    if *payload != connection_id {
                                        drop_move = Some((*payload, label.clone()));
                                    }
                                }
                                if self.sidebar_nav && this_pos == self.nav_index {
                                    ui.painter().rect_stroke(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), Stroke::new(theme::stroke::MEDIUM, theme::text()), egui::StrokeKind::Inside);
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
                                    ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(theme::radius::CONTROL), theme::surface());
                                }
                                if let Some(color) = identity {
                                    // Marca de identidad: informa de qué servidor es, no decora.
                                    let bar = Rect::from_min_size(Pos2::new(rect.min.x - 8.0, rect.min.y + 7.0), Vec2::new(3.0, theme::row(ROW_HEIGHT) - 14.0));
                                    ui.painter().rect_filled(bar, CornerRadius::same(theme::radius::TINY), color);
                                }
                                // Acciones al pasar el ratón: sustituyen a la hora, que vuelve a verse sin ratón encima.
                                let more_slot = Rect::from_center_size(Pos2::new(rect.max.x - 12.0, rect.center().y), Vec2::splat(26.0));
                                let edit_slot = Rect::from_center_size(Pos2::new(rect.max.x - 38.0, rect.center().y), Vec2::splat(26.0));
                                // El menú abierto mantiene visibles los botones aunque el ratón se haya ido de la fila.
                                let menu_state = response.id.with("menu-open");
                                let menu_open_before = ui.data(|d| d.get_temp::<bool>(menu_state)).unwrap_or(false);
                                let actions = over || menu_open_before;
                                let mut buttons = None;
                                if actions {
                                    let pencil = crate::ui::kit::IconButton::new(icons::PENCIL).show_at(ui, edit_slot, (response.id, "edit")).on_hover_text("Editar");
                                    let more = crate::ui::kit::IconButton::new(icons::DOTS).size(theme::size::ICON_XL).active(menu_open_before).show_at(ui, more_slot, (response.id, "more")).on_hover_text("Más acciones");
                                    let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more));
                                    ui.data_mut(|d| d.insert_temp(menu_state, menu_open));
                                    buttons = Some((pencil, more));
                                } else {
                                    ui.data_mut(|d| d.insert_temp(menu_state, false));
                                }

                                // Icono de tipo con la insignia del estado de la sesión en su esquina.
                                let icon_center = Pos2::new(rect.min.x + 9.0, rect.center().y);
                                let kind_icon = crate::ui::kit::connection_glyph(connection);
                                ui.painter().text(icon_center, Align2::CENTER_CENTER, kind_icon, FontId::proportional(theme::size::ICON_LG), theme::subtext());
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
                                let ago_font = FontId::new(theme::size::CAPTION, FontFamily::Proportional);
                                let ago_width = if ago.is_empty() || actions { 0.0 } else { ui.painter().layout_no_wrap(ago.clone(), ago_font.clone(), Color32::PLACEHOLDER).size().x + 10.0 };
                                let right_limit = if actions { rect.max.x - 56.0 } else { rect.max.x - 4.0 };
                                let first_line = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(right_limit - ago_width, rect.max.y)));
                                let name_font = FontId::new(theme::size::BODY, strong_family());
                                let pill_font = FontId::new(theme::size::CAPTION, strong_family());
                                let env = connection.env.as_ref().filter(|_| !headed || searching);
                                let pill_size = env.map(|env| first_line.layout_no_wrap(env.clone(), pill_font.clone(), Color32::PLACEHOLDER).size() + Vec2::new(10.0, 3.0));
                                let lock_width = if connection.insecure_protocol().is_some() { 18.0 } else { 0.0 };
                                // El nombre cede sitio al candado y a la pastilla, que siempre se ven.
                                let room = right_limit - ago_width - text_x - lock_width - pill_size.map_or(0.0, |s| s.x + 8.0);
                                let name = crate::ui::kit::fit_text(&first_line, &connection.name, &name_font, room);
                                let name_width = first_line.layout_no_wrap(name.clone(), name_font.clone(), Color32::PLACEHOLDER).size().x;
                                first_line.text(Pos2::new(text_x, rect.min.y + 9.0), Align2::LEFT_TOP, name, name_font, theme::text());
                                let mut after_name = text_x + name_width + 8.0;
                                if lock_width > 0.0 {
                                    first_line.text(Pos2::new(after_name + 5.0, rect.min.y + 19.0), Align2::CENTER_CENTER, icons::LOCK_OPEN, FontId::proportional(theme::size::LABEL), theme::red());
                                    after_name += lock_width;
                                }
                                if let (Some(env), Some(size)) = (env, pill_size) {
                                    // Pastilla con el color de identidad: dice en qué entorno estás.
                                    let tint = identity.unwrap_or_else(theme::subtext);
                                    let pill = Rect::from_min_size(Pos2::new(after_name, rect.min.y + 11.0), size);
                                    first_line.rect_filled(pill, CornerRadius::same(theme::radius::CARD), tint.gamma_multiply(0.2));
                                    first_line.text(pill.center(), Align2::CENTER_CENTER, env, pill_font, tint);
                                }
                                if ago_width > 0.0 {
                                    ui.painter().text(Pos2::new(rect.max.x - 4.0, rect.min.y + 11.0), Align2::RIGHT_TOP, &ago, ago_font, theme::subtext());
                                }
                                let second_line = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(right_limit, rect.max.y)));
                                let detail_font = FontId::new(theme::size::SMALL, FontFamily::Proportional);
                                let detail = crate::ui::kit::fit_text(&second_line, &format!("{}  ·  {}", connection.kind_label(), connection.destination()), &detail_font, right_limit - text_x);
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

                                if let Some((pencil, _)) = &buttons {
                                    if pencil.clicked() {
                                        row_action = Some((RowAction::Edit, connection.clone()));
                                    }
                                }
                                if let Some((_, more)) = &buttons {
                                    egui::Popup::menu(more).show(|ui| connection_menu(ui, connection, is_pinned, &mut row_action));
                                }

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
                                ui.add_space(theme::space::XS);
                            }
                        }
                        });
                    } else {
                        if !self.snippets.is_empty() {
                            if crate::ui::kit::button_with(ui, Button::new(RichText::new("Guardar selección").small().color(theme::subtext())), crate::ui::kit::Variant::Secondary).clicked() {
                                snippet_from_selection = true;
                            }
                            ui.add_space(theme::space::XS);
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
                                    FontId::proportional(theme::size::SMALL),
                                    color,
                                );
                                ui.painter().text(
                                    Pos2::new(rect.min.x + 18.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    format!("{label}  {}", members.len()),
                                    FontId::new(theme::size::SMALL, strong_family()),
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
                                    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), theme::surface());
                                }
                                let left = rect.min.x + 10.0;
                                let painter = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(4.0, 0.0)).with_max_x(rect.max.x - if over { 34.0 } else { 4.0 }));
                                painter.text(Pos2::new(left, rect.min.y + 9.0), Align2::LEFT_TOP, &snippet.title, FontId::new(theme::size::BODY, strong_family()), theme::text());
                                let preview = snippet.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                                let preview: String = preview.chars().take(80).collect();
                                painter.text(Pos2::new(left, rect.min.y + 27.0), Align2::LEFT_TOP, preview, FontId::new(theme::size::SMALL, FontFamily::Monospace), theme::subtext());

                                let run_slot = Rect::from_center_size(Pos2::new(rect.max.x - 18.0, rect.center().y), Vec2::splat(26.0));
                                let run_clicked = over
                                    && crate::ui::kit::IconButton::new(icons::PLAY).show_at(ui, run_slot, (response.id, "run")).on_hover_text("Insertar y ejecutar").clicked();
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
                        if system_bar && crate::ui::kit::button_with(ui, Button::new(format!("{}  Ajustes", icons::GEAR)), crate::ui::kit::Variant::Secondary).clicked() {
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

    /// Barra lateral reducida a iconos: terminal local, una insignia por conexión, buscar, nueva y ajustes.
    pub(super) fn sidebar_strip(&mut self, ui: &mut egui::Ui, connect: &mut Option<Connection>, open_settings: &mut bool, new_connection: &mut bool) {
        let system_bar = self.settings.system_titlebar;
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
                        ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), theme::surface());
                    }
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(theme::size::ICON_LG), theme::text());
                    response.on_hover_text(tip).clicked()
                };
                if icon(ui, icons::SEARCH, "Buscar (Ctrl+Shift+P)") {
                    palette = true;
                }
                ui.add_space(theme::space::XS);
                for connection in &self.connections {
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                    let response = response.hand();
                    let color = connection.color.map(theme::identity);
                    let fill = if response.hovered() { theme::selection() } else { theme::surface() };
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), fill);
                    if let Some(color) = color {
                        let bar = Rect::from_min_size(Pos2::new(rect.min.x - 6.0, rect.min.y + 7.0), Vec2::new(3.0, size.y - 14.0));
                        ui.painter().rect_filled(bar, CornerRadius::same(theme::radius::TINY), color);
                    }
                    let initial = connection.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, initial, FontId::new(theme::size::HEADING, strong_family()), theme::text());
                    let tip = format!("{}  ({})", connection.name, connection.target());
                    if response.on_hover_text(tip).clicked() {
                        *connect = Some(connection.clone());
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if system_bar && icon(ui, icons::GEAR, "Ajustes") {
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

    pub(super) fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        if self.sidebar_collapsed != collapsed {
            self.sidebar_collapsed = collapsed;
            let _ = self.store.lock().unwrap().set_meta("sidebar_collapsed", if collapsed { b"1" } else { b"0" });
        }
    }
}

pub(super) enum RowAction {
    Connect,
    Files,
    Edit,
    Duplicate,
    Pin,
    Delete,
}

/// Una fila navegable de la lista: encabezado de grupo (clave) o conexión (índice).
pub(super) enum NavRow {
    Header(String),
    Conn(usize),
}

pub(super) const PINNED_LABEL: &str = "Fijadas";

/// Entorno y color que corresponden a un grupo del sidebar; `None` en «Sin entorno».
pub(super) fn group_environment(label: &str, members: &[&Connection]) -> Option<(String, Option<u8>)> {
    match label.to_lowercase().as_str() {
        "sin entorno" => None,
        "producción" => Some(("producción".into(), Some(0))),
        "staging" => Some(("staging".into(), Some(1))),
        "desarrollo" => Some(("desarrollo".into(), Some(3))),
        "personal" => Some(("personal".into(), Some(4))),
        _ => members.first().map(|c| (c.env.clone().unwrap_or_else(|| label.to_string()), c.color)),
    }
}

pub(super) fn connection_menu(ui: &mut egui::Ui, connection: &Connection, pinned: bool, out: &mut Option<(RowAction, Connection)>) {
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
pub(super) fn sidebar_groups(connections: &[Connection], query: &str, pinned: &[i64]) -> Vec<(String, Vec<usize>)> {
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

#[cfg(test)]
mod sidebar_tests {
    use super::*;

    fn connection(id: i64, name: &str, env: Option<&str>) -> Connection {
        Connection {
            id: Some(id),
            protocol: crate::data::store::Protocol::Ssh,
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
            vars: String::new(),
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
