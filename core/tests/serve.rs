//! Integration tests for thumbnails and `shoebox serve`: the guard over
//! everything the server reads, the PIN login, and what the timeline shows.

mod common;

use std::fs;

use common::*;

#[test]
fn guard_thumbnails_and_serving_leave_originals_untouched() {
    let lib = Library::new("serve-guard");
    let before = lib.snapshot();
    let entries_before: Vec<_> = fs::read_dir(&lib.root).unwrap().map(|e| e.unwrap().file_name()).collect();

    // No thumbnails yet: the server renders them on request.
    lib.scan_opts(false, false, false);
    let server = start(&lib, None);
    let addr = server.addr;
    for path in [
        "/",
        "/app.js",
        "/app.css",
        "/i18n/i18n.js",
        "/i18n/en.json",
        "/i18n/de.json",
        "/api/info",
        "/api/folders",
        "/api/tags",
        "/api/faces",
        "/api/faces/stats",
        "/api/people",
        "/api/people/search?q=",
        "/api/groups",
        "/api/clusters",
    ] {
        assert_eq!(get(addr, path).status, 200, "{path}");
    }
    // No faces before `shoebox recognize` (its own guard tests are in recognize.rs).
    assert_eq!(get(addr, "/api/faces").json()["total"], 0);
    assert_eq!(get(addr, "/api/faces/1/crop").status, 404);
    assert_eq!(get(addr, "/api/faces/1/similar").status, 404);
    assert_eq!(get(addr, "/api/faces/manual/1/crop").status, 404);
    let timeline = get(addr, "/api/timeline").json();
    assert!(!ids(&timeline).is_empty());
    let kinds = timeline["kinds"].as_str().unwrap().as_bytes().to_vec();
    for (i, id) in ids(&timeline).into_iter().enumerate() {
        let thumb = get(addr, &format!("/api/files/{id}/thumb"));
        // Videos need ffmpeg; everything else must render.
        if kinds[i] != b'v' {
            assert_eq!(thumb.status, 200, "thumbnail of {id}");
            assert_eq!(thumb.header("content-type"), Some("image/jpeg"));
            assert_eq!(get(addr, &format!("/api/files/{id}/view")).status, 200, "view of {id}");
        }
        assert_eq!(get(addr, &format!("/api/files/{id}")).status, 200);

        let path = lib.root.join(lib.db().query_row("SELECT path FROM files WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap());
        let part = request(addr, "GET", &format!("/api/files/{id}/original"), &[("Range", "bytes=10-109")], b"");
        assert_eq!(part.status, 206);
        assert_eq!(part.body, fs::read(&path).unwrap()[10..110]);
    }
    // Thumbnails made on request are stored, with their perceptual hash.
    assert!(lib.count("SELECT count(*) FROM files WHERE kind IN ('jpeg', 'png', 'heic') AND phash IS NULL") == 0);
    assert_eq!(get(addr, "/api/files/999999/thumb").status, 404);
    server.stop().unwrap();

    // And the scan's own thumbnail pass.
    lib.scan();
    fs::remove_file(lib.path(".shoebox/thumbs.db")).unwrap();
    lib.scan();

    let after = lib.snapshot();
    assert_eq!(before.len(), after.len(), "files were added or removed outside .shoebox");
    for (path, (stamp, hash)) in &before {
        let (stamp_after, hash_after) = &after[path];
        assert_eq!(stamp, stamp_after, "timestamps or size changed: {}", path.display());
        assert_eq!(hash, hash_after, "content changed: {}", path.display());
    }
    let mut entries_after: Vec<_> = fs::read_dir(&lib.root).unwrap().map(|e| e.unwrap().file_name()).collect();
    entries_after.retain(|e| !entries_before.contains(e));
    assert_eq!(entries_after, [".shoebox"]);
}

fn with_thumbs(lib: &Library) -> rusqlite::Connection {
    let conn = lib.db();
    conn.execute("ATTACH DATABASE ?1 AS thumbs", [lib.path(".shoebox/thumbs.db").to_str().unwrap()]).unwrap();
    conn
}

#[test]
fn thumbnails_follow_content_not_paths() {
    let lib = Library::new("thumbs");
    fs::create_dir_all(lib.path("Gross")).unwrap();
    // Something photo-like: rings and a gradient (a bare gradient has no
    // structure for a perceptual hash to latch on to).
    image::RgbImage::from_fn(1600, 1200, |x, y| {
        let d = ((x as f32 - 1000.0).powi(2) + (y as f32 - 500.0).powi(2)).sqrt();
        image::Rgb([((d / 25.0).sin() * 100.0 + 128.0) as u8, (y / 5) as u8, 90])
    })
    .save(lib.path("Gross/big.jpg"))
    .unwrap();
    let stats = lib.scan();
    assert_eq!(stats.thumbs.failed, 0, "{:?}", stats.thumbs.errors);
    let conn = with_thumbs(&lib);
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    let dims = |path: &str| -> (u32, u32) {
        conn.query_row(
            "SELECT t.width, t.height FROM thumbs.thumbs t JOIN files f ON f.quick_hash = t.key WHERE f.path_nfc = ?1",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    let phash = |p: &str| -> String {
        conn.query_row("SELECT phash FROM files WHERE path_nfc = ?1", [p], |r| r.get(0)).unwrap()
    };

    // Every image has a thumbnail and a perceptual hash.
    assert_eq!(
        count(
            "SELECT count(*) FROM files f LEFT JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE f.kind NOT IN ('raw', 'video') AND (t.jpeg IS NULL OR f.phash IS NULL)"
        ),
        0
    );
    // Large images shrink to the thumbnail size, small ones are not enlarged.
    assert_eq!(dims("Gross/big.jpg"), (shoebox::thumbs::EDGE, shoebox::thumbs::EDGE * 3 / 4));
    assert_eq!(dims("Familie/Weihnachten/DSC_2001.jpg"), (320, 240));
    let thumbs_before = count("SELECT count(*) FROM thumbs.thumbs");

    // A moved file keeps its thumbnail; an identical copy shares it and gets
    // the same perceptual hash.
    fs::rename(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Familie/DSC_2001.jpg")).unwrap();
    fs::create_dir_all(lib.path("Kopie")).unwrap();
    fs::copy(lib.path("Familie/DSC_2001.jpg"), lib.path("Kopie/DSC_2001.jpg")).unwrap();
    let stats = lib.scan();
    assert_eq!((stats.moved, stats.added, stats.thumbs.made), (1, 1, 0));
    assert_eq!(stats.thumbs.phash_from_existing, 1);
    assert_eq!(phash("Familie/DSC_2001.jpg"), phash("Kopie/DSC_2001.jpg"));
    assert_eq!(count("SELECT count(*) FROM thumbs.thumbs"), thumbs_before);

    // New content: a new thumbnail, and the old one is dropped.
    std::thread::sleep(std::time::Duration::from_millis(20));
    lib.jpeg("2020-07 Urlaub Griechenland/IMG_0002.JPG", 99);
    let stats = lib.scan();
    assert_eq!((stats.changed, stats.thumbs.made, stats.thumbs.pruned), (1, 1, 1));
    assert_eq!(count("SELECT count(*) FROM thumbs.thumbs"), thumbs_before);

    // A smaller copy of a photo is close in perceptual hash, another photo is not.
    image::open(lib.path("Gross/big.jpg"))
        .unwrap()
        .resize(800, 800, image::imageops::FilterType::Triangle)
        .save(lib.path("Gross/big-small.jpg"))
        .unwrap();
    lib.scan();
    let distance = |a: &str, b: &str| {
        let hex = |p: &str| shoebox::phash::from_hex(&phash(p)).unwrap();
        shoebox::phash::distance(hex(a), hex(b))
    };
    let (near, far) = (distance("Gross/big.jpg", "Gross/big-small.jpg"), distance("Gross/big.jpg", "Familie/DSC_2001.jpg"));
    assert!(near <= 8 && far > 16, "near {near}, far {far}");
}

#[test]
fn pin_login() {
    let lib = Library::new("pin");
    lib.scan_opts(false, false, false);
    let evil = ("Host", "photos.evil.example");
    let json = ("Content-Type", "application/json");

    // Without --lan / --pin nobody but this machine gets in.
    let server = start(&lib, None);
    assert_eq!(request(server.addr, "GET", "/api/timeline", &[evil], b"").status, 401);
    assert_eq!(request(server.addr, "POST", "/api/login", &[evil, json], br#"{"pin":"1234"}"#).status, 403);
    server.stop().unwrap();

    let server = start(&lib, Some("4711"));
    let addr = server.addr;
    assert_eq!(get(addr, "/api/timeline").status, 200, "localhost needs no PIN");
    // A request that names another host (DNS rebinding, or a LAN device).
    assert_eq!(request(addr, "GET", "/api/timeline", &[evil], b"").status, 401);
    assert_eq!(request(addr, "GET", "/api/files/1/original", &[evil], b"").status, 401);
    assert_eq!(request(addr, "GET", "/", &[evil], b"").status, 200, "the login page itself is public");
    let session = request(addr, "GET", "/api/session", &[evil], b"").json();
    assert_eq!((session["authenticated"].as_bool(), session["pin_enabled"].as_bool()), (Some(false), Some(true)));

    assert_eq!(request(addr, "POST", "/api/login", &[evil, json], br#"{"pin":"0000"}"#).status, 401);
    // Changes need the X-Shoebox header, which other web pages cannot send.
    assert_eq!(bare_request(addr, "POST", "/api/login", &[evil, json], br#"{"pin":"4711"}"#).status, 403);
    let ok = request(addr, "POST", "/api/login", &[evil, json], br#"{"pin":"4711"}"#);
    assert_eq!(ok.status, 200);
    let set_cookie = ok.header("set-cookie").unwrap().to_string();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    let cookie = set_cookie.split(';').next().unwrap();
    assert_eq!(request(addr, "GET", "/api/timeline", &[evil, ("Cookie", cookie)], b"").status, 200);
    let forged = ("Cookie", "shoebox_session=forged");
    assert_eq!(request(addr, "GET", "/api/timeline", &[evil, forged], b"").status, 401);
    server.stop().unwrap();
}

#[test]
fn show_in_finder_uses_the_indexed_path_and_only_for_this_computer() {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    let lib = Library::new("reveal");
    lib.scan_opts(false, false, false);
    let before = lib.snapshot();
    let id = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let other = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let log = opened.clone();
    let opener: shoebox::serve::RevealFn = Arc::new(move |p: &Path| -> anyhow::Result<()> {
        log.lock().unwrap().push(p.to_path_buf());
        Ok(())
    });
    let server = start_with(&lib, Some("4711"), Some(opener));
    let addr = server.addr;
    let json = ("Content-Type", "application/json");
    let evil = ("Host", "photos.evil.example");

    // From this computer: the server says what to call the button, and opens
    // the path from the index, whatever the request says.
    let label = get(addr, "/api/info").json()["reveal"].as_str().map(str::to_string);
    assert_eq!(label.as_deref(), Some(shoebox::reveal::label()));
    let body = br#"{"path": "/etc/passwd"}"#;
    let ok = request(addr, "POST", &format!("/api/files/{id}/reveal?path=/etc/passwd"), &[json], body);
    assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
    let expected = lib.root.canonicalize().unwrap().join("2020-07 Urlaub Griechenland/IMG_0001.JPG");
    assert_eq!(*opened.lock().unwrap(), [expected]);

    // Like every change, it needs the X-Shoebox header.
    assert_eq!(bare_request(addr, "POST", &format!("/api/files/{id}/reveal"), &[json], b"{}").status, 403);
    // Unknown ids, and files that are gone, open nothing.
    assert_eq!(post(addr, "/api/files/999999/reveal", &serde_json::json!({})).status, 404);
    fs::remove_file(lib.root.join("2020-07 Urlaub Griechenland/IMG_0001.JPG")).unwrap();
    assert_eq!(post(addr, &format!("/api/files/{id}/reveal"), &serde_json::json!({})).status, 404);
    assert_eq!(opened.lock().unwrap().len(), 1);

    // A device on the network, logged in with the PIN, gets no button and no
    // window on this computer.
    let login = request(addr, "POST", "/api/login", &[evil, json], br#"{"pin":"4711"}"#);
    let cookie = login.header("set-cookie").unwrap().split(';').next().unwrap().to_string();
    let lan = [evil, ("Cookie", cookie.as_str()), json];
    let info = request(addr, "GET", "/api/info", &lan[..2], b"").json();
    assert!(info["reveal"].is_null(), "no button for other devices");
    assert_eq!(request(addr, "POST", &format!("/api/files/{other}/reveal"), &lan, b"{}").status, 403);
    assert_eq!(opened.lock().unwrap().len(), 1);
    server.stop().unwrap();

    // Nothing was touched (the file removed above aside).
    let after = lib.snapshot();
    for (path, (stamp, hash)) in &after {
        assert_eq!(before[path], (stamp.clone(), hash.clone()), "{}", path.display());
    }
}

#[test]
fn timeline_order_filters_and_search() {
    let lib = Library::new("timeline");
    lib.write("Familie/IMG_0009.CR2", b"raw stand-in");
    lib.scan_opts(false, true, false);
    let server = start(&lib, None);
    let addr = server.addr;
    let timeline = |q: &str| get(addr, &format!("/api/timeline{q}")).json();

    let all = timeline("");
    let all_ids = ids(&all);
    // RAW files are indexed but not shown.
    assert!(!all_ids.contains(&id_of(&lib, "Familie/IMG_0009.CR2")));
    // Newest first.
    let days: Vec<u64> = all["days"].as_array().unwrap().iter().map(|d| d.as_u64().unwrap()).collect();
    assert!(days.windows(2).all(|w| w[0] >= w[1]), "{days:?}");

    // No capture date in an event folder: the folder's month.
    let griechenland = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let pos = all_ids.iter().position(|&i| i == griechenland).unwrap();
    assert_eq!(days[pos], 20200701);
    let details = get(addr, &format!("/api/files/{griechenland}")).json();
    assert_eq!(details["date_source"], "folder");
    assert_eq!(details["tags"][0]["name"], "Urlaub Griechenland");

    // Folder (with subfolders), tag and text filters.
    let folder_id = |path: &str| -> i64 {
        lib.db().query_row("SELECT id FROM folders WHERE path_nfc = ?1", [path], |r| r.get(0)).unwrap()
    };
    let mut in_familie = ids(&timeline(&format!("?folder={}", folder_id("Familie"))));
    in_familie.sort();
    let mut expected = vec![id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg"), id_of(&lib, "Familie/Screenshot.png")];
    expected.sort();
    assert_eq!(in_familie, expected);

    let tag: i64 =
        lib.db().query_row("SELECT id FROM tags WHERE name = 'Urlaub Griechenland'", [], |r| r.get(0)).unwrap();
    let tagged = ids(&timeline(&format!("?tag={tag}")));
    assert!(tagged.contains(&griechenland) && tagged.iter().all(|i| all_ids.contains(i)));

    // Search is case-insensitive and Unicode-normalised (typed decomposed here).
    let found = ids(&timeline(&format!("?q={}", encode("O\u{308}STERREICH"))));
    assert!(found.contains(&id_of(&lib, &format!("{NFC_DIR}/IMG_0100.JPG"))));
    assert!(!found.contains(&griechenland));
    let found = ids(&timeline("?q=weihnachten%20dsc"));
    assert!(found.contains(&id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg")));
    assert!(!found.contains(&id_of(&lib, "Familie/Screenshot.png")));
    assert!(ids(&timeline("?q=nichtvorhanden")).is_empty());

    // Folder tree with counts of everything below.
    let folders = get(addr, "/api/folders").json();
    let count = |path: &str| {
        folders.as_array().unwrap().iter().find(|f| f["path"] == path).map(|f| f["count"].as_u64().unwrap())
    };
    assert_eq!(count("Familie"), Some(2));
    assert_eq!(count("Familie/Weihnachten"), Some(1));

    // With the fixtures: the Live Photo's video is folded into its still.
    if lib.count("SELECT count(*) FROM files WHERE path LIKE 'fixtures/%'") > 0 {
        let still = id_of(&lib, "fixtures/2020-07 Urlaub Griechenland/IMG_0002.HEIC");
        let video = id_of(&lib, "fixtures/2020-07 Urlaub Griechenland/IMG_0002.MOV");
        assert!(all["live"].as_array().unwrap().iter().any(|p| p[0] == still && p[1] == video));
        assert!(!all_ids.contains(&video));
        assert!(all_ids.contains(&id_of(&lib, "fixtures/2020-07 Urlaub Griechenland/VID_0003.mp4")));
    }
    server.stop().unwrap();
}

#[test]
fn library_routes_need_the_library_id() {
    let lib = Library::new("serve-library-id");
    lib.scan_opts(false, false, false);
    let server = start(&lib, None);
    let addr = server.addr;

    let libs = get(addr, "/api/libraries").json();
    assert_eq!(libs.as_array().unwrap().len(), 1);
    let id = libs[0]["id"].as_str().unwrap().to_string();
    assert_eq!(id, shoebox::serve::library_id(libs[0]["name"].as_str().unwrap()));
    assert_eq!(libs[0]["online"], true);

    // The id is part of the path, query strings survive the rewrite.
    assert_eq!(get(addr, &format!("/raw/api/lib/{id}/info")).status, 200);
    assert_eq!(get(addr, &format!("/raw/api/lib/{id}/tags?limit=1")).status, 200);
    // Without an id, or with another one, nothing is served (file ids are per database).
    for path in ["/raw/api/info", "/raw/api/files/1/thumb", "/raw/api/lib/00000000/info", "/raw/api/lib/info"] {
        assert_eq!(get(addr, path).status, 404, "{path}");
    }
    // Changes keep the X-Shoebox rule under the new paths.
    let body = br#"{"ids":[],"folder":1}"#;
    let json = ("Content-Type", "application/json");
    assert_eq!(bare_request(addr, "POST", &format!("/raw/api/lib/{id}/move"), &[json], body).status, 403);
}

/// The drive's library id and its routes, for a server with several libraries.
fn lib_ids(addr: std::net::SocketAddr) -> Vec<(String, String, bool)> {
    get(addr, "/api/libraries")
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["id"].as_str().unwrap().to_string(), l["name"].as_str().unwrap().to_string(), l["online"].as_bool().unwrap()))
        .collect()
}

#[test]
fn several_libraries_share_one_server_and_an_unplugged_drive_is_only_offline() {
    let a = Library::new("multi-a");
    let b = Library::new("multi-b");
    // Same ids in both databases (the same layout), but different photos.
    b.jpeg("Familie/Weihnachten/DSC_2001.jpg", 99);
    a.scan_opts(false, false, false);
    b.scan_opts(false, false, false);
    let before = (a.snapshot(), b.snapshot());
    let missing = std::env::temp_dir().join(format!("shoebox-it-{}-multi-never-plugged", std::process::id()));
    let server = start_many(&[&a, &b], &[&missing]);
    let addr = server.addr;

    let libs = lib_ids(addr);
    assert_eq!(libs.len(), 3);
    assert_eq!(libs.iter().map(|l| l.2).collect::<Vec<_>>(), [true, true, false], "a drive that was never plugged in is offline");
    let (id_a, id_b, id_missing) = (libs[0].0.clone(), libs[1].0.clone(), libs[2].0.clone());
    assert_eq!(libs[0].1, a.root.file_name().unwrap().to_str().unwrap());

    // Each library answers for itself; the same file id means different photos.
    let route = |id: &str, rest: &str| format!("/raw/api/lib/{id}/{rest}");
    let (ta, tb) = (get(addr, &route(&id_a, "timeline")).json(), get(addr, &route(&id_b, "timeline")).json());
    assert_eq!(ids(&ta), ids(&tb), "both databases hand out the same ids");
    let (fa, fb) = (get(addr, &route(&id_a, &format!("files/{}", ids(&ta)[0]))).json(), get(addr, &route(&id_b, &format!("files/{}", ids(&tb)[0]))).json());
    assert_eq!(fa["id"], fb["id"]);
    assert_ne!(get(addr, &route(&id_a, "info")).json()["name"], get(addr, &route(&id_b, "info")).json()["name"]);

    // Offline: 503 with a clear message, the others are not affected.
    let off = get(addr, &route(&id_missing, "timeline"));
    assert_eq!(off.status, 503);
    assert_eq!(off.json()["offline"], true);
    assert_eq!(get(addr, &route(&id_a, "timeline")).status, 200);

    // Unplug a running drive: only it goes offline. (A look at the drive is
    // trusted for two seconds.)
    let gone = b.root.with_extension("unplugged");
    std::fs::rename(&b.root, &gone).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2300));
    let r = get(addr, &route(&id_b, "timeline")); assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body[..r.body.len().min(300)]));
    let libs = lib_ids(addr);
    assert_eq!(libs.iter().map(|l| l.2).collect::<Vec<_>>(), [true, false, false]);
    assert_eq!(get(addr, &route(&id_a, "timeline")).status, 200);
    assert_eq!(get(addr, &route(&id_a, "info")).status, 200);

    // Plug it in again: it opens by itself.
    std::fs::rename(&gone, &b.root).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2300));
    assert_eq!(get(addr, &route(&id_b, "timeline")).status, 200);
    assert_eq!(lib_ids(addr).iter().map(|l| l.2).collect::<Vec<_>>(), [true, true, false]);

    // Changes on one library do not touch the other, and only read what they should.
    server.stop().unwrap();
    assert_eq!((a.snapshot(), b.snapshot()), before);
}
