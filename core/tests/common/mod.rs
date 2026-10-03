//! Shared by the integration tests: a small library in a temp folder, with
//! the fixtures from `scripts/make-fixtures.sh` copied in when
//! `SHOEBOX_FIXTURES` points at them, and the guard snapshot.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use shoebox::fingerprint::{self, Stamp};
use shoebox::{scan, verify};

pub const NFD_DIR: &str = "2019-08 Urlaub O\u{308}sterreich";
pub const NFC_DIR: &str = "2019-08 Urlaub \u{d6}sterreich";

pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(test: &str) -> Library {
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

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn write(&self, rel: &str, data: &[u8]) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    /// A real JPEG whose content depends on `seed`.
    pub fn jpeg(&self, rel: &str, seed: u8) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(320, 240, |x, y| {
            image::Rgb([(x as u8).wrapping_mul(seed), (y as u8).wrapping_add(seed), seed.wrapping_mul(40)])
        })
        .save(p)
        .unwrap();
    }

    pub fn scan(&self) -> scan::Stats {
        self.scan_with(true, false)
    }

    pub fn scan_with(&self, full_hash: bool, forget_missing: bool) -> scan::Stats {
        self.scan_opts(full_hash, true, forget_missing)
    }

    pub fn scan_opts(&self, full_hash: bool, thumbs: bool, forget_missing: bool) -> scan::Stats {
        let stats =
            scan::run(&scan::Options { root: self.root.clone(), db: None, full_hash, thumbs, forget_missing }).unwrap();
        assert!(stats.skipped.is_empty(), "skipped: {:?}", stats.skipped);
        stats
    }

    pub fn verify(&self, quick: bool) -> verify::Report {
        verify::run(&verify::Options { root: self.root.clone(), db: None, quick, limit: None }).unwrap()
    }

    pub fn db(&self) -> Connection {
        Connection::open(self.path(".shoebox/library.db")).unwrap()
    }

    /// (id, full_hash, missing) of the record at an NFC path.
    pub fn record(&self, path_nfc: &str) -> Option<(i64, Option<String>, bool)> {
        self.db()
            .query_row(
                "SELECT id, full_hash, missing_since IS NOT NULL FROM files WHERE path_nfc = ?1",
                [path_nfc],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap()
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Size, timestamps and full hash of every file outside `.shoebox`.
    pub fn snapshot(&self) -> BTreeMap<PathBuf, (Stamp, String)> {
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

pub fn copy_dir(src: &Path, dst: &Path) {
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

pub fn set_mtime(path: &Path, mtime_ns: i128) {
    let t = std::time::UNIX_EPOCH + Duration::from_nanos(mtime_ns as u64);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}
