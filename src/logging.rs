//! The log: every line also goes to `logs/openmapper.log` in openOMSI's data folder (a
//! program without a console, as on Windows, would otherwise lose it), the last run's log is
//! kept as `openmapper.previous.log`, and a crash writes its message, place and backtrace
//! there before the program ends.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Where the log is.
pub fn path() -> Option<&'static PathBuf> {
    PATH.get()
}

/// A writer for env_logger: the log file and the console.
struct Tee;

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = FILE.get() {
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = FILE.get() {
            let _ = f.lock().unwrap_or_else(|e| e.into_inner()).flush();
        }
        Ok(())
    }
}

fn open() {
    let dir = crate::install::openomsi_data_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("openmapper.log");
    let _ = std::fs::rename(&path, dir.join("openmapper.previous.log"));
    if let Ok(f) = std::fs::File::create(&path) {
        let _ = FILE.set(Mutex::new(f));
        let _ = PATH.set(path);
    }
}

/// Start the log and the crash report. `filter` is the default level (RUST_LOG overrides).
pub fn init(filter: &str) {
    open();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).target(env_logger::Target::Pipe(Box::new(Tee))).init();
    log::info!("openMapper {} on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "?".into());
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("panic in thread '{}' at {place}: {message}\n{backtrace}", thread.name().unwrap_or("?"));
        // (a thread of the program that is caught carries on; only the window's own thread
        // ending is the end of the program)
        if thread.name() == Some("main") {
            if let Some(p) = path() {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("openMapper")
                    .set_description(format!("openMapper has stopped: {message}\n\nThe log with the details: {}", p.display()))
                    .show();
            }
        }
    }));
}
