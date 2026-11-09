//! Diálogo de actualización: aviso, descarga, error y versión lista para reiniciar.

use eframe::egui::{self, RichText, ScrollArea};

use super::*;
use crate::update::{Manual, Plan, Release};

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStage {
    Offer,
    Downloading { done: u64, total: u64 },
    Installing,
    Failed(String),
    /// Instalada; `tabs` son las pestañas que cerrará el reinicio.
    Ready { tabs: usize, installer: bool },
}

pub struct UpdatePrompt {
    pub release: Release,
    /// Qué se descarga, o por qué esta copia solo puede abrir la página.
    pub plan: Result<Plan, Manual>,
    pub stage: UpdateStage,
}

pub(super) fn update_spec(prompt: &UpdatePrompt) -> Spec {
    let spec = |title: &str| Spec::new(title).width(480.0);
    match &prompt.stage {
        UpdateStage::Offer => {
            let accept = if prompt.plan.is_ok() { t("update-install") } else { t("update-download") };
            spec(t("update-title")).icon(DialogIcon::Info).accept(accept).cancel(t("update-later")).extra(t("update-skip"))
        }
        UpdateStage::Downloading { .. } => spec(t("update-downloading-title")).cancel(t("update-hide")).extra(t("update-cancel-download")),
        UpdateStage::Installing => spec(t("update-downloading-title")).cancel(t("update-hide")),
        UpdateStage::Failed(error) => spec(t("update-failed-title")).icon(DialogIcon::Warning).accept(t("update-retry")).cancel(t("pane-cerrar")).status(error.clone(), theme::red()),
        UpdateStage::Ready { .. } => spec(t("update-ready-title")).icon(DialogIcon::Info).accept(t("update-restart-now")).cancel(t("update-on-close")),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

pub(super) fn update_body(ui: &mut egui::Ui, prompt: &UpdatePrompt) -> bool {
    let version = prompt.release.version.clone();
    match &prompt.stage {
        UpdateStage::Offer | UpdateStage::Failed(_) => {
            ui.label(crate::tf!("update-body", "version" => version, "current" => crate::update::current()));
            if let Err(reason) = prompt.plan {
                ui.label(RichText::new(reason.reason()).color(theme::subtext()));
            }
            let notes = crate::update::plain_notes(&prompt.release.notes);
            if !notes.is_empty() {
                ui.add_space(theme::space::XS);
                ui.label(RichText::new(t("update-notes")).small().color(theme::subtext()));
                egui::Frame::new().fill(theme::surface()).corner_radius(theme::radius::SMALL).inner_margin(theme::space::S).show(ui, |ui| {
                    ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(notes);
                    });
                });
            }
        }
        UpdateStage::Downloading { done, total } => {
            ui.label(crate::tf!("update-downloading", "version" => version));
            let fraction = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(fraction).desired_height(theme::space::S));
            ui.label(RichText::new(crate::tf!("update-size", "done" => megabytes(*done), "total" => megabytes(*total))).small().color(theme::subtext()));
        }
        UpdateStage::Installing => {
            ui.label(t("update-installing"));
            ui.add(egui::ProgressBar::new(1.0).desired_height(theme::space::S).animate(true));
        }
        UpdateStage::Ready { tabs, installer } => {
            ui.label(crate::tf!("update-ready-body", "version" => version));
            if *installer {
                ui.label(RichText::new(t("update-ready-installer")).color(theme::subtext()));
            }
            if *tabs > 0 {
                ui.label(RichText::new(crate::tf!("update-ready-tabs", "n" => *tabs)).color(theme::subtext()));
            }
        }
    }
    false
}
