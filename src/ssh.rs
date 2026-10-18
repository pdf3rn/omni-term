//! Cliente SSH nativo (russh): un canal interactivo con PTY, alimentado por tokio.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use russh::client::{self, Handle, Handler};
use russh::keys::agent::client::AgentClient;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::client::{ChannelOpenHandle, Msg, Session as ClientSession};
use russh::{Channel, ChannelMsg};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use crate::store::{Auth, Connection, Forward, ForwardKind};
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

#[derive(Default, Clone)]
pub struct Credentials {
    pub password: Option<Zeroizing<String>>,
    pub key_passphrase: Option<Zeroizing<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TunnelState {
    Starting,
    Active,
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub struct TunnelStatus {
    pub forward: Forward,
    pub state: TunnelState,
    /// Conexiones atendidas desde que arrancó el túnel.
    pub served: u64,
}

/// Estado compartido de los túneles de una sesión, en el orden de `forwards`.
pub type Tunnels = Arc<Mutex<Vec<TunnelStatus>>>;

pub fn new_tunnels(forwards: &[Forward]) -> Tunnels {
    Arc::new(Mutex::new(
        forwards
            .iter()
            .map(|forward| TunnelStatus { forward: forward.clone(), state: TunnelState::Starting, served: 0 })
            .collect(),
    ))
}

fn set_state(tunnels: &Tunnels, index: usize, state: TunnelState) {
    if let Some(status) = tunnels.lock().unwrap().get_mut(index) {
        status.state = state;
    }
}

fn count_served(tunnels: &Tunnels, index: usize) {
    if let Some(status) = tunnels.lock().unwrap().get_mut(index) {
        status.served += 1;
    }
}

fn stop_all(tunnels: &Tunnels) {
    for status in tunnels.lock().unwrap().iter_mut() {
        if status.state == TunnelState::Active || status.state == TunnelState::Starting {
            status.state = TunnelState::Stopped;
        }
    }
}

/// Destino de un reenvío remoto (`-R`) registrado en el servidor.
pub(crate) struct RemoteTarget {
    bind_port: u32,
    dest_host: String,
    dest_port: u16,
    index: usize,
}

async fn pump<A, B>(mut a: A, mut b: B)
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
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

pub(crate) struct Client {
    known_hosts: std::path::PathBuf,
    host: String,
    port: u16,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Motivo por el que se rechazó la clave del servidor, para mostrarlo en la terminal.
    rejection: Arc<Mutex<Option<String>>>,
    remote: Arc<Mutex<Vec<RemoteTarget>>>,
    tunnels: Tunnels,
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

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut ClientSession,
    ) -> Result<(), Self::Error> {
        let target = {
            let remote = self.remote.lock().unwrap();
            remote
                .iter()
                .find(|t| t.bind_port == connected_port)
                .map(|t| (t.dest_host.clone(), t.dest_port, t.index))
        };
        let Some((dest_host, dest_port, index)) = target else {
            eprintln!("conexión remota inesperada en {connected_address}:{connected_port}");
            reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        };
        reply.accept().await;
        let tunnels = self.tunnels.clone();
        tokio::spawn(async move {
            if let Ok(local) = tokio::net::TcpStream::connect((dest_host.as_str(), dest_port)).await {
                count_served(&tunnels, index);
                pump(local, channel.into_stream()).await;
            }
        });
        Ok(())
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
    tunnels: Tunnels,
) -> Arc<dyn Backend> {
    let (commands, receiver) = mpsc::unbounded_channel();
    runtime.spawn(async move {
        let outcome = run(&connection, credentials, size, &mut feed, receiver, events, wake, known_hosts, tunnels.clone()).await;
        stop_all(&tunnels);
        match outcome {
            Ok(()) => feed.close(),
            Err(error) => {
                feed.feed(format!("\r\n\x1b[31m[omni-term] {error:#}\x1b[0m\r\n").as_bytes());
                feed.disconnect();
            }
        }
    });
    Arc::new(SshBackend { commands })
}

/// Conecta, verifica la huella del servidor y autentica. Lo comparten el terminal y el explorador SFTP.
pub(crate) async fn establish(
    connection: &Connection,
    credentials: &Credentials,
    events: std_mpsc::Sender<UiEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    known_hosts: std::path::PathBuf,
    remote: Arc<Mutex<Vec<RemoteTarget>>>,
    tunnels: Tunnels,
) -> anyhow::Result<Handle<Client>> {
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
        remote,
        tunnels,
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
    authenticate(&mut session, connection, credentials).await?;
    Ok(session)
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
    tunnels: Tunnels,
) -> anyhow::Result<()> {
    feed.feed(
        format!("Conectando a {}@{}:{}...\r\n", connection.user, connection.host, connection.port).as_bytes(),
    );

    let remote = Arc::new(Mutex::new(Vec::new()));
    let session = establish(connection, &credentials, events, wake, known_hosts, remote.clone(), tunnels.clone()).await?;
    feed.feed(b"Sesion iniciada.\r\n");

    let session = Arc::new(session);
    // Al terminar `run`, el JoinSet cancela los oyentes de los túneles.
    let mut tasks = tokio::task::JoinSet::new();
    start_forwards(&session, &tunnels, &remote, &mut tasks).await;

    let mut channel = session.channel_open_session().await.context("abriendo el canal")?;
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .context("solicitando PTY")?;
    channel.request_shell(true).await.context("iniciando el shell")?;
    if let Some(command) = connection.init_command.as_deref().filter(|c| !c.is_empty()) {
        channel.data(format!("{command}\n").as_bytes()).await?;
    }

    let mut shell_exited = false;
    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => feed.feed(&data),
                Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::ExitSignal { .. }) => shell_exited = true,
                // Canal cerrado: ordenado (`exit`, `logout`) si el transporte sigue vivo; si no, caída.
                Some(ChannelMsg::Close) | None => {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if shell_exited || !session.is_closed() {
                        break;
                    }
                    bail!("la conexión se cerró de forma inesperada");
                }
                Some(ChannelMsg::Eof) => {}
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

async fn start_forwards(
    session: &Arc<Handle<Client>>,
    tunnels: &Tunnels,
    remote: &Arc<Mutex<Vec<RemoteTarget>>>,
    tasks: &mut tokio::task::JoinSet<()>,
) {
    let forwards: Vec<Forward> = tunnels.lock().unwrap().iter().map(|t| t.forward.clone()).collect();
    for (index, forward) in forwards.into_iter().enumerate() {
        match forward.kind {
            ForwardKind::Local => {
                tasks.spawn(local_forward(session.clone(), tunnels.clone(), index, forward));
            }
            ForwardKind::Remote => {
                remote.lock().unwrap().push(RemoteTarget {
                    bind_port: forward.bind_port as u32,
                    dest_host: forward.dest_host.clone(),
                    dest_port: forward.dest_port,
                    index,
                });
                match session.tcpip_forward(forward.bind_host.clone(), forward.bind_port as u32).await {
                    Ok(_) => set_state(tunnels, index, TunnelState::Active),
                    Err(error) => {
                        remote.lock().unwrap().retain(|t| t.index != index);
                        set_state(
                            tunnels,
                            index,
                            TunnelState::Failed(format!("el servidor rechazó el reenvío remoto: {error}")),
                        );
                    }
                }
            }
        }
    }
}

async fn local_forward(session: Arc<Handle<Client>>, tunnels: Tunnels, index: usize, forward: Forward) {
    let listener = match tokio::net::TcpListener::bind((forward.bind_host.as_str(), forward.bind_port)).await {
        Ok(listener) => listener,
        Err(error) => {
            let reason = format!("no se pudo escuchar en {}:{}: {error}", forward.bind_host, forward.bind_port);
            set_state(&tunnels, index, TunnelState::Failed(reason));
            return;
        }
    };
    set_state(&tunnels, index, TunnelState::Active);
    while let Ok((local, peer)) = listener.accept().await {
        let session = session.clone();
        let tunnels = tunnels.clone();
        let dest_host = forward.dest_host.clone();
        let dest_port = forward.dest_port as u32;
        tokio::spawn(async move {
            let opened = session
                .channel_open_direct_tcpip(dest_host, dest_port, peer.ip().to_string(), peer.port() as u32)
                .await;
            if let Ok(channel) = opened {
                count_served(&tunnels, index);
                pump(local, channel.into_stream()).await;
            }
        });
    }
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
    use std::time::Instant;

    use super::*;
    use crate::term::Session;

    /// Proxy TCP que se puede cortar para simular una caída de red.
    struct CuttableProxy {
        port: u16,
        streams: Arc<Mutex<Vec<std::net::TcpStream>>>,
    }

    impl CuttableProxy {
        fn start(target: u16) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let streams = Arc::new(Mutex::new(Vec::new()));
            let kept = streams.clone();
            std::thread::spawn(move || {
                while let Ok((client, _)) = listener.accept() {
                    let Ok(server) = std::net::TcpStream::connect(("127.0.0.1", target)) else { continue };
                    kept.lock().unwrap().extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                    for (mut from, mut to) in [(client.try_clone().unwrap(), server.try_clone().unwrap()), (server, client)] {
                        std::thread::spawn(move || {
                            let _ = std::io::copy(&mut from, &mut to);
                        });
                    }
                }
            });
            Self { port, streams }
        }

        fn cut(&self) {
            for stream in self.streams.lock().unwrap().iter() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    fn screen_text(session: &Session) -> String {
        let term = session.term.lock().unwrap();
        term.renderable_content().display_iter.map(|cell| cell.cell.c).collect()
    }

    /// Prueba de extremo a extremo contra un sshd privado. Solo Linux; se omite si no hay sshd/ssh-keygen.
    #[cfg(target_os = "linux")]
    #[test]
    fn interactive_shell_over_a_private_sshd() {
        let Some(sshd) = crate::testutil::PrivateSshd::start() else { return };
        let (dir, port) = (sshd.dir.clone(), sshd.port);

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let (events, requests) = std_mpsc::channel();
        let known_hosts = dir.join("known_hosts");
        let connection = Connection {
            id: None,
            name: "test".into(),
            host: "127.0.0.1".into(),
            port,
            user: sshd.user.clone(),
            auth: Auth::Key,
            key_path: Some(sshd.key_path.clone()),
            init_command: None,
        };
        let again = connection.clone();

        // Destino de los túneles: un servidor que saluda al conectar.
        let hello = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let hello_port = hello.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::Write;
            while let Ok((mut stream, _)) = hello.accept() {
                let _ = stream.write_all(b"hello-omni\n");
            }
        });
        let free_port = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (local_bind, remote_bind) = (free_port(), free_port());
        let forward = |kind, bind_port| Forward {
            kind,
            bind_host: "127.0.0.1".into(),
            bind_port,
            dest_host: "127.0.0.1".into(),
            dest_port: hello_port,
        };
        let forwards = vec![
            forward(ForwardKind::Local, local_bind),
            forward(ForwardKind::Remote, remote_bind),
            forward(ForwardKind::Local, local_bind), // puerto repetido: debe fallar sin tumbar la sesión
        ];
        let tunnels = new_tunnels(&forwards);
        let tunnels_for_spawn = tunnels.clone();
        let handle = runtime.handle().clone();
        let hosts = known_hosts.clone();
        let session = Session::start(80, 24, || {}, move |feed| {
            Ok(spawn(&handle, connection, Credentials::default(), (80, 24), feed, events, Arc::new(|| {}), hosts, tunnels_for_spawn))
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

        // Túneles -L y -R atendiendo conexiones reales; el tercero (puerto repetido) falla aislado.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let states: Vec<TunnelState> = tunnels.lock().unwrap().iter().map(|t| t.state.clone()).collect();
            if states[0] == TunnelState::Active && states[1] == TunnelState::Active && matches!(states[2], TunnelState::Failed(_)) {
                break;
            }
            assert!(Instant::now() < deadline, "estado de túneles inesperado: {states:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
        for port in [local_bind, remote_bind] {
            use std::io::{BufRead, BufReader};
            let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "hello-omni", "túnel en el puerto {port}");
        }
        assert!(tunnels.lock().unwrap()[0].served >= 1 && tunnels.lock().unwrap()[1].served >= 1);

        // Un `exit` del usuario cierra la pestaña; no cuenta como conexión perdida.
        session.write(b"exit\n");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !session.has_exited() {
            assert!(Instant::now() < deadline, "la sesión no terminó tras exit (desconectada: {}):\n{}", session.is_disconnected(), screen_text(&session).trim());
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!session.is_disconnected());

        // Una caída abrupta (aquí, cortando un proxy TCP) deja la pestaña abierta y desconectada.
        let proxy = CuttableProxy::start(port);
        let mut again = again;
        again.port = proxy.port;
        let (events2, requests2) = std_mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(UiEvent::HostKey(request)) = requests2.recv() {
                let _ = request.reply.send(true);
            }
        });
        let handle = runtime.handle().clone();
        let hosts = known_hosts.clone();
        let second = Session::start(80, 24, || {}, move |feed| {
            Ok(spawn(&handle, again, Credentials::default(), (80, 24), feed, events2, Arc::new(|| {}), hosts, new_tunnels(&[])))
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !screen_text(&second).contains("Sesion iniciada") {
            assert!(Instant::now() < deadline, "la segunda sesión no conectó:\n{}", screen_text(&second).trim());
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        proxy.cut();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !second.is_disconnected() {
            assert!(Instant::now() < deadline, "no se detectó la caída:\n{}", screen_text(&second).trim());
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!second.has_exited());
        std::fs::remove_dir_all(&dir).ok();
    }
}
