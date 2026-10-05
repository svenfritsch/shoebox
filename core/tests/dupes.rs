//! Integration tests for the duplicates UI (phase 5d): removing copies with
//! the at-least-one-stays rule, tags carried over to the survivor, and the
//! guard around it.

mod common;

use std::fs;

use common::*;
use serde_json::{Value, json};

fn tags(lib: &Library, id: i64, source: &str) -> Vec<String> {
    lib.db()
        .prepare(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE ft.file_id = ?1 AND ft.source = ?2 ORDER BY t.name",
        )
        .unwrap()
        .query_map(rusqlite::params![id, source], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn rings(path: &std::path::Path, w: u32, h: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(w, h, |x, y| {
        let (cx, cy) = (x as f32 * 1600.0 / w as f32, y as f32 * 1200.0 / h as f32);
        let d = ((cx - 1000.0).powi(2) + (cy - 500.0).powi(2)).sqrt();
        image::Rgb([((d / 25.0).sin() * 100.0 + 128.0) as u8, (cy / 5.0) as u8, 90])
    })
    .save(path)
    .unwrap();
}

fn remove(addr: std::net::SocketAddr, keep: &[i64], remove: &[i64]) -> common::Response {
    post(addr, "/api/duplicates/remove", &json!({ "keep": keep, "remove": remove }))
}

#[test]
fn removing_copies_keeps_one_and_carries_tags_over() {
    let lib = Library::new("dupes-remove");
    let original = "Familie/Weihnachten/DSC_2001.jpg";
    fs::create_dir_all(lib.path("Kochen")).unwrap();
    fs::create_dir_all(lib.path("Familie/Advent")).unwrap();
    fs::copy(lib.path(original), lib.path("Kochen/braten.jpg")).unwrap();
    fs::copy(lib.path(original), lib.path("Familie/Advent/DSC_2001.jpg")).unwrap();
    lib.scan();
    let before = lib.snapshot();
    let orig = id_of(&lib, original);
    let kochen = id_of(&lib, "Kochen/braten.jpg");
    let advent = id_of(&lib, "Familie/Advent/DSC_2001.jpg");
    let other = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");

    let server = start(&lib, None);
    let addr = server.addr;
    post(addr, "/api/tags/add", &json!({ "ids": [kochen], "name": "Lecker" }));

    // Rules: something must stay, something must go, the two are distinct,
    // and only duplicates of a kept file can go.
    for (keep, gone) in [(vec![], vec![kochen]), (vec![orig], vec![]), (vec![orig, kochen], vec![kochen]), (vec![orig], vec![other])] {
        let r = remove(addr, &keep, &gone);
        assert_eq!(r.status, 400, "{keep:?} {gone:?}: {}", String::from_utf8_lossy(&r.body));
    }
    // Nothing happened so far.
    assert!(lib.path("Kochen/braten.jpg").exists() && lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG").exists());
    assert!(tags(&lib, orig, "user").is_empty());

    // Two copies go; the survivor gets their folder tags and own tags as
    // own tags, except "Familie", which it has as a folder tag already.
    let r = remove(addr, &[orig], &[kochen, advent]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let body: Value = r.json();
    assert_eq!(body["trashed"]["files"].as_array().unwrap().len(), 2);
    assert_eq!(tags(&lib, orig, "user"), ["Advent", "Kochen", "Lecker"]);
    assert_eq!(tags(&lib, orig, "folder"), ["Familie", "Weihnachten"]);
    // Own tags can be removed again, folder tags stay.
    post(addr, "/api/tags/remove", &json!({ "ids": [orig], "name": "Kochen" }));
    assert_eq!(tags(&lib, orig, "user"), ["Advent", "Lecker"]);
    assert_eq!(tags(&lib, orig, "folder"), ["Familie", "Weihnachten"]);
    server.stop().unwrap();

    // The guard: the survivor is untouched, the copies only changed place.
    let after = lib.snapshot();
    assert_eq!(before[&lib.path(original)], after[&lib.path(original)]);
    assert!(!lib.path("Kochen/braten.jpg").exists());
    assert_eq!(after.len(), before.len() - 2);
    assert!(lib.verify(false).is_clean());
}

fn set_taken(lib: &Library, id: i64, taken: Option<&str>) {
    lib.db().execute("UPDATE files SET taken = ?2, taken_offset = NULL WHERE id = ?1", rusqlite::params![id, taken]).unwrap();
}

fn taken_of(addr: std::net::SocketAddr, id: i64) -> Value {
    get(addr, &format!("/api/files/{id}")).json()["taken"].clone()
}

#[test]
fn removing_copies_merges_capture_dates_without_touching_files() {
    let lib = Library::new("dupes-dates");
    // Copies of one photo in other sizes (their bytes differ, so each has
    // its own date; exact copies would share one by content).
    let original = "Ringe/gross.jpg";
    rings(&lib.path(original), 1600, 1200);
    for (dir, name, w) in [("A", "a.jpg", 800), ("B", "b.jpg", 720), ("C", "c.jpg", 640)] {
        rings(&lib.path(&format!("{dir}/{name}")), w, w * 3 / 4);
    }
    lib.scan();
    let before = lib.snapshot();
    let orig = id_of(&lib, original);
    let (a, b, c) = (id_of(&lib, "A/a.jpg"), id_of(&lib, "B/b.jpg"), id_of(&lib, "C/c.jpg"));
    let server = start(&lib, None);
    let addr = server.addr;

    // The survivor has no date, a copy has one: it takes it over.
    set_taken(&lib, a, Some("2019-12-24T18:30:00"));
    set_taken(&lib, orig, None);
    assert_eq!(taken_of(addr, orig), Value::Null);
    let r = remove(addr, &[orig], &[a]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["dates_set"], 1);
    assert_eq!(taken_of(addr, orig), "2019-12-24T18:30:00");
    // Kept by a rescan (the file says nothing, the override stays).
    lib.scan();
    assert_eq!(taken_of(addr, orig), "2019-12-24T18:30:00");

    // Dates a few hours apart: the oldest wins, no question asked.
    set_taken(&lib, b, Some("2019-12-24T08:00:00"));
    let r = remove(addr, &[orig], &[b]);
    assert_eq!(r.status, 200);
    assert_eq!(taken_of(addr, orig), "2019-12-24T08:00:00");

    // Dates in different years: a real conflict. Nothing happens until the
    // user chooses.
    set_taken(&lib, c, Some("2015-06-01T10:00:00"));
    let r = remove(addr, &[orig], &[c]);
    assert_eq!(r.status, 200);
    let body = r.json();
    assert_eq!(body["conflicts"][0]["keep"], orig);
    assert_eq!(body["conflicts"][0]["dates"], json!(["2015-06-01T10:00:00", "2019-12-24T08:00:00"]));
    assert!(lib.path("C/c.jpg").exists());
    assert_eq!(taken_of(addr, orig), "2019-12-24T08:00:00");
    // A date that is none of the offered ones is no choice.
    let r = post(addr, "/api/duplicates/remove", &json!({ "keep": [orig], "remove": [c], "dates": { orig.to_string(): "2000-01-01T00:00:00" } }));
    assert!(!r.json()["conflicts"].as_array().unwrap().is_empty());
    let r = post(addr, "/api/duplicates/remove", &json!({ "keep": [orig], "remove": [c], "dates": { orig.to_string(): "2015-06-01T10:00:00" } }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.json()["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(taken_of(addr, orig), "2015-06-01T10:00:00");
    server.stop().unwrap();

    // The data is in the backup; the file itself never changed.
    let data: Value = serde_json::from_slice(&fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["taken_overrides"][0]["taken"], "2015-06-01T10:00:00");
    assert_eq!(data["taken_overrides"][0]["files"][0], original);
    let after = lib.snapshot();
    assert_eq!(before[&lib.path(original)], after[&lib.path(original)]);
    assert!(lib.verify(false).is_clean());
}
