//! FTP y FTPS (explícito e implícito) sobre `suppaftp`.
//!
//! Una conexión de control sirve para listar, crear, renombrar y borrar; cada transferencia abre
//! su propia conexión para no bloquear la navegación mientras se sube o baja un archivo.

use std::io;
use std::net::ToSocketAddrs;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Context as _;
use async_trait::async_trait;
use suppaftp::list::File;
use suppaftp::tokio::{AsyncFtpStream, AsyncNoTlsStream, AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream, TransferStream};
use suppaftp::types::{FileType, Mode};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::files::remotefs::{Reader, RemoteFs, Writer};
use crate::files::sftp::{self, Entry};
use crate::connect::ssh::Credentials;
use crate::data::store::{Connection, FilesKind, FtpOptions, FtpTls};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    user: String,
    password: String,
    options: FtpOptions,
}

enum Client {
    Plain(AsyncFtpStream),
    Tls(AsyncRustlsFtpStream),
}

/// Ejecuta la misma operación sobre cualquiera de los dos tipos de cliente.
macro_rules! on_client {
    ($client:expr, $c:ident => $body:expr) => {
        match $client {
            Client::Plain($c) => $body,
            Client::Tls($c) => $body,
        }
    };
}

fn tls_connector() -> AsyncRustlsConnector {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versiones de TLS por defecto")
        .with_root_certificates(roots)
        .with_no_client_auth();
    AsyncRustlsConnector::from(TlsConnector::from(Arc::new(config)))
}

async fn open_client(config: &Config) -> anyhow::Result<Client> {
    let address = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .with_context(|| format!("resolviendo {}", config.host))?
        .next()
        .with_context(|| format!("sin dirección para {}", config.host))?;
    let connect = async {
        Ok::<_, anyhow::Error>(match config.options.tls {
            FtpTls::None => Client::Plain(AsyncFtpStream::connect_timeout(address, CONNECT_TIMEOUT).await?),
            FtpTls::Explicit => {
                let plain = AsyncRustlsFtpStream::connect_timeout(address, CONNECT_TIMEOUT).await?;
                Client::Tls(plain.into_secure(tls_connector(), &config.host).await.context("negociando TLS (el certificado debe ser de confianza)")?)
            }
            FtpTls::Implicit => Client::Tls(
                AsyncRustlsFtpStream::connect_secure_implicit(address, tls_connector(), &config.host)
                    .await
                    .context("conectando con TLS implícito (el certificado debe ser de confianza)")?,
            ),
        })
    };
    let mut client = tokio::time::timeout(CONNECT_TIMEOUT, connect).await.context("tiempo de espera agotado")??;
    on_client!(&mut client, c => {
        c.login(config.user.as_str(), config.password.as_str()).await.context("el servidor rechazó el usuario o la contraseña")?;
        c.set_mode(if config.options.passive { Mode::Passive } else { Mode::Active });
        c.transfer_type(FileType::Binary).await?;
    });
    Ok(client)
}

fn entry_from(file: &File) -> Option<Entry> {
    let name = file.name().to_string();
    if name == "." || name == ".." {
        return None;
    }
    Some(Entry {
        name,
        is_dir: file.is_directory(),
        size: file.size() as u64,
        modified: file.modified().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()),
        permissions: None,
    })
}

pub struct FtpFs {
    config: Config,
    control: tokio::sync::Mutex<Client>,
}

impl FtpFs {
    async fn with_control<T>(&self, work: impl for<'a> FnOnce(&'a mut Client) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send + 'a>>) -> anyhow::Result<T> {
        let mut client = self.control.lock().await;
        // El servidor puede haber cerrado la conexión por inactividad: se rehace antes de seguir.
        let alive = on_client!(&mut *client, c => c.noop().await.is_ok());
        if !alive {
            *client = open_client(&self.config).await?;
        }
        work(&mut client).await
    }

    async fn entries(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        let path = path.to_string();
        self.with_control(move |client| {
            Box::pin(async move {
                let lines = on_client!(client, c => c.list(Some(path.as_str())).await).with_context(|| format!("leyendo {path}"))?;
                let mut entries: Vec<Entry> = lines.iter().filter_map(|line| File::try_from(line.as_str()).ok()).filter_map(|f| entry_from(&f)).collect();
                sftp::sort_entries(&mut entries);
                Ok(entries)
            })
        })
        .await
    }
}

#[async_trait]
impl RemoteFs for FtpFs {
    async fn home(&self) -> String {
        self.with_control(|client| Box::pin(async move { Ok(on_client!(client, c => c.pwd().await)?) })).await.unwrap_or_else(|_| "/".into())
    }

    async fn list(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        self.entries(path).await
    }

    async fn exists(&self, path: &str) -> bool {
        let (parent, name) = (sftp::parent(path), sftp::file_name(path).to_string());
        self.entries(&parent).await.map(|list| list.iter().any(|e| e.name == name)).unwrap_or(false)
    }

    async fn size(&self, path: &str) -> u64 {
        let path = path.to_string();
        self.with_control(move |client| Box::pin(async move { Ok(on_client!(client, c => c.size(path.as_str()).await)? as u64) })).await.unwrap_or(0)
    }

    async fn make_dir(&self, path: &str) -> anyhow::Result<()> {
        let path = path.to_string();
        self.with_control(move |client| Box::pin(async move { Ok(on_client!(client, c => c.mkdir(path.as_str()).await).with_context(|| format!("creando {path}"))?) })).await
    }

    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let (from, to) = (from.to_string(), to.to_string());
        self.with_control(move |client| {
            Box::pin(async move { Ok(on_client!(client, c => c.rename(from.as_str(), to.as_str()).await).with_context(|| format!("renombrando {from}"))?) })
        })
        .await
    }

    async fn remove(&self, path: &str) -> anyhow::Result<()> {
        // Se recorre el árbol de arriba abajo y se borra de abajo arriba: las carpetas se vacían antes.
        let mut pending = vec![(path.to_string(), !self.exists_as_file(path).await)];
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        while let Some((current, is_dir)) = pending.pop() {
            if is_dir {
                for child in self.entries(&current).await? {
                    pending.push((sftp::join(&current, &child.name), child.is_dir));
                }
                dirs.push(current);
            } else {
                files.push(current);
            }
        }
        for file in files {
            self.with_control(move |client| Box::pin(async move { Ok(on_client!(client, c => c.rm(file.as_str()).await).with_context(|| format!("eliminando {file}"))?) })).await?;
        }
        for dir in dirs.into_iter().rev() {
            self.with_control(move |client| Box::pin(async move { Ok(on_client!(client, c => c.rmdir(dir.as_str()).await).with_context(|| format!("eliminando {dir}"))?) })).await?;
        }
        Ok(())
    }

    async fn open_read(&self, path: &str) -> anyhow::Result<Reader> {
        let mut client = open_client(&self.config).await?;
        let transfer = match &mut client {
            Client::Plain(c) => Transfer::Plain(c.retr_as_stream(path).await.with_context(|| format!("abriendo {path}"))?),
            Client::Tls(c) => Transfer::Tls(c.retr_as_stream(path).await.with_context(|| format!("abriendo {path}"))?),
        };
        Ok(Box::new(TransferIo { transfer: Some(transfer), finishing: None, _client: client }))
    }

    async fn open_write(&self, path: &str) -> anyhow::Result<Writer> {
        let mut client = open_client(&self.config).await?;
        let transfer = match &mut client {
            Client::Plain(c) => Transfer::Plain(c.put_with_stream(path).await.with_context(|| format!("creando {path}"))?),
            Client::Tls(c) => Transfer::Tls(c.put_with_stream(path).await.with_context(|| format!("creando {path}"))?),
        };
        Ok(Box::new(TransferIo { transfer: Some(transfer), finishing: None, _client: client }))
    }
}

impl FtpFs {
    /// ¿`path` es un archivo (y no una carpeta)? Para decidir cómo borrarlo.
    async fn exists_as_file(&self, path: &str) -> bool {
        let (parent, name) = (sftp::parent(path), sftp::file_name(path).to_string());
        self.entries(&parent).await.map(|list| list.iter().any(|e| e.name == name && !e.is_dir)).unwrap_or(false)
    }
}

enum Transfer {
    Plain(TransferStream<AsyncNoTlsStream>),
    Tls(TransferStream<AsyncRustlsStream>),
}

impl Transfer {
    async fn finish(self) -> io::Result<()> {
        let result = match self {
            Transfer::Plain(t) => t.finish().await,
            Transfer::Tls(t) => t.finish().await,
        };
        result.map_err(|e| io::Error::other(e.to_string()))
    }
}

/// Lectura o escritura de un archivo: al cerrar la escritura espera la respuesta final del servidor.
struct TransferIo {
    transfer: Option<Transfer>,
    finishing: Option<Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send>>>,
    /// Mantiene viva la conexión de control mientras dura la transferencia.
    _client: Client,
}

macro_rules! on_transfer {
    ($self:expr, $t:ident => $body:expr) => {
        match $self.transfer.as_mut() {
            Some(Transfer::Plain($t)) => $body,
            Some(Transfer::Tls($t)) => $body,
            None => Poll::Ready(Err(io::Error::other("la transferencia ya terminó"))),
        }
    };
}

impl AsyncRead for TransferIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        on_transfer!(self, t => Pin::new(t).poll_read(cx, buf))
    }
}

impl AsyncWrite for TransferIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        on_transfer!(self, t => Pin::new(t).poll_write(cx, buf))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.transfer.is_none() {
            return Poll::Ready(Ok(()));
        }
        on_transfer!(self, t => Pin::new(t).poll_flush(cx))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.finishing.is_none() {
            match self.transfer.take() {
                Some(transfer) => self.finishing = Some(Box::pin(transfer.finish())),
                None => return Poll::Ready(Ok(())),
            }
        }
        let future = self.finishing.as_mut().expect("cierre en curso");
        future.as_mut().poll(cx)
    }
}

pub async fn connect(connection: &Connection, credentials: &Credentials) -> anyhow::Result<Arc<dyn RemoteFs>> {
    let FilesKind::Ftp(options) = (match connection.protocol {
        crate::data::store::Protocol::Files(kind) => kind,
        _ => anyhow::bail!("la conexión no es FTP"),
    }) else {
        anyhow::bail!("la conexión no es FTP");
    };
    let (user, password) = if options.anonymous {
        ("anonymous".to_string(), "anonymous@".to_string())
    } else {
        (connection.user.clone(), credentials.password.as_ref().map(|p| p.to_string()).unwrap_or_default())
    };
    let config = Config { host: connection.host.clone(), port: connection.port, user, password, options };
    let client = open_client(&config).await?;
    Ok(Arc::new(FtpFs { config, control: tokio::sync::Mutex::new(client) }))
}
