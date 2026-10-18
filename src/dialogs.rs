//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use eframe::egui::{self, Color32, ComboBox, FontId, Id, Modal, RichText, TextEdit};
use zeroize::Zeroizing;

use crate::cursor::HandExt;
use crate::secrets::SecretKind;
use crate::settings::{AccentChoice, CursorChoice, Settings, ThemeChoice, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK};
use crate::ssh::{HostKeyChange, HostKeyRequest, PromptRequest};
use crate::store::{Auth, Connection, Forward, ForwardKind, Snippet};
use crate::theme;

/// Atajos de entorno: nombre del botón, etiqueta que se guarda y color de identidad.
const ENVIRONMENTS: [(&str, &str, u8); 4] = [
    ("Producción", "producción", 0),
    ("Staging", "staging", 1),
    ("Desarrollo", "desarrollo", 3),
    ("Personal", "personal", 4),
];

pub struct ForwardRow {
    pub kind: ForwardKind,
    pub bind_host: String,
    pub bind_port: String,
    pub dest_host: String,
    pub dest_port: String,
}

impl ForwardRow {
    fn new() -> Self {
        Self {
            kind: ForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: String::new(),
            dest_host: "localhost".into(),
            dest_port: String::new(),
        }
    }

    fn from_forward(forward: &Forward) -> Self {
        Self {
            kind: forward.kind,
            bind_host: forward.bind_host.clone(),
            bind_port: forward.bind_port.to_string(),
            dest_host: forward.dest_host.clone(),
            dest_port: forward.dest_port.to_string(),
        }
    }

    fn to_forward(&self) -> Result<Forward, String> {
        let port = |text: &str, what: &str| -> Result<u16, String> {
            match text.trim().parse::<u16>() {
                Ok(port) if port > 0 => Ok(port),
                _ => Err(format!("El puerto {what} de un túnel debe estar entre 1 y 65535")),
            }
        };
        let bind_host = self.bind_host.trim();
        if bind_host.is_empty() {
            return Err("Cada túnel necesita una dirección de escucha".into());
        }
        if self.kind == ForwardKind::Dynamic {
            return Ok(Forward {
                kind: self.kind,
                bind_host: bind_host.into(),
                bind_port: port(&self.bind_port, "de escucha")?,
                dest_host: String::new(),
                dest_port: 0,
            });
        }
        let dest_host = self.dest_host.trim();
        if dest_host.is_empty() {
            return Err("Cada túnel necesita dirección de destino".into());
        }
        Ok(Forward {
            kind: self.kind,
            bind_host: bind_host.into(),
            bind_port: port(&self.bind_port, "de escucha")?,
            dest_host: dest_host.into(),
            dest_port: port(&self.dest_port, "de destino")?,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditTab {
    General,
    Acceso,
    Avanzado,
}

/// Resultado de «Probar conexión», actualizado desde una tarea en segundo plano.
pub enum TestState {
    Running,
    Ok(String),
    Failed(String),
}

pub type TestHandle = std::sync::Arc<std::sync::Mutex<TestState>>;

/// Errores de validación, pegados al campo que los causa.
#[derive(Default)]
pub struct FormErrors {
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub key_path: Option<String>,
    pub forwards: Vec<Option<String>>,
    /// Error al guardar que no pertenece a ningún campo.
    pub general: Option<String>,
}

impl Dialog {
    /// Error sin reintento: `title` dice qué falló y `detail` el porqué.
    pub fn failure(title: &str, detail: impl std::fmt::Display) -> Self {
        Dialog::Error(ErrorInfo { title: title.into(), detail: detail.to_string(), retry: None })
    }
}

impl FormErrors {
    fn in_tab(&self, tab: EditTab) -> bool {
        match tab {
            EditTab::General => self.name.is_some() || self.host.is_some() || self.port.is_some() || self.user.is_some(),
            EditTab::Acceso => self.key_path.is_some(),
            EditTab::Avanzado => self.forwards.iter().any(Option::is_some),
        }
    }
}

pub struct ConnForm {
    /// Pide el foco para «Nombre» la primera vez que se dibuja.
    pub focus: bool,
    /// Muestra la etiqueta y los colores a mano en vez de los atajos de entorno.
    pub custom_env: bool,
    pub tab: EditTab,
    pub errors: FormErrors,
    pub test: Option<TestHandle>,
    pub color: Option<u8>,
    pub env: String,
    pub jump: Option<i64>,
    pub forwards: Vec<ForwardRow>,
    pub id: Option<i64>,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub auth: Auth,
    pub key_path: String,
    pub init_command: String,
    pub secret: String,
}

impl ConnForm {
    pub fn new() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: String::new(),
            port: "22".into(),
            user: std::env::var("USER").unwrap_or_default(),
            auth: Auth::Agent,
            key_path: "~/.ssh/id_ed25519".into(),
            init_command: String::new(),
            secret: String::new(),
            focus: true,
            custom_env: false,
            tab: EditTab::General,
            errors: FormErrors::default(),
            test: None,
            forwards: Vec::new(),
            jump: None,
            color: None,
            env: String::new(),
        }
    }

    pub fn from_connection(connection: &Connection, forwards: &[Forward]) -> Self {
        let env = connection.env.clone().unwrap_or_default();
        let known = ENVIRONMENTS.iter().any(|(_, name, color)| env.eq_ignore_ascii_case(name) && connection.color == Some(*color));
        let custom_env = !known && (!env.is_empty() || connection.color.is_some());
        Self {
            focus: true,
            custom_env,
            jump: connection.jump,
            color: connection.color,
            env,
            forwards: forwards.iter().map(ForwardRow::from_forward).collect(),
            id: connection.id,
            name: connection.name.clone(),
            host: connection.host.clone(),
            port: connection.port.to_string(),
            user: connection.user.clone(),
            auth: connection.auth,
            key_path: connection.key_path.clone().unwrap_or_default(),
            init_command: connection.init_command.clone().unwrap_or_default(),
            secret: String::new(),
            tab: EditTab::General,
            errors: FormErrors::default(),
            test: None,
        }
    }

    /// Valida el formulario. Si hay errores los deja junto a cada campo, cambia a la pestaña
    /// del primero y devuelve `None`; si no, devuelve la conexión y sus túneles.
    pub fn validate(&mut self) -> Option<(Connection, Vec<Forward>)> {
        let mut errors = FormErrors::default();
        let name = self.name.trim().to_string();
        let host = self.host.trim().to_string();
        let user = self.user.trim().to_string();
        if name.is_empty() {
            errors.name = Some("Pon un nombre para reconocerla".into());
        }
        if host.is_empty() {
            errors.host = Some("Indica el servidor".into());
        }
        if user.is_empty() {
            errors.user = Some("Indica el usuario".into());
        }
        let port = match self.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                errors.port = Some("Un número entre 1 y 65535".into());
                0
            }
        };
        let key_path = self.key_path.trim().to_string();
        if self.auth == Auth::Key && key_path.is_empty() {
            errors.key_path = Some("Indica la ruta de la llave privada".into());
        }
        let mut forwards = Vec::new();
        for row in &self.forwards {
            match row.to_forward() {
                Ok(forward) => {
                    forwards.push(forward);
                    errors.forwards.push(None);
                }
                Err(message) => errors.forwards.push(Some(message)),
            }
        }

        let first_tab = [EditTab::General, EditTab::Acceso, EditTab::Avanzado].into_iter().find(|tab| errors.in_tab(*tab));
        if let Some(tab) = first_tab {
            self.tab = tab;
            self.errors = errors;
            return None;
        }
        self.errors = FormErrors::default();
        let connection = Connection {
            id: self.id,
            name,
            host,
            port,
            user,
            auth: self.auth,
            key_path: (self.auth == Auth::Key).then_some(key_path),
            init_command: Some(self.init_command.trim().to_string()).filter(|c| !c.is_empty()),
            jump: self.jump,
            color: self.color,
            env: Some(self.env.trim().to_string()).filter(|e| !e.is_empty()),
        };
        Some((connection, forwards))
    }
}

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
        let fields = crate::snippet_vars::variables(&snippet.body).into_iter().map(|v| (v.name, v.default)).collect();
        Self { title: snippet.title.clone(), body: snippet.body.clone(), run, fields }
    }

    pub fn rendered(&self) -> String {
        let values = self.fields.iter().cloned().collect();
        crate::snippet_vars::render(&self.body, &values)
    }
}

/// Dónde va un panel nuevo dentro de una pestaña de terminal.
#[derive(Clone, Copy)]
pub enum PaneTarget {
    /// Divide el panel con foco.
    Split(crate::layout::Dir),
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
    let mut spec = Spec::new(info.title.clone()).width(440.0).cancel("Cerrar").extra("Copiar detalle");
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Atajos,
}

pub enum Dialog {
    Error(ErrorInfo),
    HostKeyChanged(HostKeyChange),
    Settings(SettingsTab),
    Edit(ConnForm),
    Snippet(SnippetForm),
    SnippetVars(VarsPrompt),
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
    Prompt(PromptRequest, Vec<String>),
    ConfirmReset(Pending),
}

pub enum Outcome {
    None,
    Cancel,
    Save,
    Delete,
    Submit,
    Trust(bool),
    ResetMasterKey,
    /// «Probar conexión» en el editor de conexión.
    Test,
}

fn id() -> Id {
    Id::NULL.with("dialog")
}

/// Aspecto de la acción principal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Acción destructiva: botón rojo y sin confirmar con Enter.
    Danger,
}

/// Lo que cada diálogo declara de sí mismo; el marco común dibuja el resto.
pub struct Spec {
    title: String,
    context: Option<String>,
    width: f32,
    accept: Option<String>,
    accept_enabled: bool,
    tone: Tone,
    cancel: String,
    extras: Vec<String>,
    enter_submits: bool,
    backdrop_cancels: bool,
}

impl Spec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            context: None,
            width: 520.0,
            accept: None,
            accept_enabled: true,
            tone: Tone::Normal,
            cancel: "Cancelar".into(),
            extras: Vec::new(),
            enter_submits: true,
            backdrop_cancels: false,
        }
    }

    pub fn context(mut self, text: impl Into<String>) -> Self {
        self.context = Some(text.into());
        self
    }

    /// 380 px para preguntas y confirmaciones, 520 para formularios.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn accept(mut self, label: impl Into<String>) -> Self {
        self.accept = Some(label.into());
        self
    }

    pub fn accept_enabled(mut self, enabled: bool) -> Self {
        self.accept_enabled = enabled;
        self
    }

    pub fn danger(mut self) -> Self {
        self.tone = Tone::Danger;
        self.enter_submits = false;
        self.backdrop_cancels = true;
        self
    }

    pub fn cancel(mut self, label: impl Into<String>) -> Self {
        self.cancel = label.into();
        self
    }

    pub fn extra(mut self, label: impl Into<String>) -> Self {
        self.extras.push(label.into());
        self
    }

    /// Enter dentro de un campo no confirma (formularios con texto multilínea, confianza de huellas).
    pub fn no_enter(mut self) -> Self {
        self.enter_submits = false;
        self
    }

    /// Un clic fuera cierra el diálogo; solo para los que no tienen nada que perder.
    pub fn backdrop_closes(mut self) -> Self {
        self.backdrop_cancels = true;
        self
    }
}

pub enum Action {
    None,
    Accept,
    Cancel,
    Extra(usize),
}

/// Marco común: título, contexto, cuerpo y pie con los botones a la derecha.
/// `Esc` cancela; `Enter` confirma cuando el cuerpo lo pide (devolviendo `true`).
pub fn shell(ctx: &egui::Context, spec: Spec, body: impl FnOnce(&mut egui::Ui) -> bool) -> Action {
    let mut action = Action::None;
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(24));
    let response = Modal::new(id()).frame(frame).show(ctx, |ui| {
        ui.set_width(spec.width);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(RichText::new(&spec.title).font(FontId::new(18.0, crate::ui::strong_family())));
        if let Some(context) = &spec.context {
            ui.label(RichText::new(context).small().color(theme::subtext()));
        }
        ui.add_space(10.0);

        let enter = body(ui);

        ui.add_space(18.0);
        ui.horizontal(|ui| {
            for (index, label) in spec.extras.iter().enumerate() {
                let extra = egui::Button::new(label.as_str())
                    .fill(Color32::TRANSPARENT)
                    .stroke(egui::Stroke::new(1.0, theme::surface()));
                if ui.add(extra).clicked() {
                    action = Action::Extra(index);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fill = if spec.tone == Tone::Danger { theme::red() } else { theme::accent() };
                let primary = |label: &str| egui::Button::new(RichText::new(label).color(theme::base())).fill(fill);
                match &spec.accept {
                    Some(label) => {
                        if ui.add_enabled(spec.accept_enabled, primary(label)).clicked() {
                            action = Action::Accept;
                        }
                        if ui.add(egui::Button::new(spec.cancel.as_str()).fill(Color32::TRANSPARENT)).clicked() {
                            action = Action::Cancel;
                        }
                    }
                    None => {
                        if ui.add(primary(&spec.cancel)).clicked() {
                            action = Action::Cancel;
                        }
                    }
                }
            });
        });
        if enter && spec.enter_submits && spec.accept.is_some() && spec.accept_enabled && matches!(action, Action::None) {
            action = Action::Accept;
        }
    });
    if matches!(action, Action::None) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Action::Cancel;
        } else if spec.backdrop_cancels && response.backdrop_response.clicked() {
            action = Action::Cancel;
        }
    }
    action
}

/// Mínimo de caracteres de la clave maestra.
pub const MIN_MASTER_KEY: usize = 8;

/// Campo de contraseña con botón para mostrar u ocultar lo escrito.
fn secret_field(ui: &mut egui::Ui, value: &mut String, reveal: &mut bool, hint: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 76.0).max(80.0);
        let field = ui.add(TextEdit::singleline(value).password(!*reveal).hint_text(hint).desired_width(width));
        let label = if *reveal { "Ocultar" } else { "Mostrar" };
        if ui.add(egui::Button::new(RichText::new(label).small()).fill(Color32::TRANSPARENT)).clicked() {
            *reveal = !*reveal;
        }
        field
    })
    .inner
}

/// ¿Se pulsó Enter dentro de este campo?
fn entered(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn error_label(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(error) = error {
        ui.add_space(4.0);
        ui.label(RichText::new(error).color(theme::red()));
    }
}

fn plain(action: Action, accepted: Outcome) -> Outcome {
    match action {
        Action::Accept => accepted,
        Action::Cancel => Outcome::Cancel,
        Action::Extra(_) | Action::None => Outcome::None,
    }
}

pub fn show(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    master_forced: &mut bool,
    connections: &[Connection],
    settings: &mut Settings,
) -> Outcome {
    match dialog {
        Dialog::Settings(tab) => {
            let action = shell(ctx, Spec::new("Ajustes").width(520.0).cancel("Cerrar"), |ui| settings_body(ui, tab, settings, master_forced));
            plain(action, Outcome::None)
        }
        Dialog::Edit(form) => {
            let title = if form.id.is_some() { "Editar conexión" } else { "Nueva conexión" };
            let spec = Spec::new(title).width(520.0).accept("Guardar").no_enter().extra("Probar conexión");
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
            let spec = Spec::new("Servidor desconocido").accept("Confiar y conectar").no_enter();
            match shell(ctx, spec, |ui| host_key_body(ui, request)) {
                Action::Accept => Outcome::Trust(true),
                Action::Cancel => Outcome::Trust(false),
                _ => Outcome::None,
            }
        }
        Dialog::HostKeyChanged(change) => {
            let spec = Spec::new("La huella del servidor ha cambiado").cancel("Cerrar").extra("Copiar comando");
            match shell(ctx, spec, |ui| host_key_changed_body(ui, change)) {
                Action::Cancel => Outcome::Cancel,
                Action::Extra(_) => {
                    ctx.copy_text(change.forget_command());
                    Outcome::None
                }
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

/// Ancho reservado a las etiquetas del formulario.
const LABEL_WIDTH: f32 = 112.0;
const FIELD_HEIGHT: f32 = 28.0;
/// Espacio entre filas de un mismo grupo y entre grupos distintos.
const ROW_GAP: f32 = 8.0;
const GROUP_GAP: f32 = 18.0;

/// Fila de formulario: etiqueta alineada a la izquierda y contenido que ocupa el resto.
fn form_row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, FIELD_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(label);
            },
        );
        ui.vertical(content);
    });
}

/// Campo de texto con el borde en rojo mientras tiene un error.
fn text_input(ui: &mut egui::Ui, value: &mut String, width: f32, hint: &str, error: &mut Option<String>) -> egui::Response {
    let broken = error.is_some();
    let response = ui
        .scope(|ui| {
            if broken {
                let stroke = egui::Stroke::new(1.5, theme::red());
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.bg_stroke = stroke;
                widgets.hovered.bg_stroke = stroke;
            }
            ui.add_sized(
                [width, FIELD_HEIGHT],
                TextEdit::singleline(value).hint_text(hint).align(egui::Align2::LEFT_CENTER),
            )
        })
        .inner;
    if response.changed() {
        *error = None;
    }
    response
}

fn error_line(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(message) = error {
        ui.label(RichText::new(message.as_str()).small().color(theme::red()));
    }
}

/// Botón de entorno: punto de color, nombre y fondo si está elegido.
fn chip(ui: &mut egui::Ui, label: &str, color: Option<Color32>, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(13.0, egui::FontFamily::Proportional), Color32::PLACEHOLDER);
    let dot = if color.is_some() { 14.0 } else { 0.0 };
    let size = egui::vec2(galley.size().x + dot + 18.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.hand();
    let fill = if selected {
        theme::selection()
    } else if response.hovered() {
        theme::surface()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), fill);
    if !selected {
        ui.painter().rect_stroke(rect, egui::CornerRadius::same(6), egui::Stroke::new(1.0, theme::surface()), egui::StrokeKind::Inside);
    }
    let mut x = rect.min.x + 9.0;
    if let Some(color) = color {
        ui.painter().circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.5, color);
        x += dot;
    }
    let text_color = if selected { theme::text() } else { theme::subtext() };
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_color);
    response
}

fn environment_row(ui: &mut egui::Ui, form: &mut ConnForm) {
    form_row(ui, "Entorno", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 6.0);
            let none = form.env.trim().is_empty() && form.color.is_none();
            for (name, env, color) in ENVIRONMENTS {
                let selected = !form.custom_env && form.env.eq_ignore_ascii_case(env) && form.color == Some(color);
                if chip(ui, name, Some(theme::identity(color)), selected).on_hover_text(if selected { "Clic para quitarlo" } else { "Pone la etiqueta y el color" }).clicked() {
                    // Pulsar el elegido lo quita: sin entorno ni color.
                    if selected {
                        form.env.clear();
                        form.color = None;
                    } else {
                        form.env = env.to_string();
                        form.color = Some(color);
                    }
                    form.custom_env = false;
                }
            }
            let custom_selected = form.custom_env || (!none && !ENVIRONMENTS.iter().any(|(_, e, c)| form.env.eq_ignore_ascii_case(e) && form.color == Some(*c)));
            if chip(ui, "Otro…", None, custom_selected).on_hover_text("Elegir la etiqueta y el color a mano").clicked() {
                form.custom_env = true;
            }
        });
    });
    if form.custom_env {
        ui.add_space(ROW_GAP);
        form_row(ui, "Etiqueta", |ui| {
            let width = ui.available_width();
            ui.add_sized([width, FIELD_HEIGHT], TextEdit::singleline(&mut form.env).hint_text("por ejemplo, pre-producción").align(egui::Align2::LEFT_CENTER));
        });
        ui.add_space(ROW_GAP);
        form_row(ui, "Color", |ui| {
            ui.horizontal(|ui| {
                for choice in std::iter::once(None).chain((0..6).map(Some)) {
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
                    let response = response.hand();
                    match choice.map(theme::identity) {
                        Some(color) => ui.painter().circle_filled(rect.center(), 8.0, color),
                        None => ui.painter().circle_stroke(rect.center(), 8.0, egui::Stroke::new(1.5, theme::subtext())),
                    };
                    if form.color == choice {
                        ui.painter().circle_stroke(rect.center(), 11.0, egui::Stroke::new(2.0, theme::text()));
                    }
                    let name = choice.map_or("Sin color", |i| theme::IDENTITY_NAMES[i as usize]);
                    if response.on_hover_text(name).clicked() {
                        form.color = choice;
                    }
                }
            });
        });
    }
}

fn edit_body(ui: &mut egui::Ui, form: &mut ConnForm, connections: &[Connection]) -> bool {
    // Pestañas con subrayado, igual que la barra lateral; un punto rojo marca las que tienen errores.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        ui.spacing_mut().button_padding.x = 0.0;
        for (tab, label) in [(EditTab::General, "General"), (EditTab::Acceso, "Acceso"), (EditTab::Avanzado, "Avanzado")] {
            let broken = form.errors.in_tab(tab);
            let selected = form.tab == tab;
            let text = if broken { format!("{label} ●") } else { label.to_string() };
            let color = if broken { theme::red() } else if selected { theme::text() } else { theme::subtext() };
            let font = FontId::new(14.0, if selected { crate::ui::strong_family() } else { egui::FontFamily::Proportional });
            let response = ui.add(egui::Button::new(RichText::new(text).font(font).color(color)).fill(Color32::TRANSPARENT));
            if selected {
                let y = response.rect.max.y + 1.0;
                let line = if broken { theme::red() } else { theme::accent() };
                ui.painter().line_segment([egui::pos2(response.rect.min.x, y), egui::pos2(response.rect.max.x, y)], egui::Stroke::new(2.0, line));
            }
            if response.clicked() {
                form.tab = tab;
            }
        }
    });
    ui.add_space(GROUP_GAP);

    match form.tab {
        EditTab::General => {
            form_row(ui, "Nombre", |ui| {
                let width = ui.available_width();
                let response = text_input(ui, &mut form.name, width, "Nombre de la conexión", &mut form.errors.name);
                if std::mem::take(&mut form.focus) {
                    response.request_focus();
                }
                error_line(ui, &form.errors.name);
            });

            ui.add_space(GROUP_GAP);
            form_row(ui, "Servidor", |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let host_width = (ui.available_width() - 80.0 - 8.0).max(120.0);
                    text_input(ui, &mut form.host, host_width, "ejemplo.com o 10.0.0.5", &mut form.errors.host);
                    text_input(ui, &mut form.port, 80.0, "22", &mut form.errors.port);
                });
                error_line(ui, &form.errors.host);
                error_line(ui, &form.errors.port);
            });
            ui.add_space(ROW_GAP);
            form_row(ui, "Usuario", |ui| {
                let width = ui.available_width();
                text_input(ui, &mut form.user, width, "usuario del servidor", &mut form.errors.user);
                error_line(ui, &form.errors.user);
            });

            ui.add_space(GROUP_GAP);
            environment_row(ui, form);
        }
        EditTab::Acceso => {
            form_row(ui, "Autenticación", |ui| {
                ComboBox::from_id_salt("auth")
                    .width(ui.available_width())
                    .selected_text(match form.auth {
                        Auth::Password => "Contraseña",
                        Auth::Key => "Llave privada",
                        Auth::Agent => "ssh-agent",
                        Auth::Interactive => "Interactivo (2FA)",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.auth, Auth::Agent, "ssh-agent");
                        ui.selectable_value(&mut form.auth, Auth::Key, "Llave privada");
                        ui.selectable_value(&mut form.auth, Auth::Password, "Contraseña");
                        ui.selectable_value(&mut form.auth, Auth::Interactive, "Interactivo (2FA)");
                    })
                    .response
                    .hand();
            });
            if form.auth == Auth::Key {
                ui.add_space(ROW_GAP);
                form_row(ui, "Llave privada", |ui| {
                    let width = ui.available_width();
                    text_input(ui, &mut form.key_path, width, "~/.ssh/id_ed25519", &mut form.errors.key_path);
                    error_line(ui, &form.errors.key_path);
                });
            }
            if matches!(form.auth, Auth::Password | Auth::Key) {
                ui.add_space(ROW_GAP);
                form_row(ui, if form.auth == Auth::Password { "Contraseña" } else { "Passphrase" }, |ui| {
                    let width = ui.available_width();
                    ui.add_sized(
                        [width, FIELD_HEIGHT],
                        TextEdit::singleline(&mut form.secret)
                            .password(true)
                            .hint_text("opcional: se guarda en el almacén")
                            .align(egui::Align2::LEFT_CENTER),
                    );
                });
            }

            ui.add_space(GROUP_GAP);
            form_row(ui, "Saltar mediante", |ui| {
                let current = form
                    .jump
                    .and_then(|id| connections.iter().find(|c| c.id == Some(id)))
                    .map_or("Directo".to_string(), |c| c.name.clone());
                ComboBox::from_id_salt("jump")
                    .width(ui.available_width())
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.jump, None, "Directo");
                        for other in connections.iter().filter(|c| c.id != form.id) {
                            ui.selectable_value(&mut form.jump, other.id, other.name.as_str());
                        }
                    })
                    .response
                    .hand();
            });
        }
        EditTab::Avanzado => {
            form_row(ui, "Comando inicial", |ui| {
                let width = ui.available_width();
                ui.add_sized(
                    [width, FIELD_HEIGHT],
                    TextEdit::singleline(&mut form.init_command)
                        .hint_text("opcional, se ejecuta al abrir la sesión")
                        .align(egui::Align2::LEFT_CENTER),
                );
            });
            ui.add_space(GROUP_GAP);
            ui.label(RichText::new("Túneles (se activan al conectar)").small().color(theme::subtext()));
            let mut remove = None;
            form.errors.forwards.resize(form.forwards.len(), None);
            for (index, row) in form.forwards.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ComboBox::from_id_salt(("fwd-kind", index))
                        .width(92.0)
                        .selected_text(match row.kind {
                            ForwardKind::Local => "Local -L",
                            ForwardKind::Remote => "Remoto -R",
                            ForwardKind::Dynamic => "SOCKS -D",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut row.kind, ForwardKind::Local, "Local -L");
                            ui.selectable_value(&mut row.kind, ForwardKind::Remote, "Remoto -R");
                            ui.selectable_value(&mut row.kind, ForwardKind::Dynamic, "SOCKS -D");
                        })
                        .response
                        .hand();
                    let broken = form.errors.forwards[index].is_some();
                    ui.scope(|ui| {
                        if broken {
                            let stroke = egui::Stroke::new(1.5, theme::red());
                            let widgets = &mut ui.visuals_mut().widgets;
                            widgets.inactive.bg_stroke = stroke;
                            widgets.hovered.bg_stroke = stroke;
                        }
                        ui.add(TextEdit::singleline(&mut row.bind_host).desired_width(86.0).hint_text("escucha"));
                        ui.add(TextEdit::singleline(&mut row.bind_port).desired_width(54.0).hint_text("puerto"));
                        if row.kind == ForwardKind::Dynamic {
                            ui.label(RichText::new("proxy SOCKS5").small().color(theme::subtext()));
                        } else {
                            ui.label("→");
                            ui.add(TextEdit::singleline(&mut row.dest_host).desired_width(86.0).hint_text("destino"));
                            ui.add(TextEdit::singleline(&mut row.dest_port).desired_width(54.0).hint_text("puerto"));
                        }
                    });
                    if ui.add(egui::Button::new(crate::icons::X).frame(false)).on_hover_text("Quitar túnel").clicked() {
                        remove = Some(index);
                    }
                });
                if let Some(message) = &form.errors.forwards[index] {
                    ui.label(RichText::new(message.as_str()).small().color(theme::red()));
                }
            }
            if let Some(index) = remove {
                form.forwards.remove(index);
                form.errors.forwards.remove(index);
            }
            if ui.add(egui::Button::new("+ Añadir túnel").fill(Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, theme::surface()))).clicked() {
                form.forwards.push(ForwardRow::new());
                form.errors.forwards.push(None);
            }
            ui.label(
                RichText::new("-L: este equipo escucha y reenvía al destino vía el servidor. -R: el servidor escucha y reenvía a un destino alcanzable desde aquí. -D: proxy SOCKS5 local que sale por el servidor. Los cambios se aplican al reconectar.")
                    .small()
                    .color(theme::subtext()),
            );
        }
    }

    // Resultado de «Probar conexión» y error al guardar, siempre visibles.
    if let Some(test) = &form.test {
        ui.add_space(ROW_GAP);
        match &*test.lock().unwrap() {
            TestState::Running => {
                ui.label(RichText::new("Probando la conexión…").color(theme::subtext()));
            }
            TestState::Ok(message) => {
                ui.label(RichText::new(message.as_str()).color(theme::identity(3)));
            }
            TestState::Failed(message) => {
                ui.label(RichText::new(message.as_str()).color(theme::red()));
            }
        }
    }
    error_label(ui, &form.errors.general);
    false
}

const SHORTCUTS: [(&str, &[(&str, &str)]); 4] = [
    (
        "Pestañas",
        &[
            ("Ctrl+Shift+P", "Paleta de comandos"),
            ("Alt+1 … 9", "Ir a la pestaña 1 … 9 (Alt+9, la última)"),
            ("Ctrl+Tab / Ctrl+Mayús+Tab", "Pestaña siguiente / anterior"),
            ("Doble clic en la pestaña", "Renombrarla"),
        ],
    ),
    (
        "Terminal",
        &[
            ("Ctrl+Shift+C / V", "Copiar / pegar"),
            ("Ctrl+Shift+F", "Buscar en la salida"),
            ("Ctrl+Shift+D / E", "Dividir a la derecha / hacia abajo"),
            ("Ctrl+Shift+W", "Cerrar el panel"),
            ("Ctrl+= / Ctrl+− / Ctrl+0", "Aumentar / reducir / restablecer la letra"),
        ],
    ),
    (
        "Ventana",
        &[("Ctrl+Shift+B", "Mostrar u ocultar la barra lateral"), ("Ctrl+Shift+K", "Buscar en la barra lateral")],
    ),
    (
        "Explorador de archivos",
        &[
            ("F2", "Renombrar"),
            ("Supr", "Eliminar"),
            ("Enter", "Abrir la carpeta"),
            ("Retroceso", "Subir un nivel"),
            ("F5", "Actualizar"),
            ("Ctrl+A", "Seleccionar todo"),
            ("Ctrl+L", "Escribir una ruta"),
        ],
    ),
];

fn settings_body(ui: &mut egui::Ui, tab: &mut SettingsTab, settings: &mut Settings, master_forced: &mut bool) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        ui.spacing_mut().button_padding.x = 0.0;
        for (choice, label) in [(SettingsTab::General, "General"), (SettingsTab::Atajos, "Atajos")] {
            let selected = *tab == choice;
            let color = if selected { theme::text() } else { theme::subtext() };
            let font = FontId::new(14.0, if selected { crate::ui::strong_family() } else { egui::FontFamily::Proportional });
            let response = ui.add(egui::Button::new(RichText::new(label).font(font).color(color)).fill(Color32::TRANSPARENT));
            if selected {
                let y = response.rect.max.y + 1.0;
                ui.painter().line_segment([egui::pos2(response.rect.min.x, y), egui::pos2(response.rect.max.x, y)], egui::Stroke::new(2.0, theme::accent()));
            }
            if response.clicked() {
                *tab = choice;
            }
        }
    });
    ui.add_space(10.0);

    let height = (ui.ctx().content_rect().height() - 250.0).clamp(220.0, 520.0);
    egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
        let section = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(6.0);
            ui.label(RichText::new(text).small().color(theme::subtext()));
        };
        if *tab == SettingsTab::Atajos {
            for (group, entries) in SHORTCUTS {
                section(ui, group);
                for (keys, what) in entries.iter() {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(egui::vec2(200.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(*keys).color(theme::text()))
                        });
                        ui.label(RichText::new(*what).color(theme::subtext()));
                    });
                }
            }
            return;
        }
        section(ui, "Apariencia");
        ui.horizontal(|ui| {
            ui.label("Tema");
            ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, "Oscuro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::Light, "Claro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::System, "Sistema");
        });
        ui.horizontal(|ui| {
            ui.label("Acento");
            for (choice, label) in AccentChoice::ALL {
                ui.selectable_value(&mut settings.accent, choice, label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Densidad");
            ui.selectable_value(&mut settings.compact, false, "Cómoda");
            ui.selectable_value(&mut settings.compact, true, "Compacta");
        });

        section(ui, "Terminal");
        ui.horizontal(|ui| {
            ui.label("Fuente");
            let name = settings
                .font_path
                .as_deref()
                .and_then(|p| std::path::Path::new(p).file_name())
                .map_or("JetBrains Mono (incluida)".to_string(), |n| n.to_string_lossy().into_owned());
            ui.label(RichText::new(name).color(theme::subtext()));
            if ui.button("Elegir archivo…").clicked() {
                if let Some(file) = rfd::FileDialog::new().add_filter("Fuentes", &["ttf", "otf"]).pick_file() {
                    settings.font_path = Some(file.to_string_lossy().into_owned());
                }
            }
            if settings.font_path.is_some() && ui.button("Restaurar").clicked() {
                settings.font_path = None;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Tamaño de fuente");
            ui.add(egui::Slider::new(&mut settings.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE).step_by(1.0).suffix(" pt"));
        });
        ui.horizontal(|ui| {
            ui.label("Cursor");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Block, "Bloque");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Beam, "Barra");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Underline, "Subrayado");
        });
        ui.checkbox(&mut settings.blink, "Cursor parpadeante").hand();
        ui.horizontal(|ui| {
            ui.label("Historial");
            ui.add(egui::DragValue::new(&mut settings.scrollback).range(MIN_SCROLLBACK..=MAX_SCROLLBACK).speed(100.0).suffix(" líneas"));
        });
        ui.label(RichText::new("El historial se aplica a las sesiones nuevas.").small().color(theme::subtext()));

        section(ui, "Avisos");
        let mut notify = settings.notify_after > 0;
        if ui.checkbox(&mut notify, "Avisar cuando termine la actividad de una pestaña en segundo plano").hand().changed() {
            settings.notify_after = if notify { 15 } else { 0 };
        }
        if notify {
            ui.horizontal(|ui| {
                ui.label("Solo si duró al menos");
                ui.add(egui::DragValue::new(&mut settings.notify_after).range(1..=3600).suffix(" s"));
            });
        }

        section(ui, "Secretos");
        ui.checkbox(master_forced, "Usar siempre la clave maestra, aunque haya keyring").hand();
        ui.label(RichText::new("Este cambio se aplica al reiniciar.").small().color(theme::subtext()));

    });
    false
}

fn snippet_body(ui: &mut egui::Ui, form: &mut SnippetForm) -> bool {
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

fn vars_body(ui: &mut egui::Ui, prompt: &mut VarsPrompt) -> bool {
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
    ui.add_space(4.0);
    ui.label(RichText::new("Resultado").small().color(theme::subtext()));
    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
        ui.label(RichText::new(prompt.rendered()).monospace().color(theme::subtext()));
    });
    submitted
}

fn secret_body(ui: &mut egui::Ui, prompt: &mut SecretPrompt) -> bool {
    let field = secret_field(ui, &mut prompt.input, &mut prompt.reveal, "");
    field.request_focus();
    ui.checkbox(&mut prompt.save, "Guardar en el almacén de secretos").hand();
    ui.label(RichText::new("Si no la guardas, se recuerda hasta cerrar omni-term.").small().color(theme::subtext()));
    entered(ui, &field)
}

fn unlock_body(ui: &mut egui::Ui, prompt: &mut UnlockPrompt) -> bool {
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
        let confirm = ui.add(
            TextEdit::singleline(&mut prompt.confirm)
                .password(!prompt.reveal)
                .hint_text("repite la clave")
                .desired_width(f32::INFINITY),
        );
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

fn prompt_body(ui: &mut egui::Ui, request: &PromptRequest, answers: &mut Vec<String>) -> bool {
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

fn host_key_body(ui: &mut egui::Ui, request: &HostKeyRequest) -> bool {
    ui.label(format!("No se puede verificar la autenticidad de {}:{}.", request.host, request.port));
    ui.add_space(4.0);
    ui.label(RichText::new(format!("{} {}", request.algorithm, request.fingerprint)).monospace());
    ui.add_space(4.0);
    ui.label("Si confías en él, se añadirá a ~/.ssh/known_hosts.");
    false
}

fn host_key_changed_body(ui: &mut egui::Ui, change: &HostKeyChange) -> bool {
    ui.label(
        RichText::new("omni-term no se ha conectado. Puede que alguien se esté haciendo pasar por el servidor, o que el servidor se haya reinstalado.")
            .color(theme::red()),
    );
    ui.add_space(4.0);
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
    ui.add_space(4.0);
    ui.label("Si sabes que el servidor cambió de verdad, borra la huella antigua y vuelve a conectar:");
    ui.label(RichText::new(change.forget_command()).monospace().color(theme::subtext()));
    false
}
