//! Avisos de versión nueva: comprobación en segundo plano y respuesta del diálogo.

use super::*;
use crate::update::{self, Release};

const LAST_CHECK: &str = "update.last_check";
const SKIPPED: &str = "update.skipped";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl App {
    fn meta_text(&self, key: &str) -> Option<String> {
        self.store.lock().unwrap().meta(key).ok().flatten().and_then(|bytes| String::from_utf8(bytes).ok())
    }

    /// Comprobación automática: solo si el usuario la activó y pasó un día desde la última.
    pub(super) fn maybe_check_updates(&mut self) {
        if !self.settings.check_updates || self.update_check.is_some() {
            return;
        }
        let last = self.meta_text(LAST_CHECK).and_then(|t| t.parse::<i64>().ok());
        if update::due(last, now()) {
            self.start_update_check(false);
        }
    }

    /// «Buscar actualizaciones»: con una versión ya encontrada abre su aviso; si no, pregunta a GitHub.
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

    pub(super) fn poll_update_check(&mut self) {
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

    pub(super) fn open_update_dialog(&mut self) {
        // No tapa otro diálogo: el aviso de la barra de título sigue ahí para abrirlo después.
        if self.dialog.is_none() {
            if let Some(release) = self.update.clone() {
                self.dialog = Some(Dialog::Update(release));
            }
        }
    }

    pub(super) fn download_update(&mut self, release: Release) {
        if let Err(error) = open::that(&release.page) {
            self.notify(crate::tf!("update-open-failed", "error" => error.to_string()));
        }
    }

    /// «Omitir esta versión»: no se vuelve a anunciar hasta que salga otra.
    pub(super) fn skip_update(&mut self, release: Release) {
        let _ = self.store.lock().unwrap().set_meta(SKIPPED, release.version.as_bytes());
        self.update = None;
    }
}
