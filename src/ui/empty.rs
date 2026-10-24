//! Pantalla de inicio cuando no hay pestañas.

use super::*;

impl App {
    /// Pantalla sin pestañas: qué se puede hacer a continuación. Devuelve `true` si se pide un terminal local.
    pub(super) fn empty_state(&self, ui: &mut egui::Ui, area: Rect) -> Option<EmptyAction> {
        let mut action = None;
        let width = 480.0_f32.min(area.width());
        // Centrado si hay sitio; en ventanas bajas empieza arriba y se desplaza.
        let top = if area.height() > 640.0 { area.center().y - 290.0 } else { area.min.y + 8.0 };
        let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
            Pos2::new(area.center().x - width / 2.0, top),
            Pos2::new(area.center().x + width / 2.0, area.max.y),
        )));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut outer, |child| {
        child.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        child.label(RichText::new("No hay pestañas abiertas").font(FontId::new(theme::size::TITLE, strong_family())));
        child.label(RichText::new("Abre un terminal o conéctate a un servidor guardado.").color(theme::subtext()));
        child.add_space(theme::space::XS);
        child.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            // Outline: el acento marca la acción principal en el borde y el texto, sin relleno.
            if crate::ui::kit::button_rich(ui, RichText::new("Nuevo terminal local").size(theme::size::LABEL), crate::ui::kit::Variant::PrimaryOutline).clicked() {
                action = Some(EmptyAction::Local);
            }
            if crate::ui::kit::button_rich(ui, RichText::new("Nueva conexión").size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::NewConnection);
            }
            if crate::ui::kit::button_rich(ui, RichText::new("Contenedores").size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::Containers);
            }
        });

        child.add_space(theme::space::L);
        let recents: Vec<Connection> = self.recent_connections().into_iter().take(4).collect();
        if recents.is_empty() {
            child.label(RichText::new("Aún no hay conexiones guardadas.").color(theme::subtext()));
            if crate::ui::kit::button_rich(child, RichText::new("Importar de ~/.ssh/config").size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::Import);
            }
        } else {
            crate::ui::kit::section_title(child, "Recientes");
            crate::ui::kit::card_frame().inner_margin(Margin::symmetric(6, 4)).show(child, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(ui.available_width());
                for connection in &recents {
                    let (rect, response) = crate::ui::kit::row_frame(ui, 44.0, Sense::click());
                    let subtitle = format!("{}  ·  {}", connection.kind_label(), connection.destination());
                    crate::ui::kit::row_content(
                        ui,
                        rect,
                        crate::ui::kit::RowContent { leading: crate::ui::kit::Leading::Glyph(crate::ui::kit::connection_glyph(connection)), title: &connection.name, subtitle: &subtitle, right_limit: rect.max.x - 10.0 },
                    );
                    let response = response.hand();
                    if response.on_hover_text("Conectar").clicked() {
                        action = Some(EmptyAction::Connect(connection.clone()));
                    }
                }
            });
        }

        child.add_space(theme::space::L);
        crate::ui::kit::section_title(child, "Atajos");
        for (keys, what) in [
            ("Ctrl+Shift+P", "Paleta de comandos"),
            ("Ctrl+Shift+D / E", "Dividir el panel a la derecha / hacia abajo"),
            ("Ctrl+Shift+B", "Mostrar u ocultar la barra lateral"),
            ("Ctrl+,", "Abrir los ajustes"),
        ] {
            child.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(210.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(210.0);
                    for (n, combo) in keys.split(" / ").enumerate() {
                        if n > 0 {
                            ui.label(RichText::new("/").color(theme::subtext()));
                        }
                        crate::ui::kit::keycap(ui, combo);
                    }
                });
                ui.label(RichText::new(what).color(theme::subtext()));
            });
        }
        if child.add(Button::new(RichText::new("Ver todos los atajos").color(theme::accent())).frame(false)).clicked() {
            action = Some(EmptyAction::Shortcuts);
        }
        });
        action
    }
}

/// Qué se pidió en la pantalla sin pestañas.
pub(super) enum EmptyAction {
    Local,
    NewConnection,
    Containers,
    Import,
    Shortcuts,
    Connect(Connection),
}
