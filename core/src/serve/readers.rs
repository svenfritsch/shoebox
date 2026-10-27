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

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::recognize;
use crate::thumbs;

/// How many readers a library gets: one per core, two to four.
pub(super) fn pool_size(cores: usize) -> usize {
    cores.clamp(2, 4)
}

pub(super) struct Readers {
    idle: Mutex<Vec<Connection>>,
    back: Condvar,
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
        Ok(Readers { idle: Mutex::new(idle), back: Condvar::new() })
    }

    /// A reader, as soon as one is free. Fails when it is the request's first
    /// connection and its browser has gone (`Abandoned`).
    pub(super) fn take(&self) -> Result<Reader<'_>> {
        assert_free("a reader");
        let mut idle = self.idle.lock().unwrap();
        let conn = loop {
            if let Some(conn) = idle.pop() {
                break conn;
            }
            idle = self.back.wait(idle).unwrap();
        };
        drop(idle);
        let reader = Reader { conn: Some(conn), pool: self, _held: Holding::new() };
        started()?;
        Ok(reader)
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

/// Run `f` as the work of the request whose future holds `gone`.
pub(super) fn as_turn<T>(gone: Arc<AtomicBool>, f: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            TURN.with(|t| *t.borrow_mut() = None);
        }
    }
    TURN.with(|t| *t.borrow_mut() = Some(Turn { gone, started: false }));
    let _clear = Clear;
    f()
}

/// The request takes a connection: its first one ends it if the browser has
/// gone. Work that is no request's (background threads) always goes on.
fn started() -> Result<()> {
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
    /// Wait until the hold request has its connection.
    pub fn wait_taken(&self) {
        let mut s = self.state.lock().unwrap();
        while !s.0 {
            s = self.changed.wait(s).unwrap();
        }
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
        assert!(as_turn(flag.clone(), || readers.take().map(|_| ())).is_ok());
        drop(gone);
        let e = as_turn(flag.clone(), || readers.take().map(|_| ())).unwrap_err();
        assert!(e.is::<Abandoned>());
        // Once started, the request runs to its end; a change always runs.
        as_turn(flag.clone(), || {
            change_started();
            readers.take().unwrap();
        });
        // Background work belongs to no request.
        assert!(readers.take().is_ok());
        assert_eq!(readers.idle.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
