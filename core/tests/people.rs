//! People, groups, face decisions and clusters (phase 5c-2) with the fake
//! worker's embeddings (`src/bin/shoebox-fake-recognizer.rs`): a plain
//! picture of one colour is one person; a white-topped picture is that
//! person with a chosen similarity (its grey bottom). Every test checks the
//! guard: nothing here may change an original.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use rusqlite::Connection;
use serde_json::{Value, json};
use shoebox::{clusters, people, recognize};

const FAKE: &str = env!("CARGO_BIN_EXE_shoebox-fake-recognizer");

const ANNA: [u8; 3] = [200, 150, 120];
const BEN: [u8; 3] = [90, 120, 30];
const CARL: [u8; 3] = [30, 90, 120];
const DORA: [u8; 3] = [120, 30, 90];

/// Greys of the fake's white-topped pictures: similarity 0.575 (suggested,
/// not clustered), 0.45 ("maybe"), 0.25 (nothing).
const SUGGESTED: u8 = 147;
const MAYBE: u8 = 115;
const FAR: u8 = 64;

fn options(lib: &Library) -> recognize::Options {
    recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(PathBuf::from(FAKE)),
        limit: None,
        retry_failed: false,
        rotated: false,
        pets: false,
        text: false,
        text_only: false,
        timeouts: recognize::Timeouts {
            start: Duration::from_secs(10),
            reply: Duration::from_secs(2),
            exit: Duration::from_secs(2),
        },
    }
}

fn recognize(lib: &Library) -> recognize::Stats {
    let stats = recognize::run(&options(lib)).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    assert!(stats.clusters.is_some(), "recognize clusters at the end");
    stats
}

/// A library connection with `recognition.db` attached.
fn conn(lib: &Library) -> Connection {
    let path = lib.path(".shoebox/library.db");
    let conn = shoebox::db::open_shared(&path).unwrap();
    recognize::attach(&conn, &path).unwrap();
    conn
}

/// A library without the photos `Library::new` makes.
fn empty(test: &str) -> Library {
    let lib = Library::new(test);
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
    }
    lib
}

fn save(lib: &Library, rel: &str, img: image::RgbImage) {
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    img.save(p).unwrap();
}

/// A plain picture of a person; `n` makes the content unique (one pixel),
/// so photos of one person are different files with different keys. The
/// face is 100 px wide.
fn plain(lib: &Library, rel: &str, rgb: [u8; 3], n: u8) {
    let mut img = image::RgbImage::from_pixel(200, 120, image::Rgb(rgb));
    img.put_pixel(0, 0, image::Rgb([n, n, n]));
    save(lib, rel, img);
}

/// The person `rgb`, `grey` away from their plain picture (see the fake).
fn variant(lib: &Library, rel: &str, rgb: [u8; 3], grey: u8) {
    let img = image::RgbImage::from_fn(200, 200, |_, y| {
        image::Rgb(if y < 50 {
            [255, 255, 255]
        } else if y < 150 {
            rgb
        } else {
            [grey, grey, grey]
        })
    });
    save(lib, rel, img);
}

fn wait_for_clusters(addr: std::net::SocketAddr) -> Value {
    let started = Instant::now();
    loop {
        let info = get(addr, "/api/info").json();
        let c = &info["clusters"];
        if c["stale"] == false && c["running"] == false && c["pending"] == false {
            return c.clone();
        }
        assert!(started.elapsed() < Duration::from_secs(20), "clustering did not finish: {c}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// POST that must succeed.
fn ok(addr: std::net::SocketAddr, path: &str, body: &Value) -> Value {
    let r = post(addr, path, body);
    assert_eq!(r.status, 200, "{path} {body}: {}", String::from_utf8_lossy(&r.body));
    r.json()
}

/// The faces of a file as `/api/files/{id}` shows them.
fn faces_of(addr: std::net::SocketAddr, lib: &Library, rel: &str) -> Vec<Value> {
    let info = get(addr, &format!("/api/files/{}", id_of(lib, rel))).json();
    info["faces"].as_array().cloned().unwrap_or_default()
}

/// Faces of the old model left in `recognition.db`.
fn old_faces(lib: &Library) -> i64 {
    conn(lib).query_row("SELECT count(*) FROM recog.faces WHERE model = 'old'", [], |r| r.get(0)).unwrap()
}

/// The one detected face of a file.
fn face_id(addr: std::net::SocketAddr, lib: &Library, rel: &str) -> i64 {
    let faces = faces_of(addr, lib, rel);
    assert_eq!(faces.len(), 1, "{rel}: {faces:?}");
    faces[0]["id"].as_i64().unwrap()
}

fn person_faces(addr: std::net::SocketAddr, person: i64, state: &str) -> Vec<Value> {
    get(addr, &format!("/api/people/{person}/faces?state={state}")).json()["faces"].as_array().unwrap().clone()
}

/// The person as `/api/people` lists them, with the counts of suggestions a
/// naming reply leaves at 0 (only the next grouping knows them): what the
/// reply's `person_data` must be, so the page can show it without the list.
fn as_listed(addr: std::net::SocketAddr, person: i64) -> Value {
    let people = get(addr, "/api/people").json();
    let mut p = people.as_array().unwrap().iter().find(|p| p["id"] == person).expect("listed").clone();
    p["suggested"] = json!(0);
    p["maybe"] = json!(0);
    p
}

fn files_of(faces: &[Value]) -> Vec<i64> {
    let mut files: Vec<i64> = faces.iter().map(|f| f["file"].as_i64().unwrap()).collect();
    files.sort();
    files
}

fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort();
    ids
}

/// Suggestions from all confirmed faces of a person, "maybe" below
/// `SUGGEST_SIM`, nothing below `MAYBE_SIM`, never small faces, never a
/// person a face was rejected for; confirm, reject, the timeline.
#[test]
fn suggest_maybe_confirm_and_reject_under_the_guard() {
    let lib = empty("people-suggest");
    for n in 1..=3 {
        plain(&lib, &format!("Anna/a{n}.png"), ANNA, n);
    }
    variant(&lib, "Mix/suggested.png", ANNA, SUGGESTED);
    variant(&lib, "Mix/maybe.png", ANNA, MAYBE);
    variant(&lib, "Mix/far.png", ANNA, FAR);
    plain(&lib, "Ben/b1.png", BEN, 1);
    plain(&lib, "Ben/b2.png", BEN, 2);
    // Anna's colour, but only 20 px wide: listed, never suggested.
    save(&lib, "Mix/tiny.png", image::RgbImage::from_pixel(40, 40, image::Rgb(ANNA)));
    lib.scan();
    let stats = recognize(&lib);
    let summary = stats.clusters.unwrap();
    // The tiny face does not take part.
    assert_eq!((summary.faces, summary.unnamed), (8, 8));
    assert_eq!(summary.suggested + summary.maybe, 0, "nobody is named yet");
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let info = wait_for_clusters(addr);
    assert_eq!(info["unnamed"], 8);
    assert_eq!(info["people"], 0);

    // The three plain pictures of Anna are one cluster, Ben's two another;
    // the variants are too far from them to join (0.575 < 0.60).
    let list = get(addr, "/api/clusters").json();
    assert_eq!(list["unnamed"], 8);
    let clusters = list["clusters"].as_array().unwrap();
    assert_eq!(clusters.iter().map(|c| c["size"].as_u64().unwrap()).collect::<Vec<_>>(), [3, 2, 1, 1, 1]);
    let anna_files = sorted((1..=3).map(|n| id_of(&lib, &format!("Anna/a{n}.png"))).collect());
    assert_eq!(files_of(clusters[0]["faces"].as_array().unwrap()), anna_files);
    assert!(clusters[0]["suggestion"].is_null());

    // Name it: three confirmed faces. Names touch no file, so the index
    // version stays and no open page reloads its folders, tags and places.
    let version_before = get(addr, "/api/info").json()["index_version"].clone();
    let body = json!({ "name": "Anna", "generation": clusters[0]["generation"] });
    let named = ok(addr, &format!("/api/clusters/{}/name", clusters[0]["id"]), &body);
    assert_eq!(named["faces"], 3);
    let anna = named["person"]["id"].as_i64().unwrap();
    assert_eq!(named["person"]["name"], "Anna");
    // At once (before the clusters are recomputed): out of "Unnamed".
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 5);
    assert_eq!(get(addr, "/api/info").json()["index_version"], version_before);
    wait_for_clusters(addr);

    let people = get(addr, "/api/people").json();
    assert_eq!(people.as_array().unwrap().len(), 1);
    let p = &people[0];
    assert_eq!((p["name"].as_str(), p["faces"].as_u64(), p["photos"].as_u64()), (Some("Anna"), Some(3), Some(3)));
    // The reply carries the person as the list shows them (but for the counts of suggestions),
    // so the page can add them without fetching the list again.
    let mut listed = p.clone();
    listed["suggested"] = json!(0);
    listed["maybe"] = json!(0);
    assert_eq!(named["person_data"], listed);
    assert_eq!((p["suggested"].as_u64(), p["maybe"].as_u64()), (Some(1), Some(1)));
    assert!(p["cover"].is_i64());
    assert!(p["group_id"].is_null());

    let suggested = person_faces(addr, anna, "suggested");
    assert_eq!(files_of(&suggested), [id_of(&lib, "Mix/suggested.png")]);
    assert!((suggested[0]["similarity"].as_f64().unwrap() - 0.575).abs() < 0.01);
    let maybe = person_faces(addr, anna, "maybe");
    assert_eq!(files_of(&maybe), [id_of(&lib, "Mix/maybe.png")]);
    assert!((maybe[0]["similarity"].as_f64().unwrap() - 0.45).abs() < 0.01);
    // Nothing for the far one, the tiny one or Ben's.
    for rel in ["Mix/far.png", "Ben/b1.png", "Mix/tiny.png"] {
        let faces = faces_of(addr, &lib, rel);
        assert!(faces[0]["state"].is_null() && faces[0]["person"].is_null(), "{rel}: {faces:?}");
    }
    assert_eq!(faces_of(addr, &lib, "Mix/tiny.png")[0]["small"], true);
    let face = &faces_of(addr, &lib, "Mix/suggested.png")[0];
    assert_eq!((face["state"].as_str(), face["person"]["name"].as_str()), (Some("suggested"), Some("Anna")));
    assert_eq!(faces_of(addr, &lib, "Anna/a1.png")[0]["state"], "confirmed");

    // Confirm the suggestion; reject the maybe.
    let suggested_face = face_id(addr, &lib, "Mix/suggested.png");
    let maybe_face = face_id(addr, &lib, "Mix/maybe.png");
    assert_eq!(ok(addr, "/api/faces/confirm", &json!({ "faces": [suggested_face] }))["faces"], 1);
    assert_eq!(ok(addr, "/api/faces/reject", &json!({ "faces": [maybe_face] }))["person"]["id"], anna);
    // Nobody suggested: nothing to confirm or reject.
    let far_face = face_id(addr, &lib, "Mix/far.png");
    assert_eq!(post(addr, "/api/faces/confirm", &json!({ "faces": [far_face] })).status, 400);
    wait_for_clusters(addr);
    let face = &faces_of(addr, &lib, "Mix/suggested.png")[0];
    assert_eq!((face["state"].as_str(), face["person"]["id"].as_i64()), (Some("confirmed"), Some(anna)));
    let face = &faces_of(addr, &lib, "Mix/maybe.png")[0];
    assert!(face["state"].is_null(), "rejected: never suggested for Anna again: {face}");
    assert_eq!(face["rejected"], json!([anna]));
    assert_eq!(files_of(&person_faces(addr, anna, "rejected")), [id_of(&lib, "Mix/maybe.png")]);
    assert_eq!(get(addr, &format!("/api/people/{anna}")).json()["faces"], 4);

    // The faces confirmed now bridge further: with the 0.575 face as a
    // reference, the far one is no closer than before (its own direction),
    // so still nothing.
    assert!(faces_of(addr, &lib, "Mix/far.png")[0]["state"].is_null());

    // Assign by name makes a person; the same name in another spelling is
    // that person.
    // Both replies carry the person as the list shows them (`person_data`):
    // new, then with one face more.
    let b1 = face_id(addr, &lib, "Ben/b1.png");
    let named = ok(addr, "/api/faces/assign", &json!({ "faces": [b1], "name": "Ben" }));
    let ben = named["person"]["id"].as_i64().unwrap();
    assert_eq!(named["person_data"], as_listed(addr, ben));
    assert_eq!((named["person_data"]["faces"].as_u64(), named["person_data"]["photos"].as_u64()), (Some(1), Some(1)));
    let b2 = face_id(addr, &lib, "Ben/b2.png");
    let named = ok(addr, "/api/faces/assign", &json!({ "faces": [b2], "name": " ben " }));
    assert_eq!(named["person"]["id"], ben);
    assert_eq!(named["person_data"], as_listed(addr, ben));
    assert_eq!(named["person_data"]["faces"].as_u64(), Some(2));

    // The Unnamed page acts only on faces nobody has decided on (`open`): a
    // face named in the meantime (in the viewer, on another device) is
    // refused with a 409 and nothing changes, not even for the other faces.
    let open_face = face_id(addr, &lib, "Mix/far.png");
    for action in ["assign", "ignore", "not-face"] {
        let body = json!({ "faces": [open_face, b1], "name": "Someone else", "open": true });
        let stale = post(addr, &format!("/api/faces/{action}"), &body);
        assert_eq!(stale.status, 409, "{action}: {}", String::from_utf8_lossy(&stale.body));
    }
    assert_eq!(faces_of(addr, &lib, "Ben/b1.png")[0]["person"]["id"], ben, "still Ben's");
    assert!(faces_of(addr, &lib, "Mix/far.png")[0]["state"].is_null(), "untouched too");
    assert!(!get(addr, "/api/people").json().as_array().unwrap().iter().any(|p| p["name"] == "Someone else"));

    // The browser is told when the faces changed (server-sent events), no
    // polling: a decision asks for new clusters, and the stream says so.
    let mut events = event_stream(addr);
    ok(addr, "/api/faces/assign", &json!({ "faces": [b2], "name": "Ben" }));
    wait_for_event(&mut events, "event: faces");

    // Photos of a person, for the timeline.
    let mut timeline = ids(&get(addr, &format!("/api/timeline?person={anna}")).json());
    timeline.sort();
    let mut want = anna_files.clone();
    want.push(id_of(&lib, "Mix/suggested.png"));
    want.sort();
    assert_eq!(timeline, want);
    assert_eq!(ids(&get(addr, &format!("/api/timeline?person={ben}")).json()).len(), 2);

    // Undo puts a face back to "nothing decided"; ignoring takes it out of
    // "Unnamed" for good.
    ok(addr, "/api/faces/undo", &json!({ "faces": [maybe_face] }));
    wait_for_clusters(addr);
    assert_eq!(faces_of(addr, &lib, "Mix/maybe.png")[0]["state"], "maybe");
    ok(addr, "/api/faces/ignore", &json!({ "faces": [maybe_face, far_face] }));
    assert_eq!(faces_of(addr, &lib, "Mix/far.png")[0]["state"], "ignored");
    wait_for_clusters(addr);
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 0);
    let info = get(addr, "/api/info").json();
    assert_eq!(info["clusters"]["people"], 2);

    assert_eq!(lib.snapshot(), before, "people changed an original");
    // A browser still listening does not keep the server from stopping.
    let (done, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(server.stop().is_ok()).unwrap());
    assert_eq!(stopped.recv_timeout(Duration::from_secs(10)), Ok(true), "the server did not stop with an event stream open");
    drop(events);
    assert!(lib.verify(false).is_clean());
}

/// `/api/events` of the server at `addr`, its head read.
fn event_stream(addr: std::net::SocketAddr) -> std::net::TcpStream {
    use std::io::{Read, Write};
    let id = get(addr, "/api/libraries").json()[0]["id"].as_str().unwrap().to_string();
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(format!("GET /api/lib/{id}/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n").as_bytes()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("text/event-stream"), "{head}");
    s
}

/// Read the stream until `line` comes (within 10 s).
fn wait_for_event(s: &mut std::net::TcpStream, line: &str) {
    use std::io::Read;
    let mut seen = Vec::new();
    let mut buf = [0u8; 512];
    while !String::from_utf8_lossy(&seen).contains(line) {
        let n = s.read(&mut buf).unwrap_or_else(|e| panic!("no {line:?} in {:?}: {e}", String::from_utf8_lossy(&seen)));
        assert!(n > 0, "the stream ended: {:?}", String::from_utf8_lossy(&seen));
        seen.extend_from_slice(&buf[..n]);
    }
}

/// "Not a face" hides a false find everywhere, survives a new recognize
/// run with another model, is counted in the stats and on the face check
/// page, and can be undone.
#[test]
fn not_a_face_is_hidden_everywhere_and_survives_a_model_change() {
    let lib = empty("people-not-face");
    plain(&lib, "a/leg.png", CARL, 1);
    plain(&lib, "a/leg2.png", CARL, 2);
    plain(&lib, "a/dora.png", DORA, 1);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let leg = face_id(addr, &lib, "a/leg.png");
    let leg2 = face_id(addr, &lib, "a/leg2.png");
    assert_eq!(get(addr, "/api/clusters").json()["clusters"][0]["size"], 2);

    // Several at once, as from the face check page.
    assert_eq!(ok(addr, "/api/faces/not-face", &json!({ "faces": [leg, leg2, 999999] }))["faces"], 2);
    let hidden = |addr| {
        assert_eq!(faces_of(addr, &lib, "a/leg.png"), Vec::<Value>::new(), "viewer boxes");
        let list = get(addr, "/api/faces").json();
        assert_eq!(list["total"], 1, "face check page: {list}");
        let marked = get(addr, "/api/faces?not_face=true").json();
        assert_eq!(marked["total"], 2);
        let stats = get(addr, "/api/faces/stats").json();
        assert_eq!((stats["faces"].as_u64(), stats["not_faces"].as_u64()), (Some(3), Some(2)));
        let per_width: u64 = stats["widths"].as_array().unwrap().iter().map(|b| b["not_face"].as_u64().unwrap()).sum();
        assert_eq!(per_width, 2);
        wait_for_clusters(addr);
        let clusters = get(addr, "/api/clusters").json();
        assert_eq!((clusters["unnamed"].as_u64(), clusters["total"].as_u64()), (Some(1), Some(1)), "{clusters}");
        let dora_face = face_id(addr, &lib, "a/dora.png");
        let near = get(addr, &format!("/api/faces/{dora_face}/similar")).json();
        assert_eq!(near.as_array().unwrap().len(), 0, "neighbours: {near}");
    };
    hidden(addr);
    // Confirming (many faces at once) passes over decided faces.
    assert_eq!(ok(addr, "/api/faces/confirm", &json!({ "faces": [leg] }))["faces"], 0);
    assert_eq!(get(addr, "/api/faces?not_face=true").json()["total"], 2);
    server.stop().unwrap();

    // `shoebox faces stats` counts them.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox")).args(["faces", "stats"]).arg(&lib.root).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Marked \"not a face\" (false finds): 2 (66.7%)"), "{text}");
    assert!(text.contains("not a face"), "{text}");

    // Another model: new face rows, the decision still holds.
    let c = conn(&lib);
    c.execute("UPDATE recog.looked SET model = 'old'", []).unwrap();
    c.execute("UPDATE recog.faces SET model = 'old'", []).unwrap();
    drop(c);
    recognize(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(old_faces(&lib), 0, "new face rows");
    let marked = get(addr, "/api/faces?not_face=true").json();
    hidden(addr);

    // Undo from the marked list.
    let ids: Vec<i64> = marked["faces"].as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert_eq!(ok(addr, "/api/faces/undo", &json!({ "faces": ids }))["faces"], 2);
    assert_eq!(get(addr, "/api/faces").json()["total"], 3);
    assert_eq!(faces_of(addr, &lib, "a/leg.png").len(), 1);
    wait_for_clusters(addr);
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 3);
    assert_eq!(lib.snapshot(), before);
    server.stop().unwrap();
}

/// Groups (one per person; a group starts with a person and is gone when its
/// last person leaves; deleting one leaves its people without), people
/// (rename, hide, cover, merge), and splitting a cluster by naming some of
/// its faces.
#[test]
fn groups_merge_and_split() {
    let lib = empty("people-groups");
    for n in 1..=4 {
        plain(&lib, &format!("Dora/d{n}.png"), DORA, n);
    }
    plain(&lib, "Anna/a1.png", ANNA, 1);
    plain(&lib, "Anna/a2.png", ANNA, 2);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);

    // Groups: made by the user from a person, who is its first member; none
    // up front, and never an empty one.
    assert_eq!(get(addr, "/api/groups").json(), json!([]));
    let dora0 = ok(addr, "/api/people", &json!({ "name": "Dora" }))["id"].as_i64().unwrap();
    let emil = ok(addr, "/api/people", &json!({ "name": "Emil" }))["id"].as_i64().unwrap();
    let no_person = post(addr, "/api/groups", &json!({ "name": "Familie" }));
    assert!((400..500).contains(&no_person.status), "a group needs its first person: {}", no_person.status);
    assert_eq!(post(addr, "/api/groups", &json!({ "name": "Familie", "person_id": 999 })).status, 400);
    let familie = ok(addr, "/api/groups", &json!({ "name": "Familie", "person_id": dora0 }))["id"].as_i64().unwrap();
    assert_eq!(get(addr, &format!("/api/people/{dora0}")).json()["group_id"], familie);
    let freunde = ok(addr, "/api/groups", &json!({ "name": "Freunde", "person_id": emil }))["id"].as_i64().unwrap();
    assert_eq!(post(addr, "/api/groups", &json!({ "name": " FAMILIE", "person_id": emil })).status, 400);
    assert_eq!(post(addr, "/api/groups", &json!({ "name": "  ", "person_id": emil })).status, 400);
    assert_eq!(get(addr, &format!("/api/people/{emil}")).json()["group_id"], freunde, "a refused group changes nothing");
    let order = ok(addr, "/api/groups/reorder", &json!({ "ids": [freunde] }));
    let names: Vec<&str> = order.as_array().unwrap().iter().map(|g| g["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Freunde", "Familie"]);
    assert_eq!(ok(addr, &format!("/api/groups/{freunde}/rename"), &json!({ "name": "Beste Freunde" }))["name"], "Beste Freunde");

    // Split: two of Dora's four faces are Dora, one is a stranger; the
    // fourth stays unnamed.
    let list = get(addr, "/api/clusters").json();
    let dora_cluster = &list["clusters"][0];
    assert_eq!(dora_cluster["size"], 4);
    let faces: Vec<i64> = dora_cluster["faces"].as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect();
    let cid = dora_cluster["id"].as_i64().unwrap();
    let generation = dora_cluster["generation"].clone();
    let body = json!({ "name": "Dora", "faces": &faces[..2], "generation": generation });
    let named = ok(addr, &format!("/api/clusters/{cid}/name"), &body);
    let dora = named["person"]["id"].as_i64().unwrap();
    assert_eq!(dora, dora0, "the name was Dora's already");
    // Dora as the list shows her now: in her group, with the two faces.
    assert_eq!(named["person_data"], as_listed(addr, dora));
    assert_eq!((named["person_data"]["group_id"].as_i64(), named["person_data"]["faces"].as_u64()), (Some(familie), Some(2)));
    // The reply says what is left of the card: its faces changed, so it has
    // a new generation; the old one is refused. Its number is where it sits
    // in the list now (which may move: cards of the same size have no fixed
    // order), so the reply's number is used from here on.
    let left = &named["cluster"];
    assert_eq!(left["size"].as_u64(), Some(2), "{named}");
    assert_ne!(left["generation"], generation);
    let left_id = left["id"].as_i64().unwrap();
    let stale = post(addr, &format!("/api/clusters/{left_id}/ignore"), &json!({ "faces": [faces[2]], "generation": generation }));
    assert_eq!(stale.status, 409);
    let ignored = ok(addr, &format!("/api/clusters/{left_id}/ignore"), &json!({ "faces": [faces[2]], "generation": left["generation"] }));
    assert_eq!(ignored["cluster"]["size"], 1);
    // Every change groups the faces again in the background, which may
    // number the clusters anew: look once that is done (the app sends each
    // cluster's generation, so an old number is refused, not mistaken).
    wait_for_clusters(addr);
    let now = get(addr, "/api/clusters").json();
    let has = |c: &Value, face: i64| c["faces"].as_array().unwrap().iter().any(|f| f["id"] == face);
    let clusters = now["clusters"].as_array().unwrap();
    let fourth = clusters.iter().find(|c| has(c, faces[3])).unwrap();
    let left = get(addr, &format!("/api/clusters/{}/faces", fourth["id"])).json();
    assert_eq!(left.as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect::<Vec<_>>(), [faces[3]]);
    // A face of another cluster is refused.
    let other = clusters.iter().find(|c| !has(c, faces[3])).unwrap();
    let body = json!({ "name": "X", "faces": [faces[3]], "generation": other["generation"] });
    assert_eq!(post(addr, &format!("/api/clusters/{}/name", other["id"]), &body).status, 400);
    // The fourth face is now suggested for Dora (identical to her faces).
    let rest = get(addr, "/api/clusters").json();
    assert_eq!(rest["unnamed"], 3);
    let fourth = rest["clusters"].as_array().unwrap().iter().find(|c| c["faces"][0]["id"] == faces[3]).unwrap();
    assert_eq!(fourth["suggestion"]["person"]["id"], dora);
    assert_eq!(fourth["faces"][0]["state"], "suggested");
    // Old cluster numbers are refused once the clusters changed.
    let stale = post(addr, &format!("/api/clusters/{cid}/name"), &json!({ "name": "Dora", "generation": generation }));
    assert_eq!(stale.status, 409, "{}", String::from_utf8_lossy(&stale.body));

    // Anna's cluster is named "Anni" by mistake, then merged into a new Anna.
    let anni_cluster = rest["clusters"].as_array().unwrap().iter().find(|c| c["size"] == 2).unwrap()["id"].as_i64().unwrap();
    let anni = ok(addr, &format!("/api/clusters/{anni_cluster}/name"), &json!({ "name": "Anni" }))["person"]["id"].as_i64().unwrap();
    let anna = ok(addr, "/api/people", &json!({ "name": "Anna", "group_id": familie }))["id"].as_i64().unwrap();
    assert_eq!(post(addr, "/api/people", &json!({ "name": "anna" })).status, 400);
    ok(addr, &format!("/api/people/{anni}/group"), &json!({ "group_id": freunde }));
    // One face rejected for Anna, confirmed for Anni: the confirmation wins.
    let a1 = face_id(addr, &lib, "Anna/a1.png");
    post(addr, "/api/faces/reject", &json!({ "faces": [a1], "person_id": anna }));
    // (A confirmed face cannot be rejected and stay confirmed: rejecting
    // Anna takes nothing from Anni.)
    assert_eq!(faces_of(addr, &lib, "Anna/a1.png")[0]["person"]["id"], anni);
    let merged = ok(addr, &format!("/api/people/{anni}/merge"), &json!({ "into": anna }));
    assert_eq!(
        (merged["name"].as_str(), merged["faces"].as_u64(), merged["group_id"].as_i64()),
        (Some("Anna"), Some(2), Some(familie))
    );
    assert_eq!(get(addr, &format!("/api/people/{anni}")).status, 404);
    let face = &faces_of(addr, &lib, "Anna/a1.png")[0];
    assert_eq!((face["state"].as_str(), face["person"]["id"].as_i64()), (Some("confirmed"), Some(anna)));
    assert!(face.get("rejected").is_none(), "{face}");
    assert_eq!(post(addr, &format!("/api/people/{anna}/merge"), &json!({ "into": anna })).status, 400);

    // A person is in one group at most: moving changes it.
    assert_eq!(ok(addr, &format!("/api/people/{dora}/group"), &json!({ "group_id": familie }))["group_id"], familie);
    assert_eq!(ok(addr, &format!("/api/people/{dora}/group"), &json!({ "group_id": freunde }))["group_id"], freunde);
    assert_eq!(post(addr, &format!("/api/people/{dora}/group"), &json!({ "group_id": 999 })).status, 400);
    // Deleting a group: its people have none.
    assert_eq!(ok(addr, &format!("/api/groups/{freunde}/delete"), &json!({}))["people"], 2);
    assert!(get(addr, &format!("/api/people/{dora}")).json()["group_id"].is_null());
    let groups = get(addr, "/api/groups").json();
    assert_eq!((groups.as_array().unwrap().len(), groups[0]["people"].as_u64()), (1, Some(1)));

    // Never an empty group: the one the last person leaves goes, whether
    // they move to another group, to none, start a new group, are merged
    // into someone or deleted. Groups that are not left by someone stay.
    let group_names = || -> Vec<String> {
        get(addr, "/api/groups").json().as_array().unwrap().iter().map(|g| g["name"].as_str().unwrap().to_string()).collect()
    };
    ok(addr, "/api/groups", &json!({ "name": "Nachbarn", "person_id": emil }));
    assert_eq!(group_names(), ["Familie", "Nachbarn"]);
    ok(addr, "/api/groups", &json!({ "name": "Solo", "person_id": emil }));
    assert_eq!(group_names(), ["Familie", "Solo"], "Emil left Nachbarn, which is empty now");
    ok(addr, &format!("/api/people/{emil}/group"), &json!({ "group_id": familie }));
    assert_eq!(group_names(), ["Familie"], "Emil left Solo");
    let gabi = ok(addr, "/api/people", &json!({ "name": "Gabi" }))["id"].as_i64().unwrap();
    ok(addr, "/api/groups", &json!({ "name": "Gabis", "person_id": gabi }));
    ok(addr, &format!("/api/people/{gabi}/group"), &json!({ "group_id": null }));
    assert_eq!(group_names(), ["Familie"], "Gabi left Gabis for none");
    ok(addr, "/api/groups", &json!({ "name": "Gabis", "person_id": gabi }));
    ok(addr, &format!("/api/people/{gabi}/merge"), &json!({ "into": emil }));
    assert_eq!(group_names(), ["Familie"], "Gabi was merged into Emil");
    let hanna = ok(addr, "/api/people", &json!({ "name": "Hanna" }))["id"].as_i64().unwrap();
    ok(addr, "/api/groups", &json!({ "name": "Hannas", "person_id": hanna }));
    ok(addr, &format!("/api/people/{hanna}/delete"), &json!({}));
    assert_eq!(group_names(), ["Familie"], "Hanna was deleted");
    ok(addr, &format!("/api/people/{emil}/delete"), &json!({}));
    assert_eq!(get(addr, "/api/groups").json()[0]["people"], 1);

    // A misspelled name without faces can be deleted; someone with faces
    // cannot (merge them instead).
    let fran = ok(addr, "/api/people", &json!({ "name": "fran" }))["id"].as_i64().unwrap();
    assert_eq!(post(addr, &format!("/api/people/{anna}/delete"), &json!({})).status, 400);
    assert_eq!(get(addr, &format!("/api/people/{anna}")).status, 200);
    ok(addr, &format!("/api/people/{fran}/delete"), &json!({}));
    assert_eq!(get(addr, &format!("/api/people/{fran}")).status, 404);
    assert_eq!(post(addr, &format!("/api/people/{fran}/delete"), &json!({})).status, 400);

    // People list: by group order, then name; no group last.
    let names: Vec<String> =
        get(addr, "/api/people").json().as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["Anna", "Dora"]);

    // Rename, cover, hide.
    assert_eq!(ok(addr, &format!("/api/people/{dora}/rename"), &json!({ "name": "Dora M." }))["name"], "Dora M.");
    assert_eq!(post(addr, &format!("/api/people/{dora}/rename"), &json!({ "name": "ANNA" })).status, 400);
    assert_eq!(ok(addr, &format!("/api/people/{dora}/cover"), &json!({ "face": faces[1] }))["cover"], faces[1]);
    assert_eq!(post(addr, &format!("/api/people/{dora}/cover"), &json!({ "face": a1 })).status, 400);
    ok(addr, &format!("/api/people/{dora}/hide"), &json!({ "hidden": true }));
    assert_eq!(get(addr, "/api/people").json().as_array().unwrap().len(), 1);
    assert_eq!(get(addr, "/api/people?hidden=1").json().as_array().unwrap().len(), 2);

    assert_eq!(lib.snapshot(), before);
    server.stop().unwrap();
}

/// Decisions are keyed by content and box: they follow a moved file, a
/// rescan, a new model (new face rows) and a lost `recognition.db`; a box
/// that moved a little still matches, one no face overlaps is kept as "no
/// longer found".
#[test]
fn decisions_survive_a_model_change_a_move_and_a_lost_recognition_db() {
    let lib = empty("people-survive");
    plain(&lib, "2020-01 Feier/a1.png", ANNA, 1);
    plain(&lib, "2020-01 Feier/a2.png", ANNA, 2);
    plain(&lib, "2020-01 Feier/b1.png", BEN, 1);
    lib.scan();
    recognize(&lib);
    let before_files = lib.snapshot().len();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let a1 = face_id(addr, &lib, "2020-01 Feier/a1.png");
    let b1 = face_id(addr, &lib, "2020-01 Feier/b1.png");
    let anna = ok(addr, "/api/faces/assign", &json!({ "faces": [a1], "name": "Anna" }))["person"]["id"].as_i64().unwrap();
    ok(addr, "/api/faces/reject", &json!({ "faces": [b1], "person_id": anna }));
    wait_for_clusters(addr);
    assert_eq!(faces_of(addr, &lib, "2020-01 Feier/a2.png")[0]["state"], "suggested");

    // Moved: same content, same decision.
    ok(addr, "/api/move", &json!({ "ids": [id_of(&lib, "2020-01 Feier/a1.png")], "folder": "Anna" }));
    let face = &faces_of(addr, &lib, "Anna/a1.png")[0];
    assert_eq!((face["state"].as_str(), face["person"]["id"].as_i64()), (Some("confirmed"), Some(anna)));
    server.stop().unwrap();
    lib.scan();

    let check = |addr| {
        let face = &faces_of(addr, &lib, "Anna/a1.png")[0];
        assert_eq!((face["state"].as_str(), face["person"]["id"].as_i64()), (Some("confirmed"), Some(anna)), "{face}");
        assert_eq!(faces_of(addr, &lib, "2020-01 Feier/b1.png")[0]["rejected"], json!([anna]));
        wait_for_clusters(addr);
        assert_eq!(faces_of(addr, &lib, "2020-01 Feier/a2.png")[0]["state"], "suggested");
        assert_eq!(ids(&get(addr, &format!("/api/timeline?person={anna}")).json()), [id_of(&lib, "Anna/a1.png")]);
    };

    // Another model.
    let c = conn(&lib);
    c.execute("UPDATE recog.looked SET model = 'old'", []).unwrap();
    c.execute("UPDATE recog.faces SET model = 'old'", []).unwrap();
    drop(c);
    recognize(&lib);
    assert_eq!(old_faces(&lib), 0, "new face rows");
    let server = start(&lib, None);
    check(server.addr);
    server.stop().unwrap();

    // recognition.db is a cache: without it, nothing the user did is lost.
    for f in ["recognition.db", "recognition.db.bak"] {
        std::fs::remove_file(lib.path(&format!(".shoebox/{f}"))).unwrap();
    }
    recognize(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    check(addr);

    // A box that moved a little still matches; one far off stays the user's
    // confirmed face, shown from its box and name (`kept`) but not used for matching.
    let db = lib.db();
    db.execute("UPDATE face_decisions SET x = x + 0.05, y = y + 0.03 WHERE decision = 'confirmed'", []).unwrap();
    assert_eq!(faces_of(addr, &lib, "Anna/a1.png")[0]["person"]["id"], anna);
    db.execute("UPDATE face_decisions SET x = 0.0, y = 0.0, w = 0.1, h = 0.1 WHERE decision = 'confirmed'", []).unwrap();
    let info = get(addr, &format!("/api/files/{}", id_of(&lib, "Anna/a1.png"))).json();
    let faces = info["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 2, "the detected face and the kept one: {info}");
    assert!(info.get("faces_lost").is_none());
    let detected = faces.iter().find(|f| f["kept"].is_null()).unwrap();
    assert!(detected["state"].is_null(), "{info}");
    let kept = faces.iter().find(|f| f["kept"] == true).unwrap();
    assert_eq!((kept["state"].as_str(), kept["person"]["id"].as_i64()), (Some("confirmed"), Some(anna)));
    let confirmed = person_faces(addr, anna, "confirmed");
    assert_eq!((confirmed.len(), confirmed[0]["kept"].as_bool()), (1, Some(true)));
    assert_eq!(get(addr, &format!("/api/people/{anna}")).json()["faces"], 1);
    assert_eq!(lib.snapshot().len(), before_files);
    server.stop().unwrap();
    assert!(lib.verify(false).is_clean());
}

/// A face drawn by hand is the decision row itself: it shows on its photo
/// and for its person; over a detected face it is one face (the drawn box
/// wins), never two; it can be named again or deleted, not rejected.
#[test]
fn hand_drawn_faces() {
    let lib = empty("people-manual");
    save(&lib, "a/dark.png", image::RgbImage::from_pixel(200, 100, image::Rgb([0, 0, 0]))); // no face found
    plain(&lib, "a/anna.png", ANNA, 1);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let dark = id_of(&lib, "a/dark.png");
    assert_eq!(faces_of(addr, &lib, "a/dark.png").len(), 0);

    let body = json!({ "file": dark, "box": [0.1, 0.2, 0.3, 0.4], "name": "Anna" });
    let m = ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap();
    let anna = get(addr, "/api/people").json()[0]["id"].as_i64().unwrap();
    let faces = faces_of(addr, &lib, "a/dark.png");
    assert_eq!(faces.len(), 1);
    assert_eq!(
        (faces[0]["manual"].as_i64(), faces[0]["id"].as_i64(), faces[0]["state"].as_str()),
        (Some(m), None, Some("confirmed"))
    );
    assert_eq!(faces[0]["x"], 0.1);
    assert!(person_faces(addr, anna, "confirmed").iter().any(|f| f["manual"] == m));
    assert!(ids(&get(addr, &format!("/api/timeline?person={anna}")).json()).contains(&dark));
    // Nothing detected to show for her: the drawn face.
    let p = get(addr, &format!("/api/people/{anna}")).json();
    assert_eq!((p["cover"].as_i64(), p["cover_manual"].as_i64()), (None, Some(m)));
    assert_eq!(post(addr, "/api/faces/manual", &json!({ "file": dark, "box": [0.8, 0.0, 0.3, 0.4], "name": "Anna" })).status, 400);

    // Over the detected face of the other photo: one face, the drawn box.
    let detected = face_id(addr, &lib, "a/anna.png");
    let anna_file = id_of(&lib, "a/anna.png");
    let body = json!({ "file": anna_file, "box": [0.27, 0.25, 0.5, 0.5], "person_id": anna });
    let m2 = ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap();
    let faces = faces_of(addr, &lib, "a/anna.png");
    assert_eq!(faces.len(), 1, "{faces:?}");
    assert_eq!(
        (faces[0]["id"].as_i64(), faces[0]["manual"].as_i64(), faces[0]["x"].as_f64()),
        (Some(detected), Some(m2), Some(0.27))
    );
    assert_eq!(get(addr, &format!("/api/people/{anna}")).json()["faces"], 2);
    wait_for_clusters(addr);
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 0);
    // Not rejected, not "not a face": named again or deleted.
    assert_eq!(post(addr, "/api/faces/reject", &json!({ "faces": [detected], "person_id": anna })).status, 400);
    assert_eq!(post(addr, "/api/faces/not-face", &json!({ "faces": [detected] })).status, 400);
    let carla = ok(addr, "/api/faces/assign", &json!({ "faces": [detected], "name": "Carla" }))["person"]["id"].clone();
    assert_eq!(faces_of(addr, &lib, "a/anna.png")[0]["person"]["id"], carla);
    assert_eq!(faces_of(addr, &lib, "a/anna.png")[0]["manual"], m2);
    assert_eq!(ok(addr, "/api/faces/undo", &json!({ "manual": [m, m2] }))["faces"], 2);
    assert_eq!(faces_of(addr, &lib, "a/dark.png").len(), 0);
    let face = &faces_of(addr, &lib, "a/anna.png")[0];
    assert!(face.get("manual").is_none() && face["state"].is_null(), "{face}");
    assert_eq!(lib.snapshot(), before);
    server.stop().unwrap();
}

/// Groups, people and decisions are backed up like own tags: in
/// `library.db.bak` and `userdata.json` (version 3).
#[test]
fn user_data_backup_has_people_groups_and_decisions() {
    let lib = empty("people-backup");
    plain(&lib, "a/a1.png", ANNA, 1);
    plain(&lib, "a/c1.png", CARL, 1);
    lib.scan();
    recognize(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    let a1 = face_id(addr, &lib, "a/a1.png");
    let anna = ok(addr, "/api/faces/assign", &json!({ "faces": [a1], "name": "Anna" }))["person"]["id"].as_i64().unwrap();
    ok(addr, "/api/groups", &json!({ "name": "Familie", "person_id": anna }));
    ok(addr, "/api/faces/not-face", &json!({ "faces": [face_id(addr, &lib, "a/c1.png")] }));
    server.stop().unwrap();

    let data: Value = serde_json::from_slice(&std::fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["version"], 6);
    assert_eq!(data["view_turns"], json!([]));
    assert_eq!(data["groups"], json!([{ "name": "Familie", "position": 1 }]));
    assert_eq!(data["people"][0]["name"], "Anna");
    assert_eq!(data["people"][0]["group"], "Familie");
    let decisions = data["face_decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2);
    let confirmed = decisions.iter().find(|d| d["decision"] == "confirmed").unwrap();
    assert_eq!(confirmed["person"], "Anna");
    assert_eq!(confirmed["files"][0]["path"], "a/a1.png");
    assert!(confirmed["files"][0]["full_hash"].is_string());
    assert_eq!(confirmed["box"].as_array().unwrap().len(), 4);
    let not_face = decisions.iter().find(|d| d["decision"] == "not_face").unwrap();
    assert!(not_face["person"].is_null());
    let bak = Connection::open(lib.path(".shoebox/library.db.bak")).unwrap();
    let n: i64 = bak.query_row("SELECT count(*) FROM face_decisions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
}

/// Clustering stopped half-way resumes: the neighbour lists computed so far
/// are kept, and the next run computes only the rest.
#[test]
fn clustering_resumes_after_a_stop() {
    let lib = empty("people-resume");
    plain(&lib, "a/a1.png", ANNA, 1);
    plain(&lib, "a/a2.png", ANNA, 2);
    plain(&lib, "a/b1.png", BEN, 1);
    lib.scan();
    recognize(&lib);
    let c = conn(&lib);
    c.execute("DELETE FROM recog.neighbours", []).unwrap();
    c.execute("DELETE FROM recog.clusters", []).unwrap();
    let err = clusters::run(&c, &|| true, &mut |_, _| {}).unwrap_err();
    assert!(err.is::<recognize::Interrupted>());
    let state: String =
        c.query_row("SELECT state FROM recog.jobs WHERE kind = 'clusters' ORDER BY id DESC", [], |r| r.get(0)).unwrap();
    assert_eq!(state, "interrupted");
    assert!(!clusters::running(&c).unwrap());

    let mut seen = Vec::new();
    let s = clusters::run(&c, &|| false, &mut |done, total| seen.push((done, total))).unwrap();
    assert_eq!((s.faces, s.listed, s.unnamed, s.clusters), (3, 3, 3, 2));
    // Three neighbour lists, then the three faces' suggestions searched.
    assert_eq!(seen.last(), Some(&(6, 6)));
    let again = clusters::run(&c, &|| false, &mut |_, _| {}).unwrap();
    assert_eq!((again.listed, again.clusters), (0, 2), "lists are kept");
    let o = clusters::overview(&c).unwrap();
    assert_eq!((o.clusters, o.unnamed, o.state.as_deref()), (2, 3, Some("done")));
}

/// Clustering time over ~10,000 faces (the real drive has 10,043), with
/// made-up embeddings: 1,000 people with 10 faces each. Slow without
/// optimisations, so only on request:
/// `cargo test --release --test people -- --ignored --nocapture`.
#[test]
#[ignore]
fn clustering_time_over_10000_faces() {
    let lib = empty("people-time");
    lib.scan();
    let c = conn(&lib);
    let (people, per, dim) = (1000usize, 10usize, 128usize);
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    };
    let unit = |v: Vec<f32>| {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / n).collect::<Vec<f32>>()
    };
    let centres: Vec<Vec<f32>> = (0..people).map(|_| unit((0..dim).map(|_| next()).collect())).collect();
    let tx = c.unchecked_transaction().unwrap();
    tx.execute("INSERT OR IGNORE INTO folders (path, path_nfc, name, last_seen) VALUES ('', '', '', 0)", []).unwrap();
    for i in 0..people * per {
        let key = format!("key{i:05}");
        tx.execute(
            "INSERT INTO files (folder_id, path, path_nfc, name, kind, size, mtime_ns, quick_hash, added_at)
             VALUES ((SELECT id FROM folders WHERE path_nfc = ''), ?1, ?1, ?1, 'jpeg', 1, 0, ?2, 0)",
            rusqlite::params![format!("p{i}.jpg"), key],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO recog.looked (key, task, model, width, height, found, done_at) VALUES (?1, 'faces', 'm', 1600, 1200, 1, 0)",
            [&key],
        )
        .unwrap();
        let noise: Vec<f32> = unit((0..dim).map(|_| next()).collect());
        let emb = unit(centres[i % people].iter().zip(&noise).map(|(a, b)| a + 0.7 * b).collect());
        let bytes: Vec<u8> = emb.iter().flat_map(|v| v.to_le_bytes()).collect();
        tx.execute(
            "INSERT INTO recog.faces (key, model, x, y, w, h, score, emb) VALUES (?1, 'm', 0.4, 0.4, 0.1, 0.1, 0.9, ?2)",
            rusqlite::params![key, bytes],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let started = Instant::now();
    let s = clusters::run(&c, &|| false, &mut |_, _| {}).unwrap();
    let first = started.elapsed();
    // Name a tenth of the people (one face each), then again: suggestions.
    let tx = c.unchecked_transaction().unwrap();
    for p in 0..people / 10 {
        tx.execute("INSERT INTO people (name) VALUES (?1)", [format!("P{p}")]).unwrap();
        let key = format!("key{p:05}");
        tx.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, at)
             VALUES (?1, 0.4, 0.4, 0.1, 0.1, ?2, 'confirmed', 0)",
            rusqlite::params![key, p as i64 + 1],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let started = Instant::now();
    let again = clusters::run(&c, &|| false, &mut |_, _| {}).unwrap();
    let second = started.elapsed();
    println!(
        "{} faces: first run {:.2} s ({} clusters), again with {} named {:.2} s ({} suggested, {} maybe)",
        s.faces,
        first.as_secs_f64(),
        s.clusters,
        people / 10,
        second.as_secs_f64(),
        again.suggested,
        again.maybe
    );
    assert_eq!(s.faces as usize, people * per);
    // Most faces of a person end up together, few people together.
    assert!(s.clusters as usize >= people * 9 / 10 && (s.clusters as usize) < people * per / 2, "{} clusters", s.clusters);
    assert!(again.suggested as usize >= (people / 10) * (per - 1) * 9 / 10, "{} suggested", again.suggested);
    assert!(first < Duration::from_secs(60));
}

/// A picture whose left quarter is pure cyan (the fake finds no face in it
/// upright) and the rest `rgb`: a box drawn there is embedded as the person
/// of `rgb`, with landmarks unless `rgb` is grey.
fn faceless(lib: &Library, rel: &str, rgb: [u8; 3]) {
    let img = image::RgbImage::from_fn(200, 120, |x, _| image::Rgb(if x < 50 { [0, 255, 255] } else { rgb }));
    save(lib, rel, img);
}

fn folder_id(lib: &Library, path: &str) -> i64 {
    lib.db().query_row("SELECT id FROM folders WHERE path_nfc = ?1", [path], |r| r.get(0)).unwrap()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "{what} did not happen");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Faces drawn by hand get their embedding from the worker (task `embed`,
/// protocol 2): in the background in `serve` right after drawing, under
/// the guard, and again in `shoebox recognize` after a model change. One
/// aligned by landmarks counts as a reference for suggestions; a plain
/// crop's does not.
#[test]
fn drawn_faces_are_embedded_and_aligned_ones_suggest() {
    let lib = empty("people-drawn");
    faceless(&lib, "a/anna-missed.png", ANNA);
    faceless(&lib, "a/grey-missed.png", [128, 128, 128]);
    plain(&lib, "a/grey.png", [128, 128, 128], 1);
    variant(&lib, "a/anna-older.png", ANNA, SUGGESTED);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    assert_eq!(faces_of(addr, &lib, "a/anna-missed.png").len(), 0, "missed by the detector");
    assert!(faces_of(addr, &lib, "a/anna-older.png")[0]["state"].is_null());

    let draw = |rel: &str, name: &str| {
        let body = json!({ "file": id_of(&lib, rel), "box": [0.4, 0.25, 0.4, 0.5], "name": name });
        ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap()
    };
    let anna_drawn = draw("a/anna-missed.png", "Anna");
    let grey_drawn = draw("a/grey-missed.png", "Grau");
    let drawn = |lib: &Library| -> Vec<(i64, bool, String)> {
        conn(lib)
            .prepare("SELECT aligned, emb IS NOT NULL, model FROM recog.drawn ORDER BY x, key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    wait_until("embedding", || {
        drawn(&lib).len() == 2 && get(addr, "/api/info").json()["clusters"]["embedding"] == false
    });
    let mut aligned: Vec<i64> = drawn(&lib).iter().map(|d| d.0).collect();
    aligned.sort();
    assert_eq!(aligned, [0, 1], "grey: the plain crop, without landmarks");
    assert!(drawn(&lib).iter().all(|d| d.1 && d.2 == "fake-1"));
    wait_for_clusters(addr);

    // Anna's drawn face (aligned) suggests her older face; the grey one's
    // embedding is identical to the grey photo's face but not aligned, so
    // nothing is suggested from it.
    let older = &faces_of(addr, &lib, "a/anna-older.png")[0];
    assert_eq!((older["state"].as_str(), older["person"]["name"].as_str()), (Some("suggested"), Some("Anna")), "{older}");
    assert!(faces_of(addr, &lib, "a/grey.png")[0]["state"].is_null());

    // Crops of drawn faces, made under the guard like the others.
    for m in [anna_drawn, grey_drawn] {
        let crop = get(addr, &format!("/api/faces/manual/{m}/crop"));
        assert_eq!((crop.status, crop.header("content-type")), (200, Some("image/jpeg")));
    }
    assert_eq!(get(addr, "/api/faces/manual/999999/crop").status, 404);
    assert_eq!(lib.snapshot(), before, "embedding changed an original");
    server.stop().unwrap();

    // Another model: `shoebox recognize` embeds them again.
    conn(&lib).execute("UPDATE recog.drawn SET model = 'old'", []).unwrap();
    let stats = recognize(&lib);
    let d = stats.drawn.unwrap();
    assert_eq!((d.embedded, d.plain, d.failed), (2, 1, 0));
    assert!(drawn(&lib).iter().all(|d| d.2 == "fake-1"));
    // Nothing left to do: not again.
    assert_eq!(recognize(&lib).drawn.unwrap().embedded, 0);

    // A deleted drawn face's embedding goes on the next run.
    let server = start(&lib, None);
    ok(server.addr, "/api/faces/undo", &json!({ "manual": [grey_drawn] }));
    server.stop().unwrap();
    recognize(&lib);
    assert_eq!(drawn(&lib).len(), 1);
    assert_eq!(lib.snapshot(), before);
    assert!(lib.verify(false).is_clean());
}

/// People are terms of the search: several together (AND), with tags and
/// folders; free text finds them by name; the search box's suggestions
/// count within the current search. Undoing a rejection keeps every other
/// decision about the face.
#[test]
fn people_as_search_terms_and_undoing_a_rejection() {
    let lib = empty("people-search");
    plain(&lib, "Fotos/x1.png", ANNA, 1);
    plain(&lib, "Fotos/x2.png", ANNA, 2);
    plain(&lib, "Fotos/y1.png", BEN, 1);
    plain(&lib, "Other/z1.png", CARL, 1);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let assign = |rel: &str, name: &str| {
        let body = json!({ "faces": [face_id(addr, &lib, rel)], "name": name });
        ok(addr, "/api/faces/assign", &body)["person"]["id"].as_i64().unwrap()
    };
    let anna = assign("Fotos/x1.png", "Anna");
    assign("Fotos/x2.png", "Anna");
    let ben = assign("Fotos/y1.png", "Ben");
    // Ben next to Anna on x1, drawn by hand (the fake finds one face).
    let body = json!({ "file": id_of(&lib, "Fotos/x1.png"), "box": [0.0, 0.0, 0.2, 0.2], "person_id": ben });
    ok(addr, "/api/faces/manual", &body);
    let x1 = id_of(&lib, "Fotos/x1.png");
    let (x2, y1) = (id_of(&lib, "Fotos/x2.png"), id_of(&lib, "Fotos/y1.png"));

    let timeline = |q: &str| sorted(ids(&get(addr, &format!("/api/timeline?{q}")).json()));
    assert_eq!(timeline(&format!("person={anna}")), sorted(vec![x1, x2]));
    assert_eq!(timeline(&format!("person={ben}")), sorted(vec![x1, y1]));
    assert_eq!(timeline(&format!("person={anna}&person={ben}")), [x1]);
    let both = get(addr, &format!("/api/timeline?person={anna}&person={ben}")).json();
    let names: Vec<&str> = both["people"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Anna", "Ben"]);
    // With a folder and free text; free text matches names like tag names.
    assert_eq!(timeline(&format!("person={ben}&folder={}", folder_id(&lib, "Fotos"))), sorted(vec![x1, y1]));
    assert_eq!(timeline("q=anna"), sorted(vec![x1, x2]));
    assert_eq!(timeline("q=anna+ben"), [x1]);
    assert_eq!(timeline(&format!("q=fotos&person={ben}")), sorted(vec![x1, y1]));
    assert!(timeline("q=carl").is_empty(), "nobody is named Carl");

    // Suggestions for the search box: by name, counted within the search;
    // people of the search and people who would show nothing left out.
    // Without the picture and the species (they have their own checks).
    let search = |q: &str| {
        let mut hits = get(addr, &format!("/api/people/search?{q}")).json();
        for h in hits.as_array_mut().unwrap() {
            let h = h.as_object_mut().unwrap();
            assert!(h["species"].is_null(), "people are no pets");
            for k in ["species", "cover", "cover_manual"] {
                h.remove(k);
            }
        }
        hits
    };
    let all = search("q=");
    assert_eq!(all, json!([
        { "id": anna, "name": "Anna", "group_id": null, "photos": 2 },
        { "id": ben, "name": "Ben", "group_id": null, "photos": 2 },
    ]));
    assert_eq!(search("q=AN"), json!([{ "id": anna, "name": "Anna", "group_id": null, "photos": 2 }]));
    assert_eq!(search(&format!("q=&person={ben}")), json!([{ "id": anna, "name": "Anna", "group_id": null, "photos": 1 }]));
    assert_eq!(search(&format!("q=&person={anna}&person={ben}")), json!([]));
    // Tags within a search of people.
    let tags = get(addr, &format!("/api/tags?person={ben}")).json();
    assert_eq!(tags.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Fotos"]);

    // A group in the search shows the photos of any of its people (OR, where
    // several people are all of them); several groups and everything else
    // narrow it (AND).
    let freunde = ok(addr, "/api/groups", &json!({ "name": "Freunde", "person_id": ben }))["id"].as_i64().unwrap();
    assert_eq!(timeline(&format!("group={freunde}")), sorted(vec![x1, y1]), "only Ben so far");
    ok(addr, &format!("/api/people/{anna}/group"), &json!({ "group_id": freunde }));
    assert_eq!(timeline(&format!("group={freunde}")), sorted(vec![x1, x2, y1]), "Anna's photos or Ben's");
    assert_eq!(timeline(&format!("group={freunde}&person={anna}")), sorted(vec![x1, x2]));
    assert_eq!(timeline(&format!("group={freunde}&folder={}", folder_id(&lib, "Other"))), Vec::<i64>::new());
    let chips = get(addr, &format!("/api/timeline?group={freunde}")).json();
    assert_eq!(chips["groups"], json!([{ "id": freunde, "name": "Freunde" }]));
    assert_eq!(chips["count"], 3);
    assert_eq!(timeline("group=999"), Vec::<i64>::new(), "no such group");
    // Suggestions: groups by name, counted within the search; groups of the
    // search and groups that would show nothing left out.
    let groups = |q: &str| get(addr, &format!("/api/groups/search?{q}")).json();
    assert_eq!(groups("q=fre"), json!([{ "id": freunde, "name": "Freunde", "people": 2, "photos": 3 }]));
    assert_eq!(groups(&format!("q=&person={anna}"))[0]["photos"], 2);
    assert_eq!(groups("q=zzz"), json!([]));
    assert_eq!(groups(&format!("q=&group={freunde}")), json!([]));
    assert_eq!(groups(&format!("q=&folder={}", folder_id(&lib, "Other"))), json!([]));
    // Tags within a search of a group.
    let tags = get(addr, &format!("/api/tags?group={freunde}")).json();
    assert_eq!(tags.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Fotos"]);
    ok(addr, &format!("/api/groups/{freunde}/delete"), &json!({}));

    // Undo a rejection: only the rejection goes, Ben stays confirmed.
    let y1_face = face_id(addr, &lib, "Fotos/y1.png");
    ok(addr, "/api/faces/reject", &json!({ "faces": [y1_face], "person_id": anna }));
    assert_eq!(files_of(&person_faces(addr, anna, "rejected")), [y1]);
    assert_eq!(post(addr, "/api/faces/unreject", &json!({ "faces": [y1_face] })).status, 400, "person_id is needed");
    let undone = ok(addr, "/api/faces/unreject", &json!({ "faces": [y1_face], "person_id": anna }));
    assert_eq!((undone["faces"].as_u64(), undone["person"]["id"].as_i64()), (Some(1), Some(anna)));
    assert!(person_faces(addr, anna, "rejected").is_empty());
    let face = &faces_of(addr, &lib, "Fotos/y1.png")[0];
    assert_eq!((face["state"].as_str(), face["person"]["id"].as_i64()), (Some("confirmed"), Some(ben)));
    assert_eq!(ok(addr, "/api/faces/unreject", &json!({ "faces": [y1_face], "person_id": anna }))["faces"], 0);

    assert_eq!(lib.snapshot(), before);
    server.stop().unwrap();
}

// ---------------------------------------------------------------- pets (phase 7)

/// Red at least as strong as blue: the fake worker's cat; the reverse: a dog.
const SPOOKY: [u8; 3] = [200, 60, 40];
const REX: [u8; 3] = [40, 60, 200];

/// Greys of the fake's variants for pets (64 numbers, model
/// `fake-pets-1`): similarity 0.875 (suggested: ≥ 0.85, but not joined at
/// 0.90), 0.775 ("maybe": ≥ 0.75), 0.575 (below every pet threshold).
const PET_SUGGESTED: u8 = 221;
const PET_MAYBE: u8 = 195;
const PET_FAR: u8 = 147;

/// The pet among a file's faces (every fake picture also holds a face).
fn pet_of(addr: std::net::SocketAddr, lib: &Library, rel: &str) -> Value {
    let faces = faces_of(addr, lib, rel);
    let mut pets = faces.iter().filter(|f| f["species"].is_string());
    let pet = pets.next().unwrap_or_else(|| panic!("{rel}: no pet in {faces:?}")).clone();
    assert!(pets.next().is_none(), "{rel}: one pet");
    pet
}

fn face_of_person(addr: std::net::SocketAddr, lib: &Library, rel: &str) -> Value {
    let faces = faces_of(addr, lib, rel);
    let mut people = faces.iter().filter(|f| f["species"].is_null());
    let face = people.next().unwrap_or_else(|| panic!("{rel}: no face in {faces:?}")).clone();
    assert!(people.next().is_none());
    face
}

/// Cats and dogs are people like the others (named, grouped, found in the
/// timeline), but live in a space of their own: clusters and suggestions of
/// pets only mix with pets, and ask for much stronger matches than
/// faces do.
#[test]
fn pets_are_people_in_a_space_of_their_own() {
    let lib = empty("people-pets");
    for n in 1..=3 {
        plain(&lib, &format!("Pets/spooky{n}.png"), SPOOKY, n);
    }
    for n in 1..=2 {
        plain(&lib, &format!("Pets/rex{n}.png"), REX, n);
    }
    variant(&lib, "Mix/pet_suggested.png", SPOOKY, PET_SUGGESTED);
    variant(&lib, "Mix/pet_maybe.png", SPOOKY, PET_MAYBE);
    variant(&lib, "Mix/pet_far.png", SPOOKY, PET_FAR);
    plain(&lib, "Anna/a1.png", ANNA, 1);
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    let pets = stats.pets.as_ref().expect("the pets pass ran");
    assert_eq!((pets.looked, pets.faces), (9, 9), "one pet in each of the nine pictures");
    let summary = stats.clusters.as_ref().unwrap();
    assert_eq!(summary.pets.as_ref().map(|a| a.faces), Some(9), "{summary:?}");

    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);

    // Faces and pets are clustered apart; `kind` picks one.
    let all = get(addr, "/api/clusters").json();
    let pets = get(addr, "/api/clusters?kind=pets").json();
    let humans = get(addr, "/api/clusters?kind=faces").json();
    assert_eq!(all["total"].as_u64().unwrap(), pets["total"].as_u64().unwrap() + humans["total"].as_u64().unwrap());
    assert_eq!(get(addr, "/api/clusters?kind=cats").status, 400);
    let sizes = |list: &Value| -> Vec<u64> { list["clusters"].as_array().unwrap().iter().map(|c| c["size"].as_u64().unwrap()).collect() };
    // Spooky's three plain pictures, Rex's two, and the three variants and
    // Anna's photo (the fake finds a pet in every picture) alone: 0.875
    // is below the 0.90 that joins pets.
    assert_eq!(sizes(&pets), [3, 2, 1, 1, 1, 1]);
    // The unnamed page lists clusters of up to two faces apart, to pick one by one.
    let large = get(addr, "/api/clusters?kind=pets&size=large").json();
    let small = get(addr, "/api/clusters?kind=pets&size=small").json();
    assert_eq!(sizes(&large), [3]);
    assert_eq!(sizes(&small).len(), 5);
    assert!(sizes(&small).iter().all(|&n| n <= 2));
    assert_eq!((small["unnamed"].as_u64(), large["small_faces"].as_u64()), (Some(6), Some(6)), "{small} {large}");
    assert_eq!(get(addr, "/api/clusters?size=huge").status, 400);
    // The Face check lists what recognition could not place: no name, nobody suggested.
    let unmatched = get(addr, "/api/faces?kind=pets&unmatched=true").json();
    let stats = get(addr, "/api/faces/stats?kind=pets").json();
    assert_eq!(unmatched["total"], stats["unmatched"], "{unmatched} {stats}");
    assert!(unmatched["total"].as_u64().unwrap() > 0);
    for c in pets["clusters"].as_array().unwrap() {
        assert!(c["faces"].as_array().unwrap().iter().all(|f| f["species"].is_string()), "{c}");
    }
    assert!(humans["clusters"].as_array().unwrap().iter().all(|c| c["faces"].as_array().unwrap().iter().all(|f| f["species"].is_null())));
    let species_of = |c: &Value| c["faces"][0]["species"].as_str().unwrap().to_string();
    assert_eq!((species_of(&pets["clusters"][0]), species_of(&pets["clusters"][1])), ("cat".into(), "dog".into()));

    // Name the cat's cluster like any person; the person is a pet.
    let body = json!({ "name": "Spooky", "generation": pets["clusters"][0]["generation"] });
    let named = ok(addr, &format!("/api/clusters/{}/name", pets["clusters"][0]["id"]), &body);
    assert_eq!(named["faces"], 3);
    let spooky = named["person"]["id"].as_i64().unwrap();
    let rex_body = json!({ "name": "Rex", "generation": pets["clusters"][1]["generation"] });
    let rex = ok(addr, &format!("/api/clusters/{}/name", pets["clusters"][1]["id"]), &rex_body)["person"]["id"].as_i64().unwrap();
    wait_for_clusters(addr);
    let people = get(addr, "/api/people").json();
    let by_name = |name: &str| people.as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone();
    assert_eq!(by_name("Spooky")["species"], "cat");
    assert_eq!(by_name("Rex")["species"], "dog");
    assert!(by_name("Spooky")["cover"].is_i64());

    // Suggestions among pets, at the pets' thresholds.
    let suggested = pet_of(addr, &lib, "Mix/pet_suggested.png");
    assert_eq!((suggested["state"].as_str(), suggested["person"]["name"].as_str()), (Some("suggested"), Some("Spooky")));
    assert!((suggested["similarity"].as_f64().unwrap() - 0.875).abs() < 0.01);
    let maybe = pet_of(addr, &lib, "Mix/pet_maybe.png");
    assert_eq!((maybe["state"].as_str(), maybe["person"]["name"].as_str()), (Some("maybe"), Some("Spooky")));
    // 0.575 would be a suggestion for a face; for a pet it is nothing.
    let far = pet_of(addr, &lib, "Mix/pet_far.png");
    assert!(far["state"].is_null() && far["person"].is_null(), "{far}");
    assert_eq!(files_of(&person_faces(addr, spooky, "suggested")), [id_of(&lib, "Mix/pet_suggested.png")]);
    assert_eq!(files_of(&person_faces(addr, spooky, "maybe")), [id_of(&lib, "Mix/pet_maybe.png")]);
    let spooky_row = by_name("Spooky");
    assert_eq!((spooky_row["suggested"].as_u64(), spooky_row["maybe"].as_u64()), (Some(1), Some(1)));

    // The faces of the same photos are not suggested for a pet: another space.
    for rel in ["Mix/pet_suggested.png", "Mix/pet_maybe.png", "Pets/rex1.png"] {
        let face = face_of_person(addr, &lib, rel);
        assert!(face["person"].is_null() || face["state"] == "confirmed", "{rel}: {face}");
    }
    assert!(face_of_person(addr, &lib, "Mix/pet_suggested.png")["person"].is_null());

    // Confirm the suggestion like any other; the pet's timeline has the photo.
    let id = pet_of(addr, &lib, "Mix/pet_suggested.png")["id"].as_i64().unwrap();
    assert_eq!(ok(addr, "/api/faces/confirm", &json!({ "faces": [id] }))["person"]["id"], spooky);
    let mut timeline = ids(&get(addr, &format!("/api/timeline?person={spooky}")).json());
    timeline.sort();
    let mut want: Vec<i64> = (1..=3).map(|n| id_of(&lib, &format!("Pets/spooky{n}.png"))).collect();
    want.push(id_of(&lib, "Mix/pet_suggested.png"));
    want.sort();
    assert_eq!(timeline, want);

    // Pets and people share groups: put the cat, the dog and a person in one.
    let family = ok(addr, "/api/groups", &json!({ "name": "Family", "person_id": spooky }))["id"].as_i64().unwrap();
    let anna_face = face_of_person(addr, &lib, "Anna/a1.png")["id"].as_i64().unwrap();
    let anna = ok(addr, "/api/faces/assign", &json!({ "faces": [anna_face], "name": "Anna" }))["person"]["id"].as_i64().unwrap();
    for p in [spooky, rex, anna] {
        ok(addr, &format!("/api/people/{p}/group"), &json!({ "group_id": family }));
    }
    let people = get(addr, "/api/people").json();
    assert!(people.as_array().unwrap().iter().all(|p| p["group_id"] == family), "{people}");
    // The group's timeline has the photos of the cat, the dog and the person.
    let mut group_timeline = ids(&get(addr, &format!("/api/timeline?group={family}")).json());
    group_timeline.sort();
    for n in 1..=3 {
        assert!(group_timeline.contains(&id_of(&lib, &format!("Pets/spooky{n}.png"))), "pets are members like people");
    }
    assert!(group_timeline.contains(&id_of(&lib, "Anna/a1.png")));
    assert!(people.as_array().unwrap().iter().filter(|p| p["name"] == "Anna").all(|p| p["species"].is_null()));

    // A cat can also be merged into another pet (the same pet under two names).
    let merged = ok(addr, &format!("/api/people/{rex}/merge"), &json!({ "into": spooky }));
    assert_eq!(merged["id"], spooky);

    assert_eq!(lib.snapshot(), before, "nothing here may change an original");
}

/// A pet the detector missed is drawn by hand with "pet" ticked: it is a
/// confirmed pet of its person (no species chosen), embedded with the pets
/// model by the recognizer, a reference for suggestions among pets, and over a
/// detected pet it is one face, never two. It never matches a person's face.
#[test]
fn a_pet_drawn_by_hand() {
    let lib = empty("people-drawn-pet");
    // A black picture with a small patch in Spooky's colour: the fake finds no
    // pet (nor a face) in it, as its mean colour is dark ("missed"), but a
    // box drawn around the patch embeds as Spooky.
    save(
        &lib,
        "Pets/missed.png",
        image::RgbImage::from_fn(600, 300, |x, y| image::Rgb(if (80..160).contains(&x) && (80..160).contains(&y) { SPOOKY } else { [0, 0, 0] })),
    );
    // 80 px wide: large enough to count as a reference (pets under 64 px do not).
    const PATCH: [f64; 4] = [80.0 / 600.0, 80.0 / 300.0, 80.0 / 600.0, 80.0 / 300.0];
    // Spooky as the fake sees her (not named by anyone), and a look-alike, 0.875
    // away: it can only be suggested for Spooky through the drawn pet.
    plain(&lib, "Pets/spooky.png", SPOOKY, 1);
    variant(&lib, "Mix/like_spooky.png", SPOOKY, PET_SUGGESTED);
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    assert_eq!(stats.pets.as_ref().unwrap().faces, 2, "the missed picture holds no pet");
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let missed = id_of(&lib, "Pets/missed.png");
    assert_eq!(faces_of(addr, &lib, "Pets/missed.png").len(), 0);
    assert!(pet_of(addr, &lib, "Mix/like_spooky.png")["state"].is_null(), "nobody is named yet");

    // Ticked "pet": a pet of an unknown species, confirmed for its person.
    let body = json!({ "file": missed, "box": PATCH, "name": "Spooky", "pet": true });
    let m = ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap();
    let faces = faces_of(addr, &lib, "Pets/missed.png");
    assert_eq!(faces.len(), 1);
    assert_eq!((faces[0]["manual"].as_i64(), faces[0]["species"].as_str(), faces[0]["state"].as_str()), (Some(m), Some("pet"), Some("confirmed")));
    let spooky = get(addr, "/api/people").json()[0].clone();
    assert_eq!(spooky["species"], "pet", "only a drawn pet so far: {spooky}");
    assert_eq!(spooky["cover_manual"], m);
    let spooky = spooky["id"].as_i64().unwrap();
    assert_eq!(get(addr, &format!("/api/faces/manual/{m}/crop")).status, 200);

    // The recognizer embeds the drawn pet at the end of its run (the server does
    // it in the background when one is drawn): with the pets model, aligned.
    recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    let c = conn(&lib);
    let (model, aligned, len): (String, bool, i64) = c
        .query_row(
            "SELECT model, aligned, length(emb) FROM recog.drawn WHERE key = (SELECT quick_hash FROM files WHERE path_nfc = 'Pets/missed.png')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((model.as_str(), aligned, len), ("fake-pets-1", true, 64 * 4));
    drop(c);
    wait_for_clusters(addr);
    // A reference among pets: the look-alike is suggested, through the drawn pet alone.
    let like = pet_of(addr, &lib, "Mix/like_spooky.png");
    assert_eq!((like["state"].as_str(), like["person"]["name"].as_str()), (Some("suggested"), Some("Spooky")), "{like}");
    // The person's face in the same photo is another space: not suggested.
    assert!(face_of_person(addr, &lib, "Mix/like_spooky.png")["person"].is_null());

    // Over a detected pet: one face, the drawn box, and the detected species stays.
    let detected = pet_of(addr, &lib, "Pets/spooky.png");
    let spooky_file = id_of(&lib, "Pets/spooky.png");
    let body = json!({ "file": spooky_file, "box": [0.27, 0.25, 0.5, 0.5], "person_id": spooky, "pet": true });
    let m2 = ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap();
    let faces = faces_of(addr, &lib, "Pets/spooky.png");
    let pets: Vec<&Value> = faces.iter().filter(|f| f["species"].is_string()).collect();
    assert_eq!(pets.len(), 1, "{faces:?}");
    assert_eq!((pets[0]["id"].as_i64(), pets[0]["manual"].as_i64()), (detected["id"].as_i64(), Some(m2)));
    assert_eq!(pets[0]["species"], "cat");
    assert_eq!(get(addr, &format!("/api/people/{spooky}")).json()["faces"], 2);
    // A decision about a pet never lands on the person's face of the same photo.
    assert!(face_of_person(addr, &lib, "Pets/spooky.png")["state"].is_null());

    // A pet drawn as a cat keeps that species; an unknown one is refused.
    let body = json!({ "file": missed, "box": [0.6, 0.1, 0.2, 0.4], "person_id": spooky, "species": "cat" });
    let m3 = ok(addr, "/api/faces/manual", &body)["manual"].as_i64().unwrap();
    let faces = faces_of(addr, &lib, "Pets/missed.png");
    assert_eq!(faces.iter().find(|f| f["manual"].as_i64() == Some(m3)).unwrap()["species"], "cat");
    let body = json!({ "file": missed, "box": [0.6, 0.6, 0.2, 0.3], "person_id": spooky, "species": "horse" });
    assert_eq!(post(addr, "/api/faces/manual", &body).status, 400);

    // A drawn pet can be deleted like a drawn face.
    assert_eq!(ok(addr, "/api/faces/undo", &json!({ "manual": [m, m2, m3] }))["faces"], 3);
    assert_eq!(faces_of(addr, &lib, "Pets/missed.png").len(), 0);
    assert_eq!(lib.snapshot(), before, "nothing here may change an original");
    server.stop().unwrap();
}

/// A pet drawn while `serve` runs is embedded in the background, with the pets
/// model, even if no pets pass has ever run (the worker is started for pets
/// only because one is waiting); under the guard.
#[test]
fn a_pet_drawn_while_serving_is_embedded_in_the_background() {
    let lib = empty("people-drawn-pet-bg");
    save(
        &lib,
        "Pets/missed.png",
        image::RgbImage::from_fn(600, 300, |x, y| image::Rgb(if (80..160).contains(&x) && (80..160).contains(&y) { SPOOKY } else { [0, 0, 0] })),
    );
    lib.scan_opts(true, false, false);
    recognize(&lib); // faces only
    assert_eq!(conn(&lib).query_row("SELECT count(*) FROM recog.looked WHERE task = 'pets'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let body = json!({ "file": id_of(&lib, "Pets/missed.png"), "box": [80.0 / 600.0, 80.0 / 300.0, 80.0 / 600.0, 80.0 / 300.0], "name": "Spooky", "pet": true });
    ok(addr, "/api/faces/manual", &body);
    let drawn = || -> Vec<(String, bool, bool)> {
        conn(&lib)
            .prepare("SELECT model, aligned, emb IS NOT NULL FROM recog.drawn")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    wait_until("the pet's embedding", || !drawn().is_empty() && get(addr, "/api/info").json()["clusters"]["embedding"] == false);
    assert_eq!(drawn(), [("fake-pets-1".to_string(), true, true)]);
    assert_eq!(lib.snapshot(), before, "embedding changed an original");
    server.stop().unwrap();
}

/// `thumbs.db` keeps a crop only for faces waiting for a decision and for
/// the picture shown for each person; deciding a face (or choosing another
/// picture) removes the crops nobody needs, also at the next start, and
/// "Use as … picture" works from a photo too.
#[test]
fn only_faces_waiting_for_a_decision_and_pictures_keep_a_crop() {
    let lib = empty("people-crops");
    for n in 1..=3 {
        plain(&lib, &format!("Anna/a{n}.png"), ANNA, n);
    }
    plain(&lib, "Ben/b1.png", BEN, 1);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let thumbs = Connection::open(lib.path(".shoebox/thumbs.db")).unwrap();
    let stored = || -> i64 { thumbs.query_row("SELECT count(*) FROM faces WHERE jpeg IS NOT NULL", [], |r| r.get(0)).unwrap() };
    let rels = ["Anna/a1.png", "Anna/a2.png", "Anna/a3.png", "Ben/b1.png"];
    let ids: Vec<i64> = rels.iter().map(|r| face_id(addr, &lib, r)).collect();
    let crop = |id: i64| get(addr, &format!("/api/faces/{id}/crop")).status;

    // Nothing is decided: every face waits for a name and keeps its crop.
    assert!(ids.iter().all(|&id| crop(id) == 200));
    assert_eq!(stored(), 4);
    let saved: Vec<(String, f64, f64, f64, f64, Vec<u8>, i64)> = thumbs
        .prepare("SELECT key, x, y, w, h, jpeg, made_at FROM faces")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    // Anna's three faces are confirmed: Ben's face and Anna's picture stay.
    ok(addr, "/api/faces/assign", &json!({ "faces": &ids[..3], "name": "Anna" }));
    let people = get(addr, "/api/people").json();
    let anna = people[0]["id"].as_i64().unwrap();
    let cover = people[0]["cover"].as_i64().unwrap();
    assert!(ids[..3].contains(&cover), "the picture is one of her faces");
    assert_eq!(stored(), 2);
    // A decided face can still be shown (made again, not stored).
    assert!(ids.iter().all(|&id| crop(id) == 200));
    assert_eq!(stored(), 2);

    // Another picture, from a photo: the old one goes, the new one is made
    // when it is shown. Ben has no confirmed face; both ways at once is wrong.
    let next = if cover == ids[0] { 1 } else { 0 };
    let file = id_of(&lib, rels[next]);
    let changed = ok(addr, &format!("/api/people/{anna}/cover"), &json!({ "file": file }));
    assert_eq!(changed["cover"], ids[next]);
    assert_eq!(stored(), 1);
    assert_eq!(crop(ids[next]), 200);
    assert_eq!(stored(), 2);
    let ben_file = id_of(&lib, "Ben/b1.png");
    assert_eq!(post(addr, &format!("/api/people/{anna}/cover"), &json!({ "file": ben_file })).status, 400);
    assert_eq!(post(addr, &format!("/api/people/{anna}/cover"), &json!({ "file": file, "face": ids[0] })).status, 400);
    assert_eq!(get(addr, &format!("/api/people/{anna}")).json()["cover"], ids[next]);

    // Ben's face is "not a face": nothing waits for a decision any more.
    ok(addr, "/api/faces/not-face", &json!({ "faces": [ids[3]] }));
    assert_eq!(stored(), 1);
    server.stop().unwrap();

    // Crops an older version stored for every face are removed at the start,
    // and the picture stays.
    for r in &saved {
        thumbs
            .execute("INSERT OR REPLACE INTO faces (key, x, y, w, h, jpeg, error, made_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                rusqlite::params![r.0, r.1, r.2, r.3, r.4, r.5, r.6])
            .unwrap();
    }
    assert_eq!(stored(), 4);
    let server = start(&lib, None);
    assert_eq!(stored(), 1);
    assert_eq!(get(server.addr, &format!("/api/people/{anna}")).json()["cover"], ids[next]);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "the crops changed an original");
}

/// Searching by pet: `pet=cat|dog|pet` as a term, "cat", "katze", "hund",
/// "pets" as typed words, together with other terms (all must match), the
/// suggestions with their counts; named or not, drawn by hand too, and not
/// the detections marked "not a face". Only reads originals.
#[test]
fn searching_by_pet_species() {
    let lib = empty("people-pet-search");
    // The fake finds a cat in a red picture and a dog in a blue one.
    plain(&lib, "Fotos/miez1.png", SPOOKY, 1);
    plain(&lib, "Fotos/miez2.png", SPOOKY, 2);
    plain(&lib, "Fotos/bello.png", REX, 1);
    plain(&lib, "Fotos/falsch.png", SPOOKY, 3); // a plush toy, marked "not a pet" below
    save(&lib, "Fotos/leer.png", image::RgbImage::from_pixel(310, 150, image::Rgb([0, 0, 0]))); // other bytes than gemalt.png
    save(&lib, "Fotos/gemalt.png", image::RgbImage::from_pixel(300, 150, image::Rgb([0, 0, 0])));
    lib.scan_opts(true, false, false);
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let id = |rel: &str| id_of(&lib, rel);
    let found = |path: &str| sorted(ids(&get(addr, path).json()));
    let want = |rels: &[&str]| sorted(rels.iter().map(|r| id(r)).collect());

    // A plush toy the detector took for a cat, and a pet drawn by hand.
    let toy = pet_of(addr, &lib, "Fotos/falsch.png")["id"].as_i64().unwrap();
    ok(addr, "/api/faces/not-face", &json!({ "faces": [toy] }));
    let body = json!({ "file": id("Fotos/gemalt.png"), "box": [0.1, 0.2, 0.4, 0.6], "name": "Fleck", "pet": true });
    ok(addr, "/api/faces/manual", &body);

    // The three terms.
    assert_eq!(found("/api/timeline?pet=cat"), want(&["Fotos/miez1.png", "Fotos/miez2.png"]));
    assert_eq!(found("/api/timeline?pet=dog"), want(&["Fotos/bello.png"]));
    assert_eq!(
        found("/api/timeline?pet=pet"),
        want(&["Fotos/miez1.png", "Fotos/miez2.png", "Fotos/bello.png", "Fotos/gemalt.png"]),
        "any pet: cats, dogs and the drawn one, not the toy"
    );
    // All terms must match: nothing is both.
    assert!(found("/api/timeline?pet=cat&pet=dog").is_empty());
    assert_eq!(found("/api/timeline?pet=cat&pet=pet"), want(&["Fotos/miez1.png", "Fotos/miez2.png"]));
    assert!(get(addr, "/api/timeline?pet=cow").status == 400);

    // With other terms: a person (named by drawing "Fleck"), free text.
    let fleck = get(addr, "/api/people").json().as_array().unwrap().iter().find(|p| p["name"] == "Fleck").unwrap()["id"].as_i64().unwrap();
    assert_eq!(found(&format!("/api/timeline?pet=pet&person={fleck}")), want(&["Fotos/gemalt.png"]));
    assert_eq!(found("/api/timeline?pet=cat&q=miez2"), want(&["Fotos/miez2.png"]));

    // Typed words, English and German; too short or only the start of other words, nothing.
    for word in ["cat", "cats", "katze", "Katzen", "kat"] {
        assert_eq!(found(&format!("/api/timeline?q={word}")), want(&["Fotos/miez1.png", "Fotos/miez2.png"]), "{word}");
    }
    for word in ["dog", "hund", "Hunde"] {
        assert_eq!(found(&format!("/api/timeline?q={word}")), want(&["Fotos/bello.png"]), "{word}");
    }
    assert_eq!(found("/api/timeline?q=haustier").len(), 4);
    assert_eq!(found("/api/timeline?q=pets").len(), 4);
    assert!(found("/api/timeline?q=ca").is_empty() && found("/api/timeline?q=catalog").is_empty());
    // A word and a term together.
    assert_eq!(found("/api/timeline?q=katze+miez1"), want(&["Fotos/miez1.png"]));

    // The suggestions: terms that fit what is typed and show something, with counts.
    let hits = |path: &str| -> Vec<(String, u64)> {
        get(addr, path).json().as_array().unwrap().iter().map(|h| (h["species"].as_str().unwrap().to_string(), h["photos"].as_u64().unwrap())).collect()
    };
    assert_eq!(hits("/api/pets/search"), [("pet".into(), 4), ("cat".into(), 2), ("dog".into(), 1)]);
    assert_eq!(hits("/api/pets/search?q=kat"), [("cat".into(), 2)]);
    assert_eq!(hits("/api/pets/search?q=hund"), [("dog".into(), 1)]);
    assert!(hits("/api/pets/search?q=xyz").is_empty());
    // Within a search: only what would still show something, and not what is already in it.
    assert_eq!(hits("/api/pets/search?pet=cat"), [("pet".into(), 2)]);
    assert!(hits("/api/pets/search?pet=cat&q=hund").is_empty(), "no dogs among the cats");
    assert_eq!(hits(&format!("/api/pets/search?person={fleck}")), [("pet".into(), 1)]);
    assert_eq!(get(addr, "/api/pets/search?pet=cow").status, 400);

    // The tag suggestions narrow by the pet term too.
    ok(addr, "/api/tags/add", &json!({ "ids": [id("Fotos/miez1.png")], "name": "Sofa" }));
    let tags = get(addr, "/api/tags?pet=dog&limit=50").json();
    assert!(tags.as_array().unwrap().iter().all(|t| t["name"] != "Sofa"), "no Sofa among the dogs: {tags}");

    // Undoing "not a pet" brings the photo back.
    ok(addr, "/api/faces/undo", &json!({ "faces": [toy] }));
    assert_eq!(found("/api/timeline?pet=cat"), want(&["Fotos/miez1.png", "Fotos/miez2.png", "Fotos/falsch.png"]));
    assert_eq!(lib.snapshot(), before, "searching changed an original");
    server.stop().unwrap();
}

/// The face detector also finds a dog's or cat's face. When the pets pass
/// found a pet around it and no person, the face is the pet's: not listed
/// as a face (viewer, clusters, face check) unless the user decided on it.
#[test]
fn a_pets_face_is_not_a_persons() {
    let lib = empty("people-pets-face");
    // Nobody in the picture: a yellow bottom quarter (the fake's cue).
    let dog = image::RgbImage::from_fn(200, 200, |_, y| image::Rgb(if y >= 150 { [255, 255, 0] } else { [90, 90, 200] }));
    save(&lib, "Dog/dog.png", dog.clone());
    let mut other = dog;
    other.put_pixel(0, 0, image::Rgb([1, 2, 3]));
    save(&lib, "Dog/dog2.png", other);
    // A person (and a pet) in the picture: the face stays.
    plain(&lib, "Anna/a1.png", ANNA, 1);
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    let after = lib.snapshot();
    assert_eq!(before, after, "originals untouched");

    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let boxes = |rel: &str| faces_of(addr, &lib, rel);
    // The dog's pictures show the dog only; Anna's both.
    for rel in ["Dog/dog.png", "Dog/dog2.png"] {
        let faces = boxes(rel);
        assert_eq!(faces.len(), 1, "{rel}: {faces:?}");
        assert!(faces[0]["species"].is_string(), "{rel}: {faces:?}");
    }
    assert_eq!(boxes("Anna/a1.png").len(), 2);

    // Not in the face clusters, not in the face check; counted apart.
    let humans = get(addr, "/api/clusters?kind=faces").json();
    let in_clusters: u64 = humans["clusters"].as_array().unwrap().iter().map(|c| c["size"].as_u64().unwrap()).sum();
    assert_eq!(in_clusters, 1, "{humans}");
    let listed = get(addr, "/api/faces").json();
    assert_eq!(listed["total"], 1, "{listed}");
    let hidden = get(addr, "/api/faces?pet_face=true").json();
    assert_eq!(hidden["total"], 2, "{hidden}");
    let stats = get(addr, "/api/faces/stats").json();
    assert_eq!((stats["faces"].as_u64(), stats["pet_faces"].as_u64()), (Some(1), Some(2)), "{stats}");

    // Drawing a face by hand settles it: that face is shown again.
    let file = id_of(&lib, "Dog/dog.png");
    ok(addr, "/api/faces/manual", &json!({ "file": file, "box": [0.25, 0.25, 0.5, 0.5], "name": "Dog person" }));
    let faces = boxes("Dog/dog.png");
    assert!(faces.iter().any(|f| f["species"].is_null()), "{faces:?}");
}

/// A dog the detector now and then takes for a cat: once the photo is
/// Layka's, the box, the person and the pet search all say dog.
#[test]
fn a_named_pets_species_follows_the_person() {
    let lib = empty("people-pets-species");
    plain(&lib, "Layka/a.png", REX, 1);
    plain(&lib, "Layka/b.png", REX, 2);
    plain(&lib, "Layka/c.png", REX, 3);
    plain(&lib, "Layka/cat.png", SPOOKY, 1); // reads as a cat
    plain(&lib, "Other/cat.png", SPOOKY, 2);
    lib.scan_opts(true, false, false);
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let pet = |rel: &str| pet_of(addr, &lib, rel);
    assert_eq!(pet("Layka/cat.png")["species"], "cat");

    let faces: Vec<i64> = ["Layka/a.png", "Layka/b.png", "Layka/c.png", "Layka/cat.png"].iter().map(|r| pet(r)["id"].as_i64().unwrap()).collect();
    let layka = ok(addr, "/api/faces/assign", &json!({ "faces": faces, "name": "Layka" }))["person"]["id"].as_i64().unwrap();
    wait_for_clusters(addr);
    let people = get(addr, "/api/people").json();
    let row = people.as_array().unwrap().iter().find(|p| p["id"] == layka).unwrap().clone();
    assert_eq!(row["species"], "dog", "three dogs and one cat: a dog");
    // The info panel: the "cat" is Layka the dog; the other cat stays one.
    assert_eq!(pet("Layka/cat.png")["species"], "dog");
    assert_eq!(pet("Layka/cat.png")["person"]["name"], "Layka");
    assert_eq!(pet("Other/cat.png")["species"], "cat");

    let found = |term: &str| sorted(ids(&get(addr, &format!("/api/timeline?pet={term}")).json()));
    let want = |rels: &[&str]| sorted(rels.iter().map(|r| id_of(&lib, r)).collect());
    assert_eq!(found("dog"), want(&["Layka/a.png", "Layka/b.png", "Layka/c.png", "Layka/cat.png"]));
    assert_eq!(found("cat"), want(&["Other/cat.png"]), "Layka is no cat");
    assert_eq!(found("pet").len(), 5);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "originals untouched");
}

/// A cat the detector took for a dog: the user says which it is. The box,
/// the pet search and a person made from it all follow, decisions made before
/// stay with the pet, and it is kept across a restart.
#[test]
fn a_pets_species_can_be_corrected() {
    let lib = empty("people-pets-correct");
    plain(&lib, "Fotos/katze.png", SPOOKY, 1); // reads as a cat; this one is a dog
    plain(&lib, "Fotos/echt.png", SPOOKY, 2);
    plain(&lib, "Fotos/hund.png", REX, 1);
    lib.scan_opts(true, false, false);
    let stats = recognize::run(&recognize::Options { pets: true, ..options(&lib) }).unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let pet = |rel: &str| pet_of(addr, &lib, rel);
    let found = |term: &str| sorted(ids(&get(addr, &format!("/api/timeline?pet={term}")).json()));
    let want = |rels: &[&str]| sorted(rels.iter().map(|r| id_of(&lib, r)).collect());
    assert_eq!(pet("Fotos/katze.png")["species"], "cat");

    // Confirmed while it was a "cat", then corrected: the decision follows.
    let id = pet("Fotos/katze.png")["id"].as_i64().unwrap();
    ok(addr, "/api/faces/assign", &json!({ "faces": [id], "name": "Rex" }));
    let done = ok(addr, "/api/faces/species", &json!({ "faces": [id], "species": "dog" }));
    assert_eq!(done["faces"], 1);
    let after = pet("Fotos/katze.png");
    assert_eq!((after["species"].as_str(), after["state"].as_str(), after["person"]["name"].as_str()), (Some("dog"), Some("confirmed"), Some("Rex")));
    assert_eq!(found("dog"), want(&["Fotos/katze.png", "Fotos/hund.png"]));
    assert_eq!(found("cat"), want(&["Fotos/echt.png"]));
    // Saying it again, or about a person's face, changes nothing.
    assert_eq!(ok(addr, "/api/faces/species", &json!({ "faces": [id], "species": "dog" }))["faces"], 0);
    let person_face = face_of_person(addr, &lib, "Fotos/katze.png")["id"].as_i64().unwrap();
    assert_eq!(ok(addr, "/api/faces/species", &json!({ "faces": [person_face], "species": "cat" }))["faces"], 0);

    // Not a species; nothing said.
    for body in [json!({ "faces": [id], "species": "horse" }), json!({ "faces": [id] })] {
        let r = post(addr, "/api/faces/species", &body);
        assert_eq!(r.status, 400, "{body}");
    }

    // It is kept in the library, not in the server.
    server.stop().unwrap();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    assert_eq!(pet_of(addr, &lib, "Fotos/katze.png")["species"], "dog");
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "originals untouched");
}

/// What the server shows of people and clusters, compared with what a
/// fresh read of the library gives (`people::people`, `people::clusters` on a
/// connection of the test's own): the photo app keeps what it read of the
/// faces between requests (`App::people_view`), and this must never show.
fn same_as_a_fresh_read(addr: std::net::SocketAddr, lib: &Library, after: &str) {
    wait_for_clusters(addr);
    let conn = conn(lib);
    let fresh = as_sent(&people::people(&conn, true).unwrap());
    assert_eq!(get(addr, "/api/people?hidden=1").json(), fresh, "the people after {after}");
    let fresh = as_sent(&people::clusters(&conn, 0, 50, 8, None, people::Size::All).unwrap());
    assert_eq!(get(addr, "/api/clusters").json(), fresh, "the clusters after {after}");
}

/// As the server sends it: written out and read back (an f32 similarity as
/// written, not widened to f64).
fn as_sent(v: &impl serde::Serialize) -> Value {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

fn listed(addr: std::net::SocketAddr, hidden: bool) -> Vec<Value> {
    get(addr, if hidden { "/api/people?hidden=1" } else { "/api/people" }).json().as_array().unwrap().clone()
}

fn listed_person(addr: std::net::SocketAddr, id: i64) -> Option<Value> {
    listed(addr, true).into_iter().find(|p| p["id"] == id)
}

/// The people view the server keeps follows every change at once: names,
/// rejections, merging, deleting, hiding, groups, a `recognize` run and a scan
/// in other processes. After each, the lists are the same as a fresh read.
#[test]
fn the_kept_people_view_follows_every_change() {
    let lib = empty("people-kept-view");
    for n in 1..=3 {
        plain(&lib, &format!("Anna/a{n}.png"), ANNA, n);
    }
    plain(&lib, "Ben/b1.png", BEN, 1);
    plain(&lib, "Ben/b2.png", BEN, 2);
    lib.scan();
    recognize(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    same_as_a_fresh_read(addr, &lib, "the start");
    assert!(listed(addr, true).is_empty());

    // Name both clusters: in the lists at once, before the regrouping.
    let clusters = get(addr, "/api/clusters").json()["clusters"].as_array().unwrap().clone();
    assert_eq!(clusters.iter().map(|c| c["size"].as_u64().unwrap()).collect::<Vec<_>>(), [3, 2]);
    let name = |c: &Value, name: &str| {
        let body = json!({ "name": name, "generation": c["generation"] });
        ok(addr, &format!("/api/clusters/{}/name", c["id"]), &body)["person"]["id"].as_i64().unwrap()
    };
    let anna = name(&clusters[0], "Anna");
    assert_eq!(listed_person(addr, anna).unwrap()["faces"], 3);
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 2);
    let ben = name(&clusters[1], "Ben");
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 0);
    same_as_a_fresh_read(addr, &lib, "naming");

    // Reject one of Anna's faces: no longer hers, listed as rejected.
    let a1 = face_id(addr, &lib, "Anna/a1.png");
    ok(addr, "/api/faces/reject", &json!({ "faces": [a1], "person_id": anna }));
    assert_eq!(listed_person(addr, anna).unwrap()["faces"], 2);
    assert_eq!(files_of(&person_faces(addr, anna, "rejected")), [id_of(&lib, "Anna/a1.png")]);
    same_as_a_fresh_read(addr, &lib, "a rejection");

    // Hide Ben, then a group for Anna that Ben joins.
    ok(addr, &format!("/api/people/{ben}/hide"), &json!({ "hidden": true }));
    assert!(!listed(addr, false).iter().any(|p| p["id"] == ben));
    assert_eq!(listed_person(addr, ben).unwrap()["hidden"], true);
    let group = ok(addr, "/api/groups", &json!({ "name": "Family", "person_id": anna }))["id"].clone();
    assert_eq!(listed_person(addr, anna).unwrap()["group_id"], group);
    ok(addr, &format!("/api/people/{ben}/group"), &json!({ "group_id": group }));
    assert_eq!(listed_person(addr, ben).unwrap()["group_id"], group);
    same_as_a_fresh_read(addr, &lib, "hiding and a group");

    // Merge Ben into Anna: his two faces are hers.
    ok(addr, &format!("/api/people/{ben}/merge"), &json!({ "into": anna }));
    assert!(listed_person(addr, ben).is_none());
    assert_eq!(listed_person(addr, anna).unwrap()["faces"], 4);
    same_as_a_fresh_read(addr, &lib, "merging");

    // Delete someone without faces (a misspelt name).
    let typo = ok(addr, "/api/people", &json!({ "name": "Anan" }))["id"].as_i64().unwrap();
    assert!(listed_person(addr, typo).is_some());
    ok(addr, &format!("/api/people/{typo}/delete"), &json!({}));
    assert!(listed_person(addr, typo).is_none());
    same_as_a_fresh_read(addr, &lib, "deleting");
    assert_eq!(lib.snapshot(), before, "people changed an original");

    // New photos, recognised by another process: their faces show up, the
    // new one of Anna's as suggested for her.
    // (The face rejected for Anna is unnamed again.)
    let unnamed = get(addr, "/api/clusters").json()["unnamed"].as_u64().unwrap();
    assert_eq!(unnamed, 1);
    plain(&lib, "Anna/a4.png", ANNA, 4);
    plain(&lib, "Carl/c1.png", CARL, 1);
    let shoebox = env!("CARGO_BIN_EXE_shoebox");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(shoebox).args(args).arg(&lib.root).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["scan"]);
    run(&["recognize", "--recognizer", FAKE]);
    let mut before = lib.snapshot();
    wait_until("the new faces show", || get(addr, "/api/clusters").json()["unnamed"] == unnamed + 2);
    same_as_a_fresh_read(addr, &lib, "a recognize run in another process");
    let suggested = person_faces(addr, anna, "suggested");
    assert_eq!(files_of(&suggested), [id_of(&lib, "Anna/a4.png")]);

    // A photo removed, found by a scan in another process: one photo fewer
    // for Anna (her confirmed face there still counts as a face).
    let photos = listed_person(addr, anna).unwrap()["photos"].as_u64().unwrap();
    std::fs::remove_file(lib.path("Anna/a2.png")).unwrap();
    before.remove(&lib.path("Anna/a2.png"));
    run(&["scan"]);
    wait_until("the photo is gone", || listed_person(addr, anna).unwrap()["photos"] == photos - 1);
    assert_eq!(listed_person(addr, anna).unwrap()["faces"], 4);
    same_as_a_fresh_read(addr, &lib, "a scan that removed a photo");

    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "people changed an original");
}

// ------------------------------------------------------- when to regroup

/// Colours of people for the regrouping tests: far apart for the fake
/// (`person_number`), none of them a cue.
const PEOPLE: [[u8; 3]; 8] =
    [ANNA, BEN, CARL, DORA, [150, 200, 60], [60, 150, 200], [180, 80, 140], [100, 180, 160]];

/// `people` people with two photos each (one card each), recognised, with a
/// server that regroups by `rules`, after its first grouping.
fn cards_library(test: &str, people: usize, rules: shoebox::serve::RegroupRules) -> (Library, shoebox::serve::Server) {
    let lib = empty(test);
    for (p, rgb) in PEOPLE.iter().take(people).enumerate() {
        plain(&lib, &format!("p{p}/1.png"), *rgb, 1);
        plain(&lib, &format!("p{p}/2.png"), *rgb, 2);
    }
    lib.scan();
    recognize(&lib);
    let server = start_regrouping(&lib, rules);
    wait_for_clusters(server.addr);
    (lib, server)
}

/// Regroupings started so far (the jobs of every run, the server's and
/// `recognize`'s).
fn regroupings(lib: &Library) -> i64 {
    conn(lib).query_row("SELECT count(*) FROM recog.jobs WHERE kind = 'clusters'", [], |r| r.get(0)).unwrap()
}

/// Wait up to `max` for more than `before` regroupings to have started.
fn regrouped_within(lib: &Library, before: i64, max: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < max {
        if regroupings(lib) > before {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// The cards of the Unnamed page as they are now.
fn cards(addr: std::net::SocketAddr) -> Vec<Value> {
    get(addr, "/api/clusters?limit=50").json()["clusters"].as_array().unwrap().clone()
}

/// Name the first card on the Unnamed page; its face ids.
fn name_first(addr: std::net::SocketAddr, name: &str) -> Vec<i64> {
    let c = cards(addr).remove(0);
    ok(addr, &format!("/api/clusters/{}/name", c["id"]), &json!({ "name": name, "generation": c["generation"] }));
    c["faces"].as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect()
}

fn next_regrouping(addr: std::net::SocketAddr) -> Value {
    get(addr, "/api/info").json()["clusters"].clone()
}

fn rules(cards: u32, pause_ms: u64, soon_ms: u64) -> shoebox::serve::RegroupRules {
    shoebox::serve::RegroupRules { cards, pause: Duration::from_millis(pause_ms), soon: Duration::from_millis(soon_ms) }
}

/// N named cards start a regrouping before the pause is over; `/api/info`
/// says how many cards are left and how long the pause still is.
#[test]
fn n_named_cards_start_a_regrouping_before_the_pause() {
    let (lib, server) = cards_library("regroup-cards", 4, rules(3, 60_000, 200));
    let addr = server.addr;
    let guard = lib.snapshot();
    let c = next_regrouping(addr);
    assert_eq!((&c["pending"], &c["cards_left"], &c["wait_ms"]), (&json!(false), &json!(0), &json!(0)), "nothing waits: {c}");
    let before = regroupings(&lib);

    name_first(addr, "P0");
    let c = next_regrouping(addr);
    assert_eq!((&c["pending"], &c["cards_left"], &c["stale"]), (&json!(true), &json!(2), &json!(false)), "{c}");
    let wait = c["wait_ms"].as_u64().unwrap();
    assert!(wait > 55_000 && wait <= 60_000, "{c}");
    assert_eq!(c["pause_ms"], 60_000);
    name_first(addr, "P1");
    assert_eq!(next_regrouping(addr)["cards_left"], 1);
    assert!(!regrouped_within(&lib, before, Duration::from_millis(400)), "two cards are not enough");

    name_first(addr, "P2");
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)), "the third card starts it");
    let c = wait_for_clusters(addr);
    assert_eq!((&c["pending"], &c["cards_left"], &c["wait_ms"]), (&json!(false), &json!(0), &json!(0)), "{c}");
    assert_eq!(regroupings(&lib), before + 1, "one regrouping for the three cards");
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}

/// Fewer than N cards: the pause starts it; every decision starts the pause
/// again (a rejection too, without counting as a card).
#[test]
fn the_pause_starts_a_regrouping_and_each_decision_restarts_it() {
    let (lib, server) = cards_library("regroup-pause", 3, rules(10, 1500, 200));
    let addr = server.addr;
    let guard = lib.snapshot();
    let before = regroupings(&lib);
    let faces = name_first(addr, "P0");
    std::thread::sleep(Duration::from_millis(900));
    // A rejection restarts the pause but is no card.
    let other = cards(addr).remove(0)["faces"][0]["id"].as_i64().unwrap();
    let p0 = get(addr, "/api/people").json()[0]["id"].as_i64().unwrap();
    ok(addr, "/api/faces/reject", &json!({ "faces": [other], "person_id": p0 }));
    let c = next_regrouping(addr);
    assert_eq!(c["cards_left"], 9, "{c}");
    assert!(c["wait_ms"].as_u64().unwrap() > 1200, "the pause starts again: {c}");
    std::thread::sleep(Duration::from_millis(900));
    name_first(addr, "P1");
    // 1.8 s after the first name, 0.9 s after the rejection: still waiting.
    assert!(!regrouped_within(&lib, before, Duration::from_millis(900)), "each decision restarts the pause");
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)), "the pause is over");
    wait_for_clusters(addr);
    assert_eq!(regroupings(&lib), before + 1);
    assert!(faces.len() == 2);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}

/// Leaving the Unnamed page, undoing a decision made before the last
/// regrouping, taking back a rejection and merging two people start one
/// soon; undoing a newer decision shows at once and only restarts the pause.
#[test]
fn leaving_an_old_undo_an_unreject_and_a_merge_regroup_soon() {
    let (lib, server) = cards_library("regroup-soon", 4, rules(10, 60_000, 200));
    let addr = server.addr;
    let guard = lib.snapshot();
    let soon = Duration::from_secs(2);

    // Nothing waits: leaving the page starts nothing.
    let before = regroupings(&lib);
    ok(addr, "/api/clusters/left", &json!({}));
    assert!(!next_regrouping(addr)["pending"].as_bool().unwrap());
    assert!(!regrouped_within(&lib, before, Duration::from_millis(500)));

    // A name, then leaving: soon.
    let old = name_first(addr, "P0");
    ok(addr, "/api/clusters/left", &json!({}));
    assert!(next_regrouping(addr)["wait_ms"].as_u64().unwrap() <= 200);
    assert!(regrouped_within(&lib, before, soon), "leaving the page");
    wait_for_clusters(addr);

    // Undo of a decision made since then: back on the page at once, no
    // regrouping needed (only the pause starts).
    let before = regroupings(&lib);
    let new = name_first(addr, "P1");
    let left = cards(addr).len();
    ok(addr, "/api/faces/undo", &json!({ "faces": new }));
    assert_eq!(cards(addr).len(), left + 1, "the card is back at once");
    assert!(next_regrouping(addr)["wait_ms"].as_u64().unwrap() > 50_000, "{}", next_regrouping(addr));
    assert!(!regrouped_within(&lib, before, Duration::from_millis(500)));

    // Undo of the decision made before the last regrouping: only a
    // regrouping brings the card back, soon.
    let left = cards(addr).len();
    ok(addr, "/api/faces/undo", &json!({ "faces": old }));
    assert!(next_regrouping(addr)["wait_ms"].as_u64().unwrap() <= 200);
    assert!(regrouped_within(&lib, before, soon), "an old undo");
    wait_for_clusters(addr);
    assert_eq!(cards(addr).len(), left + 1, "the card is back after the regrouping");

    // A rejection taken back: soon.
    let a = name_first(addr, "Anna");
    let anna = get(addr, "/api/people").json().as_array().unwrap().iter().find(|p| p["name"] == "Anna").unwrap()["id"].as_i64().unwrap();
    let other = cards(addr).remove(0)["faces"][0]["id"].as_i64().unwrap();
    ok(addr, "/api/faces/reject", &json!({ "faces": [other], "person_id": anna }));
    ok(addr, "/api/clusters/left", &json!({}));
    wait_for_clusters(addr);
    let before = regroupings(&lib);
    ok(addr, "/api/faces/unreject", &json!({ "faces": [other], "person_id": anna }));
    assert!(regrouped_within(&lib, before, soon), "a rejection taken back");
    wait_for_clusters(addr);

    // A merge: soon, without any card named.
    let before = regroupings(&lib);
    name_first(addr, "Ben");
    ok(addr, "/api/clusters/left", &json!({}));
    wait_for_clusters(addr);
    let before_merge = regroupings(&lib);
    assert!(before_merge > before);
    let ben = get(addr, "/api/people").json().as_array().unwrap().iter().find(|p| p["name"] == "Ben").unwrap()["id"].as_i64().unwrap();
    ok(addr, &format!("/api/people/{ben}/merge"), &json!({ "into": anna }));
    assert!(regrouped_within(&lib, before_merge, soon), "a merge");
    wait_for_clusters(addr);

    // A rename or a group changes no suggestion: nothing.
    let before = regroupings(&lib);
    ok(addr, &format!("/api/people/{anna}/rename"), &json!({ "name": "Anna B." }));
    ok(addr, "/api/groups", &json!({ "name": "Family", "person_id": anna }));
    assert!(!next_regrouping(addr)["pending"].as_bool().unwrap());
    assert!(!regrouped_within(&lib, before, Duration::from_millis(500)));
    assert!(a.len() == 2);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}

/// Names given while a regrouping runs count towards the next one; the one
/// running finishes (its result is still right: the new names are seen
/// through).
#[test]
fn naming_during_a_regrouping_counts_towards_the_next() {
    let (lib, server) = cards_library("regroup-during", 6, rules(2, 60_000, 200));
    let addr = server.addr;
    let guard = lib.snapshot();
    let before = regroupings(&lib);
    name_first(addr, "P0");
    name_first(addr, "P1");
    // The second card started it; the third comes while it starts or runs,
    // or after it, and waits for the next one either way.
    name_first(addr, "P2");
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)));
    let c = wait_for_clusters_or_pending(addr);
    assert_eq!((&c["pending"], &c["cards_left"]), (&json!(true), &json!(1)), "{c}");
    assert_eq!(regroupings(&lib), before + 1);
    // The named cards are off the page; the fourth starts the next one.
    assert_eq!(cards(addr).len(), 3);
    name_first(addr, "P3");
    assert!(regrouped_within(&lib, before + 1, Duration::from_secs(2)));
    wait_for_clusters(addr);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}

/// The regrouping that runs is done; decisions may still wait.
fn wait_for_clusters_or_pending(addr: std::net::SocketAddr) -> Value {
    let started = Instant::now();
    loop {
        let c = next_regrouping(addr);
        if c["stale"] == false && c["running"] == false {
            return c;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "clustering did not finish: {c}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The last card named: nothing is left to name, so it starts soon.
#[test]
fn the_last_card_named_regroups_soon() {
    let (lib, server) = cards_library("regroup-last", 2, rules(10, 60_000, 200));
    let addr = server.addr;
    let guard = lib.snapshot();
    let before = regroupings(&lib);
    name_first(addr, "P0");
    assert!(next_regrouping(addr)["wait_ms"].as_u64().unwrap() > 50_000);
    name_first(addr, "P1");
    assert!(cards(addr).is_empty());
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)), "no card left");
    wait_for_clusters(addr);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}

// ------------------------------------------------- incremental suggestions

/// The face of `rel` (one per picture with the fake).
fn face_of(c: &Connection, rel: &str) -> i64 {
    c.query_row(
        "SELECT f.id FROM recog.faces f JOIN files fi ON fi.quick_hash = f.key WHERE fi.path_nfc = ?1",
        [rel],
        |r| r.get(0),
    )
    .unwrap()
}

/// `recog.clusters` as rows, in order.
fn cache_rows(c: &Connection) -> Vec<(i64, i64, Option<i64>, Option<f64>)> {
    c.prepare("SELECT face, cluster, person, similarity FROM recog.clusters ORDER BY face")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// A run after a change compares only what changed, and its clusters and
/// suggestions are exactly those of a run that searches everything again.
fn same_as_full(c: &Connection, what: &str) -> clusters::Summary {
    let quick = clusters::run(c, &|| false, &mut |_, _| {}).unwrap();
    let rows = cache_rows(c);
    c.execute("DELETE FROM recog.suggest_runs", []).unwrap();
    let full = clusters::run(c, &|| false, &mut |_, _| {}).unwrap();
    assert_eq!(full.searched, full.unnamed, "{what}: without a last run every face is searched");
    assert_eq!(rows, cache_rows(c), "{what}: the run that compared only what changed differs from a full one");
    quick
}

/// Each kind of change: names (new references), a rejection and taking it
/// back, an undo, a merge, a deleted person, a face drawn by hand removed.
#[test]
fn suggestions_compare_only_what_changed_and_match_a_full_run() {
    let lib = empty("people-incremental");
    for (p, rgb) in [ANNA, BEN, CARL].iter().enumerate() {
        for n in 1..=3 {
            plain(&lib, &format!("p{p}/plain{n}.png"), *rgb, n);
        }
        variant(&lib, &format!("p{p}/suggested.png"), *rgb, SUGGESTED);
        variant(&lib, &format!("p{p}/maybe.png"), *rgb, MAYBE);
        variant(&lib, &format!("p{p}/far.png"), *rgb, FAR);
    }
    lib.scan();
    let before = lib.snapshot();
    recognize(&lib);
    let c = conn(&lib);
    let assign = |rels: &[&str], name: &str| {
        let faces: Vec<i64> = rels.iter().map(|r| face_of(&c, r)).collect();
        let who = people::Who { person_id: None, name: Some(name.into()) };
        people::decide(&c, &faces, &people::Action::Assign(who)).unwrap().person.unwrap().id
    };

    // Nobody named: nothing to compare with.
    let s = same_as_full(&c, "nobody named");
    assert_eq!(s.suggested + s.maybe, 0);

    // Anna named: her look-alikes get suggestions, compared with her new
    // faces only (no face is searched among all references).
    let anna = assign(&["p0/plain1.png", "p0/plain2.png"], "Anna");
    let s = same_as_full(&c, "Anna named");
    assert_eq!((s.searched, s.compared), (0, s.unnamed), "{s:?}");
    assert!(s.suggested >= 1, "{s:?}");

    // Ben named, and one more face of Anna.
    let ben = assign(&["p1/plain1.png", "p1/plain2.png", "p1/plain3.png"], "Ben");
    assign(&["p0/plain3.png"], "Anna");
    let s = same_as_full(&c, "Ben named");
    assert_eq!(s.searched, 0, "{s:?}");

    // A rejection: only that face is searched again; taken back, the same.
    let suggested = face_of(&c, "p0/suggested.png");
    people::decide(&c, &[suggested], &people::Action::Reject(Some(anna))).unwrap();
    let s = same_as_full(&c, "a rejection");
    assert_eq!(s.searched, 1, "{s:?}");
    people::decide(&c, &[suggested], &people::Action::Unreject(anna)).unwrap();
    let s = same_as_full(&c, "a rejection taken back");
    assert_eq!(s.searched, 1, "{s:?}");

    // An undo: the face is open again (searched), Anna lost a reference
    // (her suggested faces searched again).
    people::decide(&c, &[face_of(&c, "p0/plain3.png")], &people::Action::Undo).unwrap();
    let s = same_as_full(&c, "an undo");
    assert!(s.searched >= 2 && s.searched < s.unnamed, "{s:?}");

    // Ben merged into Anna, then Carl named and deleted again.
    people::merge_people(&c, ben, anna).unwrap();
    same_as_full(&c, "a merge");
    let carl = assign(&["p2/plain1.png"], "Carl");
    same_as_full(&c, "Carl named");
    people::decide(&c, &[face_of(&c, "p2/plain1.png")], &people::Action::Undo).unwrap();
    people::delete_person(&c, carl).unwrap();
    same_as_full(&c, "Carl deleted");

    // A new model: everything is searched.
    c.execute("UPDATE recog.suggest_runs SET model = 'old'", []).unwrap();
    let s = clusters::run(&c, &|| false, &mut |_, _| {}).unwrap();
    assert_eq!(s.searched, s.unnamed);
    assert_eq!(lib.snapshot(), before, "clustering changed an original");
}

/// A `recognize` run in another process groups its new faces at its end:
/// the server notes them and does not group again. One that stopped before
/// grouping is grouped by the server.
#[test]
fn faces_grouped_by_recognize_are_not_grouped_again() {
    let (lib, server) = cards_library("regroup-recognize", 2, rules(10, 60_000, 200));
    let addr = server.addr;
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox")).args(args).arg(&lib.root).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    let before = regroupings(&lib);
    plain(&lib, "p0/3.png", PEOPLE[0], 3);
    run(&["scan"]);
    run(&["recognize", "--recognizer", FAKE]);
    assert_eq!(regroupings(&lib), before + 1, "recognize groups at its end");
    let c = next_regrouping(addr);
    assert_eq!((&c["pending"], &c["stale"]), (&json!(false), &json!(false)), "{c}");
    assert!(!regrouped_within(&lib, before + 1, Duration::from_millis(600)), "grouped again");
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 5, "the new face is on the page");

    // A run whose grouping did not happen (as if stopped before it).
    plain(&lib, "p1/3.png", PEOPLE[1], 3);
    run(&["scan"]);
    run(&["recognize", "--recognizer", FAKE]);
    conn(&lib).execute("DELETE FROM recog.jobs WHERE id = (SELECT max(id) FROM recog.jobs WHERE kind = 'clusters')", []).unwrap();
    let before = regroupings(&lib);
    next_regrouping(addr);
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)), "the server groups what was not grouped");
    let c = wait_for_clusters(addr);
    assert_eq!(c["unnamed"], 6, "{c}");
    server.stop().unwrap();
}

/// The cards with a suggestion ran out (the last regrouping left some): the
/// next one starts soon, although cards without one are left.
#[test]
fn suggestions_that_ran_out_regroup_soon() {
    let lib = empty("regroup-ran-out");
    plain(&lib, "a/1.png", ANNA, 1);
    plain(&lib, "a/2.png", ANNA, 2);
    variant(&lib, "a/suggested.png", ANNA, SUGGESTED);
    plain(&lib, "b/1.png", BEN, 1);
    plain(&lib, "b/2.png", BEN, 2);
    lib.scan();
    recognize(&lib);
    let server = start_regrouping(&lib, rules(10, 60_000, 200));
    let addr = server.addr;
    wait_for_clusters(addr);
    let guard = lib.snapshot();
    // The card of Anna's two plain pictures (in whichever order it lists them).
    let a1 = id_of(&lib, "a/1.png");
    let anna_card =
        cards(addr).into_iter().find(|c| c["faces"].as_array().unwrap().iter().any(|f| f["file"] == a1)).expect("Anna's card");
    ok(addr, &format!("/api/clusters/{}/name", anna_card["id"]), &json!({ "name": "Anna", "generation": anna_card["generation"] }));
    ok(addr, "/api/clusters/left", &json!({}));
    wait_for_clusters(addr);
    let suggested = cards(addr).into_iter().find(|c| !c["suggestion"].is_null()).expect("a card that looks like Anna");
    let before = regroupings(&lib);
    ok(addr, &format!("/api/clusters/{}/name", suggested["id"]), &json!({ "name": "Anna", "generation": suggested["generation"] }));
    assert_eq!(cards(addr).len(), 1, "Ben's card is left");
    assert!(regrouped_within(&lib, before, Duration::from_secs(2)), "the suggestions ran out");
    wait_for_clusters(addr);
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), guard, "regrouping changed an original");
}
