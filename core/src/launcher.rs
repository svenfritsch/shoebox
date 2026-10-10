//! Launcher mode: double-click the binary (no arguments) and a page opens in
//! the browser with a button for each command, so nobody needs a terminal.
//!
//! It is its own small web server, independent of `shoebox serve`: the photo
//! library does not have to run (or even exist yet) for it. The page is
//! embedded like the library UI. The commands are the CLI's own functions;
//! their output arrives through `report` as progress and one result per
//! file, and the final `Stats` / `Report` as JSON.
//!
//! Safety is the same as `serve` without `--lan`: localhost only (checked by
//! peer address and `Host` header against DNS rebinding), `X-Shoebox` on
//! every request that is not a GET, one job at a time. The launcher never
//! writes anywhere a CLI command would not: scan writes `.shoebox/`, the
//! others only read (except the two cleanup buttons, which take duplicate
//! copies off a drive after a scan or a backup check), and every one of them
//! reads originals under the guard.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::report::{self, Event};
use crate::{faces, recognize, scan, serve, verify};

/// Tried first; the next ones if it is taken.
pub const DEFAULT_PORT: u16 = 7879;
/// Failures kept for the result list (counts stay exact).
const MAX_FAILURES: usize = 2000;
const MAX_RECENT_OK: usize = 200;
const MAX_LINES: usize = 300;

pub struct Options {
    /// 0 picks a free port (tests).
    pub port: u16,
    /// Open the page in the browser, and the photo app when it starts.
    pub open_browser: bool,
    /// The JSON file with the remembered paths (default: `config_path()`).
    pub config: Option<PathBuf>,
    /// Opens a file in the file manager (default: `reveal::reveal`); tests pass their own.
    pub reveal: Option<serve::RevealFn>,
}

pub struct Launcher {
    pub addr: SocketAddr,
    pub url: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
    shared: Arc<Shared>,
}

impl Launcher {
    pub fn wait(mut self) -> Result<()> {
        match self.thread.take() {
            Some(t) => t.join().map_err(|_| anyhow::anyhow!("the launcher thread panicked"))?,
            None => Ok(()),
        }
    }

    /// Stop the launcher, and the photo app it started.
    pub fn stop(mut self) -> Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let app = self.shared.app.lock().unwrap().take();
        if let Some(app) = app {
            let _ = app.stop();
        }
        match self.thread.take() {
            Some(t) => t.join().map_err(|_| anyhow::anyhow!("the launcher thread panicked"))?,
            None => Ok(()),
        }
    }
}

/// Start the launcher and, unless told not to, open the page.
pub fn start(opts: &Options) -> Result<Launcher> {
    let (listener, addr) = bind(opts.port)?;
    let url = format!("http://localhost:{}/", addr.port());
    let shared = Arc::new(Shared {
        job: Mutex::new(JobState::default()),
        app: Mutex::new(None),
        open_browser: opts.open_browser,
        next_id: Mutex::new(0),
        config: opts.config.clone().or_else(config_path),
        reveal: opts.reveal.clone().unwrap_or_else(|| -> serve::RevealFn { Arc::new(|p: &Path| crate::reveal::reveal(p)) }),
        config_lock: Mutex::new(()),
    });
    let router = router(shared.clone());
    let (tx, rx) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new().name("shoebox-launcher".into()).spawn(move || -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let stop = async move {
                let _ = rx.await;
            };
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(stop)
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    })?;
    if opts.open_browser {
        open_browser(&url);
    }
    Ok(Launcher { addr, url, shutdown: Some(tx), thread: Some(thread), shared })
}

/// `shoebox` without arguments: run until the window is closed with Ctrl-C.
pub fn run() -> Result<()> {
    let launcher = start(&Options { port: DEFAULT_PORT, open_browser: true, config: None, reveal: None })?;
    println!("shoebox {} launcher: {}", env!("CARGO_PKG_VERSION"), launcher.url);
    println!("Leave this window open while you use shoebox; close it to stop.");
    launcher.wait()
}

fn bind(port: u16) -> Result<(std::net::TcpListener, SocketAddr)> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let ports: Vec<u16> = if port == 0 { vec![0] } else { (port..port.saturating_add(20)).collect() };
    for p in ports {
        if let Ok(l) = std::net::TcpListener::bind(SocketAddr::new(ip, p)) {
            l.set_nonblocking(true)?;
            let addr = l.local_addr()?;
            return Ok((l, addr));
        }
    }
    anyhow::bail!("no free port from {port} on")
}

/// Ask the system to open `url` in the default browser.
pub fn open_browser(url: &str) {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---------------------------------------------------------------- config

/// What the launcher remembers between starts, as JSON:
/// `{ "paths": ["/Volumes/Fotos", "/Volumes/Fotos 2"] }`, plus (all optional)
/// which of them are ticked, every folder ever added (offered in the drop-down
/// again) and the folders a backup check found to be complete copies. It lives
/// in the user's own configuration folder (not on a drive, whose mount point
/// differs from computer to computer), never next to the photos.
/// `$SHOEBOX_CONFIG` names another file.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Config {
    /// The folders in the list, in order.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The ticked ones; absent means all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<Vec<String>>,
    /// Every folder that was ever in the list, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<String>,
    /// Backup folder -> the folder a backup check found it to be a complete copy of.
    /// Written by the launcher after a clean backup check, not by the page.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub backups: std::collections::BTreeMap<String, BackupMark>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BackupMark {
    /// The folder it copies, as it was written in the list.
    pub of: String,
    /// Unix seconds of the clean check.
    pub at: u64,
}

pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SHOEBOX_CONFIG").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let home = || std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)?
    } else {
        std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()).map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))?
    };
    Some(dir.join("shoebox").join("launcher.json"))
}

impl Config {
    /// A missing or unreadable file is an empty configuration.
    pub fn load(path: &Path) -> Config {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<()> {
        use anyhow::Context;
        let dir = path.parent().context("the configuration file has no folder")?;
        std::fs::create_dir_all(dir)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Trimmed, without empty entries and duplicates.
    fn clean_list(paths: Vec<String>, max: usize) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        paths
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty() && p.len() < 4096 && seen.insert(p.clone()))
            .take(max)
            .collect()
    }

    /// The list and ticks the page sent, on top of what is already stored: the
    /// history grows, the backup marks stay as the launcher wrote them.
    fn updated(self, body: Config) -> Config {
        let paths = Config::clean_list(body.paths, 50);
        let checked = body.checked.map(|c| {
            let c = Config::clean_list(c, 50);
            paths.iter().filter(|p| c.contains(p)).cloned().collect::<Vec<_>>()
        });
        let history = Config::clean_list(paths.iter().cloned().chain(self.history).collect(), 100);
        Config { paths, checked, history, backups: self.backups }
    }
}

async fn config_get(State(shared): State<Arc<Shared>>) -> Json<Config> {
    Json(shared.config.as_deref().map(Config::load).unwrap_or_default())
}

async fn config_set(State(shared): State<Arc<Shared>>, Json(body): Json<Config>) -> Result<Json<Config>, ApiError> {
    let _lock = shared.config_lock.lock().unwrap();
    let config = shared.config.as_deref().map(Config::load).unwrap_or_default().updated(body);
    if let Some(path) = &shared.config {
        config.save(path).map_err(ApiError::Internal)?;
    }
    Ok(Json(config))
}

/// A clean backup check marks the backup folder as "backup of <original>"; any
/// other result takes the mark away. Keyed by the folders as the page wrote them.
fn mark_backup(shared: &Shared, original: &str, backup: &str, complete: bool) {
    let Some(path) = &shared.config else { return };
    let _lock = shared.config_lock.lock().unwrap();
    let mut config = Config::load(path);
    let (original, backup) = (original.trim().to_string(), backup.trim().to_string());
    if complete {
        config.backups.insert(backup, BackupMark { of: original, at: now_secs() });
    } else if config.backups.remove(&backup).is_none() {
        return;
    }
    let _ = config.save(path);
}

#[derive(Deserialize)]
struct RevealItem {
    /// One of the current job's folders.
    root: String,
    /// Relative to it, as in the result list.
    path: String,
}

#[derive(Deserialize)]
struct RevealRequest {
    items: Vec<RevealItem>,
}

/// "Show in Finder" for a file of the result list, on each folder that has it
/// (the backup check lists a file on the original and on the backup). Only
/// folders of the job on screen and paths inside them; nothing is opened.
async fn reveal_files(State(shared): State<Arc<Shared>>, Json(body): Json<RevealRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let job_roots: Vec<PathBuf> = shared.job.lock().unwrap().roots.iter().filter_map(|r| PathBuf::from(r).canonicalize().ok()).collect();
    let mut targets = Vec::new();
    for item in &body.items {
        let root = PathBuf::from(&item.root).canonicalize().ok().filter(|r| job_roots.contains(r)).ok_or(ApiError::BadRequest("not a folder of this job".into()))?;
        let Ok(file) = root.join(&item.path).canonicalize() else { continue };
        if file.starts_with(&root) && file.exists() {
            targets.push(file);
        }
    }
    let reveal = shared.reveal.clone();
    let found = targets.len();
    let opened = tokio::task::spawn_blocking(move || targets.iter().filter(|t| reveal(t).is_ok()).count()).await.unwrap_or(0);
    Ok(Json(serde_json::json!({ "found": found, "opened": opened })))
}

// ---------------------------------------------------------------- state

struct Shared {
    job: Mutex<JobState>,
    /// The photo app (`serve`) once the button started it.
    app: Mutex<Option<AppRunning>>,
    open_browser: bool,
    next_id: Mutex<u64>,
    config: Option<PathBuf>,
    reveal: serve::RevealFn,
    /// Serialises read-modify-write of the config file.
    config_lock: Mutex<()>,
}

struct AppRunning {
    roots: Vec<PathBuf>,
    url: String,
    server: serve::Server,
}

impl AppRunning {
    fn stop(self) -> Result<()> {
        self.server.stop()
    }
}

#[derive(Default, Serialize)]
struct JobState {
    id: u64,
    kind: String,
    root: String,
    running: bool,
    started_at: u64,
    finished_at: Option<u64>,
    /// `Some(true)` when the job ended with nothing wrong.
    ok: Option<bool>,
    /// Stopped with "Cancel"; what was done is kept.
    cancelled: bool,
    roots: Vec<String>,
    /// The folder being worked on (several can be given).
    current_root: Option<String>,
    /// Added in front of file names when several folders are processed.
    #[serde(skip)]
    prefix: String,
    /// One entry per folder, as far as they got.
    results: Vec<RootResult>,
    progress: Option<ProgressInfo>,
    lines: VecDeque<String>,
    ok_count: u64,
    fail_count: u64,
    /// Every file seen so far and whether all its steps worked.
    #[serde(skip)]
    seen: std::collections::HashMap<String, bool>,
    /// Files that failed, in order (up to `MAX_FAILURES`).
    failures: Vec<FileResult>,
    /// The latest files that went through.
    recent_ok: VecDeque<FileResult>,
    /// The final `Stats` / `Report` of a single folder, as the CLI's `--json`
    /// prints it (`results` has them for all folders).
    result: Option<serde_json::Value>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
struct RootResult {
    root: String,
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
struct ProgressInfo {
    label: String,
    done: u64,
    total: u64,
}

#[derive(Clone, Serialize)]
struct FileResult {
    path: String,
    ok: bool,
    note: String,
}

impl JobState {
    fn apply(&mut self, event: Event) {
        match event {
            Event::Line { text } => {
                if self.lines.len() >= MAX_LINES {
                    self.lines.pop_front();
                }
                self.lines.push_back(text);
            }
            Event::Progress { label, done, total } => self.progress = Some(ProgressInfo { label, done, total }),
            Event::File { path, ok, note } => {
                let path = if self.prefix.is_empty() { path } else { format!("{}{path}", self.prefix) };
                // A file counts once, however many steps look at it (hash,
                // thumbnail, …); one failed step makes it failed.
                let before = self.seen.get(&path).copied();
                match (before, ok) {
                    (None, true) => {
                        self.ok_count += 1;
                        self.seen.insert(path.clone(), true);
                    }
                    (None, false) => {
                        self.fail_count += 1;
                        self.seen.insert(path.clone(), false);
                    }
                    (Some(true), false) => {
                        self.ok_count -= 1;
                        self.fail_count += 1;
                        self.seen.insert(path.clone(), false);
                        self.recent_ok.retain(|f| f.path != path);
                    }
                    _ => return,
                }
                let item = FileResult { path, ok, note };
                if ok {
                    if self.recent_ok.len() >= MAX_RECENT_OK {
                        self.recent_ok.pop_front();
                    }
                    self.recent_ok.push_back(item);
                } else if self.failures.len() < MAX_FAILURES {
                    self.failures.push(item);
                }
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------- jobs

#[derive(Deserialize, Clone)]
struct JobRequest {
    /// `scan`, `scan_cleanup` (remove the copies the last scan found), `verify`,
    /// `forget_missing` (drop the records of files that are gone),
    /// `recognize`, `recognize_pets` (the same, then cats and dogs too),
    /// `recognize_rotated` (also faces lying down), `faces_stats`, `backup` or `backup_cleanup`.
    kind: String,
    /// One folder, or several in `roots`: they are processed one after the other.
    #[serde(default)]
    root: String,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    quick: bool,
    #[serde(default)]
    no_thumbs: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    retry_failed: bool,
    #[serde(default)]
    rotated: bool,
    /// Backup check: also re-read the backup drive's files.
    #[serde(default)]
    deep: bool,
    /// Cleanup (of a backup, or of the copies a scan found): delete for good
    /// instead of moving into the trash.
    #[serde(default)]
    forever: bool,
    /// `install_addons`: the face models and/or the cat and dog models.
    #[serde(default)]
    faces: bool,
    #[serde(default)]
    pets: bool,
}

/// Run one command and return its result as JSON plus whether it was clean.
/// The same functions as the CLI.
fn run_command(req: &JobRequest, roots: Vec<PathBuf>) -> Result<(serde_json::Value, bool)> {
    let root = roots[0].clone();
    Ok(match req.kind.as_str() {
        "backup" => {
            let check = crate::backup::run(&crate::backup::Options {
                primary: roots[0].clone(),
                backup: roots.get(1).cloned().ok_or_else(|| anyhow::anyhow!("a backup check needs the original drive and the backup"))?,
                deep: req.deep,
                limit: 200,
            })?;
            (serde_json::to_value(&check)?, check.ok)
        }
        "backup_cleanup" => {
            let done = crate::backup::cleanup(&crate::backup::CleanupOptions {
                primary: roots[0].clone(),
                backup: roots.get(1).cloned().ok_or_else(|| anyhow::anyhow!("a backup cleanup needs the original drive and the backup"))?,
                forever: req.forever,
            })?;
            let clean = done.skipped.is_empty();
            (serde_json::to_value(&done)?, clean)
        }
        "scan_cleanup" => {
            let done = crate::arrivals::cleanup(&crate::arrivals::CleanupOptions { root, forever: req.forever })?;
            let clean = done.skipped.is_empty();
            (serde_json::to_value(&done)?, clean)
        }
        "scan" => {
            let stats = scan::run(&scan::Options {
                root,
                db: None,
                full_hash: !req.quick,
                thumbs: !req.no_thumbs,
                forget_missing: false,
            })?;
            let clean = stats.skipped.is_empty();
            (serde_json::to_value(&stats)?, clean)
        }
        "verify" => {
            let r = verify::run(&verify::Options { root, db: None, quick: req.quick, limit: req.limit })?;
            let clean = r.is_clean();
            (serde_json::to_value(&r)?, clean)
        }
        "forget_missing" => {
            let n = scan::forget_missing_records(&root)?;
            (serde_json::json!({ "forgotten": n }), true)
        }
        "recognize" | "recognize_pets" | "recognize_rotated" => {
            let stats = recognize::run(&recognize::Options {
                root,
                db: None,
                recognizer: None,
                limit: req.limit,
                retry_failed: req.retry_failed,
                rotated: req.rotated || req.kind == "recognize_rotated",
                pets: req.kind == "recognize_pets",
                timeouts: recognize::Timeouts::default(),
            })?;
            let clean = stats.errors.is_empty() && stats.pets.as_ref().is_none_or(|a| a.errors.is_empty());
            (serde_json::to_value(&stats)?, clean)
        }
        "install_addons" => {
            // `root` is the recognizer folder next to the program (see `start_job`).
            if !req.faces && !req.pets {
                anyhow::bail!("choose at least one add-on to install");
            }
            let found = recognize::install(&root, req.faces, req.pets)?;
            let clean = (found.faces || !req.faces) && (found.pets || !req.pets);
            (serde_json::to_value(&found)?, clean)
        }
        "faces_stats" => {
            let stats = faces::print_stats(&root, None)?;
            (serde_json::to_value(&stats)?, true)
        }
        other => anyhow::bail!("unknown command {other:?}"),
    })
}

/// The folders of a request: `roots` plus `root`, each an existing folder.
/// Installing add-ons works on no drive: its one folder is `recognizer/` next
/// to the program.
fn request_roots(req: &JobRequest) -> Result<Vec<PathBuf>, ApiError> {
    if req.kind == "install_addons" {
        return recognize::program_dir().map(|d| vec![d]).ok_or_else(|| ApiError::BadRequest("the recognizer folder is not next to the shoebox program".into()));
    }
    let mut all: Vec<&str> = req.roots.iter().map(String::as_str).collect();
    if !req.root.trim().is_empty() {
        all.push(&req.root);
    }
    if all.is_empty() {
        return Err(ApiError::BadRequest("enter at least one folder".into()));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for raw in all {
        let root = PathBuf::from(raw.trim())
            .canonicalize()
            .ok()
            .filter(|r| r.is_dir())
            .ok_or_else(|| ApiError::BadRequest(format!("{raw:?} is not a folder on this computer")))?;
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

fn short_name(root: &Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string())
}

/// Recognition may run while the photo app is open: `serve` opens the
/// databases shared, skips its own clustering and embedding while a
/// recognition job is running, and a moved or changed original is skipped by
/// the guard. Scan, verify and backup checks stay locked out.
fn runs_beside_app(kind: &str) -> bool {
    matches!(kind, "recognize" | "recognize_pets" | "recognize_rotated" | "faces_stats")
}

fn start_job(shared: &Arc<Shared>, req: JobRequest) -> Result<u64, ApiError> {
    if !matches!(req.kind.as_str(), "scan" | "forget_missing" | "verify" | "recognize" | "recognize_pets" | "recognize_rotated" | "faces_stats" | "backup" | "backup_cleanup" | "scan_cleanup" | "install_addons") {
        return Err(ApiError::BadRequest(format!("unknown command {:?}", req.kind)));
    }
    if !runs_beside_app(&req.kind) && shared.app.lock().unwrap().is_some() {
        return Err(ApiError::Conflict("stop the photo app first".into()));
    }
    let roots = request_roots(&req)?;
    if req.kind.starts_with("backup") && roots.len() != 2 {
        return Err(ApiError::BadRequest("a backup check compares two folders: the original drive first, then the backup".into()));
    }
    let id = {
        let mut next = shared.next_id.lock().unwrap();
        *next += 1;
        *next
    };
    {
        let mut job = shared.job.lock().unwrap();
        if job.running {
            return Err(ApiError::Conflict(format!("{} is still running", job.kind)));
        }
        *job = JobState {
            id,
            kind: req.kind.clone(),
            root: roots[0].display().to_string(),
            roots: roots.iter().map(|r| r.display().to_string()).collect(),
            running: true,
            started_at: now_secs(),
            ..JobState::default()
        };
    }
    report::clear_cancel();
    let shared = shared.clone();
    std::thread::Builder::new()
        .name("shoebox-job".into())
        .spawn(move || {
            let sink_state = shared.clone();
            let guard = report::install(Arc::new(move |event| {
                sink_state.job.lock().unwrap().apply(event);
            }));
            // A backup check is one run over the two folders; the others run once per folder.
            let groups: Vec<Vec<PathBuf>> =
                if req.kind.starts_with("backup") { vec![roots.clone()] } else { roots.iter().map(|r| vec![r.clone()]).collect() };
            let many = groups.len() > 1;
            for group in groups {
                let root = group[0].clone();
                if report::cancelled() {
                    break;
                }
                {
                    let mut job = shared.job.lock().unwrap();
                    job.current_root = Some(root.display().to_string());
                    job.progress = None;
                    job.prefix = if many { format!("{}: ", short_name(&root)) } else { String::new() };
                }
                if many {
                    crate::say!("== {} ==", root.display());
                }
                let outcome = run_command(&req, group);
                let mut job = shared.job.lock().unwrap();
                let entry = match outcome {
                    Ok((result, clean)) => RootResult { root: root.display().to_string(), ok: clean, result: Some(result), error: None },
                    Err(e) if e.is::<report::Cancelled>() || e.is::<recognize::Interrupted>() => {
                        job.cancelled = true;
                        RootResult { root: root.display().to_string(), ok: false, result: None, error: Some(format!("{e:#}")) }
                    }
                    Err(e) => RootResult { root: root.display().to_string(), ok: false, result: None, error: Some(format!("{e:#}")) },
                };
                job.results.push(entry);
            }
            drop(guard);
            if req.kind == "backup" && req.roots.len() == 2 {
                let complete = shared.job.lock().unwrap().results.first().is_some_and(|r| r.ok);
                let cancelled = report::cancelled();
                let ran = shared.job.lock().unwrap().results.first().is_some_and(|r| r.result.is_some());
                if ran && !cancelled {
                    mark_backup(&shared, &req.roots[0], &req.roots[1], complete);
                }
            }
            let was_cancelled = report::cancelled();
            report::clear_cancel();
            let mut job = shared.job.lock().unwrap();
            job.running = false;
            job.current_root = None;
            job.prefix.clear();
            job.finished_at = Some(now_secs());
            if was_cancelled {
                job.cancelled = true;
            }
            if job.results.len() == 1 {
                job.result = job.results[0].result.clone();
                job.error = job.results[0].error.clone();
            } else {
                let failed: Vec<String> =
                    job.results.iter().filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", short_name(Path::new(&r.root))))).collect();
                if !failed.is_empty() {
                    job.error = Some(failed.join("\n"));
                }
            }
            job.ok = Some(!job.cancelled && job.fail_count == 0 && job.results.iter().all(|r| r.ok));
        })
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(id)
}

/// "Cancel": like Ctrl-C on the command line. The running command finishes
/// the file it is on, keeps what it committed and stops.
async fn job_cancel(State(shared): State<Arc<Shared>>) -> Json<serde_json::Value> {
    let running = shared.job.lock().unwrap().running;
    if running {
        report::request_cancel();
        shared.job.lock().unwrap().apply(Event::Line { text: "Cancelling…".into() });
    }
    Json(serde_json::json!({ "cancelling": running }))
}

// ---------------------------------------------------------------- routes

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/drives", get(drives))
        .route("/api/addons", post(addons))
        .route("/api/job", get(job_state).post(job_start))
        .route("/api/app", get(app_state).post(app_start))
        .route("/api/app/stop", post(app_stop))
        .route("/api/job/cancel", post(job_cancel))
        .route("/api/config", get(config_get).post(config_set))
        .route("/api/reveal", post(reveal_files))
        .fallback(asset)
        .layer(middleware::from_fn(guard))
        .with_state(shared)
}

enum ApiError {
    BadRequest(String),
    Conflict(String),
    Forbidden(&'static str),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.to_string()),
            ApiError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Only this computer, by name or number (not a name that resolves here by
/// DNS rebinding), and changes only with the `X-Shoebox` header.
async fn guard(ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    let mut res = if !serve::is_local(peer.ip(), serve::host(req.headers())) {
        ApiError::Forbidden("the launcher only answers this computer").into_response()
    } else if !safe && !req.headers().contains_key(serve::WRITE_HEADER) {
        ApiError::Forbidden("missing X-Shoebox header").into_response()
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn job_start(State(shared): State<Arc<Shared>>, Json(req): Json<JobRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let shared = shared.clone();
    let id = tokio::task::spawn_blocking(move || start_job(&shared, req))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn job_state(State(shared): State<Arc<Shared>>) -> Json<serde_json::Value> {
    let job = shared.job.lock().unwrap();
    Json(serde_json::to_value(&*job).unwrap_or_default())
}

#[derive(Deserialize)]
struct AppRequest {
    #[serde(default)]
    root: String,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    port: Option<u16>,
}

/// "Start photo app": only now does `serve` start, with every chosen folder
/// as one library. A drive that is not plugged in is offline in the app.
async fn app_start(State(shared): State<Arc<Shared>>, Json(req): Json<AppRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let url = tokio::task::spawn_blocking(move || -> Result<String, ApiError> {
        {
            let job = shared.job.lock().unwrap();
            if job.running && !runs_beside_app(&job.kind) {
                return Err(ApiError::Conflict("a command is still running".into()));
            }
        }
        let jobless = JobRequest {
            kind: String::new(),
            root: req.root.clone(),
            roots: req.roots.clone(),
            quick: false,
            no_thumbs: false,
            limit: None,
            retry_failed: false,
            rotated: false,
            deep: false,
            forever: false,
            faces: false,
            pets: false,
        };
        let roots = request_roots(&jobless)?;
        let mut app = shared.app.lock().unwrap();
        if let Some(running) = app.as_ref() {
            if running.roots == roots {
                return Ok(running.url.clone());
            }
            return Err(ApiError::Conflict("the photo app is already running; stop it first".into()));
        }
        let server = serve::start(
            &serve::Options {
                root: roots[0].clone(),
                more_roots: roots[1..].to_vec(),
                db: None,
                port: req.port.unwrap_or(serve::DEFAULT_PORT),
                lan: false,
                pin: None,
                reveal: None,
                recognizer: None,
            },
            false,
        )
        .map_err(|e| ApiError::BadRequest(format!("{e:#}")))?;
        let url = format!("http://localhost:{}/", server.addr.port());
        if shared.open_browser {
            open_browser(&url);
        }
        *app = Some(AppRunning { roots, url: url.clone(), server });
        Ok(url)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(serde_json::json!({ "url": url })))
}

async fn app_state(State(shared): State<Arc<Shared>>) -> Json<serde_json::Value> {
    let app = shared.app.lock().unwrap();
    Json(match app.as_ref() {
        Some(a) => serde_json::json!({
            "running": true,
            "url": a.url,
            "roots": a.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
        }),
        None => serde_json::json!({ "running": false }),
    })
}

async fn app_stop(State(shared): State<Arc<Shared>>) -> Result<Json<serde_json::Value>, ApiError> {
    let app = shared.app.lock().unwrap().take();
    if let Some(app) = app {
        tokio::task::spawn_blocking(move || app.stop())
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?
            .map_err(ApiError::Internal)?;
    }
    Ok(Json(serde_json::json!({ "running": false })))
}

// ---------------------------------------------------------------- drives

#[derive(Serialize, Debug, PartialEq)]
pub struct Drive {
    pub path: String,
    pub name: String,
    /// Has a `.shoebox` folder: a library lives here.
    pub library: bool,
}

/// Where the binary sits inside `<drive>/.shoebox/bin/`, the drive itself:
/// the best suggestion when it was started from the drive.
pub fn drive_of_binary(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    let shoebox = bin.parent()?;
    if bin.file_name()? == "bin" && shoebox.file_name()? == crate::db::DIR {
        return shoebox.parent().map(Path::to_path_buf);
    }
    None
}

/// Mounted drives and the one the binary was started from.
pub fn detect_drives() -> Vec<Drive> {
    let mut found: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| drive_of_binary(&e)) {
        found.push(d);
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.push("/Volumes".into());
    } else if cfg!(target_os = "windows") {
        for letter in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", letter as char));
            if p.exists() {
                found.push(p);
            }
        }
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        dirs.extend(["/media".into(), format!("/media/{user}").into(), format!("/run/media/{user}").into(), "/mnt".into()]);
    }
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            // /media/<user> holds the drives; it is not one itself.
            if dirs.contains(&p) {
                continue;
            }
            // The boot volume shows up as a link to "/".
            if p.is_dir() && !std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                found.push(p);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .map(|p| Drive {
            name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            library: p.join(crate::db::DIR).is_dir(),
            path: p.display().to_string(),
        })
        .collect()
}

/// Which add-ons (faces, pets) can be used, for this computer and for each
/// drive in `roots`; and whether they can be installed from here.
#[derive(Deserialize, Default)]
struct AddonsRequest {
    #[serde(default)]
    roots: Vec<String>,
}

async fn addons(Json(req): Json<AddonsRequest>) -> Json<serde_json::Value> {
    let program = recognize::installed(None);
    let roots: Vec<serde_json::Value> = req
        .roots
        .iter()
        .map(|r| {
            let found = recognize::installed(Some(Path::new(r)));
            serde_json::json!({ "root": r, "faces": found.faces, "pets": found.pets })
        })
        .collect();
    let dir = recognize::program_dir();
    Json(serde_json::json!({
        "installable": dir.is_some(),
        "dir": dir,
        "runtime": program.runtime,
        "models": program.models,
        "faces_models": program.faces_models,
        "pets_models": program.pets_models,
        "faces": program.faces,
        "pets": program.pets,
        "roots": roots,
    }))
}

async fn drives() -> Json<Vec<Drive>> {
    Json(tokio::task::spawn_blocking(detect_drives).await.unwrap_or_default())
}

// ---------------------------------------------------------------- page

#[derive(rust_embed::RustEmbed)]
#[folder = "launcher-web/"]
struct Assets;

/// Translations and their loader, shared with the photo app (`i18n/`).
#[derive(rust_embed::RustEmbed)]
#[folder = "i18n/"]
#[prefix = "i18n/"]
struct I18n;

async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = Assets::get(path).or_else(|| I18n::get(path)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], file.data.into_owned()).into_response()
}
