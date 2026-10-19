use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::term::{Backend, Feed};

struct PtyBackend {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

impl Backend for PtyBackend {
    fn write(&self, bytes: &[u8]) {
        let _ = self.writer.lock().unwrap().write_all(bytes);
    }

    fn resize(&self, cols: usize, rows: usize) {
        let _ = self.master.lock().unwrap().resize(size(cols, rows));
    }

    #[cfg(unix)]
    fn is_busy(&self) -> Option<bool> {
        let leader = self.master.lock().unwrap().process_group_leader()?;
        let shell = self.child.lock().unwrap().process_id()?;
        Some(leader != shell as i32)
    }

    // ConPTY no expone el grupo de procesos en primer plano.
    #[cfg(not(unix))]
    fn is_busy(&self) -> Option<bool> {
        None
    }
}

impl Drop for PtyBackend {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

fn size(cols: usize, rows: usize) -> PtySize {
    PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }
}

/// Abre `$SHELL` en un PTY local.
pub fn spawn(cols: usize, rows: usize, mut feed: Feed) -> anyhow::Result<Arc<dyn Backend>> {
    let pair = native_pty_system().openpty(size(cols, rows))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell().into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        cmd.cwd(home);
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            feed.feed(&buf[..n]);
        }
        feed.close();
    });

    Ok(Arc::new(PtyBackend {
        writer: Mutex::new(pair.master.take_writer()?),
        master: Mutex::new(pair.master),
        child: Mutex::new(child),
    }))
}

fn default_shell() -> &'static str {
    if cfg!(windows) { "powershell.exe" } else { "/bin/sh" }
}
