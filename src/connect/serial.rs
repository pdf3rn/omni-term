//! Conexiones por puerto serie: apertura, lectura en un hilo, Enter configurable y eco local.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serialport::{DataBits, FlowControl, Parity as PortParity, SerialPort, StopBits};

use crate::data::store::{Connection, Flow, Parity, Protocol, SerialOptions};
use crate::terminal::term::{Backend, Control, Feed, SerialStatus};

/// Convierte la entrada del terminal en bytes del cable y, si hace falta, en eco local.
pub fn translate(options: &SerialOptions, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut wire, mut echo) = (Vec::new(), Vec::new());
    let mut rest = input;
    while !rest.is_empty() {
        let c = rest[0];
        rest = &rest[1..];
        match c {
            b'\r' => {
                wire.extend_from_slice(options.newline.bytes());
                echo.extend_from_slice(b"\r\n");
            }
            0x7f => {
                wire.push(c);
                echo.extend_from_slice(b"\x08 \x08");
            }
            _ => {
                wire.push(c);
                if c >= 0x20 || c >= 0x80 {
                    echo.push(c);
                }
            }
        }
    }
    (wire, if options.local_echo { echo } else { Vec::new() })
}

/// Puertos disponibles: ruta y descripción. Sin `libudev` la enumeración del sistema falla en Linux,
/// así que se recurre a buscar los dispositivos habituales en `/dev`.
pub fn detect_ports() -> Vec<(String, String)> {
    let mut ports: Vec<(String, String)> = serialport::available_ports()
        .map(|list| {
            list.into_iter()
                .map(|p| {
                    let description = match p.port_type {
                        serialport::SerialPortType::UsbPort(usb) => {
                            let name = usb.product.or(usb.manufacturer).unwrap_or_else(|| "USB".into());
                            format!("{name} ({:04x}:{:04x})", usb.vid, usb.pid)
                        }
                        serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
                        serialport::SerialPortType::PciPort => "PCI".into(),
                        serialport::SerialPortType::Unknown => String::new(),
                    };
                    (p.port_name, description)
                })
                .collect()
        })
        .unwrap_or_default();
    if ports.is_empty() && cfg!(unix) {
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let usual = ["ttyUSB", "ttyACM", "ttyAMA", "cu.usb", "cu.serial", "cu.wch"];
                if usual.iter().any(|prefix| name.starts_with(prefix)) {
                    ports.push((format!("/dev/{name}"), String::new()));
                }
            }
        }
    }
    ports.sort();
    ports
}

/// Grupo que da acceso a los puertos serie en esta distribución.
fn serial_group() -> &'static str {
    let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let has = |name: &str| groups.lines().any(|l| l.split(':').next() == Some(name));
    if !has("dialout") && has("uucp") { "uucp" } else { "dialout" }
}

/// Texto para la terminal cuando no se pudo abrir el puerto, con la causa y qué hacer.
fn open_error_message(path: &str, error: &serialport::Error) -> String {
    let raw = match &error.kind {
        serialport::ErrorKind::Io(kind) => Some(*kind),
        _ => None,
    };
    let text = error.to_string();
    let lower = text.to_lowercase();
    if raw == Some(std::io::ErrorKind::PermissionDenied) || lower.contains("permission denied") {
        let group = serial_group();
        format!(
            "Permiso denegado en {path}.\r\nEn Linux hay que estar en el grupo «{group}»:\r\n  sudo usermod -aG {group} $USER\r\nY volver a iniciar sesión."
        )
    } else if matches!(error.kind, serialport::ErrorKind::NoDevice) || raw == Some(std::io::ErrorKind::NotFound) || lower.contains("no such file") {
        format!("No existe {path}. ¿Está conectado el dispositivo?")
    } else if lower.contains("busy") || lower.contains("in use") || lower.contains("denied") {
        format!("{path} está en uso por otro programa.")
    } else {
        format!("No se pudo abrir {path}: {text}")
    }
}

struct DeadBackend;

impl Backend for DeadBackend {
    fn write(&self, _bytes: &[u8]) {}
    fn resize(&self, _cols: usize, _rows: usize) {}
}

struct Lines {
    dtr: bool,
    rts: bool,
    baud: u32,
}

struct SerialBackend {
    port: Mutex<Box<dyn SerialPort>>,
    options: SerialOptions,
    lines: Mutex<Lines>,
    feed: Arc<Mutex<Feed>>,
    stop: Arc<AtomicBool>,
}

impl Backend for SerialBackend {
    fn write(&self, bytes: &[u8]) {
        let (wire, echo) = translate(&self.options, bytes);
        let _ = self.port.lock().unwrap().write_all(&wire);
        if !echo.is_empty() {
            self.feed.lock().unwrap().feed(&echo);
        }
    }

    fn resize(&self, _cols: usize, _rows: usize) {}

    fn control(&self, command: Control) {
        let mut port = self.port.lock().unwrap();
        let mut lines = self.lines.lock().unwrap();
        match command {
            Control::Baud(baud) => {
                if port.set_baud_rate(baud).is_ok() {
                    lines.baud = baud;
                }
            }
            Control::Dtr(on) => {
                if port.write_data_terminal_ready(on).is_ok() {
                    lines.dtr = on;
                }
            }
            Control::Rts(on) => {
                if port.write_request_to_send(on).is_ok() {
                    lines.rts = on;
                }
            }
            Control::Break => {
                if port.set_break().is_ok() {
                    if let Ok(other) = port.try_clone() {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(250));
                            let _ = other.clear_break();
                        });
                    }
                }
            }
        }
    }

    fn serial_status(&self) -> Option<SerialStatus> {
        let lines = self.lines.lock().unwrap();
        Some(SerialStatus { baud: lines.baud, dtr: lines.dtr, rts: lines.rts })
    }
}

impl Drop for SerialBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn red(text: &str) -> String {
    format!("\r\n\x1b[31m{text}\x1b[0m\r\n")
}

/// Abre el puerto. Si no se puede, la pestaña queda desconectada con la causa en la terminal.
pub fn spawn(connection: &Connection, feed: Feed) -> Arc<dyn Backend> {
    let options = match connection.protocol {
        Protocol::Serial(options) => options,
        _ => SerialOptions::default(),
    };
    let feed = Arc::new(Mutex::new(feed));
    let path = connection.host.clone();
    let builder = serialport::new(&path, options.baud)
        .data_bits(match options.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })
        .parity(match options.parity {
            Parity::None => PortParity::None,
            Parity::Odd => PortParity::Odd,
            Parity::Even => PortParity::Even,
        })
        .stop_bits(if options.stop_bits == 2 { StopBits::Two } else { StopBits::One })
        .flow_control(match options.flow {
            Flow::None => FlowControl::None,
            Flow::Software => FlowControl::Software,
            Flow::Hardware => FlowControl::Hardware,
        })
        .timeout(Duration::from_millis(100));
    let (port, mut reader) = match builder.open().and_then(|port| port.try_clone().map(|reader| (port, reader))) {
        Ok(pair) => pair,
        Err(error) => {
            let mut feed = feed.lock().unwrap();
            feed.feed(red(&open_error_message(&path, &error)).as_bytes());
            feed.disconnect();
            return Arc::new(DeadBackend);
        }
    };
    feed.lock().unwrap().feed(format!("Conectado a {path} a {} baudios.\r\n", options.baud).as_bytes());
    let stop = Arc::new(AtomicBool::new(false));
    let thread_feed = feed.clone();
    let thread_stop = stop.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while !thread_stop.load(Ordering::SeqCst) {
            match reader.read(&mut buffer) {
                Ok(0) => {}
                Ok(n) => thread_feed.lock().unwrap().feed(&buffer[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => {
                    let mut feed = thread_feed.lock().unwrap();
                    feed.feed(red(&format!("Dispositivo desconectado: {error}")).as_bytes());
                    feed.disconnect();
                    return;
                }
            }
        }
    });
    Arc::new(SerialBackend { port: Mutex::new(port), options, lines: Mutex::new(Lines { dtr: true, rts: true, baud: options.baud }), feed, stop })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::store::Newline;

    #[test]
    fn enter_follows_the_configured_newline() {
        for (newline, wire) in [(Newline::Cr, &b"ls\r"[..]), (Newline::Lf, b"ls\n"), (Newline::CrLf, b"ls\r\n")] {
            let options = SerialOptions { newline, ..SerialOptions::default() };
            assert_eq!(translate(&options, b"ls\r").0, wire);
        }
    }

    #[test]
    fn local_echo_is_off_by_default_and_handles_backspace_when_on() {
        let quiet = SerialOptions::default();
        assert!(translate(&quiet, b"ls\r").1.is_empty());
        let echoing = SerialOptions { local_echo: true, ..SerialOptions::default() };
        let (wire, echo) = translate(&echoing, b"a\x7f\r");
        assert_eq!(wire, b"a\x7f\r");
        assert_eq!(echo, b"a\x08 \x08\r\n");
    }

    #[test]
    fn open_errors_say_what_to_do() {
        let denied = serialport::Error::new(serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied), "Permission denied");
        let message = open_error_message("/dev/ttyUSB0", &denied);
        assert!(message.contains("usermod -aG") && message.contains("/dev/ttyUSB0"));
        let missing = serialport::Error::new(serialport::ErrorKind::NoDevice, "No such file or directory");
        assert!(open_error_message("/dev/ttyUSB9", &missing).contains("¿Está conectado"));
        let busy = serialport::Error::new(serialport::ErrorKind::Unknown, "Device or resource busy");
        assert!(open_error_message("/dev/ttyUSB0", &busy).contains("en uso"));
    }
}
