//! What the long commands (`scan`, `verify`, `recognize`) tell the person who
//! started them. On the command line it is the text they always printed; the
//! launcher installs a sink and gets the same messages as events, plus
//! progress and one result per file, so the CLI and the launcher run the
//! same code.
//!
//! The sink is process-wide: the launcher runs one job at a time.

use std::collections::BTreeMap;
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
    /// One file went through (`ok`) or did not. `note` is English; with a
    /// `key` the launcher says it in the person's language instead (the
    /// message `launcher.note.<key>` with `vars`, see `NOTES`). `of`: it was
    /// a copy of that file (a library path), which stays.
    File {
        path: String,
        ok: bool,
        note: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<&'static str>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        vars: BTreeMap<&'static str, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        of: Option<String>,
    },
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

// ---------------------------------------------------------------- cancel

/// The launcher's "Cancel": the same as Ctrl-C for a command line run. The
/// commands look at it between files, keep what they have committed and stop;
/// running them again continues.
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// The command stopped because it was cancelled.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled; what was done so far is kept, run it again to continue")
    }
}

impl std::error::Error for Cancelled {}

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

/// A note in the words of the system (an error message): shown as it is.
pub fn file(path: &str, ok: bool, note: impl Into<String>) {
    if listening() {
        emit(Event::File { path: path.to_string(), ok, note: note.into(), key: None, vars: BTreeMap::new(), of: None });
    }
}

/// The notes the launcher translates: `launcher.note.<key>` in
/// `core/i18n/*.json`, with these placeholders (`core/tests/i18n.rs`).
pub const NOTES: &[(&str, &[&str])] = &[
    ("hashed", &[]),
    ("thumb_made", &[]),
    ("looked_at", &[]),
    ("recognizer_crashed", &["why"]),
    ("verify_moved", &["path"]),
    ("verify_missing", &[]),
    ("verify_changed", &[]),
    ("verify_size_date", &[]),
    ("verify_unhashed", &[]),
    ("verify_ok", &[]),
    ("verify_damaged", &[]),
    ("backup_missing", &[]),
    ("backup_different", &[]),
    ("backup_removed_here", &[]),
    ("backup_not_safe", &[]),
    ("backup_deleted", &[]),
    ("backup_trashed", &[]),
    ("kept_dates", &[]),
    ("kept_gone", &[]),
    ("kept_unsafe", &["path"]),
    ("kept_changed", &["path"]),
    ("kept_off_drive", &["path"]),
];

/// A note the launcher can say in the person's language: `key` from `NOTES`,
/// its placeholders, and the English text.
#[derive(Debug, Clone)]
pub struct Said {
    pub key: &'static str,
    pub vars: BTreeMap<&'static str, String>,
    pub text: String,
}

impl Said {
    pub fn new(key: &'static str, text: impl Into<String>) -> Self {
        debug_assert!(NOTES.iter().any(|(k, _)| *k == key), "no note {key} in report::NOTES");
        Said { key, vars: BTreeMap::new(), text: text.into() }
    }

    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.vars.insert(name, value.into());
        self
    }
}

impl std::fmt::Display for Said {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// A file with a note the launcher translates.
pub fn said(path: &str, ok: bool, note: Said) {
    if listening() {
        emit(Event::File { path: path.to_string(), ok, note: note.text, key: Some(note.key), vars: note.vars, of: None });
    }
}

/// A copy of `of` went (to the trash, or for good: `note`).
pub fn copy_removed(path: &str, of: &str, note: impl Into<String>) {
    if listening() {
        emit(Event::File { path: path.to_string(), ok: true, note: note.into(), key: None, vars: BTreeMap::new(), of: Some(of.to_string()) });
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
