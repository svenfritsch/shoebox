//! Phase 16: a library whose index is kept next to the program is read-only
//! for shoebox: every function that would change an original refuses, the
//! photo app says so and offers none of those buttons, and the root stays as
//! it was. The tests of this file share one program folder (`SHOEBOX_HOME`,
//! set once for the process) and each register their own library in it.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use common::*;
use serde_json::json;
use shoebox::{duplicates, libraries, organize};

/// The program folder of this process, made and put in the environment once.
fn home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join(format!("shoebox-it-{}-readonly-home", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: runs once, inside the OnceLock, before any test of this file reads the variable.
        unsafe { std::env::set_var(libraries::HOME_ENV, &home) };
        home
    })
}

#[test]
fn nothing_that_changes_an_original_works_on_a_library_next_to_the_program() {
    let home = home();
    let lib = Library::new("readonly-registered");
    let other = Library::new("readonly-plain");
    libraries::add(home, "Server", &lib.root).unwrap();
    assert!(libraries::is_read_only(&lib.root));
    assert!(!libraries::is_read_only(&other.root));

    // A scan works (it only reads) and finds the index next to the program.
    lib.scan();
    other.scan();
    // `lib.db()` looks in <root>/.shoebox; the index of this library is elsewhere.
    let conn = rusqlite::Connection::open(shoebox::db::default_path(&lib.root)).unwrap();
    assert!(conn.path().unwrap().contains("libraries"));
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM files WHERE path_nfc LIKE '%.JPG' OR path_nfc LIKE '%.jpg' ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(ids.len() >= 3, "{ids:?}");

    let before = lib.snapshot();
    let root = &lib.root;
    let refused = |what: &str, r: anyhow::Result<()>| {
        let e = r.expect_err(what);
        assert!(format!("{e:#}").contains("read-only"), "{what}: {e:#}");
    };
    refused("move", organize::move_files(&conn, root, &ids[..1], "Neu").map(|_| ()));
    refused("trash", organize::trash_files(&conn, root, &ids[..1]).map(|_| ()));
    refused("trash tidy", organize::trash_files_tidy(&conn, root, &ids[..1]).map(|_| ()));
    refused("rotate", organize::rotate(&conn, root, ids[0], 1).map(|_| ()));
    refused("turn", organize::turn(&conn, root, ids[0], 1).map(|_| ()));
    refused("restore", organize::restore(&conn, root, 1).map(|_| ()));
    refused("empty trash", organize::empty_trash(&conn, root, None).map(|_| ()));
    refused("rename folder", organize::rename_folder(&conn, root, 1, "Anders").map(|_| ()));
    refused("remove copies", duplicates::remove_copies(&conn, root, &ids[..1], &ids[1..2], &HashMap::new()).map(|_| ()));

    // Nothing moved, nothing was made: the files, their times and the folder list.
    let after = lib.snapshot();
    assert_eq!(before.len(), after.len());
    for (path, (stamp, hash)) in &before {
        assert_eq!(&after[path].0, stamp, "changed: {}", path.display());
        assert_eq!(&after[path].1, hash, "changed: {}", path.display());
    }
    assert!(!lib.root.join("Neu").exists());
    assert!(!lib.root.join(".shoebox").exists());

    // The same call on a plain library still works.
    let conn = other.db();
    let id: i64 = conn.query_row("SELECT id FROM files WHERE path_nfc LIKE '%IMG_0002.JPG'", [], |r| r.get(0)).unwrap();
    let moved = organize::move_files(&conn, &other.root, &[id], "Neu").unwrap();
    assert!(moved.skipped.is_empty(), "{:?}", moved.skipped);
    assert!(other.root.join("Neu").join("IMG_0002.JPG").is_file());
}

#[test]
fn the_photo_app_says_a_library_is_read_only_and_every_change_it_could_make_is_refused() {
    let home = home();
    let lib = Library::new("readonly-serve");
    libraries::add(home, "Server serve", &lib.root).unwrap();
    lib.scan();
    let before = lib.snapshot();
    let ids: Vec<i64> = {
        let conn = rusqlite::Connection::open(shoebox::db::default_path(&lib.root)).unwrap();
        conn.prepare("SELECT id FROM files WHERE kind = 'jpeg' ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };

    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(get(addr, "/api/info").json()["read_only"], true);
    let trash = get(addr, "/api/allow-trash").json();
    assert_eq!((&trash["allow"], &trash["read_only"]), (&json!(false), &json!(true)));

    // The trash cannot be switched on, nothing can be moved, trashed or turned.
    assert_eq!(post(addr, "/api/allow-trash", &json!({ "allow": true })).status, 400);
    assert!(!get(addr, "/api/allow-trash").json()["allow"].as_bool().unwrap());
    for (path, body) in [
        ("/api/move", json!({ "ids": [ids[0]], "folder": "Neu" })),
        ("/api/trash", json!({ "ids": [ids[0]] })),
        (&format!("/api/files/{}/rotate", ids[0]), json!({ "turns": 1 })),
    ] {
        let r = post(addr, path, &body);
        let text = String::from_utf8_lossy(&r.body).to_string();
        assert!(r.status == 400 || text.contains("read-only"), "{path}: {} {text}", r.status);
    }

    // Looking works: the timeline and a thumbnail.
    assert_eq!(get(addr, "/api/timeline").status, 200);
    assert_eq!(get(addr, &format!("/api/files/{}/thumb", ids[0])).status, 200);
    let _ = server.stop();

    // Nothing in the root was touched or made, by the server or by the refused calls.
    let after = lib.snapshot();
    assert_eq!(before.len(), after.len());
    for (path, (stamp, hash)) in &before {
        assert_eq!(&after[path].0, stamp, "changed: {}", path.display());
        assert_eq!(&after[path].1, hash, "changed: {}", path.display());
    }
    assert!(!lib.root.join(".shoebox").exists());
    assert!(!lib.root.join("Neu").exists());
}

#[test]
fn a_library_on_a_drive_is_not_read_only_in_the_photo_app() {
    let _ = home();
    let lib = Library::new("readonly-plain-serve");
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(get(addr, "/api/info").json()["read_only"], false);
    assert_eq!(get(addr, "/api/allow-trash").json()["read_only"], false);
    assert_eq!(post(addr, "/api/allow-trash", &json!({ "allow": true })).status, 200);
    let _ = server.stop();
}

/// Every file and folder below `root`, relative, sorted.
fn entries(root: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = walkdir::WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .map(|e| e.unwrap().path().strip_prefix(root).unwrap().display().to_string())
        .collect();
    v.sort();
    v
}

fn wait_for_export(addr: std::net::SocketAddr) -> serde_json::Value {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let s = get(addr, "/api/export/status").json();
        if s["state"] != "running" {
            return s;
        }
        assert!(std::time::Instant::now() < end, "the export does not end");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn exporting_from_a_read_only_library_copies_out_and_never_into_its_folder() {
    let home = home();
    let lib = Library::new("readonly-export");
    // A RAW partner next to a JPEG (the CI fixtures have such pairs, and Live Photo videos): it only
    // comes along when the export asks for partners, which this one does not.
    lib.write("2020-07 Urlaub Griechenland/IMG_0001.CR2", b"not really a raw file, but named like one");
    libraries::add(home, "Server export", &lib.root).unwrap();
    lib.scan();
    // The JPEGs to export, with their paths (the library may hold more, with the fixtures of CI).
    let (ids, paths): (Vec<i64>, Vec<String>) = {
        let conn = rusqlite::Connection::open(shoebox::db::default_path(&lib.root)).unwrap();
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT id, path FROM files WHERE kind = 'jpeg' ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows.into_iter().unzip()
    };
    assert!(ids.len() >= 3, "{ids:?}");
    let before = lib.snapshot();
    let listing = entries(&lib.root);

    let server = start(&lib, None);
    let addr = server.addr;
    let request = |parent: &std::path::Path, name: &str| {
        json!({ "ids": ids, "parent": parent.display().to_string(), "name": name, "options": { "number": true, "raw": false, "live": false, "heic_to_jpeg": false } })
    };

    // Into the library's own folder, its top or a folder below it: refused, by the check and by the start,
    // and nothing is made there.
    for parent in [lib.root.clone(), lib.path("Familie"), lib.path("Familie/Weihnachten")] {
        for route in ["/api/export/check", "/api/export/start"] {
            let r = post(addr, route, &request(&parent, "copies"));
            let said = String::from_utf8_lossy(&r.body).to_string();
            assert_eq!(r.status, 400, "{route} into {}: {said}", parent.display());
            assert!(said.contains("read-only"), "{said}");
        }
    }
    assert_eq!(entries(&lib.root), listing, "something was made in the library's folder");

    // Anywhere else it works: the copies are identical and the library stays as it was.
    let out = std::env::temp_dir().join(format!("shoebox-it-{}-readonly-export-out", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let req = request(&out, "from-server");
    let check = post(addr, "/api/export/check", &req);
    assert_eq!(check.status, 200, "{}", String::from_utf8_lossy(&check.body));
    assert_eq!(check.json()["shoebox"], serde_json::Value::Null, "the destination is not a shoebox drive");
    assert_eq!(post(addr, "/api/export/start", &req).status, 200);
    let status = wait_for_export(addr);
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(status["copied"].as_u64().unwrap() as usize, ids.len(), "{status}");
    let copies: Vec<_> = std::fs::read_dir(out.join("from-server")).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(copies.len(), ids.len(), "{copies:?}");
    assert!(copies.iter().all(|c| !c.to_string_lossy().to_lowercase().ends_with(".cr2")), "the RAW partner came along: {copies:?}");
    for copy in &copies {
        // "001_IMG_0001.JPG" is the copy of an original named IMG_0001.JPG (folders may repeat a name):
        // its bytes are those of one of the originals with that name.
        let name = copy.file_name().unwrap().to_string_lossy().split_once('_').map(|(_, n)| n.to_string()).unwrap();
        let candidates: Vec<Vec<u8>> = paths
            .iter()
            .filter(|p| p.rsplit('/').next() == Some(name.as_str()))
            .map(|p| std::fs::read(lib.root.join(p)).unwrap())
            .collect();
        assert!(!candidates.is_empty(), "no original for {name}");
        assert!(candidates.contains(&std::fs::read(copy).unwrap()), "{name} differs from its original");
    }

    // Exporting again into the same folder (a merge) does not touch the library either.
    assert_eq!(post(addr, "/api/export/start", &req).status, 200);
    assert_eq!(wait_for_export(addr)["state"], "done");
    let _ = server.stop();

    // The guard: nothing in the library's folder changed or was added, not even a .shoebox folder.
    let after = lib.snapshot();
    assert_eq!(before.len(), after.len());
    for (path, (stamp, hash)) in &before {
        assert_eq!(&after[path].0, stamp, "changed: {}", path.display());
        assert_eq!(&after[path].1, hash, "changed: {}", path.display());
    }
    assert_eq!(entries(&lib.root), listing);
    assert!(!lib.root.join(".shoebox").exists());
    let _ = std::fs::remove_dir_all(&out);
}
