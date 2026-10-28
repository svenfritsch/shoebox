//! The read-only connections of a library (`App::read`), and what keeps
//! requests from waiting for each other on them.
//!
//! Requests that only read take one of a few connections of their own
//! (`query_only`, `thumbs.db` and `recognition.db` attached); every write
//! stays on `App::writer`. Two rules keep the pool from running dry:
//!
//! - **Kept views first, then a reader.** A request takes the kept views it
//!   needs (snapshot, tags, people view, counts) before it takes a reader, and
//!   never waits for anything (a view, another reader, the writer) while it
//!   holds one: four requests holding a reader while they wait for the people
//!   view would leave none for the rest. Views are built on a reader of their
//!   own, under their own lock. Debug builds check this (`assert_free`).
//! - **A request nobody waits for does not start.** `blocking` and `in_turn`
//!   run their work as a `Turn`, whose flag is set when the handler's future is
//!   dropped (the browser closed the connection). The first connection the
//!   work takes looks at it, and the request ends there. What has started runs
//!   to its end; changes always run.
//!
//! The same turn carries what the request waited for (`Waits`): a reader, the
//! writer, the people data's lock. The slow-request log (`serve::SlowLog`)
//! prints it, so a request that took long says whether it worked or waited.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use rusqlite::Connection;

use crate::db;
use crate::recognize;
use crate::thumbs;

/// How many readers a library gets: one per core, two to four.
pub(super) fn pool_size(cores: usize) -> usize {
    cores.clamp(2, 4)
}

/// How long a request waits for a free reader before it fails. Reads take
/// well under a second; a reader that stays away this long is held by code
/// that waits for something else while holding it (the rule in the module
/// docs, which only debug builds check), and an error beats a drive whose
/// reads hang for good. Not a deadline for slow requests.
const WAIT: Duration = Duration::from_secs(30);

pub(super) struct Readers {
    idle: Mutex<Vec<Connection>>,
    back: Condvar,
    wait: Duration,
    size: usize,
}

impl Readers {
    /// `n` connections to the index at `db_path`, which must be there with
    /// its schema up to date (`App::writer` is opened first).
    pub(super) fn open(db_path: &Path, n: usize) -> Result<Readers> {
        let mut idle = Vec::with_capacity(n);
        for _ in 0..n {
            let conn = db::open_existing(db_path)?;
            thumbs::attach(&conn, db_path)?;
            recognize::attach(&conn, db_path)?;
            conn.pragma_update(None, "query_only", true)?;
            idle.push(conn);
        }
        Ok(Readers { idle: Mutex::new(idle), back: Condvar::new(), wait: WAIT, size: n })
    }

    /// A reader, as soon as one is free. Fails when it is the request's first
    /// connection and its browser has gone (`Abandoned`), and when none came
    /// back within `WAIT`.
    pub(super) fn take(&self) -> Result<Reader<'_>> {
        assert_free("a reader");
        let began = Instant::now();
        let until = began + self.wait;
        let mut idle = self.idle.lock().unwrap();
        let conn = loop {
            if let Some(conn) = idle.pop() {
                break conn;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("no reader came free in {} s: a request holds one while it waits for something else", self.wait.as_secs());
            }
            idle = self.back.wait_timeout(idle, left).unwrap().0;
        };
        drop(idle);
        waited(Wait::Reader, began.elapsed());
        let reader = Reader { conn: Some(conn), pool: self, _held: Holding::new() };
        started()?;
        Ok(reader)
    }

    /// Tests only (`Hold`): every reader of the pool at once, once all are
    /// back, as long reads on all of them would hold it.
    pub(super) fn take_all(&self) -> Result<Vec<Reader<'_>>> {
        let until = Instant::now() + self.wait;
        let mut idle = self.idle.lock().unwrap();
        while idle.len() < self.size {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("the readers did not all come back in {} s", self.wait.as_secs());
            }
            idle = self.back.wait_timeout(idle, left).unwrap().0;
        }
        let all = idle.drain(..).map(|conn| Reader { conn: Some(conn), pool: self, _held: Holding::new() }).collect();
        Ok(all)
    }
}

/// A reader taken from the pool; it goes back when dropped.
pub(super) struct Reader<'a> {
    conn: Option<Connection>,
    pool: &'a Readers,
    _held: Holding,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.idle.lock().unwrap().push(conn);
            self.pool.back.notify_one();
        }
    }
}

thread_local! {
    /// Connections this thread holds: readers, and the people view's while a
    /// request reads on it (`App::with_people`).
    static HELD: Cell<usize> = const { Cell::new(0) };
    /// The request this thread works for (`as_turn`).
    static TURN: RefCell<Option<Turn>> = const { RefCell::new(None) };
}

/// Counts a connection as held by this thread while it lives.
pub(super) struct Holding(());

impl Holding {
    pub(super) fn new() -> Holding {
        HELD.with(|h| h.set(h.get() + 1));
        Holding(())
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        HELD.with(|h| h.set(h.get() - 1));
    }
}

/// Before waiting for `what` (a kept view, a reader): this thread must not
/// hold a connection meanwhile (see the module docs). Checked in debug builds,
/// so a test that takes this path fails instead of a pool that runs dry.
pub(super) fn assert_free(what: &str) {
    debug_assert_eq!(HELD.with(Cell::get), 0, "waiting for {what} while holding a connection: take kept views first, then a reader");
}

/// The work of one request on a blocking thread.
struct Turn {
    /// Set when the handler's future was dropped (`Gone`).
    gone: Arc<AtomicBool>,
    /// It has taken a connection.
    started: bool,
    /// What the request waited for so far.
    waits: Option<Arc<Waits>>,
}

/// What a request waits for, in nanoseconds, added up over its turns.
#[derive(Default)]
pub(super) struct Waits {
    reader: AtomicU64,
    writer: AtomicU64,
    people: AtomicU64,
}

/// What a request can wait for.
#[derive(Clone, Copy)]
pub(super) enum Wait {
    /// A free connection of the pool (`Readers::take`).
    Reader,
    /// `App::writer`, and the lock that puts changes in a row.
    Writer,
    /// The people data's lock (`App::with_people`).
    People,
}

impl Waits {
    /// (reader, writer, people)
    pub(super) fn totals(&self) -> (Duration, Duration, Duration) {
        let get = |a: &AtomicU64| Duration::from_nanos(a.load(Ordering::SeqCst));
        (get(&self.reader), get(&self.writer), get(&self.people))
    }
}

tokio::task_local! {
    /// The waits of the request this task serves (`measure`); its blocking
    /// work takes them along (`current_waits`).
    static CURRENT: Arc<Waits>;
}

/// Run a request's handler, and take back what its work waited for.
pub(super) async fn measure<F: Future>(f: F) -> (F::Output, Arc<Waits>) {
    let waits = Arc::new(Waits::default());
    let out = CURRENT.scope(waits.clone(), f).await;
    (out, waits)
}

/// The waits of the request this task serves, to hand to its blocking work
/// (`as_turn`). `None` outside a measured request.
pub(super) fn current_waits() -> Option<Arc<Waits>> {
    CURRENT.try_with(Arc::clone).ok()
}

/// `tokio::task::spawn_blocking` for a request's work that is no `blocking`
/// or `in_turn`: it carries the waits too and is never called off.
pub(super) fn spawn_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> tokio::task::JoinHandle<T> {
    let waits = current_waits();
    // A flag nobody sets: this work is not called off.
    let never = Arc::new(AtomicBool::new(false));
    tokio::task::spawn_blocking(move || as_turn(never, waits, f))
}

/// This thread's request waited `d` for `what`.
pub(super) fn waited(what: Wait, d: Duration) {
    TURN.with(|t| {
        if let Some(waits) = t.borrow().as_ref().and_then(|t| t.waits.as_ref()) {
            let a = match what {
                Wait::Reader => &waits.reader,
                Wait::Writer => &waits.writer,
                Wait::People => &waits.people,
            };
            a.fetch_add(d.as_nanos() as u64, Ordering::SeqCst);
        }
    });
}

/// `lock()` (which waits), counted as a wait for `what`.
pub(super) fn timed<T>(what: Wait, lock: impl FnOnce() -> T) -> T {
    let began = Instant::now();
    let got = lock();
    waited(what, began.elapsed());
    got
}

/// The browser's request was gone before its work took a connection.
#[derive(Debug)]
pub(super) struct Abandoned;

impl std::fmt::Display for Abandoned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the browser no longer waits for this request")
    }
}

impl std::error::Error for Abandoned {}

/// Lives in the handler's future: dropping it (hyper drops the future when
/// the browser closes the connection) tells the work it is not wanted.
pub(super) struct Gone(Arc<AtomicBool>);

impl Gone {
    pub(super) fn new() -> Gone {
        Gone(Arc::new(AtomicBool::new(false)))
    }

    pub(super) fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

impl Drop for Gone {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Run `f` as the work of the request whose future holds `gone`, counting
/// into `waits` what it waits for.
pub(super) fn as_turn<T>(gone: Arc<AtomicBool>, waits: Option<Arc<Waits>>, f: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            TURN.with(|t| *t.borrow_mut() = None);
        }
    }
    TURN.with(|t| *t.borrow_mut() = Some(Turn { gone, started: false, waits }));
    let _clear = Clear;
    f()
}

/// The request takes a connection (a reader, the people view's): its first
/// one ends it if the browser has gone. Work that is no request's (background
/// threads) always goes on.
pub(super) fn started() -> Result<()> {
    TURN.with(|t| match t.borrow_mut().as_mut() {
        Some(turn) if !turn.started => {
            turn.started = true;
            if turn.gone.load(Ordering::SeqCst) {
                return Err(Abandoned.into());
            }
            Ok(())
        }
        _ => Ok(()),
    })
}

/// A change takes the writer: it runs, whether the browser waits or not.
pub(super) fn change_started() {
    TURN.with(|t| {
        if let Some(turn) = t.borrow_mut().as_mut() {
            turn.started = true;
        }
    });
}

/// Tests only (`serve::Options::hold`): `/api/test/hold-reader` takes a
/// reader, `/api/test/hold-writer` the writer, and keeps it until `release`,
/// as a long read or change would. One hold at a time; the next may start
/// once the last one has answered.
#[derive(Default)]
pub struct Hold {
    /// (a connection is held, it may go)
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Hold {
    /// Wait until the hold request has its connection, at most `limit`;
    /// false if it did not get there.
    pub fn wait_taken(&self, limit: Duration) -> bool {
        let s = self.state.lock().unwrap();
        let (s, _) = self.changed.wait_timeout_while(s, limit, |s| !s.0).unwrap();
        s.0
    }

    /// Let the hold request give its connection back and answer.
    pub fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }

    /// In the hold request, with its connection taken.
    pub(super) fn hold(&self) {
        let mut s = self.state.lock().unwrap();
        s.0 = true;
        self.changed.notify_all();
        while !s.1 {
            s = self.changed.wait(s).unwrap();
        }
        *s = (false, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(test: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("shoebox-readers-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db_path = dir.join(".shoebox").join("library.db");
        let conn = db::open_shared(&db_path).unwrap();
        thumbs::attach(&conn, &db_path).unwrap();
        recognize::attach(&conn, &db_path).unwrap();
        (dir, db_path)
    }

    #[test]
    fn a_write_on_a_reader_fails() {
        let (dir, db_path) = library("write");
        let readers = Readers::open(&db_path, 2).unwrap();
        let reader = readers.take().unwrap();
        for sql in [
            "INSERT INTO settings (key, value) VALUES ('x', 'y')",
            "INSERT INTO thumbs.thumbs (key, jpeg, made_at) VALUES ('k', NULL, 0)",
            "DELETE FROM recog.faces",
            "CREATE TABLE sneaky (x)",
        ] {
            let e = reader.execute(sql, []).expect_err(sql);
            assert!(format!("{e}").contains("readonly"), "{sql}: {e}");
        }
        // Reads work, on all three databases.
        let n: i64 = reader.query_row("SELECT (SELECT count(*) FROM settings) + (SELECT count(*) FROM thumbs.thumbs) + (SELECT count(*) FROM recog.faces)", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        drop(reader);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn readers_go_back_and_an_abandoned_request_does_not_start() {
        let (dir, db_path) = library("turn");
        let readers = Readers::open(&db_path, 2).unwrap();
        // Two at a time (in two threads: one thread holds one at most), and
        // each goes back.
        let a = readers.take().unwrap();
        std::thread::scope(|s| {
            s.spawn(|| drop(readers.take().unwrap()));
        });
        drop(a);
        assert_eq!(readers.idle.lock().unwrap().len(), 2);

        let gone = Gone::new();
        let flag = gone.flag();
        assert!(as_turn(flag.clone(), None, || readers.take().map(|_| ())).is_ok());
        drop(gone);
        let e = as_turn(flag.clone(), None, || readers.take().map(|_| ())).unwrap_err();
        assert!(e.is::<Abandoned>());
        // Once started, the request runs to its end; a change always runs.
        as_turn(flag.clone(), None, || {
            change_started();
            readers.take().unwrap();
        });
        // The people view's connection is checked the same way.
        assert!(as_turn(flag.clone(), None, started).unwrap_err().is::<Abandoned>());
        assert!(as_turn(Gone::new().flag(), None, started).is_ok());
        // Background work belongs to no request.
        assert!(readers.take().is_ok());
        assert_eq!(readers.idle.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn waits_are_counted_on_the_turn_of_the_request_only() {
        let waits = Arc::new(Waits::default());
        let never = || Arc::new(AtomicBool::new(false));
        as_turn(never(), Some(waits.clone()), || {
            waited(Wait::Reader, Duration::from_millis(5));
            waited(Wait::Reader, Duration::from_millis(7));
            waited(Wait::Writer, Duration::from_millis(3));
            assert_eq!(timed(Wait::People, || std::thread::sleep(Duration::from_millis(20))), ());
        });
        // Another turn, one without waits and work of no request's: not counted.
        as_turn(never(), None, || waited(Wait::Reader, Duration::from_secs(1)));
        waited(Wait::Reader, Duration::from_secs(1));
        let (reader, writer, people) = waits.totals();
        assert_eq!((reader, writer), (Duration::from_millis(12), Duration::from_millis(3)));
        assert!(people >= Duration::from_millis(20) && people < Duration::from_secs(1), "{people:?}");
    }

    #[test]
    fn a_reader_that_never_comes_back_is_an_error_not_a_hang() {
        let (dir, db_path) = library("wait");
        let readers = Readers { wait: Duration::from_millis(100), ..Readers::open(&db_path, 1).unwrap() };
        let ((taken, is_taken), (release, released)) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let pool = &readers;
        std::thread::scope(|s| {
            s.spawn(move || {
                let _r = pool.take().unwrap();
                taken.send(()).unwrap();
                let _ = released.recv();
            });
            is_taken.recv().unwrap();
            let e = pool.take().map(|_| ()).unwrap_err();
            assert!(format!("{e}").contains("no reader came free"), "{e}");
            release.send(()).unwrap();
        });
        assert!(readers.take().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
