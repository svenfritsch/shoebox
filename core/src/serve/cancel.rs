//! Calling off work for a request the browser gave up on (the page was left,
//! the filter changed, the tab closed).
//!
//! When the browser drops a request, the server drops the handler's future,
//! but the work already handed to a blocking thread runs on and holds the
//! database connection until it is done. So each request gets a token
//! (`CallOff`): it is set when the future is dropped. The blocking thread
//! looks at it before it starts (`run`), and a connection taken for the
//! request (`Armed`) lets SQLite stop the statement at its next step.
//!
//! Work that is shared and kept (the snapshot, the people view) must not stop
//! halfway for one request: the next one would start it again and a quick
//! succession of filters would never finish it. Those run `unarmed`.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::Connection;

/// SQLite virtual machine steps between two looks at the token.
const STEPS: i32 = 2000;

thread_local! {
    static CURRENT: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Sets the token when dropped: the request's future is gone (or done).
pub(super) struct CallOff(pub(super) Arc<AtomicBool>);

impl CallOff {
    pub(super) fn new() -> CallOff {
        CallOff(Arc::new(AtomicBool::new(false)))
    }
}

impl Drop for CallOff {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Runs `f` on this thread as the work of the request with this token.
/// `None` when the request was called off before it began.
pub(super) fn run<T>(token: Arc<AtomicBool>, f: impl FnOnce() -> T) -> Option<T> {
    if token.load(Ordering::SeqCst) {
        return None;
    }
    // Put back on drop, also if `f` panics: the thread goes back to the pool and
    // must not keep the token of a request that is over (it would stop the next
    // request's writes).
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.replace(Some(token))));
    Some(f())
}

/// The token of the request this thread works for, if any.
pub(super) fn current() -> Option<Arc<AtomicBool>> {
    CURRENT.with(|c| c.borrow().clone())
}

pub(super) fn is_called_off() -> bool {
    current().is_some_and(|t| t.load(Ordering::SeqCst))
}

thread_local! {
    /// Connections (by address) that carry the handler on this thread.
    static ARMED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn addr(conn: &Connection) -> usize {
    conn as *const Connection as usize
}

pub(super) fn arm(conn: &Connection) {
    if let Some(token) = current() {
        let _ = conn.progress_handler(STEPS, Some(move || token.load(Ordering::SeqCst)));
        ARMED.with(|a| {
            let mut a = a.borrow_mut();
            if !a.contains(&addr(conn)) {
                a.push(addr(conn));
            }
        });
    }
}

pub(super) fn disarm(conn: &Connection) {
    let _ = conn.progress_handler(STEPS, None::<fn() -> bool>);
    ARMED.with(|a| a.borrow_mut().retain(|x| *x != addr(conn)));
}

/// The connection while the request uses it: a statement stops when the
/// request is called off. Takes the handler off again when dropped.
pub(super) struct Armed<'a>(&'a Connection);

impl<'a> Armed<'a> {
    pub(super) fn new(conn: &'a Connection) -> Armed<'a> {
        arm(conn);
        Armed(conn)
    }
}

impl Drop for Armed<'_> {
    fn drop(&mut self) {
        disarm(self.0);
    }
}

/// Puts the handler back on drop, only if the connection had it.
struct Rearm<'a> {
    conn: &'a Connection,
    was_armed: bool,
}

impl Drop for Rearm<'_> {
    fn drop(&mut self) {
        if self.was_armed {
            arm(self.conn);
        }
    }
}

/// `f` (building something that is kept for everyone) without the handler.
/// What the connection had before is back afterwards, also if `f` panics.
pub(super) fn unarmed<T>(conn: &Connection, f: impl FnOnce() -> T) -> T {
    let was_armed = ARMED.with(|a| a.borrow().contains(&addr(conn)));
    disarm(conn);
    let _back = Rearm { conn, was_armed };
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000000) SELECT count(*) FROM n",
            [],
            |r| r.get(0),
        )
    }

    #[test]
    fn a_called_off_request_does_not_begin() {
        let token = Arc::new(AtomicBool::new(true));
        assert!(run(token, || unreachable!()).is_none());
    }

    #[test]
    fn a_statement_stops_when_the_token_is_set() {
        let conn = Connection::open_in_memory().unwrap();
        let token = Arc::new(AtomicBool::new(false));
        // Set from inside the work, as the dropped future does from another thread.
        let t = token.clone();
        let out = run(token, || {
            let _armed = Armed::new(&conn);
            t.store(true, Ordering::SeqCst);
            busy(&conn)
        })
        .unwrap();
        let err = out.unwrap_err();
        assert!(err.to_string().contains("interrupted"), "{err}");
        // Off again afterwards: the connection works for the next request.
        assert_eq!(conn.query_row("SELECT 7", [], |r| r.get::<_, i64>(0)).unwrap(), 7);
    }

    #[test]
    fn unarmed_work_finishes_and_the_handler_comes_back() {
        let conn = Connection::open_in_memory().unwrap();
        let token = Arc::new(AtomicBool::new(false));
        let out = run(token.clone(), || {
            let _armed = Armed::new(&conn);
            token.store(true, Ordering::SeqCst);
            let kept = unarmed(&conn, || busy(&conn));
            (kept, busy(&conn))
        })
        .unwrap();
        assert_eq!(out.0.unwrap(), 5_000_000);
        assert!(out.1.is_err());
    }

    #[test]
    fn unarmed_leaves_a_connection_without_the_handler_as_it_was() {
        let conn = Connection::open_in_memory().unwrap();
        let token = Arc::new(AtomicBool::new(false));
        let out = run(token.clone(), || {
            // Not armed: nothing is switched on afterwards.
            unarmed(&conn, || ());
            token.store(true, Ordering::SeqCst);
            busy(&conn)
        })
        .unwrap();
        assert_eq!(out.unwrap(), 5_000_000);
    }

    #[test]
    fn the_handler_is_back_after_unarmed_work_panics() {
        let conn = Connection::open_in_memory().unwrap();
        let token = Arc::new(AtomicBool::new(false));
        let out = run(token.clone(), || {
            let _armed = Armed::new(&conn);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unarmed(&conn, || panic!("boom"))));
            assert!(r.is_err());
            token.store(true, Ordering::SeqCst);
            busy(&conn)
        })
        .unwrap();
        assert!(out.is_err());
    }

    #[test]
    fn the_token_is_gone_from_the_thread_after_a_panic() {
        let token = Arc::new(AtomicBool::new(false));
        let r = std::panic::catch_unwind(|| run(token, || panic!("boom")));
        assert!(r.is_err());
        assert!(current().is_none(), "the next request on this thread would inherit the token");
    }
}
