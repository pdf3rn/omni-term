//! Cliente SSH nativo (russh): un canal interactivo con PTY, alimentado por tokio.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use russh::client::{self, Handle, Handler};
use russh::keys::agent::client::AgentClient;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
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
    known_hosts: std::path::PathBuf,
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

        match check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
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
            if let Err(error) = learn_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
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
    known_hosts: std::path::PathBuf,
) -> Arc<dyn Backend> {
    let (commands, receiver) = mpsc::unbounded_channel();
    runtime.spawn(async move {
        let outcome = run(&connection, credentials, size, &mut feed, receiver, events, wake, known_hosts).await;
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
    known_hosts: std::path::PathBuf,
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
        known_hosts,
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

/// Ruta estándar de `known_hosts` del usuario.
pub fn default_known_hosts() -> std::path::PathBuf {
    std::env::home_dir().unwrap_or_default().join(".ssh").join("known_hosts")
}

#[cfg(test)]
mod tests {
    use std::process::{Command as Process, Stdio};
    use std::time::Instant;

    use super::*;
    use crate::term::Session;

    struct Sshd(std::process::Child);

    impl Drop for Sshd {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn screen_text(session: &Session) -> String {
        let term = session.term.lock().unwrap();
        term.renderable_content().display_iter.map(|cell| cell.cell.c).collect()
    }

    /// Prueba de extremo a extremo contra un sshd privado. Se omite si no hay sshd/ssh-keygen.
    #[test]
    fn interactive_shell_over_a_private_sshd() {
        let sshd_path = std::path::Path::new("/usr/sbin/sshd");
        if !sshd_path.exists() || Process::new("ssh-keygen").arg("-?").stderr(Stdio::null()).output().is_err() {
            eprintln!("sshd o ssh-keygen no disponibles; prueba omitida");
            return;
        }

        let dir = std::env::temp_dir().join(format!("omniterm-sshd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = |name: &str| {
            let status = Process::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(dir.join(name))
                .status()
                .unwrap();
            assert!(status.success());
        };
        keygen("host_key");
        keygen("client_key");
        std::fs::copy(dir.join("client_key.pub"), dir.join("authorized_keys")).unwrap();

        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let sshd = Process::new(sshd_path)
            .args(["-D", "-e", "-f", "/dev/null", "-p", &port.to_string()])
            .arg("-h")
            .arg(dir.join("host_key"))
            .args(["-o", "ListenAddress=127.0.0.1", "-o", "PidFile=none", "-o", "UsePAM=no"])
            .args(["-o", "StrictModes=no", "-o"])
            .arg(format!("AuthorizedKeysFile={}", dir.join("authorized_keys").display()))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _sshd = Sshd(sshd);
        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "sshd no arrancó");
            std::thread::sleep(Duration::from_millis(50));
        }

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let (events, requests) = std_mpsc::channel();
        let known_hosts = dir.join("known_hosts");
        let connection = Connection {
            id: None,
            name: "test".into(),
            host: "127.0.0.1".into(),
            port,
            user: std::env::var("USER").unwrap_or_else(|_| "root".into()),
            auth: Auth::Key,
            key_path: Some(dir.join("client_key").display().to_string()),
            init_command: None,
        };
        let handle = runtime.handle().clone();
        let hosts = known_hosts.clone();
        let session = Session::start(80, 24, || {}, move |feed| {
            Ok(spawn(&handle, connection, Credentials::default(), (80, 24), feed, events, Arc::new(|| {}), hosts))
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let request = requests.recv_timeout(Duration::from_secs(10)).expect("sin petición de huella");
        let UiEvent::HostKey(request) = request;
        assert!(request.fingerprint.starts_with("SHA256:"));
        request.reply.send(true).unwrap();

        std::thread::sleep(Duration::from_millis(1500));
        session.write(b"echo omni_$((20+22))\n");
        loop {
            if screen_text(&session).contains("omni_42") {
                break;
            }
            assert!(Instant::now() < deadline, "sin salida del shell remoto:\n{}", screen_text(&session).trim());
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(std::fs::read_to_string(&known_hosts).unwrap().contains("127.0.0.1"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
