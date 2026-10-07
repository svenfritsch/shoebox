//! Importing photos uploaded from the browser into a `YYYY-MM Name` folder.
//!
//! Each upload is streamed into `.shoebox/incoming/` on the same drive and
//! hashed as it arrives. Then its modification date (and on macOS its created
//! date) is set to the one the browser reported (`File.lastModified`), read
//! back, and it is renamed into place and
//! indexed. Nothing is ever replaced: a name that is taken gets a ` (2)`
//! suffix, and content that is already in the library is not imported a
//! second time unless asked for.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::classify;
use crate::db;
use crate::fingerprint;
use crate::library;
use crate::organize;
use crate::scan;

/// Below `.shoebox/`.
pub const INCOMING_DIR: &str = "incoming";

/// Setting (per library) that holds the `EventPattern` as text.
pub const EVENT_PATTERN_KEY: &str = "event_pattern";

pub fn event_pattern(conn: &rusqlite::Connection) -> Result<library::EventPattern> {
    Ok(db::setting(conn, EVENT_PATTERN_KEY)?.and_then(|t| library::EventPattern::parse(&t)).unwrap_or_default())
}

pub fn set_event_pattern(conn: &rusqlite::Connection, text: &str) -> Result<library::EventPattern> {
    let Some(pattern) = library::EventPattern::parse(text) else { bail!("“{text}” is not a folder naming pattern") };
    // The default is kept as "no setting".
    let value = (pattern != library::EventPattern::default()).then(|| pattern.format());
    db::set_setting(conn, EVENT_PATTERN_KEY, value.as_deref())?;
    Ok(pattern)
}

/// The folder the import dialog creates, named by the library's pattern
/// (default `YYYY-MM Name`).
pub fn event_folder(pattern: &library::EventPattern, year: i32, month: u32, name: &str) -> Result<String> {
    if !(1800..=2200).contains(&year) {
        bail!("{year} is not a plausible year");
    }
    if !(1..=12).contains(&month) {
        bail!("{month} is not a month");
    }
    let name = organize::check_name(name)?;
    let folder = pattern
        .folder_name(year, month, &name)
        .with_context(|| format!("the pattern {} only fits the years 2000 to 2099", pattern.format()))?;
    // It has to be read back as an event folder with this name.
    if library::parse_event(&folder).is_none_or(|e| e.name != name) {
        bail!("“{folder}” would not be recognised as an event folder: with the pattern {} the name may not start with a digit", pattern.format());
    }
    Ok(folder)
}

/// Remove uploads that were cut off (the browser went away, shoebox was
/// stopped). Only called when no upload is running.
pub fn clean_incoming(root: &Path) {
    if let Ok(entries) = fs::read_dir(root.join(db::DIR).join(INCOMING_DIR)) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".part") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Imported,
    /// The same content is in the library already; nothing was written.
    Duplicate,
}

#[derive(Debug, Serialize)]
pub struct Imported {
    pub status: Status,
    /// The new record and where the file went (NFC).
    pub id: Option<i64>,
    pub path: Option<String>,
    /// For duplicates: where the library has it.
    pub duplicate_of: Option<String>,
}

/// One file on its way in.
pub struct Upload {
    tmp: PathBuf,
    file: File,
    hasher: blake3::Hasher,
    size: u64,
    folder: String,
    name: String,
    modified: Option<SystemTime>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Upload {
    /// Check the destination (folder path and file name, NFC) and open the
    /// temporary file.
    pub fn begin(root: &Path, folder: &str, name: &str, modified_ms: Option<i64>) -> Result<Upload> {
        let folder = organize::check_folder_path(folder)?;
        let name = organize::check_name(name)?;
        if classify::kind_of(Path::new(&name)).is_none() {
            bail!("{name} is not a photo or video");
        }
        let dir = root.join(db::DIR).join(INCOMING_DIR);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let unique = format!(
            "{}-{}-{}.part",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let tmp = dir.join(unique);
        let file = File::options().write(true).create_new(true).open(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        let modified = modified_ms.filter(|&ms| ms > 0).map(|ms| UNIX_EPOCH + Duration::from_millis(ms as u64));
        Ok(Upload { tmp, file, hasher: blake3::Hasher::new(), size: 0, folder, name, modified })
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.file.write_all(data).context("write upload")?;
        self.hasher.update(data);
        self.size += data.len() as u64;
        Ok(())
    }

    /// Drop a cut-off upload.
    pub fn abort(self) {
        drop(self.file);
        let _ = fs::remove_file(&self.tmp);
    }

    /// Put the file in place and index it. Holds no lock itself; the caller
    /// keeps other writers away.
    pub fn finish(self, conn: &Connection, root: &Path, keep_duplicates: bool) -> Result<Imported> {
        let tmp = self.tmp.clone();
        let result = self.finish_inner(conn, root, keep_duplicates);
        if tmp.exists() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    fn finish_inner(self, conn: &Connection, root: &Path, keep_duplicates: bool) -> Result<Imported> {
        let Upload { tmp, file, hasher, size, folder, name, modified } = self;
        if size == 0 {
            bail!("{name} is empty");
        }
        file.sync_all().context("write upload")?;
        // After the flush: some exFAT drivers stamp the current time when
        // they write the file out, which would undo the date set before it.
        if let Some(t) = modified {
            set_dates(&file, t).context("set the dates")?;
        }
        drop(file);
        if let Some(t) = modified {
            check_dates(&tmp, t)?;
        }
        let full_hash = hasher.finalize().to_hex().to_string();

        if !keep_duplicates && let Some(existing) = already_have(conn, root, &tmp, size, &full_hash)? {
            return Ok(Imported { status: Status::Duplicate, id: None, path: None, duplicate_of: Some(existing) });
        }

        let (_, folder_raw) = organize::ensure_folder(conn, root, &folder)?;
        let dir = root.join(&folder_raw);
        let name = organize::free_name(&dir, &name)?;
        organize::rename_noreplace(&tmp, &dir.join(&name)).with_context(|| format!("move {name} into place"))?;
        let raw = if folder_raw.is_empty() { name } else { format!("{folder_raw}/{name}") };
        let tx = conn.unchecked_transaction()?;
        let id = organize::index_file(&tx, root, &raw, Some(&full_hash))?;
        tx.commit()?;
        Ok(Imported { status: Status::Imported, id: Some(id), path: Some(library::nfc(&raw)), duplicate_of: None })
    }
}

/// Give the file the date the browser reported as its modification date
/// and, where the platform allows it (macOS), as its created date: the
/// browser does not tell the original created date, and a file that keeps
/// today's created date would show as taken today.
fn set_dates(file: &File, t: SystemTime) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    set_created(file, t)?;
    file.set_modified(t)
}

#[cfg(target_os = "macos")]
fn set_created(file: &File, t: SystemTime) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }
    unsafe extern "C" {
        fn fsetattrlist(fd: libc::c_int, list: *mut AttrList, buf: *mut libc::c_void, size: usize, options: u32) -> libc::c_int;
    }
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMN_CRTIME: u32 = 0x0000_0200;
    let d = t.duration_since(UNIX_EPOCH).map_err(std::io::Error::other)?;
    let mut ts = libc::timespec { tv_sec: d.as_secs() as libc::time_t, tv_nsec: d.subsec_nanos() as _ };
    let mut list = AttrList { bitmapcount: ATTR_BIT_MAP_COUNT, reserved: 0, commonattr: ATTR_CMN_CRTIME, volattr: 0, dirattr: 0, fileattr: 0, forkattr: 0 };
    let r = unsafe { fsetattrlist(file.as_raw_fd(), &mut list, (&mut ts as *mut libc::timespec).cast(), std::mem::size_of::<libc::timespec>(), 0) };
    // A filesystem without a settable created date keeps its own; the
    // modification date is what matters there.
    if r != 0 {
        let e = std::io::Error::last_os_error();
        if ![libc::ENOTSUP, libc::EINVAL, libc::EPERM].contains(&e.raw_os_error().unwrap_or(0)) {
            return Err(e);
        }
    }
    Ok(())
}

/// Read the modification date back: an import must not silently lose it.
fn check_dates(path: &Path, wanted: SystemTime) -> Result<()> {
    let got = fs::metadata(path).and_then(|m| m.modified()).context("read the modification date")?;
    let off = got.duration_since(wanted).unwrap_or_else(|e| e.duration());
    // exFAT keeps 10 ms, FAT 2 s.
    if off > Duration::from_secs(2) {
        bail!("the drive did not keep the file's modification date");
    }
    Ok(())
}

/// The NFC path of a present file with this content. Files that have no full
/// hash yet but the same quick hash are hashed now (and keep the result).
fn already_have(conn: &Connection, root: &Path, upload: &Path, size: u64, full_hash: &str) -> Result<Option<String>> {
    let known: Option<String> = conn
        .query_row(
            "SELECT path_nfc FROM files WHERE full_hash = ?1 AND missing_since IS NULL ORDER BY path_nfc LIMIT 1",
            [full_hash],
            |r| r.get(0),
        )
        .optional()?;
    if known.is_some() {
        return Ok(known);
    }
    let quick = fingerprint::quick_hash(upload)?;
    let unhashed: Vec<(i64, String, String, i64)> = conn
        .prepare(
            "SELECT id, path, path_nfc, mtime_ns FROM files
             WHERE quick_hash = ?1 AND size = ?2 AND full_hash IS NULL AND missing_since IS NULL",
        )?
        .query_map(params![quick, size as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, raw, nfc, mtime) in unhashed {
        if let Ok(h) = scan::hash_unchanged(&root.join(&raw), size, mtime) {
            conn.execute("UPDATE files SET full_hash = ?2 WHERE id = ?1", params![id, h])?;
            if h == full_hash {
                return Ok(Some(nfc));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_folders_are_checked() {
        let d = library::EventPattern::default();
        assert_eq!(event_folder(&d, 2021, 3, " Ausflug Ö").unwrap(), "2021-03 Ausflug Ö");
        assert!(library::parse_event(&event_folder(&d, 2021, 3, "x").unwrap()).is_some());
        assert!(event_folder(&d, 2021, 13, "x").is_err());
        assert!(event_folder(&d, 21, 3, "x").is_err());
        assert!(event_folder(&d, 2021, 3, "a/b").is_err());
        assert!(event_folder(&d, 2021, 3, "").is_err());
        let short = library::EventPattern::parse("YY.MM_Name").unwrap();
        assert_eq!(event_folder(&short, 2021, 3, "Ausflug").unwrap(), "21.03_Ausflug");
        assert!(event_folder(&short, 1998, 8, "Urlaub").is_err());
        assert!(event_folder(&short, 2021, 3, "2019 Reise").is_err());
        assert!(event_folder(&d, 2021, 3, "2019 Reise").is_ok());
    }
}
