use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor};
use alacritty_terminal::Term;

/// Destino de la entrada del usuario: un PTY local o un canal SSH.
/// Quien lo implementa también alimenta la salida mediante un [`Feed`].
pub trait Backend: Send + Sync {
    fn write(&self, bytes: &[u8]);
    fn resize(&self, cols: usize, rows: usize);
}

type BackendSlot = Arc<OnceLock<Weak<dyn Backend>>>;

#[derive(Clone)]
pub struct Proxy {
    backend: BackendSlot,
    title: Arc<Mutex<String>>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if let Some(backend) = self.backend.get().and_then(Weak::upgrade) {
                    backend.write(text.as_bytes());
                }
            }
            Event::Title(title) => *self.title.lock().unwrap() = title,
            Event::ResetTitle => self.title.lock().unwrap().clear(),
            _ => {}
        }
    }
}

/// Opciones del emulador que el usuario puede cambiar en los ajustes.
#[derive(Clone, Copy, Debug)]
pub struct TermOptions {
    pub history: usize,
    pub cursor: CursorStyle,
}

impl Default for TermOptions {
    fn default() -> Self {
        Self { history: 10_000, cursor: CursorStyle { shape: CursorShape::Block, blinking: false } }
    }
}

impl TermOptions {
    fn config(self) -> Config {
        Config { kitty_keyboard: true, scrolling_history: self.history, default_cursor_style: self.cursor, ..Config::default() }
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

/// Entrada de salida del terminal: el backend la llama con los bytes que recibe.
pub struct Feed {
    term: Arc<Mutex<Term<Proxy>>>,
    processor: Processor,
    exited: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
    on_output: Arc<dyn Fn() + Send + Sync>,
}

impl Feed {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut *self.term.lock().unwrap(), bytes);
        (self.on_output)();
    }

    /// La sesión terminó a propósito: la pestaña se cierra.
    pub fn close(&mut self) {
        self.exited.store(true, Ordering::SeqCst);
        (self.on_output)();
    }

    /// La conexión se perdió o falló: la pestaña se queda para poder reconectar.
    pub fn disconnect(&mut self) {
        self.disconnected.store(true, Ordering::SeqCst);
        (self.on_output)();
    }
}

pub struct Session {
    pub term: Arc<Mutex<Term<Proxy>>>,
    pub title: Arc<Mutex<String>>,
    exited: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
    backend: Arc<dyn Backend>,
    size: (usize, usize),
}

impl Session {
    /// Crea el terminal y deja que `connect` arranque el backend con el que alimentarlo.
    pub fn start(
        cols: usize,
        rows: usize,
        options: TermOptions,
        on_output: impl Fn() + Send + Sync + 'static,
        connect: impl FnOnce(Feed) -> anyhow::Result<Arc<dyn Backend>>,
    ) -> anyhow::Result<Self> {
        let slot: BackendSlot = Arc::new(OnceLock::new());
        let title = Arc::new(Mutex::new(String::new()));
        let proxy = Proxy { backend: slot.clone(), title: title.clone() };
        let term = Arc::new(Mutex::new(Term::new(options.config(), &GridSize { cols, rows }, proxy)));
        let exited = Arc::new(AtomicBool::new(false));
        let disconnected = Arc::new(AtomicBool::new(false));

        let feed = Feed {
            term: term.clone(),
            processor: Processor::new(),
            exited: exited.clone(),
            disconnected: disconnected.clone(),
            on_output: Arc::new(on_output),
        };
        let backend = connect(feed)?;
        let _ = slot.set(Arc::downgrade(&backend));

        Ok(Self { term, title, exited, disconnected, backend, size: (cols, rows) })
    }

    /// Aplica ajustes nuevos a una sesión ya abierta.
    pub fn set_options(&self, options: TermOptions) {
        self.term.lock().unwrap().set_options(options.config());
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::SeqCst)
    }

    pub fn write(&self, bytes: &[u8]) {
        self.backend.write(bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if (cols, rows) == self.size || cols == 0 || rows == 0 {
            return;
        }
        self.size = (cols, rows);
        self.term.lock().unwrap().resize(GridSize { cols, rows });
        self.backend.resize(cols, rows);
    }
}
