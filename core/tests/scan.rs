//! Integration tests for `shoebox scan` and `shoebox verify`, including the
//! guard: scanning and verifying must leave every original byte-for-byte and
//! timestamp-for-timestamp unchanged.
//!
//! Each test builds a small library in a temp folder. If `SHOEBOX_FIXTURES`
//! points at the output of `scripts/make-fixtures.sh`, those files (HEIC,
//! video, RAW, EXIF dates, decomposed names) are copied in as well.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use shoebox::fingerprint::{self, Stamp};
use shoebox::{scan, verify};

const NFD_DIR: &str = "2019-08 Urlaub O\u{308}sterreich";
const NFC_DIR: &str = "2019-08 Urlaub \u{d6}sterreich";

struct Library {
    root: PathBuf,
}

impl Library {
    fn new(test: &str) -> Library {
        let root = std::env::temp_dir().join(format!("shoebox-it-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let lib = Library { root };

        if let Some(fixtures) = std::env::var_os("SHOEBOX_FIXTURES").filter(|f| !f.is_empty()) {
            copy_dir(Path::new(&fixtures), &lib.root.join("fixtures"));
        }
        lib.jpeg("2020-07 Urlaub Griechenland/IMG_0001.JPG", 1);
        lib.jpeg("2020-07 Urlaub Griechenland/IMG_0002.JPG", 2);
        lib.jpeg("Familie/Weihnachten/DSC_2001.jpg", 3);
        lib.jpeg(&format!("{NFD_DIR}/IMG_0100.JPG"), 4);
        lib.jpeg("Ordner mit Leerzeichen/Bild 1.jpeg", 5);
        image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([x as u8, y as u8, 9]))
            .save(lib.path("Familie/Screenshot.png"))
            .unwrap();
        lib.write("2020-07 Urlaub Griechenland/._IMG_0001.JPG", b"AppleDouble");
        lib.write(".DS_Store", b"junk");
        lib.write(".Spotlight-V100/store.db", b"x");
        lib.write("Familie/notes.txt", b"not a photo");
        lib
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, data: &[u8]) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    /// A real JPEG whose content depends on `seed`.
    fn jpeg(&self, rel: &str, seed: u8) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(320, 240, |x, y| {
            image::Rgb([(x as u8).wrapping_mul(seed), (y as u8).wrapping_add(seed), seed.wrapping_mul(40)])
        })
        .save(p)
        .unwrap();
    }

    fn scan(&self) -> scan::Stats {
        self.scan_with(true, false)
    }

    fn scan_with(&self, full_hash: bool, forget_missing: bool) -> scan::Stats {
        let stats =
            scan::run(&scan::Options { root: self.root.clone(), db: None, full_hash, forget_missing }).unwrap();
        assert!(stats.skipped.is_empty(), "skipped: {:?}", stats.skipped);
        stats
    }

    fn verify(&self, quick: bool) -> verify::Report {
        verify::run(&verify::Options { root: self.root.clone(), db: None, quick, limit: None }).unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(self.path(".shoebox/library.db")).unwrap()
    }

    /// (id, full_hash, missing) of the record at an NFC path.
    fn record(&self, path_nfc: &str) -> Option<(i64, Option<String>, bool)> {
        self.db()
            .query_row(
                "SELECT id, full_hash, missing_since IS NOT NULL FROM files WHERE path_nfc = ?1",
                [path_nfc],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Size, timestamps and full hash of every file outside `.shoebox`.
    fn snapshot(&self) -> BTreeMap<PathBuf, (Stamp, String)> {
        walkdir::WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".shoebox")
            .map(|e| e.unwrap())
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                let p = e.path();
                (p.to_path_buf(), (fingerprint::stamp(p).unwrap(), fingerprint::full_hash(p).unwrap()))
            })
            .collect()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn copy_dir(src: &Path, dst: &Path) {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.unwrap();
        let target = dst.join(entry.path().strip_prefix(src).unwrap());
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).unwrap();
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn set_mtime(path: &Path, mtime_ns: i128) {
    let t = std::time::UNIX_EPOCH + Duration::from_nanos(mtime_ns as u64);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

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
