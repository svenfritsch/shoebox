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
