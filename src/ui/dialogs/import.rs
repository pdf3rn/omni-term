//! Importación de hosts de `~/.ssh/config`: el usuario ve qué entra y elige.

use eframe::egui::{self, RichText};

use super::*;
use crate::connect::sshconfig::ImportedHost;
use crate::i18n::t;

pub struct ImportRow {
    pub host: ImportedHost,
    pub checked: bool,
    /// Ya hay una conexión igual: llega desmarcada para no duplicarla.
    pub exists: bool,
}

pub struct ImportPrompt {
    pub rows: Vec<ImportRow>,
}

impl ImportPrompt {
    /// Filas para `hosts`; los que ya existen entre `known` van desmarcados.
    pub fn new(hosts: Vec<ImportedHost>, known: &[Connection]) -> Self {
        let rows = hosts
            .into_iter()
            .map(|host| {
                let c = &host.connection;
                let exists = known.iter().any(|k| k.name == c.name || (k.is_ssh() && k.host == c.host && k.port == c.port && k.user == c.user));
                ImportRow { host, checked: !exists, exists }
            })
            .collect();
        Self { rows }
    }

    pub fn selected(&self) -> usize {
        self.rows.iter().filter(|r| r.checked).count()
    }
}

/// `count` muestra «N hosts encontrados»; la guía ya lo dice en su texto.
pub(super) fn import_body(ui: &mut egui::Ui, prompt: &mut ImportPrompt, count: bool) -> bool {
    ui.horizontal(|ui| {
        if count {
            ui.label(RichText::new(crate::tf!("import-found", "n" => prompt.rows.len())).color(theme::subtext()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let all = prompt.selected() == prompt.rows.len();
            if ui.add(egui::Button::new(RichText::new(if all { t("import-none-selected") } else { t("import-select-all") }).color(theme::accent())).frame(false)).hand().clicked() {
                prompt.rows.iter_mut().for_each(|r| r.checked = !all);
            }
        });
    });
    ui.add_space(theme::space::XS);
    egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
        for row in &mut prompt.rows {
            let c = &row.host.connection;
            let detail = format!("{}@{}:{}", c.user, c.host, c.port);
            ui.horizontal(|ui| {
                crate::ui::kit::toggle(ui, &mut row.checked);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(&c.name).strong());
                    ui.label(RichText::new(detail).small().color(theme::subtext()));
                });
                if row.exists {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(t("import-exists")).small().color(theme::subtext()));
                    });
                }
            });
            ui.add_space(theme::space::XS);
        }
    });
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "Host web\n  HostName 10.0.0.5\n  User deploy\nHost db\n  HostName 10.0.0.6\n  User admin\n";

    #[test]
    fn hosts_already_saved_arrive_unchecked() {
        let known = vec![crate::connect::sshconfig::parse(CONFIG, "me").remove(0).connection];
        let prompt = ImportPrompt::new(crate::connect::sshconfig::parse(CONFIG, "me"), &known);
        assert!(prompt.rows[0].exists && !prompt.rows[0].checked);
        assert!(!prompt.rows[1].exists && prompt.rows[1].checked);
        assert_eq!(prompt.selected(), 1);
    }

    #[test]
    fn a_host_with_the_same_address_counts_as_existing_even_under_another_name() {
        let mut renamed = crate::connect::sshconfig::parse(CONFIG, "me").remove(0).connection;
        renamed.name = "otro".into();
        let prompt = ImportPrompt::new(crate::connect::sshconfig::parse(CONFIG, "me"), &[renamed]);
        assert!(prompt.rows[0].exists);
    }
}
