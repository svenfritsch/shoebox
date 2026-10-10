//! Backup verification: a backup drive against the drive it copies, from the
//! two indexes (`shoebox backup`, `/api/all/backups`), and bit rot with
//! `--deep`. Neither drive is written to besides `.shoebox`.

mod common;

use std::fs;

use common::*;
use serde_json::json;
use shoebox::backup;

/// A drive without fixtures, scanned with full hashes.
fn drive(test: &str) -> Library {
    let lib = Library::new(test);
    let _ = fs::remove_dir_all(lib.path("fixtures"));
    lib.scan_opts(true, false, false);
    lib
}

/// `backup` is `original` as of the last backup, then life went on: a photo
/// was added to the original (missing on the backup), one was edited there
/// (different on the backup), one was deleted there (only on the backup).
fn scenario(name: &str) -> (Library, Library) {
    let original = Library::new(&format!("{name}-original"));
    let backup = Library::new(&format!("{name}-backup"));
    for lib in [&original, &backup] {
        let _ = fs::remove_dir_all(lib.path("fixtures"));
    }
    // After the backup:
    original.jpeg("Neu/neu.jpg", 77); // not on the backup yet
    original.jpeg("Familie/Weihnachten/DSC_2001.jpg", 99); // edited: other content at the same path
    fs::remove_file(original.path("Ordner mit Leerzeichen/Bild 1.jpeg")).unwrap(); // gone from the original
    original.scan_opts(true, false, false);
    backup.scan_opts(true, false, false);
    (original, backup)
}

#[test]
fn the_backup_check_finds_what_is_new_different_and_only_on_the_backup() {
    let (original, backup) = scenario("backup-check");
    let before = (original.snapshot(), backup.snapshot());

    let check = backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: false, limit: 50 }).unwrap();
    let r = &check.report;
    assert_eq!(r.missing, 1, "{r:?}");
    assert_eq!(r.missing_files[0].path, "Neu/neu.jpg");
    assert_eq!(r.different, 1);
    assert_eq!(r.different_files[0].path, "Familie/Weihnachten/DSC_2001.jpg");
    assert_eq!(r.extra, 2, "the old DSC_2001 and the deleted Bild 1 are only on the backup");
    assert_eq!(r.unhashed, 0);
    assert!(!r.up_to_date && !check.ok);
    assert!(r.backup_last_scan.is_some() && r.backup_last_new_files.is_some());

    // The backup is brought up to date (copied, scanned): everything is there.
    fs::create_dir_all(backup.path("Neu")).unwrap();
    fs::copy(original.path("Neu/neu.jpg"), backup.path("Neu/neu.jpg")).unwrap();
    fs::copy(original.path("Familie/Weihnachten/DSC_2001.jpg"), backup.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();
    backup.scan_opts(true, false, false);
    let check = backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: false, limit: 50 }).unwrap();
    assert!(check.report.up_to_date && check.ok, "{:?}", check.report);
    assert_eq!(check.report.missing + check.report.different, 0);
    assert_eq!(check.report.extra, 1, "what was deleted from the original stays on the backup: informational");

    // Same files, the copies on the backup were only changed by the copy above.
    let after = (original.snapshot(), backup.snapshot());
    assert_eq!(after.0, before.0, "the original is only read");
    // Not a backup of itself.
    assert!(backup::run(&backup::Options { primary: original.root.clone(), backup: original.root.clone(), deep: false, limit: 5 }).is_err());
}

#[test]
fn deep_check_finds_bit_rot_on_the_backup() {
    let original = drive("backup-deep-original");
    let backup = drive("backup-deep-backup");
    let ok = backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: true, limit: 5 }).unwrap();
    assert!(ok.ok, "{:?}", ok.report);
    assert!(ok.verify.as_ref().unwrap().is_clean());

    // A byte flips on the backup; size and date stay the same.
    let rotten = backup.path("Familie/Weihnachten/DSC_2001.jpg");
    let mtime = fs::metadata(&rotten).unwrap().modified().unwrap();
    let mut bytes = fs::read(&rotten).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&rotten, bytes).unwrap();
    fs::File::options().write(true).open(&rotten).unwrap().set_modified(mtime).unwrap();

    // The index comparison cannot see it (the backup's index still has the old hash)...
    let quick = backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: false, limit: 5 }).unwrap();
    assert!(quick.ok);
    // ...re-reading does.
    let deep = backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: true, limit: 5 }).unwrap();
    assert!(!deep.ok);
    assert_eq!(deep.verify.unwrap().damaged, ["Familie/Weihnachten/DSC_2001.jpg"]);
}

#[test]
fn the_api_lists_every_backup_with_the_drive_it_copies() {
    let (original, backup) = scenario("backup-api");
    // A third drive with photos of its own (little in common with the backup).
    let other = Library::new("backup-api-other");
    let _ = fs::remove_dir_all(other.path("fixtures"));
    for (rel, seed) in [("2020-07 Urlaub Griechenland/IMG_0002.JPG", 150), ("Familie/Weihnachten/DSC_2001.jpg", 151), ("Ordner mit Leerzeichen/Bild 1.jpeg", 152), ("2020-07 Urlaub Griechenland/IMG_0001.JPG", 153)] {
        other.jpeg(rel, seed);
    }
    other.scan_opts(true, false, false);
    let before = (original.snapshot(), backup.snapshot(), other.snapshot());
    let server = start_many(&[&original, &backup, &other], &[]);
    let addr = server.addr;
    let id = |lib: &Library| {
        let name = lib.root.file_name().unwrap().to_str().unwrap().to_string();
        get(addr, "/api/libraries").json().as_array().unwrap().iter().find(|l| l["name"] == name.as_str()).unwrap()["id"].as_str().unwrap().to_string()
    };
    let (i_orig, i_backup, i_other) = (id(&original), id(&backup), id(&other));

    // Nothing is a backup yet: no list.
    assert_eq!(get(addr, "/raw/api/all/backups").json().as_array().unwrap().len(), 0);
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup" })).status, 200);
    let list = get(addr, "/raw/api/all/backups").json();
    let b = &list.as_array().unwrap()[0];
    assert_eq!(b["library"], i_backup.as_str());
    assert_eq!(b["primary"], original.root.file_name().unwrap().to_str().unwrap(), "the drive holding most of it");
    assert_eq!((b["missing"].as_u64(), b["different"].as_u64(), b["extra"].as_u64()), (Some(1), Some(1), Some(2)));
    assert_eq!(b["up_to_date"], false);

    // The user can name the original.
    let named = post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup", "of": i_other }));
    assert_eq!(named.status, 200);
    assert_eq!(get(addr, "/raw/api/all/backups").json()[0]["primary"], other.root.file_name().unwrap().to_str().unwrap());
    // …but not itself or a drive that does not exist.
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup", "of": i_backup })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup", "of": "00000000" })).status, 400);
    // Not a backup any more: the relation goes too.
    post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "separate" }));
    assert_eq!(get(addr, "/raw/api/all/backups").json().as_array().unwrap().len(), 0);
    assert_eq!(backup.count("SELECT count(*) FROM settings WHERE key = 'backup_of'"), 0);
    let _ = i_orig;
    server.stop().unwrap();
    assert_eq!((original.snapshot(), backup.snapshot(), other.snapshot()), before, "only reading");
}

#[test]
fn the_command_prints_json_and_exits_with_2_when_the_backup_is_behind() {
    let (original, backup) = scenario("backup-cli");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox"))
        .args(["backup"])
        .arg(&original.root)
        .arg(&backup.root)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["report"]["missing"], 1);
    assert_eq!(json["ok"], false);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Not on the backup yet"), "the text goes to stderr");
}

// ---------------------------------------------------------------- removed copies

const ORIGINAL_PHOTO: &str = "Familie/Weihnachten/DSC_2001.jpg";
const COPY: &str = "Kochen/braten.jpg";

/// Two drives that both hold a photo and a copy of it; both scanned.
fn with_copy(name: &str) -> (Library, Library) {
    let original = Library::new(&format!("{name}-original"));
    let backup = Library::new(&format!("{name}-backup"));
    for lib in [&original, &backup] {
        let _ = fs::remove_dir_all(lib.path("fixtures"));
        fs::create_dir_all(lib.path("Kochen")).unwrap();
        fs::copy(lib.path(ORIGINAL_PHOTO), lib.path(COPY)).unwrap();
        lib.scan_opts(true, false, false);
    }
    (original, backup)
}

/// What the duplicates screen does: the copy goes, the photo stays.
fn remove_the_copy(lib: &Library) {
    let conn = lib.db();
    let (keep, gone) = (id_of(lib, ORIGINAL_PHOTO), id_of(lib, COPY));
    let r = shoebox::duplicates::remove_copies(&conn, &lib.root, &[keep], &[gone], &Default::default()).unwrap();
    assert_eq!(r.trashed.files, [COPY]);
}

fn check(original: &Library, backup: &Library) -> backup::Check {
    backup::run(&backup::Options { primary: original.root.clone(), backup: backup.root.clone(), deep: false, limit: 50 }).unwrap()
}

fn clean_up(original: &Library, backup: &Library, forever: bool) -> backup::Cleanup {
    backup::cleanup(&backup::CleanupOptions { primary: original.root.clone(), backup: backup.root.clone(), forever }).unwrap()
}

#[test]
fn a_removed_copy_is_remembered_after_the_trash_is_emptied_and_the_check_finds_it_on_the_backup() {
    let (original, backup) = with_copy("removed-found");
    assert_eq!(check(&original, &backup).report.removed, 0, "nothing removed yet");
    remove_the_copy(&original);
    // A photo deleted without the duplicates screen is not remembered: it may be the only copy.
    let unique = id_of(&original, "Ordner mit Leerzeichen/Bild 1.jpeg");
    shoebox::organize::trash_files(&original.db(), &original.root, &[unique]).unwrap();

    // The trash is emptied, and its folder goes with it.
    let emptied = shoebox::organize::empty_trash(&original.db(), &original.root, None).unwrap();
    assert!(emptied >= 2);
    assert!(!shoebox::organize::trash_dir(&original.root).exists(), "no trash folder left on the drive");
    assert_eq!(original.count("SELECT count(*) FROM trash"), 0);
    assert_eq!(original.count("SELECT count(*) FROM removed_copies"), 1, "kept for good, only the copy");

    let before = (original.snapshot(), backup.snapshot());
    let c = check(&original, &backup);
    assert_eq!(c.report.removed, 1, "{:?}", c.report);
    assert_eq!(c.report.removed_files[0].path, COPY);
    assert!(c.ok, "a backup that holds more than the original is still a complete backup");
    assert_eq!((original.snapshot(), backup.snapshot()), before, "the check writes nothing");
}

#[test]
fn the_cleanup_moves_the_removed_copy_into_the_backups_trash_and_nothing_else() {
    let (original, backup) = with_copy("removed-clean");
    remove_the_copy(&original);
    let original_before = original.snapshot();
    let backup_before = backup.snapshot();

    let done = clean_up(&original, &backup, false);
    assert_eq!(done.removed, [COPY]);
    assert!(done.skipped.is_empty());
    assert!(!backup.path(COPY).exists());
    assert!(backup.path(ORIGINAL_PHOTO).exists(), "the copy that stays is still on the backup");
    assert_eq!(backup.count("SELECT count(*) FROM trash"), 1, "recoverable from the backup's trash");
    assert_eq!(original.snapshot(), original_before, "the original is not touched");
    // Of the backup only the removed copy changed place.
    let backup_after = backup.snapshot();
    assert_eq!(backup_after[&backup.path(ORIGINAL_PHOTO)], backup_before[&backup.path(ORIGINAL_PHOTO)]);
    assert_eq!(backup_after.keys().filter(|k| !k.starts_with(backup.path(".shoebox"))).count(), backup_before.len() - 1);
    assert_eq!(check(&original, &backup).report.removed, 0, "nothing left to remove");
    // A second run finds nothing to do.
    assert!(clean_up(&original, &backup, false).removed.is_empty());
}

#[test]
fn the_cleanup_can_delete_for_good() {
    let (original, backup) = with_copy("removed-forever");
    remove_the_copy(&original);
    let done = clean_up(&original, &backup, true);
    assert_eq!(done.removed, [COPY]);
    assert!(done.forever);
    assert_eq!(backup.count("SELECT count(*) FROM trash"), 0);
    let trash = shoebox::organize::trash_dir(&backup.root);
    assert!(!trash.exists() || fs::read_dir(trash).unwrap().next().is_none(), "nothing left in the backup's trash");
    assert!(backup.path(ORIGINAL_PHOTO).exists());
}

#[test]
fn the_last_copy_on_the_backup_is_never_removed() {
    let (original, backup) = with_copy("removed-last");
    remove_the_copy(&original);
    // The backup lost the copy that stays (its index knows by the next scan).
    fs::remove_file(backup.path(ORIGINAL_PHOTO)).unwrap();
    backup.scan_opts(true, false, false);
    let c = check(&original, &backup);
    assert_eq!(c.report.removed, 0, "removing the backup's only copy of that content is never offered");
    assert!(clean_up(&original, &backup, true).removed.is_empty());
    assert!(backup.path(COPY).exists());
}

#[test]
fn a_file_that_changed_on_the_backup_since_its_scan_is_skipped() {
    let (original, backup) = with_copy("removed-changed");
    remove_the_copy(&original);
    fs::write(backup.path(COPY), b"edited later").unwrap();
    let done = clean_up(&original, &backup, false);
    assert!(done.removed.is_empty(), "{done:?}");
    assert_eq!(done.skipped.len(), 1);
    assert_eq!(fs::read(backup.path(COPY)).unwrap(), b"edited later");
}

#[test]
fn the_backup_lists_open_files_in_the_file_manager_and_cards_know_their_facts() {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    let (original, backup) = scenario("backup-reveal");
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let log = opened.clone();
    let opener: shoebox::serve::RevealFn = Arc::new(move |p: &Path| -> anyhow::Result<()> {
        log.lock().unwrap().push(p.to_path_buf());
        Ok(())
    });
    let server = start_many_with(&[&original, &backup], &[], Some(opener));
    let addr = server.addr;
    let id = |lib: &Library| {
        let name = lib.root.file_name().unwrap().to_str().unwrap().to_string();
        get(addr, "/api/libraries").json().as_array().unwrap().iter().find(|l| l["name"] == name.as_str()).unwrap()["id"].as_str().unwrap().to_string()
    };
    let (i_orig, i_backup) = (id(&original), id(&backup));
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup" })).status, 200);

    // The card of every drive: file count, last scan, when new files arrived.
    let drives = get(addr, "/raw/api/all/drives").json();
    for d in drives.as_array().unwrap() {
        assert!(d["files"].as_u64().unwrap() > 0, "{d}");
        assert!(d["last_scan"].is_i64() && d["last_new_files"].is_i64(), "{d}");
        assert!(d.get("volume").is_some() && d.get("folder").is_some(), "{d}");
    }
    // The backup says which drive it copies, for the "not on the backup yet" files.
    let b = get(addr, "/raw/api/all/backups").json()[0].clone();
    assert_eq!(b["primary_library"], i_orig.as_str());

    // A file of the first list is on the original, one of the last list on the backup.
    let reveal = |library: &str, path: &str| post(addr, "/raw/api/all/reveal", &json!({ "library": library, "path": path })).status;
    assert_eq!(reveal(&i_orig, "Neu/neu.jpg"), 200);
    let only_on_backup = b["extra_files"][0]["path"].as_str().unwrap().to_string();
    assert_eq!(reveal(&i_backup, &only_on_backup), 200);
    let opened_now = opened.lock().unwrap().clone();
    assert_eq!(opened_now[0], original.root.canonicalize().unwrap().join("Neu/neu.jpg"));
    assert_eq!(opened_now[1], backup.root.canonicalize().unwrap().join(&only_on_backup));

    // Only what the index of that drive lists: not another drive's file,
    // not a path outside the folder, not a drive that does not exist.
    assert_eq!(reveal(&i_backup, "Neu/neu.jpg"), 404);
    assert_eq!(reveal(&i_orig, "../outside.jpg"), 404);
    assert_eq!(reveal(&i_orig, "/etc/passwd"), 404);
    assert_eq!(reveal("00000000", "Neu/neu.jpg"), 404);
    assert_eq!(opened.lock().unwrap().len(), 2);
    server.stop().unwrap();
}
