use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::Term;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

#[derive(Clone)]
pub struct Proxy {
    writer: SharedWriter,
    title: Arc<Mutex<String>>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                let _ = self.writer.lock().unwrap().write_all(text.as_bytes());
            }
            Event::Title(title) => *self.title.lock().unwrap() = title,
            Event::ResetTitle => self.title.lock().unwrap().clear(),
            _ => {}
        }
    }
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Session {
    pub term: Arc<Mutex<Term<Proxy>>>,
    pub title: Arc<Mutex<String>>,
    exited: Arc<AtomicBool>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (usize, usize),
}

impl Session {
    pub fn spawn(cols: usize, rows: usize, on_output: impl Fn() + Send + 'static) -> anyhow::Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell().into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            cmd.cwd(home);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let title = Arc::new(Mutex::new(String::new()));
        let proxy = Proxy { writer: writer.clone(), title: title.clone() };
        let term = Arc::new(Mutex::new(Term::new(Config { kitty_keyboard: true, ..Config::default() }, &GridSize { cols, rows }, proxy)));
        let exited = Arc::new(AtomicBool::new(false));

        let mut reader = pair.master.try_clone_reader()?;
        let reader_term = term.clone();
        let reader_exited = exited.clone();
        std::thread::spawn(move || {
            let mut processor: Processor = Processor::new();
            let mut buf = [0u8; 16 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                processor.advance(&mut *reader_term.lock().unwrap(), &buf[..n]);
                on_output();
            }
            reader_exited.store(true, Ordering::SeqCst);
            on_output();
        });

        Ok(Self { term, title, exited, writer, master: pair.master, child, size: (cols, rows) })
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.writer.lock().unwrap().write_all(bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if (cols, rows) == self.size || cols == 0 || rows == 0 {
            return;
        }
        self.size = (cols, rows);
        self.term.lock().unwrap().resize(GridSize { cols, rows });
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn default_shell() -> &'static str {
    if cfg!(windows) { "powershell.exe" } else { "/bin/sh" }
}
