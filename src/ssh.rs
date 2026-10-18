//! Cliente SSH nativo (russh): un canal interactivo con PTY, alimentado por tokio.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use russh::client::{self, Handle, Handler};
use russh::keys::agent::client::AgentClient;
use russh::keys::known_hosts::{check_known_hosts, learn_known_hosts};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use crate::store::{Auth, Connection};
use crate::term::{Backend, Feed};

/// Petición de confianza para un servidor que no está en `known_hosts`.
pub struct HostKeyRequest {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub reply: oneshot::Sender<bool>,
}

pub enum UiEvent {
    HostKey(HostKeyRequest),
}

#[derive(Default)]
pub struct Credentials {
    pub password: Option<Zeroizing<String>>,
    pub key_passphrase: Option<Zeroizing<String>>,
}

enum Command {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

struct SshBackend {
    commands: mpsc::UnboundedSender<Command>,
}

impl Backend for SshBackend {
    fn write(&self, bytes: &[u8]) {
        let _ = self.commands.send(Command::Data(bytes.to_vec()));
    }

    fn resize(&self, cols: usize, rows: usize) {
        let _ = self.commands.send(Command::Resize(cols as u32, rows as u32));
    }
}

impl Drop for SshBackend {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

struct Client {
    host: String,
    port: u16,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Motivo por el que se rechazó la clave del servidor, para mostrarlo en la terminal.
    rejection: Arc<Mutex<Option<String>>>,
}

impl Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            *self.rejection.lock().unwrap() = Some("el servidor usa un certificado de host, no soportado aún".into());
            return Ok(false);
        };

        match check_known_hosts(&self.host, self.port, key) {
            Ok(true) => return Ok(true),
            Err(russh::keys::Error::KeyChanged { line }) => {
                *self.rejection.lock().unwrap() = Some(format!(
                    "¡LA HUELLA DEL SERVIDOR HA CAMBIADO! (línea {line} de ~/.ssh/known_hosts). \
                     Podría ser un ataque de intermediario. Conexión cancelada."
                ));
                return Ok(false);
            }
            Ok(false) | Err(_) => {}
        }

        let (reply, answer) = oneshot::channel();
        let request = HostKeyRequest {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            reply,
        };
        if self.events.send(UiEvent::HostKey(request)).is_err() {
            return Ok(false);
        }
        (self.wake)();

        if answer.await.unwrap_or(false) {
            if let Err(error) = learn_known_hosts(&self.host, self.port, key) {
                eprintln!("no se pudo escribir known_hosts: {error}");
            }
            Ok(true)
        } else {
            *self.rejection.lock().unwrap() = Some("servidor no confiable, conexión cancelada".into());
            Ok(false)
        }
    }
}

/// `true` si la llave privada necesita passphrase.
pub fn key_is_encrypted(path: &str) -> anyhow::Result<bool> {
    match russh::keys::load_secret_key(expand_home(path), None) {
        Ok(_) => Ok(false),
        Err(russh::keys::Error::KeyIsEncrypted) => Ok(true),
        Err(error) => Err(anyhow!("no se pudo leer la llave {path}: {error}")),
    }
}

fn expand_home(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => path.into(),
    }
}

/// Lanza la conexión en segundo plano y devuelve el backend de inmediato.
/// El progreso y los errores se escriben en la propia terminal.
pub fn spawn(
    runtime: &tokio::runtime::Handle,
    connection: Connection,
    credentials: Credentials,
    size: (usize, usize),
    mut feed: Feed,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Arc<dyn Backend> {
    let (commands, receiver) = mpsc::unbounded_channel();
    runtime.spawn(async move {
        let outcome = run(&connection, credentials, size, &mut feed, receiver, events, wake).await;
        match outcome {
            Ok(()) => feed.close(),
            Err(error) => {
                feed.feed(format!("\r\n\x1b[31m[omniterm] {error:#}\x1b[0m\r\n").as_bytes());
            }
        }
    });
    Arc::new(SshBackend { commands })
}

async fn run(
    connection: &Connection,
    credentials: Credentials,
    (cols, rows): (usize, usize),
    feed: &mut Feed,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<()> {
    feed.feed(
        format!("Conectando a {}@{}:{}...\r\n", connection.user, connection.host, connection.port).as_bytes(),
    );

    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let rejection = Arc::new(Mutex::new(None));
    let handler = Client {
        host: connection.host.clone(),
        port: connection.port,
        events,
        wake,
        rejection: rejection.clone(),
    };

    let mut session = match client::connect(config, (connection.host.as_str(), connection.port), handler).await {
        Ok(session) => session,
        Err(error) => {
            let reason = rejection.lock().unwrap().take();
            return Err(match reason {
                Some(reason) => anyhow!(reason),
                None => anyhow!("no se pudo conectar: {error}"),
            });
        }
    };

    authenticate(&mut session, connection, &credentials).await?;
    feed.feed(b"Sesion iniciada.\r\n");

    let mut channel = session.channel_open_session().await.context("abriendo el canal")?;
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .context("solicitando PTY")?;
    channel.request_shell(true).await.context("iniciando el shell")?;
    if let Some(command) = connection.init_command.as_deref().filter(|c| !c.is_empty()) {
        channel.data(format!("{command}\n").as_bytes()).await?;
    }

    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => feed.feed(&data),
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            command = commands.recv() => match command {
                Some(Command::Data(bytes)) => channel.data(&bytes[..]).await?,
                Some(Command::Resize(cols, rows)) => channel.window_change(cols, rows, 0, 0).await?,
                Some(Command::Close) | None => break,
            },
        }
    }
    let _ = session.disconnect(russh::Disconnect::ByApplication, "", "es").await;
    Ok(())
}

async fn authenticate(
    session: &mut Handle<Client>,
    connection: &Connection,
    credentials: &Credentials,
) -> anyhow::Result<()> {
    let user = connection.user.clone();
    let success = match connection.auth {
        Auth::Password => {
            let password = credentials.password.as_ref().context("falta la contraseña")?;
            session.authenticate_password(user, password.as_str()).await?.success()
        }
        Auth::Key => {
            let path = connection.key_path.as_deref().context("falta la ruta de la llave privada")?;
            let passphrase = credentials.key_passphrase.as_ref().map(|p| p.as_str());
            let key = russh::keys::load_secret_key(expand_home(path), passphrase)
                .map_err(|e| anyhow!("no se pudo leer la llave {path}: {e}"))?;
            let hash = session.best_supported_rsa_hash().await?.flatten();
            session.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?.success()
        }
        Auth::Agent => {
            let mut agent = AgentClient::connect_env().await.map_err(|e| anyhow!("no hay ssh-agent: {e}"))?;
            let identities = agent.request_identities().await.map_err(|e| anyhow!("ssh-agent: {e}"))?;
            if identities.is_empty() {
                bail!("el ssh-agent no tiene identidades cargadas (ssh-add)");
            }
            let hash = session.best_supported_rsa_hash().await?.flatten();
            let mut accepted = false;
            for identity in identities {
                let key = identity.public_key().into_owned();
                let result = session
                    .authenticate_publickey_with(user.clone(), key, hash, &mut agent)
                    .await
                    .map_err(|e| anyhow!("firmando con el agente: {e}"))?;
                if result.success() {
                    accepted = true;
                    break;
                }
            }
            accepted
        }
    };
    if !success {
        bail!("autenticación rechazada por el servidor");
    }
    Ok(())
}
