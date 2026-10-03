//! Integration tests for phase 3, through the web API like the UI uses it:
//! moving photos with their companions, renaming folders, the trash,
//! importing, duplicates, and finding files moved behind shoebox's back.
//!
//! The guard here: every original keeps its content, size and timestamps
//! (only its path changes), nothing is ever replaced, and afterwards a scan
//! and `verify` find the index already in line with the drive.

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use shoebox::fingerprint::{self, Stamp};

/// Every file outside `.shoebox` by content: (hash, size, mtime, created).
fn contents(lib: &Library) -> Vec<(String, Stamp)> {
    let mut v: Vec<(String, Stamp)> = lib.snapshot().into_values().map(|(stamp, hash)| (hash, stamp)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.mtime_ns.cmp(&b.1.mtime_ns)));
    v
}

/// Files in a folder (names only), sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| e.unwrap().file_name().into_string().unwrap()).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

fn tags_of(lib: &Library, id: i64) -> String {
    lib.db()
        .query_row(
            "SELECT coalesce(group_concat(name, ','), '') FROM (SELECT t.name FROM file_tags ft
               JOIN tags t ON t.id = ft.tag_id WHERE ft.file_id = ?1 ORDER BY t.name)",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

fn folder_id(lib: &Library, path: &str) -> i64 {
    lib.db().query_row("SELECT id FROM folders WHERE path_nfc = ?1", [path], |r| r.get(0)).unwrap()
}

fn path_of(lib: &Library, id: i64) -> String {
    lib.db().query_row("SELECT path_nfc FROM files WHERE id = ?1", [id], |r| r.get(0)).unwrap()
}

/// The index is in line with the drive: a scan changes nothing, verify is clean.
fn assert_index_in_line(lib: &Library) {
    let stats = lib.scan();
    assert_eq!((stats.moved, stats.added, stats.changed, stats.missing), (0, 0, 0, 0), "{stats:?}");
    assert!(lib.verify(false).is_clean());
}

#[test]
fn guard_moves_renames_and_trash_keep_every_file_intact() {
    let lib = Library::new("organize-guard");
    // A photo with its RAW and an XMP sidecar, and two names in the way.
    lib.jpeg("Familie/IMG_0009.JPG", 9);
    lib.write("Familie/IMG_0009.CR2", b"raw stand-in");
    lib.write("Familie/IMG_0009.xmp", b"<x:xmpmeta/>");
    lib.jpeg("Konflikt/IMG_0001.JPG", 50);
    lib.jpeg("Konflikt/img_0002.jpg", 51);
    lib.scan();
    let before = contents(&lib);
    let server = start(&lib, None);
    let addr = server.addr;

    let photo = id_of(&lib, "Familie/IMG_0009.JPG");
    let raw = id_of(&lib, "Familie/IMG_0009.CR2");

    // Without the header nothing happens.
    let body = json!({ "ids": [photo], "folder": "Neu" }).to_string();
    let refused = bare_request(addr, "POST", "/api/move", &[("Content-Type", "application/json")], body.as_bytes());
    assert_eq!(refused.status, 403);
    assert!(lib.path("Familie/IMG_0009.JPG").exists());

    // The photo takes its RAW and sidecar along, into a new nested folder.
    let moved = post(addr, "/api/move", &json!({ "ids": [photo], "folder": "2020-08 Neu/Unterordner" }));
    assert_eq!(moved.status, 200, "{}", String::from_utf8_lossy(&moved.body));
    let moved = moved.json();
    assert_eq!(moved["files"].as_array().unwrap().len(), 2, "{moved}");
    assert_eq!(moved["sidecars"], 1);
    assert_eq!(
        listing(&lib.path("2020-08 Neu/Unterordner")),
        ["IMG_0009.CR2", "IMG_0009.JPG", "IMG_0009.xmp"]
    );
    assert!(!lib.path("Familie/IMG_0009.JPG").exists() && !lib.path("Familie/IMG_0009.xmp").exists());
    assert_eq!(path_of(&lib, photo), "2020-08 Neu/Unterordner/IMG_0009.JPG");
    assert_eq!(path_of(&lib, raw), "2020-08 Neu/Unterordner/IMG_0009.CR2");
    assert_eq!(tags_of(&lib, photo), "Neu,Unterordner");

    // Names that are taken, also in another case, stop a move.
    let ids = [id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG"), id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG")];
    let blocked = post(addr, "/api/move", &json!({ "ids": ids, "folder": "Konflikt" })).json();
    assert_eq!(blocked["files"].as_array().unwrap().len(), 0, "{blocked}");
    assert_eq!(blocked["skipped"].as_array().unwrap().len(), 2, "{blocked}");
    assert!(lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG").exists());
    assert_eq!(listing(&lib.path("Konflikt")), ["IMG_0001.JPG", "img_0002.jpg"]);

    // Bad folder names are refused before anything happens.
    for bad in ["../draussen", ".versteckt", "a:b"] {
        let r = post(addr, "/api/move", &json!({ "ids": [photo], "folder": bad }));
        assert_eq!(r.status, 400, "{bad}");
    }

    // Rename a folder with a subfolder, then change only its case.
    let familie = folder_id(&lib, "Familie");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let r = post(addr, &format!("/api/folders/{familie}/rename"), &json!({ "path": "Familie 2" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(path_of(&lib, dsc), "Familie 2/Weihnachten/DSC_2001.jpg");
    let r = post(addr, &format!("/api/folders/{familie}/rename"), &json!({ "path": "familie 2" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(listing(&lib.root).contains(&"familie 2".to_string()));
    assert!(!listing(&lib.root).contains(&"Familie 2".to_string()));
    assert_eq!(path_of(&lib, dsc), "familie 2/Weihnachten/DSC_2001.jpg");
    assert_eq!(tags_of(&lib, dsc), "Weihnachten,familie 2");
    assert_eq!(folder_id(&lib, "familie 2/Weihnachten"), lib.db().query_row("SELECT folder_id FROM files WHERE id = ?1", [dsc], |r| r.get::<_, i64>(0)).unwrap());
    // Not into itself, not onto another folder.
    let into_itself = post(addr, &format!("/api/folders/{familie}/rename"), &json!({ "path": "familie 2/Weihnachten/x" }));
    assert_eq!(into_itself.status, 400);
    let onto_other = post(addr, &format!("/api/folders/{familie}/rename"), &json!({ "path": "KONFLIKT" }));
    assert_eq!(onto_other.status, 400);

    // A folder moved into a new parent folder.
    let konflikt = folder_id(&lib, "Konflikt");
    let r = post(addr, &format!("/api/folders/{konflikt}/rename"), &json!({ "path": "Archiv/Konflikt 2019" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(listing(&lib.path("Archiv/Konflikt 2019")), ["IMG_0001.JPG", "img_0002.jpg"]);
    assert_eq!(tags_of(&lib, id_of(&lib, "Archiv/Konflikt 2019/IMG_0001.JPG")), "Archiv,Konflikt 2019");
    let parent: i64 =
        lib.db().query_row("SELECT parent_id FROM folders WHERE id = ?1", [konflikt], |r| r.get(0)).unwrap();
    assert_eq!(parent, folder_id(&lib, "Archiv"));

    // An event folder renamed to another month: tags and timeline follow.
    let event = folder_id(&lib, "2020-07 Urlaub Griechenland");
    let r = post(addr, &format!("/api/folders/{event}/rename"), &json!({ "path": "2020-06 Urlaub Kreta" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let img = ids[0];
    assert_eq!(tags_of(&lib, img), "Urlaub Kreta");
    let timeline = get(addr, "/api/timeline").json();
    let pos = ids_of(&timeline).iter().position(|&i| i == img).unwrap();
    assert_eq!(timeline["days"][pos], 20200601);

    // Trash and restore.
    let bild = id_of(&lib, "Ordner mit Leerzeichen/Bild 1.jpeg");
    let hash = lib.record("Ordner mit Leerzeichen/Bild 1.jpeg").unwrap().1;
    let trashed = post(addr, "/api/trash", &json!({ "ids": [bild] })).json();
    assert_eq!(trashed["files"], json!(["Ordner mit Leerzeichen/Bild 1.jpeg"]), "{trashed}");
    let batch = trashed["batches"][0].as_i64().unwrap();
    assert!(!lib.path("Ordner mit Leerzeichen/Bild 1.jpeg").exists());
    assert!(lib.path(&format!(".shoebox/trash/{batch}/Bild 1.jpeg")).exists());
    assert_eq!(lib.record("Ordner mit Leerzeichen/Bild 1.jpeg"), None);
    assert!(!ids_of(&get(addr, "/api/timeline").json()).contains(&bild));
    assert_eq!(get(addr, "/api/info").json()["trash"], 1);
    let listed = get(addr, "/api/trash").json();
    assert_eq!(listed[0]["path"], "Ordner mit Leerzeichen/Bild 1.jpeg");
    assert_eq!(get(addr, &format!("/api/trash/{}/thumb", listed[0]["id"])).status, 200);

    let restored = post(addr, &format!("/api/trash/{batch}/restore"), &json!({}));
    assert_eq!(restored.status, 200, "{}", String::from_utf8_lossy(&restored.body));
    let (_, hash_after, missing) = lib.record("Ordner mit Leerzeichen/Bild 1.jpeg").unwrap();
    assert_eq!((hash_after, missing), (hash, false));
    assert_eq!(get(addr, "/api/info").json()["trash"], 0);
    assert_eq!(listing(&lib.path(".shoebox/trash")), Vec::<String>::new());

    // A scan running elsewhere: no changes meanwhile.
    lib.db()
        .execute("INSERT INTO jobs (kind, state, started_at, updated_at) VALUES ('scan', 'running', ?1, ?1)", [shoebox::db::now()])
        .unwrap();
    assert_eq!(post(addr, "/api/move", &json!({ "ids": [photo], "folder": "Neu" })).status, 409);
    lib.db().execute("UPDATE jobs SET state = 'interrupted' WHERE state = 'running'", []).unwrap();
    server.stop().unwrap();

    // Every file is still there, byte for byte, with its timestamps.
    assert_eq!(contents(&lib), before);
    assert_index_in_line(&lib);
}

fn ids_of(timeline: &Value) -> Vec<i64> {
    ids(timeline)
}

fn jpeg_bytes(seed: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(200, 150, |x, y| image::Rgb([(x as u8).wrapping_mul(seed), y as u8, seed]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

fn upload(addr: std::net::SocketAddr, folder: &str, name: &str, modified: Option<i64>, keep: bool, data: &[u8]) -> Response {
    let mut path = format!("/api/import?folder={}&name={}", encode(folder), encode(name));
    if let Some(m) = modified {
        path.push_str(&format!("&modified={m}"));
    }
    if keep {
        path.push_str("&keep=1");
    }
    request(addr, "POST", &path, &[("Content-Type", "application/octet-stream")], data)
}

#[test]
fn import_writes_into_the_event_folder_and_never_replaces() {
    let lib = Library::new("import");
    lib.scan_with(false, false); // no full hashes yet: duplicates are found anyway
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;

    let folder = post(addr, "/api/import/folder", &json!({ "year": 2021, "month": 3, "name": " Ausflug O\u{308}tztal " })).json();
    assert_eq!(folder, json!({ "folder": "2021-03 Ausflug \u{d6}tztal", "exists": false, "id": null }));
    let folder = folder["folder"].as_str().unwrap().to_string();
    assert_eq!(post(addr, "/api/import/folder", &json!({ "year": 2021, "month": 13, "name": "x" })).status, 400);

    // Streamed in, with the browser's modification date.
    let data = jpeg_bytes(7);
    let modified = 1_600_000_000_123i64;
    let r = upload(addr, &folder, "IMG_1.JPG", Some(modified), false, &data);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let r = r.json();
    assert_eq!(r["status"], "imported");
    assert_eq!(r["path"], format!("{folder}/IMG_1.JPG"));
    let path = lib.path(&format!("{folder}/IMG_1.JPG"));
    assert_eq!(fs::read(&path).unwrap(), data);
    assert_eq!(fingerprint::stamp(&path).unwrap().mtime_ns, modified as i128 * 1_000_000);
    let id = r["id"].as_i64().unwrap();
    let (_, full_hash, _) = lib.record(&format!("{folder}/IMG_1.JPG")).unwrap();
    assert_eq!(full_hash.as_deref(), Some(blake3::hash(&data).to_hex().as_str()));
    assert_eq!(tags_of(&lib, id), "Ausflug Ötztal");
    let timeline = get(addr, "/api/timeline").json();
    assert!(ids(&timeline).contains(&id));

    // The same content again (under another name): not imported twice.
    let again = upload(addr, &folder, "Kopie.jpg", None, false, &data).json();
    assert_eq!((again["status"].as_str(), again["duplicate_of"].as_str()), (Some("duplicate"), Some(r["path"].as_str().unwrap())));
    assert!(!lib.path(&format!("{folder}/Kopie.jpg")).exists());
    // Also content that is in the library but not fully hashed yet.
    let existing = fs::read(lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();
    let dup = upload(addr, &folder, "DSC.jpg", None, false, &existing).json();
    assert_eq!(dup["duplicate_of"], "Familie/Weihnachten/DSC_2001.jpg");
    // Unless asked for.
    let kept = upload(addr, &folder, "DSC.jpg", None, true, &existing).json();
    assert_eq!(kept["status"], "imported");

    // A taken name (in any case) gets a number; nothing is replaced.
    let other = jpeg_bytes(8);
    let second = upload(addr, &folder, "img_1.jpg", None, false, &other).json();
    assert_eq!(second["path"], format!("{folder}/img_1 (2).jpg"));
    assert_eq!(fs::read(&path).unwrap(), data);

    // Refused before anything is written.
    for (folder, name) in [(folder.as_str(), "../IMG_2.JPG"), (folder.as_str(), "notes.txt"), ("../draussen", "a.jpg"), (folder.as_str(), ".hidden.jpg")] {
        assert_eq!(upload(addr, folder, name, None, false, &other).status, 400, "{folder}/{name}");
    }
    assert_eq!(upload(addr, &folder, "leer.jpg", None, false, b"").status, 400);
    assert_eq!(listing(&lib.path(".shoebox/incoming")), Vec::<String>::new());
    server.stop().unwrap();

    // Nothing that was there before changed.
    let after = lib.snapshot();
    for (path, value) in &before {
        assert_eq!(after.get(path), Some(value), "{}", path.display());
    }
    assert_eq!(after.len(), before.len() + 3);
    assert_index_in_line(&lib);
}

/// Rings: something with structure for the perceptual hash.
fn rings(path: &Path, w: u32, h: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(w, h, |x, y| {
        let (cx, cy) = (x as f32 * 1600.0 / w as f32, y as f32 * 1200.0 / h as f32);
        let d = ((cx - 1000.0).powi(2) + (cy - 500.0).powi(2)).sqrt();
        image::Rgb([((d / 25.0).sin() * 100.0 + 128.0) as u8, (cy / 5.0) as u8, 90])
    })
    .save(path)
    .unwrap();
}

#[test]
fn duplicates_exact_and_near_until_decided() {
    let lib = Library::new("duplicates");
    fs::create_dir_all(lib.path("Kopie")).unwrap();
    fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Kopie/DSC_2001.jpg")).unwrap();
    rings(&lib.path("Ringe/gross.jpg"), 1600, 1200);
    rings(&lib.path("Ringe/klein.jpg"), 800, 600);
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    // Only the groups of this test's files (the fixtures, when present, share
    // one test pattern and form groups of their own).
    let groups = || -> Vec<Value> {
        let all = get(addr, "/api/duplicates").json()["groups"].as_array().unwrap().clone();
        all.into_iter()
            .filter(|g| g["files"].as_array().unwrap().iter().any(|f| !f["path"].as_str().unwrap().starts_with("fixtures/")))
            .collect()
    };
    let paths = |g: &Value| -> Vec<String> {
        let mut p: Vec<String> = g["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect();
        p.sort();
        p
    };

    let all = groups();
    let exact = all.iter().find(|g| g["exact"] == true).expect("an exact group");
    assert_eq!(paths(exact), ["Familie/Weihnachten/DSC_2001.jpg", "Kopie/DSC_2001.jpg"]);
    assert!(exact["files"].as_array().unwrap().iter().all(|f| f["same"] == 1));
    let near = all.iter().find(|g| g["exact"] == false).expect("a near group");
    assert_eq!(paths(near), ["Ringe/gross.jpg", "Ringe/klein.jpg"]);
    assert_eq!(all.len(), 2, "{all:?}");

    // Versions of one photo: linked, and gone from the list.
    let near_ids: Vec<i64> = near["files"].as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert_eq!(post(addr, "/api/duplicates/decide", &json!({ "ids": near_ids, "decision": "linked" })).status, 200);
    assert_eq!(groups().len(), 1);
    let details = get(addr, &format!("/api/files/{}", near_ids[0])).json();
    assert_eq!(details["linked"][0]["id"], near_ids[1]);
    assert_eq!(post(addr, "/api/duplicates/decide", &json!({ "ids": near_ids, "decision": "maybe" })).status, 400);

    // Forgetting the decision brings it back; one copy to the trash ends it.
    assert_eq!(post(addr, "/api/duplicates/decide", &json!({ "ids": near_ids, "decision": null })).status, 200);
    assert_eq!(groups().len(), 2);
    let copy = id_of(&lib, "Kopie/DSC_2001.jpg");
    assert_eq!(post(addr, "/api/trash", &json!({ "ids": [copy] })).status, 200);
    let left = groups();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["exact"], false);

    // Emptying the trash deletes for good.
    assert_eq!(post(addr, "/api/trash/empty", &json!({})).json()["deleted"], 1);
    assert_eq!(listing(&lib.path(".shoebox/trash")), Vec::<String>::new());
    server.stop().unwrap();
    assert!(!lib.path("Kopie/DSC_2001.jpg").exists());
    assert_index_in_line(&lib);
}

#[test]
fn files_moved_behind_shoeboxs_back_are_found_again() {
    let lib = Library::new("heal");
    lib.scan();
    let id = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(get(addr, &format!("/api/files/{id}/thumb")).status, 200);

    // Moved in the Finder while shoebox runs: the thumbnail still shows, the
    // original is not found and the search starts.
    fs::rename(lib.path("Familie/Weihnachten"), lib.path("Familie/Weihnachten 2012")).unwrap();
    assert_eq!(get(addr, &format!("/api/files/{id}/thumb")).status, 200);
    assert_eq!(get(addr, &format!("/api/files/{id}/original")).status, 404);

    let started = Instant::now();
    while lib.record("Familie/Weihnachten 2012/DSC_2001.jpg").is_none() {
        assert!(started.elapsed() < Duration::from_secs(20), "not found again");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(lib.record("Familie/Weihnachten 2012/DSC_2001.jpg").unwrap().0, id);
    // Wait for the search to finish (it holds the write lock), then serve again.
    let started = Instant::now();
    while get(addr, "/api/info").json()["busy"] == true {
        assert!(started.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(get(addr, &format!("/api/files/{id}/original")).status, 200);
    assert_eq!(get(addr, &format!("/api/files/{id}")).json()["path"], "Familie/Weihnachten 2012/DSC_2001.jpg");
    server.stop().unwrap();
}
