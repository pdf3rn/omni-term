use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::terminal::term::{Backend, Feed};

struct PtyBackend {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

impl Backend for PtyBackend {
    fn write(&self, bytes: &[u8]) {
        let _ = self.writer.lock().unwrap().write_all(bytes);
    }

    fn resize(&self, cols: usize, rows: usize) {
        let _ = self.master.lock().unwrap().resize(size(cols, rows));
    }

    #[cfg(unix)]
    fn is_busy(&self) -> Option<bool> {
        let leader = self.master.lock().unwrap().process_group_leader()?;
        let shell = self.child.lock().unwrap().process_id()?;
        Some(leader != shell as i32)
    }

    // ConPTY no expone el grupo de procesos en primer plano.
    #[cfg(not(unix))]
    fn is_busy(&self) -> Option<bool> {
        None
    }
}

impl Drop for PtyBackend {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

fn size(cols: usize, rows: usize) -> PtySize {
    PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }
}

/// Cómo abrir un terminal local: el shell del sistema en la carpeta personal, o un perfil guardado.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Intérprete; vacío usa el shell del sistema.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    /// Se escribe en el terminal nada más abrirlo.
    pub init: Option<String>,
}

impl Profile {
    pub fn from_connection(connection: &crate::data::store::Connection) -> Self {
        Self {
            program: connection.host.trim().to_string(),
            args: split_args(&connection.user).unwrap_or_default(),
            cwd: Some(connection.key_path.clone().unwrap_or_default()).filter(|c| !c.trim().is_empty()),
            env: parse_vars(&connection.vars),
            init: connection.init_command.clone().filter(|c| !c.trim().is_empty()),
        }
    }
}

/// Separa una línea de argumentos como lo haría una shell: espacios, comillas dobles y simples, `\` para escapar fuera de las simples.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => current.push(c),
            (_, '\\') if cfg!(not(windows)) => match chars.next() {
                Some(next) => {
                    current.push(next);
                    started = true;
                }
                None => current.push('\\'),
            },
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("Falta cerrar unas comillas".into());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

/// Variables `NOMBRE=valor`, una por línea; se ignoran las líneas vacías, los comentarios con `#` y las que no tienen nombre válido.
pub fn parse_vars(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            let valid = !name.is_empty() && !name.starts_with('#') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            valid.then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn expand_home(path: &str) -> std::path::PathBuf {
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from);
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => home().map(|h| h.join(rest.trim_start_matches(['/', '\\']))).unwrap_or_else(|| path.into()),
        _ => path.into(),
    }
}

/// Si `program` es una ruta a un archivo o un ejecutable que está en el `PATH`.
pub fn program_exists(program: &str) -> bool {
    let program = program.trim();
    if program.is_empty() {
        return true;
    }
    let path = expand_home(program);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file();
    }
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into()).split(';').map(str::to_lowercase).collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            dir.join(program).is_file() || extensions.iter().any(|ext| !ext.is_empty() && dir.join(format!("{program}{ext}")).is_file())
        })
    })
}

/// Si `dir` (con `~` al principio) es una carpeta existente.
pub fn dir_exists(dir: &str) -> bool {
    dir.trim().is_empty() || expand_home(dir.trim()).is_dir()
}

/// Un intérprete que se puede elegir en el formulario de un perfil.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellOption {
    pub label: String,
    pub program: String,
    /// Argumentos que acompañan al intérprete (por ejemplo `-d Ubuntu` en WSL).
    pub args: String,
}

/// Intérpretes instalados: en Linux y macOS los de `/etc/shells`; en Windows los habituales y las distribuciones de WSL.
pub fn available_shells() -> Vec<ShellOption> {
    let mut found: Vec<ShellOption> = Vec::new();
    let mut add = |label: String, program: String, args: String| {
        if !found.iter().any(|s| s.program == program && s.args == args) {
            found.push(ShellOption { label, program, args });
        }
    };
    #[cfg(unix)]
    {
        if let Ok(text) = std::fs::read_to_string("/etc/shells") {
            for line in text.lines().map(str::trim).filter(|l| l.starts_with('/') && !l.starts_with('#')) {
                if std::path::Path::new(line).is_file() {
                    let name = line.rsplit('/').next().unwrap_or(line).to_string();
                    add(name, line.to_string(), String::new());
                }
            }
        }
    }
    #[cfg(windows)]
    {
        for (label, program) in [("PowerShell 7", "pwsh.exe"), ("Windows PowerShell", "powershell.exe"), ("Símbolo del sistema", "cmd.exe")] {
            if program_exists(program) {
                add(label.to_string(), program.to_string(), String::new());
            }
        }
        for git in ["C:\\Program Files\\Git\\bin\\bash.exe", "C:\\Program Files (x86)\\Git\\bin\\bash.exe"] {
            if std::path::Path::new(git).is_file() {
                add("Git Bash".to_string(), git.to_string(), "--login -i".to_string());
            }
        }
        for distro in wsl_distributions() {
            add(format!("WSL · {distro}"), "wsl.exe".to_string(), format!("-d {distro}"));
        }
    }
    found
}

#[cfg(windows)]
fn wsl_distributions() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    let Ok(output) = std::process::Command::new("wsl.exe").args(["-l", "-q"]).creation_flags(0x0800_0000).output() else { return Vec::new() };
    // `wsl -l` imprime en UTF-16.
    let units: Vec<u16> = output.stdout.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    String::from_utf16_lossy(&units).lines().map(|l| l.trim().trim_start_matches('\u{feff}').to_string()).filter(|l| !l.is_empty()).collect()
}

/// Abre un terminal local con el intérprete, los argumentos, la carpeta y las variables de `profile`.
pub fn spawn_profile(cols: usize, rows: usize, mut feed: Feed, profile: &Profile) -> anyhow::Result<Arc<dyn Backend>> {
    let pair = native_pty_system().openpty(size(cols, rows))?;

    let program = if profile.program.is_empty() {
        std::env::var("SHELL").unwrap_or_else(|_| default_shell().into())
    } else {
        expand_home(&profile.program).to_string_lossy().into_owned()
    };
    let mut cmd = CommandBuilder::new(program);
    cmd.args(&profile.args);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for (name, value) in &profile.env {
        cmd.env(name, value);
    }
    match &profile.cwd {
        Some(dir) => cmd.cwd(expand_home(dir)),
        None => {
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                cmd.cwd(home);
            }
        }
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            feed.feed(&buf[..n]);
        }
        feed.close();
    });

    let backend = PtyBackend {
        writer: Mutex::new(pair.master.take_writer()?),
        master: Mutex::new(pair.master),
        child: Mutex::new(child),
    };
    if let Some(init) = &profile.init {
        backend.write(format!("{init}\r").as_bytes());
    }
    Ok(Arc::new(backend))
}

fn default_shell() -> &'static str {
    if cfg!(windows) { "powershell.exe" } else { "/bin/sh" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_split_like_a_shell() {
        assert_eq!(split_args("-l -i").unwrap(), ["-l", "-i"]);
        assert_eq!(split_args(r#"-c "echo hola mundo" 'a b'"#).unwrap(), ["-c", "echo hola mundo", "a b"]);
        assert_eq!(split_args("   ").unwrap(), Vec::<String>::new());
        assert_eq!(split_args(r#"""  x"#).unwrap(), ["", "x"]);
        assert!(split_args("-c \"sin cerrar").is_err());
    }

    #[test]
    fn variables_ignore_comments_and_invalid_names() {
        let vars = parse_vars("EDITOR=vim\n# nota\n\nMALO NOMBRE=1\n  PATH_EXTRA = /opt/bin \nsin igual");
        assert_eq!(vars, [("EDITOR".to_string(), "vim".to_string()), ("PATH_EXTRA".to_string(), "/opt/bin".to_string())]);
    }

    #[cfg(unix)]
    #[test]
    fn programs_and_folders_are_checked() {
        assert!(program_exists(""));
        assert!(program_exists("/bin/sh"));
        assert!(program_exists("sh"));
        assert!(!program_exists("/no/existe/shell"));
        assert!(!program_exists("no-existe-este-programa"));
        assert!(dir_exists(""));
        assert!(dir_exists("/tmp"));
        assert!(!dir_exists("/no/existe"));
    }

    #[test]
    fn a_profile_reads_its_fields_from_a_connection() {
        use crate::data::store::{Auth, Connection, Protocol};
        let connection = Connection {
            id: None,
            protocol: Protocol::Local,
            name: "dev".into(),
            host: " /bin/zsh ".into(),
            port: 0,
            user: r#"-l "-o x""#.into(),
            auth: Auth::Agent,
            key_path: Some("~/proyectos".into()),
            init_command: Some("ls".into()),
            jump: None,
            color: None,
            env: None,
            vars: "A=1".into(),
        };
        let profile = Profile::from_connection(&connection);
        assert_eq!(profile.program, "/bin/zsh");
        assert_eq!(profile.args, ["-l", "-o x"]);
        assert_eq!(profile.cwd.as_deref(), Some("~/proyectos"));
        assert_eq!(profile.env, [("A".to_string(), "1".to_string())]);
        assert_eq!(profile.init.as_deref(), Some("ls"));
    }
}
