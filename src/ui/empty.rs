//! Pantalla de inicio cuando no hay pestañas.

use crate::i18n::t;
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
        child.label(RichText::new(t("empty-no-hay-pestanas-abiertas")).font(FontId::new(theme::size::TITLE, strong_family())));
        child.label(RichText::new(t("empty-abre-un-terminal-o-conectate")).color(theme::subtext()));
        child.add_space(theme::space::XS);
        child.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            // Outline: el acento marca la acción principal en el borde y el texto, sin relleno.
            if crate::ui::kit::button_rich(ui, RichText::new(t("empty-nuevo-terminal-local")).size(theme::size::LABEL), crate::ui::kit::Variant::PrimaryOutline).clicked() {
                action = Some(EmptyAction::Local);
            }
            if crate::ui::kit::button_rich(ui, RichText::new(t("dialogs-nueva-conexion")).size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::NewConnection);
            }
            if crate::ui::kit::button_rich(ui, RichText::new(t("conn-edit-contenedores")).size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::Containers);
            }
        });

        child.add_space(theme::space::L);
        let recents: Vec<Connection> = self.recent_connections().into_iter().take(4).collect();
        if recents.is_empty() {
            child.label(RichText::new(t("empty-aun-no-hay-conexiones-guardadas")).color(theme::subtext()));
            if crate::ui::kit::button_rich(child, RichText::new(t("empty-importar-de-ssh-config")).size(theme::size::LABEL), crate::ui::kit::Variant::Secondary).clicked() {
                action = Some(EmptyAction::Import);
            }
        } else {
            crate::ui::kit::section_title(child, t("empty-recientes"));
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
                    if response.on_hover_text(t("dialogs-conectar")).clicked() {
                        action = Some(EmptyAction::Connect(connection.clone()));
                    }
                }
            });
        }

        child.add_space(theme::space::L);
        crate::ui::kit::section_title(child, t("empty-atajos"));
        let shortcuts = [
            ("Ctrl+Shift+P", t("settings-paleta-de-comandos")),
            ("Ctrl+Shift+D / E", t("empty-dividir-el-panel-a-la")),
            ("Alt+flechas", t("settings-ir-al-panel-de-al")),
            ("Ctrl+Shift+B", t("settings-mostrar-u-ocultar-la-barra")),
            ("Ctrl+,", t("settings-abrir-los-ajustes")),
        ];
        // Rejilla: la columna de teclas mide lo que la combinación más larga, sin un hueco fijo.
        egui::Grid::new("empty-shortcuts").num_columns(2).spacing(Vec2::new(theme::space::XL, theme::space::S)).show(child, |ui| {
            for (keys, what) in shortcuts {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::space::XS;
                    for (n, combo) in keys.split(" / ").enumerate() {
                        if n > 0 {
                            ui.label(RichText::new("/").color(theme::subtext()));
                        }
                        crate::ui::kit::keycap(ui, combo);
                    }
                });
                ui.label(RichText::new(what).color(theme::subtext()));
                ui.end_row();
            }
        });
        if child.add(Button::new(RichText::new(t("empty-ver-todos-los-atajos")).color(theme::accent())).frame(false)).clicked() {
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
