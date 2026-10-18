//! Cursor de mano sobre los elementos pulsables que egui no marca por sí mismo.

use eframe::egui::{CursorIcon, Response};

pub trait HandExt {
    /// Muestra el cursor de mano mientras el puntero está encima.
    fn hand(self) -> Self;
    /// Cursor de agarre para elementos que se arrastran.
    fn grab(self) -> Self;
}

impl HandExt for Response {
    fn hand(self) -> Self {
        if self.hovered() {
            self.ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        self
    }

    fn grab(self) -> Self {
        if self.dragged() {
            self.ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if self.hovered() {
            self.ctx.set_cursor_icon(CursorIcon::Grab);
        }
        self
    }
}
