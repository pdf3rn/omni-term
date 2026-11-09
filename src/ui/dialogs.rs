//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use crate::i18n::t;
use eframe::egui::{self, CollapsingHeader, Color32, FontId, Id, Modal, RichText, ScrollArea, TextEdit};
use zeroize::Zeroizing;

use crate::ui::cursor::HandExt;
use crate::data::secrets::SecretKind;
use crate::data::settings::{AccentChoice, AnsiChoice, CursorChoice, Settings, StartupChoice, ThemeChoice, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK};
use crate::connect::ssh::{HostKeyChange, HostKeyRequest, PromptRequest};
use crate::data::store::{Encoding, FilesKind, FtpOptions, FtpTls, Flow, Newline, Parity, Protocol, SerialOptions, TelnetOptions, BAUD_RATES, Auth, Connection, Forward, ForwardKind, Snippet};
use crate::ui::theme;

mod conn_edit;
mod conn_form;
mod import;
mod prompts;
mod settings;
mod shell;
mod welcome;

// Los diálogos se usan siempre como `dialogs::X`: este módulo es su fachada.
pub use self::conn_form::*;
pub use self::import::*;
pub use self::prompts::*;
pub use self::settings::*;
pub use self::shell::*;
pub use self::welcome::*;
use self::conn_edit::{edit_body, KIND_LIST_WIDTH};

impl Dialog {
    /// Error sin reintento: `title` dice qué falló y `detail` el porqué.
    pub fn failure(title: &str, detail: impl std::fmt::Display) -> Self {
        Dialog::Error(ErrorInfo { title: title.into(), detail: detail.to_string(), retry: None })
    }
}

pub enum Dialog {
    Error(ErrorInfo),
    HostKeyChanged(HostKeyChange),
    About,
    ConfirmClose(CloseConfirm),
    ConfirmPaste(PasteConfirm),
    TelnetWarning(TelnetWarning),
    Edit(ConnForm),
    Snippet(SnippetForm),
    SnippetVars(VarsPrompt),
    Password(PasswordForm),
    ConfirmTypePassword(TypePasswordConfirm),
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
    Prompt(PromptRequest, Vec<String>),
    ConfirmReset(Pending),
    RestoreSession(RestorePrompt),
    SecretsMode(SecretsModePrompt),
    Restart,
    ConfirmContainer(ContainerConfirm),
    Import(ImportPrompt),
    Welcome(WelcomePrompt),
    Update(crate::update::Release),
}

pub enum Outcome {
    None,
    Cancel,
    Save,
    Delete,
    Submit,
    Trust(bool),
    ResetMasterKey,
    /// Olvidar la huella antigua de un servidor y volver a conectar.
    Forget,
    /// «Probar conexión» en el editor de conexión.
    Test,
    /// Segunda opción de un diálogo con dos salidas afirmativas.
    Alternate,
}

pub fn show(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    connections: &[Connection],
) -> Outcome {
    match dialog {
        Dialog::ConfirmClose(confirm) => {
            let title = match confirm.action {
                CloseAction::Window => t("dialogs-cerrar-omni-term"),
                CloseAction::Tab(_) => t("dialogs-cerrar-la-pestana"),
                CloseAction::Pane { .. } => t("dialogs-cerrar-el-panel"),
                _ => t("dialogs-cerrar-pestanas"),
            };
            let spec = Spec::new(title).width(420.0).accept(t("pane-cerrar")).danger();
            let action = shell(ctx, spec, |ui| {
                let many = confirm.lines.len() > 1;
                let what = if matches!(confirm.action, CloseAction::Pane { .. }) { t("dialogs-este-panel") } else { t("dialogs-esta-pestana") };
                ui.label(if many { crate::tf!("dialogs-busy-many", "n" => confirm.lines.len()) } else { crate::tf!("dialogs-busy-one", "what" => what) });
                for line in confirm.lines.iter().take(6) {
                    ui.label(RichText::new(line).color(theme::subtext()));
                }
                if confirm.lines.len() > 6 {
                    ui.label(RichText::new(crate::tf!("dialogs-and-more", "n" => confirm.lines.len() - 6)).color(theme::subtext()));
                }
                ui.label(t("dialogs-se-perdera-lo-que-este"));
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut confirm.dont_ask, t("dialogs-no-volver-a-preguntar"));
                false
            });
            plain(action, Outcome::Delete)
        }
        Dialog::ConfirmPaste(confirm) => {
            let spec = Spec::new(t("dialogs-pegar-varias-lineas")).width(520.0).accept(t("pane-pegar")).icon(DialogIcon::Warning);
            let action = shell(ctx, spec, |ui| {
                let lines = confirm.text.lines().count();
                ui.label(if lines > 1 {
                    crate::tf!("dialogs-paste-lines", "n" => lines)
                } else {
                    t("dialogs-el-texto-termina-en-un").to_string()
                });
                ui.add_space(theme::space::XS);
                egui::Frame::new().fill(theme::surface()).corner_radius(theme::radius::SMALL).inner_margin(theme::space::S).show(ui, |ui| {
                    ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let preview: String = confirm.text.chars().take(4000).collect();
                        ui.label(RichText::new(preview.trim_end()).monospace());
                    });
                });
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut confirm.dont_ask, t("dialogs-no-volver-a-preguntar"));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::TelnetWarning(warning) => {
            let spec = Spec::new(t("dialogs-conexion-sin-cifrar")).width(440.0).accept(t("dialogs-conectar")).icon(DialogIcon::Warning).click_only();
            let action = shell(ctx, spec, |ui| {
                ui.label(crate::tf!("dialogs-cleartext", "name" => warning.connection.name.to_string(), "protocol" => warning.label.to_string()));
                ui.label(RichText::new(t("dialogs-usalo-solo-en-redes-de")).color(theme::subtext()));
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut warning.dont_ask, t("dialogs-no-volver-a-avisar-para"));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::About => {
            let action = shell(ctx, Spec::new(t("dialogs-acerca-de-omni-term")).width(420.0).cancel(t("pane-cerrar")), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::space::L;
                    ui.add(egui::Image::new(&app_logo(ui.ctx())).fit_to_exact_size(egui::Vec2::splat(64.0)));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("omni-term {}", env!("CARGO_PKG_VERSION"))).font(FontId::new(theme::size::HEADING, crate::ui::strong_family())));
                        ui.label(RichText::new(t("dialogs-terminal-con-ssh-sftp-tuneles")).color(theme::subtext()));
                    });
                });
                ui.add_space(theme::space::M);
                ui.label(RichText::new(t("dialogs-fuentes")).small().color(theme::subtext()));
                ui.label(t("dialogs-jetbrains-mono-y-noto-sans"));
                ui.label(RichText::new(t("dialogs-iconos")).small().color(theme::subtext()));
                ui.label(t("dialogs-phosphor-icons-bajo-la-licencia"));
                false
            });
            plain(action, Outcome::None)
        }
        Dialog::Edit(form) => {
            let title = if form.id.is_some() { t("dialogs-editar-conexion") } else { t("dialogs-nueva-conexion") };
            let mut spec = Spec::new(title).width(780.0).side(KIND_LIST_WIDTH).accept(t("dialogs-guardar")).ctrl_enter();
            if form.ssh_like() {
                spec = spec.extra(t("dialogs-probar-conexion"));
            }
            let status = match (&form.errors.general, &form.test) {
                (Some(error), _) => Some((error.clone(), theme::red())),
                (None, Some(test)) => Some(match &*test.lock().unwrap() {
                    TestState::Running => (t("dialogs-probando-la-conexion").to_string(), theme::subtext()),
                    TestState::Ok(message) => (message.clone(), theme::identity(3)),
                    TestState::Failed(message) => (message.clone(), theme::red()),
                }),
                (None, None) => None,
            };
            if let Some((text, color)) = status {
                spec = spec.status(text, color);
            }
            match shell(ctx, spec, |ui| edit_body(ui, form, connections)) {
                Action::Accept => Outcome::Save,
                Action::Cancel => Outcome::Cancel,
                Action::Extra(_) => Outcome::Test,
                Action::None => Outcome::None,
            }
        }
        Dialog::Snippet(form) => {
            let title = if form.id.is_some() { t("dialogs-editar-snippet") } else { t("dialogs-nuevo-snippet") };
            let action = shell(ctx, Spec::new(title).accept(t("dialogs-guardar")).ctrl_enter(), |ui| snippet_body(ui, form));
            plain(action, Outcome::Save)
        }
        Dialog::Password(form) => {
            let title = if form.id.is_some() { t("dialogs-editar-contrasena") } else { t("dialogs-nueva-contrasena") };
            let action = shell(ctx, Spec::new(title).width(440.0).accept(t("dialogs-guardar")), |ui| password_body(ui, form));
            plain(action, Outcome::Save)
        }
        Dialog::ConfirmTypePassword(confirm) => {
            // Solo con clic: un Enter de más (el de la contraseña que se iba a escribir) no debe aceptarlo.
            let spec = Spec::new(t("dialogs-escribir-la-contrasena")).width(460.0).accept(t("explorer-escribir")).icon(DialogIcon::Warning).click_only();
            let action = shell(ctx, spec, |ui| {
                ui.label(crate::tf!("dialogs-type-anyway", "what" => confirm.label.to_string()));
                let line = confirm.line.trim();
                if !line.is_empty() {
                    ui.add_space(theme::space::XS);
                    ui.label(RichText::new(t("dialogs-linea-actual")).small().color(theme::subtext()));
                    egui::Frame::new().fill(theme::surface()).corner_radius(theme::radius::SMALL).inner_margin(theme::space::S).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let shown: String = line.chars().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect();
                        ui.label(RichText::new(shown).monospace());
                    });
                }
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::SnippetVars(prompt) => {
            let accept = if prompt.run { t("dialogs-insertar-y-ejecutar") } else { t("dialogs-insertar") };
            let action = shell(ctx, Spec::new(prompt.title.clone()).accept(accept), |ui| vars_body(ui, prompt));
            plain(action, Outcome::Submit)
        }
        Dialog::Welcome(prompt) => {
            let action = shell(ctx, welcome::welcome_spec(prompt), |ui| welcome::welcome_body(ui, prompt));
            match action {
                Action::Accept if prompt.is_last() => Outcome::Submit,
                Action::Accept => {
                    prompt.step += 1;
                    Outcome::None
                }
                Action::Extra(_) => {
                    prompt.step = prompt.step.saturating_sub(1);
                    Outcome::None
                }
                Action::Cancel => Outcome::Cancel,
                Action::None => Outcome::None,
            }
        }
        Dialog::Update(release) => {
            let spec = Spec::new(t("update-title")).width(480.0).icon(DialogIcon::Info).accept(t("update-download")).cancel(t("update-later")).extra(t("update-skip"));
            let action = shell(ctx, spec, |ui| {
                ui.label(crate::tf!("update-body", "version" => release.version.clone(), "current" => crate::update::current()));
                let notes = crate::update::plain_notes(&release.notes);
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
                false
            });
            match action {
                Action::Accept => Outcome::Submit,
                Action::Extra(_) => Outcome::Alternate,
                Action::Cancel => Outcome::Cancel,
                Action::None => Outcome::None,
            }
        }
        Dialog::Import(prompt) => {
            let spec = Spec::new(t("import-title")).width(520.0).accept(crate::tf!("import-accept", "n" => prompt.selected())).accept_enabled(prompt.selected() > 0);
            let action = shell(ctx, spec, |ui| import::import_body(ui, prompt, true));
            plain(action, Outcome::Submit)
        }
        Dialog::ConfirmContainer(confirm) => {
            use crate::containers::docker::Action;
            let (title, accept, effect) = match confirm.action {
                Action::Restart => (t("dialogs-reiniciar-el-contenedor"), t("view-reiniciar"), "dialogs-container-restart-effect"),
                _ => (t("dialogs-detener-el-contenedor"), t("view-detener"), "dialogs-container-stop-effect"),
            };
            let spec = Spec::new(title).width(400.0).accept(accept).icon(DialogIcon::Warning);
            let action = shell(ctx, spec, |ui| {
                ui.label(crate::tf!(effect, "name" => confirm.container.name.to_string(), "image" => confirm.container.image.to_string()));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::RestoreSession(prompt) => {
            let spec = Spec::new(t("dialogs-restaurar-la-sesion-anterior")).width(420.0).accept(t("dialogs-restaurar")).icon(DialogIcon::Info);
            let action = shell(ctx, spec, |ui| {
                ui.label(crate::tf!("dialogs-restore-body", "tabs" => prompt.tabs));
                if prompt.remote > 0 {
                    let remote = crate::tf!("dialogs-restore-remote", "n" => prompt.remote);
                    ui.label(RichText::new(remote).color(theme::subtext()));
                }
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut prompt.remember, t("dialogs-recordar-mi-eleccion"));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::SecretsMode(prompt) => {
            let target = if prompt.to_master { t("dialogs-las-contrasenas-se-cifraran-con") } else { t("dialogs-las-contrasenas-se-guardaran-en") };
            let mut spec = Spec::new(t("dialogs-cambiar-el-almacen-de-credenciales")).width(440.0).icon(DialogIcon::Info);
            if prompt.count > 0 {
                spec = spec.accept(t("dialogs-migrar")).extra(t("dialogs-cambiar-sin-migrar"));
            } else {
                spec = spec.accept(t("dialogs-cambiar"));
            }
            let count = prompt.count;
            let action = shell(ctx, spec, |ui| {
                ui.label(target);
                if count > 0 {
                    ui.label(RichText::new(crate::tf!("dialogs-saved-passwords", "n" => count)).color(theme::subtext()));
                    ui.label(RichText::new(t("dialogs-la-migracion-se-hace-al")).small().color(theme::subtext()));
                }
                false
            });
            match action {
                Action::Accept => Outcome::Submit,
                Action::Cancel => Outcome::Cancel,
                Action::Extra(_) => Outcome::Alternate,
                Action::None => Outcome::None,
            }
        }
        Dialog::Restart => {
            let spec = Spec::new(t("dialogs-reiniciar-para-aplicar")).width(400.0).accept(t("dialogs-reiniciar-ahora")).cancel(t("dialogs-mas-tarde")).icon(DialogIcon::Info);
            let action = shell(ctx, spec, |ui| {
                ui.label(t("dialogs-el-cambio-de-almacen-de"));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::ConfirmReset(_) => {
            let spec = Spec::new(t("dialogs-borrar-secretos")).width(380.0).accept(t("dialogs-borrar-todo")).danger();
            let action = shell(ctx, spec, |ui| {
                ui.label(t("dialogs-se-borraran-todas-las-contrasenas"));
                false
            });
            plain(action, Outcome::Delete)
        }
        Dialog::Secret(prompt) => {
            let (title, label) = match prompt.kind {
                SecretKind::Password => (t("dialogs-contrasena"), t("dialogs-contrasena")),
                SecretKind::Passphrase => (t("dialogs-llave-protegida"), t("dialogs-passphrase")),
            };
            let context = crate::tf!("dialogs-secret-context", "what" => label, "user" => prompt.asking.user.to_string(), "host" => prompt.asking.host.to_string(), "name" => prompt.asking.name.to_string());
            let spec = Spec::new(title).width(380.0).context(context).accept(t("dialogs-conectar"));
            let action = shell(ctx, spec, |ui| secret_body(ui, prompt));
            plain(action, Outcome::Submit)
        }
        Dialog::Unlock(prompt) => {
            let title = if prompt.creating { t("dialogs-crear-clave-maestra") } else { t("dialogs-desbloquear-secretos") };
            let valid = !prompt.creating
                || (prompt.input.chars().count() >= MIN_MASTER_KEY && prompt.input == prompt.confirm);
            let mut spec = Spec::new(title)
                .width(440.0)
                .accept(if prompt.creating { t("explorer-crear") } else { t("dialogs-desbloquear") })
                .accept_enabled(valid);
            if !prompt.creating {
                spec = spec.extra(t("dialogs-olvide-la-clave"));
            }
            match shell(ctx, spec, |ui| unlock_body(ui, prompt)) {
                Action::Accept => Outcome::Submit,
                Action::Cancel => Outcome::Cancel,
                Action::Extra(_) => Outcome::ResetMasterKey,
                Action::None => Outcome::None,
            }
        }
        Dialog::Prompt(request, answers) => {
            let title = if request.title.trim().is_empty() { t("dialogs-verificacion-en-dos-pasos") } else { request.title.trim() };
            let mut spec = Spec::new(title).width(380.0).accept(t("dialogs-continuar"));
            if !request.instructions.trim().is_empty() {
                spec = spec.context(request.instructions.trim());
            }
            let action = shell(ctx, spec, |ui| prompt_body(ui, request, answers));
            plain(action, Outcome::Submit)
        }
        Dialog::HostKey(request) => {
            let spec = Spec::new(t("dialogs-servidor-desconocido")).icon(DialogIcon::Info).accept(t("dialogs-confiar-y-conectar")).click_only();
            match shell(ctx, spec, |ui| host_key_body(ui, request)) {
                Action::Accept => Outcome::Trust(true),
                Action::Cancel => Outcome::Trust(false),
                _ => Outcome::None,
            }
        }
        Dialog::HostKeyChanged(change) => {
            let spec = Spec::new(t("dialogs-la-huella-del-servidor-ha")).icon(DialogIcon::Error).cancel(t("pane-cerrar")).extra(t("dialogs-copiar-comando")).extra(t("dialogs-olvidar-y-volver-a-conectar"));
            match shell(ctx, spec, |ui| host_key_changed_body(ui, change)) {
                Action::Cancel => Outcome::Cancel,
                Action::Extra(0) => {
                    ctx.copy_text(change.forget_command());
                    Outcome::None
                }
                Action::Extra(_) => Outcome::Forget,
                _ => Outcome::None,
            }
        }
        Dialog::Error(info) => match error_dialog(ctx, info) {
            ErrorAction::Close => Outcome::Cancel,
            ErrorAction::Retry => Outcome::Submit,
            ErrorAction::None => Outcome::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: ForwardKind, host: &str) -> ForwardRow {
        ForwardRow { kind, bind_host: host.into(), ..ForwardRow::new() }
    }

    #[test]
    fn only_non_loopback_local_and_socks_listeners_warn() {
        for host in ["127.0.0.1", "127.0.1.1", "::1", "[::1]", "localhost", "LocalHost"] {
            assert!(!row(ForwardKind::Local, host).listens_on_network(), "{host}");
        }
        for host in ["0.0.0.0", "::", "192.168.1.5", "example.com"] {
            assert!(row(ForwardKind::Local, host).listens_on_network(), "{host}");
            assert!(row(ForwardKind::Dynamic, host).listens_on_network(), "{host}");
            assert!(!row(ForwardKind::Remote, host).listens_on_network(), "{host}");
        }
    }
}
