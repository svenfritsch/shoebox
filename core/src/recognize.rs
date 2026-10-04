//! Face recognition, step one: `shoebox recognize` finds the faces in every
//! photo and stores a box and an embedding per face. Matching faces to people
//! comes later (phase 5).
//!
//! The work is done by a separate, optional worker process (the Python
//! `recognizer/`, see `docs/protocol.md`), which this module starts and
//! supervises: a worker that hangs is killed, one that crashes is restarted,
//! and a photo that crashes it twice is recorded as failed so it cannot stop
//! a run.
//!
//! The worker never sees an original. shoebox decodes each photo itself
//! (every format, HEIC included, upright), wrapped in
//! `fingerprint::read_unchanged` like every other read, and sends a JPEG copy
//! of at most `EDGE` px.
//!
//! Results live in `.shoebox/recognition.db`, attached as `recog`, keyed by
//! quick hash like the thumbnails: copies share their faces, a moved file
//! keeps them. A separate file keeps the many embedding BLOBs out of the
//! index, and its writes (and the `recog.jobs` progress) do not count as
//! changes to the index, so the web UI does not reload while a run is going.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use libheif_rs::LibHeif;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::classify::Kind;
use crate::db::{self, Job};
use crate::fingerprint;
use crate::media;
use crate::scan::{Batch, Progress};
use crate::thumbs;

pub const FILE: &str = "recognition.db";
/// The worker protocol this core speaks (`docs/protocol.md`).
pub const PROTOCOL: u32 = 1;
/// Longer edge of the copy the worker gets. Faces down to ~25 px in it are
/// found; more pixels mostly cost time on an old machine.
pub const EDGE: u32 = 1600;
const QUALITY: u8 = 90;
/// The task this module asks for.
pub const FACES: &str = "faces";
/// Kinds that are looked at; RAW files are covered by their JPEG/HEIC twin.
const KINDS: &str = "'jpeg', 'png', 'heic'";
/// After this many crashes without a reply in between, the worker is broken.
const MAX_CRASHES_IN_ROW: u32 = 5;
/// A `running` job that has not reported progress for this long is dead.
const JOB_ALIVE_SECS: i64 = 120;

const SCHEMA_VERSION: i32 = 1;
const SCHEMA_V1: &str = "
CREATE TABLE recog.jobs (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,          -- faces
    state       TEXT NOT NULL,          -- running, done, failed, interrupted
    started_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    finished_at INTEGER,
    done        INTEGER NOT NULL DEFAULT 0,
    total       INTEGER,
    detail      TEXT
);

-- One row per content and task that has been looked at.
CREATE TABLE recog.looked (
    key     TEXT NOT NULL,              -- files.quick_hash
    task    TEXT NOT NULL,              -- faces
    model   TEXT NOT NULL,              -- the worker's model id; results of other models are redone
    width   INTEGER,                    -- of the image the worker saw
    height  INTEGER,
    found   INTEGER NOT NULL DEFAULT 0, -- number of faces
    error   TEXT,                       -- NULL on success
    done_at INTEGER NOT NULL,           -- Unix seconds
    PRIMARY KEY (key, task)
) WITHOUT ROWID;

CREATE TABLE recog.faces (
    id        INTEGER PRIMARY KEY,
    key       TEXT NOT NULL,            -- files.quick_hash
    model     TEXT NOT NULL,
    x         REAL NOT NULL,            -- box as fractions of the upright image
    y         REAL NOT NULL,
    w         REAL NOT NULL,
    h         REAL NOT NULL,
    score     REAL NOT NULL,
    landmarks TEXT,                     -- JSON [[x, y] × 5], fractions like the box
    emb       BLOB NOT NULL             -- little-endian f32, L2-normalised
);
CREATE INDEX recog.faces_key ON faces(key);
";

/// Location of `recognition.db` for a library database.
pub fn path_for(db_path: &Path) -> PathBuf {
    db_path.with_file_name(FILE)
}

/// Attach (and create or migrate) `recognition.db` next to `db_path` as
/// schema `recog`. Same durability settings as the library.
pub fn attach(conn: &Connection, db_path: &Path) -> Result<()> {
    let path = path_for(db_path);
    let path_str = path.to_str().context("recognition.db path is not valid UTF-8")?;
    conn.execute("ATTACH DATABASE ?1 AS recog", [path_str])
        .with_context(|| format!("open {}", path.display()))?;
    conn.pragma_update(Some("recog"), "journal_mode", "DELETE")?;
    conn.pragma_update(Some("recog"), "synchronous", "FULL")?;
    let version: i32 = conn.pragma_query_value(Some("recog"), "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("{FILE} has schema version {version}; this shoebox only knows {SCHEMA_VERSION} (update shoebox)");
    }
    if version < 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.pragma_update(Some("recog"), "user_version", 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Whether a `recognize` run is going on right now (in any process).
pub fn running(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM recog.jobs WHERE state = 'running' AND updated_at > ?1",
        [db::now() - JOB_ALIVE_SECS],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------- finding the worker

/// How to start a worker.
#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// The worker to use: `explicit` (`--recognizer`) or `$SHOEBOX_RECOGNIZER`
/// if given, else `recognizer/recognizer.py` in the library's `.shoebox/` or
/// next to the folder of the shoebox binary (`.shoebox/bin/../recognizer`).
/// A `.py` is run with the standalone Python in `recognizer/runtime/<os>-<arch>/`
/// when there is one, else with `python3` from `PATH`.
pub fn find_worker(root: &Path, explicit: Option<&Path>) -> Option<WorkerCommand> {
    let explicit = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("SHOEBOX_RECOGNIZER").filter(|v| !v.is_empty()).map(PathBuf::from));
    if let Some(program) = explicit {
        if program.extension().is_some_and(|e| e == "py") {
            let dir = program.parent().unwrap_or(Path::new("."));
            return Some(WorkerCommand { program: python(dir)?, args: vec![program.into_os_string()] });
        }
        return Some(WorkerCommand { program, args: Vec::new() });
    }
    let mut dirs = vec![root.join(db::DIR).join("recognizer")];
    if let Some(bin) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(bin.join("..").join("recognizer"));
    }
    dirs.into_iter().find_map(|dir| {
        let script = dir.join("recognizer.py");
        script.is_file().then(|| Some(WorkerCommand { program: python(&dir)?, args: vec![script.into_os_string()] }))?
    })
}

/// The standalone Python shipped in `<dir>/runtime/<os>-<arch>/`, else `python3`
/// from `PATH`.
fn python(dir: &Path) -> Option<PathBuf> {
    let runtime = dir.join("runtime").join(format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
    let bundled = if cfg!(windows) { runtime.join("python.exe") } else { runtime.join("bin").join("python3") };
    if bundled.is_file() {
        return Some(bundled);
    }
    let name = if cfg!(windows) { "python.exe" } else { "python3" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

// ---------------------------------------------------------------- the worker

/// How long to wait for the worker.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// From start to the hello line (Python and the models load first).
    pub start: Duration,
    /// For the reply to one picture.
    pub reply: Duration,
    /// For the worker to exit after its stdin is closed.
    pub exit: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts { start: Duration::from_secs(120), reply: Duration::from_secs(120), exit: Duration::from_secs(5) }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskInfo {
    pub model: String,
    pub dim: usize,
}

#[derive(Debug, Deserialize)]
struct Hello {
    hello: String,
    protocol: u32,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    tasks: HashMap<String, TaskInfo>,
}

#[derive(Debug, Deserialize)]
struct Reply {
    width: Option<u32>,
    height: Option<u32>,
    faces: Option<Vec<RawFace>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFace {
    bbox: [f64; 4],
    score: f64,
    #[serde(default)]
    landmarks: Vec<[f64; 2]>,
    emb: String,
}

/// A face, with coordinates as fractions of the upright picture.
#[derive(Debug, Clone, Serialize)]
pub struct Face {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    pub landmarks: Vec<[f64; 2]>,
    #[serde(skip)]
    pub emb: Vec<f32>,
}

/// The faces in one picture.
#[derive(Debug, Clone)]
pub struct Found {
    /// Size of the picture the worker looked at.
    pub width: u32,
    pub height: u32,
    pub faces: Vec<Face>,
}

/// Why a picture got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The worker answered with an error (or an answer that makes no sense).
    Refused(String),
    /// The worker exited or hung on this picture; it is restarted on the
    /// next request.
    Crashed(String),
}

/// A running worker process, with a thread that hands its stdout lines over.
struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    /// `None` once stdout is closed.
    lines: Receiver<Option<String>>,
}

enum ReadError {
    Timeout,
    Closed,
}

impl Process {
    fn spawn(cmd: &WorkerCommand) -> Result<Process> {
        let mut child = Command::new(&cmd.program)
            .args(&cmd.args)
            // No __pycache__ on the drive.
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("cannot start the recognizer {}", cmd.program.display()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("no stdout")?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Some(l)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(None);
        });
        Ok(Process { child, stdin, lines })
    }

    fn read_line(&self, deadline: Instant) -> Result<String, ReadError> {
        let wait = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(wait) {
            Ok(Some(line)) => Ok(line),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => Err(ReadError::Closed),
            Err(RecvTimeoutError::Timeout) => Err(ReadError::Timeout),
        }
    }

    fn send(&mut self, line: &str) -> std::io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or(std::io::ErrorKind::BrokenPipe)?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// How it ended, if it has.
    fn exit_status(&mut self) -> String {
        // The end of its output can come a moment before the exit is
        // visible; wait briefly so a crash is reported as one.
        let until = Instant::now() + Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return format!("exited ({status})"),
                Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                _ => return "closed its output".into(),
            }
        }
    }

    /// Close stdin, give the worker `grace` to exit, then kill it.
    fn stop(mut self, grace: Duration) {
        drop(self.stdin.take());
        let until = Instant::now() + grace;
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A supervised worker: started on first use, restarted after a crash.
pub struct Worker {
    cmd: WorkerCommand,
    timeouts: Timeouts,
    process: Option<Process>,
    faces: TaskInfo,
    version: Option<String>,
    next_id: u64,
    crashes_in_row: u32,
    /// Restarts after a crash or hang.
    pub restarts: u32,
}

impl Worker {
    /// Start the worker and check its hello. Fails if it does not start,
    /// speaks another protocol or cannot find faces.
    pub fn start(cmd: WorkerCommand, timeouts: Timeouts) -> Result<Worker> {
        let (process, hello) = launch(&cmd, &timeouts)?;
        let faces = hello.tasks.get(FACES).cloned().ok_or_else(|| anyhow!("the recognizer cannot find faces"))?;
        if faces.dim == 0 || faces.model.is_empty() {
            bail!("the recognizer reports no face model");
        }
        Ok(Worker {
            cmd,
            timeouts,
            process: Some(process),
            faces,
            version: hello.version,
            next_id: 1,
            crashes_in_row: 0,
            restarts: 0,
        })
    }

    /// Model and embedding size for faces.
    pub fn faces_model(&self) -> &TaskInfo {
        &self.faces
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Find the faces in a picture (JPEG or PNG bytes). The outer error means
    /// the worker is broken (does not start again, keeps crashing) and the
    /// run should stop.
    pub fn faces(&mut self, image: &[u8]) -> Result<Result<Found, Failure>> {
        let request = serde_json::json!({ "tasks": [FACES], "image": BASE64.encode(image) });
        Ok(self.ask(request)?.and_then(|reply| self.parse_faces(reply).map_err(Failure::Refused)))
    }

    fn ask(&mut self, mut request: serde_json::Value) -> Result<Result<Reply, Failure>> {
        if self.process.is_none() {
            let (process, hello) = launch(&self.cmd, &self.timeouts).context("restarting the recognizer")?;
            match hello.tasks.get(FACES) {
                Some(t) if t.model == self.faces.model && t.dim == self.faces.dim => {}
                _ => bail!("the recognizer came back with another face model"),
            }
            self.process = Some(process);
            self.restarts += 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        request["id"] = id.into();
        let line = serde_json::to_string(&request)?;
        let process = self.process.as_mut().expect("started above");

        let outcome = if let Err(e) = process.send(&line) {
            Err(format!("cannot send to the recognizer ({e}); it {}", process.exit_status()))
        } else {
            let deadline = Instant::now() + self.timeouts.reply;
            loop {
                match process.read_line(deadline) {
                    Ok(line) => match parse_reply(&line, id) {
                        Some(reply) => break Ok(reply),
                        None => eprintln!("recognizer: unexpected output: {}", truncate(&line, 200)),
                    },
                    Err(ReadError::Closed) => break Err(format!("the recognizer {}", process.exit_status())),
                    Err(ReadError::Timeout) => {
                        break Err(format!("the recognizer did not answer within {} s", self.timeouts.reply.as_secs()));
                    }
                }
            }
        };
        match outcome {
            Ok(reply) => {
                self.crashes_in_row = 0;
                Ok(match reply.error.clone() {
                    Some(e) => Err(Failure::Refused(e)),
                    None => Ok(reply),
                })
            }
            Err(message) => {
                if let Some(p) = self.process.take() {
                    p.stop(Duration::ZERO);
                }
                self.crashes_in_row += 1;
                if self.crashes_in_row >= MAX_CRASHES_IN_ROW {
                    bail!("the recognizer failed {MAX_CRASHES_IN_ROW} times in a row; last: {message}");
                }
                eprintln!("recognizer: {message}; restarting it");
                Ok(Err(Failure::Crashed(message)))
            }
        }
    }

    fn parse_faces(&self, reply: Reply) -> Result<Found, String> {
        let (width, height) = match (reply.width, reply.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => return Err("reply without the picture's size".into()),
        };
        let raw = reply.faces.ok_or("reply without faces")?;
        let (fw, fh) = (width as f64, height as f64);
        let mut faces = Vec::with_capacity(raw.len());
        for f in raw {
            let [x, y, w, h] = f.bbox;
            if !(f.bbox.iter().all(|v| v.is_finite()) && f.score.is_finite() && w > 0.0 && h > 0.0) {
                return Err("reply with an invalid face box".into());
            }
            let bytes = BASE64.decode(&f.emb).map_err(|e| format!("embedding is not base64: {e}"))?;
            if bytes.len() != self.faces.dim * 4 {
                return Err(format!("embedding has {} bytes, expected {}", bytes.len(), self.faces.dim * 4));
            }
            let emb: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            if !emb.iter().all(|v| v.is_finite()) {
                return Err("embedding with invalid numbers".into());
            }
            // Boxes may reach past the edge of the picture; keep the part inside.
            let (x0, y0) = ((x / fw).clamp(0.0, 1.0), (y / fh).clamp(0.0, 1.0));
            let (x1, y1) = (((x + w) / fw).clamp(0.0, 1.0), ((y + h) / fh).clamp(0.0, 1.0));
            faces.push(Face {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
                score: f.score,
                landmarks: f
                    .landmarks
                    .iter()
                    .filter(|p| p.iter().all(|v| v.is_finite()))
                    .map(|[px, py]| [(px / fw).clamp(0.0, 1.0), (py / fh).clamp(0.0, 1.0)])
                    .collect(),
                emb,
            });
        }
        Ok(Found { width, height, faces })
    }

    /// Close the worker's stdin and wait for it to exit (kill it after the
    /// exit timeout).
    pub fn stop(mut self) {
        if let Some(p) = self.process.take() {
            p.stop(self.timeouts.exit);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(p) = self.process.take() {
            p.stop(Duration::ZERO);
        }
    }
}

/// Start a worker and read its hello (other lines before it are skipped).
fn launch(cmd: &WorkerCommand, timeouts: &Timeouts) -> Result<(Process, Hello)> {
    let mut process = Process::spawn(cmd)?;
    let deadline = Instant::now() + timeouts.start;
    let hello = loop {
        match process.read_line(deadline) {
            Ok(line) => match serde_json::from_str::<Hello>(&line) {
                Ok(h) if h.hello == "shoebox-recognizer" => break h,
                _ => eprintln!("recognizer: unexpected output: {}", truncate(&line, 200)),
            },
            Err(ReadError::Closed) => {
                let status = process.exit_status();
                process.stop(Duration::ZERO);
                bail!("the recognizer {status} before it was ready");
            }
            Err(ReadError::Timeout) => {
                process.stop(Duration::ZERO);
                bail!("the recognizer did not start within {} s", timeouts.start.as_secs());
            }
        }
    };
    if hello.protocol != PROTOCOL {
        process.stop(timeouts.exit);
        bail!("the recognizer speaks protocol {}, this shoebox {PROTOCOL} (update both together)", hello.protocol);
    }
    Ok((process, hello))
}

/// A reply to request `id`, or `None` for lines that are not one.
fn parse_reply(line: &str, id: u64) -> Option<Reply> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("id")?.as_u64()? != id {
        return None;
    }
    match serde_json::from_value(value) {
        Ok(r) => Some(r),
        Err(e) => Some(Reply { width: None, height: None, faces: None, error: Some(format!("bad reply: {e}")) }),
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ---------------------------------------------------------------- `shoebox recognize`

pub struct Options {
    pub root: PathBuf,
    /// Defaults to `<root>/.shoebox/library.db`.
    pub db: Option<PathBuf>,
    /// The worker program (else `$SHOEBOX_RECOGNIZER` or the installed one).
    pub recognizer: Option<PathBuf>,
    /// Look at most at this many pictures.
    pub limit: Option<usize>,
    /// Try pictures again that failed before.
    pub retry_failed: bool,
    pub timeouts: Timeouts,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    /// Pictures looked at (one per content).
    pub looked: u64,
    pub faces: u64,
    /// Pictures that could not be looked at, stored as failed.
    pub failed: u64,
    /// Pictures skipped this time (changed or moved since the last scan).
    pub skipped: u64,
    /// Pictures still waiting (beyond `--limit`).
    pub pending: u64,
    pub restarts: u32,
    /// Results dropped because their files are gone.
    pub pruned: u64,
    pub model: String,
    /// What went wrong, per file.
    pub errors: Vec<String>,
}

/// A picture to look at: one file per content.
struct Pending {
    key: String,
    rel: String,
    kind: Kind,
    size: u64,
    mtime_ns: i64,
}

pub fn run(opts: &Options) -> Result<Stats> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    let db_path = match &opts.db {
        Some(p) => std::path::absolute(p)?,
        None => db::default_path(&root),
    };
    if !db_path.is_file() {
        bail!("no index at {} (run `shoebox scan` first)", db_path.display());
    }
    let cmd = find_worker(&root, opts.recognizer.as_deref()).ok_or_else(|| {
        anyhow!(
            "face recognition is not installed: no {} found (see recognizer/README.md)",
            root.join(db::DIR).join("recognizer").join("recognizer.py").display()
        )
    })?;
    // Shared: a scan or `serve` may be running alongside.
    let conn = db::open_shared(&db_path)?;
    attach(&conn, &db_path)?;
    if running(&conn)? {
        bail!("another `shoebox recognize` is running");
    }
    conn.execute("UPDATE recog.jobs SET state = 'interrupted', finished_at = updated_at WHERE state = 'running'", [])?;

    println!("Starting the recognizer ({})…", cmd.program.display());
    let mut worker = Worker::start(cmd, opts.timeouts)?;
    println!(
        "Recognizer {} ready: faces with {}.",
        worker.version().unwrap_or("(unknown version)"),
        worker.faces_model().model
    );
    let result = recognize(&conn, &root, &mut worker, opts.limit, opts.retry_failed);
    worker.stop();
    let stats = result?;
    db::backup_schema(&conn, "recog", &path_for(&db_path))?;
    Ok(stats)
}

/// Look for faces in every photo that has not been looked at with the
/// worker's model (newest first), store them, and drop results of content
/// that is gone. `conn` must have `recog` attached. Commits as it goes, so it
/// can be interrupted.
pub fn recognize(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    limit: Option<usize>,
    retry_failed: bool,
) -> Result<Stats> {
    let model = worker.faces_model().model.clone();
    let mut stats = Stats { model: model.clone(), ..Stats::default() };
    let mut pending: Vec<Pending> = conn
        .prepare(&format!(
            "SELECT f.quick_hash, f.path, f.kind, f.size, f.mtime_ns
             FROM files f LEFT JOIN recog.looked l ON l.key = f.quick_hash AND l.task = '{FACES}'
             WHERE f.missing_since IS NULL AND f.kind IN ({KINDS})
               AND (l.key IS NULL OR l.model != ?1 OR (?2 AND l.error IS NOT NULL))
             GROUP BY f.quick_hash
             ORDER BY max(coalesce(f.taken, '')) DESC, min(f.path_nfc)"
        ))?
        .query_map(params![model, retry_failed], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get(4)?))
        })?
        .filter_map(|row| match row {
            Ok((key, rel, kind, size, mtime_ns)) => {
                Kind::parse(&kind).map(|kind| Ok(Pending { key, rel, kind, size: size as u64, mtime_ns }))
            }
            Err(e) => Some(Err(e)),
        })
        .collect::<rusqlite::Result<_>>()?;
    if let Some(limit) = limit.filter(|&l| l < pending.len()) {
        stats.pending = (pending.len() - limit) as u64;
        pending.truncate(limit);
    }

    if !pending.is_empty() {
        println!("Faces: {} pictures to look at…", pending.len());
        let job = Job::start_in(conn, "recog.jobs", FACES)?;
        let result = look_at(conn, root, worker, pending, &job, &mut stats);
        stats.restarts = worker.restarts;
        match result {
            Ok(()) => job.finish(conn, "done", &stats)?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                let _ = job.finish(conn, "failed", &format!("{e:#}"));
                return Err(e);
            }
        }
    }

    // Trashed files keep theirs until the trash is emptied.
    let gone = "NOT IN (SELECT quick_hash FROM files)
                AND key NOT IN (SELECT quick_hash FROM trash WHERE quick_hash IS NOT NULL)";
    stats.pruned = conn.execute(&format!("DELETE FROM recog.looked WHERE key {gone}"), [])? as u64;
    conn.execute(&format!("DELETE FROM recog.faces WHERE key {gone}"), [])?;
    Ok(stats)
}

/// Decode on two threads while the worker is busy; store from this one
/// (single writer).
fn look_at(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    pending: Vec<Pending>,
    job: &Job,
    stats: &mut Stats,
) -> Result<()> {
    let started = Instant::now();
    let total = pending.len() as u64;
    let queue = Mutex::new(pending.into_iter());
    let (tx, rx) = mpsc::sync_channel::<(Pending, Result<Vec<u8>, String>)>(4);
    std::thread::scope(|scope| -> Result<()> {
        for _ in 0..2 {
            let tx = tx.clone();
            let queue = &queue;
            scope.spawn(move || {
                let lib_heif = LibHeif::new();
                loop {
                    let Some(p) = queue.lock().unwrap().next() else { break };
                    let image = prepare(&lib_heif, &root.join(&p.rel), &p);
                    if tx.send((p, image)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut batch = Batch::begin(conn)?;
        let mut progress = Progress::new(total);
        let mut done = 0;
        let result = (|| -> Result<()> {
            for (p, image) in rx.iter() {
                done += 1;
                let outcome = match image {
                    Ok(jpeg) => match worker.faces(&jpeg)? {
                        // Once more with a fresh worker: maybe it was not this picture.
                        Err(Failure::Crashed(_)) => worker.faces(&jpeg)?,
                        other => other,
                    },
                    Err(e) if thumbs::is_transient(&e) => {
                        stats.skipped += 1;
                        stats.errors.push(format!("{}: {e}", p.rel));
                        continue;
                    }
                    Err(e) => Err(Failure::Refused(e)),
                };
                match &outcome {
                    Ok(found) => {
                        stats.looked += 1;
                        stats.faces += found.faces.len() as u64;
                    }
                    Err(f) => {
                        stats.failed += 1;
                        stats.errors.push(format!("{}: {}", p.rel, failure_text(f)));
                    }
                }
                store(conn, &p.key, &stats.model, &outcome)?;
                if batch.tick()? {
                    job.progress(conn, done, Some(total))?;
                }
                progress.tick(|done, total| {
                    let per_s = done as f64 / started.elapsed().as_secs_f64().max(1e-9);
                    format!("faces {done}/{total} ({per_s:.1}/s)")
                });
            }
            Ok(())
        })();
        if result.is_err() {
            // Stop the decoders: drain the queue so they finish their current file.
            queue.lock().unwrap().by_ref().for_each(drop);
            drop(rx);
        }
        // Keep what was found before a broken worker stopped the run.
        let committed = batch.commit();
        result?;
        committed?;
        job.progress(conn, done, Some(total))
    })?;
    println!(
        "Looked at {} pictures in {:.0}s: {} faces, {} failed, {} skipped.",
        stats.looked + stats.failed,
        started.elapsed().as_secs_f64(),
        stats.faces,
        stats.failed,
        stats.skipped
    );
    for e in stats.errors.iter().take(20) {
        println!("  {e}");
    }
    if stats.errors.len() > 20 {
        println!("  … {} more", stats.errors.len() - 20);
    }
    Ok(())
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::Refused(e) => e.clone(),
        Failure::Crashed(e) => format!("recognizer crashed: {e}"),
    }
}

/// The upright copy the worker gets, read under the guard.
fn prepare(lib_heif: &LibHeif, path: &Path, p: &Pending) -> Result<Vec<u8>, String> {
    fingerprint::read_unchanged(path, p.size, p.mtime_ns, || {
        let img = media::decode_image(lib_heif, p.kind, path, EDGE).map_err(|e| format!("{e:#}"))?;
        media::encode_jpeg(&thumbs::shrink(img, EDGE), QUALITY).map_err(|e| format!("{e:#}"))
    })
}

/// Replace what is stored for a content with a new result.
fn store(conn: &Connection, key: &str, model: &str, outcome: &Result<Found, Failure>) -> Result<()> {
    conn.execute("DELETE FROM recog.faces WHERE key = ?1", [key])?;
    match outcome {
        Ok(found) => {
            conn.execute(
                "INSERT OR REPLACE INTO recog.looked (key, task, model, width, height, found, error, done_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                params![key, FACES, model, found.width, found.height, found.faces.len() as i64, db::now()],
            )?;
            let mut insert = conn.prepare_cached(
                "INSERT INTO recog.faces (key, model, x, y, w, h, score, landmarks, emb)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for f in &found.faces {
                let emb: Vec<u8> = f.emb.iter().flat_map(|v| v.to_le_bytes()).collect();
                let landmarks = (!f.landmarks.is_empty()).then(|| serde_json::to_string(&f.landmarks)).transpose()?;
                insert.execute(params![key, model, f.x, f.y, f.w, f.h, f.score, landmarks, emb])?;
            }
        }
        Err(f) => {
            conn.execute(
                "INSERT OR REPLACE INTO recog.looked (key, task, model, error, done_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![key, FACES, model, failure_text(f), db::now()],
            )?;
        }
    }
    Ok(())
}

/// The faces stored for a content: `None` if it has not been looked at (or
/// failed), else the boxes.
pub fn faces_of(conn: &Connection, key: &str) -> Result<Option<Vec<Face>>> {
    let looked: Option<bool> = conn
        .query_row(
            "SELECT error IS NULL FROM recog.looked WHERE key = ?1 AND task = ?2",
            params![key, FACES],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })?;
    if looked != Some(true) {
        return Ok(None);
    }
    let faces = conn
        .prepare("SELECT x, y, w, h, score, landmarks FROM recog.faces WHERE key = ?1 ORDER BY x")?
        .query_map([key], |r| {
            let landmarks: Option<String> = r.get(5)?;
            Ok(Face {
                x: r.get(0)?,
                y: r.get(1)?,
                w: r.get(2)?,
                h: r.get(3)?,
                score: r.get(4)?,
                landmarks: landmarks.and_then(|l| serde_json::from_str(&l).ok()).unwrap_or_default(),
                emb: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(faces))
}

/// Where the library stands, for the web UI.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    /// Photos (one per content) looked at, successfully or not.
    pub done: u64,
    pub total: u64,
    /// Faces found in present photos.
    pub faces: u64,
    /// A `recognize` run is going on.
    pub running: bool,
}

pub fn overview(conn: &Connection) -> Result<Overview> {
    let (total, done, faces): (i64, i64, i64) = conn.query_row(
        &format!(
            "SELECT count(*), count(l.key), coalesce(sum(l.found), 0) FROM
               (SELECT DISTINCT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS})) f
             LEFT JOIN recog.looked l ON l.key = f.quick_hash AND l.task = '{FACES}'"
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(Overview { done: done as u64, total: total as u64, faces: faces as u64, running: running(conn)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_matched_by_id() {
        assert!(parse_reply("not json", 1).is_none());
        assert!(parse_reply(r#"{"id": 2, "faces": []}"#, 1).is_none());
        assert!(parse_reply(r#"[1, 2]"#, 1).is_none());
        let r = parse_reply(r#"{"id": 1, "width": 10, "height": 5, "faces": []}"#, 1).unwrap();
        assert_eq!((r.width, r.height, r.error), (Some(10), Some(5), None));
        let r = parse_reply(r#"{"id": 1, "error": "cannot decode image"}"#, 1).unwrap();
        assert_eq!(r.error.as_deref(), Some("cannot decode image"));
        let r = parse_reply(r#"{"id": 1, "faces": "nonsense"}"#, 1).unwrap();
        assert!(r.error.unwrap().starts_with("bad reply"));
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("Österreich", 3), "Öst");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
