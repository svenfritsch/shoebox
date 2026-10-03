//! Importing photos uploaded from the browser into a `YYYY-MM Name` folder.
//!
//! Each upload is streamed into `.shoebox/incoming/` on the same drive and
//! hashed as it arrives. Then its modification date is set to the one the
//! browser reported (`File.lastModified`), and it is renamed into place and
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

/// `YYYY-MM Name`, the folder the import dialog creates.
pub fn event_folder(year: i32, month: u32, name: &str) -> Result<String> {
    if !(1800..=2200).contains(&year) {
        bail!("{year} is not a plausible year");
    }
    if !(1..=12).contains(&month) {
        bail!("{month} is not a month");
    }
    let name = organize::check_name(name)?;
    Ok(format!("{year:04}-{month:02} {name}"))
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
        if let Some(t) = modified {
            file.set_modified(t).context("set the modification date")?;
        }
        file.sync_all().context("write upload")?;
        drop(file);
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
        assert_eq!(event_folder(2021, 3, " Ausflug Ö").unwrap(), "2021-03 Ausflug Ö");
        assert!(library::parse_event(&event_folder(2021, 3, "x").unwrap()).is_some());
        assert!(event_folder(2021, 13, "x").is_err());
        assert!(event_folder(21, 3, "x").is_err());
        assert!(event_folder(2021, 3, "a/b").is_err());
        assert!(event_folder(2021, 3, "").is_err());
    }
}
