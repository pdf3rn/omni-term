//! Ayudas para pruebas de extremo a extremo: un sshd privado en un puerto libre.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct PrivateSshd {
    pub port: u16,
    pub dir: PathBuf,
    pub key_path: String,
    pub user: String,
    child: Child,
}

impl PrivateSshd {
    /// Arranca un sshd de usuario con autenticación por llave y SFTP interno.
    /// Devuelve `None` (y la prueba se omite) si no hay sshd o ssh-keygen.
    pub fn start() -> Option<Self> {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sshd_path = std::path::Path::new("/usr/sbin/sshd");
        if !sshd_path.exists() || Command::new("ssh-keygen").arg("-?").stderr(Stdio::null()).output().is_err() {
            eprintln!("sshd o ssh-keygen no disponibles; prueba omitida");
            return None;
        }

        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("omniterm-sshd-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = |name: &str| {
            let status = Command::new("ssh-keygen")
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
        let child = Command::new(sshd_path)
            .args(["-D", "-E"])
            .arg(dir.join("sshd.log"))
            .args(["-f", "/dev/null", "-p", &port.to_string()])
            .arg("-h")
            .arg(dir.join("host_key"))
            .args(["-o", "ListenAddress=127.0.0.1", "-o", "PidFile=none", "-o", "UsePAM=no", "-o", "LogLevel=VERBOSE"])
            .args(["-o", "StrictModes=no", "-o", "Subsystem=sftp internal-sftp", "-o"])
            .arg(format!("AuthorizedKeysFile={}", dir.join("authorized_keys").display()))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sshd = Self {
            port,
            key_path: dir.join("client_key").display().to_string(),
            dir,
            user: std::env::var("USER").unwrap_or_else(|_| "root".into()),
            child,
        };

        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "sshd no arrancó");
            std::thread::sleep(Duration::from_millis(50));
        }
        Some(sshd)
    }
}

impl PrivateSshd {
    /// Registro del sshd, para comprobar por dónde pasó una conexión.
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("sshd.log")).unwrap_or_default()
    }
}

impl Drop for PrivateSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Servidor SSH mínimo que solo acepta keyboard-interactive con un código fijo (como un 2FA).
pub struct OtpServer {
    pub port: u16,
    _runtime: tokio::runtime::Runtime,
    _dir: PathBuf,
}

struct OtpHandler {
    code: &'static str,
}

impl russh::server::Handler for OtpHandler {
    type Error = russh::Error;

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<russh::server::Response<'a>>,
    ) -> Result<russh::server::Auth, Self::Error> {
        use std::borrow::Cow;
        match response {
            None => Ok(russh::server::Auth::Partial {
                name: Cow::Borrowed("Verificación"),
                instructions: Cow::Borrowed("Introduce el código de tu aplicación"),
                prompts: Cow::Owned(vec![(Cow::Borrowed("Verification code: "), false)]),
            }),
            Some(mut answers) => match answers.next() {
                Some(answer) if answer.as_ref() == self.code.as_bytes() => Ok(russh::server::Auth::Accept),
                _ => Ok(russh::server::Auth::reject()),
            },
        }
    }
}

struct OtpFactory {
    code: &'static str,
}

impl russh::server::Server for OtpFactory {
    type Handler = OtpHandler;

    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> OtpHandler {
        OtpHandler { code: self.code }
    }
}

impl OtpServer {
    pub fn start(code: &'static str) -> Option<Self> {
        use russh::server::Server;
        if Command::new("ssh-keygen").arg("-?").stderr(Stdio::null()).output().is_err() {
            eprintln!("ssh-keygen no disponible; prueba omitida");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("omniterm-otp-{}-{code}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("host_key");
        let status = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key_path).status().unwrap();
        assert!(status.success());
        let key = russh::keys::load_secret_key(&key_path, None).unwrap();

        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            methods: russh::MethodSet::from(&[russh::MethodKind::KeyboardInteractive][..]),
            auth_rejection_time: Duration::from_millis(20),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            ..Default::default()
        });
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let port = std_listener.local_addr().unwrap().port();

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            let mut factory = OtpFactory { code };
            let _ = factory.run_on_socket(config, &listener).await;
        });
        Some(Self { port, _runtime: runtime, _dir: dir })
    }
}
