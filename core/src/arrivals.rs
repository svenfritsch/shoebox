//! After a scan: which of the files it found are copies of files the drive
//! already had?
//!
//! Photos are added to the drive by hand (a copy in the file manager), so the
//! same picture easily arrives a second time. A scan knows exactly which
//! records are new: the ones whose id is higher than any record that existed
//! when it started (`start`). A new file whose content (full hash) equals that
//! of a present file the drive had *before* the scan is an "arrival duplicate";
//! two new files that only match each other are not (there is no older copy to
//! tell which one was first: that is for the duplicates screen).
//!
//! `cleanup` takes those new copies off the library, on the user's request,
//! the way the duplicates screen does (`duplicates::remove_copies`: the copy
//! goes to `.shoebox/trash` or is deleted for good, its tags and dates go to
//! the copy that stays, and a backup check learns that it was removed on
//! purpose). The older copy is never touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db;
use crate::duplicates;
use crate::fingerprint;
use crate::organize;
use crate::report;
use crate::say;

/// Setting: the newest record id before the last scan. Only a scan that ran
/// to the end (and hashed everything it could) leaves it.
const AFTER_KEY: &str = "scan_new_after";

/// How many are listed in the scan's result (the count is exact).
const MAX_LISTED: usize = 2000;

/// A file the scan found that the drive had already.
#[derive(Debug, Clone, Serialize)]
pub struct Duplicate {
    /// The new copy (NFC path).
    pub path: String,
    /// The older one that stays.
    pub of: String,
    pub size: u64,
}

struct Candidate {
    id: i64,
    keeper: i64,
    dup: Duplicate,
}

/// Call before the scan indexes anything: forgets the last offer and returns
/// where "new" starts.
pub fn start(conn: &Connection) -> Result<i64> {
    db::set_setting(conn, AFTER_KEY, None)?;
    Ok(conn.query_row("SELECT coalesce(max(id), 0) FROM files", [], |r| r.get(0))?)
}

/// Call when the scan has hashed everything: remembers where "new" started
/// and returns the arrival duplicates.
pub fn finish(conn: &Connection, after: i64) -> Result<(Vec<Duplicate>, u64)> {
    let found = candidates(conn, after)?;
    // Nothing older to compare with (a first scan) or nothing found: no offer.
    if found.is_empty() {
        return Ok((Vec::new(), 0));
    }
    db::set_setting(conn, AFTER_KEY, Some(&after.to_string()))?;
    let total = found.len() as u64;
    Ok((found.into_iter().take(MAX_LISTED).map(|c| c.dup).collect(), total))
}

fn candidates(conn: &Connection, after: i64) -> Result<Vec<Candidate>> {
    let mut news = conn.prepare(
        "SELECT id, path_nfc, size, full_hash FROM files
         WHERE id > ?1 AND missing_since IS NULL AND full_hash IS NOT NULL ORDER BY path_nfc",
    )?;
    let mut older = conn.prepare(
        "SELECT id, path_nfc FROM files
         WHERE full_hash = ?1 AND id <= ?2 AND missing_since IS NULL ORDER BY path_nfc LIMIT 1",
    )?;
    let rows: Vec<(i64, String, i64, String)> =
        news.query_map([after], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (id, path, size, hash) in rows {
        let keeper: Option<(i64, String)> = older.query_row(params![hash, after], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        if let Some((keeper, of)) = keeper {
            out.push(Candidate { id, keeper, dup: Duplicate { path, of, size: size as u64 } });
        }
    }
    Ok(out)
}

pub struct CleanupOptions {
    pub root: PathBuf,
    /// Delete for good instead of moving into the library's trash.
    pub forever: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct Cleanup {
    /// The copies that went.
    pub removed: Vec<String>,
    /// Why some stayed: changed since the scan, not safe any more, dates to decide, …
    pub skipped: Vec<String>,
    pub forever: bool,
}

/// Remove the arrival duplicates of the last scan, each after asking again
/// whether it is still safe. Changes nothing else on the drive than those
/// files (and `.shoebox`).
pub fn cleanup(opts: &CleanupOptions) -> Result<Cleanup> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    let conn = db::open(&db::default_path(&root))?;
    let mut out = Cleanup { forever: opts.forever, ..Default::default() };
    let Some(after) = db::setting(&conn, AFTER_KEY)?.and_then(|v| v.parse::<i64>().ok()) else {
        say!("Nothing to remove: the last scan found no copies of files the drive already had (or it did not finish).");
        return Ok(out);
    };
    let found = candidates(&conn, after)?;
    let total = found.len() as u64;
    if found.is_empty() {
        say!("Nothing to remove: those copies are gone already.");
        return Ok(out);
    }
    say!("Removing {total} copies of files the drive already had…");
    report::emit(report::Event::Progress { label: "Removing copies".into(), done: 0, total });
    for (i, c) in found.iter().enumerate() {
        if report::cancelled() {
            return Err(report::Cancelled.into());
        }
        report::emit(report::Event::Progress { label: "Removing copies".into(), done: i as u64, total });
        let path = &c.dup.path;
        if let Err(why) = keeper_is_safe(&conn, &root, c) {
            out.skipped.push(format!("{path}: {why}"));
            report::file(path, false, format!("kept: {why}"));
            continue;
        }
        let removed = duplicates::remove_copies(&conn, &root, &[c.keeper], &[c.id], &HashMap::new())?;
        if !removed.conflicts.is_empty() {
            let why = "its capture date differs from the other copy's: decide on the duplicates screen";
            out.skipped.push(format!("{path}: {why}"));
            report::file(path, false, format!("kept: {why}"));
            continue;
        }
        if opts.forever {
            for batch in &removed.trashed.batches {
                organize::empty_trash(&conn, &root, Some(*batch))?;
            }
        }
        if removed.trashed.files.is_empty() {
            let why = removed.trashed.skipped.join("; ");
            report::file(path, false, &why);
            out.skipped.extend(removed.trashed.skipped);
        } else {
            let note = if opts.forever { "deleted (the drive had it already)" } else { "moved to the trash (the drive had it already)" };
            for f in &removed.trashed.files {
                report::file(f, true, note);
            }
            out.removed.extend(removed.trashed.files);
        }
    }
    say!("{} copies removed{}.", out.removed.len(), if opts.forever { " for good" } else { " (they are in the trash)" });
    Ok(out)
}

/// The copy that stays is still there, unchanged, with the same content.
fn keeper_is_safe(conn: &Connection, root: &Path, c: &Candidate) -> std::result::Result<(), String> {
    let row: Option<(String, i64, i64, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT path, size, mtime_ns, full_hash, missing_since FROM files WHERE id = ?1",
            [c.keeper],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((path, size, mtime_ns, full_hash, missing)) = row else { return Err("the copy it duplicates is gone from the library".into()) };
    let same_content: Option<String> =
        conn.query_row("SELECT full_hash FROM files WHERE id = ?1", [c.id], |r| r.get(0)).map_err(|e| e.to_string())?;
    if missing.is_some() || full_hash.is_none() || full_hash != same_content {
        return Err(format!("{} is not safe to rely on any more", c.dup.of));
    }
    match fingerprint::stamp(&root.join(&path)) {
        Ok(s) if s.size == size as u64 && s.mtime_ns == mtime_ns as i128 => Ok(()),
        Ok(_) => Err(format!("{} changed since the scan", c.dup.of)),
        Err(_) => Err(format!("{} is not on the drive any more", c.dup.of)),
    }
}
