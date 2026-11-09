//! Versiones nuevas: comprobación en segundo plano, descarga e instalación, y reinicio.

use std::sync::atomic::Ordering;

use super::*;
use crate::update::{self, Applied, Progress, Release};
use self::dialogs::{UpdatePrompt, UpdateStage};

const LAST_CHECK: &str = "update.last_check";
const SKIPPED: &str = "update.skipped";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Descarga e instalación en curso, en un hilo aparte.
pub(super) struct UpdateJob {
    progress: Arc<Progress>,
    rx: Receiver<Result<Applied, String>>,
}

/// En qué punto está la actualización encontrada.
#[derive(Default)]
pub(super) enum UpdateState {
    #[default]
    Idle,
    Working(UpdateJob),
    Failed(String),
    Ready(Applied),
}

impl App {
    fn meta_text(&self, key: &str) -> Option<String> {
        self.store.lock().unwrap().meta(key).ok().flatten().and_then(|bytes| String::from_utf8(bytes).ok())
    }

    /// Comprobación automática: solo si el usuario la activó y pasó un día desde la última.
    pub(super) fn maybe_check_updates(&mut self) {
        if !self.settings.check_updates || self.update_check.is_some() || self.update.is_some() {
            return;
        }
        let last = self.meta_text(LAST_CHECK).and_then(|t| t.parse::<i64>().ok());
        if update::due(last, now()) {
            self.start_update_check(false);
        }
    }

    /// «Buscar actualizaciones»: con una versión ya encontrada abre su diálogo; si no, pregunta a GitHub.
    pub(super) fn check_updates_now(&mut self) {
        if self.update.is_some() {
            self.open_update_dialog();
        } else {
            self.notify(t("update-checking"));
            match &mut self.update_check {
                // Una comprobación automática en curso pasa a contestar como si la hubiera pedido el usuario.
                Some((_, manual)) => *manual = true,
                None => self.start_update_check(true),
            }
        }
    }

    fn start_update_check(&mut self, manual: bool) {
        let (tx, rx) = mpsc::channel();
        let wake = self.wake();
        std::thread::spawn(move || {
            let _ = tx.send(update::fetch_latest());
            wake();
        });
        self.update_check = Some((rx, manual));
    }

    /// Recoge la comprobación y la descarga en curso, y mantiene al día el diálogo abierto.
    pub(super) fn poll_updates(&mut self) {
        self.poll_update_check();
        if let UpdateState::Working(job) = &self.update_state {
            match job.rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                Ok(Ok(applied)) => self.update_state = UpdateState::Ready(applied),
                // Cancelada por el usuario: se vuelve al aviso.
                Ok(Err(_)) if job.progress.cancel.load(Ordering::Relaxed) => self.update_state = UpdateState::Idle,
                Ok(Err(error)) => self.update_state = UpdateState::Failed(error),
                Err(mpsc::TryRecvError::Disconnected) => self.update_state = UpdateState::Failed(t("update-err-format").to_string()),
            }
        }
        let stage = self.update_stage();
        if let Some(Dialog::Update(prompt)) = &mut self.dialog {
            prompt.stage = stage;
        }
    }

    fn poll_update_check(&mut self) {
        let Some((rx, manual)) = &self.update_check else { return };
        let manual = *manual;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(t("update-err-format").to_string()),
        };
        self.update_check = None;
        match result {
            Ok(release) => {
                let _ = self.store.lock().unwrap().set_meta(LAST_CHECK, now().to_string().as_bytes());
                let newer = update::is_newer(&release.version, update::current());
                let skipped = self.meta_text(SKIPPED).as_deref() == Some(release.version.as_str());
                if newer && (manual || !skipped) {
                    self.update = Some(release);
                    if manual {
                        self.open_update_dialog();
                    }
                } else if manual {
                    self.notify(crate::tf!("update-up-to-date", "version" => update::current()));
                }
            }
            Err(error) if manual => self.notify(crate::tf!("update-failed", "error" => error)),
            Err(_) => {}
        }
    }

    fn update_stage(&self) -> UpdateStage {
        match &self.update_state {
            UpdateState::Idle => UpdateStage::Offer,
            UpdateState::Working(job) if job.progress.installing.load(Ordering::Relaxed) => UpdateStage::Installing,
            UpdateState::Working(job) => UpdateStage::Downloading { done: job.progress.done.load(Ordering::Relaxed), total: job.progress.total.load(Ordering::Relaxed) },
            UpdateState::Failed(error) => UpdateStage::Failed(error.clone()),
            UpdateState::Ready(applied) => UpdateStage::Ready { tabs: self.tabs.len(), installer: matches!(applied, Applied::Installer(_)) },
        }
    }

    /// Texto del botón de la barra de título y del menú Ayuda mientras hay una versión nueva.
    pub(super) fn update_badge(&self) -> Option<String> {
        self.update.as_ref()?;
        Some(match self.update_stage() {
            UpdateStage::Offer => t("update-available-short").to_string(),
            UpdateStage::Downloading { done, total } => crate::tf!("update-badge-downloading", "percent" => if total > 0 { done * 100 / total } else { 0 }),
            UpdateStage::Installing => t("update-badge-installing").to_string(),
            UpdateStage::Failed(_) => t("update-badge-failed").to_string(),
            UpdateStage::Ready { .. } => t("update-badge-ready").to_string(),
        })
    }

    pub(super) fn open_update_dialog(&mut self) {
        // No tapa otro diálogo: el botón de la barra de título sigue ahí para abrirlo después.
        if self.dialog.is_some() {
            return;
        }
        if let Some(release) = self.update.clone() {
            let plan = update::plan(&release, update::detect(), std::env::consts::OS, std::env::consts::ARCH);
            self.dialog = Some(Dialog::Update(UpdatePrompt { release, plan, stage: self.update_stage() }));
        }
    }

    pub(super) fn answer_update(&mut self, outcome: Outcome, prompt: UpdatePrompt) {
        match (prompt.stage, outcome) {
            (UpdateStage::Offer, Outcome::Submit) => match prompt.plan {
                Ok(plan) => self.start_update(plan, prompt.release.version),
                Err(_) => self.open_release_page(&prompt.release),
            },
            (UpdateStage::Offer, Outcome::Alternate) => self.skip_update(&prompt.release),
            (UpdateStage::Downloading { .. }, Outcome::Alternate) => {
                if let UpdateState::Working(job) = &self.update_state {
                    job.progress.cancel.store(true, Ordering::Relaxed);
                }
            }
            (UpdateStage::Failed(_), Outcome::Submit) => {
                self.update_state = UpdateState::Idle;
                if let Ok(plan) = prompt.plan {
                    self.start_update(plan, prompt.release.version);
                }
            }
            (UpdateStage::Failed(_), _) => self.update_state = UpdateState::Idle,
            (UpdateStage::Ready { .. }, Outcome::Submit) => self.restart_into_update(),
            (UpdateStage::Ready { .. }, _) => {
                if let UpdateState::Ready(Applied::Installer(setup)) = &self.update_state {
                    self.installer_on_exit = Some(setup.clone());
                }
            }
            _ => {}
        }
    }

    fn start_update(&mut self, plan: update::Plan, version: String) {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let wake = self.wake();
        let shared = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::prepare(&plan, &version, &shared, &*wake));
            wake();
        });
        self.update_state = UpdateState::Working(UpdateJob { progress, rx });
        // El diálogo se reabre en su nueva etapa: muestra la barra de progreso.
        self.open_update_dialog();
    }

    fn open_release_page(&mut self, release: &Release) {
        if let Err(error) = open::that(&release.page) {
            self.notify(crate::tf!("update-open-failed", "error" => error.to_string()));
        }
    }

    /// «Omitir esta versión»: no se vuelve a anunciar hasta que salga otra.
    fn skip_update(&mut self, release: &Release) {
        let _ = self.store.lock().unwrap().set_meta(SKIPPED, release.version.as_bytes());
        self.update = None;
    }

    fn restart_into_update(&mut self) {
        match &self.update_state {
            UpdateState::Ready(Applied::Installer(setup)) => match update::run_installer(setup, true) {
                Ok(()) => {
                    self.close_confirmed = true;
                    self.ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                Err(error) => self.notify(crate::tf!("update-restart-failed", "error" => error.to_string())),
            },
            _ => self.restart_app(),
        }
    }
}

impl Drop for App {
    /// «Al cerrar la app» con el instalador de Windows: se ejecuta ahora, sin volver a abrir la app.
    fn drop(&mut self) {
        if let Some(setup) = self.installer_on_exit.take() {
            let _ = update::run_installer(&setup, false);
        }
    }
}
