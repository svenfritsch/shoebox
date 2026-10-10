//! Copies of files the drive already had, found by the scan that adds them
//! and removed on request (`arrivals.rs`).

mod common;

use std::fs;

use common::*;
use shoebox::arrivals;

fn cleanup(lib: &Library, forever: bool) -> arrivals::Cleanup {
    arrivals::cleanup(&arrivals::CleanupOptions { root: lib.root.clone(), forever }).unwrap()
}

#[test]
fn a_scan_names_new_copies_of_old_files_and_cleanup_removes_only_those() {
    let lib = Library::new("arrivals");
    // Everything in the first scan is new: nothing older to compare with.
    let first = lib.scan();
    assert_eq!(first.duplicates_total, 0);
    assert!(first.duplicates.is_empty());

    // Added by hand: a copy of an old file, a file that is really new, two
    // copies of each other (no older one), and an old file that was moved.
    fs::create_dir_all(lib.path("Neu")).unwrap();
    fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Neu/DSC_kopie.jpg")).unwrap();
    lib.jpeg("Neu/ganz neu.jpg", 50);
    fs::copy(lib.path("Neu/ganz neu.jpg"), lib.path("Neu/ganz neu 2.jpg")).unwrap();
    fs::rename(lib.path("Ordner mit Leerzeichen/Bild 1.jpeg"), lib.path("Neu/Bild 1.jpeg")).unwrap();
    let before = lib.snapshot();

    let second = lib.scan();
    assert_eq!((second.added, second.moved), (3, 1));
    assert_eq!(second.duplicates_total, 1);
    assert_eq!(
        second.duplicates.iter().map(|d| (d.path.as_str(), d.of.as_str())).collect::<Vec<_>>(),
        [("Neu/DSC_kopie.jpg", "Familie/Weihnachten/DSC_2001.jpg")]
    );

    let done = cleanup(&lib, false);
    assert_eq!((done.removed.as_slice(), done.skipped.len()), (&["Neu/DSC_kopie.jpg".to_string()][..], 0));
    // The copy is in the trash; every other file is exactly as it was.
    assert!(!lib.path("Neu/DSC_kopie.jpg").exists());
    assert!(lib.record("Neu/DSC_kopie.jpg").is_none());
    let after = lib.snapshot();
    for (path, value) in &before {
        if path.ends_with("Neu/DSC_kopie.jpg") {
            continue;
        }
        assert_eq!(after.get(path), Some(value), "{}", path.display());
    }
    assert!(lib.record("Familie/Weihnachten/DSC_2001.jpg").is_some());
    assert_eq!(lib.count("SELECT count(*) FROM trash"), 1);
    // A backup check learns that it was removed on purpose.
    assert_eq!(lib.count("SELECT count(*) FROM removed_copies"), 1);

    // Nothing is offered a second time.
    let again = cleanup(&lib, false);
    assert!(again.removed.is_empty() && again.skipped.is_empty());
}

#[test]
fn nothing_is_offered_after_a_quick_scan_or_a_scan_without_copies() {
    let lib = Library::new("arrivals-quick");
    lib.scan();
    fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Familie/DSC_kopie.jpg")).unwrap();
    // No full hashes: the content cannot be compared yet.
    let quick = lib.scan_with(false, false);
    assert_eq!(quick.duplicates_total, 0);
    assert!(cleanup(&lib, false).removed.is_empty());
    assert!(lib.path("Familie/DSC_kopie.jpg").exists());
    // The copy was not new any more at the next scan: the duplicates screen's job.
    let next = lib.scan();
    assert_eq!(next.duplicates_total, 0);
    assert!(cleanup(&lib, false).removed.is_empty());
    assert!(lib.path("Familie/DSC_kopie.jpg").exists());
}

#[test]
fn a_changed_original_is_never_relied_on_and_forever_deletes_the_copy() {
    let lib = Library::new("arrivals-safe");
    lib.scan();
    fs::create_dir_all(lib.path("Neu")).unwrap();
    for name in ["a", "b"] {
        fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path(&format!("Neu/{name}.jpg"))).unwrap();
    }
    assert_eq!(lib.scan().duplicates_total, 2);

    // The older copy changes after the scan: nothing is removed.
    lib.jpeg("Familie/Weihnachten/DSC_2001.jpg", 99);
    let done = cleanup(&lib, false);
    assert!(done.removed.is_empty());
    assert_eq!(done.skipped.len(), 2, "{:?}", done.skipped);
    assert!(lib.path("Neu/a.jpg").exists() && lib.path("Neu/b.jpg").exists());
}

#[test]
fn for_good_leaves_nothing_in_the_trash() {
    let lib = Library::new("arrivals-forever");
    lib.scan();
    fs::create_dir_all(lib.path("Neu")).unwrap();
    fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Neu/kopie.jpg")).unwrap();
    assert_eq!(lib.scan().duplicates_total, 1);
    let done = cleanup(&lib, true);
    assert_eq!(done.removed, ["Neu/kopie.jpg"]);
    assert!(done.forever);
    assert!(!lib.path("Neu/kopie.jpg").exists());
    assert_eq!(lib.count("SELECT count(*) FROM trash"), 0);
    assert!(lib.path("Familie/Weihnachten/DSC_2001.jpg").exists());
}
