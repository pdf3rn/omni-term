//! Paleta de comandos: un buscador único para conexiones, snippets, pestañas y acciones.

use eframe::egui::{self, Align2, CornerRadius, Frame, Id, Key, Margin, Modifiers, RichText, ScrollArea, Sense, TextEdit, Vec2};

use crate::cursor::HandExt;
use crate::icons;
use crate::store::{Connection, Snippet};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    NewConnection,
    ImportConfig,
    Settings,
    SplitRight,
    SplitDown,
    CloseTab,
    ToggleSidebar,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
    Copy,
    Paste,
    SelectAll,
    Search,
    SaveSnippet,
    Quit,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    NextTab,
    PrevTab,
    Files,
    Shortcuts,
    About,
    Palette,
    Fullscreen,
    KnownHosts,
    Containers,
}

pub const COMMANDS: [(Command, &str); 27] = [
    (Command::NewTab, "Nueva pestaña de terminal local"),
    (Command::NewConnection, "Nueva conexión"),
    (Command::ImportConfig, "Importar conexiones de ~/.ssh/config"),
    (Command::Settings, "Abrir ajustes"),
    (Command::SplitRight, "Dividir el panel a la derecha"),
    (Command::SplitDown, "Dividir el panel hacia abajo"),
    (Command::CloseTab, "Cerrar la pestaña actual"),
    (Command::ToggleSidebar, "Mostrar u ocultar la barra lateral"),
    (Command::ThemeDark, "Tema oscuro"),
    (Command::ThemeLight, "Tema claro"),
    (Command::ThemeSystem, "Tema del sistema"),
    (Command::Copy, "Copiar la selección"),
    (Command::Paste, "Pegar"),
    (Command::SelectAll, "Seleccionar todo el terminal"),
    (Command::Search, "Buscar en la salida"),
    (Command::SaveSnippet, "Guardar la selección como snippet"),
    (Command::ZoomIn, "Aumentar la letra"),
    (Command::ZoomOut, "Reducir la letra"),
    (Command::ZoomReset, "Restablecer la letra"),
    (Command::NextTab, "Pestaña siguiente"),
    (Command::PrevTab, "Pestaña anterior"),
    (Command::Files, "Explorador de archivos de la conexión actual"),
    (Command::Shortcuts, "Atajos de teclado"),
    (Command::About, "Acerca de omni-term"),
    (Command::Fullscreen, "Pantalla completa"),
    (Command::KnownHosts, "Hosts conocidos"),
    (Command::Containers, "Administrador de contenedores"),
];

pub enum Target {
    Connect(Connection),
    Snippet(Snippet),
    Tab(usize),
    Command(Command),
}

pub struct Item {
    pub title: String,
    pub detail: String,
    pub kind: &'static str,
    pub target: Target,
}

pub enum Outcome {
    Open,
    Close,
    Run(usize),
}

#[derive(Default)]
pub struct Palette {
    query: String,
    selected: usize,
}

/// Elementos que cumplen todos los términos de la búsqueda; los que empiezan por ella, primero.
pub fn filter(items: &[Item], query: &str) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut matches: Vec<(bool, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let haystack = format!("{} {} {}", item.title, item.detail, item.kind).to_lowercase();
            terms.iter().all(|t| haystack.contains(t)).then(|| {
                let first = terms.first().is_some_and(|t| item.title.to_lowercase().starts_with(t.as_str()));
                (!first, index)
            })
        })
        .collect();
    matches.sort();
    matches.into_iter().map(|(_, index)| index).collect()
}

impl Palette {
    pub fn show(&mut self, ctx: &egui::Context, items: &[Item]) -> Outcome {
        let matches = filter(items, &self.query);
        self.selected = self.selected.min(matches.len().saturating_sub(1));

        let mut moved = false;
        let (down, up, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if down && !matches.is_empty() {
            self.selected = (self.selected + 1) % matches.len();
            moved = true;
        }
        if up && !matches.is_empty() {
            self.selected = (self.selected + matches.len() - 1) % matches.len();
            moved = true;
        }
        let mut outcome = Outcome::Open;
        if escape {
            outcome = Outcome::Close;
        }
        if enter {
            if let Some(index) = matches.get(self.selected) {
                outcome = Outcome::Run(*index);
            }
        }

        let id = Id::NULL.with("command-palette");
        let area = egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 90.0));
        let frame = Frame::popup(&ctx.global_style()).inner_margin(Margin::same(14));
        let response = egui::Modal::new(id).area(area).frame(frame).show(ctx, |ui| {
            ui.set_width(540.0);
            let field = ui.add(
                TextEdit::singleline(&mut self.query)
                    .hint_text("Buscar conexiones, snippets, pestañas y acciones")
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            if field.changed() {
                self.selected = 0;
            }
            ui.add_space(8.0);
            if matches.is_empty() {
                ui.label(RichText::new("Nada coincide con la búsqueda.").color(theme::subtext()));
            }
            ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (position, index) in matches.iter().enumerate() {
                    let item = &items[*index];
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
                    let response = response.hand();
                    if response.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                        self.selected = position;
                    }
                    if position == self.selected {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::selection());
                        if moved {
                            ui.scroll_to_rect(rect, None);
                        }
                    }
                    let painter = ui.painter();
                    let glyph = match item.target {
                        Target::Connect(_) => icons::ARROW_RIGHT,
                        Target::Snippet(_) => icons::PENCIL,
                        Target::Tab(_) => icons::FOLDER_CLOSED,
                        Target::Command(_) => icons::GEAR,
                    };
                    painter.text(rect.min + Vec2::new(16.0, 18.0), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(15.0), theme::subtext());
                    let title = painter.text(
                        rect.min + Vec2::new(34.0, 18.0),
                        Align2::LEFT_CENTER,
                        &item.title,
                        egui::FontId::new(14.0, crate::ui::strong_family()),
                        theme::text(),
                    );
                    if !item.detail.is_empty() {
                        painter.with_clip_rect(rect.shrink2(Vec2::new(90.0, 0.0))).text(
                            egui::pos2(title.max.x + 10.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            &item.detail,
                            egui::FontId::proportional(12.0),
                            theme::subtext(),
                        );
                    }
                    painter.text(rect.max - Vec2::new(10.0, 18.0), Align2::RIGHT_CENTER, item.kind, egui::FontId::proportional(12.0), theme::subtext());
                    if response.clicked() {
                        outcome = Outcome::Run(*index);
                    }
                }
            });
        });
        if matches!(outcome, Outcome::Open) && response.backdrop_response.clicked() {
            outcome = Outcome::Close;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(title: &str, detail: &str) -> Item {
        Item { title: title.into(), detail: detail.into(), kind: "Acción", target: Target::Command(Command::NewTab) }
    }

    #[test]
    fn every_term_must_match_and_prefix_matches_come_first() {
        let items = vec![command("Cerrar la pestaña", ""), command("Nueva pestaña", ""), command("Pestaña web", "deploy@10.0.0.1")];
        assert_eq!(filter(&items, ""), vec![0, 1, 2]);
        assert_eq!(filter(&items, "pest"), vec![2, 0, 1]);
        assert_eq!(filter(&items, "web deploy"), vec![2]);
        assert!(filter(&items, "zzz").is_empty());
    }
}
