//! Instalación de una versión nueva: qué paquete corresponde, verificación de la firma, descarga y sustitución.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{agent, get, Release};
use crate::i18n::t;

/// Clave pública de las releases; la privada firma `SHA256SUMS` en CI.
const PUBLIC_KEY: &str = include_str!("../../packaging/minisign.pub");
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.minisig";

/// Cómo está instalada la app que se está ejecutando.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    AppImage(PathBuf),
    /// El paquete `.app` de macOS.
    MacApp(PathBuf),
    /// Instalada con el instalador de Windows: la actualiza el instalador nuevo.
    WindowsSetup,
    /// Ejecutable suelto, de un `.tar.gz` o un `.zip`.
    Binary(PathBuf),
    Manual(Manual),
}

/// Por qué esta copia no se puede actualizar desde la app: solo se avisa con el enlace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manual {
    /// La instaló un gestor de paquetes (`.deb`).
    Package,
    /// La carpeta de la app no admite escritura.
    ReadOnly,
    /// No hay paquete de esta versión para este sistema.
    Unsupported,
    /// La release no trae firma.
    Unsigned,
}

impl Manual {
    pub fn reason(self) -> &'static str {
        match self {
            Manual::Package => t("update-manual-package"),
            Manual::ReadOnly => t("update-manual-readonly"),
            Manual::Unsupported => t("update-manual-unsupported"),
            Manual::Unsigned => t("update-manual-unsigned"),
        }
    }
}

/// Decide el formato por la ruta del ejecutable. `writable` dice si se puede escribir en una carpeta.
pub fn classify(os: &str, exe: &Path, appimage: Option<&Path>, writable: impl Fn(&Path) -> bool) -> Install {
    let parent_writable = |path: &Path| path.parent().is_some_and(&writable);
    if let Some(image) = appimage {
        return if parent_writable(image) { Install::AppImage(image.to_path_buf()) } else { Install::Manual(Manual::ReadOnly) };
    }
    match os {
        "macos" => match exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
            Some(app) if parent_writable(app) => Install::MacApp(app.to_path_buf()),
            Some(_) => Install::Manual(Manual::ReadOnly),
            None if parent_writable(exe) => Install::Binary(exe.to_path_buf()),
            None => Install::Manual(Manual::ReadOnly),
        },
        "windows" if exe.parent().is_some_and(|dir| dir.join("unins000.exe").exists()) => Install::WindowsSetup,
        "linux" if exe.starts_with("/usr") => Install::Manual(Manual::Package),
        "linux" | "windows" if parent_writable(exe) => Install::Binary(exe.to_path_buf()),
        "linux" | "windows" => Install::Manual(Manual::ReadOnly),
        _ => Install::Manual(Manual::Unsupported),
    }
}

/// Ruta del ejecutable al arrancar. Después de sustituirlo, `current_exe` apunta ya a la copia `.old`.
fn startup_exe() -> Option<&'static Path> {
    static EXE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    EXE.get_or_init(|| std::env::current_exe().and_then(|e| e.canonicalize()).ok()).as_deref()
}

/// Se llama al arrancar, antes de que una actualización pueda renombrar el ejecutable.
pub fn remember_startup_exe() {
    startup_exe();
}

/// La instalación de esta copia de la app.
pub fn detect() -> Install {
    let Some(exe) = startup_exe() else { return Install::Manual(Manual::Unsupported) };
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    classify(std::env::consts::OS, exe, appimage.as_deref(), can_write)
}

fn can_write(dir: &Path) -> bool {
    let probe = dir.join(".omni-term-write-test");
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

/// Nombre del paquete de `version` que sirve para esta instalación, con los nombres que publica CI.
pub fn asset_name(install: &Install, version: &str, os: &str, arch: &str) -> Option<String> {
    let base = format!("omni-term-v{version}");
    match (install, os, arch) {
        (Install::AppImage(_), "linux", "x86_64") => Some(format!("{base}-linux-x86_64.AppImage")),
        (Install::Binary(_), "linux", "x86_64") => Some(format!("{base}-linux-x86_64.tar.gz")),
        (Install::Binary(_), "windows", "x86_64") => Some(format!("{base}-windows-x86_64.zip")),
        (Install::WindowsSetup, "windows", "x86_64") => Some(format!("{base}-windows-x86_64-setup.exe")),
        (Install::MacApp(_), "macos", "aarch64") => Some(format!("{base}-macos-arm64.app.tar.gz")),
        (Install::MacApp(_), "macos", "x86_64") => Some(format!("{base}-macos-x86_64.app.tar.gz")),
        _ => None,
    }
}

/// Lo que hay que descargar para actualizar, o por qué solo se puede avisar.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub install: Install,
    pub asset: super::Asset,
    sums: String,
    signature: String,
}

pub fn plan(release: &Release, install: Install, os: &str, arch: &str) -> Result<Plan, Manual> {
    if let Install::Manual(reason) = install {
        return Err(reason);
    }
    let name = asset_name(&install, &release.version, os, arch).ok_or(Manual::Unsupported)?;
    let asset = release.asset(&name).ok_or(Manual::Unsupported)?.clone();
    let (Some(sums), Some(signature)) = (release.asset(SUMS), release.asset(SIGNATURE)) else { return Err(Manual::Unsigned) };
    Ok(Plan { install, asset, sums: sums.url.clone(), signature: signature.url.clone() })
}

/// Avance de la descarga, compartido con la interfaz. `cancel` la detiene.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    /// La descarga terminó y se está verificando o sustituyendo.
    pub installing: AtomicBool,
}

/// Qué queda por hacer después de preparar la versión nueva.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// Ya está en su sitio: se usa al reiniciar.
    Replaced,
    /// Instalador de Windows descargado, que se ejecuta al reiniciar o al cerrar.
    Installer(PathBuf),
}

/// Descarga, verifica y deja instalada la versión de `plan`. Bloquea: se llama desde un hilo aparte.
pub fn prepare(plan: &Plan, version: &str, progress: &Progress, wake: &dyn Fn()) -> Result<Applied, String> {
    let small = agent(Duration::from_secs(30));
    let sums = read_all(&small, &plan.sums)?;
    let signature = String::from_utf8(read_all(&small, &plan.signature)?).map_err(|_| t("update-err-signature").to_string())?;
    verify_signature(&sums, &signature)?;
    let expected = expected_hash(&String::from_utf8_lossy(&sums), &plan.asset.name).ok_or_else(|| t("update-err-signature").to_string())?;

    let staging = staging_dir(version);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))?;
    let file = staging.join(&plan.asset.name);
    let result = download(&plan.asset.url, plan.asset.size, &file, progress, wake).and_then(|()| {
        progress.installing.store(true, Ordering::Relaxed);
        wake();
        if sha256_file(&file).map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))? != expected {
            return Err(t("update-err-hash").to_string());
        }
        apply(&plan.install, &file, &staging)
    });
    // El instalador de Windows se queda en la carpeta para ejecutarlo luego; lo demás ya no hace falta.
    if !matches!(result, Ok(Applied::Installer(_))) {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn read_all(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let mut response = get(agent, url, "application/octet-stream")?;
    response.body_mut().with_config().limit(1024 * 1024).read_to_vec().map_err(|e| crate::tf!("update-err-network", "error" => e.to_string()))
}

fn download(url: &str, size: u64, dest: &Path, progress: &Progress, wake: &dyn Fn()) -> Result<(), String> {
    let mut response = get(&agent(Duration::from_secs(30 * 60)), url, "application/octet-stream")?;
    let total = response.body().content_length().unwrap_or(size);
    progress.total.store(total, Ordering::Relaxed);
    let mut reader = response.body_mut().with_config().limit(1024 * 1024 * 1024).reader();
    let mut out = std::fs::File::create(dest).map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_wake = std::time::Instant::now();
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(t("update-cancelled").to_string());
        }
        let read = reader.read(&mut buffer).map_err(|e| crate::tf!("update-err-network", "error" => e.to_string()))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read]).map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))?;
        done += read as u64;
        progress.done.store(done, Ordering::Relaxed);
        // La barra se repinta unas diez veces por segundo, no en cada bloque.
        if last_wake.elapsed() >= Duration::from_millis(100) {
            last_wake = std::time::Instant::now();
            wake();
        }
    }
    out.flush().map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))
}

fn verify_signature(sums: &[u8], signature: &str) -> Result<(), String> {
    verify_with(PUBLIC_KEY, sums, signature)
}

fn verify_with(public_key: &str, sums: &[u8], signature: &str) -> Result<(), String> {
    let invalid = || t("update-err-signature").to_string();
    let key = public_key.lines().find(|l| !l.starts_with("untrusted comment:") && !l.trim().is_empty()).ok_or_else(invalid)?;
    let key = minisign_verify::PublicKey::from_base64(key.trim()).map_err(|_| invalid())?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|_| invalid())?;
    key.verify(sums, &signature, false).map_err(|_| invalid())
}

/// Suma de `name` en un `SHA256SUMS` (`<hex>  <nombre>`, con `*` delante del nombre en modo binario).
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn staging_dir(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("omni-term-update-{version}"))
}

/// `ruta` + sufijo, en la misma carpeta: `omni-term` → `omni-term.old`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn apply(install: &Install, file: &Path, staging: &Path) -> Result<Applied, String> {
    let disk = |e: std::io::Error| crate::tf!("update-err-disk", "error" => e.to_string());
    match install {
        Install::AppImage(target) => {
            replace_file(target, file).map_err(disk)?;
            Ok(Applied::Replaced)
        }
        Install::Binary(target) => {
            let extracted = extract(file, staging)?;
            let name = target.file_name().unwrap_or_default();
            let new = find(&extracted, &|p| p.is_file() && p.file_name() == Some(name)).ok_or_else(|| t("update-err-archive").to_string())?;
            replace_file(target, &new).map_err(disk)?;
            Ok(Applied::Replaced)
        }
        Install::MacApp(target) => {
            let extracted = extract(file, staging)?;
            let new = find(&extracted, &|p| p.is_dir() && p.extension().is_some_and(|e| e == "app")).ok_or_else(|| t("update-err-archive").to_string())?;
            replace_dir(target, &new).map_err(disk)?;
            Ok(Applied::Replaced)
        }
        Install::WindowsSetup => Ok(Applied::Installer(file.to_path_buf())),
        Install::Manual(reason) => Err(reason.reason().to_string()),
    }
}

/// Descomprime con el `tar` del sistema, que en Windows 10+ también abre los `.zip`.
fn extract(archive: &Path, staging: &Path) -> Result<PathBuf, String> {
    let out = staging.join("extract");
    std::fs::create_dir_all(&out).map_err(|e| crate::tf!("update-err-disk", "error" => e.to_string()))?;
    let status = std::process::Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(&out).status();
    match status {
        Ok(s) if s.success() => Ok(out),
        _ => Err(t("update-err-archive").to_string()),
    }
}

fn find(dir: &Path, wanted: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if wanted(&path) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find(&path, wanted) {
                return Some(found);
            }
        }
    }
    None
}

/// Copia `new` junto a `target` y lo intercambia; el anterior queda como `.old` hasta el próximo arranque.
/// Un ejecutable en uso se puede renombrar en los tres sistemas, aunque no sobrescribir en Windows.
fn replace_file(target: &Path, new: &Path) -> std::io::Result<()> {
    let staged = sibling(target, ".new");
    let old = sibling(target, ".old");
    std::fs::copy(new, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old)?;
    if let Err(error) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target);
        return Err(error);
    }
    Ok(())
}

/// Como `replace_file`, para el paquete `.app`. `ditto` conserva la firma y los atributos.
fn replace_dir(target: &Path, new: &Path) -> std::io::Result<()> {
    let staged = sibling(target, ".new");
    let old = sibling(target, ".old");
    let _ = std::fs::remove_dir_all(&staged);
    let copied = std::process::Command::new("ditto").arg(new).arg(&staged).status()?;
    if !copied.success() {
        return Err(std::io::Error::other("ditto"));
    }
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(target, &old)?;
    if let Err(error) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target);
        return Err(error);
    }
    Ok(())
}

/// Al arrancar bien la versión nueva se borran la copia anterior y las descargas que quedaran.
pub fn cleanup(install: &Install) {
    match install {
        Install::AppImage(target) | Install::Binary(target) => {
            let _ = std::fs::remove_file(sibling(target, ".old"));
            let _ = std::fs::remove_file(sibling(target, ".new"));
        }
        Install::MacApp(target) => {
            let _ = std::fs::remove_dir_all(sibling(target, ".old"));
            let _ = std::fs::remove_dir_all(sibling(target, ".new"));
        }
        Install::WindowsSetup | Install::Manual(_) => {}
    }
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("omni-term-update-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Qué ejecutar para volver a abrir la app: en un AppImage, el propio archivo y no su copia montada.
pub fn relaunch_target() -> std::io::Result<PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(image) => Ok(PathBuf::from(image)),
        None => startup_exe().map(Path::to_path_buf).ok_or_else(|| std::io::Error::other("current_exe")),
    }
}

/// Lanza el instalador de Windows sin preguntas; con `relaunch` vuelve a abrir la app al terminar.
pub fn run_installer(setup: &Path, relaunch: bool) -> std::io::Result<()> {
    let mut command = std::process::Command::new(setup);
    command.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    if relaunch {
        command.arg("/relaunch=1");
    }
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(names: &[&str]) -> Release {
        Release {
            version: "0.2.0".into(),
            notes: String::new(),
            page: String::new(),
            assets: names.iter().map(|n| super::super::Asset { name: n.to_string(), url: format!("https://x/{n}"), size: 1 }).collect(),
        }
    }

    #[test]
    fn the_install_kind_comes_from_the_executable_path() {
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        let image = Path::new("/home/a/Apps/omni-term.AppImage");
        assert_eq!(classify("linux", Path::new("/tmp/.mount_x/usr/bin/omni-term"), Some(image), yes), Install::AppImage(image.into()));
        assert_eq!(classify("linux", Path::new("/usr/bin/omni-term"), None, yes), Install::Manual(Manual::Package));
        assert_eq!(classify("linux", Path::new("/home/a/.local/bin/omni-term"), None, yes), Install::Binary("/home/a/.local/bin/omni-term".into()));
        assert_eq!(classify("linux", Path::new("/opt/omni-term/omni-term"), None, no), Install::Manual(Manual::ReadOnly));
        let app = Path::new("/Applications/omni-term.app");
        assert_eq!(classify("macos", &app.join("Contents/MacOS/omni-term"), None, yes), Install::MacApp(app.into()));
        assert_eq!(classify("macos", &app.join("Contents/MacOS/omni-term"), None, no), Install::Manual(Manual::ReadOnly));
        assert_eq!(classify("freebsd", Path::new("/usr/local/bin/omni-term"), None, yes), Install::Manual(Manual::Unsupported));
    }

    #[test]
    fn each_install_picks_its_package() {
        let bin = Install::Binary("x".into());
        assert_eq!(asset_name(&bin, "0.2.0", "linux", "x86_64").as_deref(), Some("omni-term-v0.2.0-linux-x86_64.tar.gz"));
        assert_eq!(asset_name(&bin, "0.2.0", "windows", "x86_64").as_deref(), Some("omni-term-v0.2.0-windows-x86_64.zip"));
        assert_eq!(asset_name(&Install::WindowsSetup, "0.2.0", "windows", "x86_64").as_deref(), Some("omni-term-v0.2.0-windows-x86_64-setup.exe"));
        assert_eq!(asset_name(&Install::MacApp("x".into()), "0.2.0", "macos", "aarch64").as_deref(), Some("omni-term-v0.2.0-macos-arm64.app.tar.gz"));
        assert_eq!(asset_name(&bin, "0.2.0", "linux", "aarch64"), None);
    }

    #[test]
    fn without_a_signature_or_a_package_it_only_notifies() {
        let bin = Install::Binary("x".into());
        let full = release(&["omni-term-v0.2.0-linux-x86_64.tar.gz", SUMS, SIGNATURE]);
        assert!(plan(&full, bin.clone(), "linux", "x86_64").is_ok());
        assert_eq!(plan(&release(&["omni-term-v0.2.0-linux-x86_64.tar.gz", SUMS]), bin.clone(), "linux", "x86_64"), Err(Manual::Unsigned));
        assert_eq!(plan(&release(&[SUMS, SIGNATURE]), bin, "linux", "x86_64"), Err(Manual::Unsupported));
        assert_eq!(plan(&full, Install::Manual(Manual::Package), "linux", "x86_64"), Err(Manual::Package));
    }

    #[test]
    fn sums_are_read_by_file_name() {
        let sums = "AbC123  omni-term-v1-linux-x86_64.tar.gz\ndef456 *omni-term-v1-windows-x86_64.zip\n";
        assert_eq!(expected_hash(sums, "omni-term-v1-linux-x86_64.tar.gz").as_deref(), Some("abc123"));
        assert_eq!(expected_hash(sums, "omni-term-v1-windows-x86_64.zip").as_deref(), Some("def456"));
        assert_eq!(expected_hash(sums, "otro"), None);
    }

    /// Firmado con una clave de prueba hecha para este test, no con la del proyecto.
    const TEST_KEY: &str = "untrusted comment: minisign public key 2F789DCAC2BD7743\nRWRDd73Cyp14L59Tzwd7peUAzwdVPeRGBQAlRLlpenCBn+HxOWBmSi+4\n";
    const TEST_SUMS: &[u8] = b"hash  omni-term-v1-linux-x86_64.tar.gz\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRURDd73Cyp14L+1LW7seWiLsm0/WYsx++y9PXSm/amzzn6XUZp/LqTaaQvkzVfm44AraKnxzxAE+i7fPzDwts1GseU4itLlHiQ4=\ntrusted comment: omni-term test\nsZYprTKcDmN3mrWzcI+FJh/UkixIlZI7Oiigau71QDgPMDBuV9an1qaVxo9njeFngVd11zB0H1gMVvucBNHZBw==\n";

    #[test]
    fn only_a_signature_from_the_right_key_over_the_same_sums_passes() {
        assert!(verify_with(TEST_KEY, TEST_SUMS, TEST_SIGNATURE).is_ok());
        assert!(verify_with(TEST_KEY, b"hash  otro-archivo\n", TEST_SIGNATURE).is_err());
        assert!(verify_signature(TEST_SUMS, TEST_SIGNATURE).is_err());
        assert!(verify_signature(TEST_SUMS, "no es una firma").is_err());
    }

    #[test]
    fn replacing_keeps_the_previous_file_until_cleanup() {
        let dir = std::env::temp_dir().join(format!("omni-term-replace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("omni-term");
        let new = dir.join("downloaded");
        std::fs::write(&target, "vieja").unwrap();
        std::fs::write(&new, "nueva").unwrap();
        replace_file(&target, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nueva");
        assert_eq!(std::fs::read_to_string(sibling(&target, ".old")).unwrap(), "vieja");
        cleanup(&Install::Binary(target.clone()));
        assert!(!sibling(&target, ".old").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
