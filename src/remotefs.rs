//! Capa común de «sistema de archivos remoto»: el explorador y las transferencias no saben
//! si hablan SFTP, FTP o SMB.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{mpsc as std_mpsc, Arc};
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::task::JoinHandle;

use crate::sftp::{self, Entry, Sftp};
use crate::ssh::{Credentials, UiEvent};
use crate::store::{Connection, FilesKind, Protocol};

pub type Reader = Box<dyn AsyncRead + Send + Unpin>;
pub type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// Rutas con `/` como separador, también en SMB (cada implementación traduce las suyas).
#[async_trait]
pub trait RemoteFs: Send + Sync {
    /// Carpeta inicial.
    async fn home(&self) -> String;
    async fn list(&self, path: &str) -> anyhow::Result<Vec<Entry>>;
    async fn exists(&self, path: &str) -> bool;
    async fn size(&self, path: &str) -> u64;
    async fn make_dir(&self, path: &str) -> anyhow::Result<()>;
    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()>;
    /// Borra un archivo o una carpeta con todo su contenido.
    async fn remove(&self, path: &str) -> anyhow::Result<()>;
    async fn open_read(&self, path: &str) -> anyhow::Result<Reader>;
    /// Crea (o sobrescribe) un archivo. `shutdown` debe dejarlo completo en el servidor.
    async fn open_write(&self, path: &str) -> anyhow::Result<Writer>;

    /// ¿Admite cambiar permisos POSIX? Si no, el botón «Permisos» se deshabilita.
    fn supports_permissions(&self) -> bool {
        false
    }

    async fn set_permissions(&self, _path: &str, _mode: u32) -> anyhow::Result<()> {
        anyhow::bail!("este protocolo no admite cambiar permisos")
    }

    /// ¿Se perdió la conexión?
    fn is_closed(&self) -> bool {
        false
    }
}

#[async_trait]
impl RemoteFs for Sftp {
    async fn home(&self) -> String {
        sftp::home(&self.session).await
    }

    async fn list(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        sftp::list(&self.session, path).await
    }

    async fn exists(&self, path: &str) -> bool {
        self.session.try_exists(path.to_string()).await.unwrap_or(false)
    }

    async fn size(&self, path: &str) -> u64 {
        self.session.metadata(path.to_string()).await.ok().and_then(|m| m.size).unwrap_or(0)
    }

    async fn make_dir(&self, path: &str) -> anyhow::Result<()> {
        sftp::make_dir(&self.session, path).await
    }

    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        sftp::rename(&self.session, from, to).await
    }

    async fn remove(&self, path: &str) -> anyhow::Result<()> {
        sftp::remove(&self.session, path).await
    }

    async fn open_read(&self, path: &str) -> anyhow::Result<Reader> {
        Ok(Box::new(self.session.open(path.to_string()).await?))
    }

    async fn open_write(&self, path: &str) -> anyhow::Result<Writer> {
        Ok(Box::new(self.session.create(path.to_string()).await?))
    }

    fn supports_permissions(&self) -> bool {
        true
    }

    async fn set_permissions(&self, path: &str, mode: u32) -> anyhow::Result<()> {
        sftp::set_permissions(&self.session, path, mode).await
    }

    fn is_closed(&self) -> bool {
        Sftp::is_closed(self)
    }
}

/// Conecta con el protocolo de la conexión.
pub async fn connect(
    connection: &Connection,
    credentials: &Credentials,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    known_hosts: std::path::PathBuf,
) -> anyhow::Result<Arc<dyn RemoteFs>> {
    match connection.protocol {
        Protocol::Files(FilesKind::Ftp(_)) => crate::ftp::connect(connection, credentials).await,
        Protocol::Files(FilesKind::Smb) => crate::smb::connect(connection, credentials).await,
        Protocol::Adb => crate::adb::connect(connection).await,
        _ => {
            let client = sftp::connect(connection, credentials, events, wake, known_hosts).await?;
            Ok(Arc::new(client))
        }
    }
}

/// Extremo de la tubería hacia la tarea que habla el protocolo remoto: al terminar (EOF al leer, `shutdown` al escribir)
/// espera el resultado de la tarea, para que un fallo de red llegue como error y no como archivo truncado.
pub struct PipeEnd {
    inner: DuplexStream,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl PipeEnd {
    pub fn new(inner: DuplexStream, task: JoinHandle<io::Result<()>>) -> Self {
        Self { inner, task: Some(task) }
    }
}

impl PipeEnd {
    fn poll_task(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(task) = self.task.as_mut() else { return Poll::Ready(Ok(())) };
        match Pin::new(task).poll(cx) {
            Poll::Ready(result) => {
                self.task = None;
                Poll::Ready(result.unwrap_or_else(|e| Err(io::Error::other(e.to_string()))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncRead for PipeEnd {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before => self.poll_task(cx),
            other => other,
        }
    }
}

impl AsyncWrite for PipeEnd {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.inner).poll_shutdown(cx) {
            Poll::Ready(Ok(())) => self.poll_task(cx),
            other => other,
        }
    }
}
