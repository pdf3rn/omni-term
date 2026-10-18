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

/// Un cliente SFTP conectado. Mantener el `Handle` vivo mantiene abierta la sesión.
pub struct Sftp {
    pub session: Arc<SftpSession>,
    _connection: Arc<Handle<Client>>,
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
    Ok(Sftp { session: Arc::new(session), _connection: Arc::new(handle) })
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
        entries.push(Entry { name, is_dir, size: metadata.size.unwrap_or(0), modified: metadata.mtime.map(u64::from) });
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
    fn directories_sort_before_files_case_insensitively() {
        let entry = |name: &str, is_dir| Entry { name: name.into(), is_dir, size: 0, modified: None };
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

        // Listar y crear carpeta.
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

        // Bajar la carpeta remota a otro directorio local.
        let downloads = local.join("descargas");
        std::fs::create_dir_all(&downloads).unwrap();
        let source = (join(&remote_path, "proj"), true);
        let download = transfer::download(runtime.handle(), sftp.clone(), vec![source], downloads.clone(), transfer_events, Arc::new(|| {}));
        assert_eq!(wait(&download), TransferState::Done);
        assert_eq!(std::fs::read(downloads.join("proj/sub/b.bin")).unwrap(), big);
        assert_eq!(std::fs::read(downloads.join("proj/a (1).txt")).unwrap(), b"version dos");

        // Renombrar y borrar de forma recursiva.
        let old = join(&remote_path, "proj/a.txt");
        let new = join(&remote_path, "proj/renombrado.txt");
        runtime.block_on(rename(&sftp, &old, &new)).unwrap();
        assert!(!remote.join("proj/a.txt").exists() && remote.join("proj/renombrado.txt").exists());
        runtime.block_on(remove(&sftp, &join(&remote_path, "proj"))).unwrap();
        assert!(!remote.join("proj").exists());

        // Un error de lectura llega como error, no como pánico.
        assert!(runtime.block_on(list(&sftp, &join(&remote_path, "no-existe"))).is_err());
    }
}
