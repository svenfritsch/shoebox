//! `shoebox serve`: the web UI and its JSON API.
//!
//! Listens on localhost only, unless `--lan` is given; then other devices
//! (an iPad on the same network) can connect after entering a PIN, which is
//! printed at startup. Requests from this machine need no PIN, but only when
//! they name it as `localhost` or a loopback address (127.0.0.1, ::1): a web
//! page that points its own domain at 127.0.0.1 (DNS rebinding), or a proxy on
//! this machine that passes on another host, still has to log in.
//!
//! No response may be embedded by a page of another origin
//! (`Cross-Origin-Resource-Policy: same-origin`): otherwise any website open
//! while shoebox runs could show the photos through `<img>` or `<video>`.
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
//! A video poster the server cannot make (no system video framework and no
//! ffmpeg, or both failed) is grabbed by the browser and sent back
//! (`POST /api/files/{id}/poster`); it is kept in `thumbs.db` only after
//! decoding and encoding it again, and only while the video has no picture
//! (see `thumbs::browser_poster`).
//!
//! Self-healing paths: when a file is not where the index says (moved in the
//! Finder while shoebox runs), the server runs the scan's index step in the
//! background, which finds it again by its hashes.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use libheif_rs::LibHeif;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, oneshot, watch};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::browse::{self, Snapshot};
use crate::transfer;
use crate::classify::Kind;
use crate::clusters;
use crate::dates;
use crate::db;
use crate::duplicates;
use crate::export;
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
mod forget_api;
mod text_api;
mod regroup;

pub use regroup::Rules as RegroupRules;
use regroup::Why;

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
    /// When the clusters and suggestions are worked out again after
    /// decisions (`regroup`).
    pub regroup: RegroupRules,
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
    // The photo app does not start on a damaged list of libraries (phase 16).
    crate::libraries::check_registry()?;
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
        regroup: opts.regroup,
        closing: watch::channel(false).0,
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
    let hub = Arc::new(Hub { slots, shared, roles: Mutex::new(None), searches: Arc::new(Semaphore::new(1)), all_people: Mutex::new(None) });

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
    let closing = hub.clone();

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
                closing.shared.closing.send_replace(true);
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
    regroup: RegroupRules,
    /// Set when the server begins to stop: the event streams end, so the
    /// graceful shutdown does not wait for them forever.
    closing: watch::Sender<bool>,
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
        let name = volume::library_name(&root);
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
    /// The turn of the search box's requests over all drives (`in_turn`).
    searches: Arc<Semaphore>,
    /// The people over all drives as last worked out (`all_people_list`).
    all_people: Mutex<Option<AllPeopleKept>>,
}

const ROLES_TTL: Duration = Duration::from_secs(20);

impl App {
    /// Open one library (its index, thumbnails and faces) and start its
    /// background work.
    fn open(root: &FsPath, db_path: PathBuf, shared: &Arc<Shared>) -> Result<Arc<App>> {
        let root = root.canonicalize().with_context(|| format!("cannot open {}", volume::display_path(&root)))?;
        if !db_path.is_file() {
            bail!("no index at {} (run `shoebox scan` first)", volume::display_path(&db_path));
        }
        let conn = db::open_shared(&db_path)?;
        thumbs::attach(&conn, &db_path)?;
        recognize::attach(&conn, &db_path)?;
        let people_conn = db::open_existing(&db_path)?;
        recognize::attach(&people_conn, &db_path)?;
        people_conn.pragma_update(None, "query_only", true)?;

        let name = volume::library_name(&root);
        // Older versions kept a crop of every face looked at, and some people
        // have no picture of their own yet.
        let (covers, pruned) = people_api::tidy(&conn);
        if covers > 0 || pruned > 0 {
            println!("{name}: tidied up, {covers} people got a picture, {pruned} face crops nobody needs were removed.");
        }
        let (heal_tx, heal_rx) = mpsc::channel();
        let (backup_tx, backup_rx) = mpsc::channel();
        let (embed_tx, embed_rx) = mpsc::channel();
        let app = Arc::new(App {
            root,
            db_path,
            name,
            conn: Mutex::new(conn),
            snapshot: Mutex::new(None),
            duplicates: Mutex::new(None),
            copies: Mutex::new(None),
            tags: Mutex::new(None),
            person_photos: Mutex::new(None),
            generation: AtomicU64::new(0),
            info_counts: Mutex::new(None),
            info_faces: Mutex::new(None),
            people: Mutex::new(PeopleCache { conn: people_conn, state: None }),
            clustering: AtomicBool::new(false),
            faces_version: AtomicU64::new(0),
            writing: Mutex::new(()),
            dirty: AtomicBool::new(false),
            heal: Mutex::new(heal_tx),
            backup: Mutex::new(backup_tx),
            regroup: Arc::new(regroup::Schedule::new(shared.regroup)),
            threads: shared.workers,
            clusters_wanted: AtomicU64::new(0),
            clusters_done: AtomicU64::new(0),
            faces_seen: AtomicI64::new(-1),
            embed: Mutex::new(embed_tx),
            embedding: AtomicBool::new(false),
            events: watch::channel(0).0,
            closing: shared.closing.subscribe(),
            recognizer: shared.recognizer.clone(),
            stopping: AtomicBool::new(false),
            ffmpeg: shared.ffmpeg.clone(),
            reveal: shared.reveal.clone(),
            renders: Semaphore::new(shared.workers),
            searches: Arc::new(Semaphore::new(1)),
            auth: shared.auth.clone(),
            export: Arc::new(export::Shared::default()),
        });
        spawn_healer(Arc::downgrade(&app), heal_rx);
        spawn_backups(Arc::downgrade(&app), backup_rx);
        spawn_clusterer(Arc::downgrade(&app));
        spawn_embedder(Arc::downgrade(&app), embed_rx);
        app.regroup(Why::Now);
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
    /// What `/api/info` counts over the whole index, kept for a few seconds:
    /// the page asks often, and the counts cost seconds on a library of
    /// 100 000 photos while every other request waits for the connection.
    info_counts: Mutex<Option<(IndexVersion, Instant, Arc<InfoCounts>)>>,
    info_faces: Mutex<Option<(InfoFacesKey, Instant, Arc<(recognize::Overview, clusters::Overview)>)>>,
    /// Name and full hash of every file, for the common timeline (phase 18).
    copies: Mutex<Option<(IndexVersion, Arc<HashMap<i64, multi::CopyFacts>>)>>,
    /// Every tag with its counts (`browse::all_tags`, about 0.2 s on 100 000
    /// photos), which the search box asks for with every letter typed.
    tags: Mutex<Option<(IndexVersion, Arc<Vec<browse::Tag>>)>>,
    /// Photos per person within a filter, for the search box (`person_photos`).
    person_photos: Mutex<Option<people_api::PersonPhotos>>,
    /// Faces, decisions, present files and clusters as the people pages see
    /// them (`people_view`), on a connection of their own: reading them reads
    /// every face, seconds on a library of 100 000 photos.
    people: Mutex<PeopleCache>,
    /// The clustering of this server is running (`cluster`).
    clustering: AtomicBool,
    /// Counts its own changes to the detected faces (forgetting them), after
    /// which the people view is read again in full.
    faces_version: AtomicU64,
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
    /// When the background thread recomputes clusters and suggestions.
    regroup: Arc<regroup::Schedule>,
    /// Cores of this computer (the regrouping leaves one free).
    threads: usize,
    /// Counts the regroupings started, and those finished.
    clusters_wanted: AtomicU64,
    clusters_done: AtomicU64,
    /// `recognition.db`'s faces when they were last clustered (a number
    /// that changes when `recognize` adds or replaces faces).
    faces_seen: AtomicI64,
    /// Asks the background thread to embed faces drawn by hand.
    embed: Mutex<mpsc::Sender<()>>,
    /// It is embedding right now (the recognizer is running).
    embedding: AtomicBool,
    /// Counts the changes to the faces' state (clusters wanted, being
    /// recomputed, done; embedding): `/api/events` tells the browser at once,
    /// so it does not have to ask.
    events: watch::Sender<u64>,
    /// The server is stopping (`Shared::closing`).
    closing: watch::Receiver<bool>,
    recognizer: Option<PathBuf>,
    /// The server is stopping: background work ends early.
    stopping: AtomicBool,
    ffmpeg: Option<PathBuf>,
    reveal: RevealFn,
    /// Limits concurrent decodes to the number of cores.
    renders: Semaphore,
    /// The turn of the search box's requests and the timeline (`queued`).
    searches: Arc<Semaphore>,
    auth: Arc<Auth>,
    /// The export that is running or ran last (phase 12).
    export: Arc<export::Shared>,
}

/// Page cache of the clustering's connection to `recognition.db`, in KiB
/// (`PRAGMA cache_size`): the rewritten clusters of 300 000 faces take about
/// 15 MiB. Only what is used is allocated.
const CLUSTER_CACHE_KIB: i64 = 64 * 1024;

/// How long `/api/info` may reuse what it counted (thumbnails and faces
/// found meanwhile show up this late at most).
const INFO_COUNTS_AGE: Duration = Duration::from_secs(5);
/// The same for the faces' and clusters' counts (`recognize` runs in another process).
const INFO_FACES_AGE: Duration = Duration::from_secs(2);

/// The counts of `/api/info` that read every file.
struct InfoCounts {
    photos: i64,
    videos: i64,
    missing: i64,
    thumbs_total: i64,
    thumbs_done: i64,
    trash: i64,
    needs_date: u64,
    /// Photos hidden from the timeline (phase 20).
    hidden: u64,
}

/// What the faces' counts depend on: the index, `recognition.db` (another
/// process writes it) and how far the clustering is.
type InfoFacesKey = (IndexVersion, i64, u64, u64, bool);

/// Changes whenever another connection (a scan) commits, or this server
/// changes the library itself.
type IndexVersion = (i64, u64);

/// The people view and the connection it is read on: read only, and its own,
/// so reading the faces does not hold up the requests on `App::conn`, and
/// its `PRAGMA data_version` sees every commit to the index, this server's
/// own included.
struct PeopleCache {
    conn: Connection,
    state: Option<PeopleState>,
}

/// A people view and what it was read from.
struct PeopleState {
    /// `PRAGMA data_version` of `library.db` and `recognition.db`.
    main: i64,
    recog: i64,
    faces: u64,
    view: Arc<people::View>,
}

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

    /// The people view (`people::View`) as the index is now, with what it
    /// holds of `recognition.db` as it was when the clustering of this server
    /// started, while that runs (its bookkeeping and its new clusters are
    /// read when it is done: `refresh_people_view`). Kept while nothing it
    /// was read from changed; after a change to `library.db` only its part is
    /// read again (`View::refresh`), after one to `recognition.db` all.
    fn people_view(&self) -> Result<Arc<people::View>> {
        self.with_people(|_, v| Ok(v.clone()))
    }

    /// `f` with the people view and its connection (for the people's rows),
    /// one request at a time.
    fn with_people<T>(&self, f: impl FnOnce(&Connection, &Arc<people::View>) -> Result<T>) -> Result<T> {
        let mut cache = self.people.lock().unwrap();
        let view = self.view_now(&mut cache)?;
        f(&cache.conn, &view)
    }

    fn view_now(&self, cache: &mut PeopleCache) -> Result<Arc<people::View>> {
        let main: i64 = cache.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        let recog: i64 = cache.conn.query_row("PRAGMA recog.data_version", [], |r| r.get(0))?;
        let faces = self.faces_version.load(Ordering::SeqCst);
        if let Some(s) = &cache.state
            && s.faces == faces
            && (s.recog == recog || self.clustering.load(Ordering::SeqCst))
        {
            if s.main == main {
                return Ok(s.view.clone());
            }
            if let Some(view) = s.view.refresh(&cache.conn)? {
                let view = Arc::new(view);
                // `recog` stays what the view holds.
                cache.state = Some(PeopleState { main, recog: s.recog, faces, view: view.clone() });
                return Ok(view);
            }
        }
        let view = Arc::new(people::View::load(&cache.conn)?);
        cache.state = Some(PeopleState { main, recog, faces, view: view.clone() });
        Ok(view)
    }

    /// Everyone, as `people::people` lists them.
    fn people(&self, hidden: bool) -> Result<Vec<people::Person>> {
        self.with_people(|conn, v| people::people_in(conn, v, hidden))
    }

    /// After the clusters were recomputed: read the people view again now
    /// (if the people pages use it), before the page hears that they are
    /// done and asks.
    fn refresh_people_view(&self) {
        if self.people.lock().unwrap().state.is_none() {
            return;
        }
        if let Err(e) = self.people_view() {
            eprintln!("could not read the faces again: {e:#}");
        }
    }

    /// What makes a file the same photo as one on another drive (phase 18).
    fn copy_facts(&self, conn: &Connection) -> Result<Arc<HashMap<i64, multi::CopyFacts>>> {
        let version = self.version(conn)?;
        let mut cache = self.copies.lock().unwrap();
        if let Some((v, c)) = cache.as_ref()
            && *v == version
        {
            return Ok(c.clone());
        }
        let c = Arc::new(multi::copy_facts(conn)?);
        *cache = Some((version, c.clone()));
        Ok(c)
    }

    /// Every tag with the number of present files carrying it (`browse::all_tags`).
    /// Own tags change only through `change`, so the index version tells.
    fn all_tags(&self, conn: &Connection) -> Result<Arc<Vec<browse::Tag>>> {
        let version = self.version(conn)?;
        let mut cache = self.tags.lock().unwrap();
        if let Some((v, t)) = cache.as_ref()
            && *v == version
        {
            return Ok(t.clone());
        }
        // Hidden photos (phase 20) are not counted: a tag's number is what a click shows.
        let (files, folders) = self.snapshot(conn)?.hidden_exclusions();
        let t = Arc::new(browse::all_tags_shown(conn, &files, &folders)?);
        *cache = Some((version, t.clone()));
        Ok(t)
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

    /// Ask for the clusters and suggestions to be recomputed, when the rules
    /// say (`regroup`).
    fn regroup(&self, why: Why) {
        self.regroup.ask(why);
        self.faces_changed();
    }

    /// Tell the browsers listening on `/api/events` that the faces' state
    /// changed (they ask for `/api/info` then).
    fn faces_changed(&self) {
        self.events.send_modify(|n| *n += 1);
    }

    /// The clustering itself, on a connection of its own. Skipped while
    /// `shoebox recognize` runs: it clusters when it is done.
    fn cluster(&self) -> Result<()> {
        self.clustering.store(true, Ordering::SeqCst);
        let result = self.cluster_now();
        self.clustering.store(false, Ordering::SeqCst);
        result
    }

    fn cluster_now(&self) -> Result<()> {
        let conn = db::open_existing(&self.db_path)?;
        recognize::attach(&conn, &self.db_path)?;
        // The clusters are rewritten in one transaction. With room for all of
        // it in memory, `recognition.db` is locked against the other requests'
        // reads only while it commits, not from the first page that no longer
        // fits SQLite's default cache of 2 MiB on.
        conn.pragma_update(Some("recog"), "cache_size", -CLUSTER_CACHE_KIB)?;
        if recognize::running(&conn)? || clusters::running(&conn)? {
            return Ok(());
        }
        let seen = faces_fingerprint(&conn)?;
        let stopping = || self.stopping.load(Ordering::SeqCst);
        // Running now, and how far (the status line shows it), at most once a second.
        self.faces_changed();
        let mut told = Instant::now();
        let mut progress = |_, _| {
            if told.elapsed() >= Duration::from_secs(1) {
                told = Instant::now();
                self.faces_changed();
            }
        };
        // One core is left for the requests, which would otherwise wait for
        // the CPU (`shoebox recognize` takes them all).
        let threads = self.threads.saturating_sub(1).max(1);
        match crate::ann::with_threads(threads, || clusters::run(&conn, &stopping, &mut progress)) {
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

    /// Faces suggested for someone in the clusters' cache, on a connection
    /// of its own.
    fn suggested_now(&self) -> Result<u64> {
        let conn = db::open_existing(&self.db_path)?;
        recognize::attach(&conn, &self.db_path)?;
        Ok(clusters::overview(&conn)?.suggested)
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
        let Some(cmd) = recognize::find_worker_for(self.recognizer.as_deref(), pending.pets > 0, false) else {
            println!("{} faces and pets drawn by hand wait for the recognizer (not installed).", pending.total());
            return Ok(());
        };
        // The pet models are only loaded when a pet is waiting.
        let cmd = if pending.pets > 0 { cmd.with_pets() } else { cmd };
        self.embedding.store(true, Ordering::SeqCst);
        self.faces_changed();
        let result = (|| -> Result<recognize::DrawnStats> {
            let mut worker = recognize::Worker::start(cmd, recognize::Timeouts::default())?;
            let stopping = || self.stopping.load(Ordering::SeqCst);
            let result = recognize::embed_drawn(&conn, &self.root, &mut worker, false, &stopping);
            worker.stop();
            result
        })();
        self.embedding.store(false, Ordering::SeqCst);
        self.faces_changed();
        match result {
            Ok(s) => {
                println!("Faces and pets drawn by hand: {} embedded ({} faces without landmarks), {} failed.", s.embedded, s.plain, s.failed);
                if s.embedded > 0 {
                    self.regroup(Why::Now);
                }
                Ok(())
            }
            Err(e) if e.is::<recognize::Interrupted>() => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Copy the index to `library.db.bak` and write `userdata.json`. On a
    /// connection of its own: both read the whole index (seconds on a large
    /// one), and the requests that only read go on meanwhile; changes wait.
    fn backup(&self) -> Result<()> {
        let _writing = self.writing.lock().unwrap();
        let conn = db::open_existing(&self.db_path)?;
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

/// The latest clustering finished after the latest look for faces or pets
/// ended (`shoebox recognize` groups at its end), so the faces it found are
/// grouped already.
fn grouped_after_recognition(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT (SELECT max(finished_at) FROM recog.jobs WHERE kind = 'clusters' AND state = 'done')
             >= coalesce((SELECT max(finished_at) FROM recog.jobs WHERE kind IN ('faces', 'faces-rot', 'pets')), 0)",
        [],
        |r| Ok(r.get::<_, Option<bool>>(0)?.unwrap_or(false)),
    )?)
}

/// Recomputes clusters and suggestions when the rules say (`regroup`):
/// decisions that come in while one runs count towards the next. Ends with
/// the server.
fn spawn_clusterer(app: Weak<App>) {
    std::thread::spawn(move || {
        loop {
            let schedule = match app.upgrade() {
                Some(app) if !app.stopping.load(Ordering::SeqCst) => app.regroup.clone(),
                _ => break,
            };
            if !schedule.wait_due(Duration::from_secs(1)) {
                continue;
            }
            let Some(app) = app.upgrade() else { break };
            if app.stopping.load(Ordering::SeqCst) {
                break;
            }
            let wanted = app.clusters_wanted.fetch_add(1, Ordering::SeqCst) + 1;
            app.faces_changed();
            if let Err(e) = app.cluster() {
                eprintln!("could not group the faces: {e:#}");
            }
            app.refresh_people_view();
            // Whether it left suggestions (when they run out, the next one
            // comes soon), from the cache: the people view is only read when
            // a page asks for it.
            match app.suggested_now() {
                Ok(suggested) => schedule.grouped(suggested),
                Err(e) => eprintln!("could not count the suggestions: {e:#}"),
            }
            app.clusters_done.fetch_max(wanted, Ordering::SeqCst);
            app.faces_changed();
            drop(app);
            release_freed_memory();
        }
    });
}

/// Hand what a regrouping freed back to the system: glibc keeps freed small
/// blocks for the process, so the server stayed at about 420 MB after one on
/// the benchmark library of 100 000 photos, 80 MB with this. (Not measured on
/// macOS and Windows, whose allocators return free pages themselves.)
fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only gives unused memory of the allocator back.
    unsafe {
        libc::malloc_trim(0);
    }
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
        .route("/api/all/groups", get(all_groups))
        .route("/api/all/timeline", get(all_timeline))
        .route("/api/all/tags", get(all_tags))
        .route("/api/all/suggest", get(all_suggest))
        .route("/api/all/duplicates", get(all_duplicates))
        .route("/api/all/copies", get(all_copies))
        .route("/api/all/duplicates/folders", get(all_duplicate_folders))
        .route("/api/all/drives", get(all_drives))
        .route("/api/all/role", post(all_set_role))
        .route("/api/all/transfer", get(all_transfer).post(all_transfer_apply))
        .route("/api/all/transfer/plan", post(all_transfer_plan))
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
    // `/api/lib/{id}/compare`: which of this drive's files only it has (for the
    // badge in the info panel); needs both drives, so it is answered here.
    if tail == "compare" {
        return match compare_all(&hub, &id).await {
            Ok(v) => Json(v).into_response(),
            Err(e) => e.into_response(),
        };
    }
    let Ok(uri) = new.parse::<Uri>() else { return ApiError::NotFound.into_response() };
    // `compare=backup|original` on the timeline: the files this drive has
    // and the other drive of its backup pair does not. Worked out here,
    // where the other drive is known, and handed to the library's handler.
    let mut only = None;
    if tail == "timeline"
        && let Some(kind) = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("compare=")))
        && matches!(kind, "backup" | "original")
    {
        match compare_ids(&hub, &id, kind == "backup").await {
            Ok(ids) => only = Some(ids),
            Err(e) => return e.into_response(),
        }
    }
    *req.uri_mut() = uri;
    req.extensions_mut().insert(LibraryScope(id));
    req.extensions_mut().insert(CompareIds(only));
    router.oneshot(req).await.into_response()
}

/// The files a library's timeline is narrowed to by `compare=` (see `forward`).
#[derive(Clone)]
struct CompareIds(Option<Arc<HashSet<i64>>>);

/// `{"kind": "backup" | "original" | null, "ids": […]}`: what only this drive
/// has compared with the other drive of its backup pair (`null` and no ids for
/// a drive that is not half of a pair).
async fn compare_all(hub: &Arc<Hub>, library: &str) -> ApiResult<serde_json::Value> {
    let o = overview(hub).await?;
    let here = o.drives.iter().position(|d| d.id == library).ok_or(ApiError::NotFound)?;
    let backup = o.drives[here].role == multi::Role::Backup;
    Ok(match compare_ids(hub, library, backup).await {
        Ok(ids) => {
            let mut ids: Vec<i64> = ids.iter().copied().collect();
            ids.sort_unstable();
            serde_json::json!({ "kind": if backup { "backup" } else { "original" }, "ids": ids })
        }
        Err(ApiError::BadRequest(_)) => serde_json::json!({ "kind": null, "ids": [] }),
        Err(e) => return Err(e),
    })
}

/// The files of `library` that the other drive of its backup pair does not
/// have: for a backup, what only it holds; for the drive it copies, what is
/// not on the backup yet. A drive without such a partner gets a 400.
async fn compare_ids(hub: &Arc<Hub>, library: &str, backup: bool) -> ApiResult<Arc<HashSet<i64>>> {
    let o = overview(hub).await?;
    let library = library.to_string();
    tokio::task::spawn_blocking(move || -> ApiResult<Arc<HashSet<i64>>> {
        let here = o.drives.iter().position(|d| d.id == library).ok_or(ApiError::NotFound)?;
        if (o.drives[here].role == multi::Role::Backup) != backup {
            return Err(ApiError::BadRequest(if backup { "this drive is not a backup" } else { "this drive is a backup" }.into()));
        }
        let other = if backup {
            multi::primary_of(&o.drives, here)?
        } else {
            let mut found = None;
            for (j, d) in o.drives.iter().enumerate() {
                if d.role == multi::Role::Backup && multi::primary_of(&o.drives, j)? == Some(here) {
                    found = Some(j);
                    break;
                }
            }
            found
        };
        let other = other.ok_or_else(|| ApiError::BadRequest("no drive to compare with".into()))?;
        Ok(Arc::new(multi::only_here(&o.drives[here].db, &o.drives[other].db, backup)?))
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/events", get(events))
        .route("/api/folders", get(folders))
        .route("/api/tags", get(tags))
        .route("/api/suggest", get(suggest))
        .route("/api/timeline", get(timeline))
        .route("/api/count", get(count))
        .route("/api/files/{id}", get(file_details))
        .route("/api/files/{id}/thumb", get(thumb))
        .route("/api/files/{id}/poster", post(poster_from_browser))
        .route("/api/files/{id}/view", get(view))
        .route("/api/files/{id}/original", get(original))
        .route("/api/files/{id}/reveal", post(reveal_file))
        .route("/api/files/{id}/rotate", post(rotate_file))
        .route("/api/move", post(move_files))
        .route("/api/folders/{id}/rename", post(rename_folder))
        .route("/api/duplicates", get(duplicates_list))
        .route("/api/duplicates/decide", post(duplicates_decide))
        .route("/api/duplicates/remove", post(duplicates_remove))
        .route("/api/event-pattern", get(event_pattern_get).post(event_pattern_set))
        .route("/api/allow-trash", get(allow_trash_get).post(allow_trash_set))
        .route("/api/export/places", get(export_places))
        .route("/api/export/folders", get(export_folders))
        .route("/api/export/summary", post(export_summary))
        .route("/api/export/check", post(export_check))
        .route("/api/export/start", post(export_start))
        .route("/api/export/status", get(export_status))
        .route("/api/export/stop", post(export_stop))
        .route("/api/export/reveal", post(export_reveal))
        .route("/api/duplicates/copy-folders", get(duplicates_copy_folders).post(duplicates_set_copy_folders))
        .route("/api/trash", get(trash_list).post(trash_files))
        .route("/api/trash/{id}/thumb", get(trash_thumb))
        .route("/api/trash/{batch}/restore", post(trash_restore))
        .route("/api/trash/empty", post(trash_empty))
        .route("/api/rescan", post(rescan))
        .route("/api/tags/add", post(tags_add))
        .route("/api/tags/remove", post(tags_remove))
        .route("/api/tags/selection", post(tags_selection))
        .route("/api/favorites", post(favorites_set))
        .route("/api/hidden", post(hidden_set))
        .route("/api/hidden/folder", post(hidden_folder_set))
        .route("/api/screenshots", post(screenshots_set))
        .route("/api/geo/points", get(geo_points))
        .route("/api/files/{id}/position", post(position_set))
        .route("/api/files/positions", post(positions_set))
        .route("/api/files/positions/check", post(positions_check))
        .route("/api/files/{id}/text-hidden", post(text_api::hide))
        .route("/api/recognition/forget", get(forget_api::counts).post(forget_api::forget))
        .route("/api/text/stats", get(text_api::stats))
        .route("/api/text/limits", get(text_api::limits_get).post(text_api::limits_set))
        .route("/api/text/delete-all", post(text_api::delete_all))
        .route("/api/text/check", get(text_api::check))
        .route("/api/files/{id}/text-crop", get(text_api::crop))
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
        .route("/api/groups/search", get(people_api::groups_search))
        .route("/api/groups", get(people_api::groups).post(people_api::create_group))
        .route("/api/groups/reorder", post(people_api::reorder_groups))
        .route("/api/groups/{id}/rename", post(people_api::rename_group))
        .route("/api/groups/{id}/delete", post(people_api::delete_group))
        .route("/api/clusters", get(people_api::clusters))
        .route("/api/clusters/{id}/faces", get(people_api::cluster_faces))
        .route("/api/clusters/left", post(people_api::left_unnamed))
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

/// One at a time, in the order asked: what the search box and the timeline
/// ask for while someone types. Every one of these reads under the one
/// connection, so they wait for each other anyway; here they wait for their
/// turn before they go to a blocking thread, so a request the browser calls
/// off while it waits (a newer letter replaces it) is dropped with its turn
/// and never touches the database. Work that has started runs to its end (a
/// blocking thread cannot be stopped), so the turn goes with it.
async fn in_turn<T: Send + 'static>(queue: &Arc<Semaphore>, f: impl FnOnce() -> ApiResult<T> + Send + 'static) -> ApiResult<T> {
    let turn = queue.clone().acquire_owned().await.map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?;
    tokio::task::spawn_blocking(move || {
        let _turn = turn;
        f()
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("worker failed: {e}")))?
}

/// `blocking` in the turn of the library's searches (`in_turn`).
async fn queued<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&App) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let a = app.clone();
    in_turn(&app.searches, move || f(&a)).await
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

/// A request from this machine that addresses it as localhost or by a
/// loopback address.
pub(crate) fn is_local(peer: IpAddr, host: Option<&str>) -> bool {
    is_loopback(peer) && host.is_some_and(host_is_loopback)
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback() || matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()))
}

/// `localhost`, `127.x.x.x` or `[::1]`, with or without a port. Not another
/// address of this machine: a browser here that uses one connects from it, so
/// only a proxy could bring such a host from a loopback peer.
fn host_is_loopback(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok_and(is_loopback)
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
    h.insert(CORP, HeaderValue::from_static("same-origin"));
    res
}

/// Keeps pages of other origins from embedding a response (see the module docs).
pub(crate) const CORP: header::HeaderName = header::HeaderName::from_static("cross-origin-resource-policy");

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

/// People over all drives: the same name is the same person. `photos` counts
/// the photos the common timeline shows of them, identical copies once. With
/// a filter (the timeline's, by name) only those the list then shows, and
/// people who would show nothing are left out, as on a single drive.
async fn all_people(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<AllPeople>> {
    let apps = online_apps(&hub).await;
    let offline = hub.slots.iter().filter(|s| !apps.iter().any(|a| a.name == s.name)).map(|s| s.name.clone()).collect();
    let filter = AllFilter::of(&pairs, false)?;
    let hub2 = hub.clone();
    let people = in_turn(&hub.searches, move || all_people_list(&hub2, &apps, &filter, &pairs)).await?;
    Ok(Json(AllPeople { people: people.as_ref().clone(), offline }))
}

/// `all_people` as last worked out, kept while no drive's index or people
/// (the people view is read again after every decision) changed and the
/// filter is the same: the search box asks with every letter typed.
struct AllPeopleKept {
    drives: Vec<(String, IndexVersion, Arc<people::View>)>,
    /// The request's filter without what is typed (`q`, `limit`), sorted.
    filter: Pairs,
    people: Arc<Vec<multi::MergedPerson>>,
}

/// The people over the drives `apps` (`all_people`), hidden ones too.
fn all_people_list(hub: &Hub, apps: &[Arc<App>], filter: &AllFilter, pairs: &Pairs) -> ApiResult<Arc<Vec<multi::MergedPerson>>> {
    let mut drives = Vec::new();
    for app in apps {
        let version = app.version(&app.conn.lock().unwrap())?;
        drives.push((app.name.clone(), version, app.people_view()?));
    }
    let mut within: Pairs = pairs.iter().filter(|(k, _)| k != "q" && k != "limit").cloned().collect();
    within.sort();
    let mut kept = hub.all_people.lock().unwrap();
    if let Some(k) = kept.as_ref()
        && k.filter == within
        && k.drives.len() == drives.len()
        && k.drives.iter().zip(&drives).all(|(a, b)| a.0 == b.0 && a.1 == b.1 && Arc::ptr_eq(&a.2, &b.2))
    {
        return Ok(k.people.clone());
    }
    let mut per_drive = Vec::new();
    // By name (as `merge_people` matches them): the photos the list shows
    // of the person, a photo on two drives once (phase 18).
    let mut photos_of: HashMap<String, HashSet<multi::Photo>> = HashMap::new();
    for (d, app) in apps.iter().enumerate() {
        let conn = app.conn.lock().unwrap();
        let mut list = app.people(true)?;
        // A drive without one of the names has nothing that matches.
        let Some(query) = filter.query_for(app, &conn)? else { continue };
        let snapshot = app.snapshot(&conn)?;
        let shown: HashSet<i64> = snapshot.query(&conn, &query)?.iter().map(|it| it.id).collect();
        let copies = app.copy_facts(&conn)?;
        // Faces are kept by content: the files shown of each content key.
        let mut files_per_key: HashMap<String, Vec<i64>> = HashMap::new();
        let mut stmt = conn.prepare("SELECT id, quick_hash FROM files WHERE missing_since IS NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            if shown.contains(&id) {
                files_per_key.entry(r.get(1)?).or_default().push(id);
            }
        }
        let keys = people::keys_by_person(&conn)?;
        for p in &mut list {
            let files: Vec<i64> = keys.get(&p.id).into_iter().flatten().filter_map(|k| files_per_key.get(k)).flatten().copied().collect();
            if !filter.is_empty() {
                p.photos = files.len() as u64;
            }
            photos_of
                .entry(crate::library::nfc(p.name.trim()).to_lowercase())
                .or_default()
                .extend(files.iter().map(|&id| multi::photo(&copies, d, id)));
        }
        if !filter.is_empty() {
            list.retain(|p| p.photos > 0);
        }
        per_drive.push((library_id(&app.name), app.name.clone(), list, people::groups(&conn)?));
    }
    let mut people = multi::merge_people(&per_drive);
    for p in &mut people {
        p.photos = photos_of.get(&p.name.to_lowercase()).map_or(0, |s| s.len() as u64);
    }
    // The disk name tells the drives apart; the folder name often does not.
    for p in &mut people {
        for l in &mut p.libraries {
            l.volume = apps.iter().find(|a| library_id(&a.name) == l.library).and_then(|a| volume::placement(&a.root)).map(|p| p.volume);
        }
    }
    let people = Arc::new(people);
    *kept = Some(AllPeopleKept { drives, filter: within, people: people.clone() });
    Ok(people)
}

/// A group over all drives, by name, with the photos it would show within the
/// filter (the same name on two drives is one group).
#[derive(Serialize)]
struct AllGroup {
    name: String,
    photos: u64,
}

/// The groups whose name contains `q` that would show photos within the
/// filter, for the search box over all drives; groups in the filter are left
/// out, most photos first.
async fn all_groups(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<AllGroup>>> {
    let apps = online_apps(&hub).await;
    let filter = AllFilter::of(&pairs, false)?;
    let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
    let hits = in_turn(&hub.searches, move || all_group_hits(&apps, &filter, &needle)).await?;
    Ok(Json(hits))
}

/// `all_groups` for `needle` (folded).
fn all_group_hits(apps: &[Arc<App>], filter: &AllFilter, needle: &str) -> ApiResult<Vec<AllGroup>> {
    let mut by_name: Vec<(String, AllGroup)> = Vec::new();
    for app in apps {
        let conn = app.conn.lock().unwrap();
        let Some(query) = filter.query_for(app, &conn)? else { continue };
        let snapshot = app.snapshot(&conn)?;
        for g in people::groups(&conn)? {
            let key = db::tag_fold(&g.name);
            if !key.contains(needle) || filter.group_names.iter().any(|n| db::tag_fold(n) == key) {
                continue;
            }
            let mut with = query.clone();
            with.groups.push(g.id);
            let photos = snapshot.query(&conn, &with)?.len() as u64;
            if photos == 0 {
                continue;
            }
            match by_name.iter_mut().find(|(k, _)| *k == key) {
                Some((_, hit)) => hit.photos += photos,
                None => by_name.push((key, AllGroup { name: g.name, photos })),
            }
        }
    }
    let mut hits: Vec<AllGroup> = by_name.into_iter().map(|(_, h)| h).collect();
    hits.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(hits)
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

/// The disk a drive's folder is on (names of two folders can be alike).
fn volume_of(apps: &[Arc<App>], library: &str) -> Option<String> {
    apps.iter().find(|a| library_id(&a.name) == library).and_then(|a| volume::placement(&a.root)).map(|p| p.volume)
}

/// Contents on more than one drive, leaving out backups (and drives that
/// look like one until the user decides). `ignore` (repeatable) is a folder
/// name: a content with a copy in a folder of that name is left out.
async fn all_duplicates(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<multi::CrossDuplicates>> {
    let o = overview(&hub).await?;
    let limit = param(&pairs, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(500).min(5000);
    let ignore = terms_of(&pairs, "ignore");
    let result = tokio::task::spawn_blocking(move || {
        let mut result = multi::cross_duplicates(&o.drives, &o.roles, &ignore, limit)?;
        let mut volumes: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
        for g in &mut result.groups {
            for f in &mut g.files {
                f.volume = volumes.entry(f.library.clone()).or_insert_with(|| volume_of(&o.apps, &f.library)).clone();
            }
        }
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct CopiesQuery {
    library: String,
    file: i64,
}

/// The other copies of a photo in the common timeline (phase 18): files with
/// the same name and the same full hash on the drives that take part (not
/// backups). For the info panel's "Also on" row; read only.
async fn all_copies(State(hub): State<Arc<Hub>>, Query(q): Query<CopiesQuery>) -> ApiResult<Json<Vec<multi::OtherCopy>>> {
    let o = overview(&hub).await?;
    let list = tokio::task::spawn_blocking(move || -> ApiResult<Vec<multi::OtherCopy>> {
        let app = o.apps.iter().find(|a| library_id(&a.name) == q.library).ok_or(ApiError::NotFound)?;
        let key = {
            let conn = app.conn.lock().unwrap();
            app.copy_facts(&conn)?.get(&q.file).map(|c| c.key.clone())
        };
        let Some(key) = key else { return Ok(Vec::new()) };
        let (eligible, _) = multi::eligibility(&o.drives, &o.roles);
        let mut out = Vec::new();
        for &i in &eligible {
            let a = &o.apps[i];
            let id = library_id(&a.name);
            let files = multi::files_with_key(&a.conn.lock().unwrap(), &key)?;
            let volume = if files.is_empty() { None } else { volume::placement(&a.root).map(|p| p.volume) };
            for (file, folder_id, path) in files {
                if id == q.library && file == q.file {
                    continue;
                }
                out.push(multi::OtherCopy { library: id.clone(), name: a.name.clone(), volume: volume.clone(), id: file, folder_id, path });
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(list))
}

/// Folder names for the "Ignore folders" field: `q` (what was typed), `ignore`
/// (the names already chosen, repeatable), `limit` (default 8).
async fn all_duplicate_folders(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<multi::FolderName>>> {
    let o = overview(&hub).await?;
    let limit = param(&pairs, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(8).min(50);
    let ignore = terms_of(&pairs, "ignore");
    let text = param(&pairs, "q").unwrap_or("").to_string();
    let rows = tokio::task::spawn_blocking(move || {
        let mut rows = multi::duplicate_folders(&o.drives, &o.roles, &ignore, &text, limit)?;
        for d in rows.iter_mut().flat_map(|r| r.drives.iter_mut()) {
            d.volume = volume_of(&o.apps, &d.library);
        }
        Ok::<_, anyhow::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(rows))
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
    /// In path order, so the two versions of a photo sit side by side (the
    /// UI shows no month headings): `compare=different`.
    flat: bool,
    /// Copies left out because the same photo is shown once (phase 18).
    hidden: usize,
}

#[derive(Serialize)]
struct LibraryRef {
    id: String,
    name: String,
    /// The disk the drive is on, as the drive cards show it.
    volume: Option<String>,
    role: multi::Role,
}

/// The filter of a request over all drives, by name (the drives share names,
/// not ids). The timeline and the search suggestions (`/api/all/tags`,
/// `/api/all/people`) read it the same way, so a count in a suggestion is what
/// the list then shows.
struct AllFilter {
    tag_names: Vec<String>,
    person_names: Vec<String>,
    group_names: Vec<String>,
    text: Option<String>,
    types: Vec<browse::MediaType>,
    pets: Vec<String>,
    in_text: Vec<String>,
    names: Vec<String>,
    fav: bool,
    hidden: bool,
    dates: Vec<dates::Term>,
    needs_date: bool,
}

impl AllFilter {
    /// `with_text`: `q` is free text (the timeline). The suggestions pass false:
    /// there `q` is the word being typed, not part of the filter.
    fn of(pairs: &Pairs, with_text: bool) -> ApiResult<AllFilter> {
        Ok(AllFilter {
            tag_names: pairs.iter().filter(|(k, v)| k == "tag" && !v.is_empty()).map(|(_, v)| v.clone()).collect(),
            person_names: pairs.iter().filter(|(k, v)| k == "person" && !v.is_empty()).map(|(_, v)| v.clone()).collect(),
            group_names: pairs.iter().filter(|(k, v)| k == "group" && !v.is_empty()).map(|(_, v)| v.clone()).collect(),
            text: param(pairs, "q").map(str::to_string).filter(|s| with_text && !s.trim().is_empty()),
            types: types_of(pairs)?,
            pets: pets_of(pairs)?,
            in_text: terms_of(pairs, "text"),
            names: terms_of(pairs, "name"),
            fav: fav_of(pairs),
            hidden: hidden_of(pairs),
            dates: dates_of(pairs)?,
            needs_date: param(pairs, "nodate").is_some_and(|v| !v.is_empty() && v != "0"),
        })
    }

    /// Nothing narrows the list.
    fn is_empty(&self) -> bool {
        self.tag_names.is_empty()
            && self.person_names.is_empty()
            && self.group_names.is_empty()
            && self.text.is_none()
            && self.types.is_empty()
            && self.pets.is_empty()
            && self.in_text.is_empty()
            && self.names.is_empty()
            && !self.fav
            && !self.hidden
            && self.dates.is_empty()
            && !self.needs_date
    }

    /// The query for one drive, with the names turned into its own ids. `None`
    /// when the drive lacks one of the tags, people or groups: nothing on it matches
    /// all of them.
    fn query_for(&self, app: &App, conn: &Connection) -> ApiResult<Option<browse::Query>> {
        let mut tags = Vec::new();
        for name in &self.tag_names {
            match db::find_tag(conn, name)? {
                Some(id) => tags.push(id),
                None => return Ok(None),
            }
        }
        let known_people = if self.person_names.is_empty() { Vec::new() } else { app.people(true)? };
        let people_ids: Vec<i64> = self
            .person_names
            .iter()
            .filter_map(|n| {
                let key = db::tag_fold(n);
                known_people.iter().find(|p| db::tag_fold(&p.name) == key).map(|p| p.id)
            })
            .collect();
        if people_ids.len() != self.person_names.len() {
            return Ok(None);
        }
        let known_groups = if self.group_names.is_empty() { Vec::new() } else { people::groups(conn)? };
        let group_ids: Vec<i64> = self
            .group_names
            .iter()
            .filter_map(|n| {
                let key = db::tag_fold(n);
                known_groups.iter().find(|g| db::tag_fold(&g.name) == key).map(|g| g.id)
            })
            .collect();
        if group_ids.len() != self.group_names.len() {
            return Ok(None);
        }
        Ok(Some(browse::Query {
            folder: None,
            tags,
            text: self.text.clone(),
            people: people_ids,
            groups: group_ids,
            pets: self.pets.clone(),
            types: self.types.clone(),
            in_text: self.in_text.clone(),
            names: self.names.clone(),
            fav: self.fav,
            hidden: self.hidden,
            dates: self.dates.clone(),
            needs_date: self.needs_date,
            ..Default::default()
        }))
    }
}

/// One timeline over the drives that are there and not backups. Filters name
/// things instead of numbering them (`tag=Winter`, `person=Anna`, `q=…`):
/// ids differ from drive to drive, names are what the drives share. A drive
/// without one of the tags or people has nothing that matches all of them.
async fn all_timeline(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<AllTimeline>> {
    let o = overview(&hub).await?;
    let (mut eligible, mut left_out) = multi::eligibility(&o.drives, &o.roles);
    for s in &hub.slots {
        if !o.drives.iter().any(|d| d.id == s.id) {
            left_out.push(multi::Excluded { library: s.id.clone(), name: s.name.clone(), reason: "offline".into() });
        }
    }
    // `compare=different`: the photos the backup check lists as different on
    // the backup, from both drives of the pair (the backup is shown here on
    // purpose), the two versions of a photo next to each other.
    let mut different: Option<Vec<Arc<HashSet<i64>>>> = None;
    if param(&pairs, "compare") == Some("different") {
        let drives = o.drives.clone();
        let found = tokio::task::spawn_blocking(move || -> ApiResult<Option<(usize, usize, Vec<(i64, i64)>)>> {
            Ok(match multi::first_pair(&drives)? {
                Some((p, b)) => Some((p, b, multi::different_pairs(&drives[p].db, &drives[b].db)?)),
                None => None,
            })
        })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
        let (p, b, same_path) = found.ok_or_else(|| ApiError::BadRequest("no backup to compare with".into()))?;
        eligible = vec![p, b];
        left_out.clear();
        different = Some(vec![
            Arc::new(same_path.iter().map(|x| x.0).collect()),
            Arc::new(same_path.iter().map(|x| x.1).collect()),
        ]);
    }
    let flat = different.is_some();
    let apps: Vec<Arc<App>> = eligible.iter().map(|&i| o.apps[i].clone()).collect();
    let filter = AllFilter::of(&pairs, true)?;
    let text_terms = filter.in_text.clone();
    let libs: Vec<LibraryRef> = apps
        .iter()
        .zip(&eligible)
        .map(|(a, &i)| LibraryRef { id: library_id(&a.name), name: a.name.clone(), volume: volume::placement(&a.root).map(|p| p.volume), role: o.drives[i].role })
        .collect();

    let timeline = in_turn(&hub.searches, move || -> ApiResult<(Timeline, usize)> {
        struct Row {
            sort: String,
            path: String,
            gid: i64,
            kind: char,
            day: u32,
            version: String,
            live: Option<i64>,
            fav: bool,
            hid: bool,
            snip: Option<String>,
            est: Option<char>,
            drive: usize,
            copy: Option<multi::CopyFacts>,
        }
        let mut rows: Vec<Row> = Vec::new();
        // Photos with a heart on any drive, by the key of their copies.
        let mut fav_keys: HashSet<multi::CopyKey> = HashSet::new();
        for (d, app) in apps.iter().enumerate() {
            let conn = app.conn.lock().unwrap();
            let hearts = own_tags::favorite_ids(&conn)?;
            let copies = if flat { None } else { Some(app.copy_facts(&conn)?) };
            if let Some(copies) = &copies {
                fav_keys.extend(hearts.iter().filter_map(|id| copies.get(id)).map(|c| c.key.clone()));
            }
            let Some(mut query) = filter.query_for(app, &conn)? else { continue };
            let snapshot = app.snapshot(&conn)?;
            query.ids = different.as_ref().map(|sets| sets[d].clone());
            let items = snapshot.query(&conn, &query)?;
            let snips = if text_terms.is_empty() {
                HashMap::new()
            } else {
                crate::text::snippets(&conn, &items.iter().map(|it| it.id).collect(), &text_terms)?
            };
            for it in items {
                rows.push(Row {
                    snip: snips.get(&it.id).cloned(),
                    fav: hearts.contains(&it.id),
                    hid: it.hidden,
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
                    drive: d,
                    copy: copies.as_ref().and_then(|c| c.get(&it.id).cloned()),
                });
            }
        }
        // Identical photos once (phase 18): of the copies the filter left, one
        // stays (`multi::shown_first`), with a heart if any copy has one.
        let mut hidden = 0;
        if !flat {
            let mut best: HashMap<&multi::CopyKey, usize> = HashMap::new();
            for (i, r) in rows.iter().enumerate() {
                let Some(c) = &r.copy else { continue };
                let b = best.entry(&c.key).or_insert(i);
                let o = &rows[*b];
                if multi::shown_first((c.dated_at, r.drive, &r.path), (o.copy.as_ref().and_then(|c| c.dated_at), o.drive, &o.path)).is_lt() {
                    *b = i;
                }
            }
            let keep: HashSet<usize> = best.into_values().collect();
            let before = rows.len();
            let mut i = 0;
            rows.retain(|r| {
                i += 1;
                r.copy.is_none() || keep.contains(&(i - 1))
            });
            hidden = before - rows.len();
            for r in &mut rows {
                r.fav |= r.copy.as_ref().is_some_and(|c| fav_keys.contains(&c.key));
            }
        }
        if flat {
            rows.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.gid.cmp(&b.gid)));
        } else {
            rows.sort_by(|a, b| b.sort.cmp(&a.sort).then_with(|| a.path.cmp(&b.path)).then_with(|| a.gid.cmp(&b.gid)));
        }
        let mut t = Timeline {
            count: rows.len(),
            ids: Vec::with_capacity(rows.len()),
            kinds: String::with_capacity(rows.len()),
            days: Vec::with_capacity(rows.len()),
            versions: String::with_capacity(rows.len() * 8),
            live: Vec::new(),
            tags: Vec::new(),
            people: Vec::new(),
            groups: Vec::new(),
            favs: Vec::new(),
            hids: Vec::new(),
            snips: Vec::new(),
            est: Vec::new(),
        };
        for r in rows {
            if let Some(line) = r.snip {
                t.snips.push((r.gid, line));
            }
            if r.fav {
                t.favs.push(r.gid);
            }
            if r.hid {
                t.hids.push(r.gid);
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
        Ok((t, hidden))
    })
    .await?;
    let (timeline, hidden) = timeline;
    Ok(Json(AllTimeline { timeline, libs, left_out, flat, hidden }))
}

/// Tags over the drives that take part, by name (spellings fold together),
/// most used first. For the search suggestions of the common timeline: counts
/// are photos the timeline shows, identical copies once (phase 18). With a
/// filter (the timeline's: `tag`, `person`, `type`, `fav`, `date`, ... by name)
/// it counts only the photos the list shows and leaves out tags that would
/// show nothing, as the single drive's suggestions do; `q` is the word typed.
async fn all_tags(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<browse::Tag>>> {
    let o = overview(&hub).await?;
    let (eligible, _) = multi::eligibility(&o.drives, &o.roles);
    let apps: Vec<Arc<App>> = eligible.iter().map(|&i| o.apps[i].clone()).collect();
    let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
    let own = param(&pairs, "own").is_some_and(|v| v != "0");
    let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let filter = AllFilter::of(&pairs, false)?;
    let tags = in_turn(&hub.searches, move || all_tag_hits(&apps, &filter, &needle, own, limit)).await?;
    Ok(Json(tags))
}

/// The search box's suggestions over all drives, in one reply: what
/// `all_tags?own=1`, `all_people` (by name, hidden people left out, as on one
/// drive), `pets/search` added up over the drives and `all_groups` give for
/// `q`, counted within the filter (by name, as the common timeline's). The
/// common timeline has no date suggestions and no number on the text row, so
/// `date:`, `text:` and `name:` get an empty reply.
async fn all_suggest(State(hub): State<Arc<Hub>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<AllSuggestions>> {
    let q = param(&pairs, "q").unwrap_or("").to_string();
    let Typed::Plain(text) = typed(&q) else { return Ok(Json(AllSuggestions::default())) };
    let text = text.to_string();
    let o = overview(&hub).await?;
    let (eligible, _) = multi::eligibility(&o.drives, &o.roles);
    let taking_part: Vec<Arc<App>> = eligible.iter().map(|&i| o.apps[i].clone()).collect();
    let online = o.apps.clone();
    let filter = AllFilter::of(&pairs, false)?;
    let hub2 = hub.clone();
    let out = in_turn(&hub.searches, move || -> ApiResult<AllSuggestions> {
        let needle = db::tag_fold(&text);
        let tags = all_tag_hits(&taking_part, &filter, &needle, true, SUGGEST_TAGS)?;
        let in_filter = |name: &str| filter.person_names.iter().any(|n| db::tag_fold(n) == db::tag_fold(name));
        let people = all_people_list(&hub2, &online, &filter, &pairs)?
            .iter()
            .filter(|p| !p.hidden && !in_filter(&p.name) && db::tag_fold(&p.name).contains(&needle) && p.photos > 0)
            .take(SUGGEST_PEOPLE)
            .cloned()
            .collect();
        let mut pets: HashMap<&'static str, u64> = HashMap::new();
        for app in &taking_part {
            let conn = app.conn.lock().unwrap();
            let Some(query) = filter.query_for(app, &conn)? else { continue };
            for hit in people_api::pet_hits(app, &conn, &query, &text)? {
                *pets.entry(hit.species).or_default() += hit.photos;
            }
        }
        let mut pets: Vec<people_api::PetHit> = pets.into_iter().map(|(species, photos)| people_api::PetHit { species, photos }).collect();
        pets.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.species.cmp(b.species)));
        let mut groups = all_group_hits(&online, &filter, &needle)?;
        groups.truncate(SUGGEST_GROUPS);
        Ok(AllSuggestions { tags, people, pets, groups, ..Default::default() })
    })
    .await?;
    Ok(Json(out))
}

/// `Suggestions` over all drives: people and groups by name.
#[derive(Serialize, Default)]
struct AllSuggestions {
    tags: Vec<browse::Tag>,
    people: Vec<multi::MergedPerson>,
    pets: Vec<people_api::PetHit>,
    groups: Vec<AllGroup>,
    /// Always empty: the common timeline has no date suggestions.
    dates: Vec<dates::Row>,
    /// Always null: over all drives the text row has no number.
    text: Option<TermCount>,
    name: Option<TermCount>,
}

/// `all_tags` for `needle` (folded).
fn all_tag_hits(apps: &[Arc<App>], filter: &AllFilter, needle: &str, own: bool, limit: usize) -> ApiResult<Vec<browse::Tag>> {
    // By name: the tag, the photos that carry it and those that carry it
    // as an own tag. A photo on two drives is one photo (phase 18).
    type Merged = (browse::Tag, HashSet<multi::Photo>, HashSet<multi::Photo>);
    let mut merged: std::collections::BTreeMap<String, Merged> = std::collections::BTreeMap::new();
    for (d, app) in apps.iter().enumerate() {
        let conn = app.conn.lock().unwrap();
        // A drive without one of the names has nothing that matches.
        let Some(query) = filter.query_for(app, &conn)? else { continue };
        let snapshot = app.snapshot(&conn)?;
        let shown: HashSet<i64> = snapshot.query(&conn, &query)?.iter().map(|it| it.id).collect();
        let copies = app.copy_facts(&conn)?;
        // Whether a tag is only a folder name is a fact about the drive,
        // not about the photos shown (as on a single drive).
        let facts: HashMap<i64, browse::Tag> = app.all_tags(&conn)?.iter().map(|t| (t.id, t.clone())).collect();
        for t in browse::tag_files_within(&conn, &shown, &query.tags)? {
            let mut tag = t.tag;
            if let Some(f) = facts.get(&tag.id) {
                tag.kind = f.kind;
                if filter.is_empty() {
                    tag.last_used = f.last_used;
                }
            }
            let key = db::tag_fold(&tag.name);
            if !key.contains(needle) || (own && tag.kind == browse::TagKind::Folder) {
                continue;
            }
            let m = merged.entry(key).or_insert_with(|| (browse::Tag { id: 0, last_used: None, ..tag.clone() }, HashSet::new(), HashSet::new()));
            m.0.last_used = m.0.last_used.max(tag.last_used);
            m.1.extend(t.files.iter().map(|&id| multi::photo(&copies, d, id)));
            m.2.extend(t.manual.iter().map(|&id| multi::photo(&copies, d, id)));
        }
    }
    let merged: std::collections::BTreeMap<String, browse::Tag> = merged
        .into_iter()
        .map(|(k, (tag, photos, manual))| (k, browse::Tag { count: photos.len() as u64, manual: manual.len() as u64, ..tag }))
        .collect();
    let mut list: Vec<browse::Tag> = merged.into_values().collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    list.truncate(limit);
    Ok(list)
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

/// What carrying one drive's data over to another would do (phase 15a).
#[derive(Serialize)]
struct TransferHint {
    from: String,
    to: String,
    from_name: String,
    to_name: String,
    counts: transfer::Counts,
}

/// For every ordered pair of drives that can take part (not backups, not drives
/// that look like one until the user decides): what is on the first that the
/// second lacks, by content, from the two indexes. Pairs with nothing to carry
/// are left out. Read only.
async fn all_transfer(State(hub): State<Arc<Hub>>) -> ApiResult<Json<Vec<TransferHint>>> {
    let o = overview(&hub).await?;
    let hints = tokio::task::spawn_blocking(move || -> ApiResult<Vec<TransferHint>> {
        let (usable, _) = multi::eligibility(&o.drives, &o.roles);
        let mut sets = Vec::new();
        for &i in &usable {
            sets.push(transfer::read_drive(&o.drives[i].db)?);
        }
        let mut out = Vec::new();
        for (a, &from) in usable.iter().enumerate() {
            for (b, &to) in usable.iter().enumerate() {
                if a == b {
                    continue;
                }
                let counts = transfer::plan(&o.apps[to].conn.lock().unwrap(), &sets[a], transfer::Boxes::all())?;
                if !counts.is_nothing() {
                    out.push(TransferHint {
                        from: o.drives[from].id.clone(),
                        to: o.drives[to].id.clone(),
                        from_name: o.drives[from].name.clone(),
                        to_name: o.drives[to].name.clone(),
                        counts,
                    });
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(hints))
}

#[derive(Deserialize)]
struct TransferRequest {
    /// Where the data comes from: another drive (its id), or a `userdata.json`
    /// on this computer (its full path, phase 15b).
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    file: Option<String>,
    /// The drive that receives it.
    to: String,
    /// Which kinds to carry (default: all five).
    #[serde(default)]
    boxes: Option<transfer::Boxes>,
}

/// Where a carry over takes its data from.
enum TransferSource {
    Drive(Arc<App>),
    File(PathBuf),
}

impl TransferSource {
    /// Read the data; a file that is not a userdata.json is the request's fault.
    fn read(&self) -> ApiResult<transfer::TransferSet> {
        match self {
            TransferSource::Drive(app) => Ok(transfer::read_drive(&app.db_path)?),
            TransferSource::File(path) => Ok(transfer::read_userdata(path).map_err(bad)?.0),
        }
    }
}

/// Both ends of a carry over, if they are there and may take part: a backup is
/// refused as source and as target, and so is a drive that looks like one
/// until the user has decided. A file can only be read for a browser on this
/// computer: it is one of this computer's files, chosen in its folders.
async fn transfer_pair(hub: &Arc<Hub>, req: &TransferRequest, peer: SocketAddr, headers: &HeaderMap) -> ApiResult<(TransferSource, Arc<App>)> {
    let ids: Vec<&String> = match (&req.from, &req.file) {
        (Some(from), None) => {
            if from == &req.to {
                return Err(ApiError::BadRequest("choose two different drives".into()));
            }
            vec![from, &req.to]
        }
        (None, Some(_)) => {
            local_only(peer, headers)?;
            vec![&req.to]
        }
        _ => return Err(ApiError::BadRequest("say where the data comes from: a drive or a file".into())),
    };
    let o = overview(hub).await?;
    let mut found = Vec::new();
    for id in ids {
        let slot = hub.slots.iter().find(|s| &s.id == id).ok_or(ApiError::NotFound)?;
        let Some(i) = o.drives.iter().position(|d| &d.id == id) else {
            return Err(ApiError::Offline { library: slot.name.clone(), reason: "plug it in to carry data over".into() });
        };
        if o.drives[i].role == multi::Role::Backup {
            return Err(ApiError::BadRequest(format!("{} is a backup. Backups are not changed by the photo app.", slot.name)));
        }
        if o.roles[i].suggested_backup_of.is_some() {
            return Err(ApiError::BadRequest(format!("Decide first whether {} is a backup.", slot.name)));
        }
        found.push(o.apps[i].clone());
    }
    let to = found.pop().ok_or(ApiError::NotFound)?;
    let source = match (found.pop(), &req.file) {
        (Some(from), _) => TransferSource::Drive(from),
        (None, Some(file)) => {
            let path = PathBuf::from(file);
            if !path.is_absolute() {
                return Err(ApiError::BadRequest("choose the file".into()));
            }
            TransferSource::File(path)
        }
        (None, None) => return Err(ApiError::NotFound),
    };
    Ok((source, to))
}

/// What carrying over would do right now, for the dialog's check boxes; reads
/// both sides, writes nothing. `source_emptied`: more than half of the source
/// drive's files are gone from it (carry over before erasing a drive). For a
/// file: its name, folder and when shoebox wrote it.
async fn all_transfer_plan(
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<TransferRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (source, to) = transfer_pair(&hub, &req, peer, &headers).await?;
    let out = tokio::task::spawn_blocking(move || -> ApiResult<serde_json::Value> {
        match &source {
            TransferSource::Drive(from) => {
                let set = source.read()?;
                let counts = transfer::plan(&to.conn.lock().unwrap(), &set, transfer::Boxes::all())?;
                let emptied = transfer::looks_emptied(&from.db_path)?;
                Ok(serde_json::json!({ "from": from.name, "to": to.name, "counts": counts, "source_emptied": emptied }))
            }
            TransferSource::File(path) => {
                let (set, info) = transfer::read_userdata(path).map_err(bad)?;
                let counts = transfer::plan(&to.conn.lock().unwrap(), &set, transfer::Boxes::all())?;
                let name = path.file_name().map(|n| library::nfc(&n.to_string_lossy())).unwrap_or_default();
                let folder = path.parent().map(volume::display_path).unwrap_or_default();
                Ok(serde_json::json!({
                    "file": volume::display_path(path),
                    "file_name": name,
                    "folder": folder,
                    "written_at": info.written_at,
                    "version": info.version,
                    "to": to.name,
                    "counts": counts,
                    "source_emptied": false,
                }))
            }
        }
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    Ok(Json(out))
}

/// Carry the ticked kinds of data over: one transaction on the receiving
/// drive's `library.db`; the source (drive or file) and every photo stay as
/// they are.
async fn all_transfer_apply(
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<TransferRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (source, to) = transfer_pair(&hub, &req, peer, &headers).await?;
    let boxes = req.boxes.unwrap_or_default();
    let from = match &source {
        TransferSource::Drive(app) => app.name.clone(),
        TransferSource::File(path) => volume::display_path(path),
    };
    let set = tokio::task::spawn_blocking(move || source.read()).await.map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))??;
    let counts = change(&to, move |_, conn| {
        let counts = transfer::apply(conn, &set, boxes)?;
        if counts.people > 0 {
            // New people get their picture, like after any naming.
            people_api::tidy(conn);
        }
        Ok(counts)
    })
    .await?;
    if counts.people > 0 {
        to.regroup(Why::Now);
    }
    Ok(Json(serde_json::json!({ "from": from, "to": to.name, "counts": counts })))
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
    /// A recomputation is starting or running in this server: what the
    /// cache says may be behind the latest decisions.
    stale: bool,
    /// Faces drawn by hand are being embedded (the recognizer runs).
    embedding: bool,
    /// Decisions waiting for the next one (`pending`, `cards_left`, `wait_ms`).
    #[serde(flatten)]
    next: regroup::Status,
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
    /// The server makes video posters itself (system framework or ffmpeg);
    /// otherwise the browser does.
    video_posters: bool,
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
    /// Photos hidden from the timeline (phase 20): the sidebar's Hidden row and the suggestion.
    hidden: u64,
    /// Face recognition (`shoebox recognize`).
    faces: recognize::Overview,
    /// Clusters and suggestions (5c-2).
    clusters: ClustersInfo,
    /// The words read in photos (phase 9): how far, and whether a text search
    /// has anything to find.
    text: crate::text::Progress,
    /// Text of the "show in the file manager" button ("Show in Finder", …),
    /// only for requests from this computer; `null` for other devices.
    reveal: Option<&'static str>,
    /// Export (phase 12) saves into this computer's folders: only for requests from this computer.
    export: bool,
    /// The library keeps its index next to the program (phase 16): shoebox
    /// changes nothing in it, so the UI hides move, rotate, rename and delete.
    read_only: bool,
}

/// Server-sent events: `faces` whenever the faces' state changes (clusters
/// asked for, being recomputed, done; faces drawn by hand being embedded), so
/// the browser updates the Unnamed page and the status line without asking
/// again and again. The data is a counter; what changed is in `/api/info`.
/// The stream ends with the library (a drive unplugged) or the server.
async fn events(State(app): State<Arc<App>>) -> impl IntoResponse {
    let state = (app.events.subscribe(), app.closing.clone());
    drop(app);
    let stream = futures_util::stream::unfold(state, |(mut changes, mut closing)| async move {
        let changed = tokio::select! {
            changed = changes.changed() => changed.is_ok(),
            _ = closing.wait_for(|c| *c) => false,
        };
        if !changed {
            return None;
        }
        let n = *changes.borrow_and_update();
        Some((Ok::<_, std::convert::Infallible>(Event::default().event("faces").data(n.to_string())), (changes, closing)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn info(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<Info>> {
    let reveal = is_local(peer.ip(), host(&headers)).then(reveal::label);
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let (data_version, generation) = app.version(&conn)?;
        let version = (data_version, generation);
        let counts = {
            let mut cache = app.info_counts.lock().unwrap();
            match cache.as_ref() {
                Some((v, at, c)) if *v == version && at.elapsed() < INFO_COUNTS_AGE => c.clone(),
                _ => {
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
                    let trash: i64 = conn.query_row("SELECT count(DISTINCT batch) FROM trash", [], |r| r.get(0))?;
                    let snapshot = app.snapshot(&conn)?;
                    let needs_date = snapshot.items.iter().filter(|it| !it.hidden && needs_a_date(it)).count() as u64;
                    let hidden = snapshot.items.iter().filter(|it| it.hidden).count() as u64;
                    let c = Arc::new(InfoCounts { photos, videos, missing, thumbs_total, thumbs_done, trash, needs_date, hidden });
                    *cache = Some((version, Instant::now(), c.clone()));
                    c
                }
            }
        };
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
        let clusters_done = app.clusters_done.load(Ordering::SeqCst);
        let clusters_wanted = app.clusters_wanted.load(Ordering::SeqCst);
        let embedding = app.embedding.load(Ordering::SeqCst);
        let recog_version: i64 = conn.query_row("PRAGMA recog.data_version", [], |r| r.get(0))?;
        let faces_key = (version, recog_version, clusters_done, clusters_wanted, embedding);
        let overviews = {
            let mut cache = app.info_faces.lock().unwrap();
            match cache.as_ref() {
                Some((k, at, o)) if *k == faces_key && at.elapsed() < INFO_FACES_AGE => o.clone(),
                _ => {
                    let o = Arc::new((recognize::overview(&conn)?, clusters::overview(&conn)?));
                    *cache = Some((faces_key, Instant::now(), o.clone()));
                    o
                }
            }
        };
        let faces = overviews.0.clone();
        // New faces from a `recognize` run in another process: grouped by it
        // when it got that far (only noted here), else by this server.
        if !faces.running
            && !overviews.1.running
            && app.clusters_done.load(Ordering::SeqCst) == app.clusters_wanted.load(Ordering::SeqCst)
        {
            let seen = faces_fingerprint(&conn)?;
            if seen != app.faces_seen.load(Ordering::SeqCst) {
                if grouped_after_recognition(&conn)? {
                    app.faces_seen.store(seen, Ordering::SeqCst);
                } else {
                    app.regroup(Why::Now);
                }
            }
        }
        let clusters = ClustersInfo {
            overview: overviews.1.clone(),
            stale: clusters_done < clusters_wanted,
            embedding,
            next: app.regroup.status(),
        };
        Ok(Json(Info {
            name: app.name.clone(),
            version: env!("CARGO_PKG_VERSION"),
            photos: counts.photos as u64,
            videos: counts.videos as u64,
            missing: counts.missing as u64,
            thumbs_done: counts.thumbs_done as u64,
            thumbs_total: counts.thumbs_total as u64,
            video_posters: media::SYSTEM_VIDEO_POSTERS || app.ffmpeg.is_some(),
            jobs,
            busy,
            index_version: format!("{data_version}.{generation}"),
            trash: counts.trash as u64,
            needs_date: counts.needs_date,
            hidden: counts.hidden,
            faces,
            clusters,
            text: crate::text::progress(&conn)?,
            export: reveal.is_some(),
            read_only: crate::libraries::is_read_only(&app.root),
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
/// match), `type` (several, any matches), `q` (free text), `person` (several, all must match: photos
/// with a confirmed face of each) and `group` (several, all must match: photos
/// with a confirmed face of anyone in each).
fn filter_of(pairs: &Pairs) -> ApiResult<browse::Query> {
    let number = |v: &str| v.parse::<i64>().map_err(|_| ApiError::BadRequest(format!("not a number: {v}")));
    Ok(browse::Query {
        folder: param(pairs, "folder").filter(|v| !v.is_empty()).map(number).transpose()?,
        tags: pairs.iter().filter(|(k, v)| k == "tag" && !v.is_empty()).map(|(_, v)| number(v)).collect::<ApiResult<_>>()?,
        manual_tags: param(pairs, "manual").is_some_and(|v| !v.is_empty() && v != "0"),
        text: param(pairs, "q").map(str::to_string).filter(|s| !s.trim().is_empty()),
        people: pairs.iter().filter(|(k, v)| k == "person" && !v.is_empty()).map(|(_, v)| number(v)).collect::<ApiResult<_>>()?,
        groups: pairs.iter().filter(|(k, v)| k == "group" && !v.is_empty()).map(|(_, v)| number(v)).collect::<ApiResult<_>>()?,
        types: types_of(pairs)?,
        pets: pets_of(pairs)?,
        in_text: terms_of(pairs, "text"),
        names: terms_of(pairs, "name"),
        fav: fav_of(pairs),
        hidden: hidden_of(pairs),
        area: area_of(pairs)?,
        place: param(pairs, "place").filter(|v| !v.is_empty()).map(number).transpose()?,
        dates: dates_of(pairs)?,
        needs_date: param(pairs, "nodate").is_some_and(|v| !v.is_empty() && v != "0"),
        ids: None,
    })
}

/// The search terms of a repeated parameter (`text=…`, `name=…`), each once,
/// empty ones left out.
fn terms_of(pairs: &Pairs, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, v) in pairs.iter().filter(|(k, v)| k == key && !v.trim().is_empty()) {
        let v = v.trim().to_string();
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
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

/// `hidden=1`: only hidden photos (phase 20).
fn hidden_of(pairs: &Pairs) -> bool {
    param(pairs, "hidden").is_some_and(|v| !v.is_empty() && v != "0")
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
/// `folder`, `person`, `type`, `date`, ...), counts only the photos it shows
/// and leaves out tags that would show nothing, so suggestions narrow a
/// search; `own=1` keeps only tags the user added somewhere.
async fn tags(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<browse::Tag>>> {
    queued(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
        let own = param(&pairs, "own").is_some_and(|o| o != "0");
        let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(50);
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        Ok(Json(tag_hits(app, &conn, &filter, None, &needle, own, limit)?))
    })
    .await
}

/// Nothing in the filter narrows the photos shown.
fn shows_everything(filter: &browse::Query) -> bool {
    filter.folder.is_none()
        && filter.tags.is_empty()
        && filter.people.is_empty()
        && filter.groups.is_empty()
        && filter.pets.is_empty()
        && filter.in_text.is_empty()
        && filter.names.is_empty()
        && filter.place.is_none()
        && filter.area.is_none()
        && filter.types.is_empty()
        && !filter.fav
        && !filter.hidden
        && filter.dates.is_empty()
        && !filter.needs_date
        && filter.text.is_none()
}

/// `tags` for `needle` (folded) within `filter`, whose photos are `shown` when
/// the caller has them already (`/api/suggest`).
fn tag_hits(app: &App, conn: &Connection, filter: &browse::Query, shown: Option<&HashSet<i64>>, needle: &str, own: bool, limit: usize) -> Result<Vec<browse::Tag>> {
    let all = if shows_everything(filter) {
        app.all_tags(conn)?.as_ref().clone()
    } else {
        let computed;
        let shown = match shown {
            Some(s) => s,
            None => {
                computed = app.snapshot(conn)?.query(conn, filter)?.iter().map(|it| it.id).collect::<HashSet<i64>>();
                &computed
            }
        };
        // Whether a tag is only a folder name is a fact about the drive, not
        // about the photos shown: a tag given by hand elsewhere stays a tag.
        let kinds: HashMap<i64, browse::TagKind> = app.all_tags(conn)?.iter().map(|t| (t.id, t.kind)).collect();
        let mut within = browse::tags_within(conn, shown, &filter.tags)?;
        for t in &mut within {
            if let Some(&kind) = kinds.get(&t.id) {
                t.kind = kind;
            }
        }
        within
    };
    Ok(all
        .into_iter()
        .filter(|t| !own || t.kind != browse::TagKind::Folder)
        .filter(|t| db::tag_fold(&t.name).contains(needle))
        .take(limit)
        .collect())
}

/// What the search box's text asks for, by its prefix, as the page reads it
/// (`DATE_PREFIX` in `dates.js`, `PREFIX` in `app.js`).
#[derive(Debug, PartialEq)]
enum Typed<'a> {
    /// No prefix: every kind of suggestion.
    Plain(&'a str),
    /// `date:` or `datum:`: only dates (a month alone lists it in every year).
    Date(&'a str),
    /// `text:`: only the words read in pictures (`text=` in a search).
    Text(&'a str),
    /// `name:`: only file names (`name=` in a search).
    Name(&'a str),
}

fn typed(q: &str) -> Typed<'_> {
    let t = q.trim_start();
    if let Some((head, rest)) = t.split_once(':') {
        match head.to_ascii_lowercase().as_str() {
            "date" | "datum" => return Typed::Date(rest.trim()),
            "text" => return Typed::Text(rest.trim()),
            "name" => return Typed::Name(rest.trim()),
            _ => {}
        }
    }
    Typed::Plain(q.trim())
}

/// How many of each the search box lists.
const SUGGEST_TAGS: usize = 8;
const SUGGEST_PEOPLE: usize = 6;
const SUGGEST_GROUPS: usize = 4;

/// Everything the search box suggests for one text, in one reply: the
/// sections the page lists (empty where the prefix asks for none of them).
#[derive(Serialize, Default)]
struct Suggestions {
    tags: Vec<browse::Tag>,
    people: Vec<people_api::PersonHit>,
    pets: Vec<people_api::PetHit>,
    groups: Vec<people_api::GroupHit>,
    dates: Vec<dates::Row>,
    /// The photos whose recognized text has the typed word, within the filter
    /// (`text=` added): for plain text of two letters or more and for `text:`;
    /// else null.
    text: Option<TermCount>,
    /// The photos whose file name has the typed part, within the filter
    /// (`name=` added): only for `name:`; else null.
    name: Option<TermCount>,
}

#[derive(Serialize)]
struct TermCount {
    term: String,
    count: usize,
}

/// A section of the suggestions, empty if it could not be read (the rest are still shown).
fn section<T>(app: &App, what: &str, r: Result<Vec<T>>) -> Vec<T> {
    r.unwrap_or_else(|e| {
        eprintln!("{}: search suggestions, {what}: {e:#}", app.name);
        Vec::new()
    })
}

/// The search box's suggestions for `q` (with its prefix, see `Typed`),
/// counted within the rest of the filter (the timeline's parameters): own
/// tags as `tags?own=1`, people as `people/search`, pets as `pets/search`,
/// groups as `groups/search`, dates as `dates`, each section built by the same
/// code as those endpoints, and the number of the text row as `count?text=`.
/// `text:` and `name:` answer only that row's number (`text`, `name`). The photos the filter shows are worked out once
/// for all of them. People, pets and groups depend on the faces' tables: one
/// of them failing leaves its section empty (and says so in the log), not the
/// whole list.
async fn suggest(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Suggestions>> {
    queued(&app, move |app| {
        let q = param(&pairs, "q").unwrap_or("").to_string();
        let typed = typed(&q);
        let conn = app.conn.lock().unwrap();
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        let snapshot = app.snapshot(&conn)?;
        // How many photos the search shows with the term added, as `/count` says.
        let term_count = |term: &str, add: fn(&mut browse::Query, String)| -> Result<Option<TermCount>> {
            if term.is_empty() {
                return Ok(None);
            }
            let mut with = filter.clone();
            add(&mut with, term.to_string());
            Ok(Some(TermCount { term: term.to_string(), count: snapshot.query(&conn, &with)?.len() }))
        };
        let in_text = |q: &mut browse::Query, t: String| q.in_text.push(t);
        let (text, only_dates) = match typed {
            Typed::Plain(t) => (t, false),
            Typed::Date(t) => (t, true),
            Typed::Text(t) => return Ok(Json(Suggestions { text: term_count(t, in_text)?, ..Default::default() })),
            Typed::Name(t) => return Ok(Json(Suggestions { name: term_count(t, |q, t| q.names.push(t))?, ..Default::default() })),
        };
        let items = snapshot.query(&conn, &filter)?;
        let all_dates: Vec<(i32, Option<u32>, Option<u32>)> = items.iter().map(|it| it.date_parts()).collect();
        let dates = dates::suggest(text, only_dates, &all_dates);
        if only_dates {
            return Ok(Json(Suggestions { dates, ..Default::default() }));
        }
        // Without a filter the tags' counts are kept and the people's are
        // kept or worked out over every photo: the set is needed only then.
        let shown: Option<HashSet<i64>> = (!shows_everything(&filter)).then(|| items.iter().map(|it| it.id).collect());
        let needle = db::tag_fold(text);
        Ok(Json(Suggestions {
            tags: tag_hits(app, &conn, &filter, shown.as_ref(), &needle, true, SUGGEST_TAGS)?,
            people: section(app, "people", people_api::person_hits(app, &conn, &pairs, &filter, shown.as_ref(), &needle, SUGGEST_PEOPLE)),
            pets: section(app, "pets", people_api::pet_hits(app, &conn, &filter, text)),
            groups: section(app, "groups", people_api::group_hits(app, &conn, &filter, &needle, SUGGEST_GROUPS)),
            dates,
            // As the page offers it: the text row from two letters on.
            text: if text.chars().count() >= 2 { term_count(text, in_text)? } else { None },
            name: None,
        }))
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
    /// Names of the groups in the filter, for their chips.
    groups: Vec<people::GroupRef>,
    /// Ids of the items with a heart.
    favs: Vec<i64>,
    /// Ids of the items that are hidden from the timeline (shown by the Hidden
    /// chip, or in a hidden folder), for the eye in the viewer.
    hids: Vec<i64>,
    /// [id, line] of what a text search found in an item (the first matching
    /// line of recognized text); only with `text=` in the filter.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snips: Vec<(i64, String)>,
    /// [id, code] of the items whose date is not from the file: `m` set by the
    /// user, `f` the event folder's name, `c` the file's created date, `u` its
    /// modification date. For the "~" on the cell.
    est: Vec<(i64, char)>,
}

async fn timeline(State(app): State<Arc<App>>, axum::Extension(only): axum::Extension<CompareIds>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Timeline>> {
    let mut query = filter_of(&pairs)?;
    query.ids = only.0;
    queued(&app, move |app| {
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
        let groups = people::group_names(&conn, &query.groups)?;
        let hearts = own_tags::favorite_ids(&conn)?;
        let mut t = Timeline {
            count: items.len(),
            ids: Vec::with_capacity(items.len()),
            kinds: String::with_capacity(items.len()),
            days: Vec::with_capacity(items.len()),
            versions: String::with_capacity(items.len() * 8),
            live: Vec::new(),
            tags,
            people,
            groups,
            favs: Vec::new(),
            hids: Vec::new(),
            snips: Vec::new(),
            est: Vec::new(),
        };
        if !query.in_text.is_empty() {
            let ids: HashSet<i64> = items.iter().map(|it| it.id).collect();
            let found = crate::text::snippets(&conn, &ids, &query.in_text)?;
            t.snips = items.iter().filter_map(|it| found.get(&it.id).map(|l| (it.id, l.clone()))).collect();
        }
        drop(conn);
        for it in items {
            if hearts.contains(&it.id) {
                t.favs.push(it.id);
            }
            if it.hidden {
                t.hids.push(it.id);
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

/// How many photos a filter shows, without the list: for the number beside a
/// suggestion of the search box.
async fn count(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<serde_json::Value>> {
    let query = filter_of(&pairs)?;
    queued(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let snapshot = app.snapshot(&conn)?;
        let n = snapshot.query(&conn, &query)?.len();
        Ok(Json(serde_json::json!({ "count": n })))
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
    /// Faces found by `shoebox recognize`, drawn by hand, or confirmed by the
    /// user and no longer detected (`kept`), with who they are or might be;
    /// "not a face" is left out. `null` if it has not looked yet.
    faces: Option<Vec<people::FileFace>>,
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
    /// Lines of recognized text (phase 9) that the limits show, top to
    /// bottom, hidden ones marked; boxes are fractions of the picture in the
    /// file's orientation, like the face boxes. Empty if nothing was read.
    text: Vec<crate::text::LineOut>,
    /// Hidden from the timeline (phase 20): by the own tag, and by the folder
    /// (the NFC path of the folder with the flag, which shows it again).
    hidden: bool,
    hidden_tag: bool,
    hidden_folder: Option<String>,
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
        let text = match &key {
            Some(key) => crate::text::lines_of(&conn, key)?,
            None => Vec::new(),
        };
        let faces = match key {
            Some(key) => people::file_faces(&conn, &key, details.kind == "video")?,
            None => people::FileFaces { faces: None },
        };
        Ok(Json(FileInfo {
            details,
            date_source: item.map(|it| it.date_source),
            sort_date: item.map(|it| it.sort.clone()),
            live: item.and_then(|it| it.live),
            linked: duplicates::linked(&conn, id)?,
            faces: faces.faces,
            view_turn,
            screenshot: item.is_some_and(|it| it.shot),
            screenshot_mark,
            position: crate::geo::position_of(&conn, id)?,
            text,
            hidden: item.is_some_and(|it| it.hidden),
            hidden_tag: item.is_some_and(|it| it.hidden_tag),
            hidden_folder: item.and_then(|it| snapshot.hiding_folder(it.folder_id)).map(|f| f.path.clone()),
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

#[derive(Serialize)]
struct PosterStored {
    /// False when the video already had a picture; nothing was changed.
    stored: bool,
}

/// A frame of a video the browser grabbed because the server had no poster
/// for it. Kept as that video's poster after `thumbs::browser_poster` made
/// a picture of our own from it.
async fn poster_from_browser(State(app): State<Arc<App>>, Path(id): Path<i64>, body: Bytes) -> ApiResult<Json<PosterStored>> {
    if body.len() > thumbs::BROWSER_POSTER_MAX_BYTES {
        return Err(ApiError::BadRequest("the picture is too large".into()));
    }
    blocking(&app, move |app| {
        let key: String = app
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT quick_hash FROM files WHERE id = ?1 AND missing_since IS NULL AND kind = 'video'",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(ApiError::NotFound)?;
        let poster = thumbs::browser_poster(&body).map_err(|e| ApiError::BadRequest(format!("{e:#}")))?;
        let stored = thumbs::store_browser_poster(&app.conn.lock().unwrap(), &key, &poster)?;
        Ok(Json(PosterStored { stored }))
    })
    .await
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
    change_with(app, true, f).await
}

/// `change`; with `files_changed` false the caches of the files and folders
/// (timeline, duplicates, copies) stay and `index_version` stays the same:
/// for changes that only touch people, groups and face decisions, which none
/// of them reads. Otherwise every name given on the Unnamed page would make
/// every open page reload its folders, tags, people and places.
async fn change_with<T: Send + 'static>(
    app: &Arc<App>,
    files_changed: bool,
    f: impl FnOnce(&App, &Connection) -> Result<T> + Send + 'static,
) -> ApiResult<T> {
    blocking(app, move |app| {
        let _writing = app.writing.lock().unwrap();
        let conn = app.conn.lock().unwrap();
        if jobs_running(&conn)? {
            return Err(ApiError::Conflict("a scan is running; try again when it is done".into()));
        }
        let result = f(app, &conn);
        if files_changed {
            app.generation.fetch_add(1, Ordering::SeqCst);
        }
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
    change(&app, move |app, conn| duplicates::remove_copies_tidy(conn, &app.root, &req.keep, &req.remove, &req.dates)).await.map(Json)
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
        // A read-only library never offers the trash, whatever the setting says.
        let read_only = crate::libraries::is_read_only(&app.root);
        Ok(Json(serde_json::json!({ "allow": !read_only && db::setting(&conn, ALLOW_TRASH_KEY)?.as_deref() == Some("1"), "read_only": read_only })))
    })
    .await
}

#[derive(Deserialize)]
struct AllowTrashRequest {
    allow: bool,
}

async fn allow_trash_set(State(app): State<Arc<App>>, Json(req): Json<AllowTrashRequest>) -> ApiResult<Json<serde_json::Value>> {
    crate::libraries::ensure_writable(&app.root).map_err(bad)?;
    change(&app, move |_, conn| {
        db::set_setting(conn, ALLOW_TRASH_KEY, req.allow.then_some("1"))?;
        Ok(req.allow)
    })
    .await
    .map(|allow| Json(serde_json::json!({ "allow": allow })))
}

// ---------------------------------------------------------------- export (phase 12)

/// Export copies files onto this computer and shows its folders, so only a
/// browser on this computer may use it (not a session over the network).
fn local_only(peer: SocketAddr, headers: &HeaderMap) -> ApiResult<()> {
    if is_local(peer.ip(), host(headers)) {
        Ok(())
    } else {
        Err(ApiError::Forbidden("only this computer can export: it saves into this computer's folders"))
    }
}

fn bad(e: anyhow::Error) -> ApiError {
    ApiError::BadRequest(format!("{e:#}"))
}

async fn export_places(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<export::Places>> {
    local_only(peer, &headers)?;
    blocking(&app, |_| Ok(Json(export::places()))).await
}

async fn export_folders(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(pairs): Query<Pairs>,
) -> ApiResult<Json<export::Listing>> {
    local_only(peer, &headers)?;
    let path = pairs.iter().find(|(k, _)| k == "path").map(|(_, v)| v.clone()).unwrap_or_default();
    // `files=json`: also the `.json` files, to choose a userdata.json (Carry over data).
    let json = pairs.iter().any(|(k, v)| k == "files" && v == "json");
    blocking(&app, move |_| {
        let path = FsPath::new(&path);
        if json { export::list_json(path) } else { export::list_folders(path) }.map(Json).map_err(bad)
    })
    .await
}

#[derive(Deserialize)]
struct ExportIds {
    ids: Vec<i64>,
}

async fn export_summary(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ExportIds>,
) -> ApiResult<Json<export::Summary>> {
    local_only(peer, &headers)?;
    blocking(&app, move |app| export::summarize(&app.conn.lock().unwrap(), &req.ids).map(Json).map_err(bad)).await
}

#[derive(Deserialize)]
struct ExportRequest {
    ids: Vec<i64>,
    /// The folder to save in, and the name of the new folder inside it.
    parent: String,
    name: String,
    #[serde(default)]
    options: export::Options,
}

/// What an export would do (nothing is written): the counts, whether the
/// folder exists, and whether the copies would land on a drive with a library.
async fn export_check(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ExportRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    local_only(peer, &headers)?;
    blocking(&app, move |app| {
        let dest = export::destination(&req.parent, &req.name).map_err(bad)?;
        let plan = export::plan(&app.conn.lock().unwrap(), &req.ids, req.options, &dest).map_err(bad)?;
        let shoebox = export::shoebox_drive(&dest).map(|drive| {
            let same = export::drive_of_index(&app.db_path).is_some_and(|own| own == drive);
            let name = volume::library_name(&drive);
            serde_json::json!({ "name": library::nfc(&name), "path": volume::display_path(&drive), "same_drive": same })
        });
        Ok(Json(serde_json::json!({
            "dest": volume::display_path(&dest),
            "exists": dest.is_dir(),
            "counts": plan.counts,
            "skipped": plan.skipped,
            "shoebox": shoebox,
            "free": export::free_space(FsPath::new(&req.parent)),
        })))
    })
    .await
}

async fn export_start(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ExportRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    local_only(peer, &headers)?;
    blocking(&app, move |app| {
        if app.export.running() {
            return Err(ApiError::Conflict("an export is running already".into()));
        }
        let dest = export::destination(&req.parent, &req.name).map_err(bad)?;
        let plan = {
            let conn = app.conn.lock().unwrap();
            if jobs_running(&conn)? {
                return Err(ApiError::Conflict("a scan is running; try again when it is done".into()));
            }
            export::plan(&conn, &req.ids, req.options, &dest).map_err(bad)?
        };
        if plan.counts.total == 0 {
            return Err(ApiError::BadRequest("there is nothing to export".into()));
        }
        if let Some(free) = export::free_space(FsPath::new(&req.parent))
            && free < plan.counts.bytes
        {
            return Err(ApiError::BadRequest(format!(
                "there is not enough room: {} MB are needed and {} MB are free",
                plan.counts.bytes / 1_000_000 + 1,
                free / 1_000_000
            )));
        }
        // Say so before the thread has started, so a second click is refused.
        app.export.begin(&dest);
        let (root, shared) = (app.root.clone(), app.export.clone());
        std::thread::spawn(move || export::run(&root, plan, &dest, &shared));
        Ok(Json(serde_json::json!({ "started": true })))
    })
    .await
}

async fn export_status(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<export::Progress>> {
    local_only(peer, &headers)?;
    Ok(Json(app.export.progress.lock().unwrap().clone()))
}

async fn export_stop(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    local_only(peer, &headers)?;
    app.export.stop.store(true, Ordering::SeqCst);
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// "Show in Finder" for what the last export wrote (its first file, or the
/// folder itself). The path is the one the export reported, never one sent.
async fn export_reveal(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    local_only(peer, &headers)?;
    blocking(&app, |app| {
        let dest = PathBuf::from(app.export.progress.lock().unwrap().dest.clone());
        if dest.as_os_str().is_empty() || !dest.is_dir() {
            return Err(ApiError::NotFound);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dest)
            .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect())
            .unwrap_or_default();
        files.sort();
        let target = files.into_iter().next().unwrap_or(dest);
        (app.reveal)(&target).map_err(ApiError::Internal)?;
        Ok(Json(serde_json::json!({ "ok": true, "app": reveal::app_name() })))
    })
    .await
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
        let shown: HashSet<i64> = snapshot.items.iter().filter(|it| !it.hidden).map(|it| it.id).collect();
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
struct PositionsRequest {
    ids: Vec<i64>,
    lat: f64,
    lon: f64,
}

/// Give the selected photos one position; the ones with a position in their
/// file keep it (the answer counts them).
async fn positions_set(State(app): State<Arc<App>>, Json(req): Json<PositionsRequest>) -> ApiResult<Json<crate::geo::PositionsSet>> {
    change(&app, move |_, conn| crate::geo::set_positions(conn, &req.ids, req.lat, req.lon)).await.map(Json)
}

/// What a location dialog for these photos should say.
async fn positions_check(State(app): State<Arc<App>>, Json(req): Json<IdsRequest>) -> ApiResult<Json<crate::geo::PositionsCheck>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        crate::geo::check_positions(&conn, &req.ids).map(Json).map_err(|e| ApiError::BadRequest(format!("{e:#}")))
    })
    .await
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

/// What a date dialog for these photos should say.
async fn dates_check(State(app): State<Arc<App>>, Json(req): Json<IdsRequest>) -> ApiResult<Json<dates::Check>> {
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
    queued(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
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
        let shown: HashSet<i64> = snapshot.items.iter().filter(|it| !it.hidden).map(|it| it.id).collect();
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

#[derive(Deserialize)]
struct IdsRequest {
    ids: Vec<i64>,
}

async fn trash_files(State(app): State<Arc<App>>, Json(req): Json<IdsRequest>) -> ApiResult<Json<organize::Trashed>> {
    change(&app, move |app, conn| organize::trash_files_tidy(conn, &app.root, &req.ids)).await.map(Json)
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
struct HiddenFolderRequest {
    /// NFC path of the folder, relative to the library.
    path: String,
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

/// Hide photos from the timeline: the own tag `hidden` on or off (see `tags::HIDDEN`).
async fn hidden_set(State(app): State<Arc<App>>, Json(req): Json<FavoriteRequest>) -> ApiResult<Json<own_tags::Changed>> {
    change(&app, move |_, conn| own_tags::set_hidden(conn, &req.ids, req.on)).await.map(Json)
}

/// Hide a folder and everything below it from the timeline, or show it again. It is a
/// flag in the index (`hidden_folders`); nothing is written to the folder, so a library
/// kept next to the program takes it too.
async fn hidden_folder_set(State(app): State<Arc<App>>, Json(req): Json<HiddenFolderRequest>) -> ApiResult<Json<serde_json::Value>> {
    let changed = change(&app, move |_, conn| own_tags::set_folder_hidden(conn, &req.path, req.on)).await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
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
                // `form-action`, `base-uri` and `frame-ancestors` do not fall back
                // to `default-src`: name them, so injected markup cannot post a
                // form elsewhere or move the page's base.
                "default-src 'self'; img-src 'self' data: blob: https://tile.openstreetmap.org; media-src 'self'; style-src 'self' 'unsafe-inline'; \
                 object-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ),
    ];
    (headers, file.data).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_count_as_local() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert!(is_local(lo, Some("localhost:7878")));
        assert!(is_local(lo, Some("LOCALHOST")));
        assert!(is_local(lo, Some("127.0.0.1:7878")));
        assert!(is_local(lo, Some("127.0.0.1")));
        assert!(is_local("::1".parse().unwrap(), Some("[::1]:7878")));
        assert!(is_local("::ffff:127.0.0.1".parse().unwrap(), Some("[::ffff:127.0.0.1]:7878")));
        assert!(!is_local(lo, Some("evil.example:7878")), "DNS rebinding");
        assert!(!is_local(lo, Some("localhost.evil.example:7878")));
        assert!(!is_local(lo, None));
        // Another address, from a loopback peer: only a proxy sends that.
        assert!(!is_local(lo, Some("192.168.1.5:7878")));
        assert!(!is_local(lo, Some("100.64.0.7")));
        assert!(!is_local(lo, Some("[fe80::1]:7878")));
        assert!(!is_local(lo, Some("0.0.0.0:7878")));
        assert!(!is_local(lan, Some("192.168.1.5:7878")));
        assert!(!is_local(lan, Some("localhost:7878")));
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
