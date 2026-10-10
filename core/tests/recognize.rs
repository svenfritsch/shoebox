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
        rotated: false,
        pets: false,
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

/// A grey picture whose left part is `rgb`: a cue for the fake worker that
/// only shows when the picture is turned (see its header).
fn edged(lib: &Library, rel: &str, rgb: [u8; 3]) {
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(300, 300, |x, _| image::Rgb(if x < 90 { rgb } else { [128, 128, 128] })).save(p).unwrap();
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
    let lib = empty("recog-ctrl-c");
    solid(&lib, "a/hangs.png", [0, 0, 255]); // the fake worker never answers
    lib.scan();
    interrupt_run(&lib, &[], "Faces:", "faces");
}

/// The same for the rotated pass: the upright result is kept, the rotated
/// job is marked interrupted.
#[cfg(unix)]
#[test]
fn ctrl_c_ends_the_rotated_pass_too() {
    let lib = empty("recog-ctrl-c-rot");
    edged(&lib, "a/hangs-turned.jpg", [255, 255, 0]); // answered upright, hangs once turned
    lib.scan();
    interrupt_run(&lib, &["--rotated"], "Faces, turned", "faces-rot");
    assert_eq!(looked(&lib, "a/hangs-turned.jpg"), Some((1, None)));
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'faces-rot'"), 0);
}

/// Start `shoebox recognize` with `args`, wait for the line starting with
/// `started`, press Ctrl-C and check the run ended cleanly with its `job`
/// marked interrupted.
#[cfg(unix)]
fn interrupt_run(lib: &Library, args: &[&str], started: &str, job: &str) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut child = Command::new(env!("CARGO_BIN_EXE_shoebox"))
        .arg("recognize")
        .arg(&lib.root)
        .arg("--recognizer")
        .arg(FAKE)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    assert!(out.any(|l| l.unwrap().starts_with(started)), "the run did not start");
    std::thread::sleep(Duration::from_millis(500));
    assert!(recognize::running(&conn(lib)).unwrap());

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
    assert!(!recognize::running(&conn(lib)).unwrap());
    let interrupted = conn(lib)
        .query_row("SELECT count(*) FROM recog.jobs WHERE state = 'interrupted' AND kind = ?1", [job], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(interrupted, 1);
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

/// Protocol 2's `embed`: one embedding per box drawn by hand, in order,
/// with the landmarks found in it (none: the plain crop was embedded). A
/// worker without the task refuses, and the run goes on.
#[test]
fn embed_task_of_protocol_2() {
    let mut img = image::RgbImage::from_pixel(200, 100, image::Rgb([200, 150, 120]));
    for x in 100..200 {
        for y in 0..100 {
            img.put_pixel(x, y, image::Rgb([128, 128, 128]));
        }
    }
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Png).unwrap();
    let boxes = [[10.0, 10.0, 60.0, 80.0], [120.0, 10.0, 60.0, 80.0]];
    let mut worker = recognize::Worker::start(fake(&[]), quick()).unwrap();
    let found = worker.embed(&jpeg, &boxes).unwrap().unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].landmarks.len(), 5);
    assert!(found[0].landmarks.iter().all(|[x, y]| (0.05..0.35).contains(x) && (0.1..0.9).contains(y)), "fractions in the box");
    assert!(found[1].landmarks.is_empty(), "grey: no landmarks");
    // Like a detected face of a plain picture of that colour.
    let plain = image::RgbImage::from_pixel(200, 100, image::Rgb([200, 150, 120]));
    let mut plain_png = Vec::new();
    image::DynamicImage::ImageRgb8(plain).write_to(&mut std::io::Cursor::new(&mut plain_png), image::ImageFormat::Png).unwrap();
    let face = worker.faces(&plain_png).unwrap().unwrap().faces.remove(0);
    let sim: f32 = face.emb.iter().zip(&found[0].emb).map(|(a, b)| a * b).sum();
    assert!(sim > 0.999, "{sim}");
    worker.stop();

    let mut old = recognize::Worker::start(fake(&["--no-embed"]), quick()).unwrap();
    assert!(matches!(old.embed(&jpeg, &boxes).unwrap(), Err(Failure::Refused(e)) if e.contains("cannot embed")));
    assert!(old.faces(&plain_png).unwrap().is_ok(), "still finds faces");
    old.stop();
}

#[test]
fn workers_that_do_not_start_are_reported() {
    let err = |cmd: WorkerCommand| format!("{:#}", recognize::Worker::start(cmd, quick()).err().unwrap());
    assert!(err(fake(&["--protocol", "99"])).contains("speaks protocol 99"));
    // A worker of protocol 1 (before `embed`) is not taken: update both together.
    assert!(err(fake(&["--protocol", "1"])).contains("speaks protocol 1, this shoebox 2"));
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
    assert_eq!(worker.faces_model().unwrap().dim, 128);
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

/// A photo of a cat (red at least as strong as blue) or a dog, for the fake
/// worker's `pets` task.
const CAT: [u8; 3] = [200, 60, 40];
const DOG: [u8; 3] = [40, 60, 200];

#[test]
fn pets_are_their_own_pass_next_to_the_faces() {
    let lib = empty("recog-pets");
    solid(&lib, "Pets/cat.jpg", CAT);
    solid(&lib, "Pets/dog.jpg", DOG);
    solid(&lib, "Pets/night.jpg", [5, 5, 5]);
    solid(&lib, "Pets/bad.jpg", [10, 250, 10]);
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();

    // Without the flag nothing is looked for: no pet models are loaded.
    let faces_only = recognize::run(&options(&lib)).unwrap();
    assert!(faces_only.pets.is_none());
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'pets'"), 0);
    let faces = count(&lib, "SELECT count(*) FROM recog.faces");
    assert_eq!(faces, 2, "the cat's and the dog's picture each have a (fake) face");

    let pets_on = recognize::Options { pets: true, ..options(&lib) };
    let stats = recognize::run(&pets_on).unwrap();
    assert_eq!(lib.snapshot(), before, "the pets pass changed an original");
    assert_eq!((faces_only.looked, stats.looked), (3, 0), "the faces are not looked at again");
    let a = stats.pets.expect("the pets pass ran");
    assert_eq!((a.looked, a.faces, a.failed), (3, 2, 1), "{:?}", a.errors);
    assert_eq!(a.model, "fake-pets-1");

    // Stored with the pet model and its own embedding length; the faces stay.
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"), faces);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species = 'cat'"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species = 'dog'"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL AND model != 'fake-pets-1'"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL AND length(emb) != 64 * 4"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL AND length(emb) != 128 * 4"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'pets'"), 4);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'pets' AND error IS NOT NULL"), 1);
    let (x, w, score): (f64, f64, f64) = conn(&lib)
        .query_row("SELECT x, w, score FROM recog.faces WHERE species = 'cat'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert!((x - 0.25).abs() < 0.01 && (w - 0.5).abs() < 0.01 && score > 0.0);

    // Resumable: nothing to do the second time (the failed one waits for --retry-failed).
    let again = recognize::run(&pets_on).unwrap();
    let a = again.pets.expect("ran");
    assert_eq!((a.looked, a.failed), (0, 0));
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 2);

    // The viewer's faces of a photo include the pet (boxes in the viewer).
    let cat = recognize::faces_of(&conn(&lib), &lib.record_key("Pets/cat.jpg")).unwrap().unwrap();
    assert_eq!(cat.iter().filter_map(|f| f.species.as_deref()).collect::<Vec<_>>(), ["cat"]);
    assert_eq!(cat.len(), 2, "its face and the cat");
    assert!(lib.verify(false).is_clean());
}

#[test]
fn a_model_change_redoes_only_its_own_pass() {
    let lib = empty("recog-pets-models");
    solid(&lib, "Pets/cat.jpg", CAT);
    solid(&lib, "Pets/dog.jpg", DOG);
    lib.scan_opts(true, false, false);
    let opts = recognize::Options { pets: true, ..options(&lib) };
    recognize::run(&opts).unwrap();
    let (faces, pets) = (
        count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"),
        count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"),
    );
    assert_eq!((faces, pets), (2, 2));

    // Another pet model: the pets are looked for again, the faces are not.
    conn(&lib).execute("UPDATE recog.looked SET model = 'old' WHERE task = 'pets'", []).unwrap();
    conn(&lib).execute("UPDATE recog.faces SET model = 'old' WHERE species IS NOT NULL", []).unwrap();
    let redo = recognize::run(&opts).unwrap();
    assert_eq!(redo.looked, 0);
    assert_eq!(redo.pets.unwrap().looked, 2);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE model = 'old'"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 2);

    // Another face model: the faces are redone and the pets stay.
    conn(&lib).execute("UPDATE recog.looked SET model = 'old' WHERE task = 'faces'", []).unwrap();
    conn(&lib).execute("UPDATE recog.faces SET model = 'old' WHERE species IS NULL", []).unwrap();
    let redo = recognize::run(&opts).unwrap();
    assert_eq!((redo.looked, redo.pets.unwrap().looked), (2, 0));
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"), 2);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 2);

    // `--limit` counts per pass.
    let lib = empty("recog-pets-limit");
    for i in 0..4u8 {
        solid(&lib, &format!("Pets/{i}.jpg"), [200, 60 + i * 40, 40]);
    }
    lib.scan_opts(true, false, false);
    let first = recognize::run(&recognize::Options { pets: true, limit: Some(3), ..options(&lib) }).unwrap();
    assert_eq!((first.looked, first.pending), (3, 1));
    let a = first.pets.unwrap();
    assert_eq!((a.looked, a.pending), (3, 1));
}

/// The add-ons stand alone: with only the pet models installed the worker
/// has no faces, finds cats and dogs, and the face passes say what is missing.
#[test]
fn pets_work_without_the_face_models() {
    let lib = empty("recog-pets-only");
    solid(&lib, "Pets/cat.jpg", CAT);
    lib.scan_opts(true, false, false);
    let conn = conn(&lib);
    let mut worker = recognize::Worker::start(fake(&["--pets", "--no-faces"]), quick()).unwrap();
    assert!(worker.faces_model().is_none());
    assert_eq!(worker.pets_model().map(|a| a.model.as_str()), Some("fake-pets-1"));
    let e = format!("{:#}", recognize::recognize(&conn, &lib.root, &mut worker, None, false).unwrap_err());
    assert!(e.contains("Faces add-on is not installed"), "{e}");
    let stats = recognize::recognize_pets(&conn, &lib.root, &mut worker, None, false).unwrap();
    assert_eq!((stats.looked, stats.faces), (1, 1), "{:?}", stats.errors);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species = 'cat'"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"), 0);
    worker.stop();
    // A worker with neither add-on is refused.
    assert!(recognize::Worker::start(fake(&["--no-faces"]), quick()).is_err());
}

#[test]
fn the_pets_task_needs_a_worker_started_for_it() {
    // Without --pets the hello has no pets and the core refuses to ask.
    let mut plain = recognize::Worker::start(fake(&[]), quick()).unwrap();
    assert!(plain.pets_model().is_none());
    let jpeg = |rgb: [u8; 3]| {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(80, 40, image::Rgb(rgb))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    };
    assert!(matches!(plain.pets(&jpeg(CAT)).unwrap(), Err(Failure::Refused(e)) if e.contains("not started for pets")));
    plain.stop();

    let mut worker = recognize::Worker::start(fake(&["--pets"]), quick()).unwrap();
    let info = worker.pets_model().unwrap().clone();
    assert_eq!((info.model.as_str(), info.dim), ("fake-pets-1", 64));
    assert_eq!(worker.faces_model().unwrap().dim, 128, "faces keep their own model");
    let cat = worker.pets(&jpeg(CAT)).unwrap().unwrap();
    let dog = worker.pets(&jpeg(DOG)).unwrap().unwrap();
    assert_eq!((cat.width, cat.height, cat.faces.len()), (80, 40, 1));
    assert_eq!(cat.faces[0].species.as_deref(), Some("cat"));
    assert_eq!(dog.faces[0].species.as_deref(), Some("dog"));
    assert_eq!(cat.faces[0].emb.len(), 64);
    assert!(cat.faces[0].landmarks.is_empty());
    let same = worker.pets(&jpeg(CAT)).unwrap().unwrap();
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    assert!((dot(&cat.faces[0].emb, &same.faces[0].emb) - 1.0).abs() < 1e-4, "same colour, same pet");
    assert!(dot(&cat.faces[0].emb, &dog.faces[0].emb) < 0.9);
    assert!(worker.pets(&jpeg([5, 5, 5])).unwrap().unwrap().faces.is_empty());
    assert_eq!(
        worker.pets(&jpeg([10, 250, 10])).unwrap().unwrap_err(),
        Failure::Refused("fake: cannot handle green".into())
    );
    // Faces still work on the same worker.
    assert_eq!(worker.faces(&jpeg(CAT)).unwrap().unwrap().faces.len(), 1);
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
        // The same face lying down, for the rotated pass.
        image::open(Path::new(&face)).unwrap().rotate90().save(lib.path("Familie/face-lying.jpg")).unwrap();
    }
    lib.scan();
    let before = lib.snapshot();
    let opts = recognize::Options {
        recognizer: Some(PathBuf::from(real)),
        rotated: true,
        timeouts: Timeouts::default(),
        ..options(&lib)
    };
    let stats = recognize::run(&opts).unwrap();
    assert_eq!(lib.snapshot(), before);
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    assert!(stats.looked >= 6);
    let rotated = stats.rotated.unwrap();
    assert_eq!((rotated.looked, rotated.failed), (stats.looked, 0), "{:?}", rotated.errors);
    if std::env::var_os("SHOEBOX_TEST_FACE").is_some() {
        assert!(looked(&lib, "Familie/face.jpg").unwrap().0 >= 1);
        let faces = |path: &str, rolled: bool| -> i64 {
            conn(&lib)
                .query_row(
                    "SELECT count(*) FROM recog.faces r JOIN files f ON f.quick_hash = r.key
                     WHERE f.path_nfc = ?1 AND (r.roll != 0) = ?2",
                    rusqlite::params![path, rolled],
                    |r| r.get(0),
                )
                .unwrap()
        };
        // The upright face is not added again from the turned copies; the
        // lying one is found (by the rotated pass: YuNet misses it upright).
        assert_eq!(faces("Familie/face.jpg", true), 0);
        assert!(faces("Familie/face-lying.jpg", false) + faces("Familie/face-lying.jpg", true) >= 1);

        // Faces drawn by hand: one around the detected face (a little off),
        // aligned by the landmarks found in it and close to the detected
        // face's embedding; one on the background, the plain crop.
        let c = conn(&lib);
        let (x, y, w, h, emb): (f64, f64, f64, f64, Vec<u8>) = c
            .query_row(
                "SELECT r.x, r.y, r.w, r.h, r.emb FROM recog.faces r JOIN files f ON f.quick_hash = r.key
                 WHERE f.path_nfc = 'Familie/face.jpg' ORDER BY r.w DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        let file = id_of(&lib, "Familie/face.jpg");
        let who = shoebox::people::Who { person_id: None, name: Some("Face".into()) };
        shoebox::people::add_manual(&c, file, [x - 0.05 * w, y + 0.03 * h, w * 1.1, h], &who, None).unwrap();
        shoebox::people::add_manual(&c, file, [0.0, 0.0, 0.08, 0.08], &who, None).unwrap();
        drop(c);
        let stats = recognize::run(&opts).unwrap();
        assert_eq!(lib.snapshot(), before);
        let drawn = stats.drawn.unwrap();
        assert_eq!((drawn.embedded, drawn.failed), (2, 0), "{drawn:?}");
        let (aligned, drawn_emb): (bool, Vec<u8>) = conn(&lib)
            .query_row("SELECT aligned, emb FROM recog.drawn WHERE x > 0 OR y > 0", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(aligned, "landmarks found around the face");
        let floats = |b: &[u8]| -> Vec<f32> { b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect() };
        let sim: f32 = floats(&emb).iter().zip(floats(&drawn_emb)).map(|(a, b)| a * b).sum();
        assert!(sim > 0.8, "drawn and detected face: {sim}");
    }
}

/// `--rotated` looks at every photo again, turned 90° and 270°, under the
/// guard: a face only visible turned is added (put back upright), faces the
/// upright pass found are not counted twice, and the pass resumes and is
/// redone when the upright result changes.
#[test]
fn rotated_pass_adds_lying_faces_under_the_guard() {
    let lib = Library::new("recog-rotated");
    edged(&lib, "Tests/lying.jpg", [0, 255, 255]); // a face only when turned 90° clockwise
    lib.scan();
    let before = lib.snapshot();
    let photos = lib.count("SELECT count(DISTINCT quick_hash) FROM files WHERE kind IN ('jpeg', 'png', 'heic')");

    // Without --rotated, only the upright pass.
    let upright = recognize::run(&options(&lib)).unwrap();
    assert!(upright.rotated.is_none());
    assert_eq!(looked(&lib, "Tests/lying.jpg"), Some((0, None)));
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'faces-rot'"), 0);

    let stats = recognize::run(&recognize::Options { rotated: true, ..options(&lib) }).unwrap();
    assert_eq!(lib.snapshot(), before, "the rotated pass changed an original");
    assert_eq!(stats.looked, 0, "the upright pass had nothing left to do");
    let rotated = stats.rotated.unwrap();
    assert_eq!((rotated.looked as i64, rotated.failed, rotated.faces), (photos, 0, 1), "{:?}", rotated.errors);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'faces-rot'"), photos);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.jobs WHERE kind = 'faces-rot' AND state = 'done'"), 1);

    // The face, turned back: (0.1, 0.5, 0.2, 0.3) in the copy turned 90°
    // clockwise is (0.5, 0.7, 0.3, 0.2) upright.
    let (x, y, w, h, roll, landmarks): (f64, f64, f64, f64, i64, String) = conn(&lib)
        .query_row("SELECT x, y, w, h, roll, landmarks FROM recog.faces WHERE roll != 0", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .unwrap();
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    assert!(near(x, 0.5) && near(y, 0.7) && near(w, 0.3) && near(h, 0.2), "{x} {y} {w} {h}");
    assert_eq!(roll, 90);
    let first: Vec<[f64; 2]> = serde_json::from_str(&landmarks).unwrap();
    assert!(near(first[0][0], 0.5) && near(first[0][1], 0.9), "{first:?}");
    // Every other photo's face was found upright, so nothing is added there.
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces"), upright.faces as i64 + 1);
    assert_eq!(count(&lib, "SELECT sum(found) FROM recog.looked WHERE task = 'faces-rot'"), 1);

    // Resumable: nothing left to do.
    let again = recognize::run(&recognize::Options { rotated: true, ..options(&lib) }).unwrap();
    assert_eq!((again.looked, again.rotated.unwrap().looked), (0, 0));

    // A new upright result (another model) drops the rotated one, which is
    // then redone: still one added face, none twice.
    conn(&lib).execute("UPDATE recog.looked SET model = 'old' WHERE task = 'faces'", []).unwrap();
    let redo = recognize::run(&recognize::Options { rotated: true, ..options(&lib) }).unwrap();
    assert_eq!(redo.looked as i64, photos);
    assert_eq!((redo.rotated.as_ref().unwrap().looked as i64, redo.rotated.unwrap().faces), (photos, 1));
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE roll != 0"), 1);
    assert_eq!(lib.snapshot(), before);
    assert!(lib.verify(false).is_clean());
}

/// `shoebox faces stats`: counts, failures by message, widths in the copy,
/// tiny faces; read-only, and fine with a `recognition.db` of phase 4 (v1).
#[test]
fn faces_stats_tell_how_recognition_went() {
    let lib = empty("recog-stats");
    // The fake's face is half as wide as the picture.
    solid(&lib, "a/big.jpg", [90, 120, 30]); // 200 px wide: 100 px
    for (name, w, rgb) in [("tiny", 40u32, [30u8, 90u8, 120u8]), ("small", 70, [120, 30, 90]), ("mid", 100, [60, 60, 140])] {
        image::RgbImage::from_pixel(w, w, image::Rgb(rgb)).save(lib.path(&format!("a/{name}.png"))).unwrap();
    }
    solid(&lib, "a/dark.jpg", [0, 0, 0]);
    solid(&lib, "a/error.jpg", [0, 255, 0]);
    solid(&lib, "a/error2.jpg", [0, 250, 0]);
    lib.scan_opts(true, false, false);

    let shoebox = || {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox")).args(["faces", "stats"]).arg(&lib.root).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    assert!(shoebox().contains("No faces yet"));
    assert!(!lib.path(".shoebox/recognition.db").exists(), "stats created recognition.db");

    recognize::run(&options(&lib)).unwrap();
    let recog_db = lib.path(".shoebox/recognition.db");
    let stamp = shoebox::fingerprint::stamp(&recog_db).unwrap();
    let hash = shoebox::fingerprint::full_hash(&recog_db).unwrap();

    let check = |s: &shoebox::faces::Stats| {
        assert_eq!((s.photos, s.looked, s.failed), (7, 7, 2));
        assert_eq!(s.errors.len(), 1);
        assert_eq!((s.errors[0].message.as_str(), s.errors[0].count), ("fake: cannot handle green", 2));
        assert_eq!(s.faces, 4);
        // under 30 | 30–40 | 40–60 | 60–120 | 120+
        assert_eq!(s.widths.iter().map(|b| b.count).collect::<Vec<_>>(), [1, 1, 1, 1, 0]);
        assert_eq!(s.small, 1);
        assert_eq!(s.scores.last().unwrap().count, 4);
        assert_eq!(s.runs[0].kind, "faces");
        assert_eq!(s.runs[0].state, "done");
    };
    let conn = shoebox::faces::open_readonly(&lib.root, None).unwrap().unwrap();
    check(&shoebox::faces::stats(&conn).unwrap());
    assert!(conn.execute("DELETE FROM recog.faces", []).is_err(), "the connection is not read-only");
    drop(conn);
    let text = shoebox();
    assert!(text.contains("Photos:  7 looked at of 7"), "{text}");
    assert!(text.contains("     2  fake: cannot handle green"), "{text}");
    assert!(text.contains("Under 30 px (listed, too small for clustering): 1 (25.0%)"), "{text}");
    assert!(text.contains("Last runs:"), "{text}");
    assert_eq!(shoebox::fingerprint::stamp(&recog_db).unwrap(), stamp, "stats wrote to recognition.db");
    assert_eq!(shoebox::fingerprint::full_hash(&recog_db).unwrap(), hash);

    // As phase 4 left it: schema v1, no `roll`.
    let old = Connection::open(&recog_db).unwrap();
    old.execute_batch("ALTER TABLE faces DROP COLUMN roll; PRAGMA user_version = 1;").unwrap();
    drop(old);
    let conn = shoebox::faces::open_readonly(&lib.root, None).unwrap().unwrap();
    check(&shoebox::faces::stats(&conn).unwrap());
}

/// The face check page's API: list, sort, filter, crops (made under the
/// guard, cached in thumbs.db, rotated faces turned upright), neighbours.
#[test]
fn serve_face_check_page_under_the_guard() {
    let lib = Library::new("recog-face-page");
    edged(&lib, "Tests/lying.jpg", [0, 255, 255]);
    // A red band left of the lying face (upright x 0.5–0.8, y 0.7–0.9): the
    // top of the face when it is turned upright.
    let mut lying = image::open(lib.path("Tests/lying.jpg")).unwrap().to_rgb8();
    for (x, y, p) in lying.enumerate_pixels_mut() {
        if (100..160).contains(&x) && y >= 150 {
            *p = image::Rgb([220, 0, 0]);
        }
    }
    lying.save(lib.path("Tests/lying.jpg")).unwrap();
    image::RgbImage::from_pixel(50, 50, image::Rgb([30, 90, 120])).save(lib.path("Tests/tiny.png")).unwrap();
    solid(&lib, "Tests/same1.jpg", [200, 150, 120]);
    solid(&lib, "Tests/same2.jpg", [200, 150, 120]);
    std::fs::write(lib.path("Tests/same2.jpg"), [std::fs::read(lib.path("Tests/same2.jpg")).unwrap(), vec![0]].concat())
        .unwrap(); // other bytes, same picture
    lib.scan();
    let stats = recognize::run(&recognize::Options { rotated: true, ..options(&lib) }).unwrap();
    let total = stats.faces + stats.rotated.unwrap().faces;
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;

    let list = get(addr, "/api/faces?limit=1000").json();
    assert_eq!(list["total"], total);
    assert_eq!(list["min_cluster_px"], 30.0);
    let faces = list["faces"].as_array().unwrap().clone();
    assert_eq!(faces.len() as u64, total);
    // Smallest first by default; the 50 px picture's face is 25 px, too
    // small (the 64 px screenshot's, 32 px, is not).
    assert_eq!(faces[0]["px"], 25.0);
    assert_eq!(faces[0]["small"], true);
    assert_eq!(faces.iter().filter(|f| f["small"] == true).count(), 1);
    let px: Vec<f64> = faces.iter().map(|f| f["px"].as_f64().unwrap()).collect();
    assert!(px.windows(2).all(|w| w[0] <= w[1]));
    // The lying face is measured across the face: its box is 0.2 of 300 px
    // high (and 0.3 wide).
    let lying = faces.iter().find(|f| f["roll"] == 90).unwrap();
    assert!((lying["px"].as_f64().unwrap() - 60.0).abs() < 0.5, "{lying}");
    assert_eq!(lying["file"], id_of(&lib, "Tests/lying.jpg"));

    let small = get(addr, "/api/faces?max_px=30").json();
    assert_eq!(small["total"], 1);
    let big = get(addr, "/api/faces?min_px=30&sort=score&desc=true&limit=2&offset=1").json();
    assert_eq!(big["total"], total - 1);
    assert_eq!(big["faces"].as_array().unwrap().len(), 2);
    assert_eq!(get(addr, "/api/faces?rotated=true").json()["total"], 1);
    assert_eq!(get(addr, "/api/faces?offset=100000").json()["total"], total);
    let summary = get(addr, "/api/faces/stats").json();
    assert_eq!(summary["faces"], total);
    assert_eq!(summary["rotated_faces"], 1);

    // Crops: made from the original, stored, then served from thumbs.db.
    for f in &faces {
        let crop = get(addr, &format!("/api/faces/{}/crop", f["id"]));
        assert_eq!(crop.status, 200, "{f}");
        assert_eq!(crop.header("content-type"), Some("image/jpeg"));
        let img = image::load_from_memory(&crop.body).unwrap();
        assert!(img.width() <= shoebox::faces::CROP_EDGE && img.height() <= shoebox::faces::CROP_EDGE);
    }
    let thumbs = Connection::open(lib.path(".shoebox/thumbs.db")).unwrap();
    let stored: i64 = thumbs.query_row("SELECT count(*) FROM faces WHERE jpeg IS NOT NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(stored as u64, total);
    assert_eq!(get(addr, "/api/faces/999999/crop").status, 404);
    // The lying face is turned upright: the band is on top of its crop.
    let crop = image::load_from_memory(&get(addr, &format!("/api/faces/{}/crop", lying["id"])).body).unwrap().to_rgb8();
    let (top, bottom) = (crop.get_pixel(crop.width() / 2, 1), crop.get_pixel(crop.width() / 2, crop.height() - 2));
    assert!(top[0] > 180 && top[1] < 60 && top[2] < 60, "{top:?}");
    assert!(bottom[0] < 160 && bottom[1] > 100, "{bottom:?}");

    // Neighbours: the same picture in another file is the closest.
    let same1 = faces.iter().find(|f| f["file"] == id_of(&lib, "Tests/same1.jpg")).unwrap();
    let near = get(addr, &format!("/api/faces/{}/similar?limit=3", same1["id"])).json();
    let near = near.as_array().unwrap();
    assert_eq!(near.len(), 3);
    assert_eq!(near[0]["file"], id_of(&lib, "Tests/same2.jpg"));
    assert!((near[0]["similarity"].as_f64().unwrap() - 1.0).abs() < 1e-3);
    assert!(near.iter().all(|n| n["id"] != same1["id"]));
    assert!(near.windows(2).all(|w| w[0]["similarity"].as_f64() >= w[1]["similarity"].as_f64()));
    assert_eq!(get(addr, "/api/faces/999999/similar").status, 404);
    assert_eq!(lib.snapshot(), before, "the face page changed an original");

    // A photo that changed since the last scan gets no crop, and nothing is stored.
    thumbs.execute("DELETE FROM faces", []).unwrap();
    let path = lib.path("Tests/same1.jpg");
    set_mtime(&path, shoebox::fingerprint::stamp(&path).unwrap().mtime_ns + 1_000_000_000);
    assert_eq!(get(addr, &format!("/api/faces/{}/crop", same1["id"])).status, 404);
    let stored: i64 = thumbs.query_row("SELECT count(*) FROM faces", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, 0);
    server.stop().unwrap();
}

/// The pets check: stats, list, crops and neighbours of cats and dogs,
/// separate from the faces (own size rule and buckets), under the guard.
#[test]
fn pets_check_page_and_stats_under_the_guard() {
    let lib = empty("recog-pets-page");
    // The fake's pet is half as wide as the picture.
    for (name, w, rgb) in [("tiny", 100u32, [200u8, 60u8, 40u8]), ("small", 130, [200, 60, 90]), ("big", 300, [40, 60, 200]), ("same", 300, [40, 60, 200])] {
        std::fs::create_dir_all(lib.path("Pets")).unwrap();
        image::RgbImage::from_pixel(w, w, image::Rgb(rgb)).save(lib.path(&format!("Pets/{name}.png"))).unwrap();
    }
    // Another file of the same picture (other bytes).
    std::fs::write(lib.path("Pets/same.png"), [std::fs::read(lib.path("Pets/big.png")).unwrap(), vec![0]].concat()).unwrap();
    solid(&lib, "Pets/error.jpg", [10, 250, 10]);
    lib.scan_opts(true, false, false);

    let shoebox = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox")).args(["faces", "stats"]).args(args).arg(&lib.root).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap() + &String::from_utf8(out.stderr).unwrap()
    };
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    let a = stats.pets.as_ref().unwrap();
    // `big.png` and `same.png` hold the same picture in two files.
    assert_eq!((a.looked, a.faces, a.failed), (5 - 1, 4, 1), "{:?}", a.errors);
    let before = lib.snapshot();

    let conn = shoebox::faces::open_readonly(&lib.root, None).unwrap().unwrap();
    let s = shoebox::faces::stats_of(&conn, shoebox::pets::Space::Pets).unwrap();
    assert_eq!(s.kind, "pets");
    assert_eq!((s.photos, s.looked, s.failed, s.faces), (5, 5, 1, 4));
    assert_eq!(s.min_cluster_px, 64.0);
    // Box widths 50 | 65 | 150 | 150: under 64 | 64–100 | 100–200 | 200–400 | 400+
    assert_eq!(s.widths.iter().map(|b| b.count).collect::<Vec<_>>(), [1, 1, 2, 0, 0]);
    assert_eq!(s.small, 1);
    assert_eq!(s.runs[0].kind, "pets");
    // The faces' numbers are about faces: every picture also has a (fake) face.
    let f = shoebox::faces::stats(&conn).unwrap();
    assert_eq!((f.kind, f.faces), ("faces", 4));
    drop(conn);
    let text = shoebox(&["--pets"]);
    assert!(text.contains("Pets: 4"), "{text}");
    assert!(text.contains("Pet width"), "{text}");
    assert!(text.contains("Under 64 px (listed, too small for clustering): 1"), "{text}");

    let server = start(&lib, None);
    let addr = server.addr;
    let list = get(addr, "/api/faces?kind=pets&limit=100").json();
    assert_eq!(list["total"], 4);
    assert_eq!(list["min_cluster_px"], 64.0);
    let pets = list["faces"].as_array().unwrap().clone();
    assert!(pets.iter().all(|f| f["species"].is_string()));
    assert_eq!(pets[0]["small"], true, "smallest first");
    assert_eq!(pets.iter().filter(|f| f["small"] == true).count(), 1);
    assert_eq!(get(addr, "/api/faces?limit=100").json()["total"], 4, "faces are listed apart");
    assert!(get(addr, "/api/faces?limit=100").json()["faces"].as_array().unwrap().iter().all(|f| f["species"].is_null()));
    let summary = get(addr, "/api/faces/stats?kind=pets").json();
    assert_eq!((summary["kind"].as_str(), summary["faces"].as_u64()), (Some("pets"), Some(4)));
    assert_eq!(get(addr, "/api/faces/stats?kind=cats").status, 400);

    // Crops of the whole pet come from the original under the guard.
    for f in &pets {
        let crop = get(addr, &format!("/api/faces/{}/crop", f["id"]));
        assert_eq!(crop.status, 200, "{f}");
        assert_eq!(crop.header("content-type"), Some("image/jpeg"));
    }

    // Neighbours are pets too: the same picture in the other file first.
    let big = pets.iter().find(|f| f["file"] == id_of(&lib, "Pets/big.png")).unwrap();
    let near = get(addr, &format!("/api/faces/{}/similar?limit=10", big["id"])).json();
    let near = near.as_array().unwrap();
    assert_eq!(near.len(), 3, "the other pets only, not the faces");
    assert!(near.iter().all(|n| n["species"].is_string()));
    assert_eq!(near[0]["file"], id_of(&lib, "Pets/same.png"));
    assert!((near[0]["similarity"].as_f64().unwrap() - 1.0).abs() < 1e-3);
    assert_eq!(lib.snapshot(), before, "the pets check changed an original");
}
