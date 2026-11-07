//! Desplegable de una opción con teclado completo. Sustituye a `egui::ComboBox`, cuyas opciones no cierran la
//! lista al elegirlas con Intro o Espacio.
//!
//! Cerrado: Intro, Espacio y ↓ abren. Abierto: ↑/↓ mueven la marca (arranca en el valor actual), Inicio/Fin saltan
//! a los extremos, una letra salta a la siguiente opción que empieza por ella, Intro o Espacio eligen y cierran,
//! Esc cierra sin cambiar y Tab cierra y sigue al campo siguiente. Al cerrar con teclado el foco vuelve al desplegable.

use std::hash::Hash;

use eframe::egui::{self, Button, ComboBox, EventFilter, FocusDirection, Id, Key, Modifiers, Popup, Response, Ui};

use crate::ui::cursor::HandExt;

/// Opciones de un desplegable; se dibujan dentro de la lista con [`select`].
pub struct SelectOptions<'u> {
    ui: &'u mut Ui,
    items: Vec<(Response, String)>,
    selected: Option<usize>,
    seen: Id,
    tab: Option<bool>,
}

impl SelectOptions<'_> {
    /// Una opción; devuelve `true` cuando se elige (con ratón o teclado) y cierra la lista.
    pub fn option(&mut self, selected: bool, label: &str) -> bool {
        self.option_enabled(selected, label, true)
    }

    /// Como `option`, atenuada y sin poder elegirse si `enabled` es `false`.
    pub fn option_enabled(&mut self, selected: bool, label: &str, enabled: bool) -> bool {
        let response = self.ui.add_enabled(enabled, Button::selectable(selected, label)).hand();
        if selected {
            self.selected = Some(self.items.len());
        }
        let clicked = enabled && response.clicked();
        if enabled {
            self.items.push((response, label.to_string()));
        }
        if clicked {
            self.ui.close();
        }
        clicked
    }

    /// Opción que fija `current` a `value`, como `ui.selectable_value`.
    pub fn value<T: PartialEq>(&mut self, current: &mut T, value: T, label: &str) {
        if self.option(*current == value, label) {
            *current = value;
        }
    }

    /// Teclado de la lista abierta, una vez dibujadas todas las opciones.
    fn finish(&mut self) {
        let ctx = self.ui.ctx().clone();
        let count = self.items.len();
        if count == 0 {
            return;
        }
        // Al abrir, la marca va al valor actual.
        if ctx.data(|d| d.get_temp::<bool>(self.seen)).is_none() {
            ctx.data_mut(|d| d.insert_temp(self.seen, true));
            ctx.memory_mut(|m| m.request_focus(self.items[self.selected.unwrap_or(0)].0.id));
            return;
        }
        let Some(current) = self.items.iter().position(|(r, _)| ctx.memory(|m| m.has_focus(r.id))) else { return };
        // Las flechas y Tab son de la lista mientras tiene el foco: si no, egui los usa para saltar de widget.
        ctx.memory_mut(|m| m.set_focus_lock_filter(self.items[current].0.id, EventFilter { tab: true, vertical_arrows: true, ..Default::default() }));
        let mut target = None;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                target = Some((current + 1).min(count - 1));
            } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                target = Some(current.saturating_sub(1));
            } else if i.consume_key(Modifiers::NONE, Key::Home) {
                target = Some(0);
            } else if i.consume_key(Modifiers::NONE, Key::End) {
                target = Some(count - 1);
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                self.tab = Some(true);
            } else if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                self.tab = Some(false);
            }
        });
        if target.is_none() {
            let typed = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Text(t) = e { t.chars().next().filter(|c| !c.is_whitespace()) } else { None }));
            if let Some(letter) = typed {
                let letter = letter.to_lowercase().next().unwrap_or(letter);
                target = (1..=count).map(|step| (current + step) % count).find(|&k| self.items[k].1.to_lowercase().starts_with(letter));
            }
        }
        if let Some(index) = target {
            let response = &self.items[index].0;
            response.request_focus();
            response.scroll_to_me(None);
        }
        if self.tab.is_some() {
            self.ui.close();
        }
    }
}

/// Desplegable con `text` como valor mostrado. Devuelve la respuesta del botón.
pub fn select(ui: &mut Ui, salt: impl Hash + std::fmt::Debug + Copy, width: f32, text: impl Into<String>, options: impl FnOnce(&mut SelectOptions)) -> Response {
    let ctx = ui.ctx().clone();
    let seen = ui.make_persistent_id(("select-seen", salt));
    let was_open_id = ui.make_persistent_id(("select-open", salt));
    let was_open = ctx.data(|d| d.get_temp::<bool>(was_open_id)).unwrap_or(false);
    let mut tab = None;
    let inner = ComboBox::from_id_salt(salt).width(width).selected_text(text.into()).show_ui(ui, |ui| {
        let mut opts = SelectOptions { ui, items: Vec::new(), selected: None, seen, tab: None };
        options(&mut opts);
        opts.finish();
        tab = opts.tab;
    });
    let response = inner.response.hand();
    let popup_id = response.id.with("popup");
    let open = Popup::is_id_open(&ctx, popup_id);
    ctx.data_mut(|d| d.insert_temp(was_open_id, open));
    if !open {
        ctx.data_mut(|d| d.remove::<bool>(seen));
        if response.has_focus() {
            ctx.memory_mut(|m| m.set_focus_lock_filter(response.id, EventFilter { vertical_arrows: true, ..Default::default() }));
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                Popup::open_id(&ctx, popup_id);
            }
        }
    }
    // Cerrada con teclado (elegir, Esc o Tab), el foco vuelve al desplegable; con el ratón se queda donde se pulsó.
    if was_open && !open && (tab.is_some() || ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) || i.key_pressed(Key::Escape))) {
        response.request_focus();
        if let Some(forward) = tab {
            ctx.memory_mut(|m| m.move_focus(if forward { FocusDirection::Next } else { FocusDirection::Previous }));
        }
    }
    response
}

/// Para los menús que se abren desde un botón: al cerrarse con teclado, el foco vuelve al botón.
pub fn return_focus_after_menu(ctx: &egui::Context, button: &Response) {
    let was_open_id = button.id.with("menu-was-open");
    let was_open = ctx.data(|d| d.get_temp::<bool>(was_open_id)).unwrap_or(false);
    let open = Popup::is_id_open(ctx, Popup::default_response_id(button));
    ctx.data_mut(|d| d.insert_temp(was_open_id, open));
    if was_open && !open && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) || i.key_pressed(Key::Escape)) {
        button.request_focus();
    }
}
