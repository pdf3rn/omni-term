//! SMB 2/3 sobre la crate `smb` (Rust puro, sin SMB1).

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use futures_util::StreamExt;
use smb::{
    Client, ClientConfig, CreateOptions, Directory, FileAccessMask, FileAttributes, FileCreateArgs, FileDispositionInformation,
    FileIdBothDirectoryInformation, FileRenameInformation, ReadAt, Resource, UncPath, WriteAt,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

use crate::files::remotefs::{PipeEnd, Reader, RemoteFs, Writer};
use crate::files::sftp::{self, Entry};
use crate::connect::ssh::Credentials;
use crate::data::store::Connection;

const CHUNK: usize = 60 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SmbFs {
    /// Solo es `None` durante `drop`.
    client: Option<Arc<Client>>,
    base: UncPath,
    /// Los `Drop` de la crate `smb` lanzan tareas de tokio, así que el cliente se libera dentro del runtime
    /// aunque la pestaña se cierre desde el hilo de la interfaz.
    runtime: tokio::runtime::Handle,
}

impl Drop for SmbFs {
    fn drop(&mut self) {
        let _guard = self.runtime.enter();
        drop(self.client.take());
    }
}

fn failure(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

impl SmbFs {
    /// Ruta UNC de una ruta con `/` relativa al recurso compartido.
    fn unc(&self, path: &str) -> UncPath {
        let inner = path.trim_matches('/').replace('/', "\\");
        self.base.clone().with_path(&inner)
    }

    async fn open(&self, path: &str, args: &FileCreateArgs) -> anyhow::Result<Resource> {
        self.client.as_ref().expect("cliente SMB").create_file(&self.unc(path), args).await.map_err(failure).with_context(|| format!("abriendo {path}"))
    }

    async fn directory(&self, path: &str) -> anyhow::Result<Arc<Directory>> {
        let args = FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        match self.open(path, &args).await? {
            Resource::Directory(dir) => Ok(Arc::new(dir)),
            _ => anyhow::bail!("{path} no es una carpeta"),
        }
    }

    async fn entries(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        let dir = self.directory(path).await?;
        let mut stream = Directory::query::<FileIdBothDirectoryInformation>(&dir, "*").await.map_err(failure).with_context(|| format!("leyendo {path}"))?;
        let mut entries = Vec::new();
        while let Some(item) = stream.next().await {
            let info = item.map_err(failure)?;
            let name = info.file_name.to_string();
            if name == "." || name == ".." {
                continue;
            }
            let modified = info.last_write_time.date_time().assume_utc().unix_timestamp();
            entries.push(Entry {
                name,
                is_dir: info.file_attributes.directory(),
                size: info.end_of_file,
                modified: u64::try_from(modified).ok(),
                permissions: None,
            });
        }
        sftp::sort_entries(&mut entries);
        Ok(entries)
    }

    /// Marca para borrar un archivo o una carpeta vacía y la cierra.
    async fn delete_one(&self, path: &str) -> anyhow::Result<()> {
        let args = FileCreateArgs {
            desired_access: FileAccessMask::new().with_delete(true).with_generic_read(true),
            ..FileCreateArgs::make_open_existing(FileAccessMask::new())
        };
        let resource = self.open(path, &args).await?;
        match resource {
            Resource::File(file) => {
                file.set_info(FileDispositionInformation::default()).await.map_err(failure).with_context(|| format!("eliminando {path}"))?;
                let _ = file.close().await;
            }
            Resource::Directory(dir) => {
                dir.set_info(FileDispositionInformation::default()).await.map_err(failure).with_context(|| format!("eliminando {path}"))?;
                let _ = dir.close().await;
            }
            _ => anyhow::bail!("{path} no se puede eliminar"),
        }
        Ok(())
    }
}

#[async_trait]
impl RemoteFs for SmbFs {
    async fn home(&self) -> String {
        "/".into()
    }

    async fn list(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        self.entries(path).await
    }

    async fn exists(&self, path: &str) -> bool {
        let (parent, name) = (sftp::parent(path), sftp::file_name(path).to_string());
        self.entries(&parent).await.map(|list| list.iter().any(|e| e.name.eq_ignore_ascii_case(&name))).unwrap_or(false)
    }

    async fn size(&self, path: &str) -> u64 {
        let (parent, name) = (sftp::parent(path), sftp::file_name(path).to_string());
        self.entries(&parent).await.ok().and_then(|list| list.into_iter().find(|e| e.name == name)).map_or(0, |e| e.size)
    }

    async fn make_dir(&self, path: &str) -> anyhow::Result<()> {
        let args = FileCreateArgs::make_create_new(FileAttributes::new().with_directory(true), CreateOptions::new().with_directory_file(true));
        match self.open(path, &args).await.with_context(|| format!("creando {path}"))? {
            Resource::Directory(dir) => {
                let _ = dir.close().await;
            }
            Resource::File(file) => {
                let _ = file.close().await;
            }
            _ => {}
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let args = FileCreateArgs {
            desired_access: FileAccessMask::new().with_delete(true).with_generic_read(true),
            ..FileCreateArgs::make_open_existing(FileAccessMask::new())
        };
        let new_name = to.trim_matches('/').replace('/', "\\");
        let info = FileRenameInformation { replace_if_exists: false.into(), root_directory: 0, file_name: new_name.as_str().into() };
        match self.open(from, &args).await? {
            Resource::File(file) => {
                file.set_info(info).await.map_err(failure).with_context(|| format!("renombrando {from}"))?;
                let _ = file.close().await;
            }
            Resource::Directory(dir) => {
                dir.set_info(info).await.map_err(failure).with_context(|| format!("renombrando {from}"))?;
                let _ = dir.close().await;
            }
            _ => anyhow::bail!("{from} no se puede renombrar"),
        }
        Ok(())
    }

    async fn remove(&self, path: &str) -> anyhow::Result<()> {
        let is_dir = {
            let (parent, name) = (sftp::parent(path), sftp::file_name(path).to_string());
            self.entries(&parent).await?.into_iter().any(|e| e.name == name && e.is_dir)
        };
        let mut pending = vec![(path.to_string(), is_dir)];
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
            self.delete_one(&file).await?;
        }
        for dir in dirs.into_iter().rev() {
            self.delete_one(&dir).await?;
        }
        Ok(())
    }

    async fn open_read(&self, path: &str) -> anyhow::Result<Reader> {
        let size = self.size(path).await;
        let args = FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        let file = match self.open(path, &args).await? {
            Resource::File(file) => file,
            _ => anyhow::bail!("{path} no es un archivo"),
        };
        let (mut task_side, user_side) = tokio::io::duplex(4 * CHUNK);
        let task: JoinHandle<io::Result<()>> = tokio::spawn(async move {
            let mut buffer = vec![0u8; CHUNK];
            let mut offset = 0u64;
            let result = async {
                while offset < size {
                    let wanted = CHUNK.min((size - offset) as usize);
                    let read = file.read_at(&mut buffer[..wanted], offset).await.map_err(|e| io::Error::other(e.to_string()))?;
                    if read == 0 {
                        break;
                    }
                    task_side.write_all(&buffer[..read]).await?;
                    offset += read as u64;
                }
                Ok(())
            }
            .await;
            let _ = file.close().await;
            result
        });
        Ok(Box::new(PipeEnd::new(user_side, task)))
    }

    async fn open_write(&self, path: &str) -> anyhow::Result<Writer> {
        // Sobrescribir = borrar y crear: no depende de cómo cada servidor informe de «sobrescrito».
        if self.exists(path).await {
            self.delete_one(path).await?;
        }
        let args = FileCreateArgs {
            desired_access: FileAccessMask::new().with_generic_read(true).with_generic_write(true),
            ..FileCreateArgs::make_create_new(FileAttributes::new(), CreateOptions::new().with_non_directory_file(true))
        };
        let file = match self.open(path, &args).await.with_context(|| format!("creando {path}"))? {
            Resource::File(file) => file,
            _ => anyhow::bail!("{path} no es un archivo"),
        };
        let (mut task_side, user_side) = tokio::io::duplex(4 * CHUNK);
        let task: JoinHandle<io::Result<()>> = tokio::spawn(async move {
            let mut buffer = vec![0u8; CHUNK];
            let mut offset = 0u64;
            let result = async {
                loop {
                    let read = task_side.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    let mut written = 0;
                    while written < read {
                        let n = file.write_at(&buffer[written..read], offset).await.map_err(|e| io::Error::other(e.to_string()))?;
                        written += n;
                        offset += n as u64;
                    }
                }
                Ok(())
            }
            .await;
            let _ = file.close().await;
            result
        });
        Ok(Box::new(PipeEnd::new(user_side, task)))
    }
}

pub async fn connect(connection: &Connection, credentials: &Credentials) -> anyhow::Result<Arc<dyn RemoteFs>> {
    if connection.smb_share().trim_matches(['\\', '/']).is_empty() {
        anyhow::bail!("Falta el recurso compartido: edita la conexión e indica su nombre");
    }
    let server = if connection.port == 445 { connection.host.clone() } else { format!("{}:{}", connection.host, connection.port) };
    let base = UncPath::new(&server).map_err(failure)?.with_share(connection.smb_share()).map_err(failure)?;
    let user = if connection.smb_domain().is_empty() { connection.user.clone() } else { format!("{}\\{}", connection.smb_domain(), connection.user) };
    let password = credentials.password.as_ref().map(|p| p.to_string()).unwrap_or_default();
    // Algunos servidores no responden a la negociación multiprotocolo: se reintenta solo con SMB2.
    let mut last_error = None;
    for smb2_only in [false, true] {
        let mut config = ClientConfig::default();
        config.connection.smb2_only_negotiate = smb2_only;
        let client = Client::new(config);
        let attempt = tokio::time::timeout(CONNECT_TIMEOUT, client.share_connect(&base, &user, password.clone())).await.context("tiempo de espera agotado")?;
        match attempt {
            Ok(()) => return Ok(Arc::new(SmbFs { client: Some(Arc::new(client)), base, runtime: tokio::runtime::Handle::current() })),
            Err(error) => {
                let message = error.to_string();
                let retry = !smb2_only && message.contains("wildcard dialect");
                last_error = Some(message);
                if !retry {
                    break;
                }
            }
        }
    }
    Err(anyhow::anyhow!("{}", last_error.unwrap_or_default())).context("el servidor rechazó la conexión (revisa usuario, contraseña y recurso)")
}

