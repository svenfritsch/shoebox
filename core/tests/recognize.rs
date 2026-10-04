//! `shoebox recognize` with the fake worker (`src/bin/shoebox-fake-recognizer.rs`):
//! the guard, what is stored, and the supervision (crashes, hangs, garbage,
//! broken workers). With `SHOEBOX_RECOGNIZER` set to the real
//! `recognizer/recognizer.py`, `real_recognizer_runs_under_the_guard` runs it
//! too.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use rusqlite::Connection;
use shoebox::recognize::{self, Failure, Timeouts, WorkerCommand};

const FAKE: &str = env!("CARGO_BIN_EXE_shoebox-fake-recognizer");

fn fake(args: &[&str]) -> WorkerCommand {
    WorkerCommand { program: PathBuf::from(FAKE), args: args.iter().map(Into::into).collect() }
}

fn quick() -> Timeouts {
    Timeouts { start: Duration::from_secs(10), reply: Duration::from_secs(2), exit: Duration::from_secs(2) }
}

fn options(lib: &Library) -> recognize::Options {
    recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(PathBuf::from(FAKE)),
        limit: None,
        retry_failed: false,
        timeouts: quick(),
    }
}

/// A library connection with `recognition.db` attached, as `run` opens it.
fn conn(lib: &Library) -> Connection {
    let path = lib.path(".shoebox/library.db");
    let conn = shoebox::db::open_shared(&path).unwrap();
    recognize::attach(&conn, &path).unwrap();
    conn
}

/// A library with nothing in it (the photos `Library::new` makes are removed).
fn empty(test: &str) -> Library {
    let lib = Library::new(test);
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
    }
    lib
}

fn solid(lib: &Library, rel: &str, rgb: [u8; 3]) {
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(200, 100, image::Rgb(rgb)).save(p).unwrap();
}

/// (found, error) stored for the file at an NFC path.
fn looked(lib: &Library, path_nfc: &str) -> Option<(i64, Option<String>)> {
    conn(lib)
        .query_row(
            "SELECT l.found, l.error FROM files f JOIN recog.looked l ON l.key = f.quick_hash
             WHERE f.path_nfc = ?1 AND l.task = 'faces'",
            [path_nfc],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
}

fn count(lib: &Library, sql: &str) -> i64 {
    conn(lib).query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn guard_recognize_leaves_originals_untouched() {
    let lib = Library::new("recog-guard");
    lib.scan();
    let before = lib.snapshot();

    let stats = recognize::run(&options(&lib)).unwrap();
    assert_eq!(lib.snapshot(), before, "recognize changed an original");
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    assert_eq!(stats.model, "fake-1");

    // Every photo content once; RAW files and videos are not looked at.
    let photos = lib.count(
        "SELECT count(DISTINCT quick_hash) FROM files WHERE missing_since IS NULL AND kind IN ('jpeg', 'png', 'heic')",
    );
    assert!(photos >= 6);
    assert_eq!(stats.looked as i64, photos);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked"), photos);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces"), stats.faces as i64);
    assert!(lib.path(".shoebox/recognition.db.bak").is_file());

    // Boxes are fractions of the picture; embeddings have the model's size.
    let (x, y, w, h, emb): (f64, f64, f64, f64, Vec<u8>) = conn(&lib)
        .query_row("SELECT x, y, w, h, emb FROM recog.faces LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap();
    assert!((x - 0.25).abs() < 0.01 && (y - 0.25).abs() < 0.01 && (w - 0.5).abs() < 0.01 && (h - 0.5).abs() < 0.01);
    assert_eq!(emb.len(), 128 * 4);

    // Resumable: a second run has nothing to do.
    let again = recognize::run(&options(&lib)).unwrap();
    assert_eq!((again.looked, again.failed), (0, 0));
    assert_eq!(lib.snapshot(), before);
    assert!(lib.verify(false).is_clean());
}

/// A JPEG called `.HEIC` (some exports keep that name) is read by its
/// content: size, thumbnail and faces, under the guard. Metadata and
/// thumbnails that failed before are tried again.
#[test]
fn files_named_after_another_format_are_read_by_content() {
    let lib = empty("recog-misnamed");
    lib.jpeg("2010er/IMG_0029.jpg", 7);
    std::fs::rename(lib.path("2010er/IMG_0029.jpg"), lib.path("2010er/IMG_0029.HEIC")).unwrap();
    lib.scan();
    let before = lib.snapshot();
    let thumbs = || Connection::open(lib.path(".shoebox/thumbs.db")).unwrap();
    let made = || -> i64 { thumbs().query_row("SELECT count(*) FROM thumbs WHERE jpeg IS NOT NULL", [], |r| r.get(0)).unwrap() };
    let meta = || -> (Option<i64>, Option<String>) {
        lib.db().query_row("SELECT width, meta_error FROM files", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    assert_eq!(meta(), (Some(320), None));
    assert_eq!(made(), 1);

    let stats = recognize::run(&options(&lib)).unwrap();
    assert_eq!((stats.looked, stats.failed), (1, 0), "{:?}", stats.errors);

    // As an older shoebox left it: no metadata, a failed thumbnail.
    lib.db().execute("UPDATE files SET width = NULL, height = NULL, meta_error = 'No ftyp box'", []).unwrap();
    thumbs().execute_batch("UPDATE thumbs SET jpeg = NULL, error = 'No ftyp box'; PRAGMA user_version = 1;").unwrap();
    lib.scan();
    assert_eq!(meta(), (Some(320), None));
    assert_eq!(made(), 1);
    assert_eq!(lib.snapshot(), before);
}

/// Ctrl-C while the worker is busy ends the run at once, marks it
/// interrupted (so the next run can start right away) and stops the worker.
#[cfg(unix)]
#[test]
fn ctrl_c_ends_the_run_and_frees_the_next_one() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let lib = empty("recog-ctrl-c");
    solid(&lib, "a/hangs.png", [0, 0, 255]); // the fake worker never answers
    lib.scan();
    let mut child = Command::new(env!("CARGO_BIN_EXE_shoebox"))
        .arg("recognize")
        .arg(&lib.root)
        .arg("--recognizer")
        .arg(FAKE)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    assert!(out.any(|l| l.unwrap().starts_with("Faces:")), "the run did not start");
    std::thread::sleep(Duration::from_millis(500));
    assert!(recognize::running(&conn(&lib)).unwrap());

    // SAFETY: plain kill(2) on our own child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) };
    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(stopped.elapsed() < Duration::from_secs(10), "Ctrl-C did not end the run");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(!status.success());
    assert!(err.contains("interrupted"), "{err}");
    assert!(!recognize::running(&conn(&lib)).unwrap());
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.jobs WHERE state = 'interrupted'"), 1);
}

#[test]
fn bad_pictures_and_misbehaving_workers_do_not_stop_a_run() {
    let lib = Library::new("recog-misbehave");
    solid(&lib, "Tests/dark.jpg", [0, 0, 0]);
    solid(&lib, "Tests/crash.jpg", [255, 0, 0]);
    solid(&lib, "Tests/error.jpg", [0, 255, 0]);
    solid(&lib, "Tests/hang.jpg", [0, 0, 255]);
    solid(&lib, "Tests/garbage.jpg", [255, 0, 255]);
    lib.write("Tests/broken.jpg", b"\xFF\xD8 not really a JPEG");
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();

    let stats = recognize::run(&options(&lib)).unwrap();
    assert_eq!(lib.snapshot(), before);
    assert_eq!(looked(&lib, "Tests/dark.jpg"), Some((0, None)));
    assert_eq!(looked(&lib, "Tests/garbage.jpg"), Some((1, None)));
    assert_eq!(looked(&lib, "Tests/error.jpg"), Some((0, Some("fake: cannot handle green".into()))));
    let crashed = looked(&lib, "Tests/crash.jpg").unwrap().1.unwrap();
    assert!(crashed.starts_with("recognizer crashed: the recognizer exited"), "{crashed}");
    let hung = looked(&lib, "Tests/hang.jpg").unwrap().1.unwrap();
    assert!(hung.contains("did not answer within 2 s"), "{hung}");
    // Decoding happens in shoebox; the worker never sees a broken file.
    assert!(looked(&lib, "Tests/broken.jpg").unwrap().1.is_some());
    assert_eq!(stats.failed, 4);
    // Two tries each for the crash and the hang; a worker is only started
    // again when the next picture comes (decoding order varies).
    assert!((3..=4).contains(&stats.restarts), "{}", stats.restarts);
    // The other photos all got their face.
    assert_eq!(looked(&lib, "Familie/Weihnachten/DSC_2001.jpg"), Some((1, None)));

    // Failures are not retried, unless asked.
    assert_eq!(recognize::run(&options(&lib)).unwrap().looked, 0);
    let retry = recognize::run(&recognize::Options { retry_failed: true, ..options(&lib) }).unwrap();
    assert_eq!((retry.looked, retry.failed), (0, 4));
}

#[test]
fn a_crash_is_retried_with_a_fresh_worker() {
    let lib = Library::new("recog-crash-once");
    solid(&lib, "Tests/crash.jpg", [255, 0, 0]);
    lib.scan_opts(true, false, false);
    let marker = lib.root.join("crashed-once");
    let conn = conn(&lib);
    let mut worker = recognize::Worker::start(fake(&["--crash-once", marker.to_str().unwrap()]), quick()).unwrap();
    let stats = recognize::recognize(&conn, &lib.root, &mut worker, None, false).unwrap();
    worker.stop();
    assert!(marker.exists());
    assert_eq!((stats.failed, stats.restarts), (0, 1));
    assert_eq!(looked(&lib, "Tests/crash.jpg"), Some((1, None)));
}

#[test]
fn a_worker_that_keeps_crashing_stops_the_run() {
    let lib = empty("recog-broken");
    for i in 0..4u8 {
        solid(&lib, &format!("Tests/crash{i}.jpg"), [255, i, 0]);
    }
    lib.scan_opts(true, false, false);
    let err = recognize::run(&options(&lib)).unwrap_err();
    assert!(format!("{err:#}").contains("failed 5 times in a row"), "{err:#}");
    let state: String =
        conn(&lib).query_row("SELECT state FROM recog.jobs ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(state, "failed");
    // What was decided before it gave up is kept.
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE error LIKE 'recognizer crashed%'"), 2);
}

#[test]
fn workers_that_do_not_start_are_reported() {
    let err = |cmd: WorkerCommand| format!("{:#}", recognize::Worker::start(cmd, quick()).err().unwrap());
    assert!(err(fake(&["--protocol", "99"])).contains("speaks protocol 99"));
    let silent = Timeouts { start: Duration::from_millis(500), ..quick() };
    let e = format!("{:#}", recognize::Worker::start(fake(&["--silent"]), silent).err().unwrap());
    assert!(e.contains("did not start within"), "{e}");
    let missing = WorkerCommand { program: PathBuf::from("/nonexistent/recognizer"), args: Vec::new() };
    assert!(err(missing).contains("cannot start the recognizer"));
    let gone = WorkerCommand { program: PathBuf::from("sh"), args: vec!["-c".into(), "exit 1".into()] };
    assert!(err(gone).contains("before it was ready"));

    // Nothing installed: a clear message, nothing written.
    let lib = Library::new("recog-none");
    lib.scan_opts(false, false, false);
    let opts = recognize::Options { recognizer: None, ..options(&lib) };
    if std::env::var_os("SHOEBOX_RECOGNIZER").is_none() {
        let e = format!("{:#}", recognize::run(&opts).unwrap_err());
        assert!(e.contains("face recognition is not installed"), "{e}");
        assert!(!lib.path(".shoebox/recognition.db").exists());
    }
}

#[test]
fn single_faces_through_the_worker() {
    let mut worker = recognize::Worker::start(fake(&[]), quick()).unwrap();
    assert_eq!(worker.faces_model().dim, 128);
    let jpeg = |rgb: [u8; 3]| {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(80, 40, image::Rgb(rgb))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    };
    let a = worker.faces(&jpeg([200, 150, 120])).unwrap().unwrap();
    let b = worker.faces(&jpeg([200, 150, 120])).unwrap().unwrap();
    assert_eq!((a.width, a.height, a.faces.len()), (80, 40, 1));
    let same: f32 = a.faces[0].emb.iter().zip(&b.faces[0].emb).map(|(x, y)| x * y).sum();
    assert!((same - 1.0).abs() < 1e-4);
    assert_eq!(a.faces[0].landmarks.len(), 5);
    assert_eq!(worker.faces(b"not an image").unwrap().unwrap_err(), Failure::Refused("cannot decode image".into()));
    worker.stop();
}

#[test]
fn copies_share_results_and_gone_files_are_pruned() {
    let lib = Library::new("recog-prune");
    solid(&lib, "A/one.jpg", [90, 120, 30]);
    std::fs::create_dir_all(lib.path("B")).unwrap();
    std::fs::copy(lib.path("A/one.jpg"), lib.path("B/copy.jpg")).unwrap();
    solid(&lib, "A/two.jpg", [30, 120, 90]);
    lib.scan_opts(true, false, false);
    let all = recognize::run(&options(&lib)).unwrap();
    assert_eq!(all.looked as i64, lib.count("SELECT count(DISTINCT quick_hash) FROM files WHERE kind != 'raw' AND kind != 'video'"));

    std::fs::remove_file(lib.path("A/two.jpg")).unwrap();
    std::fs::remove_file(lib.path("A/one.jpg")).unwrap();
    lib.scan_with(true, true);
    let stats = recognize::run(&options(&lib)).unwrap();
    assert_eq!((stats.looked, stats.pruned), (0, 1));
    assert_eq!(looked(&lib, "B/copy.jpg"), Some((1, None)));
}

#[test]
fn limit_and_another_model_redo() {
    let lib = Library::new("recog-limit");
    lib.scan_opts(true, false, false);
    let total = lib.count("SELECT count(DISTINCT quick_hash) FROM files WHERE kind IN ('jpeg', 'png', 'heic')");
    let first = recognize::run(&recognize::Options { limit: Some(2), ..options(&lib) }).unwrap();
    assert_eq!((first.looked as i64, first.pending as i64), (2, total - 2));
    let rest = recognize::run(&options(&lib)).unwrap();
    assert_eq!(rest.looked as i64, total - 2);

    // Results of another model are redone (embeddings cannot be compared).
    conn(&lib).execute("UPDATE recog.looked SET model = 'old'", []).unwrap();
    conn(&lib).execute("UPDATE recog.faces SET model = 'old'", []).unwrap();
    let redo = recognize::run(&options(&lib)).unwrap();
    assert_eq!(redo.looked as i64, total);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE model = 'old'"), 0);
}

#[test]
fn serve_reports_faces() {
    let lib = Library::new("recog-serve");
    solid(&lib, "Tests/dark.jpg", [0, 0, 0]);
    lib.scan();
    let server = start(&lib, None);
    let info = get(server.addr, "/api/info").json();
    assert_eq!(info["faces"]["done"], 0);
    assert_eq!(info["faces"]["running"], false);
    let id = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    assert!(get(server.addr, &format!("/api/files/{id}")).json()["faces"].is_null());

    let stats = recognize::run(&options(&lib)).unwrap();
    let info = get(server.addr, "/api/info").json();
    assert_eq!(info["faces"]["done"], info["faces"]["total"]);
    assert_eq!(info["faces"]["faces"], stats.faces);
    let faces = get(server.addr, &format!("/api/files/{id}")).json()["faces"].clone();
    assert_eq!(faces.as_array().unwrap().len(), 1);
    assert_eq!(faces[0]["x"], 0.25);
    assert!(faces[0].get("emb").is_none());
    let dark = id_of(&lib, "Tests/dark.jpg");
    assert_eq!(get(server.addr, &format!("/api/files/{dark}")).json()["faces"], serde_json::json!([]));
    server.stop().unwrap();
}

/// The Python worker, when `SHOEBOX_RECOGNIZER` points at it (with OpenCV and
/// the models installed): every photo is looked at and nothing changes.
#[test]
fn real_recognizer_runs_under_the_guard() {
    let Some(real) = std::env::var_os("SHOEBOX_RECOGNIZER").filter(|v| !v.is_empty()) else { return };
    let lib = Library::new("recog-real");
    if let Some(face) = std::env::var_os("SHOEBOX_TEST_FACE") {
        std::fs::copy(Path::new(&face), lib.path("Familie/face.jpg")).unwrap();
    }
    lib.scan();
    let before = lib.snapshot();
    let opts = recognize::Options {
        recognizer: Some(PathBuf::from(real)),
        timeouts: Timeouts::default(),
        ..options(&lib)
    };
    let stats = recognize::run(&opts).unwrap();
    assert_eq!(lib.snapshot(), before);
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    assert!(stats.looked >= 6);
    if std::env::var_os("SHOEBOX_TEST_FACE").is_some() {
        assert!(looked(&lib, "Familie/face.jpg").unwrap().0 >= 1);
    }
}
