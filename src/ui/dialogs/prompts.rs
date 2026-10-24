//! Diálogos pequeños: errores, contraseñas, confirmaciones y avisos.

use super::*;

pub struct SnippetForm {
    pub id: Option<i64>,
    pub title: String,
    pub tag: String,
    pub body: String,
    pub error: Option<String>,
}

impl SnippetForm {
    pub fn new(body: String) -> Self {
        Self { id: None, title: String::new(), tag: String::new(), body, error: None }
    }

    pub fn from_snippet(snippet: &Snippet) -> Self {
        Self {
            id: snippet.id,
            title: snippet.title.clone(),
            tag: snippet.tag.clone(),
            body: snippet.body.clone(),
            error: None,
        }
    }

    pub fn to_snippet(&self) -> Result<Snippet, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("El título es obligatorio".into());
        }
        if self.body.trim().is_empty() {
            return Err("El contenido no puede estar vacío".into());
        }
        Ok(Snippet { id: self.id, title: title.into(), tag: self.tag.trim().into(), body: self.body.clone() })
    }
}

pub struct VarsPrompt {
    pub title: String,
    pub body: String,
    pub run: bool,
    /// Nombre y valor actual de cada variable, en orden de aparición.
    pub fields: Vec<(String, String)>,
}

impl VarsPrompt {
    pub fn new(snippet: &Snippet, run: bool) -> Self {
        let fields = crate::terminal::snippet_vars::variables(&snippet.body).into_iter().map(|v| (v.name, v.default)).collect();
        Self { title: snippet.title.clone(), body: snippet.body.clone(), run, fields }
    }

    pub fn rendered(&self) -> String {
        let values = self.fields.iter().cloned().collect();
        crate::terminal::snippet_vars::render(&self.body, &values)
    }
}

/// Dónde va un panel nuevo dentro de una pestaña de terminal.
#[derive(Clone, Copy)]
pub enum PaneTarget {
    /// Divide el panel con foco.
    Split(crate::terminal::layout::Dir),
    /// Sustituye la sesión de un panel (reconexión).
    Replace(u64),
}

/// Para qué se está abriendo una conexión.
#[derive(Clone)]
pub enum Purpose {
    Terminal,
    /// Un panel dentro de una pestaña de terminal existente.
    Pane { tab: u64, target: PaneTarget },
    /// Explorador de archivos, con las rutas (local, remota) a restaurar.
    Explorer(Option<String>, Option<String>),
}

pub struct SecretPrompt {
    pub purpose: Purpose,
    /// Conexión que se está abriendo.
    pub connection: Connection,
    /// Conexión a la que pertenece el secreto (puede ser un servidor de salto).
    pub asking: Connection,
    /// Pestaña a reemplazar al reconectar.
    pub slot: Option<u64>,
    pub kind: SecretKind,
    pub input: String,
    pub save: bool,
    pub reveal: bool,
}

/// Qué hacer cuando el almacén de secretos queda desbloqueado.
pub enum Pending {
    Open(Connection, Option<u64>, Purpose),
    Save { account: String, secret: Zeroizing<String> },
    Migrate(crate::data::secrets::MigrateFrom),
}

/// Cambio de almacén pendiente de confirmar.
pub struct SecretsModePrompt {
    pub to_master: bool,
    pub count: usize,
}

pub struct UnlockPrompt {
    pub creating: bool,
    pub reveal: bool,
    pub input: String,
    pub confirm: String,
    pub error: Option<String>,
    pub then: Pending,
}

/// Qué repetir cuando el usuario pulsa «Reintentar» en un error.
#[derive(Clone)]
pub enum Retry {
    Open(Connection, Option<u64>, Purpose),
}

/// Un fallo: qué no se pudo hacer, el detalle técnico y, si cabe, cómo reintentarlo.
pub struct ErrorInfo {
    pub title: String,
    pub detail: String,
    pub retry: Option<Retry>,
}

pub enum ErrorAction {
    None,
    Close,
    Retry,
}

/// Diálogo de error común a toda la app: detalle seleccionable, «Copiar detalle» y «Reintentar».
pub fn error_dialog(ctx: &egui::Context, info: &ErrorInfo) -> ErrorAction {
    let mut spec = Spec::new(info.title.clone()).width(440.0).cancel("Cerrar").extra("Copiar detalle").icon(DialogIcon::Error);
    if info.retry.is_some() {
        spec = spec.accept("Reintentar");
    }
    match shell(ctx, spec, |ui| {
        egui::ScrollArea::vertical().max_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.label(RichText::new(info.detail.as_str()).monospace().color(theme::subtext()));
        });
        false
    }) {
        Action::Accept => ErrorAction::Retry,
        Action::Cancel => ErrorAction::Close,
        Action::Extra(_) => {
            ctx.copy_text(info.detail.clone());
            ErrorAction::None
        }
        Action::None => ErrorAction::None,
    }
}

/// Qué se cierra cuando se confirma (las pestañas se identifican por su id, no por su posición).
#[derive(Clone, Copy)]
pub enum CloseAction {
    Tab(u64),
    Others(u64),
    Right(u64),
    Window,
}

pub struct RestorePrompt {
    pub tabs: usize,
    pub remote: usize,
    pub remember: bool,
}

pub struct CloseConfirm {
    pub action: CloseAction,
    pub lines: Vec<String>,
    pub dont_ask: bool,
}

/// Aviso previo al abrir una conexión Telnet: viaja todo en texto claro.
pub struct TelnetWarning {
    pub connection: Connection,
    pub slot: Option<u64>,
    pub purpose: Purpose,
    pub dont_ask: bool,
    /// Protocolo que no cifra: «Telnet» o «FTP sin cifrar».
    pub label: &'static str,
}

/// Detener o reiniciar un contenedor, pendiente de confirmar.
pub struct ContainerConfirm {
    pub endpoint: crate::containers::docker::Endpoint,
    pub container: crate::containers::docker::Container,
    pub action: crate::containers::docker::Action,
}

pub const MIN_MASTER_KEY: usize = 8;

pub(super) fn snippet_body(ui: &mut egui::Ui, form: &mut SnippetForm) -> bool {
    egui::Grid::new("snippet_form").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label("Título");
        ui.add(TextEdit::singleline(&mut form.title).desired_width(f32::INFINITY));
        ui.end_row();
        ui.label("Etiqueta");
        ui.add(TextEdit::singleline(&mut form.tag).hint_text("opcional").desired_width(200.0));
        ui.end_row();
    });
    ui.label("Contenido");
    ui.label(RichText::new("Usa {{variable}} o {{variable=valor por defecto}}; se pedirán al insertar.").small().color(theme::subtext()));
    ui.add(
        TextEdit::multiline(&mut form.body)
            .font(egui::TextStyle::Monospace)
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );
    error_label(ui, &form.error);
    false
}

pub(super) fn vars_body(ui: &mut egui::Ui, prompt: &mut VarsPrompt) -> bool {
    let mut submitted = false;
    egui::Grid::new("snippet_vars").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        for (index, (name, value)) in prompt.fields.iter_mut().enumerate() {
            ui.label(name.as_str());
            let field = ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
            if index == 0 {
                field.request_focus();
            }
            submitted |= entered(ui, &field);
            ui.end_row();
        }
    });
    ui.add_space(theme::space::XS);
    ui.label(RichText::new("Resultado").small().color(theme::subtext()));
    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
        ui.label(RichText::new(prompt.rendered()).monospace().color(theme::subtext()));
    });
    submitted
}

pub(super) fn secret_body(ui: &mut egui::Ui, prompt: &mut SecretPrompt) -> bool {
    let field = secret_field(ui, &mut prompt.input, &mut prompt.reveal, "");
    field.request_focus();
    crate::ui::kit::switch_labeled(ui, &mut prompt.save, "Guardar en el almacén de secretos");
    ui.label(RichText::new("Si no la guardas, se recuerda hasta cerrar omni-term.").small().color(theme::subtext()));
    entered(ui, &field)
}

pub(super) fn unlock_body(ui: &mut egui::Ui, prompt: &mut UnlockPrompt) -> bool {
    if prompt.creating {
        ui.label("No hay keyring del sistema disponible. Elige una clave maestra para cifrar tus contraseñas.");
        ui.label(RichText::new("Si la olvidas no se puede recuperar: tendrás que borrar los secretos y volver a introducirlos.").color(theme::subtext()));
    } else {
        ui.label("Introduce tu clave maestra.");
    }
    let field = secret_field(ui, &mut prompt.input, &mut prompt.reveal, "clave maestra");
    field.request_focus();
    let mut submitted = entered(ui, &field);
    if prompt.creating {
        let confirm = secret_field(ui, &mut prompt.confirm, &mut prompt.reveal, "repite la clave");
        submitted |= entered(ui, &confirm);
        let length = prompt.input.chars().count();
        if length > 0 && length < MIN_MASTER_KEY {
            ui.label(RichText::new(format!("Usa al menos {MIN_MASTER_KEY} caracteres ({length} ahora).")).small().color(theme::red()));
        } else if !prompt.confirm.is_empty() && prompt.input != prompt.confirm {
            ui.label(RichText::new("Las dos claves no coinciden.").small().color(theme::red()));
        }
    }
    error_label(ui, &prompt.error);
    submitted
}

pub(super) fn prompt_body(ui: &mut egui::Ui, request: &PromptRequest, answers: &mut Vec<String>) -> bool {
    answers.resize(request.prompts.len(), String::new());
    let mut submitted = false;
    for (index, ((question, echo), answer)) in request.prompts.iter().zip(answers.iter_mut()).enumerate() {
        ui.label(question.trim());
        let field = ui.add(TextEdit::singleline(answer).password(!echo).desired_width(f32::INFINITY));
        if index == 0 {
            field.request_focus();
        }
        submitted |= entered(ui, &field);
    }
    submitted
}

pub(super) fn host_key_body(ui: &mut egui::Ui, request: &HostKeyRequest) -> bool {
    ui.label(format!("No se puede verificar la autenticidad de {}:{}.", request.host, request.port));
    ui.add_space(theme::space::XS);
    ui.label(RichText::new(format!("{} {}", request.algorithm, request.fingerprint)).monospace());
    ui.add_space(theme::space::XS);
    ui.label("Si confías en él, se añadirá a ~/.ssh/known_hosts.");
    false
}

pub(super) fn host_key_changed_body(ui: &mut egui::Ui, change: &HostKeyChange) -> bool {
    ui.label(
        RichText::new("omni-term no se ha conectado. Puede que alguien se esté haciendo pasar por el servidor, o que el servidor se haya reinstalado.")
            .color(theme::red()),
    );
    ui.add_space(theme::space::XS);
    egui::Grid::new("host_key_changed").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
        ui.label("Servidor");
        ui.label(RichText::new(format!("{}:{}", change.host, change.port)).monospace());
        ui.end_row();
        ui.label(format!("Guardada (línea {})", change.line));
        ui.label(RichText::new(change.stored.as_deref().unwrap_or("desconocida")).monospace().color(theme::subtext()));
        ui.end_row();
        ui.label("Recibida");
        ui.label(RichText::new(format!("{} ({})", change.received, change.algorithm)).monospace());
        ui.end_row();
    });
    ui.add_space(theme::space::XS);
    ui.label("Si sabes que el servidor cambió de verdad, borra la huella antigua y vuelve a conectar:");
    ui.label(RichText::new(change.forget_command()).monospace().color(theme::subtext()));
    false
}
