//! Conexiones Telnet: negociación mínima del protocolo, eco local y codificaciones antiguas.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::store::{Connection, Encoding, Protocol, TelnetOptions};
use crate::term::{Backend, Feed};

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TERMINAL_TYPE: &[u8] = b"xterm-256color";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Caracteres 128..=255 de la página de códigos 437.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

fn decode_byte(encoding: Encoding, byte: u8, out: &mut Vec<u8>) {
    let table = |b: u8| match encoding {
        Encoding::Latin1 => char::from(b),
        Encoding::Cp437 if b >= 128 => CP437_HIGH.chars().nth(usize::from(b) - 128).unwrap_or('?'),
        _ => char::from(b),
    };
    if encoding == Encoding::Utf8 || byte < 128 {
        out.push(byte);
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(table(byte).encode_utf8(&mut buf).as_bytes());
    }
}

fn encode_char(encoding: Encoding, c: char, out: &mut Vec<u8>) {
    match encoding {
        Encoding::Utf8 => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        _ if (c as u32) < 128 => out.push(c as u8),
        Encoding::Latin1 => out.push(if (c as u32) < 256 { c as u8 } else { b'?' }),
        Encoding::Cp437 => out.push(CP437_HIGH.chars().position(|h| h == c).map_or(b'?', |p| 128 + p as u8)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Cr,
    Iac,
    Verb(u8),
    Sub,
    SubIac,
}

/// Lógica del protocolo, sin red: bytes del servidor a texto y respuestas, y entrada a bytes del cable.
pub struct Telnet {
    options: TelnetOptions,
    state: State,
    sub: Vec<u8>,
    remote_echo: bool,
    /// Opciones que hemos aceptado activar nosotros (WILL) y las que aceptamos del servidor (DO).
    us: HashSet<u8>,
    them: HashSet<u8>,
    size: (u16, u16),
}

impl Telnet {
    pub fn new(options: TelnetOptions, size: (u16, u16)) -> Self {
        Self { options, state: State::Data, sub: Vec::new(), remote_echo: false, us: HashSet::new(), them: HashSet::new(), size }
    }

    fn naws(&self) -> Vec<u8> {
        let (cols, rows) = self.size;
        let mut frame = vec![IAC, SB, OPT_NAWS];
        for byte in [cols.to_be_bytes(), rows.to_be_bytes()].concat() {
            frame.push(byte);
            if byte == IAC {
                frame.push(IAC);
            }
        }
        frame.extend([IAC, SE]);
        frame
    }

    fn verb(&mut self, verb: u8, option: u8, reply: &mut Vec<u8>) {
        match verb {
            WILL => {
                if option == OPT_ECHO {
                    self.remote_echo = true;
                }
                if matches!(option, OPT_ECHO | OPT_SGA) {
                    if self.them.insert(option) {
                        reply.extend([IAC, DO, option]);
                    }
                } else {
                    reply.extend([IAC, DONT, option]);
                }
            }
            WONT => {
                if option == OPT_ECHO {
                    self.remote_echo = false;
                }
                if self.them.remove(&option) {
                    reply.extend([IAC, DONT, option]);
                }
            }
            DO => {
                if matches!(option, OPT_TTYPE | OPT_NAWS | OPT_SGA) {
                    if self.us.insert(option) {
                        reply.extend([IAC, WILL, option]);
                        if option == OPT_NAWS {
                            reply.extend(self.naws());
                        }
                    }
                } else {
                    reply.extend([IAC, WONT, option]);
                }
            }
            DONT => {
                if self.us.remove(&option) {
                    reply.extend([IAC, WONT, option]);
                }
            }
            _ => {}
        }
    }

    fn subnegotiation(&mut self, reply: &mut Vec<u8>) {
        // El servidor pide el tipo de terminal (TERMINAL-TYPE SEND).
        if self.sub.first() == Some(&OPT_TTYPE) && self.sub.get(1) == Some(&1) {
            reply.extend([IAC, SB, OPT_TTYPE, 0]);
            reply.extend_from_slice(TERMINAL_TYPE);
            reply.extend([IAC, SE]);
        }
    }

    /// Procesa bytes del servidor: devuelve el texto para el terminal y lo que hay que responder.
    pub fn receive(&mut self, data: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut text, mut reply) = (Vec::new(), Vec::new());
        if !self.options.negotiate {
            for &byte in data {
                decode_byte(self.options.encoding, byte, &mut text);
            }
            return (text, reply);
        }
        for &byte in data {
            match self.state {
                State::Data | State::Cr => {
                    if self.state == State::Cr && byte == 0 {
                        // CR NUL es un retorno de carro sin salto de línea.
                        self.state = State::Data;
                        continue;
                    }
                    self.state = State::Data;
                    match byte {
                        IAC => self.state = State::Iac,
                        b'\r' => {
                            text.push(byte);
                            self.state = State::Cr;
                        }
                        _ => decode_byte(self.options.encoding, byte, &mut text),
                    }
                }
                State::Iac => match byte {
                    IAC => {
                        decode_byte(self.options.encoding, IAC, &mut text);
                        self.state = State::Data;
                    }
                    WILL | WONT | DO | DONT => self.state = State::Verb(byte),
                    SB => {
                        self.sub.clear();
                        self.state = State::Sub;
                    }
                    _ => self.state = State::Data,
                },
                State::Verb(verb) => {
                    self.verb(verb, byte, &mut reply);
                    self.state = State::Data;
                }
                State::Sub => {
                    if byte == IAC {
                        self.state = State::SubIac;
                    } else {
                        self.sub.push(byte);
                    }
                }
                State::SubIac => match byte {
                    SE => {
                        self.subnegotiation(&mut reply);
                        self.state = State::Data;
                    }
                    IAC => {
                        self.sub.push(IAC);
                        self.state = State::Sub;
                    }
                    _ => self.state = State::Data,
                },
            }
        }
        (text, reply)
    }

    /// Entrada del terminal (UTF-8): devuelve los bytes del cable y el eco local, si hace falta.
    pub fn send(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut wire, mut echo) = (Vec::new(), Vec::new());
        let text = String::from_utf8_lossy(input);
        for c in text.chars() {
            if !self.options.negotiate {
                encode_char(self.options.encoding, c, &mut wire);
                continue;
            }
            match c {
                '\r' => {
                    wire.extend(b"\r\n");
                    echo.extend(b"\r\n");
                }
                '\u{7f}' => {
                    wire.push(0x7f);
                    echo.extend(b"\x08 \x08");
                }
                _ => {
                    let before = wire.len();
                    encode_char(self.options.encoding, c, &mut wire);
                    // Los datos 0xFF se duplican para no confundirlos con IAC.
                    let mut escaped = Vec::new();
                    for &b in &wire[before..] {
                        escaped.push(b);
                        if b == IAC {
                            escaped.push(IAC);
                        }
                    }
                    wire.truncate(before);
                    wire.extend(&escaped);
                    if !c.is_control() {
                        encode_char(Encoding::Utf8, c, &mut echo);
                    }
                }
            }
        }
        let echoed = self.options.negotiate && !self.remote_echo;
        (wire, if echoed { echo } else { Vec::new() })
    }

    /// Avisa del nuevo tamaño si el servidor pidió NAWS.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.size = (cols, rows);
        if self.options.negotiate && self.us.contains(&OPT_NAWS) {
            self.naws()
        } else {
            Vec::new()
        }
    }
}

enum Message {
    Input(Vec<u8>),
    Resize(u16, u16),
}

struct TelnetBackend {
    tx: UnboundedSender<Message>,
}

impl Backend for TelnetBackend {
    fn write(&self, bytes: &[u8]) {
        let _ = self.tx.send(Message::Input(bytes.to_vec()));
    }

    fn resize(&self, cols: usize, rows: usize) {
        let _ = self.tx.send(Message::Resize(cols as u16, rows as u16));
    }
}

fn red(text: &str) -> String {
    format!("\r\n\x1b[31m{text}\x1b[0m\r\n")
}

/// Abre la conexión en segundo plano; el progreso y los errores se escriben en la propia terminal.
pub fn spawn(handle: &tokio::runtime::Handle, connection: Connection, size: (usize, usize), mut feed: Feed) -> Arc<dyn Backend> {
    let (tx, mut rx) = unbounded_channel::<Message>();
    let options = match connection.protocol {
        Protocol::Telnet(options) => options,
        Protocol::Ssh | Protocol::Serial(_) => TelnetOptions::default(),
    };
    let mut telnet = Telnet::new(options, (size.0 as u16, size.1 as u16));
    handle.spawn(async move {
        let address = format!("{}:{}", connection.host, connection.port);
        feed.feed(format!("Conectando a {address}…\r\n").as_bytes());
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                feed.feed(red(&format!("No se pudo conectar a {address}: {error}")).as_bytes());
                feed.disconnect();
                return;
            }
            Err(_) => {
                feed.feed(red(&format!("Tiempo de espera agotado al conectar a {address}")).as_bytes());
                feed.disconnect();
                return;
            }
        };
        let (mut reader, mut writer) = stream.into_split();
        let mut buffer = [0u8; 8192];
        let mut received_any = false;
        loop {
            tokio::select! {
                read = reader.read(&mut buffer) => match read {
                    Ok(0) => {
                        // Si el servidor habló antes de cerrar, se toma como un `exit`: la pestaña se cierra.
                        if received_any {
                            feed.close();
                        } else {
                            feed.feed(red("El servidor cerró la conexión.").as_bytes());
                            feed.disconnect();
                        }
                        return;
                    }
                    Ok(n) => {
                        received_any = true;
                        let (text, reply) = telnet.receive(&buffer[..n]);
                        if !text.is_empty() {
                            feed.feed(&text);
                        }
                        if !reply.is_empty() && writer.write_all(&reply).await.is_err() {
                            feed.disconnect();
                            return;
                        }
                    }
                    Err(error) => {
                        feed.feed(red(&format!("Conexión perdida: {error}")).as_bytes());
                        feed.disconnect();
                        return;
                    }
                },
                message = rx.recv() => match message {
                    Some(Message::Input(bytes)) => {
                        let (wire, echo) = telnet.send(&bytes);
                        if writer.write_all(&wire).await.is_err() {
                            feed.disconnect();
                            return;
                        }
                        if !echo.is_empty() {
                            feed.feed(&echo);
                        }
                    }
                    Some(Message::Resize(cols, rows)) => {
                        let frame = telnet.resize(cols, rows);
                        if !frame.is_empty() {
                            let _ = writer.write_all(&frame).await;
                        }
                    }
                    None => return,
                },
            }
        }
    });
    Arc::new(TelnetBackend { tx })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telnet(negotiate: bool, encoding: Encoding) -> Telnet {
        Telnet::new(TelnetOptions { negotiate, encoding, quiet: false }, (80, 24))
    }

    #[test]
    fn agrees_to_echo_and_sga_and_refuses_unknown_options() {
        let mut t = telnet(true, Encoding::Utf8);
        let (text, reply) = t.receive(&[IAC, WILL, OPT_ECHO, IAC, WILL, OPT_SGA, IAC, WILL, 39, b'h', b'i']);
        assert_eq!(text, b"hi");
        assert_eq!(reply, vec![IAC, DO, OPT_ECHO, IAC, DO, OPT_SGA, IAC, DONT, 39]);
        // Repetir la oferta no vuelve a responder.
        assert!(t.receive(&[IAC, WILL, OPT_ECHO]).1.is_empty());
    }

    #[test]
    fn answers_terminal_type_and_window_size() {
        let mut t = telnet(true, Encoding::Utf8);
        let (_, reply) = t.receive(&[IAC, DO, OPT_TTYPE, IAC, DO, OPT_NAWS, IAC, DO, 99]);
        let mut expected = vec![IAC, WILL, OPT_TTYPE, IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE, IAC, WONT, 99];
        assert_eq!(reply, expected);
        let (_, reply) = t.receive(&[IAC, SB, OPT_TTYPE, 1, IAC, SE]);
        expected = [&[IAC, SB, OPT_TTYPE, 0][..], TERMINAL_TYPE, &[IAC, SE]].concat();
        assert_eq!(reply, expected);
        assert_eq!(t.resize(100, 30), vec![IAC, SB, OPT_NAWS, 0, 100, 0, 30, IAC, SE]);
    }

    #[test]
    fn window_size_is_not_sent_when_the_server_never_asked() {
        let mut t = telnet(true, Encoding::Utf8);
        assert!(t.resize(100, 30).is_empty());
    }

    #[test]
    fn escaped_iac_cr_nul_and_subnegotiation_stay_out_of_the_text() {
        let mut t = telnet(true, Encoding::Latin1);
        let (text, _) = t.receive(&[b'a', IAC, IAC, b'\r', 0, b'b', IAC, SB, 99, 1, 2, IAC, SE, b'c']);
        assert_eq!(text, "a\u{ff}\rbc".as_bytes());
    }

    #[test]
    fn local_echo_is_used_until_the_server_takes_over() {
        let mut t = telnet(true, Encoding::Utf8);
        let (wire, echo) = t.send(b"ls\r");
        assert_eq!(wire, b"ls\r\n");
        assert_eq!(echo, b"ls\r\n");
        t.receive(&[IAC, WILL, OPT_ECHO]);
        let (wire, echo) = t.send(b"x");
        assert_eq!(wire, b"x");
        assert!(echo.is_empty());
    }

    #[test]
    fn raw_mode_passes_bytes_through_untouched() {
        let mut t = telnet(false, Encoding::Utf8);
        let (text, reply) = t.receive(&[IAC, WILL, OPT_ECHO, b'o', b'k']);
        assert_eq!(text, vec![IAC, WILL, OPT_ECHO, b'o', b'k']);
        assert!(reply.is_empty());
        let (wire, echo) = t.send(b"hola\r");
        assert_eq!(wire, b"hola\r");
        assert!(echo.is_empty());
    }

    #[test]
    fn legacy_encodings_convert_both_ways() {
        let mut latin = telnet(true, Encoding::Latin1);
        assert_eq!(latin.receive(&[0xe9]).0, "é".as_bytes());
        assert_eq!(latin.send("é".as_bytes()).0, vec![0xe9]);
        let mut cp = telnet(true, Encoding::Cp437);
        assert_eq!(cp.receive(&[0xc9, 0xcd, 0xbb]).0, "╔═╗".as_bytes());
        assert_eq!(cp.send("ñ".as_bytes()).0, vec![0xa4]);
        assert_eq!(cp.send("€".as_bytes()).0, vec![b'?']);
    }

    #[test]
    fn the_cp437_table_covers_all_high_bytes() {
        assert_eq!(CP437_HIGH.chars().count(), 128);
    }
}
