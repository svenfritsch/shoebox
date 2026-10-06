//! Integration tests for `shoebox scan` and `shoebox verify`, including the
//! guard: scanning and verifying must leave every original byte-for-byte and
//! timestamp-for-timestamp unchanged.
//!
//! Each test builds a small library in a temp folder. If `SHOEBOX_FIXTURES`
//! points at the output of `scripts/make-fixtures.sh`, those files (HEIC,
//! video, RAW, EXIF dates, decomposed names) are copied in as well.

mod common;

use std::fs;
use std::time::Duration;

use common::*;
use shoebox::fingerprint;

#[test]
fn guard_scan_and_verify_leave_originals_untouched() {
    let lib = Library::new("guard");
    let before = lib.snapshot();
    let entries_before: Vec<_> = fs::read_dir(&lib.root).unwrap().map(|e| e.unwrap().file_name()).collect();

    lib.scan_with(false, false);
    lib.scan();
    lib.scan();
    assert!(lib.verify(false).is_clean());
    assert!(lib.verify(true).is_clean());
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox"))
        .arg("scan")
        .arg(&lib.root)
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));

    let after = lib.snapshot();
    assert_eq!(before.len(), after.len(), "files were added or removed outside .shoebox");
    for (path, (stamp, hash)) in &before {
        let (stamp_after, hash_after) = &after[path];
        assert_eq!(stamp, stamp_after, "timestamps or size changed: {}", path.display());
        assert_eq!(hash, hash_after, "content changed: {}", path.display());
    }
    // The only new entry in the root is our own folder.
    let mut entries_after: Vec<_> = fs::read_dir(&lib.root).unwrap().map(|e| e.unwrap().file_name()).collect();
    entries_after.retain(|e| !entries_before.contains(e));
    assert_eq!(entries_after, [".shoebox"]);
}

#[test]
fn event_folders_in_every_naming_form_are_picked_up_on_the_next_scan() {
    let lib = Library::new("event-forms");
    lib.jpeg("20.07.Foo/a.jpg", 1);
    lib.jpeg("20.07_Foo/b.jpg", 2);
    lib.jpeg("98-08 Urlaub/c.jpg", 3);
    lib.jpeg("2019.05Mai/d.jpg", 4);
    lib.jpeg("2020-07-15 Tag/e.jpg", 5);
    lib.scan();
    // Folders indexed before the forms were known: the next scan reads them again.
    lib.db().execute("UPDATE folders SET event_year = NULL, event_month = NULL, event_name = NULL", []).unwrap();
    lib.scan();
    let event = |name: &str| -> Option<(i64, i64, String)> {
        let row: (Option<i64>, Option<i64>, Option<String>) = lib
            .db()
            .query_row("SELECT event_year, event_month, event_name FROM folders WHERE name = ?1", [name], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        match row {
            (Some(y), Some(m), Some(n)) => Some((y, m, n)),
            _ => None,
        }
    };
    assert_eq!(event("20.07.Foo"), Some((2020, 7, "Foo".into())));
    assert_eq!(event("20.07_Foo"), Some((2020, 7, "Foo".into())));
    assert_eq!(event("98-08 Urlaub"), Some((2098, 8, "Urlaub".into())));
    assert_eq!(event("2019.05Mai"), Some((2019, 5, "Mai".into())));
    assert_eq!(event("2020-07-15 Tag"), None);
}

#[test]
fn indexes_media_and_skips_bookkeeping() {
    let lib = Library::new("index");
    let stats = lib.scan();
    let fixtures = lib.count("SELECT count(*) FROM files WHERE path LIKE 'fixtures/%'");
    assert_eq!(stats.added as i64, 6 + fixtures);
    assert_eq!(stats.hash_pending, 0);
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE name LIKE '._%' OR name = 'notes.txt'"), 0);
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE full_hash IS NULL"), 0);
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE kind = 'jpeg' AND width = 320 AND height = 240"), 5);

    // Event folders and tags.
    let (year, month, name): (i64, i64, String) = lib
        .db()
        .query_row(
            "SELECT event_year, event_month, event_name FROM folders WHERE name = '2020-07 Urlaub Griechenland'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((year, month, name.as_str()), (2020, 7, "Urlaub Griechenland"));
    let tags: String = lib
        .db()
        .query_row(
            "SELECT group_concat(t.name, ',') FROM (SELECT t.name FROM files f
               JOIN file_tags ft ON ft.file_id = f.id JOIN tags t ON t.id = ft.tag_id
               WHERE f.path_nfc = 'Familie/Weihnachten/DSC_2001.jpg' ORDER BY t.name) t",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tags, "Familie,Weihnachten");

    // Paths are stored as found and NFC-normalised.
    let (raw, nfc): (String, String) = lib
        .db()
        .query_row("SELECT path, path_nfc FROM files WHERE name = 'IMG_0100.JPG' AND path NOT LIKE 'fixtures/%'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(raw, format!("{NFD_DIR}/IMG_0100.JPG"));
    assert_eq!(nfc, format!("{NFC_DIR}/IMG_0100.JPG"));

    // Dates from EXIF, when the fixtures are there.
    if fixtures > 0 {
        let taken: String = lib
            .db()
            .query_row(
                "SELECT taken || taken_offset FROM files WHERE path_nfc = 'fixtures/2020-07 Urlaub Griechenland/IMG_0001.JPG'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(taken, "2020-07-14T18:32:05+03:00");
    }
}

#[test]
fn rescan_skips_unchanged_files() {
    let lib = Library::new("rescan");
    let first = lib.scan();
    let second = lib.scan();
    assert_eq!(second.unchanged, first.added);
    assert_eq!(second.added + second.changed + second.moved + second.missing + second.full_hashed, 0);
}

#[test]
fn moves_keep_their_record() {
    let lib = Library::new("moves");
    lib.scan();
    let (id1, hash1, _) = lib.record("2020-07 Urlaub Griechenland/IMG_0001.JPG").unwrap();
    let (id2, _, _) = lib.record("Familie/Weihnachten/DSC_2001.jpg").unwrap();

    // A file into another folder, and a whole folder renamed.
    fs::create_dir_all(lib.path("2020-08 Neu")).unwrap();
    fs::rename(lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG"), lib.path("2020-08 Neu/IMG_0001.JPG")).unwrap();
    fs::rename(lib.path("Familie/Weihnachten"), lib.path("Familie/Weihnachten 2012")).unwrap();

    let stats = lib.scan();
    assert_eq!((stats.moved, stats.added, stats.missing, stats.full_hashed), (2, 0, 0, 0));
    assert_eq!(lib.record("2020-08 Neu/IMG_0001.JPG"), Some((id1, hash1, false)));
    assert_eq!(lib.record("Familie/Weihnachten 2012/DSC_2001.jpg").unwrap().0, id2);
    assert_eq!(lib.count("SELECT count(*) FROM folders WHERE path_nfc = 'Familie/Weihnachten'"), 0);
    let tags: String = lib
        .db()
        .query_row(
            "SELECT group_concat(name, ',') FROM (SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
               WHERE ft.file_id = ?1 ORDER BY t.name)",
            [id2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tags, "Familie,Weihnachten 2012");
    assert_eq!(lib.count("SELECT count(*) FROM tags WHERE name = 'Neu'"), 1);
}

#[test]
fn moved_copy_with_new_name_is_confirmed_by_full_hash() {
    let lib = Library::new("copy-move");
    lib.scan();
    let (id, hash, _) = lib.record("Familie/Weihnachten/DSC_2001.jpg").unwrap();

    // Copy (new mtime) under a new name, then delete the original.
    std::thread::sleep(Duration::from_millis(20));
    fs::copy(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Familie/Weihnachtsbaum.jpg")).unwrap();
    fs::remove_file(lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();

    let stats = lib.scan();
    assert_eq!((stats.moved, stats.added, stats.missing, stats.changed), (1, 0, 0, 0));
    assert_eq!(lib.record("Familie/Weihnachtsbaum.jpg"), Some((id, hash, false)));
}

#[test]
fn moves_are_found_before_full_hashes_exist() {
    let lib = Library::new("quick-move");
    lib.scan_with(false, false);
    let (id, hash, _) = lib.record("Familie/Weihnachten/DSC_2001.jpg").unwrap();
    assert_eq!(hash, None);
    fs::rename(lib.path("Familie/Weihnachten/DSC_2001.jpg"), lib.path("Familie/renamed.jpg")).unwrap();
    let stats = lib.scan_with(false, false);
    assert_eq!((stats.moved, stats.added), (1, 0));
    assert_eq!(lib.record("Familie/renamed.jpg").unwrap().0, id);
}

#[test]
fn same_size_and_quick_hash_but_different_content_is_not_a_move() {
    let lib = Library::new("not-a-move");
    // Same size, same first and last 64 KiB, different middle.
    let mut data = vec![1u8; 300_000];
    lib.write("a/original.jpg", &data);
    lib.scan();
    let (id, _, _) = lib.record("a/original.jpg").unwrap();

    fs::remove_file(lib.path("a/original.jpg")).unwrap();
    data[150_000] = 2;
    lib.write("b/other.jpg", &data);
    let stats = lib.scan();
    assert_eq!((stats.moved, stats.added, stats.missing), (0, 1, 1));
    let (id_after, _, missing) = lib.record("a/original.jpg").unwrap();
    assert_eq!((id_after, missing), (id, true));
}

#[test]
fn changed_content_is_reindexed_and_rehashed() {
    let lib = Library::new("changed");
    lib.scan();
    let (id, hash, _) = lib.record("Familie/Weihnachten/DSC_2001.jpg").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    lib.jpeg("Familie/Weihnachten/DSC_2001.jpg", 99);

    let stats = lib.scan();
    assert_eq!((stats.changed, stats.full_hashed), (1, 1));
    let (id_after, hash_after, _) = lib.record("Familie/Weihnachten/DSC_2001.jpg").unwrap();
    assert_eq!(id_after, id);
    assert!(hash_after.is_some() && hash_after != hash);
}

#[test]
fn exfat_time_zone_shift_is_not_a_change() {
    let lib = Library::new("tz");
    lib.scan();
    let shifted = lib.path("Familie/Weihnachten/DSC_2001.jpg");
    let odd = lib.path("2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let hour: i128 = 3_600_000_000_000;
    set_mtime(&shifted, fingerprint::stamp(&shifted).unwrap().mtime_ns + 2 * hour);
    set_mtime(&odd, fingerprint::stamp(&odd).unwrap().mtime_ns + 2 * hour + 1_000_000_000);

    let stats = lib.scan();
    assert_eq!((stats.tz_shifted, stats.changed), (1, 1));
    // The new mtime is stored, so the next scan skips the file outright.
    assert_eq!(lib.scan().unchanged, stats.unchanged + 2);
}

#[test]
fn missing_files_are_kept_until_forgotten() {
    let lib = Library::new("missing");
    lib.scan();
    let rel = "Ordner mit Leerzeichen/Bild 1.jpeg";
    let (id, _, _) = lib.record(rel).unwrap();
    let saved = fs::read(lib.path(rel)).unwrap();

    fs::remove_file(lib.path(rel)).unwrap();
    assert_eq!(lib.scan().missing, 1);
    assert!(lib.record(rel).unwrap().2);
    assert!(!lib.verify(true).is_clean());

    // Back again: same record.
    lib.write(rel, &saved);
    let stats = lib.scan();
    assert_eq!((stats.missing, stats.added), (0, 0));
    assert_eq!(lib.record(rel).unwrap().0, id);
    assert!(!lib.record(rel).unwrap().2);

    // Deleted for good.
    fs::remove_dir_all(lib.path("Ordner mit Leerzeichen")).unwrap();
    let stats = lib.scan_with(true, true);
    assert_eq!((stats.missing, stats.forgotten), (1, 1));
    assert_eq!(lib.record(rel), None);
    assert_eq!(lib.count("SELECT count(*) FROM folders WHERE name = 'Ordner mit Leerzeichen'"), 0);
    assert!(lib.verify(false).is_clean());
}

#[test]
fn unicode_form_change_is_not_a_move() {
    let lib = Library::new("unicode");
    lib.scan();
    let rel = format!("{NFC_DIR}/IMG_0100.JPG");
    let (id, _, _) = lib.record(&rel).unwrap();

    // Some tools rewrite names in NFC. Linux keeps both forms apart; APFS may
    // keep the old form, so expect whatever the directory now reports.
    fs::rename(lib.path(NFD_DIR), lib.path(NFC_DIR)).unwrap();
    let listed = fs::read_dir(&lib.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .find(|n| n.starts_with("2019-08"))
        .unwrap();
    let stats = lib.scan();
    let renamed = (listed != NFD_DIR) as u64;
    assert_eq!((stats.renamed_unicode, stats.moved, stats.added, stats.missing), (renamed, 0, 0, 0));
    let raw: String = lib.db().query_row("SELECT path FROM files WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(raw, format!("{listed}/IMG_0100.JPG"));
    assert!(lib.verify(false).is_clean());
}

#[test]
fn verify_reports_damage_missing_and_changes() {
    let lib = Library::new("verify");
    lib.scan();

    // Bit rot: content changes, size and mtime stay.
    let rotten = lib.path("2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let stamp = fingerprint::stamp(&rotten).unwrap();
    let mut data = fs::read(&rotten).unwrap();
    let mid = data.len() / 2;
    data[mid] ^= 0xff;
    fs::write(&rotten, &data).unwrap();
    set_mtime(&rotten, stamp.mtime_ns);

    fs::remove_file(lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    lib.jpeg("Familie/Screenshot.png", 7);

    let quick = lib.verify(true);
    assert!(quick.damaged.is_empty(), "quick mode does not read contents");
    let report = lib.verify(false);
    assert!(!report.is_clean());
    assert_eq!(report.damaged, ["2020-07 Urlaub Griechenland/IMG_0001.JPG"]);
    assert_eq!(report.missing, ["Familie/Weihnachten/DSC_2001.jpg"]);
    assert_eq!(report.changed, ["Familie/Screenshot.png"]);
}
