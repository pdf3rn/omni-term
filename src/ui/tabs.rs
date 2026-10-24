//! Pestañas: su estado, la barra de pestañas y el menú de «+».

use super::*;

pub(super) enum Body {
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

pub(super) struct Tab {
    pub(super) id: u64,
    pub(super) kind: TabKind,
    pub(super) label: String,
    /// Nombre puesto por el usuario; manda sobre el título del terminal.
    pub(super) custom: Option<String>,
    /// Hubo actividad en segundo plano que el usuario aún no ha visto.
    pub(super) attention: bool,
    pub(super) body: Body,
}

impl Body {
    pub(super) fn terminal(pane: Pane, tunnels: Option<Tunnels>) -> Self {
        let id = 1;
        Body::Terminal { panes: Tree::new(id, pane), focused: id, next_pane: id + 1, tunnels }
    }
}

impl Tab {
    /// Qué está haciendo la pestaña que se perdería al cerrarla, si algo.
    pub(super) fn busy_reason(&self) -> Option<String> {
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

    pub(super) fn pane_mut(&mut self) -> Option<&mut Pane> {
        match &mut self.body {
            Body::Terminal { panes, focused, .. } => panes.get_mut(*focused),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    /// El panel con foco de una pestaña de terminal.
    pub(super) fn pane(&self) -> Option<&Pane> {
        match &self.body {
            Body::Terminal { panes, focused, .. } => panes.get(*focused),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    pub(super) fn tunnels(&self) -> Option<Tunnels> {
        match &self.body {
            Body::Terminal { tunnels, .. } => tunnels.clone(),
            Body::Explorer(_) | Body::Containers(_) | Body::Settings(_) => None,
        }
    }

    /// Título mostrado: el del terminal con foco si lo ha fijado, y si no la etiqueta de la pestaña.
    pub(super) fn title(&self) -> String {
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

pub(super) fn tab(
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
    let font = FontId::new(theme::size::BODY, if selected { strong_family() } else { FontFamily::Proportional });
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
        ui.painter().text(Pos2::new(rect.min.x + 18.0, rect.center().y + 1.0), Align2::CENTER_CENTER, icons::LOCK_OPEN, FontId::proportional(theme::size::LABEL), theme::red());
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
            ui.painter().rect_filled(close_rect, CornerRadius::same(theme::radius::SMALL), theme::selection());
        }
        ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, icons::X, FontId::proportional(theme::size::LABEL), color);
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

/// Pestaña con trazo de acento arriba cuando está activa; el cierre aparece al pasar el ratón.
#[derive(Clone, Copy)]
pub(super) enum TabAction {
    Rename,
    Duplicate,
    CloseOthers,
    CloseRight,
    Close,
}

pub(super) struct TabOutcome {
    pub(super) clicked: bool,
    pub(super) closed: bool,
    pub(super) dragged: bool,
    pub(super) rename: bool,
    pub(super) rect: Rect,
}

/// Contenido del menú del «+»: terminal local primero, luego conexiones recientes y «Ver todas».
pub(super) fn plus_menu(ui: &mut egui::Ui, recents: &[Connection], selected: &mut usize, choice: &mut Option<PlusChoice>) {
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
            ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), theme::selection());
        }
        (rect, response.clicked())
    };

    let (rect, clicked) = row(ui, 0, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::TERMINAL, FontId::proportional(theme::size::ICON_LG), theme::text());
    ui.painter().text(rect.min + Vec2::new(42.0, 13.0), Align2::LEFT_CENTER, "Terminal local", FontId::new(theme::size::BODY, strong_family()), theme::text());
    ui.painter().text(rect.min + Vec2::new(42.0, 29.0), Align2::LEFT_CENTER, "Este equipo", FontId::new(theme::size::SMALL, FontFamily::Proportional), theme::subtext());
    if clicked {
        picked = Some(0);
    }

    if !recents.is_empty() {
        ui.add_space(theme::space::S);
        ui.label(RichText::new("Recientes").small().color(theme::subtext()));
    }
    for (offset, connection) in recents.iter().enumerate() {
        let index = offset + 1;
        let (rect, clicked) = row(ui, index, selected);
        let badge = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::splat(28.0));
        let fill = connection.color.map_or_else(theme::surface, theme::identity);
        let ink = if connection.color.is_some() { theme::base() } else { theme::text() };
        ui.painter().rect_filled(badge, CornerRadius::same(theme::radius::CONTROL), fill);
        let initial = connection.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, initial, FontId::new(theme::size::BODY, strong_family()), ink);
        ui.painter().text(rect.min + Vec2::new(44.0, 13.0), Align2::LEFT_CENTER, &connection.name, FontId::new(theme::size::BODY, strong_family()), theme::text());
        ui.painter().text(
            rect.min + Vec2::new(44.0, 29.0),
            Align2::LEFT_CENTER,
            connection.target(),
            FontId::new(theme::size::SMALL, FontFamily::Proportional),
            theme::subtext(),
        );
        let files_slot = Rect::from_center_size(Pos2::new(rect.max.x - 20.0, rect.center().y), Vec2::splat(26.0));
        let files_response = crate::ui::kit::IconButton::new(icons::FOLDER).color(theme::subtext()).show_at(ui, files_slot, ("plus-files", index));
        if files_response.on_hover_text("Abrir el explorador de archivos").clicked() {
            files_for = Some(index);
        } else if clicked {
            picked = Some(index);
        }
    }

    ui.add_space(theme::space::XS);
    let containers_index = rows - 2;
    let (rect, clicked) = row(ui, containers_index, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::DOTS, FontId::proportional(theme::size::ICON), theme::subtext());
    ui.painter().text(rect.min + Vec2::new(42.0, 20.0), Align2::LEFT_CENTER, "Contenedores", FontId::new(theme::size::BODY, FontFamily::Proportional), theme::text());
    if clicked {
        picked = Some(containers_index);
    }
    let index = rows - 1;
    let (rect, clicked) = row(ui, index, selected);
    ui.painter().text(rect.min + Vec2::new(20.0, 20.0), Align2::CENTER_CENTER, icons::SEARCH, FontId::proportional(theme::size::ICON), theme::subtext());
    ui.painter().text(rect.min + Vec2::new(42.0, 20.0), Align2::LEFT_CENTER, "Ver todas…", FontId::new(theme::size::BODY, FontFamily::Proportional), theme::text());
    ui.painter().text(rect.max - Vec2::new(10.0, 20.0), Align2::RIGHT_CENTER, "Ctrl+Shift+P", FontId::new(theme::size::SMALL, FontFamily::Proportional), theme::subtext());
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

pub(super) enum PlusChoice {
    Local,
    Connect(Connection),
    Files(Connection),
    Containers,
    All,
}

/// Una línea por túnel: punto de estado, resumen y número de conexiones atendidas.
pub(super) fn tunnel_line(ui: &mut egui::Ui, status: &ssh::TunnelStatus) {
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
        FontId::new(theme::size::SMALL, FontFamily::Proportional),
        theme::subtext(),
    );
    if status.served > 0 {
        ui.painter().text(
            Pos2::new(rect.max.x - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status.served.to_string(),
            FontId::new(theme::size::SMALL, FontFamily::Proportional),
            theme::subtext(),
        );
    }
    response.on_hover_text(tip);
}
