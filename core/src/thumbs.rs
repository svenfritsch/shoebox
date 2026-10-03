//! Previews in `.shoebox/thumbs.db` and the perceptual hash that comes from
//! the same decode.
//!
//! Thumbnails are BLOBs in their own SQLite file (one small file per
//! thumbnail would waste most of each 128 KiB exFAT cluster), keyed by the
//! file's quick hash: a moved file keeps its thumbnail, a changed file gets a
//! new one, and identical copies share one. The database is attached to the
//! library connection as `thumbs`, so one transaction covers a thumbnail and
//! the `phash` it produced.
//!
//! Originals are only read, and every read is wrapped in
//! `fingerprint::read_unchanged`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use libheif_rs::LibHeif;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::classify::Kind;
use crate::db::{self, Job};
use crate::fingerprint;
use crate::media;
use crate::phash;
use crate::scan::{Batch, Progress};

pub const FILE: &str = "thumbs.db";
/// Longer edge of a grid thumbnail. Cells are square and cropped, so this
/// covers a ~190 pt cell on a 2× screen.
pub const EDGE: u32 = 384;
const QUALITY: u8 = 80;
/// Longer edge of the full-screen view of formats browsers cannot show (HEIC).
pub const VIEW_EDGE: u32 = 2048;
const VIEW_QUALITY: u8 = 85;

const SCHEMA_VERSION: i32 = 1;
const SCHEMA_V1: &str = "
CREATE TABLE thumbs.thumbs (
    key     TEXT PRIMARY KEY,   -- files.quick_hash
    width   INTEGER,
    height  INTEGER,
    jpeg    BLOB,               -- NULL when the file could not be rendered
    error   TEXT,
    made_at INTEGER NOT NULL    -- Unix seconds
);
";

/// Location of `thumbs.db` for a library database.
pub fn path_for(db_path: &Path) -> PathBuf {
    db_path.with_file_name(FILE)
}

/// Attach (and create or migrate) `thumbs.db` next to `db_path` as schema
/// `thumbs`. Same durability settings as the library.
pub fn attach(conn: &Connection, db_path: &Path) -> Result<()> {
    let path = path_for(db_path);
    let path_str = path.to_str().context("thumbs.db path is not valid UTF-8")?;
    conn.execute("ATTACH DATABASE ?1 AS thumbs", [path_str])
        .with_context(|| format!("open {}", path.display()))?;
    conn.pragma_update(Some("thumbs"), "journal_mode", "DELETE")?;
    conn.pragma_update(Some("thumbs"), "synchronous", "FULL")?;
    let version: i32 = conn.pragma_query_value(Some("thumbs"), "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("thumbs.db has schema version {version}; this shoebox only knows {SCHEMA_VERSION} (update shoebox)");
    }
    if version < 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.pragma_update(Some("thumbs"), "user_version", 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// A rendered preview.
pub struct Rendered {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Images only; video posters are not compared.
    pub phash: Option<u64>,
}

/// What thumbnailing needs to know about a file.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime_ns: i64,
    pub duration_ms: Option<u64>,
    pub quick_hash: String,
}

/// Render a grid thumbnail. Fails (without touching anything) if the file is
/// not what the index says or changes while it is read.
pub fn render(lib_heif: &LibHeif, ffmpeg: Option<&Path>, src: &Source) -> Result<Rendered, String> {
    fingerprint::read_unchanged(&src.path, src.size, src.mtime_ns, || match src.kind {
        Kind::Raw => Err("RAW files have no preview".into()),
        Kind::Video => {
            let ffmpeg = ffmpeg.ok_or("ffmpeg not found")?;
            let jpeg = media::video_poster(ffmpeg, &src.path, src.duration_ms, EDGE, 4).map_err(|e| format!("{e:#}"))?;
            let (width, height) = image::ImageReader::new(std::io::Cursor::new(&jpeg))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .ok_or("ffmpeg returned no image")?;
            Ok(Rendered { jpeg, width, height, phash: None })
        }
        kind => {
            let img = media::decode_image(lib_heif, kind, &src.path, EDGE).map_err(|e| format!("{e:#}"))?;
            Ok(encode(&shrink(img, EDGE), QUALITY, true).map_err(|e| format!("{e:#}"))?)
        }
    })
}

/// A large JPEG for the full-screen view of a HEIC (browsers other than
/// Safari cannot show HEIC). Rendered on demand, not stored.
pub fn render_view(lib_heif: &LibHeif, src: &Source) -> Result<Vec<u8>, String> {
    fingerprint::read_unchanged(&src.path, src.size, src.mtime_ns, || {
        let img = media::decode_image(lib_heif, src.kind, &src.path, VIEW_EDGE).map_err(|e| format!("{e:#}"))?;
        media::encode_jpeg(&shrink(img, VIEW_EDGE), VIEW_QUALITY).map_err(|e| format!("{e:#}"))
    })
}

/// Fit into `edge`×`edge`; smaller images stay as they are.
fn shrink(img: DynamicImage, edge: u32) -> DynamicImage {
    if img.width().max(img.height()) <= edge { img } else { img.thumbnail(edge, edge) }
}

fn encode(img: &DynamicImage, quality: u8, with_phash: bool) -> Result<Rendered> {
    Ok(Rendered {
        jpeg: media::encode_jpeg(img, quality)?,
        width: img.width(),
        height: img.height(),
        phash: with_phash.then(|| phash::of(img)),
    })
}

/// Store a thumbnail (or the reason there is none) and the perceptual hash
/// of every image with that content.
pub fn store(conn: &Connection, key: &str, result: &Result<Rendered, String>) -> Result<()> {
    match result {
        Ok(r) => {
            conn.execute(
                "INSERT OR REPLACE INTO thumbs.thumbs (key, width, height, jpeg, error, made_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![key, r.width, r.height, r.jpeg, db::now()],
            )?;
            if let Some(h) = r.phash {
                conn.execute(
                    "UPDATE files SET phash = ?2 WHERE quick_hash = ?1 AND kind != 'video'",
                    params![key, phash::to_hex(h)],
                )?;
            }
        }
        Err(e) => {
            conn.execute(
                "INSERT OR REPLACE INTO thumbs.thumbs (key, jpeg, error, made_at) VALUES (?1, NULL, ?2, ?3)",
                params![key, e, db::now()],
            )?;
        }
    }
    Ok(())
}

/// A stored thumbnail: `Ok(jpeg)`, `Err(reason)` if rendering failed, or
/// `None` if it has not been made yet.
pub fn load(conn: &Connection, key: &str) -> Result<Option<Result<Vec<u8>, String>>> {
    let row: Option<(Option<Vec<u8>>, Option<String>)> = conn
        .query_row("SELECT jpeg, error FROM thumbs.thumbs WHERE key = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.map(|(jpeg, error)| jpeg.ok_or_else(|| error.unwrap_or_default())))
}

/// Errors that say nothing about the file itself; they are not stored, so a
/// later run (with ffmpeg in place, or the file settled) tries again.
pub fn is_transient(error: &str) -> bool {
    error == "ffmpeg not found" || error.starts_with("changed ")
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    pub made: u64,
    pub failed: u64,
    /// Videos left without a poster because ffmpeg is not available.
    pub no_ffmpeg: u64,
    /// Perceptual hashes filled in from existing thumbnails.
    pub phash_from_existing: u64,
    pub pruned: u64,
    /// Files whose thumbnail failed, with the reason.
    pub errors: Vec<String>,
}

/// Make every missing thumbnail (newest photos first), fill in perceptual
/// hashes, and drop thumbnails no record refers to. `conn` must have
/// `thumbs` attached. Commits as it goes, so it can be interrupted.
pub fn generate(conn: &Connection, root: &Path) -> Result<Stats> {
    let mut stats = Stats::default();
    let ffmpeg = media::find_ffmpeg();
    // One file per content; the others with the same quick hash share it.
    let pending: Vec<(String, Source)> = conn
        .prepare(
            "SELECT f.path, f.kind, f.size, f.mtime_ns, f.duration_ms, f.quick_hash
             FROM files f LEFT JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE t.key IS NULL AND f.missing_since IS NULL AND f.kind != 'raw'
             GROUP BY f.quick_hash
             ORDER BY max(coalesce(f.taken, '')) DESC, min(f.path_nfc)",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .filter_map(|row| {
            let (rel, kind, size, mtime_ns, duration, quick_hash) = match row {
                Ok(r) => r,
                Err(e) => return Some(Err(e)),
            };
            let kind = Kind::parse(&kind)?;
            let src = Source {
                path: root.join(&rel),
                kind,
                size: size as u64,
                mtime_ns,
                duration_ms: duration.map(|d| d as u64),
                quick_hash,
            };
            Some(Ok((rel, src)))
        })
        .collect::<rusqlite::Result<_>>()?;

    let (pending, videos_skipped): (Vec<_>, Vec<_>) =
        pending.into_iter().partition(|(_, s)| s.kind != Kind::Video || ffmpeg.is_some());
    stats.no_ffmpeg = videos_skipped.len() as u64;
    if stats.no_ffmpeg > 0 {
        println!("No ffmpeg next to shoebox or on PATH: {} videos get no poster for now.", stats.no_ffmpeg);
    }

    if !pending.is_empty() {
        println!("Thumbnails: {} to make…", pending.len());
        let job = Job::start(conn, "thumbs")?;
        let started = Instant::now();
        let total = pending.len() as u64;
        let queue = Mutex::new(pending.into_iter());
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8);
        let (tx, rx) = mpsc::sync_channel::<(String, Source, Result<Rendered, String>)>(workers * 2);
        let ffmpeg = ffmpeg.as_deref();
        std::thread::scope(|scope| -> Result<()> {
            for _ in 0..workers {
                let tx = tx.clone();
                let queue = &queue;
                scope.spawn(move || {
                    let lib_heif = LibHeif::new();
                    loop {
                        let Some((rel, src)) = queue.lock().unwrap().next() else { break };
                        let result = render(&lib_heif, ffmpeg, &src);
                        if tx.send((rel, src, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);

            // Only this thread writes, so the database sees a single writer.
            let mut batch = Batch::begin(conn)?;
            let mut progress = Progress::new(total);
            let mut done = 0;
            let result = (|| -> Result<()> {
                for (rel, src, result) in rx.iter() {
                    done += 1;
                    match &result {
                        Ok(_) => stats.made += 1,
                        Err(e) => {
                            stats.failed += 1;
                            stats.errors.push(format!("{rel}: {e}"));
                        }
                    }
                    if !matches!(&result, Err(e) if is_transient(e)) {
                        store(conn, &src.quick_hash, &result)?;
                    }
                    if batch.tick()? {
                        job.progress(conn, done, Some(total))?;
                    }
                    progress.tick(|done, total| {
                        let per_s = done as f64 / started.elapsed().as_secs_f64().max(1e-9);
                        format!("thumbnails {done}/{total} ({per_s:.1}/s)")
                    });
                }
                Ok(())
            })();
            if result.is_err() {
                // Stop the workers: drain the queue so they finish their current file.
                let mut q = queue.lock().unwrap();
                q.by_ref().for_each(drop);
            }
            result?;
            batch.commit()?;
            job.progress(conn, done, Some(total))?;
            job.finish(conn, "done", &stats)
        })?;
        println!("Made {} thumbnails in {:.0}s ({} failed).", stats.made, started.elapsed().as_secs_f64(), stats.failed);
        for e in stats.errors.iter().take(20) {
            println!("  {e}");
        }
        if stats.errors.len() > 20 {
            println!("  … {} more", stats.errors.len() - 20);
        }
    }

    stats.phash_from_existing = fill_phash(conn)?;
    // Files in the trash keep theirs until the trash is emptied.
    stats.pruned = conn.execute(
        "DELETE FROM thumbs.thumbs WHERE key NOT IN (SELECT quick_hash FROM files)
            AND key NOT IN (SELECT quick_hash FROM trash WHERE quick_hash IS NOT NULL)",
        [],
    )? as u64;
    Ok(stats)
}

/// Images whose content already has a thumbnail but no perceptual hash yet
/// (e.g. a copy that showed up after its twin was thumbnailed). The twin's
/// hash is taken over; only when there is none is the stored thumbnail
/// decoded.
fn fill_phash(conn: &Connection) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut n = tx.execute(
        "UPDATE files SET phash = (SELECT g.phash FROM files g WHERE g.quick_hash = files.quick_hash
                                   AND g.phash IS NOT NULL LIMIT 1)
         WHERE phash IS NULL AND kind NOT IN ('video', 'raw')
           AND EXISTS (SELECT 1 FROM files g WHERE g.quick_hash = files.quick_hash AND g.phash IS NOT NULL)",
        [],
    )? as u64;
    let rows: Vec<(String, Vec<u8>)> = tx
        .prepare(
            "SELECT DISTINCT f.quick_hash, t.jpeg FROM files f JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE f.phash IS NULL AND f.kind NOT IN ('video', 'raw') AND t.jpeg IS NOT NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (key, jpeg) in rows {
        if let Ok(img) = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
            n += tx.execute(
                "UPDATE files SET phash = ?2 WHERE quick_hash = ?1 AND phash IS NULL AND kind NOT IN ('video', 'raw')",
                params![key, phash::to_hex(phash::of(&img))],
            )? as u64;
        }
    }
    tx.commit()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_not_stored() {
        assert!(is_transient("ffmpeg not found"));
        assert!(is_transient("changed while being read"));
        assert!(!is_transient("JPEG: unexpected end of file"));
    }
}
