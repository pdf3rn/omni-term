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

const SWITCH_WIDTH: f32 = 30.0;
const SWITCH_HEIGHT: f32 = 16.0;

/// Interruptor de encendido y apagado, sin etiqueta.
pub fn switch(ui: &mut Ui, value: &mut bool) -> egui::Response {
    let enabled = ui.is_enabled();
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(SWITCH_WIDTH, SWITCH_HEIGHT), egui::Sense::click());
    if enabled {
        response = response.hand();
    }
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let on = ui.ctx().animate_bool_with_time(response.id, *value, 0.12);
    let track = if on > 0.5 { theme::accent() } else { theme::surface() };
    let border = if on > 0.5 { theme::accent() } else { theme::subtext() };
    let fade = if enabled { 1.0 } else { 0.45 };
    let radius = SWITCH_HEIGHT / 2.0;
    ui.painter().rect_filled(rect, egui::CornerRadius::same(radius as u8), track.gamma_multiply(fade));
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(radius as u8), egui::Stroke::new(theme::stroke::HAIRLINE, border.gamma_multiply(fade)), egui::StrokeKind::Inside);
    let travel = SWITCH_WIDTH - SWITCH_HEIGHT;
    let knob = egui::pos2(rect.min.x + radius + travel * on, rect.center().y);
    let knob_color = if *value { theme::base() } else { theme::subtext() };
    ui.painter().circle_filled(knob, radius - 3.0, knob_color.gamma_multiply(fade));
    response
}

/// Interruptor con su texto: pulsar el texto también lo cambia.
pub fn switch_labeled(ui: &mut Ui, value: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::M;
        let toggle = switch(ui, value);
        let text = ui.add(egui::Label::new(label).sense(egui::Sense::click())).hand();
        if text.clicked() {
            *value = !*value;
        }
        let mut response = toggle | text;
        if response.clicked() && !response.changed() {
            response.mark_changed();
        }
        response
    })
    .inner
}

/// Alias que usan las filas de ajustes.
pub fn toggle(ui: &mut Ui, value: &mut bool) -> egui::Response {
    switch(ui, value)
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
