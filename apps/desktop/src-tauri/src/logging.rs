//! File logging to `%APPDATA%\PhoneCam\phonecam.log` (INFO). The previous log is kept as
//! `phonecam.log.old` when the file passes ~2 MB, so there are never more than ~4 MB on disk.
//!
//! Never log secrets: the pairing secret `k`, handshake proofs and the full phone URL are kept out
//! of every `tracing` call in the workspace (the room id is logged only as a short prefix).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const FILE: &str = "phonecam.log";

struct Sink {
    path: PathBuf,
    file: File,
    written: u64,
}

impl Sink {
    fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        if std::fs::metadata(&path).map(|m| m.len() >= MAX_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&path, dir.join(format!("{FILE}.old")));
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { path, file, written })
    }

    fn write(&mut self, buf: &[u8]) {
        if self.written >= MAX_BYTES {
            // Roll over: the current file becomes `.old` (replacing the previous one).
            let old = self.path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            if std::fs::rename(&self.path, &old).is_ok() {
                if let Ok(f) = OpenOptions::new().create(true).append(true).open(&self.path) {
                    self.file = f;
                }
            } else {
                // Could not rename (file in use elsewhere): truncate in place instead.
                let _ = self.file.set_len(0);
            }
            self.written = 0;
        }
        if self.file.write_all(buf).is_ok() {
            self.written += buf.len() as u64;
        }
    }
}

#[derive(Clone)]
struct Writer(Arc<Mutex<Sink>>);

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut s) = self.0.lock() {
            s.write(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Writer {
    type Writer = Writer;
    fn make_writer(&'a self) -> Writer {
        self.clone()
    }
}

/// Installs the global subscriber. Safe to call once; failures are silent (logging must never
/// stop the app).
pub fn init(dir: &Path) {
    let Ok(sink) = Sink::open(dir) else { return };
    let writer = Writer(Arc::new(Mutex::new(sink)));
    let ok = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_target(false)
        .with_writer(writer)
        .try_init()
        .is_ok();
    if ok {
        std::panic::set_hook(Box::new(|info| {
            tracing::error!("PANIC: {info}");
        }));
        tracing::info!(
            "---- PhoneCam {} started (pid {}) ----",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_over_past_the_limit() {
        let dir = std::env::temp_dir().join(format!("phonecam-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Sink::open(&dir).unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        s.write(&chunk);
        s.write(&chunk);
        s.write(b"after\n");
        assert!(dir.join("phonecam.log.old").exists());
        assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), b"after\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
