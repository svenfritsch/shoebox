//! `shoebox serve`: the web UI and its JSON API.
//!
//! Listens on localhost only, unless `--lan` is given; then other devices
//! (an iPad on the same network) can connect after entering a PIN, which is
//! printed at startup. Requests from this machine need no PIN, but only when
//! they name it as `localhost` or an IP address: a web page that points its
//! own domain at 127.0.0.1 (DNS rebinding) still has to log in.
//!
//! Browsing only reads originals: previews come from `thumbs.db` (missing
//! ones are rendered on first request, under the guard), and originals are
//! streamed as they are, with range requests for video seeking.
//!
//! Originals change only through explicit actions (`organize.rs`): move,
//! rename a folder, trash/restore, turn a JPEG. They run one
//! at a time, never while a scan is running, and need an `X-Shoebox` header
//! (like every non-GET request), which a web page on another origin cannot
//! send without a CORS preflight that this server never grants.
//!
//! "Show in Finder / Explorer" (`POST /api/files/{id}/reveal`) asks this
//! computer to open its file manager. It only works for requests from this
//! machine, takes the path from the index (never from the request), and runs
//! the command without a shell (see `reveal.rs`).
//!
//! Own tags (`/api/tags/add`, `/api/tags/remove`, see `tags.rs`) change only
//! the index. After every change the server copies `library.db` to
//! `library.db.bak` and writes `userdata.json` (at most once a minute, and
//! when it stops).
//!
//! People, groups and face decisions (`/api/people`, `/api/groups`,
//! `/api/clusters`, `/api/faces/…`, see `people.rs`) are user data in the
//! index like own tags. Clusters and suggestions are recomputed in the
//! background (`clusters.rs`) after every such change, when the server
//! starts, and when `recognition.db` got new faces.
//!
//! Self-healing paths: when a file is not where the index says (moved in the
//! Finder while shoebox runs), the server runs the scan's index step in the
//! background, which finds it again by its hashes.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
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
use crate::clusters;
use crate::dates;
use crate::db;
use crate::duplicates;
use crate::library;
use crate::media;
use crate::multi;
use crate::organize;
use crate::people;
use crate::recognize;
use crate::reveal;
use crate::volume;
use crate::scan;
use crate::tags as own_tags;
use crate::thumbs::{self, Source};

mod faces_api;
mod people_api;

pub const DEFAULT_PORT: u16 = 7878;
const SESSION_COOKIE: &str = "shoebox_session";
const SESSION_DAYS: u64 = 30;
/// Wrong PINs allowed per minute (from all clients together).
const LOGIN_ATTEMPTS_PER_MINUTE: usize = 5;
const IMMUTABLE: &str = "private, max-age=31536000, immutable";
/// Required on every request that is not a GET (see the module docs).
pub(crate) const WRITE_HEADER: &str = "x-shoebox";
/// At most one background search for moved files in this time.
const HEAL_INTERVAL: Duration = Duration::from_secs(30);
/// At most one backup of the index and the user data in this time.
const BACKUP_INTERVAL: Duration = Duration::from_secs(60);
/// A job that has not reported progress for this long belongs to a process
/// that died.
const JOB_ALIVE_SECS: i64 = 120;
/// Changes that come in this soon after another one are clustered together.
const CLUSTER_DELAY: Duration = Duration::from_millis(300);

/// Opens a photo in the computer's file manager; tests swap in a recorder.
pub type RevealFn = Arc<dyn Fn(&FsPath) -> Result<()> + Send + Sync>;

pub struct Options {
    pub root: PathBuf,
    /// More libraries (other drives) opened alongside `root`. A drive that is
    /// not plugged in at start, or later, is shown as offline; the rest works.
    pub more_roots: Vec<PathBuf>,
    pub db: Option<PathBuf>,
    pub port: u16,
    /// Listen on all interfaces and require a PIN from other devices.
    pub lan: bool,
    /// PIN to use instead of a random one.
    pub pin: Option<String>,
    /// What "Show in Finder" runs; `None` uses the platform's own command.
    pub reveal: Option<RevealFn>,
    /// The recognizer for faces drawn by hand (else `$SHOEBOX_RECOGNIZER` or
    /// the installed one).
    pub recognizer: Option<PathBuf>,
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
    for r in &opts.more_roots {
        println!("  and {}", r.display());
    }
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
    let pin = match (&opts.pin, opts.lan) {
        (Some(p), _) if p.trim().len() < 4 => bail!("the PIN needs at least 4 characters"),
        (Some(p), _) => Some(p.trim().to_string()),
        (None, true) => Some(random_pin()?),
        (None, false) => None,
    };
    let shared = Arc::new(Shared {
        auth: Arc::new(Auth { pin: pin.clone(), sessions: Mutex::new(HashSet::new()), failures: Mutex::new(Vec::new()) }),
        reveal: opts.reveal.clone().unwrap_or_else(|| -> RevealFn { Arc::new(|p: &FsPath| reveal::reveal(p)) }),
        recognizer: opts.recognizer.clone(),
        ffmpeg: media::find_ffmpeg(),
        workers: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
    });

    // The first library must be there; the others may be unplugged.
    let first = Slot::new(&opts.root, opts.db.clone())?;
    first.open_now(&shared)?;
    let mut slots = vec![first];
    for root in &opts.more_roots {
        let slot = Slot::new(root, None)?;
        if slots.iter().any(|s| s.id == slot.id) {
            bail!("two libraries are called {:?}; the library name (the drive's name) must differ", slot.name);
        }
        if let Err(e) = slot.open_now(&shared) {
            eprintln!("{} is offline: {e:#}", slot.name);
        }
        slots.push(slot);
    }
    let hub = Arc::new(Hub { slots, shared, roles: Mutex::new(None) });

    let ip = if opts.lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let listener = std::net::TcpListener::bind((ip, opts.port))
        .with_context(|| format!("cannot listen on port {} (in use? try --port)", opts.port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let mut urls = vec![format!("http://localhost:{}/", addr.port())];
    if opts.lan {
        urls.extend(lan_address().map(|ip| format!("http://{ip}:{}/", addr.port())));
    }
    let router = hub_router(hub.clone());

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
            Ok::<(), anyhow::Error>(())
        })?;
        // Like a scan, leave a copy of the index after changing it.
        let mut result = Ok(());
        for slot in &hub.slots {
            if let Some((app, _)) = slot.live.lock().unwrap().app.take() {
                app.stopping.store(true, Ordering::SeqCst);
                if app.dirty.load(Ordering::SeqCst)
                    && let Err(e) = app.backup()
                {
                    result = Err(e);
                }
            }
        }
        result
    })?;
    Ok(Server { addr, pin, urls, shutdown: Some(tx), thread: Some(thread) })
}

/// What all libraries of one server share.
struct Shared {
    auth: Arc<Auth>,
    reveal: RevealFn,
    recognizer: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    workers: usize,
}

/// One library: open when its drive is there, offline when it is not.
struct Slot {
    id: String,
    name: String,
    root: PathBuf,
    db: Option<PathBuf>,
    live: Mutex<Live>,
}

#[derive(Default)]
struct Live {
    app: Option<(Arc<App>, Router)>,
    /// When the open library was last seen on its drive, or an open was last tried.
    checked: Option<Instant>,
    /// Why it is offline.
    error: Option<String>,
}

/// How long a look at the drive is trusted (one `stat` per request would do,
/// but thumbnails come by the hundreds).
const DRIVE_CHECK_INTERVAL: Duration = Duration::from_secs(2);

impl Slot {
    fn new(root: &FsPath, db: Option<PathBuf>) -> Result<Slot> {
        // The drive may be missing: the name comes from the path as given.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let db = db.map(std::path::absolute).transpose()?;
        Ok(Slot { id: library_id(&name), name, root, db, live: Mutex::new(Live::default()) })
    }

    fn db_path(&self) -> PathBuf {
        self.db.clone().unwrap_or_else(|| db::default_path(&self.root))
    }

    /// Open the library now, failing with the reason.
    fn open_now(&self, shared: &Arc<Shared>) -> Result<()> {
        let mut live = self.live.lock().unwrap();
        live.checked = Some(Instant::now());
        match App::open(&self.root, self.db_path(), shared) {
            Ok(app) => {
                let router = router(app.clone());
                live.app = Some((app, router));
                live.error = None;
                Ok(())
            }
            Err(e) => {
                live.error = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// The open library, if its drive is there: a vanished drive closes it
    /// (nothing else happens), a returning one opens it again.
    fn online(&self, shared: &Arc<Shared>) -> Option<(Arc<App>, Router)> {
        {
            let mut live = self.live.lock().unwrap();
            let fresh = live.checked.is_some_and(|t| t.elapsed() < DRIVE_CHECK_INTERVAL);
            if let Some((app, router)) = live.app.clone() {
                if fresh {
                    return Some((app, router));
                }
                if app.root.is_dir() && app.db_path.is_file() {
                    live.checked = Some(Instant::now());
                    return Some((app, router));
                }
                eprintln!("{} went offline", self.name);
                if let Some((app, _)) = live.app.take() {
                    app.stopping.store(true, Ordering::SeqCst);
                }
                live.error = Some("the drive is not connected".into());
                live.checked = Some(Instant::now());
                return None;
            }
            if fresh || !self.db_path().is_file() {
                return None;
            }
        }
        if self.open_now(shared).is_ok() {
            eprintln!("{} is back online", self.name);
        }
        self.live.lock().unwrap().app.clone()
    }

}

/// All libraries of a server, found by id in the routes.
struct Hub {
    slots: Vec<Slot>,
    shared: Arc<Shared>,
    /// What the drives hold in common, worked out at most every
    /// `ROLES_TTL` (it compares all hashes) and again after a role changed.
    roles: Mutex<Option<(Instant, Vec<multi::DriveRole>)>>,
}

const ROLES_TTL: Duration = Duration::from_secs(20);

impl App {
    /// Open one library (its index, thumbnails and faces) and start its
    /// background work.
    fn open(root: &FsPath, db_path: PathBuf, shared: &Arc<Shared>) -> Result<Arc<App>> {
        let root = root.canonicalize().with_context(|| format!("cannot open {}", root.display()))?;
        if !db_path.is_file() {
            bail!("no index at {} (run `shoebox scan` first)", db_path.display());
        }
        let conn = db::open_shared(&db_path)?;
        thumbs::attach(&conn, &db_path)?;
        recognize::attach(&conn, &db_path)?;

        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        // Older versions kept a crop of every face looked at, and some people
        // have no picture of their own yet.
        let (covers, pruned) = people_api::tidy(&conn);
        if covers > 0 || pruned > 0 {
            println!("{name}: tidied up, {covers} people got a picture, {pruned} face crops nobody needs were removed.");
        }
        let (heal_tx, heal_rx) = mpsc::channel();
        let (backup_tx, backup_rx) = mpsc::channel();
        let (clusters_tx, clusters_rx) = mpsc::channel();
        let (embed_tx, embed_rx) = mpsc::channel();
        let app = Arc::new(App {
            root,
            db_path,
            name,
            conn: Mutex::new(conn),
            snapshot: Mutex::new(None),
            duplicates: Mutex::new(None),
            generation: AtomicU64::new(0),
            writing: Mutex::new(()),
            dirty: AtomicBool::new(false),
            heal: Mutex::new(heal_tx),
            backup: Mutex::new(backup_tx),
            clusters: Mutex::new(clusters_tx),
            clusters_wanted: AtomicU64::new(0),
            clusters_done: AtomicU64::new(0),
            faces_seen: AtomicI64::new(-1),
            embed: Mutex::new(embed_tx),
            embedding: AtomicBool::new(false),
            recognizer: shared.recognizer.clone(),
            stopping: AtomicBool::new(false),
            ffmpeg: shared.ffmpeg.clone(),
            reveal: shared.reveal.clone(),
            renders: Semaphore::new(shared.workers),
            auth: shared.auth.clone(),
        });
        spawn_healer(Arc::downgrade(&app), heal_rx);
        spawn_backups(Arc::downgrade(&app), backup_rx);
        spawn_clusterer(Arc::downgrade(&app), clusters_rx);
        spawn_embedder(Arc::downgrade(&app), embed_rx);
        app.request_clusters();
        app.request_embed();
        Ok(app)
    }
}

struct App {
    root: PathBuf,
    db_path: PathBuf,
    name: String,
    /// One connection (library.db with thumbs.db attached); requests are
    /// short, and rendering happens outside the lock.
    conn: Mutex<Connection>,
    /// Timeline and folders, rebuilt when the index changed.
    snapshot: Mutex<Option<(IndexVersion, Arc<Snapshot>)>>,
    duplicates: Mutex<Option<(IndexVersion, Arc<Vec<duplicates::Group>>)>>,
    /// Counts this server's own changes; `PRAGMA data_version` only sees
    /// other connections' commits.
    generation: AtomicU64,
    /// Held by every change to the library and by the search for moved
    /// files, so they never overlap.
    writing: Mutex<()>,
    /// The index changed; back it up when the server stops.
    dirty: AtomicBool,
    /// Asks the background thread to look for moved files.
    heal: Mutex<mpsc::Sender<()>>,
    /// Asks the background thread for a backup of the index and user data.
    backup: Mutex<mpsc::Sender<()>>,
    /// Asks the background thread to recompute clusters and suggestions.
    clusters: Mutex<mpsc::Sender<u64>>,
    /// Counts those requests, and the requests covered by a finished run.
    clusters_wanted: AtomicU64,
    clusters_done: AtomicU64,
    /// `recognition.db`'s faces when they were last clustered (a number
    /// that changes when `recognize` adds or replaces faces).
    faces_seen: AtomicI64,
    /// Asks the background thread to embed faces drawn by hand.
    embed: Mutex<mpsc::Sender<()>>,
    /// It is embedding right now (the recognizer is running).
    embedding: AtomicBool,
    recognizer: Option<PathBuf>,
    /// The server is stopping: background work ends early.
    stopping: AtomicBool,
    ffmpeg: Option<PathBuf>,
    reveal: RevealFn,
    /// Limits concurrent decodes to the number of cores.
    renders: Semaphore,
    auth: Arc<Auth>,
}

/// Changes whenever another connection (a scan) commits, or this server
/// changes the library itself.
type IndexVersion = (i64, u64);

impl App {
    fn version(&self, conn: &Connection) -> Result<IndexVersion> {
        let data_version: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        Ok((data_version, self.generation.load(Ordering::SeqCst)))
    }

    fn snapshot(&self, conn: &Connection) -> Result<Arc<Snapshot>> {
        let version = self.version(conn)?;
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

    /// Ask for a search for moved files (at most one runs at a time).
    fn request_heal(&self) {
        let _ = self.heal.lock().unwrap().send(());
    }

    /// The search itself: the scan's index step on a connection of its own.
    /// Skipped while another process scans (it will find the moves itself).
    fn heal(&self) -> Result<()> {
        let _writing = self.writing.lock().unwrap();
        if jobs_running(&self.conn.lock().unwrap())? {
            return Ok(());
        }
        println!("Some files are not where the index says; looking for them…");
        let conn = db::open_existing(&self.db_path)?;
        let stats = scan::index_library(&conn, &self.root)?;
        println!(
            "  {} moved, {} added, {} changed, {} missing.",
            stats.moved, stats.added, stats.changed, stats.missing
        );
        db::backup(&conn, &self.db_path)?;
        Ok(())
    }

    /// Ask for the clusters and suggestions to be recomputed.
    fn request_clusters(&self) {
        let n = self.clusters_wanted.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.clusters.lock().unwrap().send(n);
    }

    /// The clustering itself, on a connection of its own. Skipped while
    /// `shoebox recognize` runs: it clusters when it is done.
    fn cluster(&self) -> Result<()> {
        let conn = db::open_existing(&self.db_path)?;
        recognize::attach(&conn, &self.db_path)?;
        if recognize::running(&conn)? || clusters::running(&conn)? {
            return Ok(());
        }
        let seen = faces_fingerprint(&conn)?;
        let stopping = || self.stopping.load(Ordering::SeqCst);
        match clusters::run(&conn, &stopping, &mut |_, _| {}) {
            Ok(s) => {
                self.faces_seen.store(seen, Ordering::SeqCst);
                if s.listed > 0 {
                    println!(
                        "Clusters: {} faces, {} without a decision in {} clusters ({:.1} s).",
                        s.faces, s.unnamed, s.clusters, s.seconds
                    );
                }
                Ok(())
            }
            Err(e) if e.is::<recognize::Interrupted>() => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Ask for the faces drawn by hand to be embedded.
    fn request_embed(&self) {
        let _ = self.embed.lock().unwrap().send(());
    }

    /// Embed the faces drawn by hand that wait for it, with the recognizer
    /// started for that and stopped again; then recompute the suggestions.
    /// Skipped while `shoebox recognize` runs (it embeds them at its end)
    /// and when no recognizer is installed.
    fn embed_drawn(&self) -> Result<()> {
        let conn = db::open_existing(&self.db_path)?;
        recognize::attach(&conn, &self.db_path)?;
        if recognize::running(&conn)? {
            return Ok(());
        }
        let model = clusters::current_model(&conn)?;
        let pets_model = clusters::current_model_of(&conn, crate::pets::Space::Pets)?;
        let pending = recognize::drawn_pending(&conn, model.as_deref(), pets_model.as_deref())?;
        if pending.total() == 0 {
            return Ok(());
        }
        let Some(cmd) = recognize::find_worker_for(&self.root, self.recognizer.as_deref(), pending.pets > 0) else {
            println!("{} faces and pets drawn by hand wait for the recognizer (not installed).", pending.total());
            return Ok(());
        };
        // The pet models are only loaded when a pet is waiting.
        let cmd = if pending.pets > 0 { cmd.with_pets() } else { cmd };
        self.embedding.store(true, Ordering::SeqCst);
        let result = (|| -> Result<recognize::DrawnStats> {
            let mut worker = recognize::Worker::start(cmd, recognize::Timeouts::default())?;
            let stopping = || self.stopping.load(Ordering::SeqCst);
            let result = recognize::embed_drawn(&conn, &self.root, &mut worker, false, &stopping);
            worker.stop();
            result
        })();
        self.embedding.store(false, Ordering::SeqCst);
        match result {
            Ok(s) => {
                println!("Faces and pets drawn by hand: {} embedded ({} faces without landmarks), {} failed.", s.embedded, s.plain, s.failed);
                if s.embedded > 0 {
                    self.request_clusters();
                }
                Ok(())
            }
            Err(e) if e.is::<recognize::Interrupted>() => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Copy the index to `library.db.bak` and write `userdata.json`.
    fn backup(&self) -> Result<()> {
        let _writing = self.writing.lock().unwrap();
        let conn = self.conn.lock().unwrap();
        db::backup(&conn, &self.db_path)?;
        own_tags::write_user_data(&conn, &self.db_path)?;
        Ok(())
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
        let path = self.root.join(rel);
        if path.symlink_metadata().is_err() {
            self.request_heal();
            return Ok(None);
        }
        Ok(Some(Source {
            path,
            kind,
            size: size as u64,
            mtime_ns,
            duration_ms: duration.map(|d| d as u64),
            quick_hash,
        }))
    }
}

/// Whether a scan (or its thumbnail or hash pass) is running right now.
fn jobs_running(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM jobs WHERE state = 'running' AND updated_at > ?1",
        [db::now() - JOB_ALIVE_SECS],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Runs searches for moved files as they are asked for: requests that come
/// in while one runs are covered by it, and searches are at least
/// `HEAL_INTERVAL` apart. Ends with the server.
fn spawn_healer(app: Weak<App>, requests: mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        while requests.recv().is_ok() {
            if let Some(wait) = last.and_then(|t| HEAL_INTERVAL.checked_sub(t.elapsed())) {
                std::thread::sleep(wait);
            }
            while requests.try_recv().is_ok() {}
            let Some(app) = app.upgrade() else { break };
            if let Err(e) = app.heal() {
                eprintln!("error while looking for moved files: {e:#}");
            }
            drop(app);
            last = Some(Instant::now());
            while requests.try_recv().is_ok() {}
        }
    });
}

/// A number that changes when `recognize` adds or replaces faces.
fn faces_fingerprint(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT count(*) * 1000003 + coalesce(max(id), 0) FROM recog.faces", [], |r| r.get(0))?)
}

/// Recomputes clusters and suggestions as they are asked for: requests that
/// come in while one runs (or within `CLUSTER_DELAY`) are covered by the
/// next run. Ends with the server.
fn spawn_clusterer(app: Weak<App>, requests: mpsc::Receiver<u64>) {
    std::thread::spawn(move || {
        while let Ok(mut wanted) = requests.recv() {
            std::thread::sleep(CLUSTER_DELAY);
            while let Ok(n) = requests.try_recv() {
                wanted = wanted.max(n);
            }
            let Some(app) = app.upgrade() else { break };
            if app.stopping.load(Ordering::SeqCst) {
                break;
            }
            if let Err(e) = app.cluster() {
                eprintln!("could not group the faces: {e:#}");
            }
            app.clusters_done.fetch_max(wanted, Ordering::SeqCst);
        }
    });
}

/// Embeds faces drawn by hand as they are asked for (requests that come in
/// while it runs are covered by the next run). Ends with the server.
fn spawn_embedder(app: Weak<App>, requests: mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        while requests.recv().is_ok() {
            while requests.try_recv().is_ok() {}
            let Some(app) = app.upgrade() else { break };
            if app.stopping.load(Ordering::SeqCst) {
                break;
            }
            if let Err(e) = app.embed_drawn() {
                eprintln!("could not embed the faces drawn by hand: {e:#}");
            }
        }
    });
}

/// Backs up the index and the user data after changes: right away after the
/// first change, then at most every `BACKUP_INTERVAL` (changes in between are
/// covered by the next one). Ends with the server, which backs up once more
/// when it stops.
fn spawn_backups(app: Weak<App>, requests: mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        while requests.recv().is_ok() {
            if let Some(wait) = last.and_then(|t| BACKUP_INTERVAL.checked_sub(t.elapsed())) {
                std::thread::sleep(wait);
            }
            while requests.try_recv().is_ok() {}
            let Some(app) = app.upgrade() else { break };
            if let Err(e) = app.backup() {
                eprintln!("could not back up the index: {e:#}");
            }
            drop(app);
            last = Some(Instant::now());
        }
    });
}

/// A library's id in the routes: eight hex digits of the NFC-normalised name
/// of its folder (the drive's name), so it is the same in every session and
/// on every machine. File ids are per database, so a file is only known by
/// (library id, file id).
pub fn library_id(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfc: String = name.nfc().collect();
    blake3::hash(nfc.as_bytes()).to_hex()[..8].to_string()
}

/// Marks a request that came in through `/api/lib/{id}/…`.
#[derive(Clone)]
#[allow(dead_code)] // the id picks the library once several are open
struct LibraryScope(String);

/// The routes that belong to no library.
fn is_global(path: &str) -> bool {
    matches!(path, "/api/session" | "/api/login" | "/api/libraries")
}

/// The server's own routes: the global ones, and `/api/lib/{id}/…`, which
/// goes to the library with that id (rewritten to `/api/…` for the handlers
/// and marked with its `LibraryScope`). An unknown id is a 404, a drive that
/// is not there a 503 "offline"; the other libraries keep working.
fn hub_router(hub: Arc<Hub>) -> Router {
    let auth = hub.shared.auth.clone();
    Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/libraries", get(libraries))
        // Over all drives (see `multi.rs`).
        .route("/api/all/people", get(all_people))
        .route("/api/all/timeline", get(all_timeline))
        .route("/api/all/tags", get(all_tags))
        .route("/api/all/duplicates", get(all_duplicates))
        .route("/api/all/drives", get(all_drives))
        .route("/api/all/role", post(all_set_role))
        .route("/api/all/backups", get(all_backups))
        .route("/api/all/reveal", post(all_reveal))
        // Not a route with parameters: those would leak into the `Path`
        // extractors of the library's own routes.
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(auth, guard))
        .with_state(hub)
}

async fn fallback(State(hub): State<Arc<Hub>>, req: Request) -> Response {
    if req.uri().path().starts_with("/api/lib/") {
        return forward(hub, req).await;
    }
    asset(req.uri().clone()).await
}

async fn forward(hub: Arc<Hub>, mut req: Request) -> Response {
    let id = req.uri().path().strip_prefix("/api/lib/").and_then(|r| r.split_once('/')).map(|(id, _)| id.to_string());
    let Some(id) = id else { return ApiError::NotFound.into_response() };
    let Some(index) = hub.slots.iter().position(|s| s.id == id) else {
        return ApiError::NotFound.into_response();
    };
    let shared = hub.shared.clone();
    let hub2 = hub.clone();
    let online = tokio::task::spawn_blocking(move || hub2.slots[index].online(&shared)).await.ok().flatten();
    let Some((_, router)) = online else {
        let slot = &hub.slots[index];
        let reason = slot.live.lock().unwrap().error.clone().unwrap_or_default();
        return ApiError::Offline { library: slot.name.clone(), reason }.into_response();
    };
    // Rewritten from the raw path, so percent-encoding stays as sent.
    let tail = req.uri().path().strip_prefix("/api/lib/").and_then(|r| r.split_once('/')).map(|(_, t)| t.to_string()).unwrap_or_default();
    let new = match req.uri().query() {
        Some(q) => format!("/api/{tail}?{q}"),
        None => format!("/api/{tail}"),
    };
    let Ok(uri) = new.parse::<Uri>() else { return ApiError::NotFound.into_response() };
    *req.uri_mut() = uri;
    req.extensions_mut().insert(LibraryScope(id));
    router.oneshot(req).await.into_response()
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/folders", get(folders))
        .route("/api/tags", get(tags))
        .route("/api/timeline", get(timeline))
        .route("/api/files/{id}", get(file_details))
        .route("/api/files/{id}/thumb", get(thumb))
        .route("/api/files/{id}/view", get(view))
        .route("/api/files/{id}/original", get(original))
        .route("/api/files/{id}/reveal", post(reveal_file))
        .route("/api/files/{id}/rotate", post(rotate_file))
        .route("/api/move", post(move_files))
        .route("/api/folders/{id}/rename", post(rename_folder))
        .route("/api/duplicates", get(duplicates_list))
        .route("/api/duplicates/decide", post(duplicates_decide))
        .route("/api/duplicates/remove", post(duplicates_remove))
        .route("/api/duplicates/same-folder", get(duplicates_same_folder).post(duplicates_remove_same_folder))
        .route("/api/event-pattern", get(event_pattern_get).post(event_pattern_set))
        .route("/api/allow-trash", get(allow_trash_get).post(allow_trash_set))
        .route("/api/duplicates/copy-folders", get(duplicates_copy_folders).post(duplicates_set_copy_folders))
        .route("/api/duplicates/lower-quality", get(duplicates_lower_quality).post(duplicates_remove_lower_quality))
        .route("/api/trash", get(trash_list).post(trash_files))
        .route("/api/trash/{id}/thumb", get(trash_thumb))
        .route("/api/trash/{batch}/restore", post(trash_restore))
        .route("/api/trash/empty", post(trash_empty))
        .route("/api/rescan", post(rescan))
        .route("/api/tags/add", post(tags_add))
        .route("/api/tags/remove", post(tags_remove))
        .route("/api/tags/selection", post(tags_selection))
        .route("/api/favorites", post(favorites_set))
        .route("/api/screenshots", post(screenshots_set))
        .route("/api/geo/points", get(geo_points))
        .route("/api/files/{id}/position", post(position_set))
        .route("/api/files/dates", post(dates_set))
        .route("/api/files/dates/check", post(dates_check))
        .route("/api/dates", get(dates_suggest))
        .route("/api/places", get(places_list).post(places_create))
        .route("/api/places/{id}/rename", post(places_rename))
        .route("/api/places/{id}/area", post(places_redraw))
        .route("/api/places/{id}/delete", post(places_delete))
        .route("/api/faces", get(faces_api::list))
        .route("/api/faces/stats", get(faces_api::stats))
        .route("/api/faces/{id}/crop", get(faces_api::crop))
        .route("/api/faces/{id}/similar", get(faces_api::similar))
        .route("/api/faces/confirm", post(people_api::confirm))
        .route("/api/faces/reject", post(people_api::reject))
        .route("/api/faces/assign", post(people_api::assign))
        .route("/api/faces/ignore", post(people_api::ignore))
        .route("/api/faces/not-face", post(people_api::not_face))
        .route("/api/faces/species", post(people_api::species))
        .route("/api/faces/undo", post(people_api::undo))
        .route("/api/faces/manual", post(people_api::manual))
        .route("/api/faces/unreject", post(people_api::unreject))
        .route("/api/faces/manual/{id}/crop", get(faces_api::manual_crop))
        .route("/api/people", get(people_api::list).post(people_api::create))
        .route("/api/people/{id}", get(people_api::get_person))
        .route("/api/people/{id}/faces", get(people_api::person_faces))
        .route("/api/people/{id}/rename", post(people_api::rename))
        .route("/api/people/{id}/merge", post(people_api::merge))
        .route("/api/people/{id}/delete", post(people_api::delete))
        .route("/api/people/{id}/hide", post(people_api::hide))
        .route("/api/people/{id}/group", post(people_api::set_group))
        .route("/api/people/{id}/cover", post(people_api::set_cover))
        .route("/api/people/search", get(people_api::search))
        .route("/api/pets/search", get(people_api::pets_search))
        .route("/api/groups", get(people_api::groups).post(people_api::create_group))
        .route("/api/groups/reorder", post(people_api::reorder_groups))
        .route("/api/groups/{id}/rename", post(people_api::rename_group))
        .route("/api/groups/{id}/delete", post(people_api::delete_group))
        .route("/api/clusters", get(people_api::clusters))
        .route("/api/clusters/{id}/faces", get(people_api::cluster_faces))
        .route("/api/clusters/{id}/name", post(people_api::name_cluster))
        .route("/api/clusters/{id}/ignore", post(people_api::ignore_cluster))
        .route("/api/clusters/{id}/not-face", post(people_api::not_face_cluster))
        .fallback(asset)
        .layer(middleware::from_fn_with_state(app.auth.clone(), guard))
        .with_state(app)
}

// ---------------------------------------------------------------- errors

enum ApiError {
    NotFound,
    /// The request cannot be done as asked (bad name, name taken, …).
    BadRequest(String),
    /// Not now: a scan is running.
    Conflict(String),
    Unauthorized,
    Forbidden(&'static str),
    TooManyRequests,
    /// The library's drive is not connected.
    Offline { library: String, reason: String },
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        if e.is::<crate::geo::NoSuchPlace>() {
            return ApiError::BadRequest(format!("{e}"));
        }
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
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "PIN required".to_string()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.to_string()),
            ApiError::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many wrong PINs; wait a minute".into()),
            ApiError::Offline { library, reason } => {
                let note = if reason.is_empty() { String::new() } else { format!(" ({reason})") };
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": format!("{library} is offline{note}"), "offline": true, "library": library })),
                )
                    .into_response();
            }
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
pub(crate) fn is_local(peer: IpAddr, host: Option<&str>) -> bool {
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

pub(crate) fn host(headers: &HeaderMap) -> Option<&str> {
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

/// Login check for the API, the write header, and security headers on
/// every response.
async fn guard(State(auth): State<Arc<Auth>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let open = !path.starts_with("/api/") || is_global(path);
    // Library routes only exist as /api/lib/{id}/…; `forward` rewrites them
    // and marks the request.
    let unscoped = path.starts_with("/api/")
        && !is_global(path)
        && !path.starts_with("/api/lib/")
        && !path.starts_with("/api/all/")
        && req.extensions().get::<LibraryScope>().is_none();
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    let mut res = if unscoped {
        ApiError::NotFound.into_response()
    } else if !safe && !req.headers().contains_key(WRITE_HEADER) {
        ApiError::Forbidden("missing X-Shoebox header").into_response()
    } else if open || auth.allows(peer.ip(), req.headers()) {
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

async fn session(State(hub): State<Arc<Hub>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Json<SessionInfo> {
    let auth = &hub.shared.auth;
    Json(SessionInfo { authenticated: auth.allows(peer.ip(), &headers), pin_enabled: auth.pin.is_some() })
}

#[derive(Serialize)]
struct LibraryInfo {
    id: String,
    name: String,
    /// False for a drive that is unplugged (with several libraries).
    online: bool,
    /// Why a library is offline.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// The libraries this server has, for the UI to build its routes from; a
/// drive that is gone is listed as offline (this also notices a drive that
/// came back).
async fn libraries(State(hub): State<Arc<Hub>>) -> Json<Vec<LibraryInfo>> {
    let list = tokio::task::spawn_blocking(move || {
        hub.slots
            .iter()
            .map(|s| {
                let online = s.online(&hub.shared).is_some();
                let reason = s.live.lock().unwrap().error.clone().filter(|_| !online);
                LibraryInfo { id: s.id.clone(), name: s.name.clone(), online, reason }
            })
            .collect()
    })
    .await
    .unwrap_or_default();
    Json(list)
}

// ---------------------------------------------------------------- all drives

/// The libraries that are there right now.
async fn online_apps(hub: &Arc<Hub>) -> Vec<Arc<App>> {
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || hub.slots.iter().filter_map(|s| s.online(&hub.shared)).map(|(app, _)| app).collect())
        .await
        .unwrap_or_default()
}

fn drives_of(apps: &[Arc<App>]) -> ApiResult<Vec<multi::Drive>> {
    apps.iter()
        .map(|app| {
            let conn = app.conn.lock().unwrap();
            let (role, backup_of) = (multi::role(&conn)?, multi::backup_of(&conn)?);
            Ok(multi::Drive { id: library_id(&app.name), name: app.name.clone(), db: app.db_path.clone(), role, backup_of })
        })
        .collect()
}

#[derive(Serialize)]
struct AllPeople {
    people: Vec<multi::MergedPerson>,
    /// Drives that are not connected: their people are missing from the list.
    offline: Vec<String>,
}

/// People over all drives: the same name is the same person.
async fn all_people(State(hub): State<Arc<Hub>>) -> ApiResult<Json<AllPeople>> {
    let apps = online_apps(&hub).await;
    let offline = hub.slots.iter().filter(|s| !apps.iter().any(|a| a.name == s.name)).map(|s| s.name.clone()).collect();
    let people = tokio::task::spawn_blocking(move || -> ApiResult<Vec<multi::MergedPerson>> {
        let mut per_drive = Vec::new();
        for app in &apps {
            let conn = app.conn.lock().unwrap();
            per_drive.push((library_id(&app.name), app.name.clone(), people::people(&conn, true)?, people::groups(&conn)?));
        }
        Ok(multi::merge_people(&per_drive))
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(AllPeople { people, offline }))
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

/// The drives that are there, with their roles (cached for a few seconds).
struct Overview {
    apps: Vec<Arc<App>>,
    drives: Vec<multi::Drive>,
    roles: Vec<multi::DriveRole>,
}

async fn overview(hub: &Arc<Hub>) -> ApiResult<Overview> {
    let apps = online_apps(hub).await;
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || -> ApiResult<Overview> {
        let drives = drives_of(&apps)?;
        let mut cache = hub.roles.lock().unwrap();
        let fresh = cache.as_ref().filter(|(t, r)| t.elapsed() < ROLES_TTL && r.len() == drives.len());
        let roles = match fresh {
            // The same drives, by id, as when it was worked out.
            Some((_, r)) if r.iter().zip(&drives).all(|(r, d)| r.library == d.id && r.role == d.role) => r.clone(),
            _ => {
                let r = multi::roles(&drives)?;
                *cache = Some((Instant::now(), r.clone()));
                r
            }
        };
        Ok(Overview { apps, drives, roles })
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?
}

/// Contents on more than one drive, leaving out backups (and drives that
/// look like one until the user decides).
async fn all_duplicates(State(hub): State<Arc<Hub>>, Query(q): Query<LimitQuery>) -> ApiResult<Json<multi::CrossDuplicates>> {
    let o = overview(&hub).await?;
    let limit = q.limit.unwrap_or(500).min(5000);
    let result = tokio::task::spawn_blocking(move || {
        let mut result = multi::cross_duplicates(&o.drives, &o.roles, limit)?;
        // The disk name tells two folders of the same name apart.
        let mut volumes: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
        for g in &mut result.groups {
            for f in &mut g.files {
                let v = volumes.entry(f.library.clone()).or_insert_with(|| {
                    o.apps.iter().find(|a| library_id(&a.name) == f.library).and_then(|a| volume::placement(&a.root)).map(|p| p.volume)
                });
                f.volume = v.clone();
            }
        }
        Ok::<_, anyhow::Error>(result)
    })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(result))
}

#[derive(Serialize)]
struct DriveState {
    library: String,
    name: String,
    online: bool,
    role: multi::Role,
    suggested_backup_of: Option<String>,
    /// Takes part in the common timeline and the duplicates across drives.
    shown_in_all: bool,
    /// The disk the library folder is on and the folder on it (see `volume.rs`).
    volume: Option<String>,
    folder: Option<String>,
    #[serde(flatten)]
    facts: multi::DriveFacts,
}

/// Every drive with its role and, where undecided, what its contents suggest.
async fn all_drives(State(hub): State<Arc<Hub>>) -> ApiResult<Json<Vec<DriveState>>> {
    let o = overview(&hub).await?;
    let list = tokio::task::spawn_blocking(move || -> ApiResult<Vec<DriveState>> {
        let (eligible, _) = multi::eligibility(&o.drives, &o.roles);
        hub.slots
            .iter()
            .map(|s| {
                let Some(i) = o.roles.iter().position(|r| r.library == s.id) else {
                    return Ok(DriveState {
                        library: s.id.clone(),
                        name: s.name.clone(),
                        online: false,
                        role: multi::Role::Unknown,
                        suggested_backup_of: None,
                        shown_in_all: false,
                        volume: None,
                        folder: None,
                        facts: multi::DriveFacts::default(),
                    });
                };
                let app = o.apps.iter().find(|a| library_id(&a.name) == s.id).ok_or(ApiError::NotFound)?;
                let place = volume::placement(&app.root);
                let facts = multi::drive_facts(&app.conn.lock().unwrap())?;
                Ok(DriveState {
                    library: s.id.clone(),
                    name: s.name.clone(),
                    online: true,
                    role: o.roles[i].role,
                    suggested_backup_of: o.roles[i].suggested_backup_of.clone(),
                    shown_in_all: eligible.contains(&i),
                    volume: place.as_ref().map(|p| p.volume.clone()),
                    folder: place.map(|p| p.folder),
                    facts,
                })
            })
            .collect()
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(list))
}

/// File ids are per database; in the common timeline an item's id is
/// `drive * ID_SPAN + id`, the drive being its position in `libs`.
const ID_SPAN: i64 = 1 << 40;

#[derive(Serialize)]
struct AllTimeline {
    #[serde(flatten)]
    timeline: Timeline,
    /// The drives in the list, in the order the ids refer to.
    libs: Vec<LibraryRef>,
    /// Drives that are left out (backups) or offline.
    left_out: Vec<multi::Excluded>,
}

#[derive(Serialize)]
struct LibraryRef {
    id: String,
    name: String,
    /// The disk the drive is on, as the drive cards show it.
    volume: Option<String>,
}

/// One timeline over the drives that are there and not backups. Filters name
/// things instead of numbering them (`tag=Winter`, `person=Anna`, `q=…`):
/// ids differ from drive to drive, names are what the drives share. A drive
/// without one of the tags or people has nothing that matches all of them.
async fn all_timeline(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<AllTimeline>> {
    let o = overview(&hub).await?;
    let (eligible, mut left_out) = multi::eligibility(&o.drives, &o.roles);
    for s in &hub.slots {
        if !o.drives.iter().any(|d| d.id == s.id) {
            left_out.push(multi::Excluded { library: s.id.clone(), name: s.name.clone(), reason: "offline".into() });
        }
    }
    let apps: Vec<Arc<App>> = eligible.iter().map(|&i| o.apps[i].clone()).collect();
    let tag_names: Vec<String> = pairs.iter().filter(|(k, v)| k == "tag" && !v.is_empty()).map(|(_, v)| v.clone()).collect();
    let person_names: Vec<String> = pairs.iter().filter(|(k, v)| k == "person" && !v.is_empty()).map(|(_, v)| v.clone()).collect();
    let text = param(&pairs, "q").map(str::to_string).filter(|s| !s.trim().is_empty());
    let types = types_of(&pairs)?;
    let pet_terms = pets_of(&pairs)?;
    let fav = fav_of(&pairs);
    let dates = dates_of(&pairs)?;
    let needs_date = param(&pairs, "nodate").is_some_and(|v| !v.is_empty() && v != "0");
    let libs: Vec<LibraryRef> = apps.iter().map(|a| LibraryRef { id: library_id(&a.name), name: a.name.clone(), volume: volume::placement(&a.root).map(|p| p.volume) }).collect();

    let timeline = tokio::task::spawn_blocking(move || -> ApiResult<Timeline> {
        struct Row {
            sort: String,
            path: String,
            gid: i64,
            kind: char,
            day: u32,
            version: String,
            live: Option<i64>,
            fav: bool,
            est: Option<char>,
        }
        let mut rows: Vec<Row> = Vec::new();
        for (d, app) in apps.iter().enumerate() {
            let conn = app.conn.lock().unwrap();
            let mut tags = Vec::new();
            for name in &tag_names {
                match db::find_tag(&conn, name)? {
                    Some(id) => tags.push(id),
                    None => break,
                }
            }
            let known_people = if person_names.is_empty() { Vec::new() } else { people::people(&conn, true)? };
            let people_ids: Vec<i64> = person_names
                .iter()
                .filter_map(|n| {
                    let key = db::tag_fold(n);
                    known_people.iter().find(|p| db::tag_fold(&p.name) == key).map(|p| p.id)
                })
                .collect();
            if tags.len() != tag_names.len() || people_ids.len() != person_names.len() {
                continue;
            }
            let snapshot = app.snapshot(&conn)?;
            let query = browse::Query {
                folder: None,
                tags,
                text: text.clone(),
                people: people_ids,
                pets: pet_terms.clone(),
                types: types.clone(),
                fav,
                dates: dates.clone(),
                needs_date,
                ..Default::default()
            };
            let hearts = own_tags::favorite_ids(&conn)?;
            for it in snapshot.query(&conn, &query)? {
                rows.push(Row {
                    fav: hearts.contains(&it.id),
                    sort: it.sort.clone(),
                    path: it.path_lower.clone(),
                    gid: d as i64 * ID_SPAN + it.id,
                    kind: match it.kind {
                        Kind::Jpeg => 'j',
                        Kind::Png => 'p',
                        Kind::Heic => 'h',
                        Kind::Video => 'v',
                        Kind::Raw => 'r',
                    },
                    day: it.day(),
                    version: it.version.clone(),
                    live: it.live.map(|v| d as i64 * ID_SPAN + v),
                    est: estimate_code(it.date_source),
                });
            }
        }
        rows.sort_by(|a, b| b.sort.cmp(&a.sort).then_with(|| a.path.cmp(&b.path)).then_with(|| a.gid.cmp(&b.gid)));
        let mut t = Timeline {
            count: rows.len(),
            ids: Vec::with_capacity(rows.len()),
            kinds: String::with_capacity(rows.len()),
            days: Vec::with_capacity(rows.len()),
            versions: String::with_capacity(rows.len() * 8),
            live: Vec::new(),
            tags: Vec::new(),
            people: Vec::new(),
            favs: Vec::new(),
            est: Vec::new(),
        };
        for r in rows {
            if r.fav {
                t.favs.push(r.gid);
            }
            if let Some(c) = r.est {
                t.est.push((r.gid, c));
            }
            t.ids.push(r.gid);
            t.kinds.push(r.kind);
            t.days.push(r.day);
            t.versions.push_str(&format!("{:0<8}", r.version));
            if let Some(v) = r.live {
                t.live.push([r.gid, v]);
            }
        }
        Ok(t)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(AllTimeline { timeline, libs, left_out }))
}

/// Tags over the drives that take part, by name (spellings fold together),
/// most used first. For the search suggestions of the common timeline.
async fn all_tags(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<browse::Tag>>> {
    let o = overview(&hub).await?;
    let (eligible, _) = multi::eligibility(&o.drives, &o.roles);
    let apps: Vec<Arc<App>> = eligible.iter().map(|&i| o.apps[i].clone()).collect();
    let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
    let own = param(&pairs, "own").is_some_and(|v| v != "0");
    let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let tags = tokio::task::spawn_blocking(move || -> ApiResult<Vec<browse::Tag>> {
        let mut merged: std::collections::BTreeMap<String, browse::Tag> = std::collections::BTreeMap::new();
        for app in &apps {
            for t in browse::all_tags(&app.conn.lock().unwrap())? {
                let key = db::tag_fold(&t.name);
                if !key.contains(&needle) || (own && t.kind == browse::TagKind::Folder) {
                    continue;
                }
                merged.entry(key).and_modify(|m| m.count += t.count).or_insert(browse::Tag { id: 0, ..t });
            }
        }
        let mut list: Vec<browse::Tag> = merged.into_values().collect();
        list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        list.truncate(limit);
        Ok(list)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(tags))
}

#[derive(Serialize)]
struct BackupState {
    library: String,
    /// The drive it copies (its id), for the "not on the backup yet" files.
    primary_library: Option<String>,
    #[serde(flatten)]
    report: Option<multi::BackupReport>,
    /// Why there is no report: the drive it copies is not there, or none matches.
    note: Option<String>,
}

/// For every drive marked as a backup (and online): what it lacks compared
/// with the drive it copies, from the indexes. `limit` lists files per kind.
async fn all_backups(State(hub): State<Arc<Hub>>, Query(q): Query<LimitQuery>) -> ApiResult<Json<Vec<BackupState>>> {
    let o = overview(&hub).await?;
    let limit = q.limit.unwrap_or(50).min(1000);
    let list = tokio::task::spawn_blocking(move || -> ApiResult<Vec<BackupState>> {
        let mut out = Vec::new();
        for (i, d) in o.drives.iter().enumerate() {
            if d.role != multi::Role::Backup {
                continue;
            }
            match multi::primary_of(&o.drives, i)? {
                Some(p) => out.push(BackupState {
                    library: d.id.clone(),
                    primary_library: Some(o.drives[p].id.clone()),
                    report: Some(multi::backup_report(&o.drives[p].db, &o.drives[p].name, &d.db, &d.name, limit)?),
                    note: None,
                }),
                None => out.push(BackupState {
                    library: d.id.clone(),
                    primary_library: None,
                    report: None,
                    note: Some("No drive that is there holds what this backup holds (is the original drive plugged in?)".into()),
                }),
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(list))
}

#[derive(Deserialize)]
struct RoleRequest {
    library: String,
    /// `backup`, `separate` or `unknown`.
    role: String,
    /// For a backup: the id of the drive it copies (else the likeliest one is used).
    #[serde(default)]
    of: Option<String>,
}

/// "This drive is a backup" / "this drive has its own photos". Stored in
/// that drive's own `library.db`; never in a photo folder.
async fn all_set_role(State(hub): State<Arc<Hub>>, Json(req): Json<RoleRequest>) -> ApiResult<Json<serde_json::Value>> {
    let role = multi::Role::parse(&req.role).ok_or_else(|| ApiError::BadRequest("role must be backup, separate or unknown".into()))?;
    let index = hub.slots.iter().position(|s| s.id == req.library).ok_or(ApiError::NotFound)?;
    let shared = hub.shared.clone();
    let hub2 = hub.clone();
    let online = tokio::task::spawn_blocking(move || hub2.slots[index].online(&shared)).await.ok().flatten();
    let Some((app, _)) = online else {
        return Err(ApiError::Offline { library: hub.slots[index].name.clone(), reason: "plug it in to change this".into() });
    };
    let of = req.of.clone().filter(|_| role == multi::Role::Backup);
    if let Some(of) = &of {
        if of == &req.library || !hub.slots.iter().any(|s| &s.id == of) {
            return Err(ApiError::BadRequest("a backup copies another drive of this server".into()));
        }
    }
    change(&app, move |_, conn| {
        multi::set_role(conn, role)?;
        multi::set_backup_of(conn, of.as_deref())
    })
    .await?;
    *hub.roles.lock().unwrap() = None;
    Ok(Json(serde_json::json!({ "library": req.library, "role": req.role })))
}

#[derive(Deserialize)]
struct LoginRequest {
    pin: String,
}

async fn login(State(hub): State<Arc<Hub>>, Json(body): Json<LoginRequest>) -> ApiResult<Response> {
    let token = hub.shared.auth.login(&body.pin)?;
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
struct ClustersInfo {
    #[serde(flatten)]
    overview: clusters::Overview,
    /// A recomputation is asked for or running in this server: what the
    /// cache says may be behind the latest decisions.
    stale: bool,
    /// Faces drawn by hand are being embedded (the recognizer runs).
    embedding: bool,
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
    index_version: String,
    /// Photos (with their companions) in the trash.
    trash: u64,
    /// Photos that need a date: none in the file, none of the user's.
    needs_date: u64,
    /// Face recognition (`shoebox recognize`).
    faces: recognize::Overview,
    /// Clusters and suggestions (5c-2).
    clusters: ClustersInfo,
    /// Text of the "show in the file manager" button ("Show in Finder", …),
    /// only for requests from this computer; `null` for other devices.
    reveal: Option<&'static str>,
}

async fn info(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<Info>> {
    let reveal = is_local(peer.ip(), host(&headers)).then(reveal::label);
    blocking(&app, move |app| {
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
        let busy = jobs_running(&conn)?;
        let (data_version, generation) = app.version(&conn)?;
        let trash: i64 = conn.query_row("SELECT count(DISTINCT batch) FROM trash", [], |r| r.get(0))?;
        let needs_date = app.snapshot(&conn)?.items.iter().filter(|it| needs_a_date(it)).count() as u64;
        let faces = recognize::overview(&conn)?;
        // New faces from a `recognize` run that stopped before clustering.
        if !faces.running
            && app.clusters_done.load(Ordering::SeqCst) == app.clusters_wanted.load(Ordering::SeqCst)
            && faces_fingerprint(&conn)? != app.faces_seen.load(Ordering::SeqCst)
        {
            app.request_clusters();
        }
        let clusters = ClustersInfo {
            overview: clusters::overview(&conn)?,
            stale: app.clusters_done.load(Ordering::SeqCst) < app.clusters_wanted.load(Ordering::SeqCst),
            embedding: app.embedding.load(Ordering::SeqCst),
        };
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
            index_version: format!("{data_version}.{generation}"),
            trash: trash as u64,
            needs_date,
            faces,
            clusters,
            reveal,
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

/// The query string as pairs, so a key can come several times
/// (`?tag=12&tag=40`).
type Pairs = Vec<(String, String)>;

fn param<'a>(pairs: &'a Pairs, key: &str) -> Option<&'a str> {
    pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `type=photo|video|live|screenshot`, several allowed (any of them matches).
fn types_of(pairs: &Pairs) -> ApiResult<Vec<browse::MediaType>> {
    pairs
        .iter()
        .filter(|(k, v)| k == "type" && !v.is_empty())
        .map(|(_, v)| browse::MediaType::parse(v).ok_or_else(|| ApiError::BadRequest(format!("unknown type: {v}"))))
        .collect()
}

/// The timeline filter of a request: `folder`, `tag` (several, all must
/// match), `type` (several, any matches), `q` (free text) and `person` (several, all must match: photos
/// with a confirmed face of each).
fn filter_of(pairs: &Pairs) -> ApiResult<browse::Query> {
    let number = |v: &str| v.parse::<i64>().map_err(|_| ApiError::BadRequest(format!("not a number: {v}")));
    Ok(browse::Query {
        folder: param(pairs, "folder").filter(|v| !v.is_empty()).map(number).transpose()?,
        tags: pairs.iter().filter(|(k, v)| k == "tag" && !v.is_empty()).map(|(_, v)| number(v)).collect::<ApiResult<_>>()?,
        text: param(pairs, "q").map(str::to_string).filter(|s| !s.trim().is_empty()),
        people: pairs.iter().filter(|(k, v)| k == "person" && !v.is_empty()).map(|(_, v)| number(v)).collect::<ApiResult<_>>()?,
        types: types_of(pairs)?,
        pets: pets_of(pairs)?,
        fav: fav_of(pairs),
        area: area_of(pairs)?,
        place: param(pairs, "place").filter(|v| !v.is_empty()).map(number).transpose()?,
        dates: dates_of(pairs)?,
        needs_date: param(pairs, "nodate").is_some_and(|v| !v.is_empty() && v != "0"),
    })
}

/// `date=1987`, `date=1987-06`, `date=1987-06-14`, repeatable (all must match,
/// each once): photos dated inside it.
fn dates_of(pairs: &Pairs) -> ApiResult<Vec<dates::Term>> {
    let mut out: Vec<dates::Term> = Vec::new();
    for (_, v) in pairs.iter().filter(|(k, v)| k == "date" && !v.is_empty()) {
        let t = dates::Term::parse_param(v).map_err(|e| ApiError::BadRequest(format!("{e:#}")))?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// A photo with no capture date in its file and no date of the user's.
fn needs_a_date(it: &browse::Item) -> bool {
    matches!(it.date_source, browse::DateSource::Folder | browse::DateSource::Created | browse::DateSource::Modified)
}

/// For the "~" on a timeline cell: m(anual), f(older name), c(reated),
/// (m)o(d)ified is `u`; a date from the file has none.
fn estimate_code(source: browse::DateSource) -> Option<char> {
    match source {
        browse::DateSource::Estimate => Some('m'),
        browse::DateSource::Folder => Some('f'),
        browse::DateSource::Created => Some('c'),
        browse::DateSource::Modified => Some('u'),
        browse::DateSource::File => None,
    }
}

/// `area=south,west,north,east`: only photos taken inside that rectangle.
fn area_of(pairs: &Pairs) -> ApiResult<Option<crate::geo::Area>> {
    let Some(v) = param(pairs, "area").filter(|v| !v.is_empty()) else { return Ok(None) };
    let n: Vec<f64> = v.split(',').map(|p| p.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| ApiError::BadRequest("area is south,west,north,east".into()))?;
    let [south, west, north, east] = n[..] else { return Err(ApiError::BadRequest("area is south,west,north,east".into())) };
    crate::geo::Area { south, west, north, east }.checked().map(Some).map_err(|e| ApiError::BadRequest(format!("{e:#}")))
}

/// `fav=1`: only favorites.
fn fav_of(pairs: &Pairs) -> bool {
    param(pairs, "fav").is_some_and(|v| !v.is_empty() && v != "0")
}

/// The pet terms of a request (`pet=cat`, `pet=dog`, `pet=pet` for any pet,
/// repeated: all must match), each once.
fn pets_of(pairs: &Pairs) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for (_, v) in pairs.iter().filter(|(k, v)| k == "pet" && !v.is_empty()) {
        if !crate::pets::is_search_species(v) {
            return Err(ApiError::BadRequest(format!("pet is cat, dog or pet, not {v:?}")));
        }
        if !out.contains(v) {
            out.push(v.clone());
        }
    }
    Ok(out)
}

/// Tags whose name contains `q`, most used first. With a filter (`tag`,
/// `folder`), counts only the photos it shows and leaves out tags that would
/// show nothing, so suggestions narrow a search; `own=1` keeps only tags the
/// user added somewhere.
async fn tags(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<browse::Tag>>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
        let own = param(&pairs, "own").is_some_and(|o| o != "0");
        let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(50);
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        let all = if filter.folder.is_none()
            && filter.tags.is_empty()
            && filter.people.is_empty()
            && filter.pets.is_empty()
            && filter.place.is_none()
            && filter.area.is_none()
        {
            browse::all_tags(&conn)?
        } else {
            let snapshot = app.snapshot(&conn)?;
            let shown: HashSet<i64> = snapshot.query(&conn, &filter)?.iter().map(|it| it.id).collect();
            browse::tags_within(&conn, &shown, &filter.tags)?
        };
        let tags = all
            .into_iter()
            .filter(|t| !own || t.kind != browse::TagKind::Folder)
            .filter(|t| db::tag_fold(&t.name).contains(&needle))
            .take(limit)
            .collect();
        Ok(Json(tags))
    })
    .await
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
    /// Names of the tags in the filter, for its chips.
    tags: Vec<browse::TagName>,
    /// Names of the people in the filter, for their chips.
    people: Vec<people::PersonRef>,
    /// Ids of the items with a heart.
    favs: Vec<i64>,
    /// [id, code] of the items whose date is not from the file: `m` set by the
    /// user, `f` the event folder's name, `c` the file's created date, `u` its
    /// modification date. For the "~" on the cell.
    est: Vec<(i64, char)>,
}

async fn timeline(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Timeline>> {
    let query = filter_of(&pairs)?;
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        if let Some(place) = query.place
            && crate::geo::place(&conn, place)?.is_none()
        {
            return Err(ApiError::BadRequest("no such place".into()));
        }
        let snapshot = app.snapshot(&conn)?;
        let items = snapshot.query(&conn, &query)?;
        let tags = browse::tag_names(&conn, &query.tags)?;
        let people = people::person_names(&conn, &query.people)?;
        let hearts = own_tags::favorite_ids(&conn)?;
        drop(conn);
        let mut t = Timeline {
            count: items.len(),
            ids: Vec::with_capacity(items.len()),
            kinds: String::with_capacity(items.len()),
            days: Vec::with_capacity(items.len()),
            versions: String::with_capacity(items.len() * 8),
            live: Vec::new(),
            tags,
            people,
            favs: Vec::new(),
            est: Vec::new(),
        };
        for it in items {
            if hearts.contains(&it.id) {
                t.favs.push(it.id);
            }
            if let Some(c) = estimate_code(it.date_source) {
                t.est.push((it.id, c));
            }
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
    /// Other versions of this photo (linked duplicates).
    linked: Vec<duplicates::Linked>,
    /// Faces found by `shoebox recognize` (and drawn by hand), with who
    /// they are or might be; "not a face" is left out. `null` if it has not
    /// looked yet.
    faces: Option<Vec<people::FileFace>>,
    /// Confirmed faces that are no longer found (after a model change).
    faces_lost: Vec<people::FaceItem>,
    /// Quarter turns clockwise that shoebox shows the photo turned (HEIC,
    /// PNG; the file itself is as it was). Face boxes are in the file's
    /// orientation.
    view_turn: i32,
    /// Counts as a screenshot (the decision below, else the score).
    screenshot: bool,
    /// The user's own decision: `true` is one, `false` is not, `null` leaves
    /// it to the score.
    screenshot_mark: Option<bool>,
    /// Where it was taken (in the file, or given by the user).
    position: Option<crate::geo::Position>,
}

async fn file_details(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<FileInfo>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let details = browse::details(&conn, id)?.ok_or(ApiError::NotFound)?;
        let snapshot = app.snapshot(&conn)?;
        let item = snapshot.items.iter().find(|it| it.id == id);
        let key: Option<String> =
            conn.query_row("SELECT quick_hash FROM files WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let view_turn = match &key {
            Some(key) => db::view_turn(&conn, key)?,
            None => 0,
        };
        let screenshot_mark = match &key {
            Some(key) => db::shot_mark(&conn, key)?,
            None => None,
        };
        let faces = match key {
            Some(key) => people::file_faces(&conn, &key)?,
            None => people::FileFaces { faces: None, lost: Vec::new() },
        };
        Ok(Json(FileInfo {
            details,
            date_source: item.map(|it| it.date_source),
            sort_date: item.map(|it| it.sort.clone()),
            live: item.and_then(|it| it.live),
            linked: duplicates::linked(&conn, id)?,
            faces: faces.faces,
            faces_lost: faces.lost,
            view_turn,
            screenshot: item.is_some_and(|it| it.shot),
            screenshot_mark,
            position: crate::geo::position_of(&conn, id)?,
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
    // A stored thumbnail is served even while its file is being looked for
    // (moved behind shoebox's back); only making one needs the file.
    let (src, turn) = blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let key: String = conn
            .query_row("SELECT quick_hash FROM files WHERE id = ?1 AND missing_since IS NULL", [id], |r| r.get(0))
            .optional()?
            .ok_or(ApiError::NotFound)?;
        let turn = db::view_turn(&conn, &key)?;
        match thumbs::load(&conn, &key)? {
            Some(Ok(bytes)) => Ok((Err(bytes), turn)),
            Some(Err(_)) => Err(ApiError::NotFound),
            None => Ok((app.source(&conn, id)?.filter(|s| s.kind != Kind::Raw).map(Ok).ok_or(ApiError::NotFound)?, turn)),
        }
    })
    .await?;
    let src = match src {
        Ok(src) => src,
        Err(stored) => return turned_jpeg(&app, stored, turn).await,
    };

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
        Ok(bytes) => turned_jpeg(&app, bytes, turn).await,
        Err(_) => Err(ApiError::NotFound),
    }
}

/// A rendered picture, turned the way the user turned it in shoebox
/// (`view_turns`); as it is when it was not.
async fn turned_jpeg(app: &Arc<App>, bytes: Vec<u8>, turn: i32) -> ApiResult<Response> {
    if turn == 0 {
        return Ok(jpeg(bytes, IMMUTABLE));
    }
    let bytes = blocking(app, move |_| {
        media::turn_jpeg(&bytes, turn, 85).map_err(ApiError::Internal)
    })
    .await?;
    Ok(jpeg(bytes, IMMUTABLE))
}

/// The full-screen image: the original where browsers can show it, a large
/// JPEG rendering for HEIC.
async fn view(State(app): State<Arc<App>>, Path(id): Path<i64>, req: Request) -> ApiResult<Response> {
    let (src, content, turn) = blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let src = app.source(&conn, id)?.ok_or(ApiError::NotFound)?;
        let content = media::content_kind(src.kind, &src.path);
        let turn = db::view_turn(&conn, &src.quick_hash)?;
        Ok((src, content, turn))
    })
    .await?;
    match src.kind {
        // A file whose name does not match its content (a JPEG called
        // `.HEIC`) is rendered, so the browser gets what the type says. A
        // picture the user turned in shoebox is rendered turned.
        Kind::Jpeg | Kind::Png | Kind::Video if content == src.kind && turn == 0 => serve_file(&src.path, req, None).await,
        Kind::Raw => Err(ApiError::NotFound),
        Kind::Jpeg | Kind::Png | Kind::Video | Kind::Heic => {
            let _permit = app.renders.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
            let bytes = blocking(&app, move |_| {
                thumbs::render_view(&LibHeif::new(), &src).map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))
            })
            .await?;
            turned_jpeg(&app, bytes, turn).await
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

/// Show the original in this computer's Finder / Explorer. Only the id comes
/// from the request; the path is the indexed one, and the file is not opened.
async fn reveal_file(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    // Another device (a session over the LAN) must not open windows here.
    if !is_local(peer.ip(), host(&headers)) {
        return Err(ApiError::Forbidden("only this computer can open its file manager"));
    }
    blocking(&app, move |app| {
        let src = app.source(&app.conn.lock().unwrap(), id)?.ok_or(ApiError::NotFound)?;
        (app.reveal)(&src.path).map_err(ApiError::Internal)?;
        Ok(Json(serde_json::json!({ "ok": true, "app": reveal::app_name() })))
    })
    .await
}

#[derive(Deserialize)]
struct AllRevealRequest {
    library: String,
    /// As in the index of that drive (relative to its folder).
    path: String,
}

/// "Show in Finder" for a file named by drive and path, as the lists of the
/// backup check give them. Only a file that is in that drive's index and still
/// there is shown; the request never supplies a path to open.
async fn all_reveal(
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<AllRevealRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if !is_local(peer.ip(), host(&headers)) {
        return Err(ApiError::Forbidden("only this computer can open its file manager"));
    }
    let apps = online_apps(&hub).await;
    let app = apps.into_iter().find(|a| library_id(&a.name) == req.library).ok_or(ApiError::NotFound)?;
    blocking(&app, move |app| {
        if !FsPath::new(&req.path).components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(ApiError::NotFound);
        }
        let known: Option<i64> = app
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM files WHERE path = ?1 AND missing_since IS NULL", [&req.path], |r| r.get(0))
            .optional()?;
        let path = app.root.join(&req.path);
        if known.is_none() || path.symlink_metadata().is_err() {
            return Err(ApiError::NotFound);
        }
        (app.reveal)(&path).map_err(ApiError::Internal)?;
        Ok(Json(serde_json::json!({ "ok": true, "app": reveal::app_name() })))
    })
    .await
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

// ---------------------------------------------------------------- changes

/// Run a change to the library: one at a time, never while a scan runs.
/// Errors are the user's to read (a name that is taken, a file that changed).
async fn change<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&App, &Connection) -> Result<T> + Send + 'static,
) -> ApiResult<T> {
    blocking(app, move |app| {
        let _writing = app.writing.lock().unwrap();
        let conn = app.conn.lock().unwrap();
        if jobs_running(&conn)? {
            return Err(ApiError::Conflict("a scan is running; try again when it is done".into()));
        }
        let result = f(app, &conn);
        app.generation.fetch_add(1, Ordering::SeqCst);
        app.dirty.store(true, Ordering::SeqCst);
        let _ = app.backup.lock().unwrap().send(());
        result.map_err(|e| {
            if e.is::<people::Stale>() {
                ApiError::Conflict(format!("{e}"))
            } else {
                ApiError::BadRequest(format!("{e:#}"))
            }
        })
    })
    .await
}

#[derive(Deserialize)]
struct MoveRequest {
    ids: Vec<i64>,
    /// Folder path relative to the library (NFC); created if needed.
    folder: String,
    /// Own tags always move along; with this the old folder's tags stay as
    /// own tags too (default: off).
    #[serde(default)]
    keep_folder_tags: bool,
}

async fn move_files(State(app): State<Arc<App>>, Json(req): Json<MoveRequest>) -> ApiResult<Json<organize::Moved>> {
    change(&app, move |app, conn| organize::move_files_with(conn, &app.root, &req.ids, &req.folder, req.keep_folder_tags)).await.map(Json)
}

#[derive(Deserialize)]
struct RotateRequest {
    /// Quarter turns clockwise; negative turns counter-clockwise.
    turns: i32,
}

/// Turn a photo: a JPEG in the file (its EXIF Orientation tag, in place), a
/// HEIC or PNG in shoebox only; see `organize::turn`.
async fn rotate_file(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<RotateRequest>,
) -> ApiResult<Json<organize::Rotated>> {
    change(&app, move |app, conn| organize::turn(conn, &app.root, id, req.turns)).await.map(Json)
}

#[derive(Deserialize)]
struct RenameRequest {
    /// The folder's new path relative to the library (NFC).
    path: String,
}

async fn rename_folder(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<RenameRequest>,
) -> ApiResult<Json<organize::RenamedFolder>> {
    change(&app, move |app, conn| organize::rename_folder(conn, &app.root, id, &req.path)).await.map(Json)
}

#[derive(Serialize)]
struct DuplicateList<'a> {
    groups: &'a [duplicates::Group],
}

async fn duplicates_list(State(app): State<Arc<App>>) -> ApiResult<Response> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let version = app.version(&conn)?;
        if let Some((v, groups)) = app.duplicates.lock().unwrap().as_ref()
            && *v == version
        {
            return Ok(Json(DuplicateList { groups: groups.as_slice() }).into_response());
        }
        let shown: HashSet<i64> = app.snapshot(&conn)?.items.iter().map(|it| it.id).collect();
        let candidates = duplicates::load(&conn, &shown)?;
        drop(conn);
        let mut groups = duplicates::find(&candidates);
        duplicates::add_tags(&app.conn.lock().unwrap(), &mut groups)?;
        let groups = Arc::new(groups);
        *app.duplicates.lock().unwrap() = Some((version, groups.clone()));
        Ok(Json(DuplicateList { groups: groups.as_slice() }).into_response())
    })
    .await
}

#[derive(Deserialize)]
struct DecideRequest {
    ids: Vec<i64>,
    /// `distinct`, `linked`, or null to forget earlier decisions.
    decision: Option<String>,
}

async fn duplicates_decide(State(app): State<Arc<App>>, Json(req): Json<DecideRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| duplicates::decide(conn, &req.ids, req.decision.as_deref()))
        .await
        .map(|n| Json(serde_json::json!({ "pairs": n })))
}

#[derive(Deserialize)]
struct RemoveCopiesRequest {
    /// Copies that stay (at least one).
    keep: Vec<i64>,
    /// Copies that go to the trash; their tags go to a copy that stays.
    remove: Vec<i64>,
    /// For capture dates that conflict: the date to take, per surviving file.
    #[serde(default)]
    dates: std::collections::HashMap<i64, String>,
}

async fn duplicates_remove(State(app): State<Arc<App>>, Json(req): Json<RemoveCopiesRequest>) -> ApiResult<Json<duplicates::Removed>> {
    change(&app, move |app, conn| duplicates::remove_copies(conn, &app.root, &req.keep, &req.remove, &req.dates)).await.map(Json)
}

fn same_folder_plan(app: &App, conn: &Connection) -> anyhow::Result<Vec<(i64, Vec<i64>)>> {
    let shown: HashSet<i64> = app.snapshot(conn)?.items.iter().map(|it| it.id).collect();
    Ok(duplicates::same_folder_plan(&duplicates::load(conn, &shown)?))
}

/// What the "same folder" button would do: contents and files.
async fn duplicates_same_folder(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let plan = same_folder_plan(app, &conn)?;
        let copies: usize = plan.iter().map(|(_, gone)| gone.len()).sum();
        Ok(Json(serde_json::json!({ "groups": plan.len(), "copies": copies })))
    })
    .await
}

/// Delete exact duplicates in the same folder without review.
async fn duplicates_remove_same_folder(State(app): State<Arc<App>>) -> ApiResult<Json<duplicates::BulkRemoved>> {
    change(&app, |app, conn| duplicates::remove_planned(conn, &app.root, &same_folder_plan(app, conn)?)).await.map(Json)
}

/// Folder names whose files count as copies, not originals.
async fn duplicates_copy_folders(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        Ok(Json(serde_json::json!({ "folders": duplicates::copy_folders(&conn)? })))
    })
    .await
}

/// How event folders are named when the app creates them (`YYYY-MM Name`).
async fn event_pattern_get(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        Ok(Json(serde_json::json!({ "pattern": library::event_pattern(&conn)?.format() })))
    })
    .await
}

#[derive(Deserialize)]
struct EventPatternRequest {
    pattern: String,
}

async fn event_pattern_set(State(app): State<Arc<App>>, Json(req): Json<EventPatternRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| library::set_event_pattern(conn, &req.pattern))
        .await
        .map(|p| Json(serde_json::json!({ "pattern": p.format() })))
}

/// Setting `allow_trash`: off unless the user turned it on in the settings
/// (the photo view and the timeline selection offer "Move to trash" only then).
const ALLOW_TRASH_KEY: &str = "allow_trash";

async fn allow_trash_get(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        Ok(Json(serde_json::json!({ "allow": db::setting(&conn, ALLOW_TRASH_KEY)?.as_deref() == Some("1") })))
    })
    .await
}

#[derive(Deserialize)]
struct AllowTrashRequest {
    allow: bool,
}

async fn allow_trash_set(State(app): State<Arc<App>>, Json(req): Json<AllowTrashRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| {
        db::set_setting(conn, ALLOW_TRASH_KEY, req.allow.then_some("1"))?;
        Ok(req.allow)
    })
    .await
    .map(|allow| Json(serde_json::json!({ "allow": allow })))
}

// ---------------------------------------------------------------- maps (phase 10)

// The Maps setting is the browser's (one for all drives), not an endpoint.

/// Every shown photo with a position, in columns like the timeline.
#[derive(Serialize)]
struct GeoPoints {
    ids: Vec<i64>,
    lats: Vec<f64>,
    lons: Vec<f64>,
}

async fn geo_points(State(app): State<Arc<App>>) -> ApiResult<Json<GeoPoints>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let snapshot = app.snapshot(&conn)?;
        let shown: HashSet<i64> = snapshot.items.iter().map(|it| it.id).collect();
        let mut out = GeoPoints { ids: Vec::new(), lats: Vec::new(), lons: Vec::new() };
        for (id, lat, lon) in crate::geo::positions(&conn)? {
            if shown.contains(&id) {
                out.ids.push(id);
                out.lats.push(lat);
                out.lons.push(lon);
            }
        }
        Ok(Json(out))
    })
    .await
}

#[derive(Deserialize)]
struct PositionRequest {
    lat: Option<f64>,
    lon: Option<f64>,
    /// Forget the position the user gave.
    #[serde(default)]
    clear: bool,
}

async fn position_set(State(app): State<Arc<App>>, Path(id): Path<i64>, Json(req): Json<PositionRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| {
        if req.clear {
            crate::geo::clear_position(conn, id)?;
        } else {
            let (Some(lat), Some(lon)) = (req.lat, req.lon) else { bail!("latitude and longitude are needed") };
            crate::geo::set_position(conn, id, lat, lon)?;
        }
        Ok(crate::geo::position_of(conn, id)?)
    })
    .await
    .map(|position| Json(serde_json::json!({ "position": position })))
}

#[derive(Deserialize)]
struct DatesRequest {
    #[serde(default)]
    ids: Vec<i64>,
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
    /// Take the user's date away (the file's own date, else the folder's
    /// month, comes back).
    #[serde(default)]
    clear: bool,
    /// Put earlier dates back (Undo): `[{id, estimate}]`, `estimate: null` for none.
    restore: Option<Vec<RestoreDate>>,
}

#[derive(Deserialize)]
struct RestoreDate {
    id: i64,
    estimate: Option<dates::Estimate>,
}

/// Give photos a date of the user's (phase 12), take it away, or put back what
/// was there. Works on every photo, also one with a capture date in its file;
/// the answer says how many of them had one and what each had before.
async fn dates_set(State(app): State<Arc<App>>, Json(req): Json<DatesRequest>) -> ApiResult<Json<dates::Changed>> {
    change(&app, move |_, conn| {
        let items: Vec<(i64, Option<dates::Estimate>)> = if let Some(restore) = req.restore {
            restore.into_iter().map(|r| (r.id, r.estimate)).collect()
        } else if req.clear {
            req.ids.iter().map(|&id| (id, None)).collect()
        } else {
            let Some(year) = req.year else { bail!("a year is needed") };
            let e = dates::Estimate { year, month: req.month, day: req.day };
            req.ids.iter().map(|&id| (id, Some(e))).collect()
        };
        dates::apply(conn, &items)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct DatesCheckRequest {
    ids: Vec<i64>,
}

/// What a date dialog for these photos should say.
async fn dates_check(State(app): State<Arc<App>>, Json(req): Json<DatesCheckRequest>) -> ApiResult<Json<dates::Check>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        dates::check(&conn, &req.ids).map(Json).map_err(|e| ApiError::BadRequest(format!("{e:#}")))
    })
    .await
}

/// The rows of the search box's "Date" group (`q` is what was typed after the
/// optional `date:` prefix, `prefix=1` when it was typed), counted within the
/// rest of the filter.
async fn dates_suggest(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<dates::Row>>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let filter = browse::Query { text: None, dates: Vec::new(), ..filter_of(&pairs)? };
        let snapshot = app.snapshot(&conn)?;
        let all: Vec<(i32, Option<u32>, Option<u32>)> = snapshot.query(&conn, &filter)?.iter().map(|it| it.date_parts()).collect();
        let prefix = param(&pairs, "prefix").is_some_and(|v| !v.is_empty() && v != "0");
        Ok(Json(dates::suggest(param(&pairs, "q").unwrap_or(""), prefix, &all)))
    })
    .await
}

/// The places with the number of shown photos inside each.
#[derive(Serialize)]
struct PlaceRow {
    #[serde(flatten)]
    place: crate::geo::Place,
    count: usize,
}

async fn places_list(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<PlaceRow>>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let snapshot = app.snapshot(&conn)?;
        let shown: HashSet<i64> = snapshot.items.iter().map(|it| it.id).collect();
        let points = crate::geo::positions(&conn)?;
        let rows = crate::geo::places(&conn)?
            .into_iter()
            .map(|place| {
                let count = points.iter().filter(|(id, lat, lon)| shown.contains(id) && place.area.contains(*lat, *lon)).count();
                PlaceRow { place, count }
            })
            .collect();
        Ok(Json(rows))
    })
    .await
}

#[derive(Deserialize)]
struct PlaceRequest {
    name: String,
    #[serde(flatten)]
    area: crate::geo::Area,
}

async fn places_create(State(app): State<Arc<App>>, Json(req): Json<PlaceRequest>) -> ApiResult<Json<crate::geo::Place>> {
    change(&app, move |_, conn| crate::geo::create_place(conn, &req.name, req.area)).await.map(Json)
}

#[derive(Deserialize)]
struct NameRequest {
    name: String,
}

async fn places_rename(State(app): State<Arc<App>>, Path(id): Path<i64>, Json(req): Json<NameRequest>) -> ApiResult<Json<crate::geo::Place>> {
    change(&app, move |_, conn| crate::geo::rename_place(conn, id, &req.name)).await.map(Json)
}

async fn places_redraw(State(app): State<Arc<App>>, Path(id): Path<i64>, Json(area): Json<crate::geo::Area>) -> ApiResult<Json<crate::geo::Place>> {
    change(&app, move |_, conn| crate::geo::redraw_place(conn, id, area)).await.map(Json)
}

async fn places_delete(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| crate::geo::delete_place(conn, id)).await.map(|_| Json(serde_json::json!({ "deleted": true })))
}

#[derive(Deserialize)]
struct CopyFoldersRequest {
    folders: Vec<String>,
}

async fn duplicates_set_copy_folders(State(app): State<Arc<App>>, Json(req): Json<CopyFoldersRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |_, conn| duplicates::set_copy_folders(conn, &req.folders))
        .await
        .map(|folders| Json(serde_json::json!({ "folders": folders })))
}

fn lower_quality_plan(app: &App, conn: &Connection) -> anyhow::Result<Vec<(i64, Vec<i64>)>> {
    let shown: HashSet<i64> = app.snapshot(conn)?.items.iter().map(|it| it.id).collect();
    Ok(duplicates::lower_quality_plan(&duplicates::load(conn, &shown)?))
}

/// What the "lower quality" button would do: photos and files.
async fn duplicates_lower_quality(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    blocking(&app, |app| {
        let conn = app.conn.lock().unwrap();
        let plan = lower_quality_plan(app, &conn)?;
        let copies: usize = plan.iter().map(|(_, gone)| gone.len()).sum();
        Ok(Json(serde_json::json!({ "groups": plan.len(), "copies": copies })))
    })
    .await
}

/// Delete versions that are surely the same photo in lower quality.
async fn duplicates_remove_lower_quality(State(app): State<Arc<App>>) -> ApiResult<Json<duplicates::BulkRemoved>> {
    change(&app, |app, conn| duplicates::remove_planned(conn, &app.root, &lower_quality_plan(app, conn)?)).await.map(Json)
}

#[derive(Deserialize)]
struct IdsRequest {
    ids: Vec<i64>,
}

async fn trash_files(State(app): State<Arc<App>>, Json(req): Json<IdsRequest>) -> ApiResult<Json<organize::Trashed>> {
    change(&app, move |app, conn| organize::trash_files(conn, &app.root, &req.ids)).await.map(Json)
}

async fn trash_list(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<organize::TrashItem>>> {
    blocking(&app, |app| Ok(Json(organize::trash_list(&app.conn.lock().unwrap())?))).await
}

/// The stored thumbnail of something in the trash (none is made for it).
async fn trash_thumb(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Response> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let key: Option<String> =
            conn.query_row("SELECT quick_hash FROM trash WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
        match key.map(|k| thumbs::load(&conn, &k)).transpose()?.flatten() {
            Some(Ok(bytes)) => Ok(jpeg(bytes, "private, no-cache")),
            _ => Err(ApiError::NotFound),
        }
    })
    .await
}

async fn trash_restore(State(app): State<Arc<App>>, Path(batch): Path<i64>) -> ApiResult<Json<organize::Restored>> {
    change(&app, move |app, conn| organize::restore(conn, &app.root, batch)).await.map(Json)
}

#[derive(Deserialize)]
struct EmptyRequest {
    /// One batch, or everything when missing.
    batch: Option<i64>,
}

async fn trash_empty(State(app): State<Arc<App>>, Json(req): Json<EmptyRequest>) -> ApiResult<Json<serde_json::Value>> {
    change(&app, move |app, conn| organize::empty_trash(conn, &app.root, req.batch))
        .await
        .map(|n| Json(serde_json::json!({ "deleted": n })))
}

/// Look for files moved outside shoebox now (the UI's "Look for changes").
async fn rescan(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    app.request_heal();
    Json(serde_json::json!({ "ok": true }))
}

// ---------------------------------------------------------------- own tags

#[derive(Deserialize)]
struct TagRequest {
    ids: Vec<i64>,
    name: String,
}

async fn tags_add(State(app): State<Arc<App>>, Json(req): Json<TagRequest>) -> ApiResult<Json<own_tags::Changed>> {
    change(&app, move |_, conn| own_tags::add(conn, &req.ids, &req.name)).await.map(Json)
}

/// Only own tags go; folder tags stay (see `tags.rs`).
async fn tags_remove(State(app): State<Arc<App>>, Json(req): Json<TagRequest>) -> ApiResult<Json<own_tags::Changed>> {
    change(&app, move |_, conn| own_tags::remove(conn, &req.ids, &req.name)).await.map(Json)
}

#[derive(Deserialize)]
struct FavoriteRequest {
    ids: Vec<i64>,
    on: bool,
}

#[derive(Deserialize)]
struct ScreenshotRequest {
    ids: Vec<i64>,
    /// `true`: these are screenshots; `false`: they are not; `null`: leave it
    /// to the score again.
    value: Option<bool>,
}

#[derive(Serialize)]
struct ScreenshotChanged {
    changed: u64,
}

/// The user's own decision whether pictures are screenshots (by content, in
/// `library.db`; the files are not touched).
async fn screenshots_set(State(app): State<Arc<App>>, Json(req): Json<ScreenshotRequest>) -> ApiResult<Json<ScreenshotChanged>> {
    change(&app, move |_, conn| db::set_shot_marks(conn, &req.ids, req.value))
        .await
        .map(|changed| Json(ScreenshotChanged { changed }))
}

/// The heart: the own tag `favorite` on or off (see `tags::FAVORITE`).
async fn favorites_set(State(app): State<Arc<App>>, Json(req): Json<FavoriteRequest>) -> ApiResult<Json<own_tags::Changed>> {
    change(&app, move |_, conn| own_tags::set_favorite(conn, &req.ids, req.on)).await.map(Json)
}

/// The own tags on a selection, for "Remove tag…".
async fn tags_selection(State(app): State<Arc<App>>, Json(req): Json<IdsRequest>) -> ApiResult<Json<Vec<own_tags::Counted>>> {
    blocking(&app, move |app| Ok(Json(own_tags::own_tags_of(&app.conn.lock().unwrap(), &req.ids)?))).await
}

// ---------------------------------------------------------------- assets

#[derive(rust_embed::Embed)]
#[folder = "web/"]
struct Assets;

/// Translations and their loader, shared with the launcher (`i18n/`).
#[derive(rust_embed::Embed)]
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
                "default-src 'self'; img-src 'self' data: blob: https://tile.openstreetmap.org; media-src 'self'; style-src 'self' 'unsafe-inline'",
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
