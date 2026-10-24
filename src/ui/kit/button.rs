//! Botones: una sola función con variantes en lugar de un estilo por cada sitio.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::ui::cursor::HandExt;
use crate::ui::theme;

/// Qué papel juega el botón en la pantalla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Acción principal: relleno con el acento.
    Primary,
    /// Acción principal sin relleno: borde y texto con el acento.
    PrimaryOutline,
    /// Acción secundaria: borde neutro.
    Secondary,
    /// Sin marco hasta que el ratón pasa por encima (enlaces, iconos, pestañas internas).
    Ghost,
    /// Acción destructiva: relleno rojo.
    Danger,
}

/// Botón de texto con el estilo de su variante.
pub fn button(ui: &mut Ui, text: &str, variant: Variant) -> egui::Response {
    button_with(ui, egui::Button::new(text), variant)
}

/// Igual que `button` con un texto ya formateado.
pub fn button_rich(ui: &mut Ui, text: RichText, variant: Variant) -> egui::Response {
    button_with(ui, egui::Button::new(text), variant)
}

/// Botón ya construido (con su tamaño mínimo, por ejemplo) al que se le aplica el estilo de la variante.
pub fn button_with(ui: &mut Ui, button: egui::Button<'_>, variant: Variant) -> egui::Response {
    if variant == Variant::Ghost {
        return ui.add(button.frame_when_inactive(false)).hand();
    }
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        match variant {
            Variant::Primary | Variant::Danger => {
                let color = if variant == Variant::Danger { theme::red() } else { theme::accent() };
                widgets.inactive.weak_bg_fill = color;
                widgets.hovered.weak_bg_fill = color.lerp_to_gamma(Color32::WHITE, 0.18);
                widgets.active.weak_bg_fill = color.lerp_to_gamma(Color32::WHITE, 0.32);
                ui.visuals_mut().override_text_color = Some(theme::base());
            }
            Variant::PrimaryOutline => {
                let width = theme::stroke::MEDIUM;
                widgets.inactive.bg_stroke = Stroke::new(width, theme::accent());
                widgets.hovered.bg_stroke = Stroke::new(width, theme::accent());
                widgets.active.bg_stroke = Stroke::new(width, theme::accent());
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.hovered.weak_bg_fill = theme::accent().gamma_multiply(0.18);
                widgets.active.weak_bg_fill = theme::accent().gamma_multiply(0.3);
                ui.visuals_mut().override_text_color = Some(theme::accent());
            }
            Variant::Secondary => {
                widgets.inactive.bg_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::selection());
                widgets.hovered.bg_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::subtext());
                widgets.active.bg_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::subtext());
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.hovered.weak_bg_fill = theme::surface();
                widgets.active.weak_bg_fill = theme::selection();
            }
            Variant::Ghost => {}
        }
        ui.add(button).hand()
    })
    .inner
}

/// Lado de los botones de icono.
const ICON_BUTTON: f32 = 26.0;

/// Lo que se dibuja dentro de un botón de icono.
pub enum IconGlyph {
    /// Glifo de la fuente de iconos.
    Text(&'static str),
    /// Cuadrado relleno: el icono de «parar» se dibuja a mano.
    Stop,
}

/// Botón de icono: sin marco, con fondo al pasar el ratón. Es el único botón de icono de la aplicación.
pub struct IconButton {
    glyph: IconGlyph,
    size: f32,
    color: Color32,
    enabled: bool,
    active: bool,
}

impl IconButton {
    pub fn new(glyph: &'static str) -> Self {
        Self { glyph: IconGlyph::Text(glyph), size: theme::size::ICON, color: theme::text(), enabled: true, active: false }
    }

    pub fn stop() -> Self {
        Self { glyph: IconGlyph::Stop, ..Self::new("") }
    }

    /// Tamaño del glifo (`theme::size::ICON…`).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    /// Desactivado: sin acción y atenuado.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Resaltado como encendido (un interruptor, o un menú abierto desde este botón).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn show(self, ui: &mut Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), if self.enabled { Sense::click() } else { Sense::hover() });
        self.paint(ui, rect, response)
    }

    /// Igual que `show`, centrado en `slot` (para colocarlo en un sitio fijo de una fila). `id` lo distingue de los demás.
    pub fn show_at(self, ui: &mut Ui, slot: Rect, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Response {
        let rect = Rect::from_center_size(slot.center(), Vec2::splat(ICON_BUTTON));
        // Un hijo no avanza el cursor del padre: la fila donde se coloca conserva su altura.
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(rect));
        self.show(&mut child)
    }

    fn paint(self, ui: &mut Ui, rect: Rect, response: egui::Response) -> egui::Response {
        let response = if self.enabled { response.hand() } else { response };
        let radius = CornerRadius::same(theme::radius::CONTROL);
        if self.active {
            ui.painter().rect_filled(rect, radius, theme::accent().gamma_multiply(0.28));
        } else if self.enabled && response.hovered() {
            ui.painter().rect_filled(rect, radius, theme::selection());
        }
        let ink = if self.enabled { self.color } else { self.color.gamma_multiply(0.45) };
        match self.glyph {
            IconGlyph::Text(text) => {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(self.size), ink);
            }
            IconGlyph::Stop => {
                ui.painter().rect_filled(Rect::from_center_size(rect.center(), Vec2::splat(11.0)), CornerRadius::same(theme::radius::TINY), ink);
            }
        }
        response
    }
}

/// Botón de icono sencillo: glifo, tamaño y color.
pub fn icon_button(ui: &mut Ui, glyph: &'static str, size: f32, color: Color32) -> egui::Response {
    IconButton::new(glyph).size(size).color(color).show(ui)
}
