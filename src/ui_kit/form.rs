//! Controles de formulario: filas con etiqueta, campos de texto, buscadores, fichas y mensajes de error.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, FontId, RichText, TextEdit, Ui};

use crate::cursor::HandExt;
use crate::theme;

/// Ancho de la columna de etiquetas de un formulario.
const LABEL_WIDTH: f32 = 112.0;

/// Pide desplazarse hasta el primer campo con error en el próximo dibujo del formulario.
static REVEAL_ERROR: AtomicBool = AtomicBool::new(false);

pub fn request_reveal_error() {
    REVEAL_ERROR.store(true, Ordering::Relaxed);
}

/// `true` una sola vez por cada petición de `request_reveal_error`.
pub fn take_reveal_error() -> bool {
    REVEAL_ERROR.swap(false, Ordering::Relaxed)
}

/// Fila de formulario: etiqueta alineada a la derecha y contenido que ocupa el resto.
pub fn form_row(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, theme::height::FIELD), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.label(label);
        });
        ui.vertical(content);
    });
}

/// Campo de texto con el borde en rojo mientras tiene un error.
pub fn text_input(ui: &mut Ui, value: &mut String, width: f32, hint: &str, error: &mut Option<String>) -> egui::Response {
    let broken = error.is_some();
    let response = ui
        .scope(|ui| {
            if broken {
                let stroke = egui::Stroke::new(theme::stroke::MEDIUM, theme::red());
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.bg_stroke = stroke;
                widgets.hovered.bg_stroke = stroke;
            }
            ui.add_sized([width, theme::height::FIELD], TextEdit::singleline(value).hint_text(hint).align(egui::Align2::LEFT_CENTER))
        })
        .inner;
    if response.changed() {
        *error = None;
    }
    if broken && take_reveal_error() {
        response.scroll_to_me(Some(egui::Align::Center));
    }
    response
}

/// Campo de búsqueda con borde fino. `width` es el ancho, o `f32::INFINITY` para ocupar todo el disponible.
pub fn search_field(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface());
        ui.add(TextEdit::singleline(value).hint_text(hint).desired_width(width).margin(egui::vec2(theme::space::M, theme::space::S)))
    })
    .inner
}

/// Campo de contraseña con el ojo para mostrar u ocultar lo escrito dentro del propio campo.
/// `width` es el ancho, o `f32::INFINITY` para ocupar todo el disponible.
pub fn password_field(ui: &mut Ui, value: &mut String, reveal: &mut bool, hint: &str, width: f32) -> egui::Response {
    let width = if width.is_finite() { width } else { ui.available_width() };
    let margin = egui::Margin { left: theme::space::S as i8, right: 32, top: theme::space::XXS as i8, bottom: theme::space::XXS as i8 };
    let field = ui.add_sized(
        [width, theme::height::FIELD],
        TextEdit::singleline(value).password(!*reveal).hint_text(hint).margin(margin).align(egui::Align2::LEFT_CENTER),
    );
    let slot = egui::Rect::from_center_size(egui::pos2(field.rect.max.x - 17.0, field.rect.center().y), egui::Vec2::splat(1.0));
    let (glyph, tip) = if *reveal { (crate::icons::EYE_OFF, "Ocultar") } else { (crate::icons::EYE, "Mostrar") };
    if super::IconButton::new(glyph).size(theme::size::ICON_SM).show_at(ui, slot, ("reveal", field.id)).on_hover_text(tip).clicked() {
        *reveal = !*reveal;
    }
    field
}

/// Mensaje de error pequeño bajo un campo.
pub fn error_line(ui: &mut Ui, error: &Option<String>) {
    if let Some(message) = error {
        ui.label(RichText::new(message.as_str()).small().color(theme::red()));
    }
}

/// Ficha elegible: nombre, punto de color opcional y fondo si está elegida.
pub fn chip(ui: &mut Ui, label: &str, color: Option<Color32>, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(theme::size::LABEL, egui::FontFamily::Proportional), Color32::PLACEHOLDER);
    let dot = if color.is_some() { 14.0 } else { 0.0 };
    let size = egui::vec2(galley.size().x + dot + 18.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.hand();
    let fill = if selected {
        theme::selection()
    } else if response.hovered() {
        theme::surface()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), fill);
    if !selected {
        ui.painter().rect_stroke(rect, egui::CornerRadius::same(theme::radius::CONTROL), egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface()), egui::StrokeKind::Inside);
    }
    let mut x = rect.min.x + 9.0;
    if let Some(color) = color {
        ui.painter().circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.5, color);
        x += dot;
    }
    let text_color = if selected { theme::text() } else { theme::subtext() };
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_color);
    response
}
