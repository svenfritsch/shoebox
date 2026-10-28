//! The read-only connections of `shoebox serve` (`serve/readers.rs`): a long
//! read on one of them, or a long change on the writer, holds up no other
//! request, and every cache kept by the index's version sees a change made
//! through the API and a commit by another process (a scan) at once.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use shoebox::serve;

/// A server whose `/api/test/hold-…` routes hold a connection (`serve::Hold`).
fn start_holding(lib: &Library) -> (serve::Server, Arc<serve::Hold>) {
    start_holding_with(lib, None)
}

fn start_holding_with(lib: &Library, slow: Option<serve::SlowLog>) -> (serve::Server, Arc<serve::Hold>) {
    let hold = Arc::new(serve::Hold::default());
    let server = serve::start(
        &serve::Options {
            root: lib.root.clone(),
            more_roots: Vec::new(),
            db: None,
            port: 0,
            lan: false,
            pin: None,
            reveal: None,
            recognizer: Some(env!("CARGO_BIN_EXE_shoebox-fake-recognizer").into()),
            regroup: REGROUP_AT_ONCE,
            hold: Some(hold.clone()),
            slow,
        },
        false,
    )
    .unwrap();
    (server, hold)
}

/// A GET that must answer while the connection is held. On a thread of its
/// own, so a request stuck behind the held connection fails the test (after
/// a minute: no measure of speed, only of not waiting for the hold) instead
/// of hanging it.
fn answers(addr: SocketAddr, path: &str) -> Response {
    let (tx, rx) = mpsc::channel();
    let p = path.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(get(addr, &p));
    });
    rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("{path} waited behind the held connection"))
}

/// `during` while a request holds a reader (`what` = "reader") or the writer.
fn holding(addr: SocketAddr, hold: &serve::Hold, what: &str, during: impl FnOnce()) {
    struct Release<'a>(&'a serve::Hold);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let path = format!("/api/test/hold-{what}");
    let holder = std::thread::spawn(move || get(addr, &path));
    assert!(hold.wait_taken(Duration::from_secs(60)), "the hold request never got its {what}");
    {
        let _release = Release(hold);
        during();
    }
    let held = holder.join().unwrap();
    assert_eq!(held.status, 200, "{}", String::from_utf8_lossy(&held.body));
}

fn timeline_len(addr: SocketAddr) -> usize {
    ids(&get(addr, "/api/timeline").json()).len()
}

/// POST that must succeed.
fn ok(addr: SocketAddr, path: &str, body: &Value) -> Value {
    let r = post(addr, path, body);
    assert_eq!(r.status, 200, "{path} {body}: {}", String::from_utf8_lossy(&r.body));
    r.json()
}

#[test]
fn a_long_read_holds_up_no_other_request() {
    let lib = Library::new("readers-held");
    // No thumbnails yet: the first is made on request and stored on the writer.
    lib.scan_opts(false, false, false);
    let (server, hold) = start_holding(&lib);
    let addr = server.addr;
    let id = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    holding(addr, &hold, "reader", || {
        assert_eq!(answers(addr, "/api/groups").status, 200);
        assert_eq!(answers(addr, "/api/info").status, 200);
        // Made, stored on the writer; then read from thumbs.db.
        for _ in 0..2 {
            let thumb = answers(addr, &format!("/api/files/{id}/thumb"));
            assert_eq!((thumb.status, thumb.header("content-type")), (200, Some("image/jpeg")));
        }
    });
    server.stop().unwrap();
}

#[test]
fn a_long_change_holds_up_no_read() {
    let lib = Library::new("readers-writer");
    lib.scan();
    // One photo without a thumbnail: it is made while the change runs.
    let fresh = "2020-07 Urlaub Griechenland/IMG_0002.JPG";
    let thumbs = rusqlite::Connection::open(lib.path(".shoebox/thumbs.db")).unwrap();
    assert_eq!(thumbs.execute("DELETE FROM thumbs WHERE key = ?1", [lib.record_key(fresh)]).unwrap(), 1);
    let (server, hold) = start_holding(&lib);
    let addr = server.addr;
    let version = get(addr, "/api/info").json()["index_version"].clone();
    let before = timeline_len(addr);
    let thumb = id_of(&lib, "Ordner mit Leerzeichen/Bild 1.jpeg");
    let fresh = id_of(&lib, fresh);
    holding(addr, &hold, "writer", || {
        // A scan commits meanwhile.
        lib.db().execute("UPDATE files SET missing_since = 1 WHERE path_nfc = 'Familie/Weihnachten/DSC_2001.jpg'", []).unwrap();
        let info = answers(addr, "/api/info");
        assert_eq!(info.status, 200);
        // The version as the writer last told it: the change holding it moves
        // it when it is done.
        assert_eq!(info.json()["index_version"], version);
        assert_eq!(answers(addr, "/api/timeline").status, 200);
        assert_eq!(answers(addr, "/api/groups").status, 200);
        assert_eq!(answers(addr, &format!("/api/files/{thumb}/thumb")).status, 200);
        // Made, and sent without waiting to store it.
        let made = answers(addr, &format!("/api/files/{fresh}/thumb"));
        assert_eq!((made.status, made.header("content-type")), (200, Some("image/jpeg")));
    });
    // The first read after it sees what was committed meanwhile.
    assert_ne!(get(addr, "/api/info").json()["index_version"], version);
    assert_eq!(timeline_len(addr), before - 1);
    server.stop().unwrap();
}

/// A file a change is moving is renamed on disk before the change commits the
/// index (`organize`): a read in between finds it after the change, not a 404
/// and a search of the whole drive for it.
#[test]
fn a_file_being_moved_is_found_once_the_change_is_done() {
    let lib = Library::new("readers-moving");
    lib.scan();
    let (server, hold) = start_holding(&lib);
    let addr = server.addr;
    let (from, to) = ("Familie/Weihnachten/DSC_2001.jpg", "Familie/DSC_2001.jpg");
    let id = id_of(&lib, from);
    let (tx, rx) = mpsc::channel();
    holding(addr, &hold, "writer", || {
        // What a move does while it holds the writer: the file first, ...
        std::fs::rename(lib.path(from), lib.path(to)).unwrap();
        std::thread::spawn(move || {
            let _ = tx.send(get(addr, &format!("/api/files/{id}/view")));
        });
        // (only that it does not answer while the change runs)
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "answered before the change was done");
        // ... then the index.
        lib.db().execute("UPDATE files SET path = ?2, path_nfc = ?2 WHERE id = ?1", rusqlite::params![id, to]).unwrap();
    });
    let view = rx.recv_timeout(Duration::from_secs(60)).expect("the view answers once the change is done");
    assert_eq!(view.status, 200, "{}", String::from_utf8_lossy(&view.body));
    assert_eq!(view.body, std::fs::read(lib.path(to)).unwrap());
    server.stop().unwrap();
}

/// `/api/tags` as name → count.
fn tag_counts(addr: SocketAddr) -> std::collections::BTreeMap<String, u64> {
    get(addr, "/api/tags?limit=500")
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["count"].as_u64().unwrap()))
        .collect()
}

fn duplicate_groups(addr: SocketAddr) -> usize {
    get(addr, "/api/duplicates").json()["groups"].as_array().unwrap().len()
}

/// The timeline (`browse::Snapshot`), `/api/info`'s counts, the tags and the
/// duplicates are kept by the index's version.
#[test]
fn the_kept_files_see_every_change_at_once() {
    let lib = Library::new("readers-files");
    let _ = std::fs::remove_dir_all(lib.path("fixtures"));
    std::fs::create_dir_all(lib.path("Kopie")).unwrap();
    std::fs::copy(lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG"), lib.path("Kopie/IMG_0001.JPG")).unwrap();
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    let (hidden, tagged, gone) =
        (id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg"), id_of(&lib, "Ordner mit Leerzeichen/Bild 1.jpeg"), id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG"));
    let pair = [id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG"), id_of(&lib, "Kopie/IMG_0001.JPG")];
    let shown = timeline_len(addr);
    let info = get(addr, "/api/info").json();
    let (photos, missing) = (info["photos"].as_u64().unwrap(), info["missing"].as_u64().unwrap());
    assert_eq!(info["hidden"], 0);
    assert!(!tag_counts(addr).contains_key("Winter"));
    assert_eq!(duplicate_groups(addr), 1);

    // Through the API.
    ok(addr, "/api/hidden", &json!({ "ids": [hidden], "on": true }));
    assert_eq!(timeline_len(addr), shown - 1);
    assert_eq!(get(addr, "/api/info").json()["hidden"], 1);
    ok(addr, "/api/tags/add", &json!({ "ids": [tagged], "name": "Winter" }));
    assert_eq!(tag_counts(addr).get("Winter"), Some(&1));
    ok(addr, "/api/duplicates/decide", &json!({ "ids": pair, "decision": "linked" }));
    assert_eq!(duplicate_groups(addr), 0);

    // By another process.
    let other = lib.db();
    other.execute("UPDATE files SET missing_since = 1 WHERE id = ?1", [gone]).unwrap();
    assert_eq!(timeline_len(addr), shown - 2);
    let info = get(addr, "/api/info").json();
    assert_eq!((info["photos"].as_u64(), info["missing"].as_u64()), (Some(photos - 1), Some(missing + 1)));
    other.execute("DELETE FROM file_tags WHERE tag_id = (SELECT id FROM tags WHERE name = 'Winter')", []).unwrap();
    assert!(!tag_counts(addr).contains_key("Winter"));
    other.execute("DELETE FROM dup_decisions", []).unwrap();
    assert_eq!(duplicate_groups(addr), 1);
    server.stop().unwrap();
}

/// What makes a file the same photo as one on another drive (`multi::copy_facts`).
#[test]
fn the_kept_copies_see_every_change_at_once() {
    // Two drives of their own that share one photo (same name, same content).
    let make = |test: &str, seed: u8| {
        let lib = Library::new(test);
        let _ = std::fs::remove_dir_all(lib.path("fixtures"));
        lib.jpeg("2020-07 Urlaub Griechenland/IMG_0002.JPG", seed);
        lib.jpeg("Familie/Weihnachten/DSC_2001.jpg", seed + 1);
        lib.jpeg(&format!("{NFD_DIR}/IMG_0100.JPG"), seed + 2);
        lib.jpeg("Ordner mit Leerzeichen/Bild 1.jpeg", seed + 3);
        lib.scan_opts(true, false, false);
        lib
    };
    let (a, b) = (make("readers-copies-a", 10), make("readers-copies-b", 100));
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let libs = get(addr, "/api/libraries").json();
    let ia = libs[0]["id"].as_str().unwrap().to_string();
    let shared = id_of(&a, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let copies = || get(addr, &format!("/raw/api/all/copies?library={ia}&file={shared}")).json().as_array().unwrap().len();
    assert_eq!(copies(), 1);

    // By another process: another content is another photo, and back.
    let hash: String = a.db().query_row("SELECT full_hash FROM files WHERE id = ?1", [shared], |r| r.get(0)).unwrap();
    a.db().execute("UPDATE files SET full_hash = 'other' WHERE id = ?1", [shared]).unwrap();
    assert_eq!(copies(), 0);
    a.db().execute("UPDATE files SET full_hash = ?2 WHERE id = ?1", rusqlite::params![shared, hash]).unwrap();
    assert_eq!(copies(), 1);
    // Through the API: in the trash, it is no photo of the drive's.
    ok(addr, &format!("/raw/api/lib/{ia}/trash"), &json!({ "ids": [shared] }));
    assert_eq!(copies(), 0);
    server.stop().unwrap();
}

/// A plain picture of the person `rgb` for the fake recognizer (one face);
/// `n` makes the content unique.
fn plain(lib: &Library, rel: &str, rgb: [u8; 3], n: u8) {
    let mut img = image::RgbImage::from_pixel(200, 120, image::Rgb(rgb));
    img.put_pixel(0, 0, image::Rgb([n, n, n]));
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    img.save(p).unwrap();
}

fn wait_for_clusters(addr: SocketAddr) {
    let started = std::time::Instant::now();
    loop {
        let c = get(addr, "/api/info").json()["clusters"].clone();
        if c["stale"] == false && c["running"] == false && c["pending"] == false {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "clustering did not finish: {c}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The faces' counts in `/api/info` and the photos of each person in the
/// search box, on one drive and over all drives, are kept by the index's
/// version and the people view.
#[test]
fn the_kept_face_counts_see_every_change_at_once() {
    let lib = Library::new("readers-faces");
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
    }
    for n in 1..=4 {
        plain(&lib, &format!("Anna/a{n}.png"), [200, 40, 40], n);
    }
    lib.scan();
    let stats = shoebox::recognize::run(&shoebox::recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(env!("CARGO_BIN_EXE_shoebox-fake-recognizer").into()),
        limit: None,
        retry_failed: false,
        rotated: false,
        pets: false,
        text: false,
        text_only: false,
        timeouts: Default::default(),
    })
    .unwrap();
    assert_eq!((stats.failed, stats.faces), (0, 4), "{:?}", stats.errors);
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let file = |n: u8| id_of(&lib, &format!("Anna/a{n}.png"));
    for n in 1..=4 {
        let face = get(addr, &format!("/api/files/{}", file(n))).json()["faces"][0]["id"].clone();
        ok(addr, "/api/faces/assign", &json!({ "faces": [face], "name": "Anna" }));
    }
    wait_for_clusters(addr);
    let photos = || {
        let hits = get(addr, "/api/people/search?q=ann").json();
        let all = get(addr, "/raw/api/all/people?q=ann").json();
        let all = all["people"].as_array().unwrap().iter().find(|p| p["name"] == "Anna").unwrap()["photos"].as_u64();
        (hits[0]["photos"].as_u64(), all)
    };
    assert_eq!(photos(), (Some(4), Some(4)));
    let info = get(addr, "/api/info").json();
    assert_eq!((info["faces"]["faces"].as_u64(), info["clusters"]["people"].as_u64()), (Some(4), Some(1)));

    // Through the API: a photo hidden is not counted, a person made is.
    ok(addr, "/api/hidden", &json!({ "ids": [file(1)], "on": true }));
    assert_eq!(photos(), (Some(3), Some(3)));
    assert_eq!(get(addr, "/api/info").json()["clusters"]["people"], 1);
    ok(addr, "/api/people", &json!({ "name": "Ben" }));
    assert_eq!(get(addr, "/api/info").json()["clusters"]["people"], 2);

    // By another process: a scan misses a photo, `recognize` forgets one.
    let other = lib.db();
    other.execute("UPDATE files SET missing_since = 1 WHERE id = ?1", [file(2)]).unwrap();
    assert_eq!(photos(), (Some(2), Some(2)));
    assert_eq!(get(addr, "/api/info").json()["faces"]["done"], 3);
    other.execute("ATTACH DATABASE ?1 AS recog", [lib.path(".shoebox/recognition.db").to_str().unwrap()]).unwrap();
    let key = lib.record_key("Anna/a3.png");
    other.execute("DELETE FROM recog.looked WHERE key = ?1", [&key]).unwrap();
    assert_eq!(get(addr, "/api/info").json()["faces"]["done"], 2);
    server.stop().unwrap();
}

// ---------------------------------------------------------------- slow-request log

type Lines = Arc<std::sync::Mutex<Vec<String>>>;

/// A server whose slow-request log (`serve::SlowLog`) keeps its lines for the
/// test, for requests over `limit`.
fn start_logging(lib: &Library, limit: Duration) -> (serve::Server, Arc<serve::Hold>, Lines) {
    let lines = Lines::default();
    let sink = lines.clone();
    let log = serve::SlowLog::new(limit, move |line| sink.lock().unwrap().push(line.to_string()));
    let (server, hold) = start_holding_with(lib, Some(log));
    (server, hold, lines)
}

/// The milliseconds a line says the request waited for `what` ("a reader",
/// "the writer", "the people data").
fn waited_ms(line: &str, what: &str) -> u64 {
    let rest = line.split_once(&format!("{what} ")).unwrap_or_else(|| panic!("no {what} in {line}")).1;
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
}

/// The one line of the log about the request whose target ends with `tail`.
fn line_about(lines: &Lines, tail: &str) -> String {
    let lines = lines.lock().unwrap();
    let mine: Vec<&String> = lines.iter().filter(|l| l.split(" took ").next().unwrap().ends_with(tail)).collect();
    assert_eq!(mine.len(), 1, "one line about {tail}, in {lines:?}");
    mine[0].clone()
}

/// A request held past the limit by what it does itself (here: it holds a
/// reader) says what it is, how long it took and that it waited for nothing.
#[test]
fn a_request_held_past_the_limit_logs_one_line() {
    let lib = Library::new("slow-held");
    lib.scan();
    let (server, hold, lines) = start_logging(&lib, Duration::from_millis(100));
    let addr = server.addr;
    holding(addr, &hold, "reader", || {
        // Time must pass for the request to be slow.
        std::thread::sleep(Duration::from_millis(250));
        assert!(lines.lock().unwrap().is_empty(), "logged before the request was done");
    });
    let line = line_about(&lines, "/test/hold-reader");
    assert_eq!(lines.lock().unwrap().len(), 1);
    assert!(line.starts_with("slow request: GET /api/lib/"), "{line}");
    let took: u64 = line.split(" took ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
    assert!(took >= 250, "{line}");
    assert_eq!((waited_ms(&line, "a reader"), waited_ms(&line, "the writer"), waited_ms(&line, "the people data")), (0, 0, 0), "{line}");
    server.stop().unwrap();
}

/// A request that waits for a connection says for which, and how long: a
/// reader when the pool is held, the writer behind a change, the people data's
/// lock behind a slow reading of the people view. The query stays in the path.
#[test]
fn a_request_logs_what_it_waited_for() {
    let lib = Library::new("slow-waits");
    lib.scan();
    let (server, hold, lines) = start_logging(&lib, Duration::from_millis(100));
    let addr = server.addr;
    for (held, send, tail, label) in [
        ("all-readers", "GET", "/groups?x=1", "a reader"),
        ("writer", "POST", "/people", "the writer"),
        ("people", "GET", "/people?hidden=1", "the people data"),
    ] {
        let target = tail.to_string();
        let mut waiting = None;
        holding(addr, &hold, held, || {
            waiting = Some(std::thread::spawn(move || {
                if send == "POST" { post(addr, "/api/people", &json!({ "name": "Slow" })) } else { get(addr, &format!("/api{target}")) }
            }));
            // Time must pass for it to wait.
            std::thread::sleep(Duration::from_millis(300));
        });
        let reply = waiting.take().unwrap().join().unwrap();
        assert_eq!(reply.status, 200, "{held}: {}", String::from_utf8_lossy(&reply.body));
        let line = line_about(&lines, tail);
        assert!(line.starts_with(&format!("slow request: {send} /api/lib/")), "{line}");
        assert!(waited_ms(&line, label) >= 200, "{held}: {line}");
        for other in ["a reader", "the writer", "the people data"].into_iter().filter(|o| *o != label) {
            assert!(waited_ms(&line, other) < 100, "{held}: {line}");
        }
    }
    server.stop().unwrap();
}

/// Requests within the limit leave no line, however many.
#[test]
fn a_quick_request_logs_nothing() {
    let lib = Library::new("slow-quick");
    lib.scan();
    let (server, _hold, lines) = start_logging(&lib, Duration::from_secs(30));
    let addr = server.addr;
    for path in ["/api/info", "/api/groups", "/api/timeline", "/api/people", "/raw/api/all/people"] {
        assert_eq!(get(addr, path).status, 200, "{path}");
    }
    let seen = lines.lock().unwrap().clone();
    assert!(seen.is_empty(), "{seen:?}");
    server.stop().unwrap();
}
