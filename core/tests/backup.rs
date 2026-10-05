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
