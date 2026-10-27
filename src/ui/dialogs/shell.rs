//! Marco común de los diálogos: título, cuerpo y pie con botones.

use super::*;

/// Aspecto de la acción principal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Acción destructiva: botón rojo y sin confirmar con el teclado.
    Danger,
}

/// Qué tecla confirma el diálogo, como si se pulsara su botón principal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Submit {
    Enter,
    /// Formularios con texto multilínea, donde Enter escribe un salto de línea.
    CtrlEnter,
    /// Acciones destructivas o de seguridad: solo con un clic deliberado.
    Never,
}

/// Lo que cada diálogo declara de sí mismo; el marco común dibuja el resto.
/// Icono de estado a la izquierda del cuerpo en confirmaciones, errores y avisos.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogIcon {
    Warning,
    Error,
    Info,
}

pub struct Spec {
    icon: Option<DialogIcon>,
    title: String,
    context: Option<String>,
    width: f32,
    accept: Option<String>,
    accept_enabled: bool,
    tone: Tone,
    cancel: String,
    extras: Vec<String>,
    submit: Submit,
    backdrop_cancels: bool,
    status: Option<(String, Color32)>,
    /// Ancho de una columna lateral hundida entre el título y el pie, a la izquierda; el cuerpo dibuja su contenido.
    side_width: f32,
}

impl Spec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            context: None,
            width: 520.0,
            accept: None,
            accept_enabled: true,
            tone: Tone::Normal,
            cancel: "Cancelar".into(),
            extras: Vec::new(),
            submit: Submit::Enter,
            backdrop_cancels: false,
            status: None,
            side_width: 0.0,
        }
    }

    pub fn icon(mut self, icon: DialogIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn context(mut self, text: impl Into<String>) -> Self {
        self.context = Some(text.into());
        self
    }

    /// 380 px para preguntas y confirmaciones, 520 para formularios.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn accept(mut self, label: impl Into<String>) -> Self {
        self.accept = Some(label.into());
        self
    }

    pub fn accept_enabled(mut self, enabled: bool) -> Self {
        self.accept_enabled = enabled;
        self
    }

    pub fn danger(mut self) -> Self {
        self.tone = Tone::Danger;
        self.submit = Submit::Never;
        self.backdrop_cancels = true;
        self
    }

    pub fn cancel(mut self, label: impl Into<String>) -> Self {
        self.cancel = label.into();
        self
    }

    pub fn extra(mut self, label: impl Into<String>) -> Self {
        self.extras.push(label.into());
        self
    }

    /// Confirma con Ctrl+Enter en lugar de Enter (formularios con texto multilínea).
    pub fn ctrl_enter(mut self) -> Self {
        self.submit = Submit::CtrlEnter;
        self
    }

    /// Sin atajo de teclado: avisos de seguridad que solo se aceptan con un clic.
    pub fn click_only(mut self) -> Self {
        self.submit = Submit::Never;
        self
    }

    /// Una línea de estado sobre el pie (resultado de una prueba, error al guardar).
    pub fn status(mut self, text: impl Into<String>, color: Color32) -> Self {
        self.status = Some((text.into(), color));
        self
    }

    /// Reserva una columna lateral hundida pegada al borde izquierdo, entre la franja del título y la del pie.
    pub fn side(mut self, width: f32) -> Self {
        self.side_width = width;
        self
    }

    /// Un clic fuera cierra el diálogo; solo para los que no tienen nada que perder.
    pub fn backdrop_closes(mut self) -> Self {
        self.backdrop_cancels = true;
        self
    }
}

pub enum Action {
    None,
    Accept,
    Cancel,
    Extra(usize),
}

/// Marco común: título, contexto, cuerpo y pie con los botones a la derecha.
/// `Esc` cancela. Enter (o Ctrl+Enter, según `Spec`) pulsa el botón principal: el de aceptar o, si no
/// hay, el de cerrar. Con un botón enfocado con Tab, Enter pulsa ese botón. El cuerpo devuelve `true`
/// cuando se pulsó Enter en uno de sus campos de una línea.
pub fn shell(ctx: &egui::Context, spec: Spec, body: impl FnOnce(&mut egui::Ui) -> bool) -> Action {
    let mut action = Action::None;
    // En el fotograma en que aparece, el Enter que lo abrió (p. ej. en la barra lateral) no lo confirma.
    // Se guarda (abierto en, visto por última vez en): un hueco de fotogramas significa que es un diálogo nuevo.
    let now = ctx.cumulative_frame_nr();
    let shown_key = id().with("shown");
    let just_opened = ctx.data_mut(|d| {
        let (opened, last) = d.get_temp::<(u64, u64)>(shown_key).unwrap_or((now, now));
        let opened = if last + 1 < now { now } else { opened };
        d.insert_temp(shown_key, (opened, now));
        opened == now
    });
    // Se consume antes del cuerpo para que un campo multilínea no lo convierta en salto de línea.
    let ctrl_enter = !just_opened && spec.submit == Submit::CtrlEnter && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
    let enter = !just_opened && spec.submit == Submit::Enter && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::Enter));
    let strip = if theme::is_light() { theme::surface() } else { theme::base() };
    let frame = egui::Frame::popup(&ctx.global_style())
        .inner_margin(egui::Margin::ZERO)
        .corner_radius(egui::CornerRadius::same(theme::radius::CONTROL));
    let icon = spec.icon.or((spec.tone == Tone::Danger).then_some(DialogIcon::Warning));
    let response = Modal::new(id()).frame(frame).show(ctx, |ui| {
        ui.set_width(spec.width);
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        let side = spec.side_width;
        // Hueco para el fondo de la columna lateral, que se rellena al final cuando se conoce la altura.
        let side_bg = ui.painter().add(egui::Shape::Noop);

        // Franja del título, con la X para cerrar.
        let (full_title, _) = ui.allocate_exact_size(egui::vec2(spec.width, 38.0), egui::Sense::hover());
        let title_rect = full_title;
        ui.painter().rect_filled(title_rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, strip);
        ui.painter().line_segment([title_rect.left_bottom(), title_rect.right_bottom()], egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface()));
        ui.painter().text(title_rect.center(), egui::Align2::CENTER_CENTER, &spec.title, FontId::new(theme::size::BODY, crate::ui::strong_family()), theme::text());
        let close_slot = egui::Rect::from_center_size(egui::pos2(title_rect.max.x - 22.0, title_rect.center().y), egui::vec2(26.0, 26.0));
        let close = crate::ui::kit::IconButton::new(crate::ui::icons::X).size(theme::size::ICON_SM).show_at(ui, close_slot, "dialog-close");
        if close.on_hover_text("Cerrar").clicked() {
            action = Action::Cancel;
        }

        let mut field_enter = false;
        let body_margin = if side > 0.0 { egui::Margin::ZERO } else { egui::Margin { left: 18, right: 18, top: 16, bottom: 18 } };
        egui::Frame::NONE.inner_margin(body_margin).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            if side > 0.0 {
                // El cuerpo es más claro que el de un diálogo normal: los campos se hunden en lugar de destacar.
                ui.visuals_mut().extreme_bg_color = theme::mantle();
            }
            if let Some(context) = &spec.context {
                ui.label(RichText::new(context).small().color(theme::subtext()));
            }
            let mut body = Some(body);
            let mut content = |ui: &mut egui::Ui| -> bool {
                let body = body.take().expect("el cuerpo se dibuja una vez");
                match icon {
                    Some(icon) => {
                        let mut enter = false;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (glyph, color) = match icon {
                                DialogIcon::Warning => (crate::ui::icons::WARNING, theme::identity(1)),
                                DialogIcon::Error => (crate::ui::icons::ERROR, theme::red()),
                                DialogIcon::Info => (crate::ui::icons::INFO, theme::accent()),
                            };
                            ui.add_sized([26.0, 26.0], egui::Label::new(RichText::new(glyph).size(24.0).color(color)));
                            ui.vertical(|ui| enter = body(ui));
                        });
                        enter
                    }
                    None => body(ui),
                }
            };
            field_enter = content(ui);
        });

        if let Some((text, color)) = &spec.status {
            ui.horizontal(|ui| {
                ui.add_space(side);
                egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 18, top: 0, bottom: 10 }).show(ui, |ui| {
                    ui.label(RichText::new(text.as_str()).small().color(*color));
                });
            });
        }

        // Franja de botones.
        let (full_foot, _) = ui.allocate_exact_size(egui::vec2(spec.width, 56.0), egui::Sense::hover());
        let foot_rect = full_foot;
        ui.painter().rect_filled(foot_rect, egui::CornerRadius { nw: 0, ne: 0, sw: 6, se: 6 }, strip);
        ui.painter().line_segment([foot_rect.left_top(), foot_rect.right_top()], egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface()));
        let mut foot = ui.new_child(egui::UiBuilder::new().max_rect(foot_rect.shrink2(egui::vec2(14.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        foot.spacing_mut().item_spacing.x = 8.0;
        let size = egui::vec2(0.0, 30.0);
        for (index, label) in spec.extras.iter().enumerate() {
            let extra = egui::Button::new(label.as_str()).min_size(size);
            if crate::ui::kit::button_with(&mut foot, extra, crate::ui::kit::Variant::Secondary).clicked() {
                action = Action::Extra(index);
            }
        }
        foot.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let variant = if spec.tone == Tone::Danger { crate::ui::kit::Variant::Danger } else { crate::ui::kit::Variant::Primary };
            let primary = |label: &str| egui::Button::new(label).min_size(size);
            match &spec.accept {
                Some(label) => {
                    let accept = ui.add_enabled_ui(spec.accept_enabled, |ui| crate::ui::kit::button_with(ui, primary(label), variant)).inner;
                    let accept = if spec.submit == Submit::CtrlEnter { accept.on_hover_text("Ctrl+Enter") } else { accept };
                    if accept.clicked() {
                        action = Action::Accept;
                    }
                    let cancel = egui::Button::new(spec.cancel.as_str()).min_size(size);
                    if crate::ui::kit::button_with(ui, cancel, crate::ui::kit::Variant::Secondary).clicked() {
                        action = Action::Cancel;
                    }
                }
                None => {
                    if crate::ui::kit::button_with(ui, primary(&spec.cancel), variant).clicked() {
                        action = Action::Cancel;
                    }
                }
            }
        });
        if side > 0.0 {
            let panel = egui::Rect::from_min_max(egui::pos2(full_title.min.x, full_title.max.y), egui::pos2(full_title.min.x + side, full_foot.min.y));
            let form = egui::Rect::from_min_max(egui::pos2(panel.max.x, panel.min.y), egui::pos2(full_title.max.x, panel.max.y));
            ui.painter().set(side_bg, egui::Shape::Vec(vec![
                egui::Shape::rect_filled(panel, egui::CornerRadius::ZERO, theme::sunken()),
                egui::Shape::rect_filled(form, egui::CornerRadius::ZERO, theme::base()),
                egui::Shape::line_segment([panel.right_top(), panel.right_bottom()], egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface())),
            ]));
        }
        // Con un widget enfocado con Tab, Enter ya lo pulsa a él; un campo de una línea suelta el foco al pulsarlo.
        let unfocused = ui.memory(|m| m.focused().is_none());
        let submitted = ctrl_enter || (enter && (unfocused || field_enter));
        if submitted && matches!(action, Action::None) {
            match &spec.accept {
                Some(_) if spec.accept_enabled => action = Action::Accept,
                Some(_) => {}
                None => action = Action::Cancel,
            }
        }
    });
    if matches!(action, Action::None) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Action::Cancel;
        } else if spec.backdrop_cancels && response.backdrop_response.clicked() {
            action = Action::Cancel;
        }
    }
    action
}

/// Logo de la app como textura, creada una sola vez.
pub fn app_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let id = Id::NULL.with("app-logo");
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")).expect("assets/icon.png es un PNG válido");
    let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    let texture = ctx.load_texture("app-logo", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    texture
}

pub(super) fn id() -> Id {
    Id::NULL.with("dialog")
}

pub(super) fn plain(action: Action, accepted: Outcome) -> Outcome {
    match action {
        Action::Accept => accepted,
        Action::Cancel => Outcome::Cancel,
        Action::Extra(_) | Action::None => Outcome::None,
    }
}

pub(super) fn entered(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

pub(super) fn error_label(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(error) = error {
        ui.add_space(theme::space::XS);
        ui.label(RichText::new(error).color(theme::red()));
    }
}

/// Campo de contraseña con el ojo dentro para mostrar u ocultar lo escrito.
pub(super) fn secret_field(ui: &mut egui::Ui, value: &mut String, reveal: &mut bool, hint: &str) -> egui::Response {
    crate::ui::kit::password_field(ui, value, reveal, hint, f32::INFINITY)
}
