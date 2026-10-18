//! Ayudas para pruebas de extremo a extremo: un sshd privado en un puerto libre.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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
            .args(["-D", "-e", "-f", "/dev/null", "-p", &port.to_string()])
            .arg("-h")
            .arg(dir.join("host_key"))
            .args(["-o", "ListenAddress=127.0.0.1", "-o", "PidFile=none", "-o", "UsePAM=no"])
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

impl Drop for PrivateSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
