//! Grupo de opciones con una sola parada de Tab: Tab entra y sale del grupo, las flechas mueven la marca
//! y Espacio o Intro eligen la opción marcada. Lo usan las pastillas de entorno, los colores y la lista de tipos.

use std::hash::Hash;

use eframe::egui::{self, CornerRadius, Id, Key, Modifiers, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::ui::theme;

pub struct Roving {
    anchor: Id,
    count: usize,
    focused: bool,
    cursor: usize,
    activated: Option<usize>,
    bounds: Option<Rect>,
}

impl Roving {
    /// Empieza el grupo. `selected` es la opción elegida (donde arranca la marca al entrar), `vertical` decide si
    /// se usan ↑/↓ o ←/→, y con `select_on_move` mover la marca ya elige la opción (una lista de tipos).
    pub fn begin(ui: &mut Ui, salt: impl Hash + std::fmt::Debug, count: usize, selected: Option<usize>, vertical: bool, select_on_move: bool) -> Self {
        let anchor = ui.scope_id().with(("roving", salt));
        let state_id = anchor.with("state");
        let focused = ui.memory(|m| m.has_focus(anchor));
        let start = selected.unwrap_or(0).min(count.saturating_sub(1));
        let (was_focused, mut cursor): (bool, usize) = ui.data(|d| d.get_temp(state_id)).unwrap_or((false, start));
        if focused && !was_focused {
            cursor = start;
        }
        let mut activated = None;
        if focused && count > 0 {
            // Las flechas son del grupo mientras tiene el foco: si no, egui las usa para saltar de widget.
            ui.memory_mut(|m| m.set_focus_lock_filter(anchor, egui::EventFilter { horizontal_arrows: !vertical, vertical_arrows: vertical, ..Default::default() }));
            let (back, forward) = if vertical { (Key::ArrowUp, Key::ArrowDown) } else { (Key::ArrowLeft, Key::ArrowRight) };
            let step = ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, forward) {
                    1
                } else if i.consume_key(Modifiers::NONE, back) {
                    -1
                } else {
                    0
                }
            });
            if step != 0 {
                cursor = (cursor as i32 + step).clamp(0, count as i32 - 1) as usize;
                if select_on_move {
                    activated = Some(cursor);
                }
            }
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space) || i.consume_key(Modifiers::NONE, Key::Enter)) {
                activated = Some(cursor);
            }
        }
        ui.data_mut(|d| d.insert_temp(state_id, (focused, cursor)));
        Self { anchor, count, focused, cursor, activated, bounds: None }
    }

    /// Registra la opción `index` ya dibujada: la marca va sobre ella cuando el grupo tiene el foco, y pulsarla la marca.
    pub fn item(&mut self, ui: &mut Ui, index: usize, response: &Response) {
        self.bounds = Some(self.bounds.map_or(response.rect, |b| b.union(response.rect)));
        if response.clicked() {
            self.cursor = index;
        }
        if self.focused && self.cursor == index && index < self.count {
            ui.painter().rect_stroke(
                response.rect,
                CornerRadius::same(theme::radius::CONTROL),
                Stroke::new(theme::stroke::HAIRLINE, theme::accent().gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// La opción elegida con el teclado en este fotograma, si la hubo.
    pub fn activated(&self) -> Option<usize> {
        self.activated
    }

    /// Cierra el grupo: lo registra como una sola parada de Tab, en el sitio donde se dibujó.
    pub fn finish(self, ui: &mut Ui) {
        let Some(bounds) = self.bounds else { return };
        let state_id = self.anchor.with("state");
        ui.data_mut(|d| d.insert_temp(state_id, (self.focused, self.cursor)));
        // Un punto: el anillo global de foco ignora los widgets sin tamaño; el de la opción lo pinta `item`.
        ui.interact(Rect::from_min_size(bounds.min, Vec2::splat(1.0)), self.anchor, Sense::focusable_noninteractive());
    }
}
