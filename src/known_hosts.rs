//! Lectura y edición del archivo `known_hosts` de OpenSSH.

use std::path::Path;

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
    std::fs::write(path, text)
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
}
