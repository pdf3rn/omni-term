//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use eframe::egui::{self, CollapsingHeader, Color32, ComboBox, FontId, Id, Modal, RichText, ScrollArea, TextEdit};
use zeroize::Zeroizing;

use crate::ui::cursor::HandExt;
use crate::data::secrets::SecretKind;
use crate::data::settings::{AccentChoice, AnsiChoice, CursorChoice, Settings, StartupChoice, ThemeChoice, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK};
use crate::connect::ssh::{HostKeyChange, HostKeyRequest, PromptRequest};
use crate::data::store::{Encoding, FilesKind, FtpOptions, FtpTls, Flow, Newline, Parity, Protocol, SerialOptions, TelnetOptions, BAUD_RATES, Auth, Connection, Forward, ForwardKind, Snippet};
use crate::ui::theme;

mod conn_edit;
mod conn_form;
mod prompts;
mod settings;
mod shell;

// Los diálogos se usan siempre como `dialogs::X`: este módulo es su fachada.
pub use self::conn_form::*;
pub use self::prompts::*;
pub use self::settings::*;
pub use self::shell::*;
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
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
    Prompt(PromptRequest, Vec<String>),
    ConfirmReset(Pending),
    RestoreSession(RestorePrompt),
    SecretsMode(SecretsModePrompt),
    Restart,
    ConfirmContainer(ContainerConfirm),
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
                CloseAction::Window => "Cerrar omni-term",
                CloseAction::Tab(_) => "Cerrar la pestaña",
                _ => "Cerrar pestañas",
            };
            let spec = Spec::new(title).width(420.0).accept("Cerrar").danger();
            let action = shell(ctx, spec, |ui| {
                let many = confirm.lines.len() > 1;
                ui.label(if many { format!("{} pestañas tienen algo en ejecución:", confirm.lines.len()) } else { "Esta pestaña tiene algo en ejecución:".to_string() });
                for line in confirm.lines.iter().take(6) {
                    ui.label(RichText::new(line).color(theme::subtext()));
                }
                if confirm.lines.len() > 6 {
                    ui.label(RichText::new(format!("… y {} más", confirm.lines.len() - 6)).color(theme::subtext()));
                }
                ui.label("Se perderá lo que esté en curso.");
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut confirm.dont_ask, "No volver a preguntar");
                false
            });
            plain(action, Outcome::Delete)
        }
        Dialog::ConfirmPaste(confirm) => {
            let spec = Spec::new("Pegar varias líneas").width(520.0).accept("Pegar").icon(DialogIcon::Warning);
            let action = shell(ctx, spec, |ui| {
                let lines = confirm.text.lines().count();
                ui.label(if lines > 1 {
                    format!("El texto tiene {lines} líneas: el shell puede ejecutar cada una al pegarlo.")
                } else {
                    "El texto termina en un salto de línea: el shell puede ejecutarlo al pegarlo.".to_string()
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
                crate::ui::kit::switch_labeled(ui, &mut confirm.dont_ask, "No volver a preguntar");
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::TelnetWarning(warning) => {
            let spec = Spec::new("Conexión sin cifrar").width(440.0).accept("Conectar").icon(DialogIcon::Warning).no_enter();
            let action = shell(ctx, spec, |ui| {
                ui.label(format!("«{}» usa {}: todo lo que envíes, contraseñas incluidas, viaja en texto claro y cualquiera en la red puede leerlo.", warning.connection.name, warning.label));
                ui.label(RichText::new("Úsalo solo en redes de confianza o con equipos que no admiten SSH.").color(theme::subtext()));
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut warning.dont_ask, "No volver a avisar para esta conexión");
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::About => {
            let action = shell(ctx, Spec::new("Acerca de omni-term").width(420.0).cancel("Cerrar"), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::space::L;
                    ui.add(egui::Image::new(&app_logo(ui.ctx())).fit_to_exact_size(egui::Vec2::splat(64.0)));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("omni-term {}", env!("CARGO_PKG_VERSION"))).font(FontId::new(theme::size::HEADING, crate::ui::strong_family())));
                        ui.label(RichText::new("Terminal con SSH, SFTP, túneles y snippets, hecho con Rust y egui.").color(theme::subtext()));
                    });
                });
                ui.add_space(theme::space::M);
                ui.label(RichText::new("Fuentes").small().color(theme::subtext()));
                ui.label("JetBrains Mono y Noto Sans, bajo la licencia SIL Open Font License.");
                ui.label(RichText::new("Iconos").small().color(theme::subtext()));
                ui.label("Phosphor Icons, bajo la licencia MIT.");
                false
            });
            plain(action, Outcome::None)
        }
        Dialog::Edit(form) => {
            let title = if form.id.is_some() { "Editar conexión" } else { "Nueva conexión" };
            let mut spec = Spec::new(title).width(780.0).side(KIND_LIST_WIDTH).accept("Guardar").no_enter();
            if form.ssh_like() {
                spec = spec.extra("Probar conexión");
            }
            let status = match (&form.errors.general, &form.test) {
                (Some(error), _) => Some((error.clone(), theme::red())),
                (None, Some(test)) => Some(match &*test.lock().unwrap() {
                    TestState::Running => ("Probando la conexión…".to_string(), theme::subtext()),
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
            let title = if form.id.is_some() { "Editar snippet" } else { "Nuevo snippet" };
            let action = shell(ctx, Spec::new(title).accept("Guardar").no_enter(), |ui| snippet_body(ui, form));
            plain(action, Outcome::Save)
        }
        Dialog::SnippetVars(prompt) => {
            let accept = if prompt.run { "Insertar y ejecutar" } else { "Insertar" };
            let action = shell(ctx, Spec::new(prompt.title.clone()).accept(accept), |ui| vars_body(ui, prompt));
            plain(action, Outcome::Submit)
        }
        Dialog::ConfirmContainer(confirm) => {
            use crate::containers::docker::Action;
            let (title, accept, effect) = match confirm.action {
                Action::Restart => ("Reiniciar el contenedor", "Reiniciar", "se reiniciará y perderá las conexiones abiertas"),
                _ => ("Detener el contenedor", "Detener", "dejará de responder"),
            };
            let spec = Spec::new(title).width(400.0).accept(accept).icon(DialogIcon::Warning);
            let action = shell(ctx, spec, |ui| {
                ui.label(format!("«{}» ({}) {effect}.", confirm.container.name, confirm.container.image));
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::RestoreSession(prompt) => {
            let spec = Spec::new("Restaurar la sesión anterior").width(420.0).accept("Restaurar").icon(DialogIcon::Info);
            let action = shell(ctx, spec, |ui| {
                let tabs = if prompt.tabs == 1 { "1 pestaña".to_string() } else { format!("{} pestañas", prompt.tabs) };
                ui.label(format!("La sesión anterior tenía {tabs} abiertas. ¿Quieres volver a abrirlas?"));
                if prompt.remote > 0 {
                    let remote = if prompt.remote == 1 { "Se reconectará 1 sesión remota.".to_string() } else { format!("Se reconectarán {} sesiones remotas.", prompt.remote) };
                    ui.label(RichText::new(remote).color(theme::subtext()));
                }
                ui.add_space(theme::space::XS);
                crate::ui::kit::switch_labeled(ui, &mut prompt.remember, "Recordar mi elección");
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::SecretsMode(prompt) => {
            let target = if prompt.to_master { "Las contraseñas se cifrarán con una clave maestra." } else { "Las contraseñas se guardarán en el llavero del sistema." };
            let mut spec = Spec::new("Cambiar el almacén de credenciales").width(440.0).icon(DialogIcon::Info);
            if prompt.count > 0 {
                spec = spec.accept("Migrar").extra("Cambiar sin migrar");
            } else {
                spec = spec.accept("Cambiar");
            }
            let count = prompt.count;
            let action = shell(ctx, spec, |ui| {
                ui.label(target);
                if count > 0 {
                    let saved = if count == 1 { "1 contraseña guardada".to_string() } else { format!("{count} contraseñas guardadas") };
                    ui.label(RichText::new(format!("Hay {saved} en el almacén actual. Si no las migras, omni-term te las volverá a pedir.")).color(theme::subtext()));
                    ui.label(RichText::new("La migración se hace al reiniciar y borra las copias antiguas solo si todo salió bien.").small().color(theme::subtext()));
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
            let spec = Spec::new("Reiniciar para aplicar").width(400.0).accept("Reiniciar ahora").cancel("Más tarde").icon(DialogIcon::Info);
            let action = shell(ctx, spec, |ui| {
                ui.label("El cambio de almacén de credenciales se aplica al reiniciar omni-term. Las pestañas abiertas se restauran.");
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::ConfirmReset(_) => {
            let spec = Spec::new("Borrar secretos").width(380.0).accept("Borrar todo").danger();
            let action = shell(ctx, spec, |ui| {
                ui.label("Se borrarán todas las contraseñas guardadas con la clave maestra y la clave misma. No se puede deshacer.");
                false
            });
            plain(action, Outcome::Delete)
        }
        Dialog::Secret(prompt) => {
            let (title, label) = match prompt.kind {
                SecretKind::Password => ("Contraseña", "Contraseña"),
                SecretKind::Passphrase => ("Llave protegida", "Passphrase"),
            };
            let context = format!("{label} para {}@{} ({})", prompt.asking.user, prompt.asking.host, prompt.asking.name);
            let spec = Spec::new(title).width(380.0).context(context).accept("Conectar");
            let action = shell(ctx, spec, |ui| secret_body(ui, prompt));
            plain(action, Outcome::Submit)
        }
        Dialog::Unlock(prompt) => {
            let title = if prompt.creating { "Crear clave maestra" } else { "Desbloquear secretos" };
            let valid = !prompt.creating
                || (prompt.input.chars().count() >= MIN_MASTER_KEY && prompt.input == prompt.confirm);
            let mut spec = Spec::new(title)
                .width(440.0)
                .accept(if prompt.creating { "Crear" } else { "Desbloquear" })
                .accept_enabled(valid);
            if !prompt.creating {
                spec = spec.extra("Olvidé la clave…");
            }
            match shell(ctx, spec, |ui| unlock_body(ui, prompt)) {
                Action::Accept => Outcome::Submit,
                Action::Cancel => Outcome::Cancel,
                Action::Extra(_) => Outcome::ResetMasterKey,
                Action::None => Outcome::None,
            }
        }
        Dialog::Prompt(request, answers) => {
            let title = if request.title.trim().is_empty() { "Verificación en dos pasos" } else { request.title.trim() };
            let mut spec = Spec::new(title).width(380.0).accept("Continuar");
            if !request.instructions.trim().is_empty() {
                spec = spec.context(request.instructions.trim());
            }
            let action = shell(ctx, spec, |ui| prompt_body(ui, request, answers));
            plain(action, Outcome::Submit)
        }
        Dialog::HostKey(request) => {
            let spec = Spec::new("Servidor desconocido").icon(DialogIcon::Info).accept("Confiar y conectar").no_enter();
            match shell(ctx, spec, |ui| host_key_body(ui, request)) {
                Action::Accept => Outcome::Trust(true),
                Action::Cancel => Outcome::Trust(false),
                _ => Outcome::None,
            }
        }
        Dialog::HostKeyChanged(change) => {
            let spec = Spec::new("La huella del servidor ha cambiado").icon(DialogIcon::Error).cancel("Cerrar").extra("Copiar comando").extra("Olvidar y volver a conectar");
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
