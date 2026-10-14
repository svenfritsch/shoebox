//! Phase 16: a library whose index is kept next to the program is read-only
//! for shoebox: every function that would change an original refuses, and the
//! root stays as it was. One test in this file, so setting `SHOEBOX_HOME` for
//! the process cannot disturb another test.

mod common;

use std::collections::HashMap;
use std::fs;

use common::*;
use shoebox::{duplicates, libraries, organize};

#[test]
fn nothing_that_changes_an_original_works_on_a_library_next_to_the_program() {
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-readonly-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    // Safe: this file holds one test, so no other thread reads the environment.
    unsafe { std::env::set_var(libraries::HOME_ENV, &home) };

    let lib = Library::new("readonly-registered");
    let other = Library::new("readonly-plain");
    libraries::add(&home, "Server", &lib.root).unwrap();
    assert!(libraries::is_read_only(&lib.root));
    assert!(!libraries::is_read_only(&other.root));

    // A scan works (it only reads) and finds the index next to the program.
    lib.scan();
    other.scan();
    let conn = lib.db();
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

    let _ = fs::remove_dir_all(&home);
}
