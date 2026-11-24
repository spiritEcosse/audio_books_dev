use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

use crate::error::AppError;

const READ_CHUNK_BYTES: usize = 8192;
const FALLBACK_SHELL: &str = "/bin/zsh";
pub const EXIT_EVENT: &str = "pty-exit";

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// Holds at most one running terminal session (the embedded Claude Code console).
#[derive(Default)]
pub struct PtyState(Mutex<Option<Session>>);

impl PtyState {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<Session>>, AppError> {
        self.0.lock().map_err(|_| AppError::new("terminal lock poisoned"))
    }

    pub fn spawn(
        &self,
        app: AppHandle,
        cwd: &Path,
        rows: u16,
        cols: u16,
        output: Channel<InvokeResponseBody>,
    ) -> Result<(), AppError> {
        let mut slot = self.lock()?;
        if let Some(mut old) = slot.take() {
            let _ = old.child.kill();
        }

        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| AppError::new(format!("cannot open terminal: {e}")))?;

        // A login + interactive shell gives `claude` the user's real PATH,
        // which a Finder-launched .app does not inherit.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| FALLBACK_SHELL.into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.args(["-l", "-i", "-c", "claude"]);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::new(format!("cannot start claude: {e}")))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AppError::new(format!("cannot read terminal: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| AppError::new(format!("cannot write terminal: {e}")))?;

        std::thread::spawn(move || {
            let mut buf = [0u8; READ_CHUNK_BYTES];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || output.send(InvokeResponseBody::Raw(buf[..n].to_vec())).is_err() {
                    break;
                }
            }
            let _ = app.emit(EXIT_EVENT, ());
        });

        *slot = Some(Session { master: pair.master, writer, child });
        Ok(())
    }

    pub fn write(&self, data: &str) -> Result<(), AppError> {
        let mut slot = self.lock()?;
        let session = slot.as_mut().ok_or_else(|| AppError::new("terminal is not running"))?;
        session
            .writer
            .write_all(data.as_bytes())
            .and_then(|_| session.writer.flush())
            .map_err(|e| AppError::new(format!("terminal write failed: {e}")))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), AppError> {
        let slot = self.lock()?;
        let Some(session) = slot.as_ref() else { return Ok(()) };
        session
            .master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| AppError::new(format!("terminal resize failed: {e}")))
    }

    pub fn kill(&self) -> Result<(), AppError> {
        if let Some(mut session) = self.lock()?.take() {
            let _ = session.child.kill();
        }
        Ok(())
    }
}
