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

fn start_launcher() -> launcher::Launcher {
    launcher::start(&launcher::Options { port: 0, open_browser: false }).unwrap()
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
fn the_photo_app_starts_only_when_asked() {
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

    assert_eq!(lpost(addr, "/api/app/stop", &json!({})).status, 200);
    assert_eq!(lget(addr, "/api/app").json()["running"], false);
    launcher.stop().unwrap();
}

#[test]
fn the_drive_is_found_from_where_the_binary_sits() {
    let exe = Path::new("/Volumes/Fotos/.shoebox/bin/shoebox-macos");
    assert_eq!(launcher::drive_of_binary(exe).as_deref(), Some(Path::new("/Volumes/Fotos")));
    assert_eq!(launcher::drive_of_binary(Path::new("/usr/local/bin/shoebox")), None);
}
