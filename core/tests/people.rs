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
use shoebox::{clusters, recognize};

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
        if c["stale"] == false && c["running"] == false {
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

    // Name it: three confirmed faces.
    let body = json!({ "name": "Anna", "generation": list["generation"] });
    let named = ok(addr, &format!("/api/clusters/{}/name", clusters[0]["id"]), &body);
    assert_eq!(named["faces"], 3);
    let anna = named["person"]["id"].as_i64().unwrap();
    assert_eq!(named["person"]["name"], "Anna");
    // At once (before the clusters are recomputed): out of "Unnamed".
    assert_eq!(get(addr, "/api/clusters").json()["unnamed"], 5);
    wait_for_clusters(addr);

    let people = get(addr, "/api/people").json();
    assert_eq!(people.as_array().unwrap().len(), 1);
    let p = &people[0];
    assert_eq!((p["name"].as_str(), p["faces"].as_u64(), p["photos"].as_u64()), (Some("Anna"), Some(3), Some(3)));
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
    let b1 = face_id(addr, &lib, "Ben/b1.png");
    let ben = ok(addr, "/api/faces/assign", &json!({ "faces": [b1], "name": "Ben" }))["person"]["id"].as_i64().unwrap();
    let b2 = face_id(addr, &lib, "Ben/b2.png");
    assert_eq!(ok(addr, "/api/faces/assign", &json!({ "faces": [b2], "name": " ben " }))["person"]["id"], ben);

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
    server.stop().unwrap();
    assert!(lib.verify(false).is_clean());
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

/// Groups (one per person; deleting one leaves its people without), people
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

    // Groups: made by the user, none up front.
    assert_eq!(get(addr, "/api/groups").json(), json!([]));
    let familie = ok(addr, "/api/groups", &json!({ "name": "Familie" }))["id"].as_i64().unwrap();
    let freunde = ok(addr, "/api/groups", &json!({ "name": "Freunde" }))["id"].as_i64().unwrap();
    assert_eq!(post(addr, "/api/groups", &json!({ "name": " FAMILIE" })).status, 400);
    assert_eq!(post(addr, "/api/groups", &json!({ "name": "  " })).status, 400);
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
    let generation = list["generation"].clone();
    let body = json!({ "name": "Dora", "faces": &faces[..2], "generation": generation });
    let dora = ok(addr, &format!("/api/clusters/{cid}/name"), &body)["person"]["id"].as_i64().unwrap();
    ok(addr, &format!("/api/clusters/{cid}/ignore"), &json!({ "faces": [faces[2]], "generation": generation }));
    // A face of another cluster is refused.
    let anna_cluster = list["clusters"][1]["id"].as_i64().unwrap();
    assert_eq!(post(addr, &format!("/api/clusters/{anna_cluster}/name"), &json!({ "name": "X", "faces": [faces[3]] })).status, 400);
    let left = get(addr, &format!("/api/clusters/{cid}/faces")).json();
    assert_eq!(left.as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect::<Vec<_>>(), [faces[3]]);
    wait_for_clusters(addr);
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
    assert_eq!(ok(addr, &format!("/api/groups/{freunde}/delete"), &json!({}))["people"], 1);
    assert!(get(addr, &format!("/api/people/{dora}")).json()["group_id"].is_null());
    let groups = get(addr, "/api/groups").json();
    assert_eq!((groups.as_array().unwrap().len(), groups[0]["people"].as_u64()), (1, Some(1)));

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

    // A box that moved a little still matches; one far off is kept as "no
    // longer found".
    let db = lib.db();
    db.execute("UPDATE face_decisions SET x = x + 0.05, y = y + 0.03 WHERE decision = 'confirmed'", []).unwrap();
    assert_eq!(faces_of(addr, &lib, "Anna/a1.png")[0]["person"]["id"], anna);
    db.execute("UPDATE face_decisions SET x = 0.0, y = 0.0, w = 0.1, h = 0.1 WHERE decision = 'confirmed'", []).unwrap();
    let info = get(addr, &format!("/api/files/{}", id_of(&lib, "Anna/a1.png"))).json();
    assert!(info["faces"][0]["state"].is_null(), "{info}");
    let lost = info["faces_lost"].as_array().unwrap();
    assert_eq!((lost.len(), lost[0]["lost"].as_bool(), lost[0]["person"]["id"].as_i64()), (1, Some(true), Some(anna)));
    let confirmed = person_faces(addr, anna, "confirmed");
    assert_eq!((confirmed.len(), confirmed[0]["lost"].as_bool()), (1, Some(true)));
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
/// `library.db.bak` and `userdata.json` (version 2).
#[test]
fn user_data_backup_has_people_groups_and_decisions() {
    let lib = empty("people-backup");
    plain(&lib, "a/a1.png", ANNA, 1);
    plain(&lib, "a/c1.png", CARL, 1);
    lib.scan();
    recognize(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    let familie = ok(addr, "/api/groups", &json!({ "name": "Familie" }))["id"].clone();
    let a1 = face_id(addr, &lib, "a/a1.png");
    let anna = ok(addr, "/api/faces/assign", &json!({ "faces": [a1], "name": "Anna" }))["person"]["id"].as_i64().unwrap();
    ok(addr, &format!("/api/people/{anna}/group"), &json!({ "group_id": familie }));
    ok(addr, "/api/faces/not-face", &json!({ "faces": [face_id(addr, &lib, "a/c1.png")] }));
    server.stop().unwrap();

    let data: Value = serde_json::from_slice(&std::fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["version"], 2);
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
    assert_eq!(seen.last(), Some(&(3, 3)));
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
