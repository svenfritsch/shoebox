//! `shoebox verify`: check the files on the drive against the index.
//!
//! Every present file is re-hashed and compared with its stored full hash.
//! A file whose size and mtime still match but whose content does not is
//! damaged (bit rot or a writer that restored the mtime). Files are checked
//! longest-unverified first, so `--limit` runs cover the library over time.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use rusqlite::params;
use serde::Serialize;

use crate::say;
use crate::db::{self, Job};
use crate::fingerprint;
use crate::scan::{Batch, Progress, hash_unchanged, human_bytes, rate};

pub struct Options {
    pub root: PathBuf,
    pub db: Option<PathBuf>,
    /// Only compare size and mtime; do not read file contents.
    pub quick: bool,
    /// Check at most this many files.
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub checked: u64,
    pub ok: u64,
    pub bytes: u64,
    /// In the index but not on the drive.
    pub missing: Vec<String>,
    /// Size or mtime differs from the index: modified since the last scan.
    pub changed: Vec<String>,
    /// Size and mtime match, content does not.
    pub damaged: Vec<String>,
    /// No full hash yet (scan has not finished hashing).
    pub unhashed: u64,
    /// Not at its old path, but the same content is present elsewhere.
    pub relocated: u64,
    pub errors: Vec<String>,
    pub database_ok: bool,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.database_ok
            && self.missing.is_empty()
            && self.changed.is_empty()
            && self.damaged.is_empty()
            && self.errors.is_empty()
    }
}

pub fn run(opts: &Options) -> Result<Report> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    let db_path = opts.db.clone().unwrap_or_else(|| db::default_path(&root));
    if !db_path.is_file() {
        anyhow::bail!("no index at {} (run `shoebox scan` first)", db_path.display());
    }
    let conn = db::open(&db_path)?;
    let mut report = Report::default();

    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    report.database_ok = check == "ok";
    if !report.database_ok {
        report.errors.push(format!("library.db: {check} (a copy is in library.db.bak)"));
    }

    let files: Vec<(i64, String, u64, i64, Option<String>, bool)> = conn
        .prepare(
            "SELECT id, path, size, mtime_ns, full_hash, missing_since IS NOT NULL FROM files
             ORDER BY verified_at IS NOT NULL, verified_at, path_nfc",
        )?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?, r.get(4)?, r.get(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let files = &files[..opts.limit.unwrap_or(usize::MAX).min(files.len())];
    let total_bytes: u64 = files.iter().map(|f| f.2).sum();
    say!(
        "Verifying {} files ({}){}…",
        files.len(),
        human_bytes(total_bytes),
        if opts.quick { ", size and date only" } else { "" }
    );

    let job = Job::start(&conn, "verify")?;
    let started = Instant::now();
    let mut batch = Batch::begin(&conn)?;
    let mut progress = Progress::new(total_bytes);
    for (id, rel, size, mtime, full_hash, missing) in files {
        if crate::report::cancelled() {
            batch.commit()?;
            job.finish(&conn, "interrupted", &report)?;
            return Err(crate::report::Cancelled.into());
        }
        report.checked += 1;
        let path = root.join(rel);
        progress.add(*size, |done, total| {
            format!("verified {} of {} ({})", human_bytes(done), human_bytes(total), rate(done, started.elapsed()))
        });
        let stamp = match fingerprint::stamp(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Not a loss when the same content is still indexed (and
                // present) under another path: the file was moved or merged
                // into a copy there, and that copy is verified on its own.
                if let Some(other) = full_hash.as_deref().and_then(|h| other_copy(&conn, *id, h)) {
                    crate::report::file(rel, true, format!("moved: the same content is at {other}"));
                    report.relocated += 1;
                    continue;
                }
                crate::report::file(rel, false, "missing from the drive");
                report.missing.push(rel.clone());
                continue;
            }
            Err(e) => {
                crate::report::file(rel, false, e.to_string());
                report.errors.push(format!("{rel}: {e}"));
                continue;
            }
        };
        if *missing || stamp.size != *size || stamp.mtime_ns != *mtime as i128 {
            crate::report::file(rel, false, "changed since the last scan");
            report.changed.push(rel.clone());
            continue;
        }
        if opts.quick {
            crate::report::file(rel, true, "size and date match");
            report.ok += 1;
            continue;
        }
        let Some(expected) = full_hash else {
            crate::report::file(rel, true, "no full hash yet");
            report.unhashed += 1;
            continue;
        };
        match hash_unchanged(&path, *size, *mtime) {
            Ok(hash) if &hash == expected => {
                report.ok += 1;
                report.bytes += size;
                crate::report::file(rel, true, "content matches");
                conn.execute("UPDATE files SET verified_at = ?2 WHERE id = ?1", params![id, db::now()])?;
                if batch.tick()? {
                    job.progress(&conn, report.checked, Some(files.len() as u64))?;
                }
            }
            Ok(_) => {
                crate::report::file(rel, false, "DAMAGED: content differs although size and date match");
                report.damaged.push(rel.clone());
            }
            Err(e) => {
                crate::report::file(rel, false, e.to_string());
                report.errors.push(format!("{rel}: {e}"));
            }
        }
    }
    batch.commit()?;
    job.progress(&conn, report.checked, Some(files.len() as u64))?;
    job.finish(&conn, if report.is_clean() { "done" } else { "failed" }, &report)?;
    print_report(&report);
    Ok(report)
}

/// Another present file with this full hash.
fn other_copy(conn: &rusqlite::Connection, id: i64, hash: &str) -> Option<String> {
    conn.query_row(
        "SELECT path_nfc FROM files WHERE full_hash = ?1 AND id != ?2 AND missing_since IS NULL LIMIT 1",
        params![hash, id],
        |r| r.get(0),
    )
    .ok()
}

fn print_report(r: &Report) {
    say!();
    say!("Checked {} files: {} OK.", r.checked, r.ok);
    if r.unhashed > 0 {
        say!("{} files have no full hash yet (run `shoebox scan`).", r.unhashed);
    }
    if r.relocated > 0 {
        say!("{} files are not at their old path, but the same content is on the drive elsewhere.", r.relocated);
    }
    let list = |title: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        say!("{title} ({}):", items.len());
        for i in items.iter().take(50) {
            say!("  {i}");
        }
        if items.len() > 50 {
            say!("  … {} more", items.len() - 50);
        }
    };
    list("DAMAGED: content differs although size and date match", &r.damaged);
    list("Missing from the drive", &r.missing);
    list("Changed since the last scan (run `shoebox scan`)", &r.changed);
    list("Errors", &r.errors);
    if !r.database_ok {
        say!("The database failed its integrity check.");
    }
}
