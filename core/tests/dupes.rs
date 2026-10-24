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

/// Like `rings`, with the JPEG quality given: a small picture can weigh more than a big one.
fn rings_quality(path: &std::path::Path, w: u32, h: u32, quality: u8) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let (cx, cy) = (x as f32 * 1600.0 / w as f32, y as f32 * 1200.0 / h as f32);
        let d = ((cx - 1000.0).powi(2) + (cy - 500.0).powi(2)).sqrt();
        image::Rgb([((d / 25.0).sin() * 100.0 + 128.0) as u8, (cy / 5.0) as u8, 90])
    });
    let mut out = fs::File::create(path).unwrap();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&img).unwrap();
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
fn same_size_other_bytes_is_never_suggested() {
    let lib = Library::new("dupes-same-size");
    // A camera JPEG and a developed export of the same shot: same pixels,
    // other bytes, one of them without a capture date, the copy-style name on
    // the export. Both stay open: nothing is suggested for either.
    rings(&lib.path("Familie/IMG_3281.JPG"), 1600, 1200);
    tweaked_copy(&lib.path("Familie/IMG_3281.JPG"), &lib.path("Export/IMG_3281 2.jpg"));
    lib.scan();
    let (a, b) = (id_of(&lib, "Familie/IMG_3281.JPG"), id_of(&lib, "Export/IMG_3281 2.jpg"));
    set_taken(&lib, a, Some("2020-05-13T14:23:00"));
    set_taken(&lib, b, None);
    let server = start(&lib, None);
    let all = groups(server.addr);
    let (fa, fb) = (file_of(&all, "Familie/IMG_3281.JPG"), file_of(&all, "Export/IMG_3281 2.jpg"));
    assert_eq!(group_of(&all, "Familie/IMG_3281.JPG")["kind"], "similar");
    assert_ne!(fa["row"], fb["row"]);
    assert_eq!((fa["keeper"].as_i64(), fb["keeper"].as_i64()), (None, None));
    assert!(fa["pick"] == true && fb["pick"] == true);
    server.stop().unwrap();
}

#[test]
fn the_same_pixels_in_another_format_are_one_photo_with_no_suggestion() {
    let lib = Library::new("dupes-other-format");
    // A picture and its conversion (HEIC and the JPEG made from it, here a
    // JPEG and a PNG): one photo, shown with the resolution group, and
    // nothing is suggested, since which format to keep is the user's call.
    rings(&lib.path("Fotos/IMG_3069.jpg"), 1600, 1200);
    tweaked_copy(&lib.path("Fotos/IMG_3069.jpg"), &lib.path("Fotos/IMG_3069.png"));
    lib.scan();
    let server = start(&lib, None);
    let all = groups(server.addr);
    let (fj, fp) = (file_of(&all, "Fotos/IMG_3069.jpg"), file_of(&all, "Fotos/IMG_3069.png"));
    assert_eq!(group_of(&all, "Fotos/IMG_3069.jpg")["kind"], "resolution");
    assert_eq!(fj["row"], fp["row"]);
    assert_eq!((fj["keeper"].as_i64(), fp["keeper"].as_i64()), (None, None));
    server.stop().unwrap();
}

#[test]
fn removing_one_format_keeps_the_other_with_the_same_name() {
    let lib = Library::new("dupes-same-name");
    // The HEIC and the smaller JPEG of it side by side (with a RAW and an
    // edit file of the same name): only the ticked file goes, never the one
    // that stays or anything else next to it.
    rings(&lib.path("Fotos/IMG_3070.jpg"), 1600, 1200);
    rings(&lib.path("Fotos/IMG_3070.png"), 800, 600);
    fs::write(lib.path("Fotos/IMG_3070.aae"), "edits").unwrap();
    fs::copy(lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG"), lib.path("Fotos/IMG_3070.CR2")).unwrap();
    lib.scan();
    let (big, small) = (id_of(&lib, "Fotos/IMG_3070.jpg"), id_of(&lib, "Fotos/IMG_3070.png"));
    let server = start(&lib, None);
    let addr = server.addr;
    post(addr, "/api/tags/add", &json!({ "ids": [small], "name": "Klein" }));
    assert_eq!(group_of(&groups(addr), "Fotos/IMG_3070.jpg")["kind"], "resolution");

    let r = remove(addr, &[big], &[small]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let body: Value = r.json();
    assert_eq!(body["trashed"]["files"], json!(["Fotos/IMG_3070.png"]));
    for stays in ["IMG_3070.jpg", "IMG_3070.aae", "IMG_3070.CR2"] {
        assert!(lib.path(&format!("Fotos/{stays}")).exists(), "{stays}");
    }
    assert!(!lib.path("Fotos/IMG_3070.png").exists());
    assert_eq!(tags(&lib, big, "user"), ["Klein"]);
    server.stop().unwrap();
}

#[test]
fn a_copy_put_back_from_the_trash_is_a_duplicate_again() {
    let lib = Library::new("dupes-put-back");
    rings(&lib.path("Ringe/gross.jpg"), 1600, 1200);
    rings(&lib.path("Ringe/klein.jpg"), 800, 600);
    fs::create_dir_all(lib.path("Kopien")).unwrap();
    fs::copy(lib.path("Ringe/gross.jpg"), lib.path("Kopien/gross.jpg")).unwrap();
    lib.scan();
    let id = |p: &str| id_of(&lib, p);
    let (gross, klein, kopie) = (id("Ringe/gross.jpg"), id("Ringe/klein.jpg"), id("Kopien/gross.jpg"));
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(group_of(&groups(addr), "Ringe/gross.jpg")["files"].as_array().unwrap().len(), 3);

    let r = remove(addr, &[gross], &[klein, kopie]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let batches: Vec<i64> = r.json()["trashed"]["batches"].as_array().unwrap().iter().map(|b| b.as_i64().unwrap()).collect();
    assert_eq!(batches.len(), 2);
    // (With the full fixtures other groups are there too.)
    let in_group = |all: &[Value]| all.iter().any(|g| g["files"].as_array().unwrap().iter().any(|f| f["path"] == "Ringe/gross.jpg"));
    assert!(!in_group(&groups(addr)));

    // Both come back to the group straight away, the smaller one too
    // (found by its look, not its bytes), without a scan.
    for b in batches {
        let r = post(addr, &format!("/api/trash/{b}/restore"), &json!({}));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }
    let all = groups(addr);
    let paths: std::collections::BTreeSet<&str> =
        group_of(&all, "Ringe/gross.jpg")["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["Kopien/gross.jpg", "Ringe/gross.jpg", "Ringe/klein.jpg"].into());
    assert_eq!(group_of(&all, "Ringe/gross.jpg")["kind"], "resolution");
    server.stop().unwrap();
}

fn trash_entry(addr: std::net::SocketAddr, path: &str) -> Value {
    let list = get(addr, "/api/trash").json();
    list.as_array().unwrap().iter().find(|t| t["path"] == path).unwrap_or_else(|| panic!("{path} not in {list}")).clone()
}

#[test]
fn the_trash_names_the_copy_that_stayed() {
    let lib = Library::new("dupes-trash-kept");
    rings(&lib.path("Ringe/gross.jpg"), 1600, 1200);
    rings(&lib.path("Ringe/klein.jpg"), 800, 600);
    fs::create_dir_all(lib.path("Kopien")).unwrap();
    fs::copy(lib.path("Ringe/gross.jpg"), lib.path("Kopien/gross.jpg")).unwrap();
    lib.scan();
    let id = |p: &str| id_of(&lib, p);
    let (gross, klein, kopie) = (id("Ringe/gross.jpg"), id("Ringe/klein.jpg"), id("Kopien/gross.jpg"));
    let other = id("2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let server = start(&lib, None);
    let addr = server.addr;

    // Removed as duplicates: both name the copy that stayed, the smaller
    // version too (its content differs, the kept one is found by its own).
    assert_eq!(remove(addr, &[gross], &[klein, kopie]).status, 200);
    for path in ["Ringe/klein.jpg", "Kopien/gross.jpg"] {
        let kept = &trash_entry(addr, path)["duplicate_of"];
        assert_eq!((kept["id"].as_i64(), kept["path"].as_str(), kept["kind"].as_str()), (Some(gross), Some("Ringe/gross.jpg"), Some("jpeg")), "{path}");
        assert_eq!(kept["version"].as_str().unwrap().len(), 8);
    }
    // Deleted from the timeline: no duplicate.
    assert_eq!(post(addr, "/api/trash", &json!({ "ids": [other] })).status, 200);
    assert!(trash_entry(addr, "2020-07 Urlaub Griechenland/IMG_0001.JPG").get("duplicate_of").is_none());

    // Put back, then deleted from the timeline: the earlier removal does not count.
    let batch = trash_entry(addr, "Ringe/klein.jpg")["batch"].as_i64().unwrap();
    assert_eq!(post(addr, &format!("/api/trash/{batch}/restore"), &json!({})).status, 200);
    let klein = id("Ringe/klein.jpg");
    assert_eq!(post(addr, "/api/trash", &json!({ "ids": [klein] })).status, 200);
    assert!(trash_entry(addr, "Ringe/klein.jpg").get("duplicate_of").is_none());

    // The copy that stayed goes too: the duplicate says it is gone.
    assert_eq!(post(addr, "/api/trash", &json!({ "ids": [gross] })).status, 200);
    assert_eq!(trash_entry(addr, "Kopien/gross.jpg")["duplicate_of"], json!({ "id": null, "path": null, "kind": null, "version": null }));
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

#[test]
fn a_copy_with_fewer_pixels_but_a_bigger_file_is_flagged() {
    let lib = Library::new("dupes-bigger-file");
    // The same picture twice: the smaller one saved at full quality weighs
    // more than the larger one saved at a low quality.
    rings_quality(&lib.path("Selfies/IMG_1794_1.jpg"), 1600, 1200, 20);
    rings_quality(&lib.path("Selfies/IMG_1794.jpg"), 1520, 1140, 100);
    // And a plain messenger copy: fewer pixels and a smaller file.
    rings_quality(&lib.path("Messenger/IMG-WA0001.jpg"), 400, 300, 20);
    lib.scan();
    let big = lib.path("Selfies/IMG_1794_1.jpg").metadata().unwrap().len();
    let heavy = lib.path("Selfies/IMG_1794.jpg").metadata().unwrap().len();
    assert!(heavy > big, "the fixture needs the smaller picture to be the bigger file ({heavy} vs {big})");
    let server = start(&lib, None);
    let all = groups(server.addr);
    let best = id_of(&lib, "Selfies/IMG_1794_1.jpg");
    let (f, w) = (file_of(&all, "Selfies/IMG_1794.jpg"), file_of(&all, "Messenger/IMG-WA0001.jpg"));
    // Both have fewer pixels than the best; only the heavier one is flagged.
    assert_eq!((f["keeper"].as_i64(), w["keeper"].as_i64()), (Some(best), Some(best)));
    assert_eq!((f["bigger_file"].as_bool(), w["bigger_file"].as_bool()), (Some(true), Some(false)));
    assert_eq!(file_of(&all, "Selfies/IMG_1794_1.jpg")["bigger_file"].as_bool(), Some(false));
    server.stop().unwrap();
}
