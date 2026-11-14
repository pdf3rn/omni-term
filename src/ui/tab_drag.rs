//! Arrastrar una pestaña por la tira: reordenarla entre las demás o meterla en otra pestaña como panel dividido.
//! Esc, soltar en un sitio no válido o fuera de la ventana cancela.

use std::time::Instant;

use eframe::egui::{Id, LayerId, Order};

use super::pane_drag::{preview, zone, Zone};
use super::*;

/// Tiempo sobre otra pestaña antes de que se active para soltar en sus paneles.
const HOVER_ACTIVATE: Duration = Duration::from_millis(500);
/// Fracción de cada lado de una pestaña que cuenta como «entre pestañas»; el resto la mete dentro.
const SLOT_EDGE: f32 = 0.25;
/// Lo que hay que mover el puntero antes de que un clic sea un arrastre.
const DRAG_THRESHOLD: f32 = 4.0;

pub struct TabDrag {
    tab: u64,
    origin: Pos2,
    moved: bool,
    /// Pestaña bajo el puntero y desde cuándo, para activarla al rato.
    hover: Option<(u64, Instant)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drop {
    /// Hueco entre pestañas (0 = antes de la primera, `len` = después de la última).
    Slot(usize),
    /// Dentro de esa pestaña, a la derecha de su panel con foco.
    Into(u64),
    /// Junto a un panel de la pestaña visible.
    Pane { tab: u64, target: u64, side: Side },
    /// Un destino que no admite la pestaña (otro tipo).
    Refused,
}

/// Hueco de la tira en el que cae `x` y, si cae en el centro de una pestaña que admite la arrastrada, esa pestaña.
fn strip_drop(tabs: &[(u64, Rect)], dragged: u64, x: f32, can_merge: impl Fn(u64) -> bool) -> Drop {
    for (id, rect) in tabs {
        let edge = rect.width() * SLOT_EDGE;
        if *id != dragged && can_merge(*id) && x > rect.min.x + edge && x < rect.max.x - edge {
            return Drop::Into(*id);
        }
    }
    Drop::Slot(tabs.iter().filter(|(_, rect)| rect.center().x < x).count())
}

/// Posición final de la pestaña `from` al soltarla en el hueco `slot`; `None` si no cambia de sitio.
fn reordered(from: usize, slot: usize) -> Option<usize> {
    let to = if slot > from { slot - 1 } else { slot };
    (to != from).then_some(to)
}

/// X de la línea que marca el hueco `slot` entre pestañas.
fn slot_x(tabs: &[(u64, Rect)], slot: usize) -> Option<f32> {
    match (slot.checked_sub(1).and_then(|i| tabs.get(i)), tabs.get(slot)) {
        (Some((_, before)), Some((_, after))) => Some((before.max.x + after.min.x) / 2.0),
        (Some((_, before)), None) => Some(before.max.x),
        (None, Some((_, after))) => Some(after.min.x),
        (None, None) => None,
    }
}

impl App {
    /// Si el contenido de la pestaña `tab` se puede meter en `other`: los dos son terminales del mismo tipo.
    fn tab_accepts(&self, tab: u64, other: u64) -> bool {
        let (Some(a), Some(b)) = (self.tabs.iter().find(|t| t.id == tab), self.tabs.iter().find(|t| t.id == other)) else { return false };
        tab != other && matches!(a.kind, TabKind::Local | TabKind::Connection(_)) && a.kind == b.kind && matches!(a.body, Body::Terminal { .. }) && matches!(b.body, Body::Terminal { .. })
    }

    pub(super) fn start_tab_drag(&mut self, tab: u64, origin: Pos2) {
        if self.tabs.iter().any(|t| t.id == tab) {
            self.tab_drag = Some(TabDrag { tab, origin, moved: false, hover: None });
        }
    }

    fn tab_drop_at(&self, drag: &TabDrag, pointer: Pos2) -> Option<(Drop, Rect)> {
        let targets = &self.drop_targets;
        if targets.tab_bar.contains(pointer) {
            let bar = targets.tab_bar.y_range();
            return Some(match strip_drop(&targets.tabs, drag.tab, pointer.x, |other| self.tab_accepts(drag.tab, other)) {
                Drop::Into(id) => (Drop::Into(id), targets.tabs.iter().find(|(t, _)| *t == id).map_or(Rect::NOTHING, |(_, rect)| *rect)),
                Drop::Slot(slot) => {
                    let line = slot_x(&targets.tabs, slot).map_or(Rect::NOTHING, |x| Rect::from_x_y_ranges(x - theme::stroke::THICK..=x + theme::stroke::THICK, bar));
                    (Drop::Slot(slot), line)
                }
                other => (other, Rect::NOTHING),
            });
        }
        // Sobre el contenido de otra pestaña: junto al panel bajo el puntero.
        let (tab, leaves) = targets.panes.as_ref()?;
        if *tab == drag.tab {
            return None;
        }
        let (target, rect) = leaves.iter().find(|(_, rect)| rect.contains(pointer))?;
        if !self.tab_accepts(drag.tab, *tab) {
            return Some((Drop::Refused, *rect));
        }
        // El centro cuenta como «a la derecha»: soltar sobre un panel siempre hace algo previsible.
        let side = match zone(*rect, pointer) {
            Zone::Edge(side) => side,
            Zone::Center => Side::Right,
        };
        Some((Drop::Pane { tab: *tab, target: *target, side }, preview(*rect, Zone::Edge(side))))
    }

    /// Sigue el arrastre de una pestaña: pinta el destino, activa pestañas al pasar por encima y aplica el
    /// movimiento al soltar.
    pub(super) fn drive_tab_drag(&mut self, ctx: &egui::Context) {
        let Some(mut drag) = self.tab_drag.take() else { return };
        if !self.tabs.iter().any(|t| t.id == drag.tab) || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return;
        }
        let (down, pointer) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.hover_pos()));
        if !drag.moved {
            // Un clic sin desplazamiento no es un arrastre: lo atiende la propia pestaña.
            drag.moved = pointer.is_some_and(|p| p.distance(drag.origin) > DRAG_THRESHOLD);
            if !drag.moved {
                if down {
                    self.tab_drag = Some(drag);
                }
                return;
            }
        }
        let target = pointer.and_then(|p| self.tab_drop_at(&drag, p));
        if !down {
            if let Some((drop, _)) = target {
                self.apply_tab_drop(&drag, drop);
            }
            return;
        }

        // Pasar un rato sobre otra pestaña la activa, para soltar en uno de sus paneles.
        let over_tab = match target {
            Some((Drop::Into(tab), _)) => Some(tab),
            _ => None,
        };
        match over_tab {
            Some(tab) => match drag.hover {
                Some((hovered, since)) if hovered == tab => {
                    if since.elapsed() >= HOVER_ACTIVATE {
                        if let Some(index) = self.tabs.iter().position(|t| t.id == tab) {
                            self.active = index;
                        }
                        drag.hover = None;
                    }
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
                _ => {
                    drag.hover = Some((tab, Instant::now()));
                    ctx.request_repaint_after(HOVER_ACTIVATE);
                }
            },
            None => drag.hover = None,
        }

        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::NULL.with("tab-drop")));
        match target {
            Some((Drop::Refused, _)) => ctx.set_cursor_icon(egui::CursorIcon::NotAllowed),
            Some((Drop::Slot(_), line)) => {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                painter.rect_filled(line, CornerRadius::same(theme::radius::TINY), theme::accent());
            }
            Some((_, rect)) => {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                painter.rect(rect, CornerRadius::same(theme::radius::SMALL), theme::accent().gamma_multiply(0.22), Stroke::new(theme::stroke::MEDIUM, theme::accent()), egui::StrokeKind::Inside);
            }
            None => ctx.set_cursor_icon(egui::CursorIcon::Grabbing),
        }
        ctx.request_repaint();
        self.tab_drag = Some(drag);
    }

    fn apply_tab_drop(&mut self, drag: &TabDrag, drop: Drop) {
        self.renaming = None;
        match drop {
            Drop::Refused => {}
            Drop::Slot(slot) => {
                let Some(from) = self.tabs.iter().position(|t| t.id == drag.tab) else { return };
                if let Some(to) = reordered(from, slot) {
                    let tab = self.tabs.remove(from);
                    self.tabs.insert(to.min(self.tabs.len()), tab);
                }
                // La pestaña arrastrada queda activa en su sitio nuevo.
                self.active = self.tabs.iter().position(|t| t.id == drag.tab).unwrap_or(self.active);
            }
            Drop::Into(tab) => self.merge_tab(drag, tab, None),
            Drop::Pane { tab, target, side } => self.merge_tab(drag, tab, Some((target, side))),
        }
    }

    /// Mete todos los paneles de la pestaña arrastrada en `dest`, conservando su disposición: junto a `at`, o a
    /// la derecha del panel con foco. La pestaña de origen desaparece.
    fn merge_tab(&mut self, drag: &TabDrag, dest: u64, at: Option<(u64, Side)>) {
        if !self.tab_accepts(drag.tab, dest) {
            return;
        }
        let Some(from) = self.tabs.iter().position(|t| t.id == drag.tab) else { return };
        let removed = self.tabs.remove(from);
        self.container_shells.remove(&removed.id);
        let Body::Terminal { panes: source, focused: source_focus, tunnels, .. } = removed.body else { return };
        let Some(index) = self.tabs.iter().position(|t| t.id == dest) else { return };
        let Body::Terminal { panes, focused, next_pane, zoomed, tunnels: own } = &mut self.tabs[index].body else { return };
        let (target, side) = at.filter(|(target, _)| panes.contains(*target)).unwrap_or((*focused, Side::Right));
        let mut new_focus = None;
        let mut relabel = |old: u64, pane: &mut Pane| {
            let id = *next_pane;
            *next_pane += 1;
            pane.set_id(id);
            if old == source_focus {
                new_focus = Some(id);
            }
            id
        };
        if !panes.graft(target, side, source, &mut relabel) {
            return;
        }
        if let Some(id) = new_focus {
            *focused = id;
        }
        *zoomed = None;
        // Las dos pestañas eran de la misma conexión: los túneles de la que desaparece pasan a esta si no tenía.
        if own.is_none() {
            *own = tunnels;
        }
        self.active = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tres pestañas de 100 px con 2 px entre ellas.
    fn tabs() -> Vec<(u64, Rect)> {
        (0..3u64).map(|i| (i + 1, Rect::from_min_size(Pos2::new(i as f32 * 102.0, 0.0), Vec2::new(100.0, 30.0)))).collect()
    }

    #[test]
    fn the_middle_of_a_tab_drops_into_it_and_the_sides_between_tabs() {
        let any = |_: u64| true;
        assert_eq!(strip_drop(&tabs(), 1, 150.0, any), Drop::Into(2));
        assert_eq!(strip_drop(&tabs(), 1, 110.0, any), Drop::Slot(1));
        assert_eq!(strip_drop(&tabs(), 1, 190.0, any), Drop::Slot(2));
        assert_eq!(strip_drop(&tabs(), 3, 5.0, any), Drop::Slot(0));
        assert_eq!(strip_drop(&tabs(), 1, 900.0, any), Drop::Slot(3));
    }

    #[test]
    fn a_tab_that_cannot_hold_the_dragged_one_only_orders() {
        let none = |_: u64| false;
        assert_eq!(strip_drop(&tabs(), 1, 150.0, none), Drop::Slot(1));
        assert_eq!(strip_drop(&tabs(), 1, 160.0, none), Drop::Slot(2));
        assert_eq!(strip_drop(&tabs(), 3, 120.0, none), Drop::Slot(1));
        // Sobre sí misma no se mete en sí misma.
        assert_eq!(strip_drop(&tabs(), 2, 150.0, |_| true), Drop::Slot(1));
    }

    #[test]
    fn dropping_into_a_slot_gives_the_final_position() {
        assert_eq!(reordered(2, 0), Some(0));
        assert_eq!(reordered(0, 3), Some(2));
        assert_eq!(reordered(0, 1), None);
        assert_eq!(reordered(1, 1), None);
        assert_eq!(reordered(1, 2), None);
        assert_eq!(reordered(1, 3), Some(2));
    }

    #[test]
    fn the_slot_line_sits_between_tabs() {
        assert_eq!(slot_x(&tabs(), 0), Some(0.0));
        assert_eq!(slot_x(&tabs(), 1), Some(101.0));
        assert_eq!(slot_x(&tabs(), 3), Some(304.0));
        assert_eq!(slot_x(&[], 0), None);
    }
}
