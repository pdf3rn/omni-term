//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use eframe::egui::{self, ComboBox, Id, Modal, RichText, TextEdit};
use zeroize::Zeroizing;

use crate::secrets::SecretKind;
use crate::ssh::HostKeyRequest;
use crate::store::{Auth, Connection, Forward, ForwardKind};
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
        let dest_host = self.dest_host.trim();
        if bind_host.is_empty() || dest_host.is_empty() {
            return Err("Cada túnel necesita dirección de escucha y de destino".into());
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
        }
    }

    pub fn from_connection(connection: &Connection, forwards: &[Forward]) -> Self {
        Self {
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
        })
    }
}

pub struct SecretPrompt {
    pub connection: Connection,
    /// Pestaña a reemplazar al reconectar.
    pub slot: Option<u64>,
    pub kind: SecretKind,
    pub input: String,
    pub save: bool,
}

/// Qué hacer cuando el almacén de secretos queda desbloqueado.
pub enum Pending {
    Open(Connection, Option<u64>),
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
    Edit(ConnForm),
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
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
    let (fill, text) = if destructive { (theme::RED, theme::BASE) } else { (theme::ACCENT, theme::BASE) };
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
        ui.label(RichText::new(error).color(theme::RED));
    }
}

fn enter_pressed(ui: &egui::Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

pub fn show(ctx: &egui::Context, dialog: &mut Dialog, master_forced: bool) -> Outcome {
    let mut outcome = Outcome::None;
    let response = Modal::new(id()).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        match dialog {
            Dialog::Edit(form) => outcome = edit(ui, form),
            Dialog::Secret(prompt) => outcome = secret(ui, prompt),
            Dialog::Unlock(prompt) => outcome = unlock(ui, prompt, master_forced),
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

fn edit(ui: &mut egui::Ui, form: &mut ConnForm) -> Outcome {
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
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut form.auth, Auth::Agent, "ssh-agent");
                ui.selectable_value(&mut form.auth, Auth::Key, "Llave privada");
                ui.selectable_value(&mut form.auth, Auth::Password, "Contraseña");
            });
        ui.end_row();
        if form.auth == Auth::Key {
            ui.label("Llave");
            ui.add(TextEdit::singleline(&mut form.key_path).desired_width(240.0));
            ui.end_row();
        }
        if form.auth != Auth::Agent {
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
    ui.label(RichText::new("Túneles (se activan al conectar)").small().color(theme::SUBTEXT));
    let mut remove = None;
    for (index, row) in form.forwards.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ComboBox::from_id_salt(("fwd-kind", index))
                .width(96.0)
                .selected_text(match row.kind {
                    ForwardKind::Local => "Local -L",
                    ForwardKind::Remote => "Remoto -R",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut row.kind, ForwardKind::Local, "Local -L");
                    ui.selectable_value(&mut row.kind, ForwardKind::Remote, "Remoto -R");
                });
            ui.add(TextEdit::singleline(&mut row.bind_host).desired_width(84.0).hint_text("escucha"));
            ui.add(TextEdit::singleline(&mut row.bind_port).desired_width(48.0).hint_text("puerto"));
            ui.label("→");
            ui.add(TextEdit::singleline(&mut row.dest_host).desired_width(84.0).hint_text("destino"));
            ui.add(TextEdit::singleline(&mut row.dest_port).desired_width(48.0).hint_text("puerto"));
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
        RichText::new("-L: este equipo escucha y reenvía al destino vía el servidor. -R: el servidor escucha y reenvía a un destino alcanzable desde aquí. Los cambios se aplican al reconectar.")
            .small()
            .color(theme::SUBTEXT),
    );
    error_label(ui, &form.error);
    match buttons(ui, "Guardar") {
        Some(true) => Outcome::Save,
        Some(false) => Outcome::Cancel,
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
    ui.label(format!(
        "{label} para {}@{} ({})",
        prompt.connection.user, prompt.connection.host, prompt.connection.name
    ));
    let field = ui.add(TextEdit::singleline(&mut prompt.input).password(true).desired_width(f32::INFINITY));
    field.request_focus();
    ui.checkbox(&mut prompt.save, "Guardar en el almacén de secretos");
    let submitted = field.lost_focus() && enter_pressed(ui);
    match buttons(ui, "Conectar") {
        Some(true) => Outcome::Submit,
        Some(false) => Outcome::Cancel,
        None if submitted => Outcome::Submit,
        None => Outcome::None,
    }
}

fn unlock(ui: &mut egui::Ui, prompt: &mut UnlockPrompt, master_forced: bool) -> Outcome {
    ui.heading(if prompt.creating { "Crear clave maestra" } else { "Desbloquear secretos" });
    ui.add_space(6.0);
    if prompt.creating {
        ui.label("No hay keyring del sistema disponible. Elige una clave maestra para cifrar tus contraseñas.");
        ui.label(RichText::new("Si la olvidas no se puede recuperar: tendrás que borrar los secretos y volver a introducirlos.").color(theme::SUBTEXT));
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
    let _ = master_forced;

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
