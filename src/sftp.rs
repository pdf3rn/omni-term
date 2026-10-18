//! Cliente SFTP sobre un canal de una sesión SSH propia (russh-sftp).

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use russh::client::Handle;
use russh_sftp::client::SftpSession;

use crate::ssh::{self, Client, Credentials, UiEvent};
use crate::store::Connection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Segundos desde la época Unix.
    pub modified: Option<u64>,
    /// Bits de permisos POSIX (por ejemplo `0o755`), si el sistema los informa.
    pub permissions: Option<u32>,
}

/// Une rutas POSIX (SFTP siempre usa `/` en el cable).
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
    }
}

pub fn file_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// Directorios primero y luego por nombre sin distinguir mayúsculas.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

/// Ordena por la columna elegida; las carpetas siempre van antes que los archivos.
pub fn sort_by(entries: &mut [Entry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        let by_key = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortKey::Modified => a.modified.cmp(&b.modified).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        };
        b.is_dir.cmp(&a.is_dir).then(if ascending { by_key } else { by_key.reverse() })
    });
}

/// Un cliente SFTP conectado. Mantener el `Handle` vivo mantiene abierta la sesión.
pub struct Sftp {
    pub session: Arc<SftpSession>,
    connection: Arc<Handle<Client>>,
}

impl Sftp {
    /// ¿Se perdió la conexión SSH subyacente?
    pub fn is_closed(&self) -> bool {
        self.connection.is_closed()
    }
}

pub async fn connect(
    connection: &Connection,
    credentials: &Credentials,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    known_hosts: std::path::PathBuf,
) -> anyhow::Result<Sftp> {
    let handle = ssh::establish(
        connection,
        credentials,
        events,
        wake,
        known_hosts,
        Arc::new(Mutex::new(Vec::new())),
        ssh::new_tunnels(&[]),
    )
    .await?;
    let channel = handle.channel_open_session().await.context("abriendo el canal")?;
    channel.request_subsystem(true, "sftp").await.context("el servidor no ofrece el subsistema sftp")?;
    let session = SftpSession::new(channel.into_stream()).await.context("iniciando SFTP")?;
    Ok(Sftp { session: Arc::new(session), connection: Arc::new(handle) })
}

pub async fn home(sftp: &SftpSession) -> String {
    sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into())
}

pub async fn list(sftp: &SftpSession, path: &str) -> anyhow::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in sftp.read_dir(path.to_string()).await.with_context(|| format!("leyendo {path}"))? {
        let name = item.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let metadata = item.metadata();
        let mut is_dir = metadata.is_dir();
        if metadata.is_symlink() {
            // Un enlace a una carpeta se navega como carpeta.
            if let Ok(target) = sftp.metadata(join(path, &name)).await {
                is_dir = target.is_dir();
            }
        }
        entries.push(Entry {
            name,
            is_dir,
            size: metadata.size.unwrap_or(0),
            modified: metadata.mtime.map(u64::from),
            permissions: metadata.permissions.map(|p| p & 0o7777),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

pub async fn make_dir(sftp: &SftpSession, path: &str) -> anyhow::Result<()> {
    sftp.create_dir(path.to_string()).await.with_context(|| format!("creando {path}"))
}

pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> anyhow::Result<()> {
    sftp.rename(from.to_string(), to.to_string()).await.with_context(|| format!("renombrando {from}"))
}

pub async fn set_permissions(sftp: &SftpSession, path: &str, mode: u32) -> anyhow::Result<()> {
    let attributes = russh_sftp::protocol::FileAttributes { permissions: Some(mode & 0o7777), ..Default::default() };
    sftp.set_metadata(path.to_string(), attributes).await.with_context(|| format!("cambiando permisos de {path}"))
}

/// Permisos en el formato de `ls -l`, por ejemplo `rwxr-xr-x`.
pub fn format_mode(mode: u32) -> String {
    let bit = |mask: u32, on: char| if mode & mask != 0 { on } else { '-' };
    let exec = |exec: u32, special: u32, set: char, unset: char| match (mode & exec != 0, mode & special != 0) {
        (true, true) => set,
        (false, true) => unset,
        (true, false) => 'x',
        (false, false) => '-',
    };
    [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        exec(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        exec(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        exec(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect()
}

/// Borra un archivo o una carpeta con todo su contenido.
pub async fn remove(sftp: &SftpSession, path: &str) -> anyhow::Result<()> {
    let mut pending = vec![(path.to_string(), false)];
    let mut dirs_after = Vec::new();
    while let Some((current, _)) = pending.pop() {
        let metadata = sftp.symlink_metadata(current.clone()).await.with_context(|| format!("leyendo {current}"))?;
        if metadata.is_dir() {
            for item in sftp.read_dir(current.clone()).await? {
                let name = item.file_name();
                if name != "." && name != ".." {
                    pending.push((join(&current, &name), false));
                }
            }
            dirs_after.push(current);
        } else {
            sftp.remove_file(current.clone()).await.with_context(|| format!("eliminando {current}"))?;
        }
    }
    // Las carpetas se vacían antes de borrarse: se recorren de las más profundas a las más someras.
    for dir in dirs_after.into_iter().rev() {
        sftp.remove_dir(dir.clone()).await.with_context(|| format!("eliminando {dir}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_path_helpers() {
        assert_eq!(join("/home/u", "a.txt"), "/home/u/a.txt");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(parent("/home/u/docs/"), "/home/u");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("relativo"), "/");
        assert_eq!(file_name("/home/u/a.txt"), "a.txt");
        assert_eq!(file_name("/home/u/docs/"), "docs");
    }

    #[test]
    fn columns_sort_in_both_directions_keeping_folders_first() {
        let file = |name: &str, size, modified| Entry { name: name.into(), is_dir: false, size, modified: Some(modified), permissions: None };
        let folder = |name: &str| Entry { name: name.into(), is_dir: true, size: 0, modified: Some(1), permissions: None };
        let mut entries = vec![file("b.txt", 300, 20), folder("zeta"), file("a.txt", 100, 30), file("C.txt", 200, 10), folder("alfa")];

        sort_by(&mut entries, SortKey::Size, false);
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["zeta", "alfa", "b.txt", "C.txt", "a.txt"]);

        sort_by(&mut entries, SortKey::Modified, true);
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["alfa", "zeta", "C.txt", "b.txt", "a.txt"]);

        sort_by(&mut entries, SortKey::Name, true);
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["alfa", "zeta", "a.txt", "b.txt", "C.txt"]);
    }

    #[test]
    fn modes_are_formatted_like_ls() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o644), "rw-r--r--");
        assert_eq!(format_mode(0o000), "---------");
        assert_eq!(format_mode(0o4755), "rwsr-xr-x");
        assert_eq!(format_mode(0o4644), "rwSr--r--");
        assert_eq!(format_mode(0o1777), "rwxrwxrwt");
        assert_eq!(format_mode(0o1776), "rwxrwxrwT");
    }

    #[test]
    fn directories_sort_before_files_case_insensitively() {
        let entry = |name: &str, is_dir| Entry { name: name.into(), is_dir, size: 0, modified: None, permissions: None };
        let mut entries = vec![entry("zeta.txt", false), entry("Beta", true), entry("alfa.txt", false), entry("alpha", true)];
        sort_entries(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "alfa.txt", "zeta.txt"]);
    }

    /// Listar, crear, subir, bajar, renombrar y borrar contra un sshd privado. Solo Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn sftp_operations_against_a_private_sshd() {
        use std::time::{Duration, Instant};

        use crate::store::Auth;
        use crate::transfer::{self, Resolution, TransferEvent, TransferState};

        let Some(sshd) = crate::testutil::PrivateSshd::start() else { return };
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();

        let (events, requests) = std_mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(UiEvent::HostKey(request)) = requests.recv() {
                let _ = request.reply.send(true);
            }
        });
        let connection = Connection {
            id: None,
            name: "test".into(),
            host: "127.0.0.1".into(),
            port: sshd.port,
            user: sshd.user.clone(),
            auth: Auth::Key,
            key_path: Some(sshd.key_path.clone()),
            init_command: None,
            jump: None,
            color: None,
            env: None,
        };
        let known_hosts = sshd.dir.join("known_hosts");
        let client = runtime
            .block_on(connect(&connection, &Credentials::default(), events, Arc::new(|| {}), known_hosts))
            .expect("conexión SFTP");
        let sftp = client.session.clone();

        let remote = sshd.dir.join("remote");
        let local = sshd.dir.join("local");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(local.join("proj/sub")).unwrap();
        let remote_path = remote.to_string_lossy().into_owned();

        // Los conflictos se resuelven con "Renombrar a todos".
        let (transfer_events, conflicts) = std_mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(TransferEvent::Conflict(conflict)) = conflicts.recv() {
                let _ = conflict.reply.send((Resolution::Rename, true));
            }
        });
        let wait = |transfer: &transfer::Transfer| {
            let deadline = Instant::now() + Duration::from_secs(20);
            while transfer.state() == TransferState::Running {
                assert!(Instant::now() < deadline, "la transferencia no terminó");
                std::thread::sleep(Duration::from_millis(50));
            }
            transfer.state()
        };

        assert!(runtime.block_on(list(&sftp, &remote_path)).unwrap().is_empty());
        runtime.block_on(make_dir(&sftp, &join(&remote_path, "vacia"))).unwrap();
        let listed = runtime.block_on(list(&sftp, &remote_path)).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_dir && listed[0].name == "vacia");
        assert!(!runtime.block_on(home(&sftp)).is_empty());

        // Subir una carpeta con un archivo grande para cruzar varios bloques.
        let big: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        std::fs::write(local.join("proj/a.txt"), b"version uno").unwrap();
        std::fs::write(local.join("proj/sub/b.bin"), &big).unwrap();
        let upload = transfer::upload(runtime.handle(), sftp.clone(), vec![local.join("proj")], remote_path.clone(), transfer_events.clone(), Arc::new(|| {}));
        assert_eq!(wait(&upload), TransferState::Done);
        assert_eq!(upload.progress(), (big.len() as u64 + 11, big.len() as u64 + 11));
        assert_eq!(std::fs::read(remote.join("proj/a.txt")).unwrap(), b"version uno");
        assert_eq!(std::fs::read(remote.join("proj/sub/b.bin")).unwrap(), big);

        // Segunda subida con el archivo cambiado: el destino existe y se renombra, sin pisar el original.
        std::fs::write(local.join("proj/a.txt"), b"version dos").unwrap();
        let again = transfer::upload(runtime.handle(), sftp.clone(), vec![local.join("proj")], remote_path.clone(), transfer_events.clone(), Arc::new(|| {}));
        assert_eq!(wait(&again), TransferState::Done);
        assert_eq!(std::fs::read(remote.join("proj/a.txt")).unwrap(), b"version uno");
        assert_eq!(std::fs::read(remote.join("proj/a (1).txt")).unwrap(), b"version dos");

        let downloads = local.join("descargas");
        std::fs::create_dir_all(&downloads).unwrap();
        let source = (join(&remote_path, "proj"), true);
        let download = transfer::download(runtime.handle(), sftp.clone(), vec![source], downloads.clone(), transfer_events, Arc::new(|| {}));
        assert_eq!(wait(&download), TransferState::Done);
        assert_eq!(std::fs::read(downloads.join("proj/sub/b.bin")).unwrap(), big);
        assert_eq!(std::fs::read(downloads.join("proj/a (1).txt")).unwrap(), b"version dos");

        // Cambiar permisos: se ve en el listado y en el archivo real.
        let target = join(&remote_path, "proj/sub/b.bin");
        runtime.block_on(set_permissions(&sftp, &target, 0o640)).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(remote.join("proj/sub/b.bin")).unwrap().permissions().mode() & 0o7777, 0o640);
        }
        let listed = runtime.block_on(list(&sftp, &join(&remote_path, "proj/sub"))).unwrap();
        let changed = listed.iter().find(|e| e.name == "b.bin").unwrap();
        let untouched = listed.iter().find(|e| e.name == "b (1).bin").unwrap();
        assert_eq!(changed.permissions, Some(0o640));
        assert_eq!(untouched.permissions, Some(0o644));

        let old = join(&remote_path, "proj/a.txt");
        let new = join(&remote_path, "proj/renombrado.txt");
        runtime.block_on(rename(&sftp, &old, &new)).unwrap();
        assert!(!remote.join("proj/a.txt").exists() && remote.join("proj/renombrado.txt").exists());
        runtime.block_on(remove(&sftp, &join(&remote_path, "proj"))).unwrap();
        assert!(!remote.join("proj").exists());

        // Un error de lectura llega como error, no como pánico.
        assert!(runtime.block_on(list(&sftp, &join(&remote_path, "no-existe"))).is_err());
    }

    /// ProxyJump: se llega al servidor de destino a través de otro servidor SSH.
    #[cfg(target_os = "linux")]
    #[test]
    fn connects_through_a_jump_host() {
        use crate::ssh::JumpHost;
        use crate::store::Auth;

        let Some(jump) = crate::testutil::PrivateSshd::start() else { return };
        let Some(target) = crate::testutil::PrivateSshd::start() else { return };
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();

        let (events, requests) = std_mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = requests.recv() {
                if let UiEvent::HostKey(request) = event {
                    let _ = request.reply.send(true);
                }
            }
        });
        let connection = |sshd: &crate::testutil::PrivateSshd, name: &str| Connection {
            id: None,
            name: name.into(),
            host: "127.0.0.1".into(),
            port: sshd.port,
            user: sshd.user.clone(),
            auth: Auth::Key,
            key_path: Some(sshd.key_path.clone()),
            init_command: None,
            jump: None,
            color: None,
            env: None,
        };
        let credentials = Credentials {
            jump: Some(Box::new(JumpHost { connection: connection(&jump, "salto"), credentials: Credentials::default() })),
            ..Default::default()
        };
        std::fs::create_dir_all(target.dir.join("datos")).unwrap();
        std::fs::write(target.dir.join("datos/hola.txt"), b"hola").unwrap();

        let client = runtime
            .block_on(connect(&connection(&target, "destino"), &credentials, events, Arc::new(|| {}), target.dir.join("known_hosts")))
            .expect("conexión a través del salto");
        let entries = runtime.block_on(list(&client.session, &target.dir.join("datos").to_string_lossy())).unwrap();
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["hola.txt"]);

        // Sin el salto no habría tráfico en el servidor intermedio: aquí sí aceptó una sesión.
        assert!(jump.log().contains("Accepted publickey"), "el servidor de salto no participó:\n{}", jump.log());
    }
}
