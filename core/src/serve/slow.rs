//! The slow-request log: one line on stderr for every library request
//! (`/api/lib/<id>/…`, `/api/all/…`) that takes over a second, saying where
//! the time went: working, or waiting for a reader, the writer or the people
//! data's lock (`readers::Waits`). The incident behind it (a Face check
//! request that held up the whole app for minutes) would have shown as one
//! line for that request and one per request behind it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use super::readers;

/// How slow is slow, and where the line goes.
#[derive(Clone)]
pub struct SlowLog {
    pub limit: Duration,
    pub print: Arc<dyn Fn(&str) + Send + Sync>,
}

impl SlowLog {
    /// Tests: what takes longer than `limit` is handed to `print` instead of
    /// stderr (`serve::Options::slow`).
    pub fn new(limit: Duration, print: impl Fn(&str) + Send + Sync + 'static) -> SlowLog {
        SlowLog { limit, print: Arc::new(print) }
    }
}

impl Default for SlowLog {
    /// Over a second, on stderr.
    fn default() -> SlowLog {
        SlowLog::new(Duration::from_secs(1), |line| eprintln!("{line}"))
    }
}

/// Library requests only; the event stream stays open on purpose.
fn counts(path: &str) -> bool {
    let tail = path.strip_prefix("/api/lib/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
    (path.starts_with("/api/all/") || tail.is_some()) && tail != Some("events")
}

/// Time the handler until it returns its response; a file or video that
/// streams afterwards is not part of it.
pub(super) async fn log(State(log): State<SlowLog>, req: Request, next: Next) -> Response {
    if !counts(req.uri().path()) {
        return next.run(req).await;
    }
    let (method, target) = (req.method().clone(), req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default());
    let began = Instant::now();
    let (res, waits) = readers::measure(next.run(req)).await;
    let took = began.elapsed();
    if took > log.limit {
        let (reader, writer, people) = waits.totals();
        let ms = |d: Duration| d.as_millis();
        (log.print)(&format!(
            "slow request: {method} {target} took {} ms (waited for a reader {} ms, the writer {} ms, the people data {} ms)",
            ms(took),
            ms(reader),
            ms(writer),
            ms(people)
        ));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_library_requests_count() {
        assert!(counts("/api/lib/ab12/faces"));
        assert!(counts("/api/all/people"));
        assert!(!counts("/api/lib/ab12/events"));
        assert!(!counts("/api/libraries"));
        assert!(!counts("/api/session"));
        assert!(!counts("/index.html"));
    }
}
