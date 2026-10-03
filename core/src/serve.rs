//! `shoebox serve`: the web UI and its JSON API.
//!
//! Listens on localhost only, unless `--lan` is given; then other devices
//! (an iPad on the same network) can connect after entering a PIN, which is
//! printed at startup. Requests from this machine need no PIN, but only when
//! they name it as `localhost` or an IP address: a web page that points its
//! own domain at 127.0.0.1 (DNS rebinding) still has to log in.
//!
//! The server only reads originals: previews come from `thumbs.db` (missing
//! ones are rendered on first request, under the guard), and originals are
//! streamed as they are, with range requests for video seeking. The only
//! writes go to `.shoebox/` (thumbnails and perceptual hashes).

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use libheif_rs::LibHeif;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, oneshot};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::browse::{self, Snapshot};
use crate::classify::Kind;
use crate::db;
use crate::media;
use crate::thumbs::{self, Source};

pub const DEFAULT_PORT: u16 = 7878;
const SESSION_COOKIE: &str = "shoebox_session";
const SESSION_DAYS: u64 = 30;
/// Wrong PINs allowed per minute (from all clients together).
const LOGIN_ATTEMPTS_PER_MINUTE: usize = 5;
const IMMUTABLE: &str = "private, max-age=31536000, immutable";

pub struct Options {
    pub root: PathBuf,
    pub db: Option<PathBuf>,
    pub port: u16,
    /// Listen on all interfaces and require a PIN from other devices.
    pub lan: bool,
    /// PIN to use instead of a random one.
    pub pin: Option<String>,
}

/// A running server; dropping it does not stop it, `stop` does.
pub struct Server {
    pub addr: SocketAddr,
    pub pin: Option<String>,
    pub urls: Vec<String>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl Server {
    /// Stop accepting connections and wait for open requests to finish.
    pub fn stop(mut self) -> Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.wait()
    }

    /// Run until the server stops (Ctrl-C).
    pub fn wait(mut self) -> Result<()> {
        match self.thread.take().map(|t| t.join()) {
            Some(Ok(r)) => r,
            Some(Err(_)) => bail!("server thread panicked"),
            None => Ok(()),
        }
    }
}

/// `shoebox serve`: start, print where to connect, run until Ctrl-C.
pub fn run(opts: &Options) -> Result<()> {
    let server = start(opts, true)?;
    println!("shoebox {} is serving {}", env!("CARGO_PKG_VERSION"), opts.root.display());
    for url in &server.urls {
        println!("  {url}");
    }
    match &server.pin {
        Some(pin) => println!("PIN for other devices: {pin}"),
        None => println!("Only this computer can connect (use --lan for other devices)."),
    }
    println!("Press Ctrl-C to stop.");
    server.wait()
}

/// Bind and serve in a background thread. With `ctrl_c`, the server also
/// stops on Ctrl-C.
pub fn start(opts: &Options, ctrl_c: bool) -> Result<Server> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    let db_path = match &opts.db {
        Some(p) => std::path::absolute(p)?,
        None => db::default_path(&root),
    };
    if !db_path.is_file() {
        bail!("no index at {} (run `shoebox scan` first)", db_path.display());
    }
    let conn = db::open_shared(&db_path)?;
    thumbs::attach(&conn, &db_path)?;

    let pin = match (&opts.pin, opts.lan) {
        (Some(p), _) if p.trim().len() < 4 => bail!("the PIN needs at least 4 characters"),
        (Some(p), _) => Some(p.trim().to_string()),
        (None, true) => Some(random_pin()?),
        (None, false) => None,
    };
    let ip = if opts.lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let listener = std::net::TcpListener::bind((ip, opts.port))
        .with_context(|| format!("cannot listen on port {} (in use? try --port)", opts.port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let mut urls = vec![format!("http://localhost:{}/", addr.port())];
    if opts.lan {
        urls.extend(lan_address().map(|ip| format!("http://{ip}:{}/", addr.port())));
    }

    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let app = Arc::new(App {
        root,
        name,
        conn: Mutex::new(conn),
        snapshot: Mutex::new(None),
        ffmpeg: media::find_ffmpeg(),
        renders: Semaphore::new(workers),
        auth: Auth { pin: pin.clone(), sessions: Mutex::new(HashSet::new()), failures: Mutex::new(Vec::new()) },
    });
    let router = router(app);

    let (tx, rx) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new().name("shoebox-serve".into()).spawn(move || -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let stop = async move {
                if ctrl_c {
                    tokio::select! {
                        _ = rx => {}
                        _ = tokio::signal::ctrl_c() => println!("\nStopping."),
                    }
                } else {
                    let _ = rx.await;
                }
            };
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(stop)
                .await?;
            Ok(())
        })
    })?;
    Ok(Server { addr, pin, urls, shutdown: Some(tx), thread: Some(thread) })
}

struct App {
    root: PathBuf,
    name: String,
    /// One connection (library.db with thumbs.db attached); requests are
    /// short, and rendering happens outside the lock.
    conn: Mutex<Connection>,
    /// Timeline and folders, rebuilt when another process changed the index.
    snapshot: Mutex<Option<(i64, Arc<Snapshot>)>>,
    ffmpeg: Option<PathBuf>,
    /// Limits concurrent decodes to the number of cores.
    renders: Semaphore,
    auth: Auth,
}

impl App {
    fn snapshot(&self, conn: &Connection) -> Result<Arc<Snapshot>> {
        // Changes whenever another connection (a running scan) commits.
        let version: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        let mut cache = self.snapshot.lock().unwrap();
        if let Some((v, s)) = cache.as_ref()
            && *v == version
        {
            return Ok(s.clone());
        }
        let s = Arc::new(Snapshot::load(conn)?);
        *cache = Some((version, s.clone()));
        Ok(s)
    }

    /// The file behind an id, if it is present on the drive.
    fn source(&self, conn: &Connection, id: i64) -> Result<Option<Source>> {
        let row: Option<(String, String, i64, i64, Option<i64>, String)> = conn
            .query_row(
                "SELECT path, kind, size, mtime_ns, duration_ms, quick_hash FROM files
                 WHERE id = ?1 AND missing_since IS NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((rel, kind, size, mtime_ns, duration, quick_hash)) = row else { return Ok(None) };
        // Stored paths are relative and plain; refuse anything else outright.
        if !FsPath::new(&rel).components().all(|c| matches!(c, Component::Normal(_))) {
            return Ok(None);
        }
        let Some(kind) = Kind::parse(&kind) else { return Ok(None) };
        Ok(Some(Source {
            path: self.root.join(rel),
            kind,
            size: size as u64,
            mtime_ns,
            duration_ms: duration.map(|d| d as u64),
            quick_hash,
        }))
    }
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/info", get(info))
        .route("/api/folders", get(folders))
        .route("/api/tags", get(tags))
        .route("/api/timeline", get(timeline))
        .route("/api/files/{id}", get(file_details))
        .route("/api/files/{id}/thumb", get(thumb))
        .route("/api/files/{id}/view", get(view))
        .route("/api/files/{id}/original", get(original))
        .fallback(asset)
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

// ---------------------------------------------------------------- errors

enum ApiError {
    NotFound,
    Unauthorized,
    Forbidden(&'static str),
    TooManyRequests,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "PIN required".to_string()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.to_string()),
            ApiError::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many wrong PINs; wait a minute".into()),
            ApiError::Internal(e) => {
                eprintln!("error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// Run database work (or decoding) off the async threads.
async fn blocking<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&App) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || f(&app))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("worker failed: {e}")))?
}

// ---------------------------------------------------------------- auth

struct Auth {
    pin: Option<String>,
    sessions: Mutex<HashSet<String>>,
    failures: Mutex<Vec<Instant>>,
}

impl Auth {
    fn allows(&self, peer: IpAddr, headers: &HeaderMap) -> bool {
        is_local(peer, host(headers)) || self.has_session(headers)
    }

    fn has_session(&self, headers: &HeaderMap) -> bool {
        cookie(headers, SESSION_COOKIE).is_some_and(|t| self.sessions.lock().unwrap().contains(t))
    }

    /// Check a PIN; returns a new session token.
    fn login(&self, pin: &str) -> ApiResult<String> {
        let Some(expected) = &self.pin else {
            return Err(ApiError::Forbidden("other devices are not allowed; start shoebox serve with --lan"));
        };
        let mut failures = self.failures.lock().unwrap();
        failures.retain(|t| t.elapsed() < Duration::from_secs(60));
        if failures.len() >= LOGIN_ATTEMPTS_PER_MINUTE {
            return Err(ApiError::TooManyRequests);
        }
        if !constant_time_eq(pin.trim().as_bytes(), expected.as_bytes()) {
            failures.push(Instant::now());
            return Err(ApiError::Unauthorized);
        }
        let token = random_hex(32).map_err(ApiError::Internal)?;
        self.sessions.lock().unwrap().insert(token.clone());
        Ok(token)
    }
}

/// A request from this machine that addresses it by IP or as localhost.
fn is_local(peer: IpAddr, host: Option<&str>) -> bool {
    let peer_local = peer.is_loopback() || matches!(peer, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()));
    peer_local && host.is_some_and(host_is_literal)
}

/// `localhost` or an IP address, with or without a port.
fn host_is_literal(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok()
}

fn host(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|h| h.to_str().ok())
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn random_pin() -> Result<String> {
    let mut buf = [0u8; 8];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
    Ok(format!("{:06}", u64::from_le_bytes(buf) % 1_000_000))
}

/// The address other devices on the network reach this machine at. Connecting
/// a UDP socket sends nothing; it only picks the outgoing interface.
fn lan_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// Login check for the API, and security headers on every response.
async fn guard(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let open = !path.starts_with("/api/") || path == "/api/session" || path == "/api/login";
    let mut res = if open || app.auth.allows(peer.ip(), req.headers()) {
        next.run(req).await
    } else {
        ApiError::Unauthorized.into_response()
    };
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    res
}

#[derive(Serialize)]
struct SessionInfo {
    authenticated: bool,
    /// Whether a PIN can be entered at all (`--lan`).
    pin_enabled: bool,
}

async fn session(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Json<SessionInfo> {
    Json(SessionInfo { authenticated: app.auth.allows(peer.ip(), &headers), pin_enabled: app.auth.pin.is_some() })
}

#[derive(Deserialize)]
struct LoginRequest {
    pin: String,
}

async fn login(State(app): State<Arc<App>>, Json(body): Json<LoginRequest>) -> ApiResult<Response> {
    let token = app.auth.login(&body.pin)?;
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_DAYS * 24 * 3600
    );
    let mut res = Json(serde_json::json!({ "ok": true })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|e| ApiError::Internal(e.into()))?);
    Ok(res)
}

// ---------------------------------------------------------------- library

#[derive(Serialize)]
struct JobInfo {
    kind: String,
    state: String,
    started_at: i64,
    updated_at: i64,
    finished_at: Option<i64>,
    done: i64,
    total: Option<i64>,
}

#[derive(Serialize)]
struct Info {
    name: String,
    version: &'static str,
    photos: u64,
    videos: u64,
    missing: u64,
    thumbs_done: u64,
    thumbs_total: u64,
    ffmpeg: bool,
    /// Latest job of each kind.
    jobs: Vec<JobInfo>,
    /// A scan (or another job) is running right now.
    busy: bool,
    /// Changes when the index changes; the UI reloads the timeline then.
    index_version: i64,
}

async fn info(State(app): State<Arc<App>>) -> ApiResult<Json<Info>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let (photos, videos, missing): (i64, i64, i64) = conn.query_row(
            "SELECT count(*) FILTER (WHERE missing_since IS NULL AND kind NOT IN ('video', 'raw')),
                    count(*) FILTER (WHERE missing_since IS NULL AND kind = 'video'),
                    count(*) FILTER (WHERE missing_since IS NOT NULL)
             FROM files",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let (thumbs_total, thumbs_done): (i64, i64) = conn.query_row(
            "SELECT count(DISTINCT f.quick_hash), count(DISTINCT t.key)
             FROM files f LEFT JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE f.missing_since IS NULL AND f.kind != 'raw'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let jobs: Vec<JobInfo> = conn
            .prepare(
                "SELECT kind, state, started_at, updated_at, finished_at, done, total FROM jobs
                 WHERE id IN (SELECT max(id) FROM jobs GROUP BY kind) ORDER BY id DESC",
            )?
            .query_map([], |r| {
                Ok(JobInfo {
                    kind: r.get(0)?,
                    state: r.get(1)?,
                    started_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    finished_at: r.get(4)?,
                    done: r.get(5)?,
                    total: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        // A job whose process died stays 'running' until the next scan; only
        // count it as busy while it is still reporting progress.
        let busy = jobs.iter().any(|j| j.state == "running" && db::now() - j.updated_at < 120);
        let index_version = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        Ok(Json(Info {
            name: app.name.clone(),
            version: env!("CARGO_PKG_VERSION"),
            photos: photos as u64,
            videos: videos as u64,
            missing: missing as u64,
            thumbs_done: thumbs_done as u64,
            thumbs_total: thumbs_total as u64,
            ffmpeg: app.ffmpeg.is_some(),
            jobs,
            busy,
            index_version,
        }))
    })
    .await
}

async fn folders(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<browse::Folder>>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        Ok(Json(app.snapshot(&conn)?.folders.clone()))
    })
    .await
}

#[derive(Deserialize)]
struct TagQuery {
    q: Option<String>,
    limit: Option<usize>,
}

async fn tags(State(app): State<Arc<App>>, Query(q): Query<TagQuery>) -> ApiResult<Json<Vec<browse::Tag>>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let needle = q.q.as_deref().map(|s| crate::library::nfc(s.trim()).to_lowercase()).unwrap_or_default();
        let tags = browse::all_tags(&conn)?
            .into_iter()
            .filter(|t| t.name.to_lowercase().contains(&needle))
            .take(q.limit.unwrap_or(50))
            .collect();
        Ok(Json(tags))
    })
    .await
}

#[derive(Deserialize)]
struct TimelineQuery {
    folder: Option<i64>,
    tag: Option<i64>,
    q: Option<String>,
}

/// The timeline in columns, which keeps 100,000 entries at about 2 MB.
#[derive(Serialize)]
struct Timeline {
    count: usize,
    ids: Vec<i64>,
    /// One letter per item: j(peg), p(ng), h(eic), v(ideo).
    kinds: String,
    /// `YYYYMMDD` of the sort date.
    days: Vec<u32>,
    /// 8 characters per item, appended to thumbnail URLs for caching.
    versions: String,
    /// [still id, video id] of Live Photos.
    live: Vec<[i64; 2]>,
}

async fn timeline(State(app): State<Arc<App>>, Query(q): Query<TimelineQuery>) -> ApiResult<Json<Timeline>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let snapshot = app.snapshot(&conn)?;
        let query = browse::Query { folder: q.folder, tag: q.tag, text: q.q.filter(|s| !s.trim().is_empty()) };
        let items = snapshot.query(&conn, &query)?;
        drop(conn);
        let mut t = Timeline {
            count: items.len(),
            ids: Vec::with_capacity(items.len()),
            kinds: String::with_capacity(items.len()),
            days: Vec::with_capacity(items.len()),
            versions: String::with_capacity(items.len() * 8),
            live: Vec::new(),
        };
        for it in items {
            t.ids.push(it.id);
            t.kinds.push(match it.kind {
                Kind::Jpeg => 'j',
                Kind::Png => 'p',
                Kind::Heic => 'h',
                Kind::Video => 'v',
                Kind::Raw => 'r',
            });
            t.days.push(it.day());
            t.versions.push_str(&format!("{:0<8}", it.version));
            if let Some(v) = it.live {
                t.live.push([it.id, v]);
            }
        }
        Ok(Json(t))
    })
    .await
}

#[derive(Serialize)]
struct FileInfo {
    #[serde(flatten)]
    details: browse::Details,
    date_source: Option<browse::DateSource>,
    sort_date: Option<String>,
    live: Option<i64>,
}

async fn file_details(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<FileInfo>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let details = browse::details(&conn, id)?.ok_or(ApiError::NotFound)?;
        let snapshot = app.snapshot(&conn)?;
        let item = snapshot.items.iter().find(|it| it.id == id);
        Ok(Json(FileInfo {
            details,
            date_source: item.map(|it| it.date_source),
            sort_date: item.map(|it| it.sort.clone()),
            live: item.and_then(|it| it.live),
        }))
    })
    .await
}

fn jpeg(bytes: Vec<u8>, cache: &'static str) -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")), (header::CACHE_CONTROL, HeaderValue::from_static(cache))],
        bytes,
    )
        .into_response()
}

async fn thumb(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Response> {
    let (src, stored) = blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let src = app.source(&conn, id)?.ok_or(ApiError::NotFound)?;
        let stored = thumbs::load(&conn, &src.quick_hash)?;
        Ok((src, stored))
    })
    .await?;
    match stored {
        Some(Ok(bytes)) => return Ok(jpeg(bytes, IMMUTABLE)),
        Some(Err(_)) => return Err(ApiError::NotFound),
        None if src.kind == Kind::Raw => return Err(ApiError::NotFound),
        None => {}
    }

    // Not made yet (scan ran with --no-thumbs, or is still busy): make it now.
    let _permit = app.renders.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let made = blocking(&app, move |app| {
        // Another request may have made it while this one waited.
        if let Some(stored) = thumbs::load(&app.conn.lock().unwrap(), &src.quick_hash)? {
            return Ok(stored);
        }
        let result = thumbs::render(&LibHeif::new(), app.ffmpeg.as_deref(), &src);
        if !matches!(&result, Err(e) if thumbs::is_transient(e))
            && let Err(e) = thumbs::store(&app.conn.lock().unwrap(), &src.quick_hash, &result)
        {
            eprintln!("could not store thumbnail: {e:#}"); // still worth sending
        }
        Ok(result.map(|r| r.jpeg))
    })
    .await?;
    match made {
        Ok(bytes) => Ok(jpeg(bytes, IMMUTABLE)),
        Err(_) => Err(ApiError::NotFound),
    }
}

/// The full-screen image: the original where browsers can show it, a large
/// JPEG rendering for HEIC.
async fn view(State(app): State<Arc<App>>, Path(id): Path<i64>, req: Request) -> ApiResult<Response> {
    let src = blocking(&app, move |app| app.source(&app.conn.lock().unwrap(), id)?.ok_or(ApiError::NotFound)).await?;
    match src.kind {
        Kind::Jpeg | Kind::Png | Kind::Video => serve_file(&src.path, req, None).await,
        Kind::Raw => Err(ApiError::NotFound),
        Kind::Heic => {
            let _permit = app.renders.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
            let bytes = blocking(&app, move |_| {
                thumbs::render_view(&LibHeif::new(), &src).map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))
            })
            .await?;
            Ok(jpeg(bytes, IMMUTABLE))
        }
    }
}

#[derive(Deserialize)]
struct OriginalQuery {
    download: Option<u8>,
}

/// The original file, byte for byte, with range requests (video seeking).
async fn original(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Query(q): Query<OriginalQuery>,
    req: Request,
) -> ApiResult<Response> {
    let src = blocking(&app, move |app| app.source(&app.conn.lock().unwrap(), id)?.ok_or(ApiError::NotFound)).await?;
    let name = src.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let disposition = q.download.is_some_and(|d| d != 0).then(|| content_disposition(&name));
    serve_file(&src.path, req, disposition).await
}

async fn serve_file(path: &FsPath, req: Request, disposition: Option<String>) -> ApiResult<Response> {
    let res = ServeFile::new(path).oneshot(req).await.map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?;
    let mut res = res.map(Body::new);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    if let Some(d) = disposition.and_then(|d| HeaderValue::from_str(&d).ok()) {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, d);
    }
    Ok(res)
}

/// `attachment` with the name in RFC 5987 form (umlauts, spaces).
fn content_disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    let encoded: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"._-".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

// ---------------------------------------------------------------- assets

#[derive(rust_embed::Embed)]
#[folder = "web/"]
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
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data: blob:; media-src 'self'; style-src 'self' 'unsafe-inline'",
            ),
        ),
    ];
    (headers, file.data).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_hosts_count_as_local() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert!(is_local(lo, Some("localhost:7878")));
        assert!(is_local(lo, Some("127.0.0.1:7878")));
        assert!(is_local("::1".parse().unwrap(), Some("[::1]:7878")));
        assert!(!is_local(lo, Some("evil.example:7878")), "DNS rebinding");
        assert!(!is_local(lo, None));
        assert!(!is_local(lan, Some("192.168.1.5:7878")));
    }

    #[test]
    fn cookies_and_pins() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; shoebox_session=abc; b=2"));
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "shoebox"), None);
        assert!(constant_time_eq(b"1234", b"1234"));
        assert!(!constant_time_eq(b"1234", b"1235"));
        assert!(!constant_time_eq(b"1234", b"12345"));
        let pin = random_pin().unwrap();
        assert!(pin.len() == 6 && pin.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn wrong_pins_are_rate_limited() {
        let auth = Auth { pin: Some("4711".into()), sessions: Mutex::default(), failures: Mutex::default() };
        for _ in 0..LOGIN_ATTEMPTS_PER_MINUTE {
            assert!(matches!(auth.login("0000"), Err(ApiError::Unauthorized)));
        }
        assert!(matches!(auth.login("4711"), Err(ApiError::TooManyRequests)));
        auth.failures.lock().unwrap().clear();
        let token = auth.login(" 4711 ").ok().unwrap();
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(&format!("{SESSION_COOKIE}={token}")).unwrap());
        assert!(auth.has_session(&h));
    }

    #[test]
    fn download_names_are_encoded() {
        assert_eq!(
            content_disposition("Bild Ö.jpg"),
            "attachment; filename=\"Bild__.jpg\"; filename*=UTF-8''Bild%20%C3%96.jpg"
        );
    }
}
