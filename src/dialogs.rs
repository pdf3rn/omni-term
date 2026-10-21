//! Diálogos modales: edición de conexión, contraseñas, clave maestra y confianza de host.

use eframe::egui::{self, CollapsingHeader, Color32, ComboBox, FontId, Id, Modal, RichText, ScrollArea, TextEdit};
use zeroize::Zeroizing;

use crate::cursor::HandExt;
use crate::secrets::SecretKind;
use crate::settings::{AccentChoice, CursorChoice, Settings, ThemeChoice, MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK};
use crate::ssh::{HostKeyChange, HostKeyRequest, PromptRequest};
use crate::store::{Encoding, FilesKind, FtpOptions, FtpTls, Flow, Newline, Parity, Protocol, SerialOptions, TelnetOptions, BAUD_RATES, Auth, Connection, Forward, ForwardKind, Snippet};
use crate::theme;

/// Pide desplazarse hasta el primer campo con error en el próximo dibujo del formulario.
static REVEAL_ERROR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

impl ForwardRow {
    /// Un túnel `-L` o `-D` que escucha fuera de este equipo lo puede usar cualquiera de la red.
    fn listens_on_network(&self) -> bool {
        let host = self.bind_host.trim().trim_matches(['[', ']']);
        self.kind != ForwardKind::Remote
            && !host.is_empty()
            && !host.eq_ignore_ascii_case("localhost")
            && host != "::1"
            && !host.starts_with("127.")
    }
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
    pub share: Option<String>,
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

pub type AdbDevices = std::sync::Arc<std::sync::Mutex<Option<Result<Vec<crate::adb::Device>, String>>>>;

pub struct ConnForm {
    /// 0 no es de archivos; 1 SFTP; 2 FTP; 3 SMB.
    pub files: u8,
    pub ftp_tls: FtpTls,
    pub ftp_passive: bool,
    pub ftp_anonymous: bool,
    pub ftp_quiet: bool,
    pub smb_share: String,
    pub smb_domain: String,
    /// Dispositivo Android por `adb`; el número de serie (o `ip:puerto`) va en `host`.
    pub adb: bool,
    /// Resultado de la última búsqueda de dispositivos, que corre en segundo plano.
    pub adb_devices: Option<AdbDevices>,
    /// Conexión por puerto serie; el puerto va en `host`.
    pub serial: bool,
    pub baud: String,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow: Flow,
    pub newline: Newline,
    pub local_echo: bool,
    /// Puertos detectados la última vez que se pulsó «Detectar».
    pub ports: Vec<(String, String)>,
    /// Conexión Telnet en lugar de SSH.
    pub telnet: bool,
    pub negotiate: bool,
    pub encoding: Encoding,
    /// Se conserva el «no avisar» de la conexión que se edita.
    pub quiet: bool,
    /// Pide el foco para «Nombre» la primera vez que se dibuja.
    pub focus: bool,
    /// Muestra la etiqueta y los colores a mano en vez de los atajos de entorno.
    pub custom_env: bool,
    /// «Avanzado» desplegado.
    pub advanced_open: bool,
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
        let serial = SerialOptions::default();
        Self {
            files: 0,
            ftp_tls: FtpTls::None,
            ftp_passive: true,
            ftp_anonymous: false,
            ftp_quiet: false,
            smb_share: String::new(),
            smb_domain: String::new(),
            adb: false,
            adb_devices: None,
            serial: false,
            baud: serial.baud.to_string(),
            data_bits: serial.data_bits,
            parity: serial.parity,
            stop_bits: serial.stop_bits,
            flow: serial.flow,
            newline: serial.newline,
            local_echo: serial.local_echo,
            ports: Vec::new(),
            telnet: false,
            negotiate: true,
            encoding: Encoding::Utf8,
            quiet: false,
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
            advanced_open: false,
            errors: FormErrors::default(),
            test: None,
            forwards: Vec::new(),
            jump: None,
            color: None,
            env: String::new(),
        }
    }

    pub fn is_ssh(&self) -> bool {
        !self.telnet && !self.serial && !self.adb && self.files == 0
    }

    /// Usa los campos de SSH (usuario, acceso, llave, salto): SSH y SFTP.
    pub fn ssh_like(&self) -> bool {
        self.is_ssh() || self.files == 1
    }

    pub fn from_connection(connection: &Connection, forwards: &[Forward]) -> Self {
        let env = connection.env.clone().unwrap_or_default();
        let known = ENVIRONMENTS.iter().any(|(_, name, color)| env.eq_ignore_ascii_case(name) && connection.color == Some(*color));
        let custom_env = !known && (!env.is_empty() || connection.color.is_some());
        let options = match connection.protocol {
            Protocol::Telnet(options) => Some(options),
            _ => None,
        };
        let serial = match connection.protocol {
            Protocol::Serial(options) => Some(options),
            _ => None,
        };
        let serial_defaults = serial.unwrap_or_default();
        let (files, ftp) = match connection.protocol {
            Protocol::Files(FilesKind::Sftp) => (1, FtpOptions::default()),
            Protocol::Files(FilesKind::Ftp(options)) => (2, options),
            Protocol::Files(FilesKind::Smb) => (3, FtpOptions::default()),
            _ => (0, FtpOptions::default()),
        };
        Self {
            files,
            ftp_tls: ftp.tls,
            ftp_passive: ftp.passive,
            ftp_anonymous: ftp.anonymous,
            ftp_quiet: ftp.quiet,
            smb_share: if files == 3 { connection.smb_share().to_string() } else { String::new() },
            smb_domain: if files == 3 { connection.smb_domain().to_string() } else { String::new() },
            adb: connection.is_adb(),
            adb_devices: None,
            serial: serial.is_some(),
            baud: serial_defaults.baud.to_string(),
            data_bits: serial_defaults.data_bits,
            parity: serial_defaults.parity,
            stop_bits: serial_defaults.stop_bits,
            flow: serial_defaults.flow,
            newline: serial_defaults.newline,
            local_echo: serial_defaults.local_echo,
            ports: Vec::new(),
            telnet: options.is_some(),
            negotiate: options.map_or(true, |o| o.negotiate),
            encoding: options.map_or(Encoding::Utf8, |o| o.encoding),
            quiet: options.is_some_and(|o| o.quiet),
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
            advanced_open: false,
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
        if host.is_empty() && !self.adb {
            errors.host = Some(if self.serial { "Indica el puerto, por ejemplo /dev/ttyUSB0" } else { "Indica el servidor" }.into());
        }
        let mut baud = 0;
        if self.serial {
            match self.baud.trim().parse::<u32>() {
                Ok(value) if value > 0 => baud = value,
                _ => errors.port = Some("Una velocidad mayor que 0".into()),
            }
        }
        let user_needed = self.ssh_like() || self.files == 3 || (self.files == 2 && !self.ftp_anonymous);
        if user.is_empty() && user_needed {
            errors.user = Some("Indica el usuario".into());
        }
        if self.files == 3 && self.smb_share.trim().trim_matches(['\\', '/']).is_empty() {
            errors.share = Some("Indica el recurso compartido".into());
        }
        let port = if self.serial || self.adb {
            0
        } else {
            match self.port.trim().parse::<u16>() {
                Ok(port) if port > 0 => port,
                _ => {
                    errors.port = Some("Un número entre 1 y 65535".into());
                    0
                }
            }
        };
        let key_path = self.key_path.trim().to_string();
        if self.auth == Auth::Key && key_path.is_empty() {
            errors.key_path = Some("Indica la ruta de la llave privada".into());
        }
        let mut forwards = Vec::new();
        for row in self.forwards.iter().filter(|_| self.is_ssh()) {
            match row.to_forward() {
                Ok(forward) => {
                    forwards.push(forward);
                    errors.forwards.push(None);
                }
                Err(message) => errors.forwards.push(Some(message)),
            }
        }

        let has_error = [&errors.name, &errors.host, &errors.port, &errors.user, &errors.key_path, &errors.share].iter().any(|e| e.is_some())
            || errors.forwards.iter().any(Option::is_some);
        if has_error {
            self.advanced_open |= errors.forwards.iter().any(Option::is_some);
            REVEAL_ERROR.store(true, std::sync::atomic::Ordering::Relaxed);
            self.errors = errors;
            return None;
        }
        self.errors = FormErrors::default();
        let telnet = self.telnet || self.serial || self.adb || self.files >= 2;
        let anonymous = self.files == 2 && self.ftp_anonymous;
        let connection = Connection {
            id: self.id,
            protocol: if self.files > 0 {
                Protocol::Files(match self.files {
                    1 => FilesKind::Sftp,
                    2 => FilesKind::Ftp(FtpOptions { tls: self.ftp_tls, passive: self.ftp_passive, anonymous: self.ftp_anonymous, quiet: self.ftp_quiet }),
                    _ => FilesKind::Smb,
                })
            } else if self.adb {
                Protocol::Adb
            } else if self.serial {
                Protocol::Serial(SerialOptions {
                    baud,
                    data_bits: self.data_bits,
                    parity: self.parity,
                    stop_bits: self.stop_bits,
                    flow: self.flow,
                    newline: self.newline,
                    local_echo: self.local_echo,
                })
            } else if self.telnet {
                Protocol::Telnet(TelnetOptions { negotiate: self.negotiate, encoding: self.encoding, quiet: self.quiet })
            } else {
                Protocol::Ssh
            },
            name,
            host,
            port,
            user: if anonymous { "anonymous".into() } else if self.telnet || self.serial || self.adb { String::new() } else { user },
            // FTP y SMB piden la contraseña al conectar; el anónimo no la pide.
            auth: match self.files {
                2 if anonymous => Auth::Agent,
                2 | 3 => Auth::Password,
                _ if telnet => Auth::Agent,
                _ => self.auth,
            },
            key_path: if self.files == 3 {
                Some(self.smb_domain.trim().to_string()).filter(|d| !d.is_empty())
            } else {
                (!telnet && self.auth == Auth::Key).then_some(key_path)
            },
            init_command: if self.files == 3 {
                Some(self.smb_share.trim().trim_matches(['\\', '/']).to_string())
            } else {
                Some(self.init_command.trim().to_string()).filter(|c| !c.is_empty() && !telnet)
            },
            jump: if telnet { None } else { self.jump },
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Atajos,
    Hosts,
}

/// Qué se cierra cuando se confirma (las pestañas se identifican por su id, no por su posición).
#[derive(Clone, Copy)]
pub enum CloseAction {
    Tab(u64),
    Others(u64),
    Right(u64),
    Window,
}

pub struct CloseConfirm {
    pub action: CloseAction,
    pub lines: Vec<String>,
    pub dont_ask: bool,
}

/// Lista de `known_hosts` de la página de ajustes, leída una vez y recargada al cambiar.
#[derive(Default)]
pub struct KnownHostsView {
    entries: Vec<crate::known_hosts::KnownHost>,
    pub query: String,
    pub loaded: bool,
    error: Option<String>,
    /// Entrada que el usuario pidió olvidar; la procesa la aplicación.
    pub delete: Option<crate::known_hosts::KnownHost>,
}

impl KnownHostsView {
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        match crate::known_hosts::load(&crate::ssh::default_known_hosts()) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.error = Some(format!("No se pudo leer known_hosts: {error}"));
            }
        }
    }
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

pub enum Dialog {
    Error(ErrorInfo),
    HostKeyChanged(HostKeyChange),
    About,
    ConfirmClose(CloseConfirm),
    TelnetWarning(TelnetWarning),
    Edit(ConnForm),
    Snippet(SnippetForm),
    SnippetVars(VarsPrompt),
    Secret(SecretPrompt),
    Unlock(UnlockPrompt),
    HostKey(HostKeyRequest),
    Prompt(PromptRequest, Vec<String>),
    ConfirmReset(Pending),
    ConfirmContainer(ContainerConfirm),
}

/// Detener o reiniciar un contenedor, pendiente de confirmar.
pub struct ContainerConfirm {
    pub endpoint: crate::docker::Endpoint,
    pub container: crate::docker::Container,
    pub action: crate::docker::Action,
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
/// Icono de estado a la izquierda del cuerpo en confirmaciones, errores y avisos.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogIcon {
    Warning,
    Error,
    Info,
}

pub struct Spec {
    icon: Option<DialogIcon>,
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
    status: Option<(String, Color32)>,
}

impl Spec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            icon: None,
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
            status: None,
        }
    }

    pub fn icon(mut self, icon: DialogIcon) -> Self {
        self.icon = Some(icon);
        self
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

    /// Una línea de estado sobre el pie (resultado de una prueba, error al guardar).
    pub fn status(mut self, text: impl Into<String>, color: Color32) -> Self {
        self.status = Some((text.into(), color));
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
    let strip = if theme::is_light() { theme::surface() } else { theme::base() };
    let frame = egui::Frame::popup(&ctx.global_style())
        .inner_margin(egui::Margin::ZERO)
        .corner_radius(egui::CornerRadius::same(6));
    let icon = spec.icon.or((spec.tone == Tone::Danger).then_some(DialogIcon::Warning));
    let response = Modal::new(id()).frame(frame).show(ctx, |ui| {
        ui.set_width(spec.width);
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

        // Franja del título, con la X para cerrar.
        let (title_rect, _) = ui.allocate_exact_size(egui::vec2(spec.width, 38.0), egui::Sense::hover());
        ui.painter().rect_filled(title_rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, strip);
        ui.painter().line_segment([title_rect.left_bottom(), title_rect.right_bottom()], egui::Stroke::new(1.0, theme::surface()));
        ui.painter().text(title_rect.center(), egui::Align2::CENTER_CENTER, &spec.title, FontId::new(14.0, crate::ui::strong_family()), theme::text());
        let close_rect = egui::Rect::from_center_size(egui::pos2(title_rect.max.x - 22.0, title_rect.center().y), egui::vec2(26.0, 26.0));
        let close = ui.interact(close_rect, ui.auto_id_with("dialog-close"), egui::Sense::click()).hand();
        if close.hovered() {
            ui.painter().rect_filled(close_rect, egui::CornerRadius::same(6), theme::selection());
        }
        ui.painter().text(close_rect.center(), egui::Align2::CENTER_CENTER, crate::icons::X, FontId::proportional(14.0), theme::text());
        if close.on_hover_text("Cerrar").clicked() {
            action = Action::Cancel;
        }

        let mut enter = false;
        egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 18, top: 16, bottom: 18 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            if let Some(context) = &spec.context {
                ui.label(RichText::new(context).small().color(theme::subtext()));
            }
            let mut body = Some(body);
            let mut content = |ui: &mut egui::Ui| -> bool {
                let body = body.take().expect("el cuerpo se dibuja una vez");
                match icon {
                    Some(icon) => {
                        let mut enter = false;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (glyph, color) = match icon {
                                DialogIcon::Warning => (crate::icons::WARNING, theme::identity(1)),
                                DialogIcon::Error => (crate::icons::ERROR, theme::red()),
                                DialogIcon::Info => (crate::icons::INFO, theme::accent()),
                            };
                            ui.add_sized([26.0, 26.0], egui::Label::new(RichText::new(glyph).size(24.0).color(color)));
                            ui.vertical(|ui| enter = body(ui));
                        });
                        enter
                    }
                    None => body(ui),
                }
            };
            enter = content(ui);
        });

        if let Some((text, color)) = &spec.status {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 18, top: 0, bottom: 10 }).show(ui, |ui| {
                ui.label(RichText::new(text.as_str()).small().color(*color));
            });
        }

        // Franja de botones.
        let (foot_rect, _) = ui.allocate_exact_size(egui::vec2(spec.width, 56.0), egui::Sense::hover());
        ui.painter().rect_filled(foot_rect, egui::CornerRadius { nw: 0, ne: 0, sw: 6, se: 6 }, strip);
        ui.painter().line_segment([foot_rect.left_top(), foot_rect.right_top()], egui::Stroke::new(1.0, theme::surface()));
        let mut foot = ui.new_child(egui::UiBuilder::new().max_rect(foot_rect.shrink2(egui::vec2(14.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        foot.spacing_mut().item_spacing.x = 8.0;
        let size = egui::vec2(0.0, 30.0);
        for (index, label) in spec.extras.iter().enumerate() {
            let extra = egui::Button::new(label.as_str()).min_size(size);
            if crate::ui::outline_add(&mut foot, extra, false).clicked() {
                action = Action::Extra(index);
            }
        }
        foot.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let fill = if spec.tone == Tone::Danger { theme::red() } else { theme::accent() };
            let primary = |label: &str| egui::Button::new(RichText::new(label).color(theme::base())).min_size(size);
            match &spec.accept {
                Some(label) => {
                    if ui.add_enabled_ui(spec.accept_enabled, |ui| crate::ui::solid_add(ui, primary(label), fill)).inner.clicked() {
                        action = Action::Accept;
                    }
                    let cancel = egui::Button::new(spec.cancel.as_str()).min_size(size);
                    if crate::ui::outline_add(ui, cancel, false).clicked() {
                        action = Action::Cancel;
                    }
                }
                None => {
                    if crate::ui::solid_add(ui, primary(&spec.cancel), fill).clicked() {
                        action = Action::Cancel;
                    }
                }
            }
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

pub const MIN_MASTER_KEY: usize = 8;

/// Campo de contraseña con botón para mostrar u ocultar lo escrito.
fn secret_field(ui: &mut egui::Ui, value: &mut String, reveal: &mut bool, hint: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 76.0).max(80.0);
        let field = ui.add(TextEdit::singleline(value).password(!*reveal).hint_text(hint).desired_width(width));
        let label = if *reveal { "Ocultar" } else { "Mostrar" };
        if ui.add(egui::Button::new(RichText::new(label).small()).frame_when_inactive(false)).clicked() {
            *reveal = !*reveal;
        }
        field
    })
    .inner
}

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
                ui.add_space(4.0);
                ui.checkbox(&mut confirm.dont_ask, "No volver a preguntar").hand();
                false
            });
            plain(action, Outcome::Delete)
        }
        Dialog::TelnetWarning(warning) => {
            let spec = Spec::new("Conexión sin cifrar").width(440.0).accept("Conectar").icon(DialogIcon::Warning).no_enter();
            let action = shell(ctx, spec, |ui| {
                ui.label(format!("«{}» usa {}: todo lo que envíes, contraseñas incluidas, viaja en texto claro y cualquiera en la red puede leerlo.", warning.connection.name, warning.label));
                ui.label(RichText::new("Úsalo solo en redes de confianza o con equipos que no admiten SSH.").color(theme::subtext()));
                ui.add_space(4.0);
                ui.checkbox(&mut warning.dont_ask, "No volver a avisar para esta conexión").hand();
                false
            });
            plain(action, Outcome::Submit)
        }
        Dialog::About => {
            let action = shell(ctx, Spec::new("Acerca de omni-term").width(420.0).cancel("Cerrar"), |ui| {
                ui.label(RichText::new(format!("omni-term {}", env!("CARGO_PKG_VERSION"))).font(FontId::new(16.0, crate::ui::strong_family())));
                ui.label(RichText::new("Terminal con SSH, SFTP, túneles y snippets, hecho con Rust y egui.").color(theme::subtext()));
                ui.add_space(8.0);
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
            let mut spec = Spec::new(title).width(700.0).accept("Guardar").no_enter();
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
            use crate::docker::Action;
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

const LABEL_WIDTH: f32 = 112.0;
const FIELD_HEIGHT: f32 = 28.0;
/// Espacio entre filas de un mismo grupo y entre grupos distintos.
const ROW_GAP: f32 = 8.0;
const GROUP_GAP: f32 = 18.0;

/// Fila de formulario: etiqueta alineada a la derecha y contenido que ocupa el resto.
fn form_row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, FIELD_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL_WIDTH);
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
    if broken && REVEAL_ERROR.swap(false, std::sync::atomic::Ordering::Relaxed) {
        response.scroll_to_me(Some(egui::Align::Center));
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

/// El puerto por defecto sigue al protocolo mientras no se haya tocado.
fn set_default_port(form: &mut ConnForm) {
    let target = match (form.telnet, form.files) {
        (true, _) => "23",
        (_, 2) if form.ftp_tls == FtpTls::Implicit => "990",
        (_, 2) => "21",
        (_, 3) => "445",
        _ => "22",
    };
    if matches!(form.port.trim(), "21" | "22" | "23" | "445" | "990") {
        form.port = target.into();
    }
}

/// Filas propias de FTP (cuenta y seguridad) y de SMB (recurso compartido, cuenta y dominio).
fn files_rows(ui: &mut egui::Ui, form: &mut ConnForm) {
    if form.files == 3 {
        ui.add_space(ROW_GAP);
        form_row(ui, "Recurso", |ui| {
            let width = ui.available_width();
            text_input(ui, &mut form.smb_share, width, "nombre del recurso compartido", &mut form.errors.share);
            error_line(ui, &form.errors.share);
        });
    }
    if form.files == 2 {
        ui.add_space(ROW_GAP);
        form_row(ui, "Cuenta", |ui| {
            ui.checkbox(&mut form.ftp_anonymous, "Acceso anónimo").hand();
        });
    }
    if form.files == 3 || !form.ftp_anonymous {
        ui.add_space(ROW_GAP);
        form_row(ui, "Usuario", |ui| {
            let width = ui.available_width();
            text_input(ui, &mut form.user, width, "usuario", &mut form.errors.user);
            error_line(ui, &form.errors.user);
        });
    }
    if form.files == 3 {
        ui.add_space(ROW_GAP);
        form_row(ui, "Dominio", |ui| {
            let width = ui.available_width();
            ui.add_sized([width, FIELD_HEIGHT], TextEdit::singleline(&mut form.smb_domain).hint_text("opcional").align(egui::Align2::LEFT_CENTER));
        });
    } else {
        ui.add_space(ROW_GAP);
        form_row(ui, "Seguridad", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tls, label) in [(FtpTls::None, "Sin cifrar"), (FtpTls::Explicit, "FTPS explícito"), (FtpTls::Implicit, "FTPS implícito")] {
                    if ui.selectable_label(form.ftp_tls == tls, label).hand().clicked() && form.ftp_tls != tls {
                        form.ftp_tls = tls;
                        set_default_port(form);
                    }
                }
            });
        });
        ui.add_space(ROW_GAP);
        form_row(ui, "Modo", |ui| {
            ui.checkbox(&mut form.ftp_passive, "Pasivo (recomendado tras un cortafuegos)").hand();
        });
        if form.ftp_tls == FtpTls::None {
            ui.add_space(ROW_GAP);
            form_row(ui, "", |ui| {
                ui.label(RichText::new("FTP sin cifrar: la contraseña y los archivos viajan en texto claro.").small().color(theme::red()));
            });
        }
    }
}

/// Filas del formulario de una conexión serie: puerto, velocidad, formato, flujo, Enter y eco.
fn adb_rows(ui: &mut egui::Ui, form: &mut ConnForm) {
    form_row(ui, "Dispositivo", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let width = (ui.available_width() - 90.0 - 8.0).max(120.0);
            text_input(ui, &mut form.host, width, "vacío: el único conectado", &mut form.errors.host);
            let detect = crate::ui::outline_add(ui, egui::Button::new("Detectar").min_size(egui::vec2(90.0, FIELD_HEIGHT)), false);
            if detect.clicked() {
                let slot: AdbDevices = std::sync::Arc::new(std::sync::Mutex::new(None));
                let result = slot.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    *result.lock().unwrap() = Some(crate::adb::detect());
                    ctx.request_repaint();
                });
                form.adb_devices = Some(slot);
            }
            egui::Popup::menu(&detect).show(|ui| {
                ui.set_min_width(300.0);
                let found = form.adb_devices.as_ref().map(|slot| slot.lock().unwrap().clone());
                match found {
                    None | Some(None) => {
                        ui.label(RichText::new("Buscando dispositivos…").color(theme::subtext()));
                    }
                    Some(Some(Err(error))) => {
                        ui.label(RichText::new(error).color(theme::red()));
                    }
                    Some(Some(Ok(devices))) if devices.is_empty() => {
                        ui.label(RichText::new("No hay dispositivos. Activa la depuración USB y enchufa el móvil.").color(theme::subtext()));
                    }
                    Some(Some(Ok(devices))) => {
                        for device in devices {
                            let label = if device.description.is_empty() { device.serial.clone() } else { format!("{}   {}", device.serial, device.description) };
                            let label = if device.state == "device" { label } else { format!("{label}   ({})", device.state) };
                            if ui.button(label).clicked() {
                                form.host = device.serial;
                                ui.close();
                            }
                        }
                    }
                }
            });
        });
        error_line(ui, &form.errors.host);
        ui.label(RichText::new("Para un móvil por Wi-Fi escribe ip:puerto (por ejemplo 192.168.1.5:5555).").small().color(theme::subtext()));
    });
}

fn serial_rows(ui: &mut egui::Ui, form: &mut ConnForm) {
    form_row(ui, "Puerto", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let width = (ui.available_width() - 90.0 - 8.0).max(120.0);
            text_input(ui, &mut form.host, width, if cfg!(windows) { "COM3" } else { "/dev/ttyUSB0" }, &mut form.errors.host);
            let detect = crate::ui::outline_add(ui, egui::Button::new("Detectar").min_size(egui::vec2(90.0, FIELD_HEIGHT)), false);
            if detect.clicked() {
                form.ports = crate::serial::detect_ports();
            }
            egui::Popup::menu(&detect).show(|ui| {
                ui.set_min_width(260.0);
                if form.ports.is_empty() {
                    ui.label(RichText::new("No se detectó ningún puerto.").color(theme::subtext()));
                }
                for (path, description) in form.ports.clone() {
                    let label = if description.is_empty() { path.clone() } else { format!("{path}   {description}") };
                    if ui.button(label).clicked() {
                        form.host = path;
                        ui.close();
                    }
                }
            });
        });
        error_line(ui, &form.errors.host);
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "Velocidad", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            text_input(ui, &mut form.baud, 100.0, "115200", &mut form.errors.port);
            let speeds = crate::ui::outline_add(ui, egui::Button::new(format!("{}  baudios", crate::icons::CARET_DOWN)).min_size(egui::vec2(0.0, FIELD_HEIGHT)), false);
            egui::Popup::menu(&speeds).show(|ui| {
                for rate in BAUD_RATES {
                    if ui.button(rate.to_string()).clicked() {
                        form.baud = rate.to_string();
                        ui.close();
                    }
                }
            });
        });
        error_line(ui, &form.errors.port);
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "Formato", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for bits in [5u8, 6, 7, 8] {
                ui.selectable_value(&mut form.data_bits, bits, bits.to_string());
            }
            ui.add_space(8.0);
            for (parity, label) in [(Parity::None, "Sin paridad"), (Parity::Odd, "Impar"), (Parity::Even, "Par")] {
                ui.selectable_value(&mut form.parity, parity, label);
            }
            ui.add_space(8.0);
            for stop in [1u8, 2] {
                ui.selectable_value(&mut form.stop_bits, stop, format!("{stop} parada"));
            }
        });
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "Flujo", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (flow, label) in [(Flow::None, "Ninguno"), (Flow::Software, "XON/XOFF"), (Flow::Hardware, "RTS/CTS")] {
                ui.selectable_value(&mut form.flow, flow, label);
            }
        });
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "Enter envía", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (newline, label) in Newline::ALL {
                ui.selectable_value(&mut form.newline, newline, label);
            }
            ui.add_space(12.0);
            ui.checkbox(&mut form.local_echo, "Eco local").hand();
        });
    });
}

/// Tipos de conexión del selector: añadir uno nuevo es una fila aquí y sus brazos en `ConnKind::of` y `apply`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnKind {
    Ssh,
    Telnet,
    Serial,
    Sftp,
    Ftp,
    Smb,
    Adb,
}

impl ConnKind {
    /// (tipo, nombre, grupo): el selector pone un encabezado cada vez que cambia el grupo.
    const ALL: [(ConnKind, &'static str, &'static str); 7] = [
        (ConnKind::Ssh, "SSH", "Terminal"),
        (ConnKind::Telnet, "Telnet", "Terminal"),
        (ConnKind::Serial, "Puerto serie", "Terminal"),
        (ConnKind::Sftp, "SFTP", "Archivos"),
        (ConnKind::Ftp, "FTP / FTPS", "Archivos"),
        (ConnKind::Smb, "SMB", "Archivos"),
        (ConnKind::Adb, "Android (ADB)", "Otros"),
    ];

    fn of(form: &ConnForm) -> Self {
        match (form.files, form.adb, form.serial, form.telnet) {
            (1, ..) => ConnKind::Sftp,
            (2, ..) => ConnKind::Ftp,
            (3, ..) => ConnKind::Smb,
            (_, true, ..) => ConnKind::Adb,
            (_, _, true, _) => ConnKind::Serial,
            (_, _, _, true) => ConnKind::Telnet,
            _ => ConnKind::Ssh,
        }
    }

    fn apply(self, form: &mut ConnForm) {
        form.telnet = self == ConnKind::Telnet;
        form.adb = self == ConnKind::Adb;
        form.serial = self == ConnKind::Serial;
        form.files = match self {
            ConnKind::Sftp => 1,
            ConnKind::Ftp => 2,
            ConnKind::Smb => 3,
            _ => 0,
        };
        set_default_port(form);
    }

    fn group(self) -> &'static str {
        Self::ALL.iter().find(|(kind, ..)| *kind == self).map_or("", |(_, _, group)| group)
    }
}

/// Categorías del raíl izquierdo: (grupo de `ConnKind::ALL`, icono). Una categoría nueva es una fila más.
const CATEGORIES: [(&str, &str); 3] = [("Terminal", crate::icons::TERMINAL), ("Archivos", crate::icons::FOLDER), ("Otros", crate::icons::DOTS)];

const RAIL_BUTTON: f32 = 56.0;
const RAIL_WIDTH: f32 = RAIL_BUTTON + 16.0;

/// Botones cuadrados de categoría, fijos a la izquierda mientras el formulario se desplaza.
fn category_rail(ui: &mut egui::Ui, form: &mut ConnForm) {
    let current = ConnKind::of(form).group();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.set_width(RAIL_BUTTON);
        for (group, icon) in CATEGORIES {
            let selected = group == current;
            let (rect, response) = ui.allocate_exact_size(egui::vec2(RAIL_BUTTON, RAIL_BUTTON), egui::Sense::click());
            let response = response.hand().on_hover_text(group);
            let (fill, ink) = if selected { (theme::accent(), theme::base()) } else { (Color32::TRANSPARENT, theme::text()) };
            let radius = egui::CornerRadius::same(6);
            ui.painter().rect_filled(rect, radius, if !selected && response.hovered() { theme::selection() } else { fill });
            if !selected {
                ui.painter().rect_stroke(rect, radius, egui::Stroke::new(1.0, theme::surface()), egui::StrokeKind::Inside);
            }
            ui.painter().text(rect.center() - egui::vec2(0.0, 8.0), egui::Align2::CENTER_CENTER, icon, FontId::proportional(22.0), ink);
            ui.painter().text(rect.center() + egui::vec2(0.0, 17.0), egui::Align2::CENTER_CENTER, group, FontId::proportional(10.5), ink);
            if response.clicked() && !selected {
                if let Some((kind, ..)) = ConnKind::ALL.iter().find(|(_, _, g)| *g == group) {
                    kind.apply(form);
                }
            }
        }
    });
}

/// Tipos de la categoría elegida: la primera línea del formulario.
fn kind_row(ui: &mut egui::Ui, form: &mut ConnForm) {
    // Sin etiqueta: los botones del raíl ya dicen qué se está eligiendo.
    form_row(ui, "", |ui| {
        let current = ConnKind::of(form);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (kind, name, group) in ConnKind::ALL {
                if group == current.group() && ui.selectable_label(current == kind, name).hand().clicked() && current != kind {
                    kind.apply(form);
                }
            }
        });
    });
}

fn ssh_access_rows(ui: &mut egui::Ui, form: &mut ConnForm, connections: &[Connection]) {
    ui.add_space(ROW_GAP);
    form_row(ui, "Usuario", |ui| {
        let width = ui.available_width();
        text_input(ui, &mut form.user, width, "usuario del servidor", &mut form.errors.user);
        error_line(ui, &form.errors.user);
    });
    ui.add_space(ROW_GAP);
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
    ui.add_space(ROW_GAP);
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

fn telnet_rows(ui: &mut egui::Ui, form: &mut ConnForm) {
    ui.add_space(ROW_GAP);
    form_row(ui, "Codificación", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (encoding, label) in Encoding::ALL {
                ui.selectable_value(&mut form.encoding, encoding, label);
            }
        });
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "Opciones", |ui| {
        ui.checkbox(&mut form.negotiate, "Negociar Telnet (eco, tamaño de ventana y tipo de terminal)").hand();
        if !form.negotiate {
            ui.label(RichText::new("Sin negociar es una conexión TCP simple: no se traducen Enter ni opciones.").small().color(theme::subtext()));
        }
    });
    ui.add_space(ROW_GAP);
    form_row(ui, "", |ui| {
        ui.label(RichText::new("Telnet no cifra: contraseñas y comandos viajan en texto claro.").small().color(theme::red()));
    });
}

/// Comando inicial y túneles, solo de SSH; se despliega solo si un túnel tiene un error.
fn advanced_rows(ui: &mut egui::Ui, form: &mut ConnForm) {
    let shown = CollapsingHeader::new(RichText::new("Avanzado").color(theme::subtext()))
        .id_salt("conn-advanced")
        .open(Some(form.advanced_open))
        .show(ui, |ui| {
            ui.add_space(ROW_GAP);
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
                    let line = ui.label(RichText::new(message.as_str()).small().color(theme::red()));
                    if REVEAL_ERROR.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        line.scroll_to_me(Some(egui::Align::Center));
                    }
                } else if row.listens_on_network() {
                    ui.label(RichText::new("Escucha fuera de este equipo: otros dispositivos de la red podrán usar este túnel.").small().color(theme::identity(1)));
                }
            }
            if let Some(index) = remove {
                form.forwards.remove(index);
                form.errors.forwards.remove(index);
            }
            if crate::ui::outline_add(ui, egui::Button::new("+ Añadir túnel"), false).clicked() {
                form.forwards.push(ForwardRow::new());
                form.errors.forwards.push(None);
            }
            ui.label(
                RichText::new("-L: este equipo escucha y reenvía al destino vía el servidor. -R: el servidor escucha y reenvía a un destino alcanzable desde aquí. -D: proxy SOCKS5 local que sale por el servidor. Los cambios se aplican al reconectar.")
                    .small()
                    .color(theme::subtext()),
            );
        });
    if shown.header_response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        form.advanced_open = !form.advanced_open;
    }
}

fn edit_body(ui: &mut egui::Ui, form: &mut ConnForm, connections: &[Connection]) -> bool {
    // Pantalla menos título, pie, márgenes y la línea de estado.
    let room = (ui.ctx().content_rect().height() * 0.9 - 38.0 - 56.0 - 34.0 - 34.0).max(240.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        category_rail(ui, form);
        ui.add_space(RAIL_WIDTH - RAIL_BUTTON);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ScrollArea::vertical()
                .max_height(room)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // La barra flotante no debe tapar el borde de los campos.
                    ui.set_max_width(ui.available_width() - 10.0);
                    // Altura mínima para que el diálogo no salte al cambiar entre tipos de distinta longitud.
                    ui.set_min_height(380.0_f32.min(room));
                    form_fields(ui, form, connections)
                })
                .inner
        })
        .inner
    })
    .inner
}

fn form_fields(ui: &mut egui::Ui, form: &mut ConnForm, connections: &[Connection]) -> bool {
    kind_row(ui, form);
    ui.add_space(GROUP_GAP);
    form_row(ui, "Nombre", |ui| {
        let width = ui.available_width();
        let response = text_input(ui, &mut form.name, width, "Nombre de la conexión", &mut form.errors.name);
        if std::mem::take(&mut form.focus) {
            response.request_focus();
        }
        error_line(ui, &form.errors.name);
    });
    // El entorno va junto al nombre: es lo que evita escribir en el servidor equivocado.
    ui.add_space(ROW_GAP);
    environment_row(ui, form);

    ui.add_space(GROUP_GAP);
    let kind = ConnKind::of(form);
    if kind == ConnKind::Serial {
        serial_rows(ui, form);
        return false;
    }
    if kind == ConnKind::Adb {
        adb_rows(ui, form);
        return false;
    }
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
    match kind {
        ConnKind::Ssh | ConnKind::Sftp => ssh_access_rows(ui, form, connections),
        ConnKind::Ftp | ConnKind::Smb => files_rows(ui, form),
        ConnKind::Telnet => telnet_rows(ui, form),
        ConnKind::Serial | ConnKind::Adb => {}
    }
    if kind == ConnKind::Ssh {
        ui.add_space(GROUP_GAP);
        advanced_rows(ui, form);
    }
    false
}

const SHORTCUTS: [(&str, &[(&str, &str)]); 4] = [
    (
        "Pestañas",
        &[
            ("Ctrl+Shift+P", "Paleta de comandos"),
            ("Ctrl+,", "Abrir los ajustes"),
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
        &[("Ctrl+Shift+B", "Mostrar u ocultar la barra lateral"), ("Ctrl+Shift+K", "Buscar en la barra lateral"), ("F11", "Pantalla completa")],
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

/// Lista de secciones de la página de ajustes.
const SETTINGS_SECTIONS: [(SettingsTab, &str); 3] = [(SettingsTab::General, "General"), (SettingsTab::Atajos, "Atajos de teclado"), (SettingsTab::Hosts, "Hosts conocidos")];
const SETTINGS_MAX_WIDTH: f32 = 720.0;

/// Página de ajustes de una pestaña: lista de secciones a la izquierda y el contenido a la derecha.
pub fn settings_page(ui: &mut egui::Ui, tab: &mut SettingsTab, settings: &mut Settings, master_forced: &mut bool, hosts: &mut KnownHostsView, connections: &[Connection]) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
        ui.vertical(|ui| {
            ui.set_width(168.0);
            for (choice, label) in SETTINGS_SECTIONS {
                let selected = *tab == choice;
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
                let response = response.hand();
                if selected {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), theme::selection());
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), theme::surface());
                }
                let font = FontId::new(14.0, if selected { crate::ui::strong_family() } else { egui::FontFamily::Proportional });
                ui.painter().text(rect.min + egui::vec2(12.0, 16.0), egui::Align2::LEFT_CENTER, label, font, if selected { theme::text() } else { theme::subtext() });
                if response.clicked() && !selected {
                    *tab = choice;
                    hosts.loaded = false;
                }
            }
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.set_max_width(SETTINGS_MAX_WIDTH);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(SETTINGS_MAX_WIDTH - 12.0);
                settings_content(ui, *tab, settings, master_forced, hosts, connections);
            });
        });
    });
}

fn settings_content(ui: &mut egui::Ui, tab: SettingsTab, settings: &mut Settings, master_forced: &mut bool, hosts: &mut KnownHostsView, connections: &[Connection]) {
    if tab == SettingsTab::Hosts {
        known_hosts_body(ui, hosts, connections);
        return;
    }
    if tab == SettingsTab::Atajos {
        for (group, entries) in SHORTCUTS {
            settings_card(ui, group, |ui| {
                for (index, (keys, what)) in entries.iter().enumerate() {
                    setting_row(ui, what, None, index + 1 < entries.len(), |ui| {
                        for (n, combo) in keys.split(" / ").enumerate() {
                            if n > 0 {
                                ui.label(RichText::new("/").color(theme::subtext()));
                            }
                            keycap(ui, combo);
                        }
                    });
                }
            });
        }
        return;
    }
    settings_card(ui, "Apariencia", |ui| {
        setting_row(ui, "Tema", None, true, |ui| {
            ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, "Oscuro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::Light, "Claro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::System, "Sistema");
        });
        setting_row(ui, "Acento", Some("Color de lo seleccionado"), true, |ui| {
            for (choice, label) in AccentChoice::ALL {
                ui.selectable_value(&mut settings.accent, choice, label);
            }
        });
        setting_row(ui, "Densidad", Some("Altura de las filas de las listas"), true, |ui| {
            ui.selectable_value(&mut settings.compact, false, "Cómoda");
            ui.selectable_value(&mut settings.compact, true, "Compacta");
        });
        setting_row(ui, "Barra de menú", Some("Si está oculta, Alt la muestra"), true, |ui| {
            toggle(ui, &mut settings.menu_bar);
        });
        setting_row(ui, "Confirmar al cerrar pestañas", Some("Solo si tienen algo en ejecución"), false, |ui| {
            toggle(ui, &mut settings.confirm_close);
        });
    });

    settings_card(ui, "Terminal", |ui| {
        setting_row(ui, "Fuente", None, true, |ui| {
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
        setting_row(ui, "Tamaño de la letra", None, true, |ui| {
            ui.add(egui::Slider::new(&mut settings.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE).step_by(1.0).suffix(" pt"));
        });
        setting_row(ui, "Cursor", None, true, |ui| {
            ui.selectable_value(&mut settings.cursor, CursorChoice::Block, "Bloque");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Beam, "Barra");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Underline, "Subrayado");
        });
        setting_row(ui, "Cursor parpadeante", None, true, |ui| {
            toggle(ui, &mut settings.blink);
        });
        setting_row(ui, "Historial", Some("Se aplica a las sesiones nuevas"), false, |ui| {
            ui.add(egui::DragValue::new(&mut settings.scrollback).range(MIN_SCROLLBACK..=MAX_SCROLLBACK).speed(100.0).suffix(" líneas"));
        });
    });

    settings_card(ui, "Avisos", |ui| {
        let mut notify = settings.notify_after > 0;
        setting_row(ui, "Avisar al terminar la actividad", Some("De una pestaña que no está a la vista"), notify, |ui| {
            if toggle(ui, &mut notify).changed() {
                settings.notify_after = if notify { 15 } else { 0 };
            }
        });
        if notify {
            setting_row(ui, "Solo si duró al menos", None, false, |ui| {
                ui.add(egui::DragValue::new(&mut settings.notify_after).range(1..=3600).suffix(" s"));
            });
        }
    });

    settings_card(ui, "Secretos", |ui| {
        setting_row(ui, "Usar siempre la clave maestra", Some("Aunque haya keyring. Se aplica al reiniciar"), false, |ui| {
            toggle(ui, master_forced);
        });
    });
}

/// Grupo de ajustes: título y una tarjeta con sus filas.
fn settings_card(ui: &mut egui::Ui, title: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(GROUP_GAP);
    ui.label(RichText::new(title).font(FontId::new(12.5, crate::ui::strong_family())).color(theme::subtext()));
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme::mantle())
        .stroke(egui::Stroke::new(1.0, theme::surface()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width());
            rows(ui);
        });
}

const SETTING_LABEL_WIDTH: f32 = 290.0;

/// Casilla con borde propio: sobre el fondo de una tarjeta clara la casilla vacía casi no se vería.
fn toggle(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    ui.scope(|ui| {
        let stroke = egui::Stroke::new(1.0, theme::subtext());
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = stroke;
        widgets.hovered.bg_stroke = stroke;
        ui.checkbox(value, "").hand()
    })
    .inner
}

/// Combinación de teclas con aspecto de tecla.
pub fn keycap(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::surface())
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(FontId::new(12.5, egui::FontFamily::Monospace)).color(theme::text()));
        });
}

/// Una fila: el nombre (y una pista) a la izquierda y el control en una columna común a la derecha.
fn setting_row(ui: &mut egui::Ui, label: &str, hint: Option<&str>, divider: bool, controls: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.set_min_height(48.0);
        ui.allocate_ui_with_layout(egui::vec2(SETTING_LABEL_WIDTH, 48.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.set_min_width(SETTING_LABEL_WIDTH);
            let lines = if hint.is_some() { 2.0 } else { 1.0 };
            ui.add_space((48.0 - lines * 17.0 - if hint.is_some() { 1.0 } else { 0.0 }) / 2.0);
            ui.label(RichText::new(label).color(theme::text()));
            if let Some(hint) = hint {
                ui.label(RichText::new(hint).small().color(theme::subtext()));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            controls(ui);
        });
    });
    if divider {
        let y = ui.cursor().min.y;
        let x = ui.max_rect().x_range();
        ui.painter().hline(x, y, egui::Stroke::new(1.0, theme::surface()));
    }
}

fn known_hosts_body(ui: &mut egui::Ui, view: &mut KnownHostsView, connections: &[Connection]) {
    view.ensure_loaded();
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0, theme::surface());
        ui.add(
            TextEdit::singleline(&mut view.query)
                .hint_text("Buscar por host, conexión o huella")
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(8.0, 6.0)),
        );
    });
    if view.error.is_none() && !view.entries.is_empty() {
        ui.label(RichText::new(format!("{} entradas en {}", view.entries.len(), crate::ssh::default_known_hosts().display())).small().color(theme::subtext()));
    }
    ui.add_space(6.0);
    if let Some(error) = &view.error {
        ui.label(RichText::new(error).color(theme::red()));
        return;
    }
    if view.entries.is_empty() {
        ui.label(RichText::new("Aún no hay hosts conocidos. Aparecen aquí cuando confías en un servidor al conectar.").color(theme::subtext()));
        return;
    }
    let query = view.query.trim().to_lowercase();
    let mut shown = 0;
    for entry in &view.entries {
        let name = entry.display();
        let endpoints = entry.endpoints();
        let connection = connections.iter().find(|c| endpoints.iter().any(|(h, p)| h.eq_ignore_ascii_case(&c.host) && *p == c.port));
        let haystack = format!("{} {} {}", entry.hosts.join(" "), connection.map_or("", |c| c.name.as_str()), entry.fingerprint).to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }
        shown += 1;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::hover());
        if response.hovered() {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(6), theme::surface());
        }
        let title = ui.painter().text(
            rect.min + egui::vec2(10.0, 15.0),
            egui::Align2::LEFT_CENTER,
            &name,
            FontId::new(14.0, crate::ui::strong_family()),
            if entry.hashed { theme::subtext() } else { theme::text() },
        );
        if let Some(connection) = connection {
            ui.painter().text(egui::pos2(title.max.x + 10.0, rect.min.y + 15.0), egui::Align2::LEFT_CENTER, &connection.name, FontId::proportional(12.0), theme::accent());
        }
        let short: String = entry.fingerprint.chars().take(28).collect();
        ui.painter().text(
            rect.min + egui::vec2(10.0, 34.0),
            egui::Align2::LEFT_CENTER,
            format!("{}  ·  {short}…", entry.key_type),
            FontId::new(12.0, egui::FontFamily::Monospace),
            theme::subtext(),
        );
        for (offset, glyph, tip) in [(56.0, crate::icons::COPY, "Copiar la huella"), (24.0, crate::icons::TRASH, "Olvidar este host")] {
            let button = egui::Rect::from_center_size(egui::pos2(rect.max.x - offset, rect.center().y), egui::vec2(28.0, 28.0));
            let response = ui.interact(button, ui.auto_id_with(("known-host", entry.index, glyph)), egui::Sense::click()).hand();
            if response.hovered() {
                ui.painter().rect_filled(button, egui::CornerRadius::same(6), theme::selection());
            }
            ui.painter().text(button.center(), egui::Align2::CENTER_CENTER, glyph, FontId::proportional(16.0), theme::text());
            if response.on_hover_text(tip).clicked() {
                if glyph == crate::icons::COPY {
                    ui.ctx().copy_text(entry.fingerprint.clone());
                } else {
                    view.delete = Some(entry.clone());
                }
            }
        }
    }
    if shown == 0 {
        ui.label(RichText::new("Ningún host coincide con la búsqueda.").color(theme::subtext()));
    }
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
