//! Forgetting what recognition found (`forget.rs`): the automatic finds of
//! faces or pets, keeping the user's names and decisions (they take hold again
//! when the photos are recognized again), or everything including them. The fake
//! worker finds a face and a pet in every picture. Originals stay untouched.

mod common;

use std::path::PathBuf;

use common::*;
use rusqlite::Connection;
use serde_json::{Value, json};
use shoebox::recognize;

const FAKE: &str = env!("CARGO_BIN_EXE_shoebox-fake-recognizer");
const ANNA: [u8; 3] = [200, 150, 120];
const SPOOKY: [u8; 3] = [200, 60, 40];

fn run_recognition(lib: &Library) {
    let stats = recognize::run(&recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(PathBuf::from(FAKE)),
        limit: None,
        retry_failed: false,
        rotated: false,
        pets: true,
        text: false,
        text_only: false,
        timeouts: Default::default(),
    })
    .unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
}

fn conn(lib: &Library) -> Connection {
    let path = lib.path(".shoebox/library.db");
    let conn = shoebox::db::open_shared(&path).unwrap();
    recognize::attach(&conn, &path).unwrap();
    conn
}

fn count(lib: &Library, sql: &str) -> i64 {
    conn(lib).query_row(sql, [], |r| r.get(0)).unwrap()
}

fn picture(lib: &Library, rel: &str, rgb: [u8; 3], n: u8) {
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut img = image::RgbImage::from_pixel(200, 120, image::Rgb(rgb));
    img.put_pixel(0, 0, image::Rgb([n, n, n]));
    img.save(p).unwrap();
}

fn ok(addr: std::net::SocketAddr, path: &str, body: &Value) -> Value {
    let r = post(addr, path, body);
    assert_eq!(r.status, 200, "{path} {body}: {}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn wait_for_clusters(addr: std::net::SocketAddr) {
    let started = std::time::Instant::now();
    loop {
        let c = get(addr, "/api/info").json()["clusters"].clone();
        if c["stale"] == false && c["running"] == false {
            return;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "clustering did not finish: {c}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn faces_of(addr: std::net::SocketAddr, lib: &Library, rel: &str) -> Vec<Value> {
    get(addr, &format!("/api/files/{}", id_of(lib, rel))).json()["faces"].as_array().cloned().unwrap_or_default()
}

fn person_with_species(faces: &[Value], pet: bool) -> i64 {
    faces.iter().find(|f| f["species"].is_string() == pet).unwrap()["id"].as_i64().unwrap()
}

#[test]
fn forgetting_finds_keeps_the_names_and_forgetting_everything_does_not() {
    let lib = Library::new("forget");
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
    }
    for n in 1..=3 {
        picture(&lib, &format!("Anna/a{n}.png"), ANNA, n);
        picture(&lib, &format!("Pets/s{n}.png"), SPOOKY, n);
    }
    lib.scan_opts(true, false, false);
    run_recognition(&lib);
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);

    // Name a person (Anna) and a pet (Spooky).
    let anna_face = person_with_species(&faces_of(addr, &lib, "Anna/a1.png"), false);
    ok(addr, "/api/faces/assign", &json!({ "faces": [anna_face], "name": "Anna" }));
    let pet_face = person_with_species(&faces_of(addr, &lib, "Pets/s1.png"), true);
    ok(addr, "/api/faces/assign", &json!({ "faces": [pet_face], "name": "Spooky" }));
    wait_for_clusters(addr);

    let counts = get(addr, "/api/recognition/forget").json();
    assert_eq!(counts["faces"]["finds"], 6, "{counts}");
    assert_eq!(counts["pets"]["finds"], 6, "{counts}");
    assert_eq!(counts["faces"]["photos"], 6);
    assert_eq!((counts["faces"]["decisions"].as_u64(), counts["faces"]["people"].as_u64()), (Some(1), Some(1)), "{counts}");
    assert_eq!((counts["pets"]["decisions"].as_u64(), counts["pets"]["people"].as_u64()), (Some(1), Some(1)), "{counts}");

    assert_eq!(post(addr, "/api/recognition/forget", &json!({ "space": "cats" })).status, 400);

    // Forget the pets' automatic finds: the faces stay, so does the pet's name
    // and decision.
    let done = ok(addr, "/api/recognition/forget", &json!({ "space": "pets" }));
    assert_eq!((done["finds"].as_u64(), done["decisions"].as_u64()), (Some(6), Some(0)), "{done}");
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"), 6);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'pets'"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.looked WHERE task = 'faces'"), 6);
    assert_eq!(count(&lib, "SELECT count(*) FROM people WHERE name = 'Spooky'"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM face_decisions WHERE species IS NOT NULL"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.neighbours n JOIN recog.faces f ON f.id = n.face WHERE f.species IS NOT NULL"), 0);
    // The pet Spooky is still on the photo, from the user's confirmed decision: with its
    // box and name, but flagged as not used for matching.
    let on_photo = faces_of(addr, &lib, "Pets/s1.png");
    let kept = on_photo.iter().find(|f| f["kept"] == true).expect("the confirmed pet is shown");
    assert_eq!((kept["person"]["name"].as_str(), kept["state"].as_str()), (Some("Spooky"), Some("confirmed")), "{kept}");
    assert!(on_photo.iter().filter(|f| f["species"].is_string() && f["kept"].is_null()).count() == 0, "no detected pet is left");

    // The photos are recognized again: the pet is back and is still Spooky.
    drop(server);
    run_recognition(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let again = faces_of(addr, &lib, "Pets/s1.png");
    let pet = again.iter().find(|f| f["species"].is_string()).expect("the pet is found again");
    assert_eq!(pet["person"]["name"], "Spooky", "{pet}");

    // Forgetting only the automatic finds of the faces: the photo counts as not looked at any
    // more, but Anna's confirmed face is still listed on it (and not as "not looked at").
    ok(addr, "/api/recognition/forget", &json!({ "space": "faces" }));
    let info = get(addr, &format!("/api/files/{}", id_of(&lib, "Anna/a1.png"))).json();
    let shown = info["faces"].as_array().expect("faces are listed, not 'not looked at'");
    let kept = shown.iter().find(|f| f["kept"] == true).expect("Anna's confirmed face is listed");
    assert_eq!((kept["person"]["name"].as_str(), kept["state"].as_str()), (Some("Anna"), Some("confirmed")), "{info}");
    assert!(shown.iter().all(|f| f["species"].is_string() || f["kept"] == true), "no detected face of a person is left: {info}");
    let anna_photos = ids(&get(addr, "/api/timeline?person=1").json());
    assert_eq!(anna_photos.len(), 1);
    // Recognize again to bring the faces back, then go on with the full reset.
    drop(server);
    run_recognition(&lib);
    let server = start(&lib, None);
    let addr = server.addr;
    wait_for_clusters(addr);
    let back = faces_of(addr, &lib, "Anna/a1.png");
    assert!(back.iter().any(|f| f["kept"].is_null() && f["person"]["name"] == "Anna"), "{back:?}");

    // Forget everything about the faces: Anna is gone, Spooky is not.
    let done = ok(addr, "/api/recognition/forget", &json!({ "space": "faces", "everything": true }));
    assert_eq!((done["finds"].as_u64(), done["decisions"].as_u64(), done["people"].as_u64()), (Some(6), Some(1), Some(1)), "{done}");
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NULL"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM people WHERE name = 'Anna'"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM people WHERE name = 'Spooky'"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM face_decisions"), 1);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces WHERE species IS NOT NULL"), 6);

    // Nothing is left to forget in the faces; the pets' everything takes the rest.
    let counts = get(addr, "/api/recognition/forget").json();
    assert_eq!((counts["faces"]["finds"].as_u64(), counts["faces"]["decisions"].as_u64()), (Some(0), Some(0)), "{counts}");
    ok(addr, "/api/recognition/forget", &json!({ "space": "pets", "everything": true }));
    assert_eq!(count(&lib, "SELECT count(*) FROM people"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM face_decisions"), 0);
    assert_eq!(count(&lib, "SELECT count(*) FROM recog.faces"), 0);

    assert_eq!(lib.snapshot(), before, "nothing here may change an original");
}

#[test]
fn recognition_stats_covers_every_kind() {
    let lib = Library::new("overview");
    lib.scan_opts(true, false, false);
    run_recognition(&lib);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox"))
        .args(["stats", lib.root.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let o: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(o["started"], true);
    assert!(o["faces"]["looked"].as_u64().unwrap() > 0, "{o}");
    assert!(o["pets"]["looked"].as_u64().unwrap() > 0, "{o}");
    assert_eq!(o["text"]["looked"], 0, "{o}");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Faces:") && text.contains("Pets:") && text.contains("Text: not run yet"), "{text}");
}
