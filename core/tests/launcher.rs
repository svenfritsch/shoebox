//! Integration tests for the launcher: the guard (originals untouched by
//! every command it runs), the safety rules (localhost only, `X-Shoebox`),
//! per-file results, and starting the photo app only on request.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use shoebox::launcher;

/// Jobs share one cancel flag and one report sink per process: the tests that
/// run jobs take turns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_launcher() -> launcher::Launcher {
    launcher::start(&launcher::Options { port: 0, open_browser: false, config: None, reveal: None }).unwrap()
}

/// The launcher's routes are not library routes: `/raw` sends them as written.
fn lget(addr: SocketAddr, path: &str) -> Response {
    get(addr, &format!("/raw{path}"))
}

fn lpost(addr: SocketAddr, path: &str, body: &Value) -> Response {
    post(addr, &format!("/raw{path}"), body)
}

fn run_job(addr: SocketAddr, body: Value) -> Value {
    let started = lpost(addr, "/api/job", &body);
    assert_eq!(started.status, 200, "{}", String::from_utf8_lossy(&started.body));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let job = lget(addr, "/api/job").json();
        if !job["running"].as_bool().unwrap() {
            return job;
        }
        assert!(Instant::now() < deadline, "job did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn guard_commands_run_from_the_launcher_leave_originals_untouched() {
    let _turn = serial();
    let lib = Library::new("launcher-guard");
    let before = lib.snapshot();
    let launcher = start_launcher();
    let addr = launcher.addr;
    let root = lib.root.display().to_string();

    for path in ["/", "/launcher.js", "/launcher.css", "/api/drives", "/api/job", "/api/app"] {
        assert_eq!(lget(addr, path).status, 200, "{path}");
    }

    // Scan: every hashed file is a result, none failed.
    let scan = run_job(addr, json!({ "kind": "scan", "root": root }));
    assert_eq!(scan["ok"], true, "{scan}");
    assert!(scan["result"]["added"].as_u64().unwrap() >= 5);
    assert!(scan["ok_count"].as_u64().unwrap() >= 5);
    assert_eq!(scan["fail_count"], 0);
    assert!(scan["recent_ok"].as_array().unwrap().iter().all(|f| f["ok"] == true));

    // Verify: all fine, then a file vanishes and shows up as one failure.
    let clean = run_job(addr, json!({ "kind": "verify", "root": root }));
    assert_eq!(clean["ok"], true, "{clean}");
    assert!(clean["result"]["checked"].as_u64().unwrap() >= 5);
    assert!(clean["progress"]["total"].as_u64().unwrap() > 0, "verify reports progress");

    std::fs::remove_file(lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();
    let broken = run_job(addr, json!({ "kind": "verify", "root": root }));
    assert_eq!(broken["ok"], false);
    assert_eq!(broken["fail_count"], 1);
    assert_eq!(broken["failures"][0]["path"], "Familie/Weihnachten/DSC_2001.jpg");
    assert_eq!(broken["result"]["missing"].as_array().unwrap().len(), 1);

    // Face stats before any recognition: a result, not an error.
    let faces = run_job(addr, json!({ "kind": "faces_stats", "root": root }));
    assert_eq!(faces["ok"], true, "{faces}");
    assert!(faces["result"].is_null());

    // Recognize without a worker fails with a message, and the launcher lives on.
    let recognize = run_job(addr, json!({ "kind": "recognize", "root": root, "limit": 1 }));
    assert_eq!(recognize["ok"], false);
    assert_eq!(lget(addr, "/api/job").status, 200);

    // Everything above only read the originals (the deleted file aside).
    let mut expected = before;
    expected.remove(&lib.path("Familie/Weihnachten/DSC_2001.jpg"));
    assert_eq!(lib.snapshot(), expected);
    launcher.stop().unwrap();
}

#[test]
fn only_this_computer_and_only_with_the_header() {
    let launcher = start_launcher();
    let addr = launcher.addr;
    let evil = ("Host", "evil.example");
    let json_ct = ("Content-Type", "application/json");
    // DNS rebinding: a name that is not localhost or a number.
    assert_eq!(request(addr, "GET", "/raw/api/job", &[evil], b"").status, 403);
    assert_eq!(request(addr, "GET", "/raw/", &[evil], b"").status, 403);
    // Changes need X-Shoebox.
    let body = br#"{"kind":"scan","root":"/"}"#;
    assert_eq!(bare_request(addr, "POST", "/raw/api/job", &[json_ct], body).status, 403);
    assert_eq!(bare_request(addr, "POST", "/raw/api/app", &[json_ct], br#"{"root":"/"}"#).status, 403);
    // Bad input is refused politely.
    assert_eq!(lpost(addr, "/api/job", &json!({ "kind": "format", "root": "/" })).status, 400);
    assert_eq!(lpost(addr, "/api/job", &json!({ "kind": "scan", "root": "/no/such/folder" })).status, 400);
    assert_eq!(lget(addr, "/api/nothing").status, 404);
    assert_eq!(lget(addr, "/api/job").json()["running"], false);
    launcher.stop().unwrap();
}

#[test]
fn the_launcher_serves_its_translations() {
    let launcher = start_launcher();
    let addr = launcher.addr;
    assert_eq!(lget(addr, "/i18n/i18n.js").status, 200);
    for lang in ["en", "de"] {
        let messages = lget(addr, &format!("/i18n/{lang}.json"));
        assert_eq!(messages.status, 200, "{lang}");
        assert_eq!(messages.header("content-type"), Some("application/json"));
        assert!(messages.json()["launcher.step1"].is_string(), "{lang}");
    }
    launcher.stop().unwrap();
}

#[test]
fn the_photo_app_starts_only_when_asked() {
    let _turn = serial();
    let lib = Library::new("launcher-app");
    let launcher = start_launcher();
    let addr = launcher.addr;
    let root = lib.root.display().to_string();

    assert_eq!(lget(addr, "/api/app").json()["running"], false);
    // No index yet: a clear message instead of a crash.
    let early = lpost(addr, "/api/app", &json!({ "root": root, "port": 0 }));
    assert_eq!(early.status, 400);
    assert!(early.json()["error"].as_str().unwrap().contains("shoebox scan"));

    run_job(addr, json!({ "kind": "scan", "root": root, "no_thumbs": true }));
    let started = lpost(addr, "/api/app", &json!({ "root": root, "port": 0 }));
    assert_eq!(started.status, 200, "{}", String::from_utf8_lossy(&started.body));
    let url = started.json()["url"].as_str().unwrap().to_string();
    let app: SocketAddr = url.trim_start_matches("http://localhost:").trim_end_matches('/').parse::<u16>().map(|p| ([127, 0, 0, 1], p).into()).unwrap();
    assert_eq!(get(app, "/api/info").json()["name"], lib.root.file_name().unwrap().to_str().unwrap());
    assert_eq!(lget(addr, "/api/app").json()["running"], true);
    // Asking again for the same folder gives the same app.
    assert_eq!(lpost(addr, "/api/app", &json!({ "root": root })).json()["url"], url);
    // While the photo app runs, nothing else may change the library.
    let busy = lpost(addr, "/api/job", &json!({ "kind": "scan", "root": root }));
    assert_eq!(busy.status, 409);
    assert!(busy.json()["error"].as_str().unwrap().contains("stop the photo app"));

    assert_eq!(lpost(addr, "/api/app/stop", &json!({})).status, 200);
    assert_eq!(lget(addr, "/api/app").json()["running"], false);
    // Stopped: commands work again.
    assert_eq!(run_job(addr, json!({ "kind": "verify", "root": root, "quick": true }))["ok"], true);
    launcher.stop().unwrap();
}

#[test]
fn the_drive_is_found_from_where_the_binary_sits() {
    let exe = Path::new("/Volumes/Fotos/.shoebox/bin/shoebox-macos");
    assert_eq!(launcher::drive_of_binary(exe).as_deref(), Some(Path::new("/Volumes/Fotos")));
    assert_eq!(launcher::drive_of_binary(Path::new("/usr/local/bin/shoebox")), None);
}

#[test]
fn several_folders_run_one_after_the_other_with_a_result_each() {
    let _turn = serial();
    let a = Library::new("launcher-many-a");
    let b = Library::new("launcher-many-b");
    let missing = std::env::temp_dir().join("shoebox-launcher-nothing-here");
    let launcher = start_launcher();
    let addr = launcher.addr;

    let bad = lpost(addr, "/api/job", &json!({ "kind": "scan", "roots": [a.root, missing] }));
    assert_eq!(bad.status, 400, "one wrong folder stops the request before anything runs");

    let roots = [a.root.display().to_string(), b.root.display().to_string()];
    let job = run_job(addr, json!({ "kind": "scan", "roots": roots }));
    assert_eq!(job["ok"], true, "{job}");
    assert_eq!(job["results"].as_array().unwrap().len(), 2);
    assert!(job["results"].as_array().unwrap().iter().all(|r| r["ok"] == true && r["result"]["added"].as_u64().unwrap() >= 5));
    // Results name the folder they belong to.
    let name_a = a.root.file_name().unwrap().to_str().unwrap();
    let name_b = b.root.file_name().unwrap().to_str().unwrap();
    let paths: Vec<String> = job["recent_ok"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect();
    assert!(paths.iter().any(|p| p.starts_with(&format!("{name_a}: "))), "{paths:?}");
    assert!(paths.iter().any(|p| p.starts_with(&format!("{name_b}: "))), "{paths:?}");
    // Both got their index.
    assert!(a.path(".shoebox/library.db").is_file() && b.path(".shoebox/library.db").is_file());
    launcher.stop().unwrap();
}

#[test]
fn the_remembered_folders_are_a_json_file() {
    let dir = std::env::temp_dir().join(format!("shoebox-launcher-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("nested").join("launcher.json");
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(file.clone()), reveal: None }).unwrap();
    let addr = launcher.addr;

    assert_eq!(lget(addr, "/api/config").json(), json!({ "paths": [] }), "no file yet");
    let saved = lpost(addr, "/api/config", &json!({ "paths": [" /Volumes/Fotos ", "/Volumes/Fotos", "", "/Volumes/Fotos 2"] }));
    assert_eq!(saved.status, 200);
    let expected = json!({ "paths": ["/Volumes/Fotos", "/Volumes/Fotos 2"], "history": ["/Volumes/Fotos", "/Volumes/Fotos 2"] });
    assert_eq!(saved.json(), expected, "trimmed, no duplicates");
    // A plain JSON file a person can read and edit.
    let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(on_disk, expected);
    assert_eq!(launcher::Config::load(&file).paths.len(), 2);
    launcher.stop().unwrap();

    // The next start offers them again; a broken file is just empty.
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(file.clone()), reveal: None }).unwrap();
    assert_eq!(lget(launcher.addr, "/api/config").json()["paths"][0], "/Volumes/Fotos");
    launcher.stop().unwrap();
    std::fs::write(&file, b"{ not json").unwrap();
    assert_eq!(launcher::Config::load(&file).paths.len(), 0);
    // Changing it needs the header like everything else.
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(file), reveal: None }).unwrap();
    let json_ct = ("Content-Type", "application/json");
    assert_eq!(bare_request(launcher.addr, "POST", "/raw/api/config", &[json_ct], br#"{"paths":["/x"]}"#).status, 403);
    launcher.stop().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn recognize_pets_runs_the_pets_pass_under_the_guard() {
    let _turn = serial();
    let lib = Library::new("launcher-pets");
    // A red picture is a cat, a blue one a dog, for the fake recognizer.
    image::RgbImage::from_pixel(64, 64, image::Rgb([200, 60, 40])).save(lib.path("cat.png")).unwrap();
    image::RgbImage::from_pixel(64, 64, image::Rgb([40, 60, 200])).save(lib.path("dog.png")).unwrap();
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();
    unsafe { std::env::set_var("SHOEBOX_RECOGNIZER", env!("CARGO_BIN_EXE_shoebox-fake-recognizer")) };
    let launcher = start_launcher();
    let root = lib.root.display().to_string();

    // The plain button looks for faces only; the pets button also for cats and dogs.
    let faces = run_job(launcher.addr, json!({ "kind": "recognize", "root": root }));
    assert_eq!(faces["ok"], true, "{faces}");
    assert!(faces["result"]["pets"].is_null(), "no pet models were asked for");
    let pets = run_job(launcher.addr, json!({ "kind": "recognize_pets", "root": root }));
    assert_eq!(pets["ok"], true, "{pets}");
    // The fake finds one pet in every picture (the library's own photos, and the
    // fixtures when CI has them, so the count is the library's, not a constant).
    let photos = lib.count("SELECT count(DISTINCT quick_hash) FROM files WHERE missing_since IS NULL AND kind IN ('jpeg', 'png', 'heic')");
    assert!(photos >= 8, "{photos}");
    assert_eq!(pets["result"]["pets"]["faces"], photos, "{pets}");
    assert_eq!(pets["result"]["pets"]["looked"], photos);
    assert_eq!(pets["result"]["faces"], 0, "the faces were done by the first run");
    let clustered = pets["result"]["clusters"]["pets"]["faces"].as_i64().unwrap();
    assert!(clustered > 0 && clustered <= photos, "clustered in a space of their own: {pets}");
    assert_eq!(pets["result"]["pets"]["model"], "fake-pets-1");
    assert_eq!(lib.snapshot(), before, "looking for pets changed an original");
    launcher.stop().unwrap();
}

#[test]
fn cancel_stops_a_running_command_like_ctrl_c_and_keeps_what_was_done() {
    let _turn = serial();
    let lib = Library::new("launcher-cancel");
    // A picture the fake recognizer never answers: the run hangs until cancelled.
    image::RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 255])).save(lib.path("blue.png")).unwrap();
    lib.scan_opts(true, false, false);
    unsafe { std::env::set_var("SHOEBOX_RECOGNIZER", env!("CARGO_BIN_EXE_shoebox-fake-recognizer")) };
    let launcher = start_launcher();
    let addr = launcher.addr;
    let root = lib.root.display().to_string();

    // Nothing to cancel.
    assert_eq!(lpost(addr, "/api/job/cancel", &json!({})).json()["cancelling"], false);

    assert_eq!(lpost(addr, "/api/job", &json!({ "kind": "recognize", "root": root })).status, 200);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let job = lget(addr, "/api/job").json();
        let waiting = job["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("pictures to look at"));
        if waiting && job["ok_count"].as_u64().unwrap() + job["fail_count"].as_u64().unwrap() >= 1 {
            break;
        }
        assert!(job["running"] == true, "the run ended by itself: {job}");
        assert!(Instant::now() < deadline, "never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(lpost(addr, "/api/job/cancel", &json!({})).json()["cancelling"], true);
    let deadline = Instant::now() + Duration::from_secs(60);
    let job = loop {
        let job = lget(addr, "/api/job").json();
        if job["running"] == false {
            break job;
        }
        assert!(Instant::now() < deadline, "cancel did not stop the run");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(job["cancelled"], true, "{job}");
    assert_eq!(job["ok"], false);
    // What was found before is kept, and the job is marked as stopped, not failed.
    let state: String = rusqlite::Connection::open(lib.path(".shoebox/recognition.db"))
        .unwrap()
        .query_row("SELECT state FROM jobs ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "interrupted");

    // A cancelled scan resumes: the next command starts normally.
    unsafe { std::env::remove_var("SHOEBOX_RECOGNIZER") };
    let again = run_job(addr, json!({ "kind": "verify", "root": root, "quick": true }));
    assert_eq!(again["cancelled"], false);
    assert_eq!(again["ok"], true, "{again}");
    launcher.stop().unwrap();
}

#[test]
fn a_backup_check_runs_over_two_folders_and_lists_what_is_missing() {
    let _turn = serial();
    let original = Library::new("launcher-backup-original");
    let backup = Library::new("launcher-backup-copy");
    original.scan();
    backup.scan();
    original.jpeg("Neu/neu.jpg", 77);
    original.scan();
    let launcher = start_launcher();
    let addr = launcher.addr;
    let (o, b) = (original.root.display().to_string(), backup.root.display().to_string());

    // Exactly two folders: the original first, then the backup.
    assert_eq!(lpost(addr, "/api/job", &json!({ "kind": "backup", "roots": [o] })).status, 400);
    let job = run_job(addr, json!({ "kind": "backup", "roots": [o, b] }));
    assert_eq!(job["ok"], false, "{job}");
    assert_eq!(job["results"].as_array().unwrap().len(), 1, "one run over both folders");
    assert_eq!(job["result"]["report"]["missing"], 1);
    assert_eq!(job["failures"][0]["path"], "Neu/neu.jpg");
    assert_eq!(job["failures"][0]["note"], "not on the backup yet");
    launcher.stop().unwrap();
}

#[test]
fn ticked_folders_history_and_backup_marks_are_remembered() {
    let dir = std::env::temp_dir().join(format!("shoebox-launcher-ticks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("launcher.json");
    std::fs::create_dir_all(&dir).unwrap();
    // A mark the launcher wrote earlier stays when the page saves its list.
    std::fs::write(&file, br#"{"paths":[],"backups":{"/b":{"of":"/a","at":5}}}"#).unwrap();
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(file.clone()), reveal: None }).unwrap();
    let addr = launcher.addr;

    let saved = lpost(addr, "/api/config", &json!({ "paths": ["/a", "/b", "/c"], "checked": ["/c", "/a", "/gone"] })).json();
    assert_eq!(saved["checked"], json!(["/a", "/c"]), "ticks in list order, only folders that are in the list");
    assert_eq!(saved["backups"]["/b"], json!({ "of": "/a", "at": 5 }), "the page cannot write marks, nor lose them");
    // Taking a folder out of the list keeps it in the history, newest first.
    let saved = lpost(addr, "/api/config", &json!({ "paths": ["/c"], "backups": { "/c": { "of": "/x", "at": 1 } } })).json();
    assert_eq!(saved["history"], json!(["/c", "/a", "/b"]));
    assert!(saved.get("checked").is_none(), "no ticks stored: all are ticked");
    assert_eq!(saved["backups"].as_object().unwrap().len(), 1, "a mark sent by the page is ignored");
    launcher.stop().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_clean_backup_check_marks_the_backup_and_a_failed_one_takes_it_away() {
    let _turn = serial();
    let dir = std::env::temp_dir().join(format!("shoebox-launcher-mark-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("launcher.json");
    let original = Library::new("launcher-mark-original");
    let backup = Library::new("launcher-mark-copy");
    original.scan();
    backup.scan();
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(file.clone()), reveal: None }).unwrap();
    let addr = launcher.addr;
    let (o, b) = (original.root.display().to_string(), backup.root.display().to_string());

    let job = run_job(addr, json!({ "kind": "backup", "roots": [o, b] }));
    assert_eq!(job["ok"], true, "{job}");
    let mark = &lget(addr, "/api/config").json()["backups"][&b];
    assert_eq!(mark["of"], json!(o));
    assert!(mark["at"].as_u64().unwrap() > 0);

    original.jpeg("Neu/neu.jpg", 77);
    original.scan();
    let job = run_job(addr, json!({ "kind": "backup", "roots": [o, b] }));
    assert_eq!(job["ok"], false);
    assert!(lget(addr, "/api/config").json().get("backups").is_none(), "no longer a complete copy: the label goes");
    launcher.stop().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_result_can_be_shown_on_both_drives_but_only_inside_the_jobs_folders() {
    let _turn = serial();
    let original = Library::new("launcher-reveal-original");
    let backup = Library::new("launcher-reveal-copy");
    original.scan();
    backup.scan();
    original.jpeg("Neu/neu.jpg", 77);
    original.scan();
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::<std::path::PathBuf>::new()));
    let sink = opened.clone();
    let reveal: shoebox::serve::RevealFn = std::sync::Arc::new(move |p: &Path| {
        sink.lock().unwrap().push(p.to_path_buf());
        Ok(())
    });
    let launcher = launcher::start(&launcher::Options { port: 0, open_browser: false, config: Some(std::env::temp_dir().join(format!("shoebox-reveal-{}.json", std::process::id()))), reveal: Some(reveal) }).unwrap();
    let addr = launcher.addr;
    let (o, b) = (original.root.display().to_string(), backup.root.display().to_string());

    let before = original.snapshot();
    // Before any job there is nothing to show.
    assert_eq!(lpost(addr, "/api/reveal", &json!({ "items": [{ "root": o, "path": "Neu/neu.jpg" }] })).status, 400);
    run_job(addr, json!({ "kind": "backup", "roots": [o, b] }));
    let shown = lpost(addr, "/api/reveal", &json!({ "items": [{ "root": o, "path": "Neu/neu.jpg" }, { "root": b, "path": "Neu/neu.jpg" }] }));
    assert_eq!(shown.json()["opened"], 1, "only the original has the file");
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert!(opened.lock().unwrap()[0].ends_with("Neu/neu.jpg"));
    // Paths that leave the folder, and folders that are not the job's, are refused.
    let outside = lpost(addr, "/api/reveal", &json!({ "items": [{ "root": o, "path": "../" }] }));
    assert_eq!(outside.json()["opened"], 0);
    assert_eq!(lpost(addr, "/api/reveal", &json!({ "items": [{ "root": "/tmp", "path": "x" }] })).status, 400);
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert_eq!(original.snapshot(), before, "showing a file changes nothing");
    launcher.stop().unwrap();
}
