//! Arrastrar un panel por su asa: a otro sitio de la división, a otra pestaña del mismo tipo o a una
//! pestaña nueva. La sesión se mueve viva; Esc, soltar en un sitio no válido o fuera de la ventana cancela.

use std::time::Instant;

use eframe::egui::{Id, LayerId, Order};

use super::*;

/// Tiempo sobre una pestaña antes de que se active para soltar en sus paneles.
const HOVER_ACTIVATE: Duration = Duration::from_millis(500);
/// Fracción de cada borde de un panel que lo divide por ese lado; el centro intercambia.
const EDGE_ZONE: f32 = 0.25;
/// Fracción de cada borde de una pestaña que cuenta como «entre pestañas».
const TAB_EDGE: f32 = 0.2;

pub struct PaneDrag {
    pub tab: u64,
    pub pane: u64,
    kind: TabKind,
    /// Pestaña bajo el puntero y desde cuándo, para activarla al rato.
    hover: Option<(u64, Instant)>,
}

pub struct DropTargets {
    /// Pestaña de los paneles visibles y la zona de cada uno.
    pub panes: Option<(u64, Vec<(u64, Rect)>)>,
    pub tab_bar: Rect,
    pub tabs: Vec<(u64, Rect)>,
}

impl Default for DropTargets {
    fn default() -> Self {
        Self { panes: None, tab_bar: Rect::NOTHING, tabs: Vec::new() }
    }
}

/// Zona de un panel destino.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Edge(Side),
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drop {
    Pane { tab: u64, target: u64, zone: Zone },
    /// Sobre la pestaña: a la derecha de su panel con foco.
    Tab(u64),
    /// Una pestaña nueva en esta posición.
    NewTab(usize),
    /// Un destino que no admite el panel (otro tipo de pestaña).
    Refused,
}

/// En qué zona de `rect` cae `pointer`: el borde más cercano si está a menos de `EDGE_ZONE`, si no el centro.
pub fn zone(rect: Rect, pointer: Pos2) -> Zone {
    let x = ((pointer.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    let y = ((pointer.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
    let edges = [(x, Side::Left), (1.0 - x, Side::Right), (y, Side::Up), (1.0 - y, Side::Down)];
    let (distance, side) = edges.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).expect("cuatro bordes");
    if distance < EDGE_ZONE { Zone::Edge(side) } else { Zone::Center }
}

/// Dónde quedará el panel al soltarlo en esa zona.
pub fn preview(rect: Rect, zone: Zone) -> Rect {
    let half = rect.size() / 2.0;
    match zone {
        Zone::Center => rect,
        Zone::Edge(Side::Left) => Rect::from_min_size(rect.min, Vec2::new(half.x, rect.height())),
        Zone::Edge(Side::Right) => Rect::from_min_max(Pos2::new(rect.min.x + half.x, rect.min.y), rect.max),
        Zone::Edge(Side::Up) => Rect::from_min_size(rect.min, Vec2::new(rect.width(), half.y)),
        Zone::Edge(Side::Down) => Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + half.y), rect.max),
    }
}

impl App {
    /// Si un panel de la pestaña `tab` (de tipo `kind`) se puede soltar en `other`.
    fn accepts(&self, kind: TabKind, other: u64) -> bool {
        self.tabs.iter().any(|t| t.id == other && t.kind == kind && matches!(t.body, Body::Terminal { .. }))
    }

    /// Si el panel de una pestaña de este tipo se puede arrastrar: hace falta otro sitio adonde llevarlo.
    pub(super) fn can_drag_pane(&self, tab: u64, panes: usize) -> bool {
        let Some(kind) = self.tabs.iter().find(|t| t.id == tab).map(|t| t.kind) else { return false };
        // Las pestañas de contenedor recuerdan su contenedor por pestaña: no se mezclan.
        if !matches!(kind, TabKind::Local | TabKind::Connection(_)) {
            return false;
        }
        panes > 1 || self.tabs.iter().any(|t| t.id != tab && t.kind == kind && matches!(t.body, Body::Terminal { .. }))
    }

    pub(super) fn start_pane_drag(&mut self, tab: u64, pane: u64) {
        if let Some(kind) = self.tabs.iter().find(|t| t.id == tab).map(|t| t.kind) {
            self.pane_drag = Some(PaneDrag { tab, pane, kind, hover: None });
        }
    }

    fn source_panes(&self, drag: &PaneDrag) -> Option<usize> {
        self.tabs.iter().find(|t| t.id == drag.tab).and_then(|t| match &t.body {
            Body::Terminal { panes, .. } if panes.contains(drag.pane) => Some(panes.len()),
            _ => None,
        })
    }

    /// Destino bajo el puntero y el rectángulo que lo señala.
    fn drop_at(&self, drag: &PaneDrag, pointer: Pos2, panes: usize) -> Option<(Drop, Rect)> {
        let targets = &self.drop_targets;
        if targets.tab_bar.contains(pointer) {
            let bar = targets.tab_bar.y_range();
            let line = |x: f32| Rect::from_x_y_ranges(x - theme::stroke::THICK..=x + theme::stroke::THICK, bar);
            // Ascender a pestaña solo tiene sentido si la pestaña de origen conserva otros paneles.
            let new_tab = |index: usize, x: f32| (panes > 1).then_some((Drop::NewTab(index), line(x)));
            for (index, (tab, rect)) in targets.tabs.iter().enumerate() {
                if !rect.x_range().contains(pointer.x) {
                    continue;
                }
                let edge = rect.width() * TAB_EDGE;
                if pointer.x < rect.min.x + edge {
                    return new_tab(index, rect.min.x);
                }
                if pointer.x > rect.max.x - edge {
                    return new_tab(index + 1, rect.max.x);
                }
                if *tab == drag.tab {
                    return None;
                }
                let drop = if self.accepts(drag.kind, *tab) { Drop::Tab(*tab) } else { Drop::Refused };
                return Some((drop, *rect));
            }
            let last = targets.tabs.last().map_or(targets.tab_bar.min.x, |(_, rect)| rect.max.x);
            return if pointer.x > last { new_tab(targets.tabs.len(), last) } else { None };
        }
        let (tab, leaves) = targets.panes.as_ref()?;
        let (target, rect) = leaves.iter().find(|(_, rect)| rect.contains(pointer))?;
        if *tab == drag.tab && *target == drag.pane {
            return None;
        }
        if *tab != drag.tab && !self.accepts(drag.kind, *tab) {
            return Some((Drop::Refused, *rect));
        }
        let zone = zone(*rect, pointer);
        Some((Drop::Pane { tab: *tab, target: *target, zone }, preview(*rect, zone)))
    }

    /// Sigue el arrastre de un panel: pinta el destino, activa pestañas al pasar por encima y aplica el
    /// movimiento al soltar.
    pub(super) fn drive_pane_drag(&mut self, ctx: &egui::Context) {
        let Some(mut drag) = self.pane_drag.take() else { return };
        let Some(panes) = self.source_panes(&drag) else { return };
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return;
        }
        let (down, pointer) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.hover_pos()));
        let target = pointer.and_then(|p| self.drop_at(&drag, p, panes));
        if !down {
            match target {
                Some((Drop::Refused, _)) | None => {}
                Some((drop, _)) => self.apply_drop(&drag, drop),
            }
            return;
        }

        // Pasar un rato sobre otra pestaña la activa, para soltar en uno de sus paneles.
        match target {
            Some((Drop::Tab(tab), _)) => match drag.hover {
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
            _ => drag.hover = None,
        }

        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::NULL.with("pane-drop")));
        match target {
            Some((Drop::Refused, _)) => ctx.set_cursor_icon(egui::CursorIcon::NotAllowed),
            Some((_, rect)) => {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                painter.rect(
                    rect,
                    CornerRadius::same(theme::radius::SMALL),
                    theme::accent().gamma_multiply(0.22),
                    Stroke::new(theme::stroke::MEDIUM, theme::accent()),
                    egui::StrokeKind::Inside,
                );
            }
            None => ctx.set_cursor_icon(egui::CursorIcon::Grabbing),
        }
        self.pane_drag = Some(drag);
    }

    /// Saca un panel de su pestaña; si era el único, la pestaña se cierra y entrega también sus túneles.
    fn take_pane(&mut self, tab: u64, pane: u64) -> Option<(Pane, Option<Tunnels>)> {
        let index = self.tabs.iter().position(|t| t.id == tab)?;
        let Body::Terminal { panes, focused, zoomed, .. } = &mut self.tabs[index].body else { return None };
        if panes.len() > 1 {
            let (taken, heir) = panes.take(pane)?;
            if *focused == pane {
                *focused = heir;
            }
            *zoomed = None;
            return Some((taken, None));
        }
        let removed = self.tabs.remove(index);
        self.container_shells.remove(&removed.id);
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        match removed.body {
            Body::Terminal { panes, tunnels, .. } => panes.into_leaf().map(|pane| (pane, tunnels)),
            _ => None,
        }
    }

    fn apply_drop(&mut self, drag: &PaneDrag, drop: Drop) {
        self.renaming = None;
        match drop {
            Drop::Refused => {}
            Drop::Pane { tab, target, zone } if tab == drag.tab => {
                let Some(Body::Terminal { panes, focused, zoomed, .. }) = self.tabs.iter_mut().find(|t| t.id == tab).map(|t| &mut t.body) else { return };
                match zone {
                    Zone::Center => {
                        panes.swap(drag.pane, target);
                    }
                    Zone::Edge(side) => {
                        if let Some((pane, _)) = panes.take(drag.pane) {
                            panes.insert(target, side, drag.pane, pane);
                        }
                    }
                }
                *focused = drag.pane;
                *zoomed = None;
            }
            Drop::Pane { tab, target, zone: Zone::Center } => self.swap_across(drag, tab, target),
            Drop::Pane { tab, target, zone: Zone::Edge(side) } => self.move_into(drag, tab, Some((target, side))),
            Drop::Tab(tab) => self.move_into(drag, tab, None),
            Drop::NewTab(index) => {
                let label = self.tabs.iter().find(|t| t.id == drag.tab).map(|t| t.label.clone()).unwrap_or_default();
                let Some((mut pane, _)) = self.take_pane(drag.tab, drag.pane) else { return };
                pane.set_id(1);
                let id = self.take_tab_id();
                let index = index.min(self.tabs.len());
                self.tabs.insert(index, Tab { id, kind: drag.kind, label, custom: None, attention: false, body: Body::terminal(pane, None) });
                self.active = index;
            }
        }
    }

    /// Lleva el panel a otra pestaña: junto a `target` por `side`, o a la derecha de su panel con foco.
    fn move_into(&mut self, drag: &PaneDrag, tab: u64, at: Option<(u64, Side)>) {
        if !self.accepts(drag.kind, tab) {
            return;
        }
        let Some((mut pane, tunnels)) = self.take_pane(drag.tab, drag.pane) else { return };
        let Some(index) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let Body::Terminal { panes, focused, next_pane, zoomed, tunnels: own } = &mut self.tabs[index].body else { return };
        let (target, side) = at.filter(|(target, _)| panes.contains(*target)).unwrap_or((*focused, Side::Right));
        let id = *next_pane;
        *next_pane += 1;
        pane.set_id(id);
        panes.insert(target, side, id, pane);
        *focused = id;
        *zoomed = None;
        // Si la pestaña de origen se cerró, sus túneles siguen vivos aquí (la misma conexión).
        if own.is_none() {
            *own = tunnels;
        }
        self.active = index;
    }

    /// Intercambia un panel con otro de otra pestaña; cada uno ocupa el sitio del otro.
    fn swap_across(&mut self, drag: &PaneDrag, tab: u64, target: u64) {
        if !self.accepts(drag.kind, tab) {
            return;
        }
        let (Some(from), Some(to)) = (self.tabs.iter().position(|t| t.id == drag.tab), self.tabs.iter().position(|t| t.id == tab)) else { return };
        let (low, high) = (from.min(to), from.max(to));
        let (head, tail) = self.tabs.split_at_mut(high);
        let (a, b) = (&mut head[low], &mut tail[0]);
        let (source, destination) = if from < to { (a, b) } else { (b, a) };
        let (Body::Terminal { panes: src, zoomed: src_zoom, .. }, Body::Terminal { panes: dst, focused, zoomed: dst_zoom, .. }) = (&mut source.body, &mut destination.body) else { return };
        let (Some(moving), Some(other)) = (src.get_mut(drag.pane), dst.get_mut(target)) else { return };
        std::mem::swap(moving, other);
        // Cada panel toma el id de la hoja en la que ha quedado.
        moving.set_id(drag.pane);
        other.set_id(target);
        *focused = target;
        *src_zoom = None;
        *dst_zoom = None;
        self.active = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))
    }

    #[test]
    fn the_closest_edge_wins_and_the_middle_swaps() {
        assert_eq!(zone(rect(), Pos2::new(20.0, 100.0)), Zone::Edge(Side::Left));
        assert_eq!(zone(rect(), Pos2::new(390.0, 100.0)), Zone::Edge(Side::Right));
        assert_eq!(zone(rect(), Pos2::new(200.0, 10.0)), Zone::Edge(Side::Up));
        assert_eq!(zone(rect(), Pos2::new(200.0, 190.0)), Zone::Edge(Side::Down));
        assert_eq!(zone(rect(), Pos2::new(200.0, 100.0)), Zone::Center);
    }

    #[test]
    fn the_preview_is_the_half_the_pane_will_take() {
        assert_eq!(preview(rect(), Zone::Edge(Side::Left)).width(), 200.0);
        assert_eq!(preview(rect(), Zone::Edge(Side::Down)).min.y, 100.0);
        assert_eq!(preview(rect(), Zone::Center), rect());
    }
}
