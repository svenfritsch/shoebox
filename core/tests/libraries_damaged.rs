//! Phase 16: if the list of libraries (`libraries/libraries.json`) cannot be
//! read, shoebox must stop, not treat a library kept next to the program as an
//! ordinary folder: it would scan it into a `.shoebox` folder of its own inside
//! the folder it promised never to write to, and the read-only rules would be
//! off. One test in this file, so setting `SHOEBOX_HOME` for the process cannot
//! disturb another test.

mod common;

use std::collections::HashMap;
use std::fs;

use common::*;
use shoebox::{export, libraries, organize, scan, serve, verify};

fn entries(root: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = walkdir::WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .map(|e| e.unwrap().path().strip_prefix(root).unwrap().display().to_string())
        .collect();
    v.sort();
    v
}

#[test]
fn a_damaged_list_of_libraries_stops_everything_instead_of_treating_the_folder_as_an_ordinary_one() {
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-damaged-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    // SAFETY: the only test of this file, set before anything reads the variable.
    unsafe { std::env::set_var(libraries::HOME_ENV, &home) };

    let lib = Library::new("damaged-registry");
    libraries::add(&home, "Srv", &lib.root).unwrap();
    lib.scan();
    let index = shoebox::db::default_path(&lib.root);
    assert!(index.starts_with(&home), "{}", index.display());
    let id: i64 = rusqlite::Connection::open(&index).unwrap().query_row("SELECT id FROM files ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    let before = lib.snapshot();
    let listing = entries(&lib.root);

    // The list is damaged (a half-written copy, a bad edit, a wrong version).
    let registry = home.join("libraries/libraries.json");
    let good = fs::read(&registry).unwrap();
    for damage in [&b"{ not json"[..], &br#"{"version":2,"libraries":[]}"#[..]] {
        fs::write(&registry, damage).unwrap();

        let opts = scan::Options { root: lib.root.clone(), db: None, full_hash: true, thumbs: false, forget_missing: false };
        let e = scan::run(&opts).unwrap_err();
        assert!(format!("{e:#}").contains("libraries.json"), "{e:#}");
        assert!(!lib.root.join(".shoebox").exists(), "a .shoebox folder was made in the library's folder");
        assert!(verify::run(&verify::Options { root: lib.root.clone(), db: None, quick: true, limit: None }).is_err());

        // A change to an original is refused too (the index is opened as the page would).
        let conn = rusqlite::Connection::open(&index).unwrap();
        let e = organize::move_files(&conn, &lib.root, &[id], "Neu").unwrap_err();
        assert!(format!("{e:#}").contains("libraries.json"), "{e:#}");
        let e = shoebox::duplicates::remove_copies(&conn, &lib.root, &[id], &[id + 1], &HashMap::new()).unwrap_err();
        assert!(format!("{e:#}").contains("libraries.json"), "{e:#}");

        // An export cannot be pointed into the folder either (nor anywhere: the list decides where it may not go).
        assert!(export::destination(&lib.root.display().to_string(), "copies").is_err());

        // The photo app does not start on it.
        let started = serve::start(
            &serve::Options { root: lib.root.clone(), more_roots: Vec::new(), db: None, port: 0, lan: false, pin: None, reveal: None, recognizer: None, regroup: Default::default(), hold: None },
            false,
        );
        assert!(started.is_err(), "the photo app started on a library whose list is damaged");
        assert!(!lib.root.join(".shoebox").exists());
        assert!(libraries::check_registry().is_err());
    }

    // Nothing in the folder changed or was added.
    assert_eq!(entries(&lib.root), listing);
    let after = lib.snapshot();
    assert_eq!(before.len(), after.len());
    for (path, (stamp, hash)) in &before {
        assert_eq!(&after[path].0, stamp, "changed: {}", path.display());
        assert_eq!(&after[path].1, hash, "changed: {}", path.display());
    }

    // The list put back: everything works again, and the library is still read-only.
    fs::write(&registry, good).unwrap();
    assert!(libraries::check_registry().is_ok());
    lib.scan();
    assert!(!lib.root.join(".shoebox").exists());
    assert!(organize::move_files(&rusqlite::Connection::open(&index).unwrap(), &lib.root, &[id], "Neu").unwrap_err().to_string().contains("read-only"));
    let _ = fs::remove_dir_all(&home);
}
