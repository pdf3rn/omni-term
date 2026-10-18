//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use eframe::egui::{self, ComboBox, Id, Modal, RichText, TextEdit};
use zeroize::Zeroizing;

use crate::secrets::SecretKind;
use crate::settings::{CursorChoice, Settings, ThemeChoice, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK};
use crate::ssh::{HostKeyRequest, PromptRequest};
use crate::store::{Auth, Connection, Forward, ForwardKind, Snippet};
use crate::theme;

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

pub struct ConnForm {
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
    pub error: Option<String>,
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
            error: None,
            forwards: Vec::new(),
            jump: None,
        }
    }

    pub fn from_connection(connection: &Connection, forwards: &[Forward]) -> Self {
        Self {
            jump: connection.jump,
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
            error: None,
        }
    }

    pub fn to_forwards(&self) -> Result<Vec<Forward>, String> {
        self.forwards.iter().map(ForwardRow::to_forward).collect()
    }

    /// Valida el formulario y construye la conexión.
    pub fn to_connection(&self) -> Result<Connection, String> {
        let name = self.name.trim();
        let host = self.host.trim();
        let user = self.user.trim();
        if name.is_empty() || host.is_empty() || user.is_empty() {
            return Err("Nombre, host y usuario son obligatorios".into());
        }
        let port: u16 = self.port.trim().parse().map_err(|_| "El puerto debe ser un número entre 1 y 65535".to_string())?;
        if port == 0 {
            return Err("El puerto debe ser un número entre 1 y 65535".into());
        }
        let key_path = self.key_path.trim();
        if self.auth == Auth::Key && key_path.is_empty() {
            return Err("Indica la ruta de la llave privada".into());
        }
        Ok(Connection {
            id: self.id,
            name: name.into(),
            host: host.into(),
            port,
            user: user.into(),
            auth: self.auth,
            key_path: (self.auth == Auth::Key).then(|| key_path.to_string()),
            init_command: Some(self.init_command.trim().to_string()).filter(|c| !c.is_empty()),
            jump: self.jump,
        })
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
}

/// Qué hacer cuando el almacén de secretos queda desbloqueado.
pub enum Pending {
    Open(Connection, Option<u64>, Purpose),
    Save { account: String, secret: Zeroizing<String> },
}

pub struct UnlockPrompt {
    pub creating: bool,
    pub input: String,
    pub confirm: String,
    pub error: Option<String>,
    pub then: Pending,
}

pub enum Dialog {
    Settings,
    Edit(ConnForm),
    Snippet(SnippetForm),
    SnippetVars(VarsPrompt),
    ConfirmDeleteSnippet { id: i64, title: String },
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
    Prompt(PromptRequest, Vec<String>),
    ConfirmDelete { id: i64, name: String },
    ConfirmReset(Pending),
    Message(String),
}

pub enum Outcome {
    None,
    Cancel,
    Save,
    Delete,
    Submit,
    Trust(bool),
    ResetMasterKey,
}

fn id() -> Id {
    Id::NULL.with("dialog")
}

fn buttons(ui: &mut egui::Ui, accept: &str) -> Option<bool> {
    let destructive = matches!(accept, "Eliminar" | "Borrar todo");
    let (fill, text) = if destructive { (theme::red(), theme::base()) } else { (theme::accent(), theme::base()) };
    let mut result = None;
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let primary = egui::Button::new(RichText::new(accept).color(text)).fill(fill);
        if ui.add(primary).clicked() {
            result = Some(true);
        }
        let ghost = egui::Button::new("Cancelar").fill(egui::Color32::TRANSPARENT);
        if ui.add(ghost).clicked() {
            result = Some(false);
        }
    });
    result
}

fn error_label(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(error) = error {
        ui.add_space(4.0);
        ui.label(RichText::new(error).color(theme::red()));
    }
}

fn enter_pressed(ui: &egui::Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

pub fn show(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    master_forced: &mut bool,
    connections: &[Connection],
    settings: &mut Settings,
) -> Outcome {
    let mut outcome = Outcome::None;
    let response = Modal::new(id()).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        match dialog {
            Dialog::Settings => outcome = settings_dialog(ui, settings, master_forced),
            Dialog::Edit(form) => outcome = edit(ui, form, connections),
            Dialog::Prompt(request, answers) => outcome = prompt(ui, request, answers),
            Dialog::Snippet(form) => outcome = snippet(ui, form),
            Dialog::SnippetVars(prompt) => outcome = vars(ui, prompt),
            Dialog::ConfirmDeleteSnippet { title, .. } => {
                ui.heading("Eliminar snippet");
                ui.add_space(6.0);
                ui.label(format!("¿Eliminar \"{title}\"?"));
                match buttons(ui, "Eliminar") {
                    Some(true) => outcome = Outcome::Delete,
                    Some(false) => outcome = Outcome::Cancel,
                    None => {}
                }
            }
            Dialog::Secret(prompt) => outcome = secret(ui, prompt),
            Dialog::Unlock(prompt) => outcome = unlock(ui, prompt),
            Dialog::HostKey(request) => outcome = host_key(ui, request),
            Dialog::ConfirmDelete { name, .. } => {
                ui.heading("Eliminar conexión");
                ui.add_space(6.0);
                ui.label(format!("¿Eliminar \"{name}\"? Su contraseña guardada se conserva en el almacén."));
                match buttons(ui, "Eliminar") {
                    Some(true) => outcome = Outcome::Delete,
                    Some(false) => outcome = Outcome::Cancel,
                    None => {}
                }
            }
            Dialog::ConfirmReset(_) => {
                ui.heading("Borrar secretos");
                ui.add_space(6.0);
                ui.label("Se borrarán todas las contraseñas guardadas con la clave maestra y la clave misma. No se puede deshacer.");
                match buttons(ui, "Borrar todo") {
                    Some(true) => outcome = Outcome::Delete,
                    Some(false) => outcome = Outcome::Cancel,
                    None => {}
                }
            }
            Dialog::Message(text) => {
                ui.label(text.as_str());
                ui.add_space(8.0);
                if ui.button("Aceptar").clicked() {
                    outcome = Outcome::Cancel;
                }
            }
        }
    });
    if matches!(outcome, Outcome::None) && response.should_close() && !matches!(dialog, Dialog::HostKey(_)) {
        outcome = Outcome::Cancel;
    }
    outcome
}

fn edit(ui: &mut egui::Ui, form: &mut ConnForm, connections: &[Connection]) -> Outcome {
    ui.heading(if form.id.is_some() { "Editar conexión" } else { "Nueva conexión" });
    ui.add_space(8.0);
    egui::Grid::new("conn_form").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label("Nombre");
        ui.add(TextEdit::singleline(&mut form.name).desired_width(240.0));
        ui.end_row();
        ui.label("Host");
        ui.add(TextEdit::singleline(&mut form.host).desired_width(240.0));
        ui.end_row();
        ui.label("Puerto");
        ui.add(TextEdit::singleline(&mut form.port).desired_width(80.0));
        ui.end_row();
        ui.label("Usuario");
        ui.add(TextEdit::singleline(&mut form.user).desired_width(240.0));
        ui.end_row();
        ui.label("Autenticación");
        ComboBox::from_id_salt("auth")
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
            });
        ui.end_row();
        ui.label("Saltar mediante");
        let current = form
            .jump
            .and_then(|id| connections.iter().find(|c| c.id == Some(id)))
            .map_or("Directo".to_string(), |c| c.name.clone());
        ComboBox::from_id_salt("jump").selected_text(current).show_ui(ui, |ui| {
            ui.selectable_value(&mut form.jump, None, "Directo");
            for other in connections.iter().filter(|c| c.id != form.id) {
                ui.selectable_value(&mut form.jump, other.id, other.name.as_str());
            }
        });
        ui.end_row();
        if form.auth == Auth::Key {
            ui.label("Llave");
            ui.add(TextEdit::singleline(&mut form.key_path).desired_width(240.0));
            ui.end_row();
        }
        if matches!(form.auth, Auth::Password | Auth::Key) {
            ui.label(if form.auth == Auth::Password { "Contraseña" } else { "Passphrase" });
            ui.add(
                TextEdit::singleline(&mut form.secret)
                    .password(true)
                    .hint_text("opcional: se guarda en el almacén")
                    .desired_width(240.0),
            );
            ui.end_row();
        }
        ui.label("Comando inicial");
        ui.add(TextEdit::singleline(&mut form.init_command).hint_text("opcional").desired_width(240.0));
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(RichText::new("Túneles (se activan al conectar)").small().color(theme::subtext()));
    let mut remove = None;
    for (index, row) in form.forwards.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ComboBox::from_id_salt(("fwd-kind", index))
                .width(96.0)
                .selected_text(match row.kind {
                    ForwardKind::Local => "Local -L",
                    ForwardKind::Remote => "Remoto -R",
                    ForwardKind::Dynamic => "SOCKS -D",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut row.kind, ForwardKind::Local, "Local -L");
                    ui.selectable_value(&mut row.kind, ForwardKind::Remote, "Remoto -R");
                    ui.selectable_value(&mut row.kind, ForwardKind::Dynamic, "SOCKS -D");
                });
            ui.add(TextEdit::singleline(&mut row.bind_host).desired_width(84.0).hint_text("escucha"));
            ui.add(TextEdit::singleline(&mut row.bind_port).desired_width(48.0).hint_text("puerto"));
            if row.kind == ForwardKind::Dynamic {
                ui.label(RichText::new("proxy SOCKS5").small().color(theme::subtext()));
            } else {
                ui.label("→");
                ui.add(TextEdit::singleline(&mut row.dest_host).desired_width(84.0).hint_text("destino"));
                ui.add(TextEdit::singleline(&mut row.dest_port).desired_width(48.0).hint_text("puerto"));
            }
            if ui.add(egui::Button::new("×").frame(false)).on_hover_text("Quitar túnel").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        form.forwards.remove(index);
    }
    if ui.add(egui::Button::new("+ Añadir túnel").fill(egui::Color32::TRANSPARENT)).clicked() {
        form.forwards.push(ForwardRow::new());
    }
    ui.label(
        RichText::new("-L: este equipo escucha y reenvía al destino vía el servidor. -R: el servidor escucha y reenvía a un destino alcanzable desde aquí. -D: proxy SOCKS5 local que sale por el servidor. Los cambios se aplican al reconectar.")
            .small()
            .color(theme::subtext()),
    );
    error_label(ui, &form.error);
    match buttons(ui, "Guardar") {
        Some(true) => Outcome::Save,
        Some(false) => Outcome::Cancel,
        None => Outcome::None,
    }
}

fn settings_dialog(ui: &mut egui::Ui, settings: &mut Settings, master_forced: &mut bool) -> Outcome {
    ui.heading("Ajustes");
    ui.add_space(4.0);
    let section = |ui: &mut egui::Ui, text: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(text).small().color(theme::subtext()));
    };

    section(ui, "Apariencia");
    ui.horizontal(|ui| {
        ui.label("Tema");
        ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, "Oscuro");
        ui.selectable_value(&mut settings.theme, ThemeChoice::Light, "Claro");
        ui.selectable_value(&mut settings.theme, ThemeChoice::System, "Sistema");
    });
    ui.horizontal(|ui| {
        ui.label("Tamaño de fuente");
        ui.add(egui::Slider::new(&mut settings.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE).step_by(1.0).suffix(" pt"));
    });

    section(ui, "Terminal");
    ui.horizontal(|ui| {
        ui.label("Cursor");
        ui.selectable_value(&mut settings.cursor, CursorChoice::Block, "Bloque");
        ui.selectable_value(&mut settings.cursor, CursorChoice::Beam, "Barra");
        ui.selectable_value(&mut settings.cursor, CursorChoice::Underline, "Subrayado");
    });
    ui.checkbox(&mut settings.blink, "Cursor parpadeante");
    ui.horizontal(|ui| {
        ui.label("Historial");
        ui.add(egui::DragValue::new(&mut settings.scrollback).range(MIN_SCROLLBACK..=MAX_SCROLLBACK).speed(100.0).suffix(" líneas"));
    });
    ui.label(RichText::new("El historial se aplica a las sesiones nuevas.").small().color(theme::subtext()));

    section(ui, "Secretos");
    ui.checkbox(master_forced, "Usar siempre la clave maestra, aunque haya keyring");
    ui.label(RichText::new("Este cambio se aplica al reiniciar.").small().color(theme::subtext()));

    ui.add_space(10.0);
    if ui.add(egui::Button::new(RichText::new("Cerrar").color(theme::base())).fill(theme::accent())).clicked() {
        return Outcome::Cancel;
    }
    Outcome::None
}

fn snippet(ui: &mut egui::Ui, form: &mut SnippetForm) -> Outcome {
    ui.heading(if form.id.is_some() { "Editar snippet" } else { "Nuevo snippet" });
    ui.add_space(8.0);
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
    match buttons(ui, "Guardar") {
        Some(true) => Outcome::Save,
        Some(false) => Outcome::Cancel,
        None => Outcome::None,
    }
}

fn vars(ui: &mut egui::Ui, prompt: &mut VarsPrompt) -> Outcome {
    ui.heading(prompt.title.clone());
    ui.add_space(6.0);
    let mut submitted = false;
    egui::Grid::new("snippet_vars").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        for (index, (name, value)) in prompt.fields.iter_mut().enumerate() {
            ui.label(name.as_str());
            let field = ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
            if index == 0 {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submitted = true;
            }
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    ui.label(RichText::new("Resultado").small().color(theme::subtext()));
    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
        ui.label(RichText::new(prompt.rendered()).monospace().color(theme::subtext()));
    });
    let accept = if prompt.run { "Insertar y ejecutar" } else { "Insertar" };
    match buttons(ui, accept) {
        Some(true) => Outcome::Submit,
        Some(false) => Outcome::Cancel,
        None if submitted => Outcome::Submit,
        None => Outcome::None,
    }
}

fn secret(ui: &mut egui::Ui, prompt: &mut SecretPrompt) -> Outcome {
    let (title, label) = match prompt.kind {
        SecretKind::Password => ("Contraseña", "Contraseña"),
        SecretKind::Passphrase => ("Llave protegida", "Passphrase"),
    };
    ui.heading(title);
    ui.add_space(6.0);
    ui.label(format!("{label} para {}@{} ({})", prompt.asking.user, prompt.asking.host, prompt.asking.name));
    let field = ui.add(TextEdit::singleline(&mut prompt.input).password(true).desired_width(f32::INFINITY));
    field.request_focus();
    ui.checkbox(&mut prompt.save, "Guardar en el almacén de secretos");
    ui.label(RichText::new("Si no la guardas, se recuerda hasta cerrar omni-term.").small().color(theme::subtext()));
    let submitted = field.lost_focus() && enter_pressed(ui);
    match buttons(ui, "Conectar") {
        Some(true) => Outcome::Submit,
        Some(false) => Outcome::Cancel,
        None if submitted => Outcome::Submit,
        None => Outcome::None,
    }
}

fn unlock(ui: &mut egui::Ui, prompt: &mut UnlockPrompt) -> Outcome {
    ui.heading(if prompt.creating { "Crear clave maestra" } else { "Desbloquear secretos" });
    ui.add_space(6.0);
    if prompt.creating {
        ui.label("No hay keyring del sistema disponible. Elige una clave maestra para cifrar tus contraseñas.");
        ui.label(RichText::new("Si la olvidas no se puede recuperar: tendrás que borrar los secretos y volver a introducirlos.").color(theme::subtext()));
    } else {
        ui.label("Introduce tu clave maestra.");
    }
    let field = ui.add(TextEdit::singleline(&mut prompt.input).password(true).hint_text("clave maestra").desired_width(f32::INFINITY));
    field.request_focus();
    let mut submitted = field.lost_focus() && enter_pressed(ui);
    if prompt.creating {
        ui.add(TextEdit::singleline(&mut prompt.confirm).password(true).hint_text("repite la clave").desired_width(f32::INFINITY));
    }
    error_label(ui, &prompt.error);

    let mut outcome = Outcome::None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(if prompt.creating { "Crear" } else { "Desbloquear" }).clicked() {
            submitted = true;
        }
        if ui.button("Cancelar").clicked() {
            outcome = Outcome::Cancel;
        }
        if !prompt.creating && ui.button("Olvidé la clave…").clicked() {
            outcome = Outcome::ResetMasterKey;
        }
    });
    if submitted && matches!(outcome, Outcome::None) {
        outcome = Outcome::Submit;
    }
    outcome
}

fn prompt(ui: &mut egui::Ui, request: &PromptRequest, answers: &mut Vec<String>) -> Outcome {
    answers.resize(request.prompts.len(), String::new());
    ui.heading(if request.title.trim().is_empty() { "Verificación en dos pasos" } else { request.title.trim() });
    if !request.instructions.trim().is_empty() {
        ui.label(RichText::new(request.instructions.trim()).color(theme::subtext()));
    }
    let mut submitted = false;
    for (index, ((question, echo), answer)) in request.prompts.iter().zip(answers.iter_mut()).enumerate() {
        ui.label(question.trim());
        let field = ui.add(TextEdit::singleline(answer).password(!echo).desired_width(f32::INFINITY));
        if index == 0 {
            field.request_focus();
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submitted = true;
        }
    }
    match buttons(ui, "Continuar") {
        Some(true) => Outcome::Submit,
        Some(false) => Outcome::Cancel,
        None if submitted => Outcome::Submit,
        None => Outcome::None,
    }
}

fn host_key(ui: &mut egui::Ui, request: &HostKeyRequest) -> Outcome {
    ui.heading("Servidor desconocido");
    ui.add_space(6.0);
    ui.label(format!("No se puede verificar la autenticidad de {}:{}.", request.host, request.port));
    ui.add_space(4.0);
    ui.label(RichText::new(format!("{} {}", request.algorithm, request.fingerprint)).monospace());
    ui.add_space(4.0);
    ui.label("Si confías en él, se añadirá a ~/.ssh/known_hosts.");
    match buttons(ui, "Confiar y conectar") {
        Some(true) => Outcome::Trust(true),
        Some(false) => Outcome::Trust(false),
        None => Outcome::None,
    }
}
