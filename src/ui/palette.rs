//! Paleta de comandos: un buscador único para conexiones, snippets, pestañas y acciones.
//! En modo contraseñas es el selector del gestor de contraseñas (Ctrl+Alt+P).

use crate::i18n::t;
use eframe::egui::{self, Align2, CornerRadius, Frame, Id, Key, Margin, Modifiers, RichText, ScrollArea, Sense, TextEdit, Vec2};

use crate::ui::cursor::HandExt;
use crate::ui::icons;
use crate::data::store::{Connection, Snippet};
use crate::ui::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    NewConnection,
    ImportConfig,
    Settings,
    SplitRight,
    SplitDown,
    TogglePaneZoom,
    EqualizePanes,
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
    TypePassword,
    Passwords,
    Welcome,
}

pub const COMMANDS: [(Command, &str); 32] = [
    (Command::NewTab, "palette-nueva-pestana-de-terminal-local"),
    (Command::NewConnection, "dialogs-nueva-conexion"),
    (Command::ImportConfig, "palette-importar-conexiones-de-ssh-config"),
    (Command::Settings, "palette-abrir-ajustes"),
    (Command::SplitRight, "palette-dividir-el-panel-a-la"),
    (Command::SplitDown, "palette-dividir-el-panel-hacia-abajo"),
    (Command::TogglePaneZoom, "settings-ampliar-o-restaurar-el-panel"),
    (Command::EqualizePanes, "native-menu-igualar-paneles"),
    (Command::CloseTab, "palette-cerrar-la-pestana-actual"),
    (Command::ToggleSidebar, "settings-mostrar-u-ocultar-la-barra"),
    (Command::ThemeDark, "native-menu-tema-oscuro"),
    (Command::ThemeLight, "native-menu-tema-claro"),
    (Command::ThemeSystem, "native-menu-tema-del-sistema"),
    (Command::Copy, "palette-copiar-la-seleccion"),
    (Command::Paste, "pane-pegar"),
    (Command::SelectAll, "palette-seleccionar-todo-el-terminal"),
    (Command::Search, "settings-buscar-en-la-salida"),
    (Command::SaveSnippet, "palette-guardar-la-seleccion-como-snippet"),
    (Command::ZoomIn, "native-menu-aumentar-la-letra"),
    (Command::ZoomOut, "native-menu-reducir-la-letra"),
    (Command::ZoomReset, "native-menu-restablecer-la-letra"),
    (Command::NextTab, "native-menu-pestana-siguiente"),
    (Command::PrevTab, "native-menu-pestana-anterior"),
    (Command::Files, "palette-explorador-de-archivos-de-la"),
    (Command::Shortcuts, "settings-atajos-de-teclado"),
    (Command::Welcome, "welcome-menu"),
    (Command::About, "dialogs-acerca-de-omni-term"),
    (Command::Fullscreen, "settings-pantalla-completa"),
    (Command::KnownHosts, "settings-hosts-conocidos"),
    (Command::Containers, "palette-administrador-de-contenedores"),
    (Command::TypePassword, "palette-escribir-una-contrasena-guardada-e"),
    (Command::Passwords, "palette-gestionar-contrasenas"),
];

/// De dónde sale la contraseña que se escribe en el terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordSource {
    /// La guardada para una conexión (la de la pestaña activa).
    Connection(i64),
    /// Una del gestor de contraseñas.
    Saved(i64),
}

pub enum Target {
    Connect(Connection),
    Snippet(Snippet),
    Tab(usize),
    Command(Command),
    Password(PasswordSource),
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
    /// Elemento elegido; `true` si fue con Alt+Enter (en el selector de contraseñas: escribir sin enviar).
    Run(usize, bool),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    All,
    /// Contraseñas para escribir en un panel concreto (el que tenía el foco al abrirla).
    Passwords { tab: u64, pane: u64 },
}

#[derive(Default)]
pub struct Palette {
    query: String,
    selected: usize,
    pub mode: Mode,
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
    /// Selector de contraseñas para escribir en el panel `pane` de la pestaña `tab`.
    pub fn passwords(tab: u64, pane: u64) -> Self {
        Self { mode: Mode::Passwords { tab, pane }, ..Self::default() }
    }

    pub fn show(&mut self, ctx: &egui::Context, items: &[Item]) -> Outcome {
        let matches = filter(items, &self.query);
        self.selected = self.selected.min(matches.len().saturating_sub(1));

        let mut moved = false;
        let (down, up, enter, alt_enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::ALT, Key::Enter),
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
        if enter || alt_enter {
            if let Some(index) = matches.get(self.selected) {
                outcome = Outcome::Run(*index, alt_enter);
            }
        }
        let passwords = matches!(self.mode, Mode::Passwords { .. });

        let id = Id::NULL.with("command-palette");
        let area = egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 90.0));
        let frame = Frame::popup(&ctx.global_style()).inner_margin(Margin::same(14));
        let response = egui::Modal::new(id).area(area).frame(frame).show(ctx, |ui| {
            ui.set_width(540.0);
            let field = ui.add(
                TextEdit::singleline(&mut self.query)
                    .hint_text(if passwords { t("palette-buscar-contrasenas") } else { t("palette-buscar-conexiones-snippets-pestana") })
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            if field.changed() {
                self.selected = 0;
            }
            ui.add_space(theme::space::M);
            if matches.is_empty() {
                ui.label(RichText::new(t("palette-nada-coincide-con-la-busqueda")).color(theme::subtext()));
            }
            let list_height = if passwords { 300.0 } else { 340.0 };
            ScrollArea::vertical().max_height(list_height).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (position, index) in matches.iter().enumerate() {
                    let item = &items[*index];
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
                    let response = response.hand();
                    if response.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                        self.selected = position;
                    }
                    if position == self.selected {
                        ui.painter().rect_filled(rect, CornerRadius::same(theme::radius::CONTROL), theme::selection());
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
                        Target::Password(_) => icons::LOCK,
                    };
                    painter.text(rect.min + Vec2::new(16.0, 18.0), Align2::CENTER_CENTER, glyph, egui::FontId::proportional(theme::size::ICON), theme::subtext());
                    let title = painter.text(
                        rect.min + Vec2::new(34.0, 18.0),
                        Align2::LEFT_CENTER,
                        &item.title,
                        egui::FontId::new(theme::size::BODY, crate::ui::strong_family()),
                        theme::text(),
                    );
                    if !item.detail.is_empty() {
                        painter.with_clip_rect(rect.shrink2(Vec2::new(90.0, 0.0))).text(
                            egui::pos2(title.max.x + 10.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            &item.detail,
                            egui::FontId::proportional(theme::size::SMALL),
                            theme::subtext(),
                        );
                    }
                    painter.text(rect.max - Vec2::new(10.0, 18.0), Align2::RIGHT_CENTER, item.kind, egui::FontId::proportional(theme::size::SMALL), theme::subtext());
                    if response.clicked() {
                        outcome = Outcome::Run(*index, ui.input(|i| i.modifiers.alt));
                    }
                }
            });
            if passwords {
                ui.add_space(theme::space::S);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::space::XS;
                    crate::ui::kit::keycap(ui, "Enter");
                    ui.label(RichText::new(t("palette-escribir-y-enviar")).small().color(theme::subtext()));
                    ui.add_space(theme::space::M);
                    crate::ui::kit::keycap(ui, "Alt+Enter");
                    ui.label(RichText::new(t("palette-solo-escribir")).small().color(theme::subtext()));
                });
            }
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
