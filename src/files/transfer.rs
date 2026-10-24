//! Transferencias SFTP en segundo plano: subir y bajar archivos y carpetas con progreso y cancelación.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use crate::files::remotefs::RemoteFs;
use crate::files::sftp;

const CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Replace,
    Skip,
    Rename,
}

/// Pregunta a la interfaz qué hacer cuando el destino ya existe.
pub struct Conflict {
    pub name: String,
    /// Respuesta y si aplica a todos los conflictos restantes de la transferencia.
    pub reply: oneshot::Sender<(Resolution, bool)>,
}

pub enum TransferEvent {
    Conflict(Conflict),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone)]
pub struct Transfer {
    pub label: String,
    pub upload: bool,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    state: Arc<Mutex<TransferState>>,
    cancel: Arc<AtomicBool>,
    started: std::time::Instant,
}

impl Transfer {
    fn new(label: String, upload: bool) -> Self {
        Self {
            label,
            upload,
            done: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            state: Arc::new(Mutex::new(TransferState::Running)),
            cancel: Arc::new(AtomicBool::new(false)),
            started: std::time::Instant::now(),
        }
    }

    /// Velocidad media en bytes por segundo y segundos que faltan, si ya hay datos suficientes.
    pub fn rate(&self) -> Option<(f64, u64)> {
        let (done, total) = self.progress();
        let elapsed = self.started.elapsed().as_secs_f64();
        if done == 0 || elapsed < 0.5 {
            return None;
        }
        let speed = done as f64 / elapsed;
        Some((speed, (total.saturating_sub(done) as f64 / speed).ceil() as u64))
    }

    /// Bytes transferidos y total previsto.
    pub fn progress(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub fn state(&self) -> TransferState {
        self.state.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn finish(&self, result: anyhow::Result<()>) {
        let state = match result {
            Ok(()) if self.cancelled() => TransferState::Cancelled,
            Ok(()) => TransferState::Done,
            Err(error) => TransferState::Failed(format!("{error:#}")),
        };
        *self.state.lock().unwrap() = state;
    }
}

struct Item {
    source: String,
    target: String,
    is_dir: bool,
    size: u64,
}

/// `informe.txt` → `informe (1).txt`; los archivos ocultos sin extensión conservan el punto inicial.
pub fn numbered(name: &str, n: usize) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

struct Context {
    events: Sender<TransferEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    policy: Option<Resolution>,
}

impl Context {
    async fn resolve(&mut self, name: &str) -> Resolution {
        if let Some(policy) = self.policy {
            return policy;
        }
        let (reply, answer) = oneshot::channel();
        if self.events.send(TransferEvent::Conflict(Conflict { name: name.into(), reply })).is_err() {
            return Resolution::Skip;
        }
        (self.wake)();
        match answer.await {
            Ok((resolution, all)) => {
                if all {
                    self.policy = Some(resolution);
                }
                resolution
            }
            Err(_) => Resolution::Skip,
        }
    }
}

async fn copy<R, W>(reader: &mut R, writer: &mut W, transfer: &Transfer) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; CHUNK];
    loop {
        if transfer.cancelled() {
            return Ok(());
        }
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).await?;
        transfer.done.fetch_add(read as u64, Ordering::Relaxed);
    }
    writer.flush().await?;
    writer.shutdown().await?;
    Ok(())
}

/// Sube archivos y carpetas locales a `remote_dir`.
pub fn upload(
    runtime: &tokio::runtime::Handle,
    fs: Arc<dyn RemoteFs>,
    sources: Vec<PathBuf>,
    remote_dir: String,
    events: Sender<TransferEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Transfer {
    let transfer = Transfer::new(describe(sources.iter().map(|p| p.to_string_lossy().into_owned())), true);
    let handle = transfer.clone();
    runtime.spawn(async move {
        let result = run_upload(&handle, fs.as_ref(), sources, &remote_dir, Context { events, wake: wake.clone(), policy: None }).await;
        handle.finish(result);
        wake();
    });
    transfer
}

async fn run_upload(
    transfer: &Transfer,
    fs: &dyn RemoteFs,
    sources: Vec<PathBuf>,
    remote_dir: &str,
    mut context: Context,
) -> anyhow::Result<()> {
    let target_dir = remote_dir.to_string();
    let items = tokio::task::spawn_blocking(move || plan_local(&sources, &target_dir)).await??;
    transfer.total.store(items.iter().map(|i| i.size).sum(), Ordering::Relaxed);

    for item in items {
        if transfer.cancelled() {
            return Ok(());
        }
        if item.is_dir {
            if !fs.exists(&item.target).await {
                fs.make_dir(&item.target).await?;
            }
            continue;
        }
        let mut target = item.target.clone();
        if fs.exists(&target).await {
            match context.resolve(sftp::file_name(&target)).await {
                Resolution::Skip => {
                    transfer.done.fetch_add(item.size, Ordering::Relaxed);
                    continue;
                }
                Resolution::Replace => {}
                Resolution::Rename => target = free_remote_name(fs, &target).await,
            }
        }
        let mut local = tokio::fs::File::open(&item.source).await?;
        let mut remote = fs.open_write(&target).await?;
        copy(&mut local, &mut remote, transfer).await?;
    }
    Ok(())
}

async fn free_remote_name(fs: &dyn RemoteFs, target: &str) -> String {
    let dir = sftp::parent(target);
    let name = sftp::file_name(target);
    for n in 1.. {
        let candidate = sftp::join(&dir, &numbered(name, n));
        if !fs.exists(&candidate).await {
            return candidate;
        }
    }
    unreachable!()
}

fn plan_local(sources: &[PathBuf], remote_dir: &str) -> anyhow::Result<Vec<Item>> {
    let mut items = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = sources
        .iter()
        .filter_map(|path| path.file_name().map(|name| (path.clone(), sftp::join(remote_dir, &name.to_string_lossy()))))
        .rev()
        .collect();
    while let Some((path, target)) = stack.pop() {
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            items.push(Item { source: path.to_string_lossy().into_owned(), target: target.clone(), is_dir: true, size: 0 });
            let mut children: Vec<_> = std::fs::read_dir(&path)?.filter_map(Result::ok).collect();
            children.sort_by_key(|entry| entry.file_name());
            for child in children.into_iter().rev() {
                stack.push((child.path(), sftp::join(&target, &child.file_name().to_string_lossy())));
            }
        } else {
            items.push(Item { source: path.to_string_lossy().into_owned(), target, is_dir: false, size: metadata.len() });
        }
    }
    Ok(items)
}

/// Baja rutas remotas (con indicación de si son carpetas) a `local_dir`.
pub fn download(
    runtime: &tokio::runtime::Handle,
    fs: Arc<dyn RemoteFs>,
    sources: Vec<(String, bool)>,
    local_dir: PathBuf,
    events: Sender<TransferEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Transfer {
    let transfer = Transfer::new(describe(sources.iter().map(|(path, _)| path.clone())), false);
    let handle = transfer.clone();
    runtime.spawn(async move {
        let result = run_download(&handle, fs.as_ref(), sources, &local_dir, Context { events, wake: wake.clone(), policy: None }).await;
        handle.finish(result);
        wake();
    });
    transfer
}

async fn run_download(
    transfer: &Transfer,
    fs: &dyn RemoteFs,
    sources: Vec<(String, bool)>,
    local_dir: &Path,
    mut context: Context,
) -> anyhow::Result<()> {
    let mut items: Vec<Item> = Vec::new();
    let mut stack: Vec<(String, bool, PathBuf)> = Vec::new();
    for (path, is_dir) in sources.into_iter().rev() {
        let target = local_dir.join(sftp::file_name(&path));
        stack.push((path, is_dir, target));
    }
    while let Some((path, is_dir, target)) = stack.pop() {
        if is_dir {
            items.push(Item { source: path.clone(), target: target.to_string_lossy().into_owned(), is_dir: true, size: 0 });
            let children = fs.list(&path).await?;
            for child in children.into_iter().rev() {
                stack.push((sftp::join(&path, &child.name), child.is_dir, target.join(&child.name)));
            }
        } else {
            let size = fs.size(&path).await;
            items.push(Item { source: path, target: target.to_string_lossy().into_owned(), is_dir: false, size });
        }
    }
    transfer.total.store(items.iter().map(|i| i.size).sum(), Ordering::Relaxed);

    for item in items {
        if transfer.cancelled() {
            return Ok(());
        }
        let mut target = PathBuf::from(&item.target);
        if item.is_dir {
            tokio::fs::create_dir_all(&target).await?;
            continue;
        }
        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            match context.resolve(&target.file_name().unwrap_or_default().to_string_lossy()).await {
                Resolution::Skip => {
                    transfer.done.fetch_add(item.size, Ordering::Relaxed);
                    continue;
                }
                Resolution::Replace => {}
                Resolution::Rename => target = free_local_name(&target),
            }
        }
        let mut remote = fs.open_read(&item.source).await?;
        let mut local = tokio::fs::File::create(&target).await?;
        copy(&mut remote, &mut local, transfer).await?;
    }
    Ok(())
}

fn free_local_name(target: &Path) -> PathBuf {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
    (1..).map(|n| dir.join(numbered(&name, n))).find(|candidate| !candidate.exists()).unwrap()
}

fn describe(paths: impl Iterator<Item = String>) -> String {
    let names: Vec<String> = paths
        .map(|p| p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("").to_string())
        .collect();
    match names.as_slice() {
        [] => "Transferencia".to_string(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} y {} más", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names_keep_extension_and_hidden_dots() {
        assert_eq!(numbered("informe.txt", 1), "informe (1).txt");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered(".bashrc", 1), ".bashrc (1)");
        assert_eq!(numbered("LEEME", 3), "LEEME (3)");
    }

    #[test]
    fn descriptions_name_the_first_item() {
        assert_eq!(describe(["/a/b/c.txt".to_string()].into_iter()), "c.txt");
        assert_eq!(describe(["/a/x".to_string(), "/a/y".to_string(), "/a/z".to_string()].into_iter()), "x y 2 más");
    }

    #[test]
    fn local_plan_lists_parents_before_children_with_sizes() {
        let dir = std::env::temp_dir().join(format!("omniterm-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj/sub")).unwrap();
        std::fs::write(dir.join("proj/a.txt"), b"12345").unwrap();
        std::fs::write(dir.join("proj/sub/b.txt"), b"123").unwrap();

        let items = plan_local(&[dir.join("proj")], "/srv").unwrap();
        let targets: Vec<_> = items.iter().map(|i| i.target.as_str()).collect();
        assert_eq!(targets, ["/srv/proj", "/srv/proj/a.txt", "/srv/proj/sub", "/srv/proj/sub/b.txt"]);
        assert_eq!(items.iter().map(|i| i.size).sum::<u64>(), 8);
        std::fs::remove_dir_all(dir).ok();
    }
}
