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

use crate::say;
use crate::classify::Kind;
use crate::db::{self, Job};
use crate::fingerprint;
use crate::media;
use crate::phash;
use crate::screenshots;
use crate::scan::{Batch, Progress};

pub const FILE: &str = "thumbs.db";
/// Longer edge of a grid thumbnail. Cells are square and cropped, so this
/// covers a ~190 pt cell on a 2× screen.
pub const EDGE: u32 = 384;
const QUALITY: u8 = 80;
/// Longer edge of the full-screen view of formats browsers cannot show (HEIC).
pub const VIEW_EDGE: u32 = 2048;
const VIEW_QUALITY: u8 = 85;

const SCHEMA_VERSION: i32 = 4;
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
/// v3: face crops (`faces.rs`), keyed by content and box like the faces in
/// `recognition.db`.
const SCHEMA_V3: &str = "
CREATE TABLE IF NOT EXISTS thumbs.faces (
    key     TEXT NOT NULL,      -- files.quick_hash
    x       REAL NOT NULL,      -- the face's box, as in recog.faces
    y       REAL NOT NULL,
    w       REAL NOT NULL,
    h       REAL NOT NULL,
    jpeg    BLOB,               -- NULL when the photo could not be read
    error   TEXT,
    made_at INTEGER NOT NULL,
    PRIMARY KEY (key, x, y, w, h)
) WITHOUT ROWID;
";

/// v4: where a picture came from. NULL: made by shoebox from the original;
/// `browser`: a video frame a browser grabbed (`store_browser_poster`), which
/// a scan with a poster maker at hand replaces.
const SCHEMA_V4: &str = "ALTER TABLE thumbs.thumbs ADD COLUMN source TEXT;";
/// `thumbs.source` of a video frame sent by a browser.
const FROM_BROWSER: &str = "browser";
/// Largest browser frame accepted, as sent (the UI sends ~20 KB).
pub const BROWSER_POSTER_MAX_BYTES: usize = 512 * 1024;
/// Largest side of a browser frame accepted before it is shrunk.
const BROWSER_POSTER_MAX_EDGE: u32 = 2048;

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
    if version < 2 {
        // v2 decodes images by content, not name (a JPEG called `.HEIC`):
        // try the ones that failed before once more.
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM thumbs.thumbs WHERE jpeg IS NULL", [])?;
        tx.pragma_update(Some("thumbs"), "user_version", 2)?;
        tx.commit()?;
    }
    if version < 3 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V3)?;
        tx.pragma_update(Some("thumbs"), "user_version", 3)?;
        tx.commit()?;
    }
    if version < 4 {
        // Another connection (the server next to a scan) may be doing the
        // same: look and change under one write lock.
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let has_source: bool = tx.query_row(
            "SELECT count(*) > 0 FROM pragma_table_info('thumbs', 'thumbs') WHERE name = 'source'",
            [],
            |r| r.get(0),
        )?;
        if !has_source {
            tx.execute_batch(SCHEMA_V4)?;
        }
        tx.pragma_update(Some("thumbs"), "user_version", 4)?;
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
    /// Images only: how flat the picture is (`screenshots::pixel_score`).
    pub shot_pixels: Option<u8>,
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
            let jpeg = media::video_poster_jpeg(ffmpeg, &src.path, src.duration_ms, EDGE, QUALITY)?;
            let (width, height) = image::ImageReader::new(std::io::Cursor::new(&jpeg))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .ok_or("the video gave no image")?;
            Ok(Rendered { jpeg, width, height, phash: None, shot_pixels: None })
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
pub(crate) fn shrink(img: DynamicImage, edge: u32) -> DynamicImage {
    if img.width().max(img.height()) <= edge { img } else { img.thumbnail(edge, edge) }
}

fn encode(img: &DynamicImage, quality: u8, with_phash: bool) -> Result<Rendered> {
    let jpeg = media::encode_jpeg(img, quality)?;
    // Measured on the stored JPEG, so a backfill from stored thumbnails agrees.
    let shot_pixels = with_phash
        .then(|| image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok())
        .flatten()
        .map(|back| screenshots::pixel_score(&back));
    Ok(Rendered {
        jpeg,
        width: img.width(),
        height: img.height(),
        phash: with_phash.then(|| phash::of(img)),
        shot_pixels,
    })
}

/// Store a thumbnail (or the reason there is none) and the perceptual hash
/// of every image with that content. A failure does not replace a frame a
/// browser sent; it is noted next to it, so the next scan does not try again.
pub fn store(conn: &Connection, key: &str, result: &Result<Rendered, String>) -> Result<()> {
    match result {
        Ok(r) => {
            conn.execute(
                "INSERT OR REPLACE INTO thumbs.thumbs (key, width, height, jpeg, error, made_at, source)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL)",
                params![key, r.width, r.height, r.jpeg, db::now()],
            )?;
            if let Some(p) = r.shot_pixels {
                conn.execute(
                    "UPDATE files SET shot_pixels = ?2 WHERE quick_hash = ?1 AND kind IN ('jpeg', 'png', 'heic')",
                    params![key, p],
                )?;
            }
            if let Some(h) = r.phash {
                conn.execute(
                    "UPDATE files SET phash = ?2 WHERE quick_hash = ?1 AND kind != 'video'",
                    params![key, phash::to_hex(h)],
                )?;
            }
        }
        Err(e) => {
            conn.execute(
                "INSERT INTO thumbs.thumbs (key, jpeg, error, made_at) VALUES (?1, NULL, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET
                     jpeg = CASE WHEN source = 'browser' THEN jpeg END,
                     width = CASE WHEN source = 'browser' THEN width END,
                     height = CASE WHEN source = 'browser' THEN height END,
                     error = excluded.error, made_at = excluded.made_at",
                params![key, e, db::now()],
            )?;
        }
    }
    Ok(())
}

/// A video frame grabbed by a browser, made into a poster of our own: decoded
/// (it must be a JPEG of sensible size), shrunk to `EDGE` and encoded again,
/// so no byte the browser sent is stored.
pub fn browser_poster(bytes: &[u8]) -> Result<Rendered> {
    if bytes.len() > BROWSER_POSTER_MAX_BYTES {
        bail!("the picture is larger than {} KiB", BROWSER_POSTER_MAX_BYTES / 1024);
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(BROWSER_POSTER_MAX_EDGE);
    limits.max_image_height = Some(BROWSER_POSTER_MAX_EDGE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().context("not a JPEG picture")?;
    encode(&shrink(img, EDGE), QUALITY, false)
}

/// Keep a browser's frame as the poster of the video content `key`, only
/// while that content has no picture (none yet, or only the reason it
/// failed; that reason stays, so a scan does not try again). Returns whether
/// it was stored.
pub fn store_browser_poster(conn: &Connection, key: &str, poster: &Rendered) -> Result<bool> {
    let n = conn.execute(
        "INSERT INTO thumbs.thumbs (key, width, height, jpeg, error, made_at, source)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)
         ON CONFLICT (key) DO UPDATE SET
             width = excluded.width, height = excluded.height, jpeg = excluded.jpeg,
             made_at = excluded.made_at, source = excluded.source
         WHERE jpeg IS NULL",
        params![key, poster.width, poster.height, poster.jpeg, db::now(), FROM_BROWSER],
    )?;
    Ok(n > 0)
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
    error == media::NO_POSTER_MAKER || error.starts_with("changed ")
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    pub made: u64,
    pub failed: u64,
    /// Videos left without a poster because this build has no system video
    /// framework and there is no ffmpeg.
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
    let poster_maker = media::SYSTEM_VIDEO_POSTERS || ffmpeg.is_some();
    // One file per content; the others with the same quick hash share it.
    // A video poster a browser made is made again here when there is
    // something to make it with, unless that failed before; a video the
    // system framework could not read is tried again once ffmpeg is there.
    let pending: Vec<(String, Source)> = conn
        .prepare(
            "SELECT f.path, f.kind, f.size, f.mtime_ns, f.duration_ms, f.quick_hash
             FROM files f LEFT JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE (t.key IS NULL
                    OR (?1 AND f.kind = 'video' AND t.source = 'browser' AND t.error IS NULL)
                    OR (?2 AND f.kind = 'video' AND t.error LIKE '%' || ?3))
               AND f.missing_since IS NULL AND f.kind != 'raw'
             GROUP BY f.quick_hash
             ORDER BY max(coalesce(f.taken, '')) DESC, min(f.path_nfc)",
        )?
        .query_map(rusqlite::params![poster_maker, ffmpeg.is_some(), media::NO_FFMPEG_SUFFIX], |r| {
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
        pending.into_iter().partition(|(_, s)| s.kind != Kind::Video || poster_maker);
    stats.no_ffmpeg = videos_skipped.len() as u64;
    if stats.no_ffmpeg > 0 {
        say!("No ffmpeg next to shoebox or on PATH: {} videos get no poster for now.", stats.no_ffmpeg);
    }

    if !pending.is_empty() {
        say!("Thumbnails: {} to make…", pending.len());
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
                    if crate::report::cancelled() {
                        return Err(crate::report::Cancelled.into());
                    }
                    done += 1;
                    match &result {
                        Ok(_) => {
                            stats.made += 1;
                            crate::report::file(&rel, true, "thumbnail made");
                        }
                        Err(e) => {
                            stats.failed += 1;
                            crate::report::file(&rel, false, e.to_string());
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
        say!("Made {} thumbnails in {:.0}s ({} failed).", stats.made, started.elapsed().as_secs_f64(), stats.failed);
        for e in stats.errors.iter().take(20) {
            say!("  {e}");
        }
        if stats.errors.len() > 20 {
            say!("  … {} more", stats.errors.len() - 20);
        }
    }

    stats.phash_from_existing = fill_phash(conn)?;
    fill_shot_pixels(conn)?;
    // Files in the trash keep theirs until the trash is emptied.
    let gone = "key NOT IN (SELECT quick_hash FROM files)
                AND key NOT IN (SELECT quick_hash FROM trash WHERE quick_hash IS NOT NULL)";
    stats.pruned = conn.execute(&format!("DELETE FROM thumbs.thumbs WHERE {gone}"), [])? as u64;
    conn.execute(&format!("DELETE FROM thumbs.faces WHERE {gone}"), [])?;
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

/// One file put back from the trash: its perceptual hash taken over from a
/// twin, else measured on the thumbnail the trash kept, so the duplicates
/// screen finds it again without a scan. Nothing without `thumbs.db`
/// attached (the next scan's `fill_phash` does it then).
pub(crate) fn fill_phash_of(conn: &Connection, id: i64) -> Result<()> {
    let image = "id = ?1 AND phash IS NULL AND kind NOT IN ('video', 'raw')";
    conn.execute(
        &format!(
            "UPDATE files SET phash = (SELECT g.phash FROM files g WHERE g.quick_hash = files.quick_hash
                                       AND g.phash IS NOT NULL LIMIT 1)
             WHERE {image}"
        ),
        [id],
    )?;
    let attached: bool =
        conn.query_row("SELECT count(*) FROM pragma_database_list WHERE name = 'thumbs'", [], |r| r.get::<_, i64>(0))? > 0;
    if !attached {
        return Ok(());
    }
    let jpeg: Option<Vec<u8>> = conn
        .query_row(
            &format!("SELECT t.jpeg FROM files JOIN thumbs.thumbs t ON t.key = files.quick_hash WHERE {image} AND t.jpeg IS NOT NULL"),
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(img) = jpeg.and_then(|j| image::load_from_memory_with_format(&j, image::ImageFormat::Jpeg).ok()) {
        conn.execute("UPDATE files SET phash = ?2 WHERE id = ?1", params![id, phash::to_hex(phash::of(&img))])?;
    }
    Ok(())
}

/// Images that have a thumbnail but no flatness yet (made before phase 11, or
/// a copy that showed up later): taken over from a twin, else measured on the
/// stored thumbnail. The originals are not opened.
fn fill_shot_pixels(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE files SET shot_pixels = (SELECT g.shot_pixels FROM files g WHERE g.quick_hash = files.quick_hash
                                         AND g.shot_pixels IS NOT NULL LIMIT 1)
         WHERE shot_pixels IS NULL AND kind IN ('jpeg', 'png', 'heic')
           AND EXISTS (SELECT 1 FROM files g WHERE g.quick_hash = files.quick_hash AND g.shot_pixels IS NOT NULL)",
        [],
    )?;
    let rows: Vec<(String, Vec<u8>)> = tx
        .prepare(
            "SELECT DISTINCT f.quick_hash, t.jpeg FROM files f JOIN thumbs.thumbs t ON t.key = f.quick_hash
             WHERE f.shot_pixels IS NULL AND f.kind IN ('jpeg', 'png', 'heic') AND t.jpeg IS NOT NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (key, jpeg) in rows {
        if let Ok(img) = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
            tx.execute(
                "UPDATE files SET shot_pixels = ?2 WHERE quick_hash = ?1 AND shot_pixels IS NULL",
                params![key, screenshots::pixel_score(&img)],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 7])));
        media::encode_jpeg(&img, 90).unwrap()
    }

    fn thumbs_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("ATTACH DATABASE ':memory:' AS thumbs", []).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V4).unwrap();
        conn
    }

    fn row(conn: &Connection, key: &str) -> (Option<Vec<u8>>, Option<String>, Option<String>) {
        conn.query_row("SELECT jpeg, error, source FROM thumbs.thumbs WHERE key = ?1", [key], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
    }

    #[test]
    fn a_browser_frame_is_made_into_a_poster_of_our_own() {
        let sent = jpeg(1000, 600);
        let poster = browser_poster(&sent).unwrap();
        assert_eq!((poster.width, poster.height), (EDGE, EDGE * 6 / 10));
        assert_ne!(poster.jpeg, sent);
        assert!(poster.phash.is_none() && poster.shot_pixels.is_none());
        let small = browser_poster(&jpeg(200, 100)).unwrap();
        assert_eq!((small.width, small.height), (200, 100));

        assert!(browser_poster(b"not a picture").is_err());
        assert!(browser_poster(&jpeg(3000, 10)).is_err(), "too wide");
        let mut png = Vec::new();
        DynamicImage::ImageRgb8(image::RgbImage::new(10, 10))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(browser_poster(&png).is_err(), "only JPEG");
        let mut huge = jpeg(100, 100);
        huge.resize(BROWSER_POSTER_MAX_BYTES + 1, 0);
        assert!(browser_poster(&huge).is_err(), "too many bytes");
    }

    #[test]
    fn a_browser_poster_only_fills_a_gap() {
        let conn = thumbs_db();
        let poster = browser_poster(&jpeg(400, 300)).unwrap();

        // No picture yet: kept.
        assert!(store_browser_poster(&conn, "a", &poster).unwrap());
        assert_eq!(row(&conn, "a"), (Some(poster.jpeg.clone()), None, Some("browser".into())));
        // Already one: left alone.
        assert!(!store_browser_poster(&conn, "a", &browser_poster(&jpeg(300, 300)).unwrap()).unwrap());
        assert_eq!(row(&conn, "a").0, Some(poster.jpeg.clone()));

        // A picture made by shoebox is never replaced.
        let made = Rendered { jpeg: jpeg(10, 10), width: 10, height: 10, phash: None, shot_pixels: None };
        store(&conn, "b", &Ok(made)).unwrap();
        assert!(!store_browser_poster(&conn, "b", &poster).unwrap());
        assert_eq!(row(&conn, "b").2, None);

        // Over a failure: kept, and the reason stays so a scan does not retry.
        store(&conn, "c", &Err("ffmpeg: invalid data".into())).unwrap();
        assert!(store_browser_poster(&conn, "c", &poster).unwrap());
        assert_eq!(row(&conn, "c"), (Some(poster.jpeg.clone()), Some("ffmpeg: invalid data".into()), Some("browser".into())));
        assert_eq!(load(&conn, "c").unwrap(), Some(Ok(poster.jpeg.clone())));

        // A later failure keeps the browser's frame and notes the reason ...
        store(&conn, "a", &Err("ffmpeg: no frame".into())).unwrap();
        assert_eq!(row(&conn, "a"), (Some(poster.jpeg.clone()), Some("ffmpeg: no frame".into()), Some("browser".into())));
        // ... and a poster made by shoebox replaces it.
        let made = jpeg(12, 12);
        store(&conn, "a", &Ok(Rendered { jpeg: made.clone(), width: 12, height: 12, phash: None, shot_pixels: None })).unwrap();
        assert_eq!(row(&conn, "a"), (Some(made), None, None));
    }

    #[test]
    fn transient_errors_are_not_stored() {
        assert!(is_transient("ffmpeg not found"));
        assert!(is_transient("changed while being read"));
        assert!(!is_transient("JPEG: unexpected end of file"));
    }
}
