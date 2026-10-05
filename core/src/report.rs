//! What the long commands (`scan`, `verify`, `recognize`) tell the person who
//! started them. On the command line it is the text they always printed; the
//! launcher installs a sink and gets the same messages as events, plus
//! progress and one result per file, so the CLI and the launcher run the
//! same code.
//!
//! The sink is process-wide: the launcher runs one job at a time.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A line of the usual output.
    Line { text: String },
    /// How far the current step is (`done` of `total` in the step's own unit).
    Progress { label: String, done: u64, total: u64 },
    /// One file went through (`ok`) or did not.
    File { path: String, ok: bool, note: String },
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
/// With `--json`, standard output carries only the final result.
static TO_STDERR: AtomicBool = AtomicBool::new(false);

/// Keeps a sink installed; the previous state returns when it is dropped.
pub struct Installed(Option<Sink>);

pub fn install(sink: Sink) -> Installed {
    Installed(SINK.lock().unwrap().replace(sink))
}

impl Drop for Installed {
    fn drop(&mut self) {
        *SINK.lock().unwrap() = self.0.take();
    }
}

/// Send the usual output to standard error (so standard output can be JSON).
pub fn text_to_stderr(on: bool) {
    TO_STDERR.store(on, Ordering::SeqCst);
}

pub fn emit(event: Event) {
    let sink = SINK.lock().unwrap().clone();
    if let Some(sink) = sink {
        sink(event);
    }
}

/// True when someone besides the console listens (the launcher).
pub fn listening() -> bool {
    SINK.lock().unwrap().is_some()
}

pub fn line(text: String) {
    if TO_STDERR.load(Ordering::SeqCst) {
        let _ = writeln!(std::io::stderr(), "{text}");
    } else {
        let _ = writeln!(std::io::stdout(), "{text}");
    }
    emit(Event::Line { text });
}

pub fn file(path: &str, ok: bool, note: impl Into<String>) {
    if listening() {
        emit(Event::File { path: path.to_string(), ok, note: note.into() });
    }
}

/// A failure written as `path: reason`, as the commands collect them.
pub fn failed_text(entry: &str) {
    if listening() {
        let (path, note) = entry.split_once(": ").unwrap_or((entry, ""));
        file(path, false, note);
    }
}

/// `println!` that also reaches the launcher.
#[macro_export]
macro_rules! say {
    () => { $crate::report::line(String::new()) };
    ($($t:tt)*) => { $crate::report::line(format!($($t)*)) };
}
