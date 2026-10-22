//! Tarjetas y filas de ajustes, títulos de sección y teclas.

use eframe::egui::{self, FontId, Margin, RichText, Ui};

use crate::cursor::HandExt;
use crate::theme;

/// Título de una sección: letra pequeña en negrita con el color secundario.
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::new(theme::size::SECTION, crate::ui::strong_family())).color(theme::subtext()));
}

/// Marco de una tarjeta: fondo, borde fino y esquinas redondeadas. Quien lo usa añade su margen interior.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::mantle())
        .stroke(egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface()))
        .corner_radius(egui::CornerRadius::same(theme::radius::CARD))
}

/// Grupo con título y una tarjeta que contiene sus filas (ver `setting_row`).
pub fn card(ui: &mut Ui, title: &str, rows: impl FnOnce(&mut Ui)) {
    ui.add_space(theme::space::GROUP);
    section_title(ui, title);
    ui.add_space(theme::space::XS);
    card_frame().inner_margin(Margin::symmetric(14, 4)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_width(ui.available_width());
        rows(ui);
    });
}

const SETTING_LABEL_WIDTH: f32 = 290.0;
const SETTING_ROW_HEIGHT: f32 = 48.0;

/// Una fila: el nombre (y una pista) a la izquierda y el control en una columna común a la derecha.
pub fn setting_row(ui: &mut Ui, label: &str, hint: Option<&str>, divider: bool, controls: impl FnOnce(&mut Ui)) {
    // La columna del nombre se estrecha en ventanas pequeñas para que los controles quepan.
    let label_width = (ui.available_width() * 0.42).clamp(150.0, SETTING_LABEL_WIDTH);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::L;
        ui.set_min_height(SETTING_ROW_HEIGHT);
        ui.allocate_ui_with_layout(egui::vec2(label_width, SETTING_ROW_HEIGHT), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.set_min_width(label_width);
            let lines = if hint.is_some() { 2.0 } else { 1.0 };
            ui.add_space((SETTING_ROW_HEIGHT - lines * 17.0 - if hint.is_some() { 1.0 } else { 0.0 }) / 2.0);
            ui.label(RichText::new(label).color(theme::text()));
            if let Some(hint) = hint {
                ui.label(RichText::new(hint).small().color(theme::subtext()));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::XS;
            controls(ui);
        });
    });
    if divider {
        let y = ui.cursor().min.y;
        let x = ui.max_rect().x_range();
        ui.painter().hline(x, y, egui::Stroke::new(theme::stroke::HAIRLINE, theme::surface()));
    }
}

/// Casilla con borde propio: sobre el fondo de una tarjeta clara la casilla vacía casi no se vería.
pub fn toggle(ui: &mut Ui, value: &mut bool) -> egui::Response {
    ui.scope(|ui| {
        let stroke = egui::Stroke::new(theme::stroke::HAIRLINE, theme::subtext());
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = stroke;
        widgets.hovered.bg_stroke = stroke;
        ui.checkbox(value, "").hand()
    })
    .inner
}

/// Combinación de teclas con aspecto de tecla.
pub fn keycap(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::surface())
        .corner_radius(egui::CornerRadius::same(theme::radius::SMALL))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(FontId::new(theme::size::SECTION, egui::FontFamily::Monospace)).color(theme::text()));
        });
}
