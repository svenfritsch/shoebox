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

/// The hash of file `id` with its first `n` bits turned.
fn flip_bits(lib: &Library, id: i64, n: u32) -> String {
    let h: String = lib.db().query_row("SELECT phash FROM files WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    let v = u64::from_str_radix(&h, 16).unwrap() ^ ((1u64 << n) - 1);
    format!("{v:016x}")
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

    // The page gets each copy's tags: folder tags and own tags apart.
    let groups = get(addr, "/api/duplicates").json();
    let copy = groups["groups"][0]["files"].as_array().unwrap().iter().find(|f| f["id"] == kochen).unwrap().clone();
    assert_eq!(copy["tags"], json!([{ "name": "Kochen", "own": false }, { "name": "Lecker", "own": true }]));

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

#[test]
fn own_tags_of_a_lower_quality_copy_go_to_the_better_one() {
    let lib = Library::new("dupes-lower-quality");
    rings(&lib.path("Ringe/gross.jpg"), 1600, 1200);
    rings(&lib.path("Ringe/klein.jpg"), 800, 600);
    lib.scan();
    let (gross, klein) = (id_of(&lib, "Ringe/gross.jpg"), id_of(&lib, "Ringe/klein.jpg"));
    let server = start(&lib, None);
    let addr = server.addr;
    // The user tagged the smaller copy by mistake.
    post(addr, "/api/tags/add", &json!({ "ids": [klein], "name": "Handgetippt" }));
    assert!(tags(&lib, gross, "user").is_empty());

    let r = remove(addr, &[gross], &[klein]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(!lib.path("Ringe/klein.jpg").exists());
    // The tag lives on the survivor, and the trash keeps a copy too.
    assert_eq!(tags(&lib, gross, "user"), ["Handgetippt"]);
    let in_trash: String = lib.db().query_row("SELECT user_tags FROM trash", [], |r| r.get(0)).unwrap();
    assert!(in_trash.contains("Handgetippt"), "{in_trash}");
    server.stop().unwrap();
}

#[test]
fn same_folder_bulk_only_touches_exact_duplicates_in_one_folder() {
    let lib = Library::new("dupes-bulk");
    let original = "Familie/Weihnachten/DSC_2001.jpg";
    // Exact copies: two next to the original, one in another folder.
    fs::copy(lib.path(original), lib.path("Familie/Weihnachten/DSC_2001 (2).jpg")).unwrap();
    fs::copy(lib.path(original), lib.path("Familie/Weihnachten/DSC_2001 (3).jpg")).unwrap();
    fs::create_dir_all(lib.path("Kochen")).unwrap();
    fs::copy(lib.path(original), lib.path("Kochen/braten.jpg")).unwrap();
    // A pair in one folder the user already decided to keep; and a near one.
    fs::copy(lib.path("Ordner mit Leerzeichen/Bild 1.jpeg"), lib.path("Ordner mit Leerzeichen/Bild 1b.jpeg")).unwrap();
    rings(&lib.path("Ringe/gross.jpg"), 1600, 1200);
    rings(&lib.path("Ringe/klein.jpg"), 800, 600);
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let (b1, b2) = (id_of(&lib, "Ordner mit Leerzeichen/Bild 1.jpeg"), id_of(&lib, "Ordner mit Leerzeichen/Bild 1b.jpeg"));
    post(addr, "/api/duplicates/decide", &json!({ "ids": [b1, b2], "decision": "linked" }));
    let c = id_of(&lib, "Familie/Weihnachten/DSC_2001 (2).jpg");
    post(addr, "/api/tags/add", &json!({ "ids": [c], "name": "Lecker" }));

    // The preview counts the two extra copies and nothing else.
    let preview = get(addr, "/api/duplicates/same-folder").json();
    assert_eq!((preview["groups"].as_i64(), preview["copies"].as_i64()), (Some(1), Some(2)));

    let r = post(addr, "/api/duplicates/same-folder", &json!({}));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let body = r.json();
    assert_eq!((body["groups"].as_i64(), body["removed"].as_i64()), (Some(1), Some(2)));
    // The one with the original name stays (not "(2)" or "(3)"), the other
    // folder, the decided pair and the near pair are as they were.
    assert!(lib.path("Familie/Weihnachten/DSC_2001.jpg").exists());
    let left: Vec<bool> = ["DSC_2001.jpg", "DSC_2001 (2).jpg", "DSC_2001 (3).jpg"]
        .iter()
        .map(|n| lib.path(&format!("Familie/Weihnachten/{n}")).exists())
        .collect();
    assert_eq!(left, [true, false, false]);
    for p in ["Kochen/braten.jpg", "Ordner mit Leerzeichen/Bild 1.jpeg", "Ordner mit Leerzeichen/Bild 1b.jpeg", "Ringe/gross.jpg", "Ringe/klein.jpg"] {
        assert!(lib.path(p).exists(), "{p}");
    }
    // The "Lecker" of a deleted copy lives on.
    let stays = ["DSC_2001.jpg", "DSC_2001 (2).jpg", "DSC_2001 (3).jpg"]
        .iter()
        .find(|n| lib.path(&format!("Familie/Weihnachten/{n}")).exists())
        .unwrap();
    let stays = id_of(&lib, &format!("Familie/Weihnachten/{stays}"));
    assert_eq!(tags(&lib, stays, "user"), ["Lecker"]);
    // Nothing left to do; a second run changes nothing.
    assert_eq!(get(addr, "/api/duplicates/same-folder").json()["copies"], 0);
    server.stop().unwrap();
    let after = lib.snapshot();
    assert_eq!(after.len(), before.len() - 2);
    for (p, v) in &after {
        assert_eq!(&before[p], v, "{}", p.display());
    }
    assert!(lib.verify(false).is_clean());
}

#[test]
fn lower_quality_versions_of_the_same_photo_go_without_review() {
    let lib = Library::new("dupes-quality");
    rings(&lib.path("Fotos/IMG_1.jpg"), 1600, 1200);
    rings(&lib.path("WhatsApp/IMG-WA0001.jpg"), 800, 600); // sent via a messenger: smaller, no metadata
    rings(&lib.path("Fotos/IMG_2.jpg"), 800, 600); // another shot: its capture time differs
    rings(&lib.path("Fotos/IMG_3.jpg"), 1280, 720); // another shape
    lib.scan();
    let before = lib.snapshot();
    let id = |p: &str| id_of(&lib, p);
    let (orig, wa, other, wide) = (id("Fotos/IMG_1.jpg"), id("WhatsApp/IMG-WA0001.jpg"), id("Fotos/IMG_2.jpg"), id("Fotos/IMG_3.jpg"));
    // The same picture, recompressed so hard that its hash drifts (6 bits).
    lib.db().execute("UPDATE files SET phash = ?2 WHERE id = ?1", rusqlite::params![wa, flip_bits(&lib, orig, 6)]).unwrap();
    set_taken(&lib, orig, Some("2021-05-01T12:00:00"));
    set_taken(&lib, other, Some("2021-05-03T12:00:00"));
    set_taken(&lib, wide, Some("2021-05-01T12:00:00"));
    let server = start(&lib, None);
    let addr = server.addr;

    // The page: the messenger copy shares a row with the original and is
    // marked as the worse one; the others are rows of their own.
    let groups = get(addr, "/api/duplicates").json();
    let files: Vec<Value> = groups["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["files"].as_array().unwrap().clone())
        .filter(|f| [orig, wa, other, wide].contains(&f["id"].as_i64().unwrap()))
        .collect();
    let by = |i: i64| files.iter().find(|f| f["id"] == i).unwrap().clone();
    assert_eq!(by(wa)["keeper"], orig);
    assert_eq!(by(wa)["row"], by(orig)["row"]);
    for i in [orig, other, wide] {
        assert_eq!(by(i)["keeper"], Value::Null, "{i}");
    }
    assert_ne!(by(other)["row"], by(orig)["row"]);
    assert_ne!(by(wide)["row"], by(orig)["row"]);

    let preview = get(addr, "/api/duplicates/lower-quality").json();
    assert_eq!((preview["groups"].as_i64(), preview["copies"].as_i64()), (Some(1), Some(1)));
    let r = post(addr, "/api/duplicates/lower-quality", &json!({}));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["removed"], 1);
    assert!(!lib.path("WhatsApp/IMG-WA0001.jpg").exists());
    for p in ["Fotos/IMG_1.jpg", "Fotos/IMG_2.jpg", "Fotos/IMG_3.jpg"] {
        assert!(lib.path(p).exists(), "{p}");
    }
    // Nothing is lost: the folder of the removed copy is a tag of the original.
    assert_eq!(tags(&lib, orig, "user"), ["WhatsApp"]);
    assert_eq!(get(addr, "/api/duplicates/lower-quality").json()["copies"], 0);
    server.stop().unwrap();
    let after = lib.snapshot();
    assert_eq!(after.len(), before.len() - 1);
    for (p, v) in &after {
        assert_eq!(&before[p], v, "{}", p.display());
    }
    assert!(lib.verify(false).is_clean());
}

fn groups(addr: std::net::SocketAddr) -> Vec<Value> {
    get(addr, "/api/duplicates").json()["groups"].as_array().unwrap().clone()
}

fn file_of<'a>(groups: &'a [Value], path: &str) -> &'a Value {
    groups.iter().flat_map(|g| g["files"].as_array().unwrap()).find(|f| f["path"] == path).unwrap()
}

fn group_of<'a>(groups: &'a [Value], path: &str) -> &'a Value {
    groups.iter().find(|g| g["files"].as_array().unwrap().iter().any(|f| f["path"] == path)).unwrap()
}

#[test]
fn identical_copies_keep_the_file_with_the_original_name() {
    let lib = Library::new("dupes-names");
    let original = "Familie/Weihnachten/DSC_2001.jpg";
    fs::create_dir_all(lib.path("Kochen")).unwrap();
    fs::copy(lib.path(original), lib.path("Kochen/DSC_2001 - Copy.jpg")).unwrap();
    fs::copy(lib.path(original), lib.path("Kochen/DSC_2001 (2).jpg")).unwrap();
    lib.scan();
    let server = start(&lib, None);
    let all = groups(server.addr);
    let g = group_of(&all, original);
    assert_eq!(g["kind"], "identical");
    assert_eq!(g["files"].as_array().unwrap().len(), 3);
    // One row; only the original name is the pick, whatever the scan order.
    let rows: std::collections::HashSet<i64> = g["files"].as_array().unwrap().iter().map(|f| f["row"].as_i64().unwrap()).collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(file_of(&all, original)["pick"], true);
    assert_eq!(file_of(&all, "Kochen/DSC_2001 - Copy.jpg")["pick"], false);
    assert_eq!(file_of(&all, "Kochen/DSC_2001 (2).jpg")["pick"], false);
    server.stop().unwrap();
}

#[test]
fn groups_are_identical_same_photo_in_another_size_or_similar_shots() {
    let lib = Library::new("dupes-kinds");
    // A series: two shots a few seconds apart (the second is one pixel wider,
    // so the bytes differ), and a smaller copy that came back from a messenger.
    rings(&lib.path("Serie/A.jpg"), 1600, 1200);
    rings(&lib.path("Serie/B.jpg"), 1601, 1200);
    rings(&lib.path("Messenger/IMG-WA0001.jpg"), 800, 600);
    lib.scan();
    let id = |p: &str| id_of(&lib, p);
    let (a, b, wa) = (id("Serie/A.jpg"), id("Serie/B.jpg"), id("Messenger/IMG-WA0001.jpg"));
    set_taken(&lib, a, Some("2024-06-02T11:20:04"));
    set_taken(&lib, b, Some("2024-06-02T11:20:09"));
    let server = start(&lib, None);
    let all = groups(server.addr);
    // Different shots: "similar", a row each; the messenger copy goes with
    // the better shot it is the same photo as, and is the one to drop.
    assert_eq!(group_of(&all, "Serie/A.jpg")["kind"], "similar");
    let (fa, fb, fw) = (file_of(&all, "Serie/A.jpg"), file_of(&all, "Serie/B.jpg"), file_of(&all, "Messenger/IMG-WA0001.jpg"));
    assert_ne!(fa["row"], fb["row"]);
    assert_eq!(fw["row"], fb["row"]);
    assert_eq!((fa["pick"].as_bool(), fb["pick"].as_bool(), fw["pick"].as_bool()), (Some(true), Some(true), Some(false)));
    assert_eq!(fw["keeper"], b);
    server.stop().unwrap();

    // Without the second shot the same two files are one photo in two sizes.
    fs::remove_file(lib.path("Serie/B.jpg")).unwrap();
    lib.scan_with(true, true);
    let server = start(&lib, None);
    let all = groups(server.addr);
    assert_eq!(group_of(&all, "Serie/A.jpg")["kind"], "resolution");
    assert_eq!(file_of(&all, "Messenger/IMG-WA0001.jpg")["keeper"], a);
    server.stop().unwrap();
}

/// A copy of `from` with one pixel changed: other bytes, same picture and size.
fn tweaked_copy(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    let mut img = image::open(from).unwrap().to_rgb8();
    img.put_pixel(3, 3, image::Rgb([1, 2, 3]));
    img.save(to).unwrap();
}

#[test]
fn shots_of_a_series_are_not_one_photo_in_two_versions() {
    let lib = Library::new("dupes-series");
    rings(&lib.path("2010er/IMG_4284 1.JPG"), 1600, 1200);
    tweaked_copy(&lib.path("2010er/IMG_4284 1.JPG"), &lib.path("2010er/IMG_4285 1.JPG"));
    // Without capture dates, other names: the same picture saved twice.
    tweaked_copy(&lib.path("2010er/IMG_4284 1.JPG"), &lib.path("Export/foto.jpg"));
    lib.scan();
    let (a, b) = (id_of(&lib, "2010er/IMG_4284 1.JPG"), id_of(&lib, "2010er/IMG_4285 1.JPG"));
    // Same capture second, same size: a burst.
    set_taken(&lib, a, Some("2015-12-21T15:20:00"));
    set_taken(&lib, b, Some("2015-12-21T15:20:00"));
    let server = start(&lib, None);
    let all = groups(server.addr);
    let (fa, fb) = (file_of(&all, "2010er/IMG_4284 1.JPG"), file_of(&all, "2010er/IMG_4285 1.JPG"));
    assert_ne!(fa["row"], fb["row"]);
    assert_eq!((fa["keeper"].as_i64(), fb["keeper"].as_i64()), (None, None));
    assert!(fa["pick"] == true && fb["pick"] == true);
    server.stop().unwrap();

    // Without any capture date the numbers in the names still tell the
    // shots apart; the " 1" import suffix is ignored.
    set_taken(&lib, a, None);
    set_taken(&lib, b, None);
    let server = start(&lib, None);
    let all = groups(server.addr);
    assert_ne!(file_of(&all, "2010er/IMG_4284 1.JPG")["row"], file_of(&all, "2010er/IMG_4285 1.JPG")["row"]);
    server.stop().unwrap();
}

#[test]
fn files_in_a_copies_folder_are_never_the_original() {
    let lib = Library::new("dupes-copies-folder");
    // A packaged InDesign project: its "Link" folder holds copies. The Link
    // folder sorts (and so is scanned) first.
    lib.jpeg("Album/Projekt/Link/IMG_0508.JPG", 7);
    fs::create_dir_all(lib.path("Fotos")).unwrap();
    fs::copy(lib.path("Album/Projekt/Link/IMG_0508.JPG"), lib.path("Fotos/IMG_0508.JPG")).unwrap();
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    let pick = |all: &[Value], path: &str| file_of(all, path)["pick"].as_bool().unwrap();

    // Nothing named yet: a tie, the earlier record is the pick.
    assert_eq!(get(addr, "/api/duplicates/copy-folders").json()["folders"], json!([]));
    let all = groups(addr);
    assert_ne!(pick(&all, "Album/Projekt/Link/IMG_0508.JPG"), pick(&all, "Fotos/IMG_0508.JPG"));

    // Named (case matters, any level of the path): "link" is not "Link".
    post(addr, "/api/duplicates/copy-folders", &json!({ "folders": ["link"] }));
    let all = groups(addr);
    assert_eq!(file_of(&all, "Album/Projekt/Link/IMG_0508.JPG")["in_copies"], false);
    // Named: the one outside is the pick.
    assert_eq!(post(addr, "/api/duplicates/copy-folders", &json!({ "folders": ["Link", "Pfad/x"] })).status, 400);
    let r = post(addr, "/api/duplicates/copy-folders", &json!({ "folders": [" Link ", "link", "Link"] }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["folders"], json!(["Link", "link"]));
    let all = groups(addr);
    assert!(pick(&all, "Fotos/IMG_0508.JPG") && !pick(&all, "Album/Projekt/Link/IMG_0508.JPG"));
    assert_eq!(file_of(&all, "Album/Projekt/Link/IMG_0508.JPG")["in_copies"], true);
    assert_eq!(file_of(&all, "Fotos/IMG_0508.JPG")["in_copies"], false);
    server.stop().unwrap();
}
