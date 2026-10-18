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

    /// ¿Hay un proceso en primer plano además del shell? `None` si el backend no puede saberlo.
    fn is_busy(&self) -> Option<bool> {
        None
    }
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

/// Racha de salida: de la primera a la última escritura sin pausas largas.
#[derive(Default)]
pub struct Activity {
    first: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
    reported: bool,
}

/// Una pausa de este tamaño sin salida cierra la racha.
pub const BURST_GAP: std::time::Duration = std::time::Duration::from_secs(3);

impl Activity {
    fn record(&mut self) {
        self.record_at(std::time::Instant::now());
    }

    fn record_at(&mut self, now: std::time::Instant) {
        if self.last.is_none_or(|last| now.duration_since(last) > BURST_GAP) {
            self.first = Some(now);
        }
        // Salida nueva: lo ya visto o avisado deja de contar, aunque la racha siga.
        self.reported = false;
        self.last = Some(now);
    }

    /// Duración de la última racha y tiempo desde su última salida, si hay una sin avisar.
    pub fn pending(&self) -> Option<(std::time::Duration, std::time::Duration)> {
        self.pending_at(std::time::Instant::now())
    }

    fn pending_at(&self, now: std::time::Instant) -> Option<(std::time::Duration, std::time::Duration)> {
        let (first, last) = (self.first?, self.last?);
        (!self.reported).then(|| (last.duration_since(first), now.duration_since(last)))
    }

    /// ¿Hubo salida en el último `within`?
    pub fn recent(&self, within: std::time::Duration) -> bool {
        self.last.is_some_and(|last| last.elapsed() <= within)
    }

    pub fn mark_reported(&mut self) {
        self.reported = true;
    }
}

/// Entrada de salida del terminal: el backend la llama con los bytes que recibe.
pub struct Feed {
    term: Arc<Mutex<Term<Proxy>>>,
    processor: Processor,
    exited: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
    on_output: Arc<dyn Fn() + Send + Sync>,
    activity: Arc<Mutex<Activity>>,
}

impl Feed {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut *self.term.lock().unwrap(), bytes);
        self.activity.lock().unwrap().record();
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
    pub activity: Arc<Mutex<Activity>>,
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
        let activity = Arc::new(Mutex::new(Activity::default()));

        let feed = Feed {
            term: term.clone(),
            processor: Processor::new(),
            exited: exited.clone(),
            disconnected: disconnected.clone(),
            on_output: Arc::new(on_output),
            activity: activity.clone(),
        };
        let backend = connect(feed)?;
        let _ = slot.set(Arc::downgrade(&backend));

        Ok(Self { term, title, activity, exited, disconnected, backend, size: (cols, rows) })
    }

    /// Aplica ajustes nuevos a una sesión ya abierta.
    pub fn set_options(&self, options: TermOptions) {
        self.term.lock().unwrap().set_options(options.config());
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn is_busy(&self) -> Option<bool> {
        self.backend.is_busy()
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

#[cfg(test)]
mod activity_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn output_without_long_pauses_is_one_burst_and_is_reported_once() {
        let start = Instant::now();
        let mut activity = Activity::default();
        assert!(activity.pending_at(start).is_none());
        for second in 0..10 {
            activity.record_at(start + Duration::from_secs(second));
        }
        let (length, quiet) = activity.pending_at(start + Duration::from_secs(12)).unwrap();
        assert_eq!(length, Duration::from_secs(9));
        assert_eq!(quiet, Duration::from_secs(3));
        activity.mark_reported();
        assert!(activity.pending_at(start + Duration::from_secs(13)).is_none());
    }

    #[test]
    fn a_long_pause_starts_a_new_burst() {
        let start = Instant::now();
        let mut activity = Activity::default();
        activity.record_at(start);
        activity.record_at(start + Duration::from_secs(1));
        activity.mark_reported();
        activity.record_at(start + Duration::from_secs(30));
        let (length, _) = activity.pending_at(start + Duration::from_secs(31)).unwrap();
        assert_eq!(length, Duration::ZERO);
    }
}
