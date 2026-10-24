//! Android Debug Bridge: cliente propio del protocolo del servidor `adb` (TCP 5037).
//!
//! El servidor `adb` ya gestiona USB, la autorización del móvil y los dispositivos inalámbricos;
//! aquí solo se le piden dispositivos, un `shell` (terminal) y el protocolo `sync` (archivos).

use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::files::remotefs::{PipeEnd, Reader, RemoteFs, Writer};
use crate::files::sftp::Entry;
use crate::data::store::Connection;
use crate::terminal::term::{Backend, Feed};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Tamaño máximo de un bloque `DATA` del protocolo `sync`.
const SYNC_CHUNK: usize = 64 * 1024;
const DEFAULT_PORT: u16 = 5037;

/// Dispositivo que informa el servidor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    /// Modelo legible, si el servidor lo dio.
    pub description: String,
}

/// Interpreta la respuesta de `host:devices-l`.
pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let description = parts
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "))
                .unwrap_or_default();
            Some(Device { serial, state, description })
        })
        .collect()
}

/// Comillas simples para pasar una ruta al shell del móvil.
fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn red(text: &str) -> String {
    format!("\r\n\x1b[31m{text}\x1b[0m\r\n")
}

/// Un paquete del protocolo `shell` v2: id, longitud (4 bytes little-endian) y datos.
fn shell_frame(id: u8, data: &[u8]) -> Vec<u8> {
    let mut frame = vec![id];
    frame.extend((data.len() as u32).to_le_bytes());
    frame.extend(data);
    frame
}

const ID_STDIN: u8 = 0;
const ID_STDOUT: u8 = 1;
const ID_STDERR: u8 = 2;
const ID_EXIT: u8 = 3;
const ID_WINDOW: u8 = 5;

/// Saca de `buffer` los paquetes completos y deja los datos que faltan.
fn parse_frames(buffer: &mut Vec<u8>) -> Vec<(u8, Vec<u8>)> {
    let mut frames = Vec::new();
    loop {
        if buffer.len() < 5 {
            break;
        }
        let length = u32::from_le_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
        if buffer.len() < 5 + length {
            break;
        }
        let id = buffer[0];
        let data = buffer[5..5 + length].to_vec();
        buffer.drain(..5 + length);
        frames.push((id, data));
    }
    frames
}

async fn read_u32(stream: &mut TcpStream) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    stream.read_exact(&mut bytes).await?;
    Ok(u32::from_le_bytes(bytes))
}

/// Mensaje con longitud en cuatro dígitos hexadecimales.
async fn read_hex_string(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).await?;
    let length = usize::from_str_radix(std::str::from_utf8(&length)?, 16).context("longitud inválida del servidor adb")?;
    let mut data = vec![0u8; length];
    stream.read_exact(&mut data).await?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// Envía una petición del servidor y espera `OKAY`.
async fn request(stream: &mut TcpStream, payload: &str) -> anyhow::Result<()> {
    stream.write_all(format!("{:04x}{payload}", payload.len()).as_bytes()).await?;
    let mut status = [0u8; 4];
    stream.read_exact(&mut status).await?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => bail!("{}", read_hex_string(stream).await?),
        other => bail!("respuesta inesperada del servidor adb: {:?}", String::from_utf8_lossy(other)),
    }
}

fn sync_request(id: &[u8; 4], argument: &[u8]) -> Vec<u8> {
    let mut frame = id.to_vec();
    frame.extend((argument.len() as u32).to_le_bytes());
    frame.extend(argument);
    frame
}

/// Servidor `adb` al que se habla; en producción, el local.
#[derive(Clone)]
pub struct Adb {
    address: String,
}

impl Adb {
    pub fn local() -> Self {
        let port = std::env::var("ANDROID_ADB_SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
        Self { address: format!("127.0.0.1:{port}") }
    }

    #[cfg(test)]
    fn at(address: String) -> Self {
        Self { address }
    }

    async fn dial(&self) -> io::Result<TcpStream> {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&self.address)).await {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "tiempo de espera agotado")),
        }
    }

    /// Conecta; si el servidor no corre, lo arranca con `adb start-server` y reintenta.
    async fn connect(&self) -> anyhow::Result<TcpStream> {
        if let Ok(stream) = self.dial().await {
            return Ok(stream);
        }
        let started = tokio::task::spawn_blocking(|| std::process::Command::new("adb").arg("start-server").output()).await?;
        match started {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                bail!("No se encontró «adb». Instala Android platform-tools y añádelo al PATH.")
            }
            Err(error) => bail!("No se pudo ejecutar adb: {error}"),
            Ok(_) => {}
        }
        self.dial().await.with_context(|| format!("no se pudo conectar con el servidor adb en {}", self.address))
    }

    pub async fn devices(&self) -> anyhow::Result<Vec<Device>> {
        let mut stream = self.connect().await?;
        request(&mut stream, "host:devices-l").await?;
        Ok(parse_devices(&read_hex_string(&mut stream).await?))
    }

    /// Comprueba que el dispositivo está listo y devuelve su número de serie. Una dirección
    /// `ip:puerto` que aún no aparece se conecta antes (`adb connect`).
    pub async fn resolve(&self, wanted: &str) -> anyhow::Result<String> {
        let wanted = wanted.trim();
        let mut devices = self.devices().await?;
        if !wanted.is_empty() && wanted.contains(':') && !devices.iter().any(|d| d.serial == wanted) {
            let mut stream = self.connect().await?;
            request(&mut stream, &format!("host:connect:{wanted}")).await?;
            let answer = read_hex_string(&mut stream).await?;
            if answer.contains("failed") || answer.contains("cannot") || answer.contains("unable") {
                bail!("No se pudo conectar con {wanted}: {answer}");
            }
            devices = self.devices().await?;
        }
        let device = if wanted.is_empty() {
            match devices.as_slice() {
                [] => bail!("No hay ningún dispositivo conectado. Enchufa el móvil con la depuración USB activada."),
                [only] => only,
                _ => bail!("Hay varios dispositivos conectados: elige uno en la conexión."),
            }
        } else {
            devices
                .iter()
                .find(|d| d.serial == wanted)
                .with_context(|| format!("No se encuentra el dispositivo {wanted}. ¿Está conectado y con la depuración USB activada?"))?
        };
        match device.state.as_str() {
            "device" => Ok(device.serial.clone()),
            "unauthorized" => bail!("El móvil no ha autorizado este equipo: acepta «Permitir depuración USB» en su pantalla y reconecta."),
            "offline" => bail!("El dispositivo está sin conexión: desenchúfalo y vuelve a conectarlo."),
            other => bail!("El dispositivo {} no está listo (estado: {other}).", device.serial),
        }
    }

    async fn features(&self, serial: &str) -> Vec<String> {
        let result = async {
            let mut stream = self.connect().await?;
            request(&mut stream, &format!("host-serial:{serial}:features")).await?;
            anyhow::Ok(read_hex_string(&mut stream).await?)
        }
        .await;
        result.map(|text| text.trim().split(',').map(str::to_string).collect()).unwrap_or_default()
    }

    async fn transport(&self, serial: &str) -> anyhow::Result<TcpStream> {
        let mut stream = self.connect().await?;
        request(&mut stream, &format!("host:transport:{serial}")).await?;
        Ok(stream)
    }
}

/// Dispositivos para el botón «Detectar» del diálogo.
pub fn detect() -> Result<Vec<Device>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(Adb::local().devices()).map_err(|e| format!("{e:#}"))
}

enum Message {
    Input(Vec<u8>),
    Resize(usize, usize),
}

struct AdbBackend {
    tx: UnboundedSender<Message>,
}

impl Backend for AdbBackend {
    fn write(&self, bytes: &[u8]) {
        let _ = self.tx.send(Message::Input(bytes.to_vec()));
    }

    fn resize(&self, cols: usize, rows: usize) {
        let _ = self.tx.send(Message::Resize(cols, rows));
    }
}

/// Abre el shell del dispositivo en segundo plano; el progreso y los errores salen en la terminal.
pub fn spawn(handle: &tokio::runtime::Handle, connection: Connection, size: (usize, usize), feed: Feed) -> Arc<dyn Backend> {
    let (tx, rx) = unbounded_channel::<Message>();
    handle.spawn(run_shell(Adb::local(), connection.host.clone(), size, feed, rx));
    Arc::new(AdbBackend { tx })
}

async fn run_shell(
    adb: Adb,
    wanted: String,
    size: (usize, usize),
    mut feed: Feed,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Message>,
) {
    feed.feed(b"Conectando con el dispositivo\xe2\x80\xa6\r\n");
    let opened = async {
        let serial = adb.resolve(&wanted).await?;
        let v2 = adb.features(&serial).await.iter().any(|f| f == "shell_v2");
        let mut stream = adb.transport(&serial).await?;
        request(&mut stream, if v2 { "shell,v2,TERM=xterm-256color,pty:" } else { "shell:" }).await?;
        if v2 {
            stream.write_all(&shell_frame(ID_WINDOW, format!("{}x{},0x0", size.1, size.0).as_bytes())).await?;
        }
        anyhow::Ok((serial, v2, stream))
    }
    .await;
    let (serial, v2, stream) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            feed.feed(red(&format!("{error:#}")).as_bytes());
            feed.disconnect();
            return;
        }
    };
    feed.feed(format!("Conectado a {serial}.\r\n").as_bytes());

    let (mut reader, mut writer) = stream.into_split();
    let mut buffer = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) => {
                    feed.close();
                    return;
                }
                Ok(n) if v2 => {
                    pending.extend_from_slice(&buffer[..n]);
                    for (id, data) in parse_frames(&mut pending) {
                        match id {
                            ID_STDOUT | ID_STDERR => feed.feed(&data),
                            ID_EXIT => {
                                feed.close();
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(n) => feed.feed(&buffer[..n]),
                Err(error) => {
                    feed.feed(red(&format!("Conexión perdida: {error}")).as_bytes());
                    feed.disconnect();
                    return;
                }
            },
            message = rx.recv() => match message {
                Some(Message::Input(bytes)) => {
                    let wire = if v2 { shell_frame(ID_STDIN, &bytes) } else { bytes };
                    if writer.write_all(&wire).await.is_err() {
                        feed.disconnect();
                        return;
                    }
                }
                Some(Message::Resize(cols, rows)) => {
                    if v2 {
                        let _ = writer.write_all(&shell_frame(ID_WINDOW, format!("{rows}x{cols},0x0").as_bytes())).await;
                    }
                }
                None => return,
            },
        }
    }
}

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

/// Archivos del dispositivo por el protocolo `sync`; cada operación abre su propia conexión.
pub struct AdbFs {
    adb: Adb,
    serial: String,
    shell_v2: bool,
}

#[derive(Clone, Copy)]
struct Stat {
    mode: u32,
    size: u32,
    modified: u32,
}

impl Stat {
    fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
}

impl AdbFs {
    async fn sync(&self) -> anyhow::Result<TcpStream> {
        let mut stream = self.adb.transport(&self.serial).await?;
        request(&mut stream, "sync:").await?;
        Ok(stream)
    }

    async fn stat(&self, path: &str) -> anyhow::Result<Option<Stat>> {
        let mut stream = self.sync().await?;
        stream.write_all(&sync_request(b"STAT", path.as_bytes())).await?;
        let mut id = [0u8; 4];
        stream.read_exact(&mut id).await?;
        if &id != b"STAT" {
            bail!("respuesta inesperada a STAT");
        }
        let stat = Stat { mode: read_u32(&mut stream).await?, size: read_u32(&mut stream).await?, modified: read_u32(&mut stream).await? };
        Ok((stat.mode != 0 || stat.size != 0 || stat.modified != 0).then_some(stat))
    }

    /// Ejecuta un comando del shell y falla si termina con error o, sin `shell_v2`, si escribe algo.
    async fn run(&self, command: &str) -> anyhow::Result<()> {
        let mut stream = self.adb.transport(&self.serial).await?;
        if self.shell_v2 {
            request(&mut stream, &format!("shell,v2,raw:{command}")).await?;
            let mut pending = Vec::new();
            let mut output = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stream.read(&mut buffer).await?;
                if n == 0 {
                    return Ok(());
                }
                pending.extend_from_slice(&buffer[..n]);
                for (id, data) in parse_frames(&mut pending) {
                    match id {
                        ID_STDOUT | ID_STDERR => output.extend(data),
                        ID_EXIT if data.first().copied().unwrap_or(0) != 0 => {
                            bail!("{}", String::from_utf8_lossy(&output).trim().to_string())
                        }
                        ID_EXIT => return Ok(()),
                        _ => {}
                    }
                }
            }
        } else {
            request(&mut stream, &format!("shell:{command}")).await?;
            let mut output = Vec::new();
            stream.read_to_end(&mut output).await?;
            let text = String::from_utf8_lossy(&output);
            if text.trim().is_empty() { Ok(()) } else { bail!("{}", text.trim()) }
        }
    }
}

#[async_trait]
impl RemoteFs for AdbFs {
    async fn home(&self) -> String {
        match self.stat("/sdcard").await {
            Ok(Some(stat)) if stat.is_dir() => "/sdcard".into(),
            _ => "/".into(),
        }
    }

    async fn list(&self, path: &str) -> anyhow::Result<Vec<Entry>> {
        let mut stream = self.sync().await?;
        stream.write_all(&sync_request(b"LIST", path.as_bytes())).await?;
        let mut entries = Vec::new();
        loop {
            let mut id = [0u8; 4];
            stream.read_exact(&mut id).await?;
            match &id {
                b"DENT" => {
                    let (mode, size, modified, name_length) =
                        (read_u32(&mut stream).await?, read_u32(&mut stream).await?, read_u32(&mut stream).await?, read_u32(&mut stream).await?);
                    let mut name = vec![0u8; name_length as usize];
                    stream.read_exact(&mut name).await?;
                    let name = String::from_utf8_lossy(&name).into_owned();
                    if name == "." || name == ".." {
                        continue;
                    }
                    entries.push((name, Stat { mode, size, modified }));
                }
                b"DONE" => {
                    for _ in 0..4 {
                        read_u32(&mut stream).await?;
                    }
                    break;
                }
                b"FAIL" => {
                    let length = read_u32(&mut stream).await? as usize;
                    let mut message = vec![0u8; length];
                    stream.read_exact(&mut message).await?;
                    bail!("{}", String::from_utf8_lossy(&message));
                }
                _ => bail!("respuesta inesperada a LIST"),
            }
        }
        let mut result = Vec::with_capacity(entries.len());
        for (name, stat) in entries {
            // Un enlace simbólico a una carpeta se navega como carpeta.
            let is_dir = if stat.mode & S_IFMT == S_IFLNK {
                let target = crate::files::sftp::join(path, &name);
                self.stat(&target).await.ok().flatten().is_some_and(|s| s.is_dir())
            } else {
                stat.is_dir()
            };
            result.push(Entry { name, is_dir, size: u64::from(stat.size), modified: Some(u64::from(stat.modified)), permissions: Some(stat.mode & 0o7777) });
        }
        Ok(result)
    }

    async fn exists(&self, path: &str) -> bool {
        matches!(self.stat(path).await, Ok(Some(_)))
    }

    async fn size(&self, path: &str) -> u64 {
        self.stat(path).await.ok().flatten().map_or(0, |s| u64::from(s.size))
    }

    async fn make_dir(&self, path: &str) -> anyhow::Result<()> {
        self.run(&format!("mkdir -p {}", quote(path))).await
    }

    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.run(&format!("mv {} {}", quote(from), quote(to))).await
    }

    async fn remove(&self, path: &str) -> anyhow::Result<()> {
        self.run(&format!("rm -rf {}", quote(path))).await
    }

    async fn open_read(&self, path: &str) -> anyhow::Result<Reader> {
        let mut stream = self.sync().await?;
        stream.write_all(&sync_request(b"RECV", path.as_bytes())).await?;
        let (mut task_side, user_side) = tokio::io::duplex(4 * SYNC_CHUNK);
        let task = tokio::spawn(async move {
            loop {
                let mut id = [0u8; 4];
                stream.read_exact(&mut id).await?;
                match &id {
                    b"DATA" => {
                        let length = read_u32(&mut stream).await? as usize;
                        let mut data = vec![0u8; length];
                        stream.read_exact(&mut data).await?;
                        task_side.write_all(&data).await?;
                    }
                    b"DONE" => {
                        read_u32(&mut stream).await?;
                        return Ok(());
                    }
                    b"FAIL" => {
                        let length = read_u32(&mut stream).await? as usize;
                        let mut message = vec![0u8; length];
                        stream.read_exact(&mut message).await?;
                        return Err(io::Error::other(String::from_utf8_lossy(&message).into_owned()));
                    }
                    _ => return Err(io::Error::other("respuesta inesperada a RECV")),
                }
            }
        });
        Ok(Box::new(PipeEnd::new(user_side, task)))
    }

    async fn open_write(&self, path: &str) -> anyhow::Result<Writer> {
        let mut stream = self.sync().await?;
        stream.write_all(&sync_request(b"SEND", format!("{path},{}", 0o100644).as_bytes())).await?;
        let (mut task_side, user_side) = tokio::io::duplex(4 * SYNC_CHUNK);
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; SYNC_CHUNK];
            loop {
                let read = task_side.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                stream.write_all(&sync_request(b"DATA", &buffer[..read])).await?;
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
            let mut done = b"DONE".to_vec();
            done.extend(now.to_le_bytes());
            stream.write_all(&done).await?;
            let mut id = [0u8; 4];
            stream.read_exact(&mut id).await?;
            match &id {
                b"OKAY" => {
                    read_u32(&mut stream).await?;
                    Ok(())
                }
                b"FAIL" => {
                    let length = read_u32(&mut stream).await? as usize;
                    let mut message = vec![0u8; length];
                    stream.read_exact(&mut message).await?;
                    Err(io::Error::other(String::from_utf8_lossy(&message).into_owned()))
                }
                _ => Err(io::Error::other("respuesta inesperada a SEND")),
            }
        });
        Ok(Box::new(PipeEnd::new(user_side, task)))
    }
}

/// Conexión del explorador a un dispositivo.
pub async fn connect(connection: &Connection) -> anyhow::Result<Arc<dyn RemoteFs>> {
    let adb = Adb::local();
    let serial = adb.resolve(&connection.host).await?;
    let shell_v2 = adb.features(&serial).await.iter().any(|f| f == "shell_v2");
    Ok(Arc::new(AdbFs { adb, serial, shell_v2 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[test]
    fn devices_are_parsed_with_state_and_model() {
        let text = "R58M123  device usb:1-1 product:x model:Pixel_6 device:oriole transport_id:1\n192.168.1.5:5555 unauthorized transport_id:2\n";
        let devices = parse_devices(text);
        assert_eq!(devices.len(), 2);
        assert_eq!((devices[0].serial.as_str(), devices[0].state.as_str(), devices[0].description.as_str()), ("R58M123", "device", "Pixel 6"));
        assert_eq!(devices[1].state, "unauthorized");
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn frames_are_split_across_reads() {
        let mut stream = shell_frame(ID_STDOUT, b"hola");
        stream.extend(shell_frame(ID_EXIT, &[3]));
        let mut buffer = stream[..7].to_vec();
        assert!(parse_frames(&mut buffer).is_empty());
        buffer.extend_from_slice(&stream[7..]);
        let frames = parse_frames(&mut buffer);
        assert_eq!(frames, vec![(ID_STDOUT, b"hola".to_vec()), (ID_EXIT, vec![3])]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn paths_are_quoted_for_the_device_shell() {
        assert_eq!(quote("/sdcard/a b"), "'/sdcard/a b'");
        assert_eq!(quote("/sdcard/it's"), "'/sdcard/it'\\''s'");
    }

    /// Servidor `adb` mínimo: dispositivos, transporte, `sync` y comandos de archivo sobre un sistema de archivos en memoria.
    #[derive(Default)]
    struct Mock {
        files: BTreeMap<String, Vec<u8>>,
        dirs: BTreeSet<String>,
        state: String,
    }

    async fn read_exact_vec(stream: &mut TcpStream, n: usize) -> io::Result<Vec<u8>> {
        let mut data = vec![0u8; n];
        stream.read_exact(&mut data).await?;
        Ok(data)
    }

    async fn serve(listener: TcpListener, mock: Arc<Mutex<Mock>>) {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let mock = mock.clone();
            tokio::spawn(async move {
                let _ = serve_one(stream, mock).await;
            });
        }
    }

    fn okay_with(text: &str) -> Vec<u8> {
        format!("OKAY{:04x}{text}", text.len()).into_bytes()
    }

    async fn serve_one(mut stream: TcpStream, mock: Arc<Mutex<Mock>>) -> anyhow::Result<()> {
        loop {
            let length = usize::from_str_radix(std::str::from_utf8(&read_exact_vec(&mut stream, 4).await?)?, 16)?;
            let payload = String::from_utf8(read_exact_vec(&mut stream, length).await?)?;
            match payload.as_str() {
                "host:devices-l" => {
                    let state = mock.lock().unwrap().state.clone();
                    stream.write_all(&okay_with(&format!("SERIAL1 {state} model:Test_Phone\n"))).await?;
                    return Ok(());
                }
                p if p.starts_with("host-serial:") => {
                    stream.write_all(&okay_with("shell_v2,cmd")).await?;
                    return Ok(());
                }
                p if p.starts_with("host:transport:") => stream.write_all(b"OKAY").await?,
                "sync:" => {
                    stream.write_all(b"OKAY").await?;
                    return serve_sync(stream, mock).await;
                }
                p if p.starts_with("shell,v2,raw:") => {
                    stream.write_all(b"OKAY").await?;
                    let command = &p["shell,v2,raw:".len()..];
                    let code = run_command(&mock, command);
                    stream.write_all(&shell_frame(ID_EXIT, &[code])).await?;
                    return Ok(());
                }
                p if p.starts_with("shell,v2,TERM") => {
                    stream.write_all(b"OKAY").await?;
                    stream.write_all(&shell_frame(ID_STDOUT, b"$ ")).await?;
                    let mut pending = Vec::new();
                    let mut buffer = [0u8; 1024];
                    loop {
                        let n = stream.read(&mut buffer).await?;
                        if n == 0 {
                            return Ok(());
                        }
                        pending.extend_from_slice(&buffer[..n]);
                        for (id, data) in parse_frames(&mut pending) {
                            if id == ID_STDIN {
                                stream.write_all(&shell_frame(ID_STDOUT, &data)).await?;
                                if data == b"exit\n" {
                                    stream.write_all(&shell_frame(ID_EXIT, &[0])).await?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                other => {
                    stream.write_all(format!("FAIL{:04x}{}", 14 + other.len(), format!("unknown host service: {other}")).as_bytes()).await?;
                    return Ok(());
                }
            }
        }
    }

    fn unquote(text: &str) -> String {
        text.trim().trim_matches('\'').to_string()
    }

    fn run_command(mock: &Arc<Mutex<Mock>>, command: &str) -> u8 {
        let mut mock = mock.lock().unwrap();
        if let Some(path) = command.strip_prefix("mkdir -p ") {
            mock.dirs.insert(unquote(path));
        } else if let Some(rest) = command.strip_prefix("mv ") {
            let (from, to) = rest.split_once("' '").map(|(a, b)| (unquote(a), unquote(b))).unwrap_or_default();
            match mock.files.remove(&from) {
                Some(data) => {
                    mock.files.insert(to, data);
                }
                None => return 1,
            }
        } else if let Some(path) = command.strip_prefix("rm -rf ") {
            let path = unquote(path);
            mock.files.retain(|name, _| *name != path && !name.starts_with(&format!("{path}/")));
            mock.dirs.retain(|dir| *dir != path && !dir.starts_with(&format!("{path}/")));
        } else {
            return 127;
        }
        0
    }

    async fn serve_sync(mut stream: TcpStream, mock: Arc<Mutex<Mock>>) -> anyhow::Result<()> {
        loop {
            let id = read_exact_vec(&mut stream, 4).await?;
            let length = read_u32(&mut stream).await? as usize;
            let argument = String::from_utf8(read_exact_vec(&mut stream, length).await?)?;
            match id.as_slice() {
                b"STAT" => {
                    let (mode, size) = {
                        let mock = mock.lock().unwrap();
                        if mock.dirs.contains(&argument) {
                            (0o040755u32, 0u32)
                        } else if let Some(file) = mock.files.get(&argument) {
                            (0o100644, file.len() as u32)
                        } else {
                            (0, 0)
                        }
                    };
                    let mut reply = b"STAT".to_vec();
                    for value in [mode, size, if mode == 0 { 0 } else { 1_700_000_000 }] {
                        reply.extend(value.to_le_bytes());
                    }
                    stream.write_all(&reply).await?;
                }
                b"LIST" => {
                    let prefix = format!("{}/", argument.trim_end_matches('/'));
                    let mut reply = Vec::new();
                    let rows: Vec<(String, u32, u32)> = {
                        let mock = mock.lock().unwrap();
                        let dirs = mock.dirs.iter().filter_map(|d| d.strip_prefix(&prefix).filter(|n| !n.contains('/')).map(|n| (n.to_string(), 0o040755u32, 0u32)));
                        let files = mock.files.iter().filter_map(|(f, data)| f.strip_prefix(&prefix).filter(|n| !n.contains('/')).map(|n| (n.to_string(), 0o100644u32, data.len() as u32)));
                        dirs.chain(files).collect()
                    };
                    for (name, mode, size) in rows {
                        reply.extend(b"DENT");
                        for value in [mode, size, 1_700_000_000, name.len() as u32] {
                            reply.extend(value.to_le_bytes());
                        }
                        reply.extend(name.as_bytes());
                    }
                    reply.extend(b"DONE");
                    reply.extend([0u8; 16]);
                    stream.write_all(&reply).await?;
                }
                b"RECV" => {
                    let data = mock.lock().unwrap().files.get(&argument).cloned();
                    match data {
                        Some(data) => {
                            for chunk in data.chunks(SYNC_CHUNK) {
                                stream.write_all(&sync_request(b"DATA", chunk)).await?;
                            }
                            let mut done = b"DONE".to_vec();
                            done.extend(0u32.to_le_bytes());
                            stream.write_all(&done).await?;
                        }
                        None => stream.write_all(&sync_request(b"FAIL", b"No such file or directory")).await?,
                    }
                }
                b"SEND" => {
                    let path = argument.rsplit_once(',').map_or(argument.clone(), |(p, _)| p.to_string());
                    let mut data = Vec::new();
                    loop {
                        let kind = read_exact_vec(&mut stream, 4).await?;
                        let value = read_u32(&mut stream).await?;
                        if kind == b"DONE" {
                            break;
                        }
                        data.extend(read_exact_vec(&mut stream, value as usize).await?);
                    }
                    mock.lock().unwrap().files.insert(path, data);
                    let mut reply = b"OKAY".to_vec();
                    reply.extend(0u32.to_le_bytes());
                    stream.write_all(&reply).await?;
                }
                _ => return Ok(()),
            }
        }
    }

    async fn start(state: &str) -> (Adb, Arc<Mutex<Mock>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mock = Arc::new(Mutex::new(Mock { state: state.into(), ..Mock::default() }));
        tokio::spawn(serve(listener, mock.clone()));
        (Adb::at(address), mock)
    }

    async fn fs(adb: &Adb) -> AdbFs {
        let serial = adb.resolve("").await.unwrap();
        AdbFs { adb: adb.clone(), serial, shell_v2: true }
    }

    #[tokio::test]
    async fn the_only_device_is_picked_and_unready_states_explain_themselves() {
        let (adb, _) = start("device").await;
        assert_eq!(adb.resolve("").await.unwrap(), "SERIAL1");
        assert_eq!(adb.resolve("SERIAL1").await.unwrap(), "SERIAL1");
        let missing = adb.resolve("OTRO").await.unwrap_err().to_string();
        assert!(missing.contains("OTRO"), "{missing}");

        let (adb, _) = start("unauthorized").await;
        assert!(adb.resolve("").await.unwrap_err().to_string().contains("Permitir depuración USB"));
        let (adb, _) = start("offline").await;
        assert!(adb.resolve("").await.unwrap_err().to_string().contains("sin conexión"));
    }

    #[tokio::test]
    async fn files_are_listed_downloaded_and_uploaded() {
        let (adb, mock) = start("device").await;
        {
            let mut mock = mock.lock().unwrap();
            mock.dirs.insert("/sdcard".into());
            mock.dirs.insert("/sdcard/DCIM".into());
            mock.files.insert("/sdcard/nota.txt".into(), b"hola".to_vec());
        }
        let fs = fs(&adb).await;
        assert_eq!(fs.home().await, "/sdcard");
        let mut entries = fs.list("/sdcard").await.unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), vec![("DCIM", true), ("nota.txt", false)]);
        assert_eq!(fs.size("/sdcard/nota.txt").await, 4);
        assert!(fs.exists("/sdcard/nota.txt").await && !fs.exists("/sdcard/nada").await);

        let mut data = Vec::new();
        fs.open_read("/sdcard/nota.txt").await.unwrap().read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hola");
        assert!(fs.open_read("/sdcard/nada").await.unwrap().read_to_end(&mut Vec::new()).await.is_err());

        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut writer = fs.open_write("/sdcard/grande.bin").await.unwrap();
        writer.write_all(&big).await.unwrap();
        writer.shutdown().await.unwrap();
        assert_eq!(mock.lock().unwrap().files["/sdcard/grande.bin"], big);
    }

    #[tokio::test]
    async fn folders_are_created_renamed_and_removed_through_the_shell() {
        let (adb, mock) = start("device").await;
        mock.lock().unwrap().files.insert("/sdcard/a.txt".into(), b"x".to_vec());
        let fs = fs(&adb).await;
        fs.make_dir("/sdcard/nueva carpeta").await.unwrap();
        assert!(mock.lock().unwrap().dirs.contains("/sdcard/nueva carpeta"));
        fs.rename("/sdcard/a.txt", "/sdcard/b.txt").await.unwrap();
        assert!(mock.lock().unwrap().files.contains_key("/sdcard/b.txt"));
        assert!(fs.rename("/sdcard/falta", "/sdcard/otra").await.is_err());
        fs.remove("/sdcard/b.txt").await.unwrap();
        assert!(mock.lock().unwrap().files.is_empty());
    }

    #[tokio::test]
    async fn the_shell_echoes_input_until_exit() {
        let (adb, _) = start("device").await;
        let mut stream = adb.transport("SERIAL1").await.unwrap();
        request(&mut stream, "shell,v2,TERM=xterm-256color,pty:").await.unwrap();
        stream.write_all(&shell_frame(ID_STDIN, b"exit\n")).await.unwrap();
        let mut pending = Vec::new();
        let mut seen = Vec::new();
        let mut buffer = [0u8; 256];
        while !seen.iter().any(|(id, _)| *id == ID_EXIT) {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            pending.extend_from_slice(&buffer[..n]);
            seen.extend(parse_frames(&mut pending));
        }
        let text: Vec<u8> = seen.iter().filter(|(id, _)| *id == ID_STDOUT).flat_map(|(_, d)| d.clone()).collect();
        assert_eq!(text, b"$ exit\n");
    }
}
