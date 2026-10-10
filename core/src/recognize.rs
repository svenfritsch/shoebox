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
//!
//! The detector misses faces rolled more than ~30–45° (people lying down).
//! `--rotated` adds a second pass (task `faces-rot`) that sends the same copy
//! turned 90° and 270° and turns the boxes back; a face found that way is
//! only kept where the upright pass found none.
//!
//! Faces the user drew by hand (5c-3) get their embedding from the worker
//! too (task `embed`, protocol 2): at the end of every run, and in the
//! background in `shoebox serve` right after one is drawn (`embed_drawn`).
//!
//! Cats and dogs (phase 6, `pets.rs`) have a pass of their own,
//! `--pets`, task `pets`: the worker (started with `--pets`) finds
//! them and embeds each box with another model. They are rows of
//! `recog.faces` with `species` set, so the viewer, the people and the face
//! check treat them like faces, and no face pass ever touches them.
//!
//! After the passes, the faces are grouped and matched to the people the
//! user named (`clusters.rs`, a cache in the same file).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use libheif_rs::LibHeif;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::say;
use crate::classify::Kind;
use crate::db::{self, Job};
use crate::fingerprint;
use crate::media;
use crate::scan::{Batch, Progress};
use crate::thumbs;

pub const FILE: &str = "recognition.db";
/// The worker protocol this core speaks (`docs/protocol.md`).
pub const PROTOCOL: u32 = 2;
/// Longer edge of the copy the worker gets. Faces down to ~25 px in it are
/// found; more pixels mostly cost time on an old machine.
pub const EDGE: u32 = 1600;
const QUALITY: u8 = 90;
/// The task this module asks for.
pub const FACES: &str = "faces";
/// The second pass over turned copies (`--rotated`), with its own rows in
/// `recog.looked` and `recog.jobs`.
pub const FACES_ROT: &str = "faces-rot";
/// The embedding of a box drawn by hand (protocol 2).
pub const EMBED: &str = "embed";
/// Cats and dogs (an optional task of protocol 2): its own pass, model and
/// embeddings (`pets.rs`).
pub const PETS: &str = "pets";
/// The embedding of a pet's box drawn by hand (with `--pets`).
pub const EMBED_PETS: &str = "embed-pets";
/// A face from the turned copies is kept only if it overlaps every face of
/// the upright pass (and every one kept before it) less than this.
pub const ROTATED_MAX_IOU: f64 = 0.3;
/// Kinds that are looked at; RAW files are covered by their JPEG/HEIC twin.
pub(crate) const KINDS: &str = "'jpeg', 'png', 'heic'";
/// After this many crashes without a reply in between, the worker is broken.
const MAX_CRASHES_IN_ROW: u32 = 5;
/// While waiting for the worker, look this often whether Ctrl-C was pressed.
const POLL: Duration = Duration::from_millis(200);
/// A `running` job that has not reported progress for this long is dead.
const JOB_ALIVE_SECS: i64 = 120;

const SCHEMA_VERSION: i32 = 6;
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
/// v2: faces found by the rotated pass (`--rotated`).
const SCHEMA_V2: &str = "
-- 0: upright pass; 90 or 270: found in the copy turned that far clockwise
-- (box and landmarks are stored upright all the same).
ALTER TABLE recog.faces ADD COLUMN roll INTEGER NOT NULL DEFAULT 0;
";
/// v3 (5c-2): the clustering cache (`clusters.rs`). Recomputed from the
/// faces and the user's decisions in `library.db`; nothing here is user data.
const SCHEMA_V3: &str = "
-- The nearest neighbours of a face (among faces large enough to cluster),
-- computed once per face, so a clustering run resumes where it stopped.
CREATE TABLE IF NOT EXISTS recog.neighbours (
    face INTEGER PRIMARY KEY,       -- recog.faces.id
    list BLOB NOT NULL              -- (face id i64, similarity f32) little-endian, most similar first
);
-- Every face without a decision: its cluster and the person suggested for it.
CREATE TABLE IF NOT EXISTS recog.clusters (
    face       INTEGER PRIMARY KEY, -- recog.faces.id
    cluster    INTEGER NOT NULL,    -- 1 is the largest; numbers change with every run
    person     INTEGER,             -- people.id in library.db, if one is close enough
    similarity REAL
);
";

/// v4 (5c-3): embeddings of faces drawn by hand. The box is the user's
/// (`face_decisions` in `library.db`, `manual = 1`); only its embedding is
/// cached here.
const SCHEMA_V4: &str = "
CREATE TABLE IF NOT EXISTS recog.drawn (
    key     TEXT NOT NULL,              -- files.quick_hash
    x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,  -- the drawn box
    model   TEXT NOT NULL,              -- the faces model; redone after a model change
    aligned INTEGER NOT NULL DEFAULT 0, -- landmarks found in the box: aligned like a detected face
    emb     BLOB,                       -- little-endian f32, L2-normalised; NULL with error
    error   TEXT,
    done_at INTEGER NOT NULL,
    PRIMARY KEY (key, x, y, w, h)
);
";

/// v5 (phase 6): cats and dogs are faces with a species.
const SCHEMA_V5: &str = "
-- NULL: a person's face; 'cat' or 'dog': a pet (its box holds the whole
-- pet, its embedding comes from another model and has another length).
ALTER TABLE recog.faces ADD COLUMN species TEXT;
";

/// v6 (phase 7): where the pets pass saw people, so that a pet's face is not
/// taken for a person's (`pets::is_pet_face`). Boxes are fractions.
const SCHEMA_V6: &str = "
CREATE TABLE IF NOT EXISTS recog.bodies (
    key TEXT NOT NULL,
    x   REAL NOT NULL,
    y   REAL NOT NULL,
    w   REAL NOT NULL,
    h   REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS recog.bodies_key ON bodies (key);
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
    if version < 2 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.pragma_update(Some("recog"), "user_version", 2)?;
        tx.commit()?;
    }
    if version < 3 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V3)?;
        tx.pragma_update(Some("recog"), "user_version", 3)?;
        tx.commit()?;
    }
    if version < 4 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V4)?;
        tx.pragma_update(Some("recog"), "user_version", 4)?;
        tx.commit()?;
    }
    if version < 5 {
        let tx = conn.unchecked_transaction()?;
        // A re-run (the tests lower user_version) must not add the column twice.
        if !db::has_column(&tx, "recog", "faces", "species")? {
            tx.execute_batch(SCHEMA_V5)?;
        }
        tx.pragma_update(Some("recog"), "user_version", 5)?;
        tx.commit()?;
    }
    if version < 6 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V6)?;
        tx.pragma_update(Some("recog"), "user_version", 6)?;
        tx.commit()?;
    }
    Ok(())
}

/// Whether a `recognize` run is going on right now (in any process). The
/// clustering after it has a job of its own (`clusters::running`).
pub fn running(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM recog.jobs WHERE state = 'running' AND updated_at > ?1 AND kind IN ('faces', 'faces-rot', 'pets')",
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

impl WorkerCommand {
    /// The same worker, asked to load the cat and dog models too.
    pub fn with_pets(&self) -> WorkerCommand {
        let mut cmd = self.clone();
        cmd.args.push("--pets".into());
        cmd
    }
}

/// The worker to use: `explicit` (`--recognizer`) or `$SHOEBOX_RECOGNIZER`
/// if given, else the first of the folders from [`worker_dirs`] that holds a
/// `recognizer.py`, the models the run needs (the pet models with `pets`, else
/// the face models) and the standalone Python for this computer; failing
/// that, the first with the models (it then runs with `python3` from `PATH`),
/// else the first with a `recognizer.py`. A folder that has only the faces
/// does not hide an install of the pets, and one installed on another kind of
/// Mac (no `runtime/<os>-<arch>/` here) does not hide a complete one.
pub fn find_worker(root: &Path, explicit: Option<&Path>) -> Option<WorkerCommand> {
    find_worker_for(root, explicit, false)
}

/// [`find_worker`] for a run that needs the pet models when `pets` is set.
pub fn find_worker_for(root: &Path, explicit: Option<&Path>, pets: bool) -> Option<WorkerCommand> {
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
    let dirs: Vec<PathBuf> = worker_dirs(Some(root)).into_iter().filter(|d| d.join("recognizer.py").is_file()).collect();
    let models = |d: &PathBuf| if pets { has_pets(d) } else { has_faces(d) };
    let dir = dirs
        .iter()
        .find(|d| models(d) && bundled_python(d).is_some())
        .or_else(|| dirs.iter().find(|d| models(d)))
        .or(dirs.first())?;
    Some(WorkerCommand { program: python(dir)?, args: vec![dir.join("recognizer.py").into_os_string()] })
}

/// Where an installed recognizer is looked for, best first: `recognizer/` next
/// to the shoebox program (the downloaded folder, on the computer or on a
/// drive), then the drive's own `.shoebox/recognizer/` (made by
/// `install.sh <drive>` or `install.ps1 -Root <drive>`; a program in the older `.shoebox/bin/` layout lands
/// here too). The drive's only serves when nothing is installed next to the
/// program, so a program folder on the computer is used for every drive.
pub fn worker_dirs(root: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(bin) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(bin.join("recognizer"));
    }
    dirs.extend(root.map(|r| r.join(db::DIR).join("recognizer")));
    dirs
}

const FACE_MODELS: [&str; 2] = ["face_detection_yunet_2023mar.onnx", "face_recognition_sface_2021dec.onnx"];
const PET_DETECTOR: &str = "object_detection_yolox_2022nov.onnx";
const PET_EMBEDDERS: [&str; 2] = ["dinov2_small.onnx", "image_classification_ppresnet50_2022jan.onnx"];

fn has_faces(dir: &Path) -> bool {
    FACE_MODELS.iter().all(|m| dir.join("models").join(m).is_file())
}

fn has_pets(dir: &Path) -> bool {
    dir.join("models").join(PET_DETECTOR).is_file() && PET_EMBEDDERS.iter().any(|m| dir.join("models").join(m).is_file())
}

/// What can be recognized for a library: the add-ons found by
/// [`worker_dirs`]. Nothing is started; the files are looked at.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Installed {
    /// The standalone Python and OpenCV for this kind of computer.
    pub runtime: bool,
    /// Models are there (for a runtime that may be missing on this computer).
    pub models: bool,
    /// … which ones, whether or not the runtime for this computer is there.
    pub faces_models: bool,
    pub pets_models: bool,
    pub faces: bool,
    pub pets: bool,
    /// The folder the best install is in (the one `recognize` would use).
    pub dir: Option<PathBuf>,
}

pub fn installed(root: Option<&Path>) -> Installed {
    let dirs: Vec<PathBuf> = worker_dirs(root).into_iter().filter(|d| d.join("recognizer.py").is_file()).collect();
    // Only a folder with the Python for this computer counts as installed:
    // the models work everywhere, the runtime is per kind of computer.
    let ready: Vec<&PathBuf> = dirs.iter().filter(|d| bundled_python(d).is_some()).collect();
    Installed {
        runtime: !ready.is_empty(),
        models: dirs.iter().any(|d| has_faces(d) || has_pets(d)),
        faces_models: dirs.iter().any(|d| has_faces(d)),
        pets_models: dirs.iter().any(|d| has_pets(d)),
        faces: ready.iter().any(|d| has_faces(d)),
        pets: ready.iter().any(|d| has_pets(d)),
        dir: ready.first().map(|d| (*d).clone()),
    }
}

/// The installer script of this system: `install.sh` (macOS, Linux) or
/// `install.ps1` (Windows, run by PowerShell, which every Windows 10 and 11 has).
const INSTALLER: &str = if cfg!(windows) { "install.ps1" } else { "install.sh" };

/// The folder the installer fills when it is given no path: `recognizer/` next
/// to the shoebox program, if that is where the downloaded folder keeps it.
pub fn program_dir() -> Option<PathBuf> {
    let bin = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let dir = bin.join("recognizer");
    dir.join(INSTALLER).is_file().then_some(dir)
}

/// The standalone Python of this kind of computer in `<dir>/runtime/<os>-<arch>/`.
fn bundled_python(dir: &Path) -> Option<PathBuf> {
    let runtime = dir.join("runtime").join(format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
    let bundled = if cfg!(windows) { runtime.join("python.exe") } else { runtime.join("bin").join("python3") };
    bundled.is_file().then_some(bundled)
}

/// The standalone Python shipped in `<dir>/runtime/<os>-<arch>/`, else `python3`
/// from `PATH`.
fn python(dir: &Path) -> Option<PathBuf> {
    if let Some(bundled) = bundled_python(dir) {
        return Some(bundled);
    }
    let name = if cfg!(windows) { "python.exe" } else { "python3" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// The command that runs the installer script `script` for the add-ons asked
/// for, with its errors on the same stream as its output (the script itself
/// prints its errors to stdout on Windows).
fn installer_command(script: &Path, faces: bool, pets: bool) -> Command {
    let mut cmd;
    if cfg!(windows) {
        // Bypass: the policy "scripts are disabled" is the default on some
        // editions and the downloaded script carries a web mark.
        cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(script);
        if faces {
            cmd.arg("-Faces");
        }
        if pets {
            cmd.arg("-Pets");
        }
        // No black console window on top of the Control Panel.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    } else {
        cmd = Command::new("sh");
        cmd.arg("-c").arg("exec sh \"$0\" \"$@\" 2>&1").arg(script);
        if faces {
            cmd.arg("--faces");
        }
        if pets {
            cmd.arg("--pets");
        }
    }
    cmd
}

/// Stop the installer and what it started (tar, pip). On Windows killing the
/// shell alone would leave those running.
fn stop_installer(child: &mut std::process::Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Run the installer (`install.sh`, or `install.ps1` on Windows) from `dir`
/// (see [`program_dir`]) to fill `dir` with the runtime and the models of the
/// add-ons asked for: `faces` and/or `pets`. Its output goes to the report line
/// by line; "Cancel" stops it. Needs the internet.
pub fn install(dir: &Path, faces: bool, pets: bool) -> Result<Installed> {
    let script = dir.join(INSTALLER);
    if !script.is_file() {
        bail!("{} is missing", script.display());
    }
    let mut cmd = installer_command(&script, faces, pets);
    cmd.env("SHOEBOX_QUIET", "1").stdin(Stdio::null()).stdout(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("cannot run {}", script.display()))?;
    let mut out = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = std::io::Read::read(&mut out, &mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut pending = Vec::new();
    let emit = |pending: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(pending).trim().to_string();
        pending.clear();
        if !text.is_empty() {
            say!("{text}");
        }
    };
    loop {
        match rx.recv_timeout(POLL) {
            Ok(chunk) => {
                for b in chunk {
                    if b == b'\n' || b == b'\r' {
                        emit(&mut pending);
                    } else {
                        pending.push(b);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if crate::report::cancelled() {
            stop_installer(&mut child);
            return Err(Interrupted.into());
        }
    }
    emit(&mut pending);
    let status = child.wait()?;
    if !status.success() {
        bail!("the installation failed ({status}); check the internet connection and try again");
    }
    Ok(installed(None))
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

#[derive(Debug, Clone, Deserialize)]
struct Reply {
    width: Option<u32>,
    height: Option<u32>,
    faces: Option<Vec<RawFace>>,
    embed: Option<Vec<RawEmbedded>>,
    #[serde(rename = "embed-pets")]
    embed_pets: Option<Vec<RawEmbedded>>,
    pets: Option<Vec<RawPet>>,
    /// People the pets pass saw, in pixels (`[x, y, w, h]`).
    people: Option<Vec<[f64; 4]>>,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPet {
    species: String,
    bbox: [f64; 4],
    score: f64,
    emb: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawEmbedded {
    /// Empty when no landmarks were found in the box.
    #[serde(default)]
    landmarks: Vec<[f64; 2]>,
    emb: String,
}

/// The embedding of a box drawn by hand.
#[derive(Debug, Clone)]
pub struct Embedded {
    /// Landmarks found in the box (fractions of the picture), so the face
    /// was aligned like a detected one; empty if the plain crop was embedded.
    pub landmarks: Vec<[f64; 2]>,
    pub emb: Vec<f32>,
}

#[derive(Debug, Clone, Deserialize)]
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
    /// 0, or 90/270 for a face the rotated pass found in a turned copy.
    pub roll: u16,
    /// `cat` or `dog` for a pet, `None` for a person's face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
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
    /// People seen by the pets pass (fractions); empty otherwise.
    pub people: Vec<[f64; 4]>,
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
    Interrupted,
}

// ---------------------------------------------------------------- Ctrl-C

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The run was stopped with Ctrl-C. What was found so far is kept.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted; what was found so far is kept, run it again to continue")
    }
}

impl std::error::Error for Interrupted {}

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed) || crate::report::cancelled()
}

/// Let Ctrl-C (and `kill`) end a run in an orderly way: the current batch is
/// committed and the job marked `interrupted`, so the next run can start at
/// once. A second Ctrl-C ends the process the usual way.
#[cfg(unix)]
fn catch_interrupts() {
    extern "C" fn handler(signal: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
        // SAFETY: signal() is async-signal-safe; this restores the default.
        unsafe { libc::signal(signal, libc::SIG_DFL) };
    }
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to an atomic and calls signal().
        unsafe { libc::signal(signal, handler as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

#[cfg(not(unix))]
fn catch_interrupts() {}

impl Process {
    fn spawn(cmd: &WorkerCommand) -> Result<Process> {
        let mut command = Command::new(&cmd.program);
        command
            .args(&cmd.args)
            // No __pycache__ on the drive.
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // Its own process group, so Ctrl-C in the terminal reaches only
        // shoebox, which then stops the worker itself.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
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
        loop {
            if interrupted() {
                return Err(ReadError::Interrupted);
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(POLL);
            match self.lines.recv_timeout(wait) {
                Ok(Some(line)) => return Ok(line),
                Ok(None) | Err(RecvTimeoutError::Disconnected) => return Err(ReadError::Closed),
                Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => return Err(ReadError::Timeout),
                Err(RecvTimeoutError::Timeout) => {}
            }
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
    /// Absent when only the pet models are installed.
    faces: Option<TaskInfo>,
    /// Cats and dogs, if the worker was started with them (`--pets`).
    pets: Option<TaskInfo>,
    /// The worker can embed boxes drawn by hand, comparably to its faces.
    embed: bool,
    /// … and boxes drawn around pets, comparably to its pets.
    embed_pets: bool,
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
        let faces = hello.tasks.get(FACES).cloned();
        if faces.as_ref().is_some_and(|f| f.dim == 0 || f.model.is_empty()) {
            bail!("the recognizer reports no face model");
        }
        let embed = faces.as_ref().is_some_and(|f| hello.tasks.get(EMBED).is_some_and(|e| e.model == f.model && e.dim == f.dim));
        let pets = hello.tasks.get(PETS).filter(|a| a.dim > 0 && !a.model.is_empty()).cloned();
        let embed_pets = pets
            .as_ref()
            .is_some_and(|a| hello.tasks.get(EMBED_PETS).is_some_and(|e| e.model == a.model && e.dim == a.dim));
        if faces.is_none() && pets.is_none() {
            bail!("the recognizer cannot find faces");
        }
        Ok(Worker {
            cmd,
            timeouts,
            process: Some(process),
            faces,
            pets,
            embed,
            embed_pets,
            version: hello.version,
            next_id: 1,
            crashes_in_row: 0,
            restarts: 0,
        })
    }

    /// Model and embedding size for faces, if the face models are installed.
    pub fn faces_model(&self) -> Option<&TaskInfo> {
        self.faces.as_ref()
    }

    /// Model and embedding size for pets, if the worker does them.
    pub fn pets_model(&self) -> Option<&TaskInfo> {
        self.pets.as_ref()
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

    /// Find the cats and dogs in a picture. The faces of the result are the
    /// pets (their `species` is set). The outer error as for `faces`.
    pub fn pets(&mut self, image: &[u8]) -> Result<Result<Found, Failure>> {
        if self.pets.is_none() {
            return Ok(Err(Failure::Refused("the recognizer was not started for pets".into())));
        }
        let request = serde_json::json!({ "tasks": [PETS], "image": BASE64.encode(image) });
        Ok(self.ask(request)?.and_then(|reply| self.parse_pets(reply).map_err(Failure::Refused)))
    }

    /// Embeddings of boxes in a picture (`[x, y, w, h]` in its pixels), in
    /// the same order. The outer error as for `faces`.
    pub fn embed(&mut self, image: &[u8], boxes: &[[f64; 4]]) -> Result<Result<Vec<Embedded>, Failure>> {
        if !self.embed {
            return Ok(Err(Failure::Refused("the recognizer cannot embed faces drawn by hand".into())));
        }
        let request = serde_json::json!({ "tasks": [EMBED], "image": BASE64.encode(image), "boxes": boxes });
        Ok(self.ask(request)?.and_then(|reply| {
            let raw = reply.embed.clone();
            self.parse_embedded(&reply, raw, boxes.len(), self.faces.as_ref().map_or(0, |f| f.dim)).map_err(Failure::Refused)
        }))
    }

    /// Embeddings of boxes drawn around pets (`[x, y, w, h]` in the
    /// picture's pixels), made like the ones of detected pets. The outer
    /// error as for `faces`.
    pub fn embed_pets(&mut self, image: &[u8], boxes: &[[f64; 4]]) -> Result<Result<Vec<Embedded>, Failure>> {
        let Some(dim) = self.pets.as_ref().filter(|_| self.embed_pets).map(|a| a.dim) else {
            return Ok(Err(Failure::Refused("the recognizer cannot embed pets drawn by hand".into())));
        };
        let request = serde_json::json!({ "tasks": [EMBED_PETS], "image": BASE64.encode(image), "boxes": boxes });
        Ok(self.ask(request)?.and_then(|reply| {
            let raw = reply.embed_pets.clone();
            self.parse_embedded(&reply, raw, boxes.len(), dim).map_err(Failure::Refused)
        }))
    }

    /// Whether the worker can embed boxes drawn around pets.
    pub fn can_embed_pets(&self) -> bool {
        self.embed_pets
    }

    fn parse_embedded(&self, reply: &Reply, raw: Option<Vec<RawEmbedded>>, boxes: usize, dim: usize) -> Result<Vec<Embedded>, String> {
        let (fw, fh) = match (reply.width, reply.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
            _ => return Err("reply without the picture's size".into()),
        };
        let raw = raw.ok_or("reply without embeddings")?;
        if raw.len() != boxes {
            return Err(format!("{} embeddings for {boxes} boxes", raw.len()));
        }
        raw.into_iter()
            .map(|e| {
                Ok(Embedded {
                    landmarks: e
                        .landmarks
                        .iter()
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .map(|[px, py]| [(px / fw).clamp(0.0, 1.0), (py / fh).clamp(0.0, 1.0)])
                        .collect(),
                    emb: self.decode_emb(&e.emb, dim)?,
                })
            })
            .collect()
    }

    fn decode_emb(&self, b64: &str, dim: usize) -> Result<Vec<f32>, String> {
        let bytes = BASE64.decode(b64).map_err(|e| format!("embedding is not base64: {e}"))?;
        if bytes.len() != dim * 4 {
            return Err(format!("embedding has {} bytes, expected {}", bytes.len(), dim * 4));
        }
        let emb: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if !emb.iter().all(|v| v.is_finite()) {
            return Err("embedding with invalid numbers".into());
        }
        Ok(emb)
    }

    fn ask(&mut self, mut request: serde_json::Value) -> Result<Result<Reply, Failure>> {
        if self.process.is_none() {
            let (process, hello) = launch(&self.cmd, &self.timeouts).context("restarting the recognizer")?;
            match (hello.tasks.get(FACES), &self.faces) {
                (None, None) => {}
                (Some(t), Some(f)) if t.model == f.model && t.dim == f.dim => {}
                _ => bail!("the recognizer came back with another face model"),
            }
            if let Some(a) = &self.pets {
                match hello.tasks.get(PETS) {
                    Some(t) if t.model == a.model && t.dim == a.dim => {}
                    _ => bail!("the recognizer came back with another pet model"),
                }
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
                    Err(ReadError::Interrupted) => return Err(Interrupted.into()),
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
            let emb = self.decode_emb(&f.emb, self.faces.as_ref().map_or(0, |f| f.dim))?;
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
                roll: 0,
                species: None,
                emb,
            });
        }
        Ok(Found { width, height, faces, people: Vec::new() })
    }

    fn parse_pets(&self, reply: Reply) -> Result<Found, String> {
        let dim = self.pets.as_ref().map_or(0, |a| a.dim);
        let (width, height) = match (reply.width, reply.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => return Err("reply without the picture's size".into()),
        };
        let raw = reply.pets.ok_or("reply without pets")?;
        let (fw, fh) = (width as f64, height as f64);
        // A worker without the people boxes (older) just sends none.
        let people: Vec<[f64; 4]> = reply
            .people
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.iter().all(|v| v.is_finite()) && b[2] > 0.0 && b[3] > 0.0)
            .map(|[x, y, w, h]| {
                let (x0, y0) = ((x / fw).clamp(0.0, 1.0), (y / fh).clamp(0.0, 1.0));
                let (x1, y1) = (((x + w) / fw).clamp(0.0, 1.0), ((y + h) / fh).clamp(0.0, 1.0));
                [x0, y0, x1 - x0, y1 - y0]
            })
            .collect();
        let mut faces = Vec::with_capacity(raw.len());
        for a in raw {
            let [x, y, w, h] = a.bbox;
            if !(a.bbox.iter().all(|v| v.is_finite()) && a.score.is_finite() && w > 0.0 && h > 0.0) {
                return Err("reply with an invalid pet box".into());
            }
            if !crate::pets::is_species(&a.species) {
                return Err(format!("reply with an unknown species {:?}", truncate(&a.species, 20)));
            }
            let emb = self.decode_emb(&a.emb, dim)?;
            let (x0, y0) = ((x / fw).clamp(0.0, 1.0), (y / fh).clamp(0.0, 1.0));
            let (x1, y1) = (((x + w) / fw).clamp(0.0, 1.0), ((y + h) / fh).clamp(0.0, 1.0));
            faces.push(Face {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
                score: a.score,
                landmarks: Vec::new(),
                roll: 0,
                species: Some(a.species),
                emb,
            });
        }
        Ok(Found { width, height, faces, people })
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
            Err(ReadError::Interrupted) => {
                process.stop(Duration::ZERO);
                return Err(Interrupted.into());
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
        Err(e) => Some(Reply {
            width: None,
            height: None,
            faces: None,
            embed: None,
            embed_pets: None,
            pets: None,
            people: None,
            error: Some(format!("bad reply: {e}")),
        }),
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
    /// Look at most at this many pictures (per pass).
    pub limit: Option<usize>,
    /// Try pictures again that failed before.
    pub retry_failed: bool,
    /// After the upright pass, look at the photos turned 90° and 270° too
    /// (faces of people lying down).
    pub rotated: bool,
    /// Then look for cats and dogs too (`pets.rs`): their own pass, with
    /// the worker started with the pet models.
    pub pets: bool,
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
    /// The rotated pass (`--rotated`), if it ran; its `faces` are the ones
    /// it added.
    pub rotated: Option<Box<Stats>>,
    /// The pets pass (`--pets`), if it ran; its `faces` are the cats
    /// and dogs found.
    pub pets: Option<Box<Stats>>,
    /// Faces drawn by hand that got their embedding in this run.
    pub drawn: Option<DrawnStats>,
    /// The clustering after the run.
    pub clusters: Option<crate::clusters::Summary>,
}

/// A picture to look at: one file per content.
struct Pending {
    key: String,
    rel: String,
    kind: Kind,
    size: u64,
    mtime_ns: i64,
}

/// One of the two passes over the photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Upright,
    /// The copy turned 90° and 270°, for faces the upright pass missed.
    Rotated,
    /// Cats and dogs, upright, with the pet models.
    Pets,
}

impl Pass {
    fn task(self) -> &'static str {
        match self {
            Pass::Upright => FACES,
            Pass::Rotated => FACES_ROT,
            Pass::Pets => PETS,
        }
    }

    /// How far (clockwise) the copies sent to the worker are turned.
    fn rolls(self) -> &'static [u16] {
        match self {
            Pass::Upright | Pass::Pets => &[0],
            Pass::Rotated => &[90, 270],
        }
    }
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
    let cmd = find_worker_for(&root, opts.recognizer.as_deref(), opts.pets).ok_or_else(|| {
        anyhow!(
            "face recognition is not installed: no {} found, nor a recognizer/ folder next to shoebox \
             (install the Faces add-on in the Control Panel, step 1, or see recognizer/README.md)",
            root.join(db::DIR).join("recognizer").join("recognizer.py").display()
        )
    })?;
    // Shared: a scan or `serve` may be running alongside.
    let conn = db::open_shared(&db_path)?;
    attach(&conn, &db_path)?;
    if running(&conn)? {
        bail!(
            "another `shoebox recognize` is running (one stopped without Ctrl-C counts as running for {} minutes)",
            JOB_ALIVE_SECS / 60
        );
    }
    conn.execute(
        "UPDATE recog.jobs SET state = 'interrupted', finished_at = updated_at
         WHERE state = 'running' AND kind IN ('faces', 'faces-rot', 'pets')",
        [],
    )?;

    catch_interrupts();
    say!("Starting the recognizer ({})…", cmd.program.display());
    let cmd = if opts.pets { cmd.with_pets() } else { cmd };
    let mut worker = Worker::start(cmd, opts.timeouts)?;
    match worker.faces_model() {
        Some(f) => say!("Recognizer {} ready: faces with {}.", worker.version().unwrap_or("(unknown version)"), f.model),
        None => say!("Recognizer {} ready: no face models installed, so only cats and dogs.", worker.version().unwrap_or("(unknown version)")),
    }
    if opts.pets {
        match worker.pets_model() {
            Some(a) => say!("Cats and dogs with {}.", a.model),
            None => {
                worker.stop();
                bail!(
                    "the recognizer cannot find pets: its models are missing or it is too old \
                     (run recognizer/fetch-models.sh (fetch-models.ps1 on Windows), or the installer in recognizer/ again; see recognizer/README.md)"
                );
            }
        }
    }
    if worker.faces_model().is_none() && !opts.pets {
        worker.stop();
        bail!("the Faces add-on is not installed (install it in the Control Panel, step 1, or run the installer in recognizer/)");
    }
    let first = if worker.faces_model().is_some() {
        recognize(&conn, &root, &mut worker, opts.limit, opts.retry_failed)
    } else {
        Ok(Stats::default())
    };
    let result = first.and_then(|mut stats| {
        if opts.rotated && worker.faces_model().is_some() {
            let rotated = recognize_rotated(&conn, &root, &mut worker, opts.limit, opts.retry_failed)?;
            stats.rotated = Some(Box::new(rotated));
        }
        if opts.pets {
            let pets = recognize_pets(&conn, &root, &mut worker, opts.limit, opts.retry_failed)?;
            stats.pets = Some(Box::new(pets));
        }
        let drawn = embed_drawn(&conn, &root, &mut worker, opts.retry_failed, &interrupted)?;
        if drawn.embedded + drawn.failed > 0 {
            say!(
                "Faces and pets drawn by hand: {} embedded ({} faces without landmarks: not used for suggestions), {} failed.",
                drawn.embedded, drawn.plain, drawn.failed
            );
        }
        stats.drawn = Some(drawn);
        Ok(stats)
    });
    worker.stop();
    // Clusters and suggestions from scratch, with what was found now.
    let result = result.and_then(|mut stats| {
        if crate::clusters::running(&conn)? {
            say!("Clusters: `shoebox serve` is grouping the faces right now; it takes the new ones too.");
        } else {
            stats.clusters = Some(crate::clusters::run_printing(&conn, &interrupted)?);
        }
        Ok(stats)
    });
    // An interrupted run keeps what it found, so that is backed up too.
    if result.as_ref().map_or_else(|e| e.is::<Interrupted>(), |_| true) {
        db::backup_schema(&conn, "recog", &path_for(&db_path))?;
    }
    result
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
    let Some(model) = worker.faces_model().map(|f| f.model.clone()) else {
        bail!("the Faces add-on is not installed");
    };
    let pending = pending(conn, &not_looked_sql(FACES), &model, retry_failed)?;
    let stats = run_pass(conn, root, worker, Pass::Upright, pending, limit)?;

    // Trashed files keep theirs until the trash is emptied.
    let gone = "NOT IN (SELECT quick_hash FROM files)
                AND key NOT IN (SELECT quick_hash FROM trash WHERE quick_hash IS NOT NULL)";
    let pruned = conn.execute(&format!("DELETE FROM recog.looked WHERE key {gone} AND task = '{FACES}'"), [])? as u64;
    conn.execute(&format!("DELETE FROM recog.looked WHERE key {gone}"), [])?;
    forget_neighbours(conn, &format!("key {gone}"), [])?;
    conn.execute(&format!("DELETE FROM recog.faces WHERE key {gone}"), [])?;
    conn.execute(&format!("DELETE FROM recog.bodies WHERE key {gone}"), [])?;
    Ok(Stats { pruned, ..stats })
}

/// Photos not looked at yet for `task` with the worker's model (`?1`), or
/// that failed and are tried again (`?2`): newest first.
fn not_looked_sql(task: &str) -> String {
    format!(
        "SELECT f.quick_hash, f.path, f.kind, f.size, f.mtime_ns
         FROM files f LEFT JOIN recog.looked l ON l.key = f.quick_hash AND l.task = '{task}'
         WHERE f.missing_since IS NULL AND f.kind IN ({KINDS})
           AND (l.key IS NULL OR l.model != ?1 OR (?2 AND l.error IS NOT NULL))
         GROUP BY f.quick_hash
         ORDER BY max(coalesce(f.taken, '')) DESC, min(f.path_nfc)"
    )
}

/// The pets pass: every photo that has not been looked at for cats and
/// dogs with the worker's pet model (newest first), at most `limit`.
/// Needs a worker started for pets. Resumable like `recognize`; `conn`
/// must have `recog` attached.
pub fn recognize_pets(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    limit: Option<usize>,
    retry_failed: bool,
) -> Result<Stats> {
    let Some(model) = worker.pets_model().map(|a| a.model.clone()) else {
        bail!("the recognizer was not started for pets");
    };
    let pending = pending(conn, &not_looked_sql(PETS), &model, retry_failed)?;
    run_pass(conn, root, worker, Pass::Pets, pending, limit)
}

/// The rotated pass: every photo the upright pass looked at (with the same
/// model) is looked at again, turned 90° and 270°, unless that was done
/// already. Faces found where the upright pass found none are added.
/// Resumable like `recognize`; `conn` must have `recog` attached.
pub fn recognize_rotated(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    limit: Option<usize>,
    retry_failed: bool,
) -> Result<Stats> {
    let Some(model) = worker.faces_model().map(|f| f.model.clone()) else {
        bail!("the Faces add-on is not installed");
    };
    let pending = pending(
        conn,
        &format!(
            "SELECT f.quick_hash, f.path, f.kind, f.size, f.mtime_ns
             FROM files f
             JOIN recog.looked u ON u.key = f.quick_hash AND u.task = '{FACES}' AND u.model = ?1 AND u.error IS NULL
             LEFT JOIN recog.looked l ON l.key = f.quick_hash AND l.task = '{FACES_ROT}'
             WHERE f.missing_since IS NULL AND f.kind IN ({KINDS})
               AND (l.key IS NULL OR l.model != ?1 OR (?2 AND l.error IS NOT NULL))
             GROUP BY f.quick_hash
             ORDER BY max(coalesce(f.taken, '')) DESC, min(f.path_nfc)"
        ),
        &model,
        retry_failed,
    )?;
    run_pass(conn, root, worker, Pass::Rotated, pending, limit)
}

/// The pictures a pending-query (`?1` model, `?2` retry failed) returns.
fn pending(conn: &Connection, sql: &str, model: &str, retry_failed: bool) -> Result<Vec<Pending>> {
    Ok(conn
        .prepare(sql)?
        .query_map(params![model, retry_failed], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get(4)?))
        })?
        .filter_map(|row| match row {
            Ok((key, rel, kind, size, mtime_ns)) => {
                Kind::parse(&kind).map(|kind| Ok(Pending { key, rel, kind, size: size as u64, mtime_ns }))
            }
            Err(e) => Some(Err(e)),
        })
        .collect::<rusqlite::Result<_>>()?)
}

/// Look at `pending` (at most `limit`) in a job of the pass's own kind.
fn run_pass(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    pass: Pass,
    mut pending: Vec<Pending>,
    limit: Option<usize>,
) -> Result<Stats> {
    let model = match pass {
        Pass::Pets => worker.pets_model().map(|a| a.model.clone()).unwrap_or_default(),
        _ => worker.faces_model().map(|f| f.model.clone()).unwrap_or_default(),
    };
    let mut stats = Stats { model, ..Stats::default() };
    if let Some(limit) = limit.filter(|&l| l < pending.len()) {
        stats.pending = (pending.len() - limit) as u64;
        pending.truncate(limit);
    }
    if pending.is_empty() {
        return Ok(stats);
    }
    match pass {
        Pass::Upright => say!("Faces: {} pictures to look at…", pending.len()),
        Pass::Rotated => say!("Faces, turned 90° and 270°: {} pictures to look at…", pending.len()),
        Pass::Pets => say!("Cats and dogs: {} pictures to look at…", pending.len()),
    }
    let job = Job::start_in(conn, "recog.jobs", pass.task())?;
    let result = look_at(conn, root, worker, pass, pending, &job, &mut stats);
    stats.restarts = worker.restarts;
    match result {
        Ok(()) => job.finish(conn, "done", &stats)?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            let state = if e.is::<Interrupted>() { "interrupted" } else { "failed" };
            let _ = job.finish(conn, state, &format!("{e:#}"));
            return Err(e);
        }
    }
    Ok(stats)
}

/// The copies of one picture the worker gets, turned by `Pass::rolls`.
struct Prepared {
    /// (roll, JPEG)
    copies: Vec<(u16, Vec<u8>)>,
}

/// Decode on two threads while the worker is busy; store from this one
/// (single writer).
fn look_at(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    pass: Pass,
    pending: Vec<Pending>,
    job: &Job,
    stats: &mut Stats,
) -> Result<()> {
    let started = Instant::now();
    let total = pending.len() as u64;
    let queue = Mutex::new(pending.into_iter());
    let (tx, rx) = mpsc::sync_channel::<(Pending, Result<Prepared, String>)>(4);
    std::thread::scope(|scope| -> Result<()> {
        for _ in 0..2 {
            let tx = tx.clone();
            let queue = &queue;
            scope.spawn(move || {
                let lib_heif = LibHeif::new();
                loop {
                    let Some(p) = queue.lock().unwrap().next() else { break };
                    let image = prepare(&lib_heif, &root.join(&p.rel), &p, pass.rolls());
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
                if interrupted() {
                    return Err(Interrupted.into());
                }
                done += 1;
                let outcome = match image {
                    Ok(prepared) => ask_all(worker, pass, prepared)?,
                    Err(e) if thumbs::is_transient(&e) => {
                        stats.skipped += 1;
                        crate::report::file(&p.rel, false, e.to_string());
                        stats.errors.push(format!("{}: {e}", p.rel));
                        continue;
                    }
                    Err(e) => Err(Failure::Refused(e)),
                };
                if let Err(f) = &outcome {
                    stats.failed += 1;
                    crate::report::file(&p.rel, false, failure_text(f));
                    stats.errors.push(format!("{}: {}", p.rel, failure_text(f)));
                } else {
                    crate::report::file(&p.rel, true, "looked at");
                }
                let kept = match pass {
                    Pass::Upright => store(conn, &p.key, &stats.model, &outcome)?,
                    Pass::Rotated => store_rotated(conn, &p.key, &stats.model, &outcome)?,
                    Pass::Pets => store_pets(conn, &p.key, &stats.model, &outcome)?,
                };
                if outcome.is_ok() {
                    stats.looked += 1;
                    stats.faces += kept as u64;
                }
                if batch.tick()? {
                    job.progress(conn, done, Some(total))?;
                }
                progress.tick(|done, total| {
                    let per_s = done as f64 / started.elapsed().as_secs_f64().max(1e-9);
                    format!("{} {done}/{total} ({per_s:.1}/s)", pass.task())
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
    let what = match pass {
        Pass::Upright => "faces",
        Pass::Rotated => "faces added",
        Pass::Pets => "pets",
    };
    say!(
        "Looked at {} pictures in {:.0}s: {} {what}, {} failed, {} skipped.",
        stats.looked + stats.failed,
        started.elapsed().as_secs_f64(),
        stats.faces,
        stats.failed,
        stats.skipped
    );
    for e in stats.errors.iter().take(20) {
        say!("  {e}");
    }
    if stats.errors.len() > 20 {
        say!("  … {} more", stats.errors.len() - 20);
    }
    Ok(())
}

/// Send every copy of a picture to the worker (a crash is tried once more
/// with a fresh worker: maybe it was not this picture) and put the faces of
/// the turned copies back upright. The first failure fails the picture.
fn ask_all(worker: &mut Worker, pass: Pass, prepared: Prepared) -> Result<Result<Found, Failure>> {
    let mut all: Option<Found> = None;
    for (roll, jpeg) in prepared.copies {
        let ask = |w: &mut Worker| if pass == Pass::Pets { w.pets(&jpeg) } else { w.faces(&jpeg) };
        let found = match ask(worker)? {
            Err(Failure::Crashed(_)) => ask(worker)?,
            other => other,
        };
        let found = match found {
            Ok(found) => found,
            Err(f) => return Ok(Err(f)),
        };
        let (width, height) = if roll % 180 == 0 { (found.width, found.height) } else { (found.height, found.width) };
        let people = if roll == 0 { found.people } else { Vec::new() };
        let faces = found.faces.into_iter().map(|f| unrotate(f, roll));
        match &mut all {
            Some(a) => a.faces.extend(faces),
            None => all = Some(Found { width, height, faces: faces.collect(), people }),
        }
    }
    Ok(all.ok_or_else(|| Failure::Refused("nothing to look at".into())))
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct DrawnStats {
    /// Drawn faces embedded, of them without landmarks (the plain crop).
    pub embedded: u64,
    pub plain: u64,
    /// Faces whose photo could not be read or that the worker refused.
    pub failed: u64,
    /// Skipped this time (the photo changed since the last scan).
    pub skipped: u64,
}

/// Embed the faces and pets drawn by hand that have no embedding of the
/// worker's model yet (`retry_failed`: also those that failed before), photo
/// by photo, each read under the guard. A face drawn by hand is embedded with
/// the faces model, a pet (a decision with a species) with the pets
/// model, which needs a worker started for pets: without one the pets
/// wait. Embeddings of boxes no longer drawn are dropped. `stop` is asked
/// between photos. `conn` has `recog` attached.
pub fn embed_drawn(
    conn: &Connection,
    root: &Path,
    worker: &mut Worker,
    retry_failed: bool,
    stop: &dyn Fn() -> bool,
) -> Result<DrawnStats> {
    let faces_model = worker.faces_model().map(|f| f.model.clone()).unwrap_or_default();
    let pets_model = worker.pets_model().filter(|_| worker.can_embed_pets()).map(|a| a.model.clone());
    conn.execute(
        "DELETE FROM recog.drawn WHERE NOT EXISTS (SELECT 1 FROM face_decisions d WHERE d.manual = 1
           AND d.key = recog.drawn.key AND d.x = recog.drawn.x AND d.y = recog.drawn.y AND d.w = recog.drawn.w AND d.h = recog.drawn.h)",
        [],
    )?;
    // Per content: a present file and the boxes still to embed (`true`: a pet).
    let mut todo: Vec<(Pending, Vec<([f64; 4], bool)>)> = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT d.key, d.x, d.y, d.w, d.h, f.path, f.kind, f.size, f.mtime_ns, d.species IS NOT NULL
             FROM face_decisions d
             JOIN files f ON f.quick_hash = d.key AND f.missing_since IS NULL AND f.kind IN ({KINDS})
             LEFT JOIN recog.drawn e ON e.key = d.key AND e.x = d.x AND e.y = d.y AND e.w = d.w AND e.h = d.h
             WHERE d.manual = 1 AND (d.species IS NULL OR ?3 IS NOT NULL) AND (d.species IS NOT NULL OR ?1 != '')
               AND (e.key IS NULL OR e.model != CASE WHEN d.species IS NULL THEN ?1 ELSE ?3 END
                    OR (?2 AND e.error IS NOT NULL))
             ORDER BY d.key, f.path_nfc"
        ))?;
        let mut rows = stmt.query(params![faces_model, retry_failed, pets_model])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            let b: ([f64; 4], bool) = ([r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?], r.get(9)?);
            let Some(kind) = Kind::parse(&r.get::<_, String>(6)?) else { continue };
            if todo.last().is_none_or(|(p, _)| p.key != key) {
                let p = Pending { key, rel: r.get(5)?, kind, size: r.get::<_, i64>(7)? as u64, mtime_ns: r.get(8)? };
                todo.push((p, Vec::new()));
            }
            let boxes = &mut todo.last_mut().expect("pushed above").1;
            if !boxes.contains(&b) {
                boxes.push(b);
            }
        }
    }
    let mut stats = DrawnStats::default();
    let lib_heif = LibHeif::new();
    for (p, boxes) in todo {
        if stop() {
            return Err(Interrupted.into());
        }
        let copy = fingerprint::read_unchanged(&root.join(&p.rel), p.size, p.mtime_ns, || {
            let img = media::decode_image(&lib_heif, p.kind, &root.join(&p.rel), EDGE).map_err(|e| format!("{e:#}"))?;
            let img = thumbs::shrink(img, EDGE);
            let (w, h) = (img.width() as f64, img.height() as f64);
            media::encode_jpeg(&img, QUALITY).map(|jpeg| (jpeg, w, h)).map_err(|e| format!("{e:#}"))
        });
        // One outcome per box, in order: faces go to `embed`, pets to
        // `embed_pets`, each in one request (a crash is tried once more).
        let outcomes: Vec<Result<Embedded, Failure>> = match copy {
            Ok((jpeg, w, h)) => {
                let mut out: Vec<Option<Result<Embedded, Failure>>> = vec![None; boxes.len()];
                for pets in [false, true] {
                    let which: Vec<usize> = (0..boxes.len()).filter(|&i| boxes[i].1 == pets).collect();
                    if which.is_empty() {
                        continue;
                    }
                    let px: Vec<[f64; 4]> = which.iter().map(|&i| {
                        let b = boxes[i].0;
                        [b[0] * w, b[1] * h, b[2] * w, b[3] * h]
                    }).collect();
                    let ask = |worker: &mut Worker| if pets { worker.embed_pets(&jpeg, &px) } else { worker.embed(&jpeg, &px) };
                    let answer = match ask(worker)? {
                        Err(Failure::Crashed(_)) => ask(worker)?,
                        other => other,
                    };
                    match answer {
                        Ok(list) => {
                            for (&i, e) in which.iter().zip(list) {
                                out[i] = Some(Ok(e));
                            }
                        }
                        Err(f) => {
                            for &i in &which {
                                out[i] = Some(Err(f.clone()));
                            }
                        }
                    }
                }
                out.into_iter().map(|o| o.expect("every box was asked")).collect()
            }
            Err(e) if thumbs::is_transient(&e) => {
                stats.skipped += 1;
                continue;
            }
            Err(e) => boxes.iter().map(|_| Err(Failure::Refused(e.clone()))).collect(),
        };
        let tx = conn.unchecked_transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO recog.drawn (key, x, y, w, h, model, aligned, emb, error, done_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for (&(b, pet), outcome) in boxes.iter().zip(&outcomes) {
                let model = if pet { pets_model.as_deref().unwrap_or_default() } else { faces_model.as_str() };
                // A pet's box is "aligned" like a detected pet's: the whole
                // pet is embedded, there are no landmarks to find.
                let (aligned, emb, error) = match outcome {
                    Ok(e) => {
                        let bytes: Vec<u8> = e.emb.iter().flat_map(|v| v.to_le_bytes()).collect();
                        (pet || !e.landmarks.is_empty(), Some(bytes), None)
                    }
                    Err(f) => (false, None, Some(failure_text(f))),
                };
                match (&error, aligned) {
                    (Some(_), _) => stats.failed += 1,
                    (None, true) => stats.embedded += 1,
                    (None, false) => {
                        stats.embedded += 1;
                        stats.plain += 1;
                    }
                }
                insert.execute(params![p.key, b[0], b[1], b[2], b[3], model, aligned, emb, error, db::now()])?;
            }
        }
        tx.commit()?;
    }
    Ok(stats)
}

/// Drawn faces and pets that wait for an embedding.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrawnPending {
    pub faces: u64,
    pub pets: u64,
}

impl DrawnPending {
    pub fn total(self) -> u64 {
        self.faces + self.pets
    }
}

/// Faces and pets drawn by hand (on present photos) not yet tried with the
/// faces model (`faces_model`, any model if `None`) or the pets model
/// (`pets_model`); ones that failed wait for `--retry-failed`.
pub fn drawn_pending(conn: &Connection, faces_model: Option<&str>, pets_model: Option<&str>) -> Result<DrawnPending> {
    let count = |pets: bool, model: Option<&str>| -> Result<u64> {
        let n: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM face_decisions d
                 WHERE d.manual = 1 AND (d.species IS NOT NULL) = ?2
                   AND d.key IN (SELECT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS}))
                   AND NOT EXISTS (SELECT 1 FROM recog.drawn e WHERE e.key = d.key AND e.x = d.x AND e.y = d.y
                                     AND e.w = d.w AND e.h = d.h AND (?1 IS NULL OR e.model = ?1))"
            ),
            params![model, pets],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    };
    Ok(DrawnPending { faces: count(false, faces_model)?, pets: count(true, pets_model)? })
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::Refused(e) => e.clone(),
        Failure::Crashed(e) => format!("recognizer crashed: {e}"),
    }
}

/// The upright copy the worker gets, turned clockwise by each of `rolls`,
/// read under the guard.
fn prepare(lib_heif: &LibHeif, path: &Path, p: &Pending, rolls: &[u16]) -> Result<Prepared, String> {
    fingerprint::read_unchanged(path, p.size, p.mtime_ns, || {
        let img = media::decode_image(lib_heif, p.kind, path, EDGE).map_err(|e| format!("{e:#}"))?;
        let img = thumbs::shrink(img, EDGE);
        let copies = rolls
            .iter()
            .map(|&roll| {
                let turned = match roll {
                    90 => img.rotate90(),
                    180 => img.rotate180(),
                    270 => img.rotate270(),
                    _ => img.clone(),
                };
                media::encode_jpeg(&turned, QUALITY).map(|jpeg| (roll, jpeg)).map_err(|e| format!("{e:#}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Prepared { copies })
    })
}

/// A point given as fractions of a copy turned `roll`° clockwise, as
/// fractions of the upright picture.
fn unrotate_point([u, v]: [f64; 2], roll: u16) -> [f64; 2] {
    match roll {
        90 => [v, 1.0 - u],
        180 => [1.0 - u, 1.0 - v],
        270 => [1.0 - v, u],
        _ => [u, v],
    }
}

/// A face found in a copy turned `roll`° clockwise, with its box and
/// landmarks turned back onto the upright picture.
pub fn unrotate(face: Face, roll: u16) -> Face {
    let [ax, ay] = unrotate_point([face.x, face.y], roll);
    let [bx, by] = unrotate_point([face.x + face.w, face.y + face.h], roll);
    Face {
        x: ax.min(bx),
        y: ay.min(by),
        w: (ax - bx).abs(),
        h: (ay - by).abs(),
        landmarks: face.landmarks.iter().map(|&p| unrotate_point(p, roll)).collect(),
        roll,
        ..face
    }
}

/// Overlap of two boxes (x, y, w, h): intersection over union.
pub fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let ix = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let iy = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// Replace what is stored for a content with a new result of the upright
/// pass; returns the number of faces stored. The rotated pass depends on
/// what is found upright, so its result is dropped too and redone later.
fn store(conn: &Connection, key: &str, model: &str, outcome: &Result<Found, Failure>) -> Result<usize> {
    // Pets have a pass of their own: this one leaves their rows alone.
    forget_neighbours(conn, "key = ?1 AND species IS NULL", [key])?;
    conn.execute("DELETE FROM recog.faces WHERE key = ?1 AND species IS NULL", [key])?;
    conn.execute("DELETE FROM recog.looked WHERE key = ?1 AND task = ?2", params![key, FACES_ROT])?;
    store_looked(conn, key, FACES, model, outcome)?;
    match outcome {
        Ok(found) => insert_faces(conn, key, model, &found.faces),
        Err(_) => Ok(0),
    }
}

/// Store a result of the rotated pass: of its faces, those that overlap no
/// face of the upright pass (nor a better one of its own) are added. Returns
/// the number added.
fn store_rotated(conn: &Connection, key: &str, model: &str, outcome: &Result<Found, Failure>) -> Result<usize> {
    forget_neighbours(conn, "key = ?1 AND roll != 0 AND species IS NULL", [key])?;
    conn.execute("DELETE FROM recog.faces WHERE key = ?1 AND roll != 0 AND species IS NULL", [key])?;
    let found = match outcome {
        Ok(found) => found,
        Err(_) => return store_looked(conn, key, FACES_ROT, model, outcome).map(|_| 0),
    };
    let mut kept: Vec<[f64; 4]> = conn
        .prepare_cached("SELECT x, y, w, h FROM recog.faces WHERE key = ?1 AND species IS NULL")?
        .query_map([key], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))?
        .collect::<rusqlite::Result<_>>()?;
    let mut candidates: Vec<&Face> = found.faces.iter().collect();
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut added = Vec::new();
    for f in candidates {
        let b = [f.x, f.y, f.w, f.h];
        if kept.iter().all(|&k| iou(k, b) < ROTATED_MAX_IOU) {
            kept.push(b);
            added.push(f.clone());
        }
    }
    let added_found = Found { width: found.width, height: found.height, faces: added, people: Vec::new() };
    store_looked(conn, key, FACES_ROT, model, &Ok(added_found.clone()))?;
    insert_faces(conn, key, model, &added_found.faces)
}

/// Replace what is stored for a content with a new result of the pets
/// pass; returns the number of pets stored. Faces are not touched.
fn store_pets(conn: &Connection, key: &str, model: &str, outcome: &Result<Found, Failure>) -> Result<usize> {
    forget_neighbours(conn, "key = ?1 AND species IS NOT NULL", [key])?;
    conn.execute("DELETE FROM recog.faces WHERE key = ?1 AND species IS NOT NULL", [key])?;
    conn.execute("DELETE FROM recog.bodies WHERE key = ?1", [key])?;
    store_looked(conn, key, PETS, model, outcome)?;
    match outcome {
        Ok(found) => {
            let mut insert = conn.prepare_cached("INSERT INTO recog.bodies (key, x, y, w, h) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for [x, y, w, h] in &found.people {
                insert.execute(params![key, x, y, w, h])?;
            }
            insert_faces(conn, key, model, &found.faces)
        }
        Err(_) => Ok(0),
    }
}

/// Drop the neighbour lists of faces about to be deleted: a new face can get
/// the id of a deleted one, and must not inherit its list.
fn forget_neighbours(conn: &Connection, faces_where: &str, params: impl rusqlite::Params) -> Result<()> {
    conn.prepare_cached(&format!(
        "DELETE FROM recog.neighbours WHERE face IN (SELECT id FROM recog.faces WHERE {faces_where})"
    ))?
    .execute(params)?;
    Ok(())
}

fn store_looked(conn: &Connection, key: &str, task: &str, model: &str, outcome: &Result<Found, Failure>) -> Result<()> {
    match outcome {
        Ok(found) => conn.execute(
            "INSERT OR REPLACE INTO recog.looked (key, task, model, width, height, found, error, done_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
            params![key, task, model, found.width, found.height, found.faces.len() as i64, db::now()],
        )?,
        Err(f) => conn.execute(
            "INSERT OR REPLACE INTO recog.looked (key, task, model, error, done_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, task, model, failure_text(f), db::now()],
        )?,
    };
    Ok(())
}

fn insert_faces(conn: &Connection, key: &str, model: &str, faces: &[Face]) -> Result<usize> {
    let mut insert = conn.prepare_cached(
        "INSERT INTO recog.faces (key, model, x, y, w, h, score, landmarks, emb, roll, species)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for f in faces {
        let emb: Vec<u8> = f.emb.iter().flat_map(|v| v.to_le_bytes()).collect();
        let landmarks = (!f.landmarks.is_empty()).then(|| serde_json::to_string(&f.landmarks)).transpose()?;
        insert.execute(params![key, model, f.x, f.y, f.w, f.h, f.score, landmarks, emb, f.roll, f.species])?;
    }
    Ok(faces.len())
}

/// The faces stored for a content: `None` if it has not been looked at (or
/// failed), else the boxes.
pub fn faces_of(conn: &Connection, key: &str) -> Result<Option<Vec<Face>>> {
    // Looked at for faces or for pets, successfully.
    let looked: bool = conn.query_row(
        "SELECT count(*) > 0 FROM recog.looked WHERE key = ?1 AND task IN (?2, ?3) AND error IS NULL",
        params![key, FACES, PETS],
        |r| r.get(0),
    )?;
    if !looked {
        return Ok(None);
    }
    let faces = conn
        .prepare("SELECT x, y, w, h, score, landmarks, roll, species FROM recog.faces WHERE key = ?1 ORDER BY x")?
        .query_map([key], |r| {
            let landmarks: Option<String> = r.get(5)?;
            Ok(Face {
                x: r.get(0)?,
                y: r.get(1)?,
                w: r.get(2)?,
                h: r.get(3)?,
                score: r.get(4)?,
                landmarks: landmarks.and_then(|l| serde_json::from_str(&l).ok()).unwrap_or_default(),
                roll: r.get(6)?,
                species: r.get(7)?,
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
    /// Photos looked at for cats and dogs (`--pets`), successfully or
    /// not, and the pets found in present photos.
    pub pets_done: u64,
    pub pets: u64,
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
    let (pets_done, pets): (i64, i64) = conn.query_row(
        &format!(
            "SELECT count(l.key), coalesce(sum(l.found), 0) FROM
               (SELECT DISTINCT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS})) f
             JOIN recog.looked l ON l.key = f.quick_hash AND l.task = '{PETS}'"
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Overview {
        done: done as u64,
        total: total as u64,
        faces: faces as u64,
        pets_done: pets_done as u64,
        pets: pets as u64,
        running: running(conn)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_command_names_the_add_ons() {
        let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let cmd = installer_command(Path::new("/a b/recognizer/install"), true, false);
        let args = args(&cmd);
        if cfg!(windows) {
            assert_eq!(cmd.get_program(), "powershell.exe");
            assert!(args.contains(&"-Faces".to_string()) && !args.contains(&"-Pets".to_string()));
            assert!(args.contains(&"Bypass".to_string()));
        } else {
            assert_eq!(cmd.get_program(), "sh");
            assert!(args.contains(&"--faces".to_string()) && !args.contains(&"--pets".to_string()));
        }
        assert!(args.iter().any(|a| a.ends_with("install")), "the script path is one argument, spaces and all");
    }

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

    fn face(x: f64, y: f64, w: f64, h: f64) -> Face {
        Face { x, y, w, h, score: 0.9, landmarks: vec![[x, y]], roll: 0, species: None, emb: Vec::new() }
    }

    fn close(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn turned_faces_are_put_back_upright() {
        // A 400×300 picture turned 90° clockwise is 300×400; the pixel at
        // (u, v) in it came from (v, 300 - u) upright.
        let f = unrotate(face(0.1, 0.5, 0.2, 0.3), 90);
        assert!(close([f.x, f.y, f.w, f.h], [0.5, 0.7, 0.3, 0.2]), "{f:?}");
        assert!(close([f.landmarks[0][0], f.landmarks[0][1], 0.0, 0.0], [0.5, 0.9, 0.0, 0.0]));
        assert_eq!(f.roll, 90);
        // Turned 270°: (u, v) came from (400 - v, u).
        let f = unrotate(face(0.1, 0.5, 0.2, 0.3), 270);
        assert!(close([f.x, f.y, f.w, f.h], [0.2, 0.1, 0.3, 0.2]), "{f:?}");
        assert!(close([f.landmarks[0][0], f.landmarks[0][1], 0.0, 0.0], [0.5, 0.1, 0.0, 0.0]));
        // Turning there and back is the identity.
        let f = unrotate(unrotate(face(0.1, 0.5, 0.2, 0.3), 90), 270);
        assert!(close([f.x, f.y, f.w, f.h], [0.1, 0.5, 0.2, 0.3]), "{f:?}");
        let g = unrotate(unrotate(face(0.1, 0.5, 0.2, 0.3), 90), 180);
        let h = unrotate(face(0.1, 0.5, 0.2, 0.3), 270);
        assert!(close([g.x, g.y, g.w, g.h], [h.x, h.y, h.w, h.h]));
    }

    #[test]
    fn overlap_of_boxes() {
        assert_eq!(iou([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]), 1.0);
        assert_eq!(iou([0.0, 0.0, 1.0, 1.0], [2.0, 2.0, 1.0, 1.0]), 0.0);
        assert!((iou([0.0, 0.0, 2.0, 1.0], [1.0, 0.0, 2.0, 1.0]) - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(iou([0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("Österreich", 3), "Öst");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
