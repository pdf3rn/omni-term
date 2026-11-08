//! Guía de inicio: idioma, apariencia, almacén de credenciales e importación de hosts.

use eframe::egui::{self, RichText};

use super::*;
use crate::i18n::{t, Lang};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WelcomeStep {
    Language,
    Appearance,
    Secrets,
    Import,
}

pub struct WelcomePrompt {
    pub steps: Vec<WelcomeStep>,
    pub step: usize,
    /// Elegidos en la guía; se aplican al momento, así que el idioma y el tema cambian mientras se lee.
    pub language: Option<Lang>,
    pub theme: ThemeChoice,
    pub ansi: AnsiChoice,
    pub master: bool,
    pub keyring_available: bool,
    pub active_master: bool,
    pub import: Option<ImportPrompt>,
}

impl WelcomePrompt {
    /// El paso de importación solo está si hay hosts que ofrecer.
    pub fn new(settings: &Settings, keyring_available: bool, active_master: bool, hosts: Vec<crate::connect::sshconfig::ImportedHost>, known: &[Connection]) -> Self {
        let mut steps = vec![WelcomeStep::Language, WelcomeStep::Appearance, WelcomeStep::Secrets];
        let import = (!hosts.is_empty()).then(|| ImportPrompt::new(hosts, known));
        if import.is_some() {
            steps.push(WelcomeStep::Import);
        }
        Self {
            steps,
            step: 0,
            language: settings.language,
            theme: settings.theme,
            ansi: settings.ansi,
            master: active_master || !keyring_available,
            keyring_available,
            active_master,
            import,
        }
    }

    pub fn current(&self) -> WelcomeStep {
        self.steps[self.step.min(self.steps.len() - 1)]
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 >= self.steps.len()
    }
}

const MIN_BODY_HEIGHT: f32 = 112.0;

pub(super) fn welcome_spec(prompt: &WelcomePrompt) -> Spec {
    let accept = if !prompt.is_last() {
        t("welcome-next").to_string()
    } else {
        match prompt.import.as_ref().map(ImportPrompt::selected).filter(|n| *n > 0) {
            Some(n) => crate::tf!("welcome-finish-import", "n" => n),
            None => t("welcome-finish").to_string(),
        }
    };
    let mut spec = Spec::new(t("welcome-title"))
        .width(520.0)
        .status(crate::tf!("welcome-step", "n" => prompt.step + 1, "total" => prompt.steps.len()), theme::subtext())
        .accept(accept)
        .cancel(t("welcome-skip"));
    if prompt.step > 0 {
        spec = spec.extra(t("welcome-back"));
    }
    spec
}

pub(super) fn welcome_body(ui: &mut egui::Ui, prompt: &mut WelcomePrompt) -> bool {
    // Los pasos cortos miden lo mismo para que el diálogo no salte al avanzar.
    ui.set_min_height(MIN_BODY_HEIGHT);
    match prompt.current() {
        WelcomeStep::Language => {
            heading(ui, t("welcome-language"), t("welcome-language-hint"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut prompt.language, None, t("settings-language-auto"));
                for lang in Lang::ALL {
                    ui.selectable_value(&mut prompt.language, Some(lang), lang.native_name());
                }
            });
        }
        WelcomeStep::Appearance => {
            heading(ui, t("welcome-appearance"), t("welcome-appearance-hint"));
            ui.label(t("welcome-window-theme"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut prompt.theme, ThemeChoice::Dark, t("settings-oscuro"));
                ui.selectable_value(&mut prompt.theme, ThemeChoice::Light, t("settings-claro"));
                ui.selectable_value(&mut prompt.theme, ThemeChoice::System, t("settings-sistema"));
            });
            ui.add_space(theme::space::M);
            ui.label(t("settings-tema-del-terminal"));
            let themes = &crate::data::term_theme::THEMES;
            let current = prompt.ansi.index() as usize;
            crate::ui::kit::select(ui, "welcome-terminal-theme", 250.0, themes[current].name, |options| {
                for (index, theme) in themes.iter().enumerate() {
                    let preview = |painter: &egui::Painter, rect: egui::Rect| super::settings::paint_theme_preview(painter, rect, index as u8);
                    if options.option_decorated(index == current, theme.name, preview) {
                        prompt.ansi = AnsiChoice::new(index as u8);
                    }
                }
            });
        }
        WelcomeStep::Secrets => {
            heading(ui, t("welcome-secrets"), t("welcome-secrets-hint"));
            ui.horizontal(|ui| {
                ui.add_enabled_ui(prompt.keyring_available, |ui| {
                    ui.selectable_value(&mut prompt.master, false, t("settings-llavero-del-sistema"));
                });
                ui.selectable_value(&mut prompt.master, true, t("settings-clave-maestra"));
            });
            ui.label(RichText::new(if prompt.master { t("welcome-secrets-master") } else { t("welcome-secrets-keyring") }).small().color(theme::subtext()));
            let note = if !prompt.keyring_available {
                t("settings-este-sistema-no-tiene-llavero")
            } else if prompt.master != prompt.active_master {
                t("welcome-secrets-restart")
            } else {
                ""
            };
            if !note.is_empty() {
                ui.label(RichText::new(note).small().color(theme::subtext()));
            }
        }
        WelcomeStep::Import => {
            let found = prompt.import.as_ref().map_or(0, |i| i.rows.len());
            heading(ui, t("welcome-import"), &crate::tf!("welcome-import-hint", "n" => found));
            if let Some(import) = prompt.import.as_mut() {
                import::import_body(ui, import, false);
            }
        }
    }
    false
}

fn heading(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.label(RichText::new(title).heading());
    ui.label(RichText::new(hint).color(theme::subtext()));
    ui.add_space(theme::space::M);
}

/// Si hay que dar la bienvenida: no se hizo ya y la base está vacía (sin conexiones ni ajustes guardados).
pub fn is_first_run(done: bool, connections: usize, has_saved_settings: bool) -> bool {
    !done && connections == 0 && !has_saved_settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_empty_install_gets_the_guide() {
        assert!(is_first_run(false, 0, false));
        assert!(!is_first_run(true, 0, false));
        assert!(!is_first_run(false, 3, false));
        assert!(!is_first_run(false, 0, true));
    }

    #[test]
    fn the_import_step_appears_only_with_hosts() {
        let settings = Settings::default();
        let without = WelcomePrompt::new(&settings, true, false, Vec::new(), &[]);
        assert_eq!(without.steps, [WelcomeStep::Language, WelcomeStep::Appearance, WelcomeStep::Secrets]);
        let hosts = crate::connect::sshconfig::parse("Host a\n  HostName 1.1.1.1\n", "me");
        let with = WelcomePrompt::new(&settings, true, false, hosts, &[]);
        assert_eq!(with.steps.last(), Some(&WelcomeStep::Import));
    }

    #[test]
    fn without_a_keyring_the_master_key_is_the_only_choice() {
        let prompt = WelcomePrompt::new(&Settings::default(), false, false, Vec::new(), &[]);
        assert!(prompt.master);
    }
}
