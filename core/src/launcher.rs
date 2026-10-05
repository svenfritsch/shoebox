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
//! others only read, and every one of them reads originals under the guard.

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
    let launcher = start(&Options { port: DEFAULT_PORT, open_browser: true })?;
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

// ---------------------------------------------------------------- state

struct Shared {
    job: Mutex<JobState>,
    /// The photo app (`serve`) once the button started it.
    app: Mutex<Option<AppRunning>>,
    open_browser: bool,
    next_id: Mutex<u64>,
}

struct AppRunning {
    root: PathBuf,
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
    progress: Option<ProgressInfo>,
    lines: VecDeque<String>,
    ok_count: u64,
    fail_count: u64,
    /// Files that failed, in order (up to `MAX_FAILURES`).
    failures: Vec<FileResult>,
    /// The latest files that went through.
    recent_ok: VecDeque<FileResult>,
    /// The command's final `Stats` / `Report`, as the CLI's `--json` prints.
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
                let item = FileResult { path, ok, note };
                if ok {
                    self.ok_count += 1;
                    if self.recent_ok.len() >= MAX_RECENT_OK {
                        self.recent_ok.pop_front();
                    }
                    self.recent_ok.push_back(item);
                } else {
                    self.fail_count += 1;
                    if self.failures.len() < MAX_FAILURES {
                        self.failures.push(item);
                    }
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
    /// `scan`, `verify`, `recognize` or `faces_stats`.
    kind: String,
    root: String,
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
}

/// Run one command and return its result as JSON plus whether it was clean.
/// The same functions as the CLI.
fn run_command(req: &JobRequest, root: PathBuf) -> Result<(serde_json::Value, bool)> {
    Ok(match req.kind.as_str() {
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
        "recognize" => {
            let stats = recognize::run(&recognize::Options {
                root,
                db: None,
                recognizer: None,
                limit: req.limit,
                retry_failed: req.retry_failed,
                rotated: req.rotated,
                timeouts: recognize::Timeouts::default(),
            })?;
            let clean = stats.errors.is_empty();
            (serde_json::to_value(&stats)?, clean)
        }
        "faces_stats" => {
            let stats = faces::print_stats(&root, None)?;
            (serde_json::to_value(&stats)?, true)
        }
        other => anyhow::bail!("unknown command {other:?}"),
    })
}

fn start_job(shared: &Arc<Shared>, req: JobRequest) -> Result<u64, ApiError> {
    if !matches!(req.kind.as_str(), "scan" | "verify" | "recognize" | "faces_stats") {
        return Err(ApiError::BadRequest(format!("unknown command {:?}", req.kind)));
    }
    let root = PathBuf::from(req.root.trim());
    let root = root
        .canonicalize()
        .ok()
        .filter(|r| r.is_dir())
        .ok_or_else(|| ApiError::BadRequest(format!("{:?} is not a folder on this computer", req.root)))?;
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
            root: root.display().to_string(),
            running: true,
            started_at: now_secs(),
            ..JobState::default()
        };
    }
    let shared = shared.clone();
    std::thread::Builder::new()
        .name("shoebox-job".into())
        .spawn(move || {
            let sink_state = shared.clone();
            let guard = report::install(Arc::new(move |event| {
                sink_state.job.lock().unwrap().apply(event);
            }));
            let outcome = run_command(&req, root);
            drop(guard);
            let mut job = shared.job.lock().unwrap();
            job.running = false;
            job.finished_at = Some(now_secs());
            match outcome {
                Ok((result, clean)) => {
                    job.result = Some(result);
                    job.ok = Some(clean && job.fail_count == 0);
                }
                Err(e) => {
                    job.error = Some(format!("{e:#}"));
                    job.ok = Some(false);
                }
            }
        })
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(id)
}

// ---------------------------------------------------------------- routes

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/drives", get(drives))
        .route("/api/job", get(job_state).post(job_start))
        .route("/api/app", get(app_state).post(app_start))
        .route("/api/app/stop", post(app_stop))
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
    root: String,
    #[serde(default)]
    port: Option<u16>,
}

/// "Start photo app": only now does `serve` start.
async fn app_start(State(shared): State<Arc<Shared>>, Json(req): Json<AppRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let url = tokio::task::spawn_blocking(move || -> Result<String, ApiError> {
        if shared.job.lock().unwrap().running {
            return Err(ApiError::Conflict("a command is still running".into()));
        }
        let root = PathBuf::from(req.root.trim())
            .canonicalize()
            .ok()
            .filter(|r| r.is_dir())
            .ok_or_else(|| ApiError::BadRequest(format!("{:?} is not a folder on this computer", req.root)))?;
        let mut app = shared.app.lock().unwrap();
        if let Some(running) = app.as_ref() {
            if running.root == root {
                return Ok(running.url.clone());
            }
            return Err(ApiError::Conflict(format!("the photo app is already running for {}", running.root.display())));
        }
        let server = serve::start(
            &serve::Options {
                root: root.clone(),
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
        *app = Some(AppRunning { root, url: url.clone(), server });
        Ok(url)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(serde_json::json!({ "url": url })))
}

async fn app_state(State(shared): State<Arc<Shared>>) -> Json<serde_json::Value> {
    let app = shared.app.lock().unwrap();
    Json(match app.as_ref() {
        Some(a) => serde_json::json!({ "running": true, "url": a.url, "root": a.root.display().to_string() }),
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
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
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

async fn drives() -> Json<Vec<Drive>> {
    Json(tokio::task::spawn_blocking(detect_drives).await.unwrap_or_default())
}

// ---------------------------------------------------------------- page

#[derive(rust_embed::RustEmbed)]
#[folder = "launcher-web/"]
struct Assets;

async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = Assets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], file.data.into_owned()).into_response()
}
