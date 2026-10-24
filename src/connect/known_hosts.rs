//! Lectura y edición del archivo `known_hosts` de OpenSSH.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownHost {
    /// Posición de la línea en el archivo, desde 0.
    pub index: usize,
    pub raw: String,
    /// Nombres tal como están guardados (`host`, `[host]:puerto` o una IP); vacío si van con hash.
    pub hosts: Vec<String>,
    pub hashed: bool,
    pub key_type: String,
    pub fingerprint: String,
}

impl KnownHost {
    /// Nombre para mostrar: el primero legible, o «Host protegido (hash)».
    pub fn display(&self) -> String {
        match self.hosts.first() {
            Some(first) => first.clone(),
            None => "Host protegido (hash)".to_string(),
        }
    }

    /// Pares (host, puerto) que cubre la entrada.
    pub fn endpoints(&self) -> Vec<(String, u16)> {
        self.hosts.iter().map(|h| split_endpoint(h)).collect()
    }
}

/// `[host]:puerto` o `host` (puerto 22).
fn split_endpoint(token: &str) -> (String, u16) {
    if let Some(rest) = token.strip_prefix('[') {
        if let Some((host, port)) = rest.split_once("]:") {
            if let Ok(port) = port.parse() {
                return (host.to_string(), port);
            }
        }
    }
    (token.to_string(), 22)
}

fn fingerprint(key_base64: &str) -> Option<String> {
    let blob = STANDARD.decode(key_base64).ok()?;
    Some(format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob))))
}

pub fn parse(text: &str) -> Vec<KnownHost> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let mut parts = trimmed.split_whitespace();
            let mut first = parts.next()?;
            // Marcadores `@cert-authority` y `@revoked` van antes de los nombres.
            if first.starts_with('@') {
                first = parts.next()?;
            }
            let key_type = parts.next()?.to_string();
            let fingerprint = fingerprint(parts.next()?)?;
            let hashed = first.starts_with('|');
            let hosts = if hashed { Vec::new() } else { first.split(',').map(str::to_string).collect() };
            Some(KnownHost { index, raw: line.to_string(), hosts, hashed, key_type, fingerprint })
        })
        .collect()
}

pub fn load(path: &Path) -> std::io::Result<Vec<KnownHost>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_lines(path: &Path, lines: &[String]) -> std::io::Result<()> {
    let mut text = lines.join("\n");
    if !lines.is_empty() {
        text.push('\n');
    }
    let path = resolve(path);
    let staged = stage(&path, text.as_bytes())?;
    commit(&staged, &path)
}

/// Sigue un enlace simbólico (por ejemplo, de un gestor de dotfiles) para no reemplazarlo por un archivo.
fn resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp{}", std::process::id()));
    path.with_file_name(name)
}

/// Escribe `bytes` en un archivo temporal junto a `path`: modo 0600 si es nuevo, los permisos de siempre si ya existía.
fn stage(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        if let Ok(existing) = std::fs::metadata(path) {
            file.set_permissions(existing.permissions())?;
        }
        file.sync_all()
    })();
    match result {
        Ok(()) => Ok(temp),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn commit(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(temp);
    })
}

/// Añade la huella de `host:port` sin dejar el archivo a medias: se escribe una copia y se reemplaza de una vez.
pub fn learn(path: &Path, host: &str, port: u16, key: &russh::keys::PublicKey) -> std::io::Result<()> {
    let path = resolve(path);
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let staged = stage(&path, &existing)?;
    if let Err(error) = russh::keys::known_hosts::learn_known_hosts_path(host, port, key, &staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(std::io::Error::other(error.to_string()));
    }
    commit(&staged, &path)
}

/// Borra la línea de `entry`. Si el archivo cambió desde que se leyó, la busca por su contenido.
/// Devuelve la posición de la que se quitó.
pub fn remove(path: &Path, entry: &KnownHost) -> std::io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let position = if lines.get(entry.index) == Some(&entry.raw) {
        entry.index
    } else {
        lines
            .iter()
            .position(|l| *l == entry.raw)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "la huella ya no está en known_hosts"))?
    };
    lines.remove(position);
    write_lines(path, &lines)?;
    Ok(position)
}

/// Vuelve a poner una línea borrada (para «Deshacer»).
pub fn insert(path: &Path, index: usize, raw: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines.insert(index.min(lines.len()), raw.to_string());
    write_lines(path, &lines)
}

/// Entradas legibles que cubren `host:port`.
pub fn entries_for(entries: &[KnownHost], host: &str, port: u16) -> Vec<KnownHost> {
    entries
        .iter()
        .filter(|e| e.endpoints().iter().any(|(h, p)| h.eq_ignore_ascii_case(host) && *p == port))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdZxbzb4tAbnK6TrIehUXW7Z5jpMDnBlbRiDyF8Q4fS";

    fn sample() -> String {
        format!("# comentario\n\nexample.com,10.0.0.5 ssh-ed25519 {ED}\n[srv.test]:2222 ssh-ed25519 {ED} nota\n|1|c2FsdA==|aGFzaA== ssh-ed25519 {ED}\n@revoked old.test ssh-ed25519 {ED}\n")
    }

    #[test]
    fn parses_names_ports_hashes_and_fingerprints() {
        let entries = parse(&sample());
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].hosts, vec!["example.com", "10.0.0.5"]);
        assert_eq!(entries[0].index, 2);
        assert_eq!(entries[0].key_type, "ssh-ed25519");
        assert!(entries[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(entries[1].endpoints(), vec![("srv.test".to_string(), 2222)]);
        assert!(entries[2].hashed && entries[2].hosts.is_empty());
        assert_eq!(entries[2].display(), "Host protegido (hash)");
        assert_eq!(entries[3].hosts, vec!["old.test"]);
    }

    #[test]
    fn finds_entries_by_host_and_port() {
        let entries = parse(&sample());
        assert_eq!(entries_for(&entries, "EXAMPLE.com", 22).len(), 1);
        assert_eq!(entries_for(&entries, "srv.test", 2222).len(), 1);
        assert!(entries_for(&entries, "srv.test", 22).is_empty());
    }

    #[test]
    fn removing_and_restoring_a_line_round_trips() {
        let dir = std::env::temp_dir().join(format!("omniterm-kh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        std::fs::write(&path, sample()).unwrap();
        let entries = load(&path).unwrap();
        let target = entries[1].clone();

        let at = remove(&path, &target).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("srv.test"));
        insert(&path, at, &target.raw).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), sample());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        assert!(load(Path::new("/no/existe/known_hosts")).unwrap().is_empty());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omniterm-kh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key() -> russh::keys::PublicKey {
        russh::keys::PublicKey::from_openssh(&format!("ssh-ed25519 {ED}")).unwrap()
    }

    #[test]
    fn learning_appends_and_leaves_no_temporary_file() {
        let dir = scratch("learn");
        let path = dir.join("known_hosts");
        std::fs::write(&path, sample()).unwrap();
        learn(&path, "new.test", 2200, &key()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(&sample()) && text.contains("[new.test]:2200"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_is_private_and_an_existing_one_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let fresh = dir.join("fresh");
        learn(&fresh, "a.test", 22, &key()).unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o600);

        let shared = dir.join("shared");
        std::fs::write(&shared, sample()).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
        learn(&shared, "b.test", 22, &key()).unwrap();
        assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o644);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn writing_through_a_symlink_keeps_the_link() {
        let dir = scratch("link");
        let real = dir.join("real");
        std::fs::write(&real, sample()).unwrap();
        let link = dir.join("known_hosts");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        learn(&link, "c.test", 22, &key()).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("c.test"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn learning_fails_when_the_folder_is_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("ro");
        let path = dir.join("known_hosts");
        std::fs::write(&path, sample()).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let writable_anyway = std::fs::write(dir.join("probe"), "").is_ok();
        let result = learn(&path, "d.test", 22, &key());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !writable_anyway {
            assert!(result.is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), sample());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
