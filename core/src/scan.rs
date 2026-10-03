//! `shoebox scan`: bring the index in line with what is on the drive.
//!
//! 1. walk the library (stat only, nothing is opened)
//! 2. moves: a new path whose size matches a vanished record is confirmed
//!    by quick hash (and full hash where needed) and keeps its record
//! 3. every other path: unchanged size + mtime is skipped without opening;
//!    an exFAT time-zone shift is told apart from a real change by quick
//!    hash; new and changed files get quick hash and metadata
//! 4. records whose file is gone are marked missing (kept for later moves)
//! 5. thumbnails and perceptual hashes for files that lack them (`thumbs.rs`)
//! 6. full hashes for every file that lacks one (resumable: progress is
//!    committed as it goes, so an interrupted run continues where it stopped)
//!
//! Originals are only opened for reading. Each file's stamp is taken before
//! and after it is read; a file that changes meanwhile is skipped.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nom_exif::MediaParser;
use rusqlite::{Connection, params};
use serde::Serialize;
use walkdir::WalkDir;

use crate::classify::{self, Kind};
use crate::db::{self, Job};
use crate::fingerprint::{self, Stamp};
use crate::library::{self, RelPath};
use crate::media;
use crate::thumbs;

/// A stored mtime may differ from the file's by a whole number of quarter
/// hours (up to 14 h) when an exFAT drive was written in another time zone.
const TZ_STEP_NS: i64 = 15 * 60 * 1_000_000_000;
const TZ_MAX_NS: i64 = 14 * 60 * 60 * 1_000_000_000;

/// Commit at least this often so an interrupted scan keeps its progress.
const COMMIT_EVERY: usize = 500;
const COMMIT_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);

pub struct Options {
    pub root: PathBuf,
    /// Defaults to `<root>/.shoebox/library.db`.
    pub db: Option<PathBuf>,
    /// Compute missing full hashes after indexing.
    pub full_hash: bool,
    /// Make missing thumbnails (and perceptual hashes) after indexing.
    pub thumbs: bool,
    /// Delete the records of files that are missing after this scan
    /// (deliberately deleted photos) instead of keeping them for move detection.
    pub forget_missing: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    pub files: u64,
    pub folders: u64,
    pub ignored: u64,
    pub unchanged: u64,
    pub tz_shifted: u64,
    pub changed: u64,
    pub added: u64,
    pub moved: u64,
    pub renamed_unicode: u64,
    pub missing: u64,
    pub forgotten: u64,
    pub full_hashed: u64,
    pub bytes_full_hashed: u64,
    pub hash_pending: u64,
    pub thumbs: thumbs::Stats,
    /// Files that could not be indexed this time, with the reason.
    pub skipped: Vec<String>,
    /// The `scan` job that brought the index in line.
    #[serde(skip)]
    pub job_id: i64,
}

/// Steps 1–4: walk the library and bring the index in line with it, as one
/// `scan` job. Also used by `shoebox serve` to follow files that were moved
/// outside shoebox (self-healing paths); `conn` may then be a shared one.
pub fn index_library(conn: &Connection, root: &Path) -> Result<Stats> {
    let mut stats = Stats::default();
    let walked = walk(root, &mut stats);
    stats.files = walked.files.len() as u64;
    stats.folders = walked.folders.len() as u64;
    println!("Found {} media files in {} folders ({} system entries skipped).", stats.files, stats.folders, stats.ignored);

    let job = Job::start(conn, "scan")?;
    stats.job_id = job.id;
    match index(conn, root, &walked, job.id, &mut stats) {
        Ok(()) => {
            job.progress(conn, stats.files, Some(stats.files))?;
            job.finish(conn, "done", &stats)?;
            Ok(stats)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            let _ = job.finish(conn, "failed", &format!("{e:#}"));
            Err(e)
        }
    }
}

pub fn run(opts: &Options) -> Result<Stats> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    let db_path = match &opts.db {
        Some(p) => absolute(p)?,
        None => db::default_path(&root),
    };
    if db_path.starts_with(&root) && !db_path.starts_with(root.join(db::DIR)) {
        bail!("the database must be outside the library or inside its {} folder", db::DIR);
    }
    let conn = db::open(&db_path)?;

    println!("Scanning {}", root.display());
    let mut stats = index_library(&conn, &root)?;
    println!(
        "Index: {} added, {} changed, {} moved, {} unchanged ({} with time-zone shift), {} missing.",
        stats.added,
        stats.changed,
        stats.moved,
        stats.unchanged + stats.tz_shifted,
        stats.tz_shifted,
        stats.missing
    );
    if opts.forget_missing {
        stats.forgotten = forget_missing(&conn, stats.job_id)?;
        println!("Forgot {} missing files.", stats.forgotten);
    }
    db::backup(&conn, &db_path)?;

    if opts.thumbs {
        thumbs::attach(&conn, &db_path)?;
        stats.thumbs = thumbs::generate(&conn, &root)?;
        db::backup(&conn, &db_path)?;
    }
    if opts.full_hash {
        hash_pending(&conn, &root, &mut stats)?;
        db::backup(&conn, &db_path)?;
    }
    stats.hash_pending =
        conn.query_row("SELECT count(*) FROM files WHERE full_hash IS NULL AND missing_since IS NULL", [], |r| {
            r.get::<_, i64>(0)
        })? as u64;
    if stats.hash_pending > 0 {
        println!("{} files still need a full hash (run `shoebox scan` again).", stats.hash_pending);
    }
    if !stats.skipped.is_empty() {
        println!("Skipped ({}):", stats.skipped.len());
        for s in &stats.skipped {
            println!("  {s}");
        }
    }
    Ok(stats)
}

fn absolute(p: &Path) -> Result<PathBuf> {
    let p = std::path::absolute(p)?;
    // Resolve the parent so `starts_with(root)` sees through symlinks.
    match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
        (Some(dir), Some(name)) => Ok(dir.join(name)),
        _ => Ok(p),
    }
}

pub(crate) struct Found {
    pub(crate) rel: RelPath,
    pub(crate) kind: Kind,
    pub(crate) stamp: Stamp,
}

struct Walked {
    files: Vec<Found>,
    folders: Vec<RelPath>,
}

fn walk(root: &Path, stats: &mut Stats) -> Walked {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut seen_nfc = HashSet::new();
    let mut ignored = 0u64;
    let walker = WalkDir::new(root).sort_by_file_name().into_iter().filter_entry(|e| {
        let skip = e.depth() > 0 && classify::is_ignored(&e.file_name().to_string_lossy());
        ignored += skip as u64;
        !skip
    });
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                stats.skipped.push(format!("{}: {e}", e.path().unwrap_or(root).display()));
                continue;
            }
        };
        let is_dir = entry.file_type().is_dir();
        if !is_dir && !entry.file_type().is_file() {
            continue; // symlinks, sockets, …
        }
        let kind = if is_dir {
            None
        } else {
            match classify::kind_of(entry.path()) {
                Some(k) => Some(k),
                None => continue,
            }
        };
        let Some(rel) = RelPath::new(root, entry.path()) else {
            stats.skipped.push(format!("{}: name is not valid UTF-8", entry.path().display()));
            continue;
        };
        if !seen_nfc.insert(rel.nfc.clone()) {
            stats.skipped.push(format!("{}: another entry has the same name (Unicode-normalised)", rel.raw));
            continue;
        }
        match kind {
            None => folders.push(rel),
            Some(kind) => match fingerprint::stamp(entry.path()) {
                Ok(stamp) => files.push(Found { rel, kind, stamp }),
                Err(e) => stats.skipped.push(format!("{}: {e}", rel.raw)),
            },
        }
    }
    stats.ignored = ignored;
    Walked { files, folders }
}

/// The parts of a `files` row the scan compares against.
struct Rec {
    id: i64,
    path: String,
    path_nfc: String,
    name: String,
    size: u64,
    mtime_ns: i64,
    created_ns: Option<i64>,
    quick_hash: String,
    full_hash: Option<String>,
    missing: bool,
}

fn load_records(conn: &Connection) -> Result<HashMap<String, Rec>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, path_nfc, name, size, mtime_ns, created_ns, quick_hash, full_hash,
                missing_since IS NOT NULL FROM files",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(2)?,
            Rec {
                id: r.get(0)?,
                path: r.get(1)?,
                path_nfc: r.get(2)?,
                name: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                mtime_ns: r.get(5)?,
                created_ns: r.get(6)?,
                quick_hash: r.get(7)?,
                full_hash: r.get(8)?,
                missing: r.get(9)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Hashes computed while matching moves, reused by the main pass.
#[derive(Default, Clone)]
struct Known {
    quick: Option<String>,
    full: Option<String>,
}

fn index(conn: &Connection, root: &Path, walked: &Walked, job_id: i64, stats: &mut Stats) -> Result<()> {
    let mut batch = Batch::begin(conn)?;
    let folder_ids = upsert_folders(conn, &walked.folders, job_id)?;
    let mut records = load_records(conn)?;

    let found_nfc: HashSet<&str> = walked.files.iter().map(|f| f.rel.nfc.as_str()).collect();
    let mut known: Vec<Known> = vec![Known::default(); walked.files.len()];
    let mut moved = vec![false; walked.files.len()];

    // Moves: new paths against records whose path no longer exists.
    let mut vanished: HashMap<u64, Vec<&Rec>> = HashMap::new();
    for (path_nfc, rec) in &records {
        if !found_nfc.contains(path_nfc.as_str()) {
            vanished.entry(rec.size).or_default().push(rec);
        }
    }
    let mut moves = Vec::new(); // (found index, record id, old path_nfc)
    for (i, f) in walked.files.iter().enumerate() {
        if records.contains_key(&f.rel.nfc) {
            continue;
        }
        let Some(candidates) = vanished.get_mut(&f.stamp.size) else { continue };
        if candidates.is_empty() {
            continue;
        }
        let path = root.join(&f.rel.raw);
        let quick = match fingerprint::quick_hash(&path) {
            Ok(q) => q,
            Err(e) => {
                stats.skipped.push(format!("{}: {e}", f.rel.raw));
                continue;
            }
        };
        known[i].quick = Some(quick.clone());
        if let Some(pos) = match_move(&path, f, &quick, candidates, &mut known[i]) {
            let rec = candidates.swap_remove(pos);
            moves.push((i, rec.id, rec.path_nfc.clone()));
        }
    }
    for (i, id, old_nfc) in moves {
        let f = &walked.files[i];
        moved[i] = true;
        conn.execute(
            "UPDATE files SET path = ?2, path_nfc = ?3, name = ?4, folder_id = ?5, missing_since = NULL WHERE id = ?1",
            params![id, f.rel.raw, f.rel.nfc, f.rel.name(), folder_ids[f.rel.parent_nfc()]],
        )?;
        set_folder_tags(conn, id, &f.rel)?;
        let mut rec = records.remove(&old_nfc).expect("moved record exists");
        rec.path = f.rel.raw.clone();
        rec.path_nfc = f.rel.nfc.clone();
        rec.missing = false;
        records.insert(f.rel.nfc.clone(), rec);
        stats.moved += 1;
        batch.tick()?;
    }

    // Everything else.
    let mut parser = MediaParser::new();
    let mut progress = Progress::new(walked.files.len() as u64);
    for (i, f) in walked.files.iter().enumerate() {
        progress.tick(|done, total| format!("indexed {done}/{total}"));
        let path = root.join(&f.rel.raw);
        let folder_id = folder_ids[f.rel.parent_nfc()];
        let Some(rec) = records.get(&f.rel.nfc) else {
            match read_file(&mut parser, &path, f, known[i].quick.take()) {
                Ok(info) => {
                    let id = insert_file(conn, f, folder_id, &info)?;
                    set_folder_tags(conn, id, &f.rel)?;
                    stats.added += 1;
                }
                Err(e) => stats.skipped.push(format!("{}: {e}", f.rel.raw)),
            }
            batch.tick()?;
            continue;
        };

        if rec.path != f.rel.raw {
            // Same name, different Unicode form (e.g. rewritten decomposed).
            conn.execute("UPDATE files SET path = ?2 WHERE id = ?1", params![rec.id, f.rel.raw])?;
            stats.renamed_unicode += 1;
        }
        if rec.missing {
            conn.execute("UPDATE files SET missing_since = NULL WHERE id = ?1", [rec.id])?;
        }

        let mtime = ns(f.stamp.mtime_ns);
        let created = f.stamp.created_ns.map(ns);
        let same_size = f.stamp.size == rec.size;
        // A move confirmed by full hash has the stored content, whatever its mtime.
        let same_content = known[i].full.is_some() && known[i].full == rec.full_hash;
        let tz_shifted = || {
            is_tz_shift(mtime - rec.mtime_ns)
                && match known[i].quick.clone() {
                    Some(q) => q == rec.quick_hash,
                    None => fingerprint::quick_hash(&path).is_ok_and(|q| q == rec.quick_hash),
                }
        };
        if same_size && (mtime == rec.mtime_ns || same_content) {
            if mtime != rec.mtime_ns || created != rec.created_ns {
                update_stamp(conn, rec.id, mtime, created)?;
            }
            if !moved[i] {
                stats.unchanged += 1;
            }
        } else if same_size && tz_shifted() {
            update_stamp(conn, rec.id, mtime, created)?;
            stats.tz_shifted += 1;
        } else {
            match read_file(&mut parser, &path, f, None) {
                Ok(info) => {
                    update_file(conn, rec.id, f, &info)?;
                    stats.changed += 1;
                }
                Err(e) => stats.skipped.push(format!("{}: {e}", f.rel.raw)),
            }
        }
        if batch.tick()? {
            // Shows that the scan is alive (`shoebox serve` waits for it).
            conn.execute("UPDATE jobs SET done = ?2, updated_at = ?3 WHERE id = ?1", params![job_id, i as i64, db::now()])?;
        }
    }

    // Records whose file is gone: keep them (a later scan may find the file
    // elsewhere), just mark them missing.
    let now = db::now();
    for (path_nfc, rec) in &records {
        if !rec.missing && !found_nfc.contains(path_nfc.as_str()) {
            conn.execute("UPDATE files SET missing_since = ?2 WHERE id = ?1", params![rec.id, now])?;
            stats.missing += 1;
        }
    }
    prune(conn, job_id)?;
    batch.commit()
}

/// Pick the vanished record a new file was moved from, if any. Same quick
/// hash is required. Same name and mtime (a plain move keeps both) is enough
/// on top of that; otherwise the full hash must match where one is stored.
fn match_move(path: &Path, f: &Found, quick: &str, candidates: &[&Rec], known: &mut Known) -> Option<usize> {
    let mtime = ns(f.stamp.mtime_ns);
    let mut matching: Vec<usize> = (0..candidates.len()).filter(|&j| candidates[j].quick_hash == quick).collect();
    // Prefer same name and mtime, then same name.
    matching.sort_by_key(|&j| (candidates[j].name != f.rel.name(), candidates[j].mtime_ns != mtime));
    for j in matching {
        let c = candidates[j];
        if c.name == f.rel.name() && c.mtime_ns == mtime {
            return Some(j);
        }
        let Some(expected) = &c.full_hash else { return Some(j) };
        if known.full.is_none() {
            known.full = Some(fingerprint::full_hash(path).ok()?);
        }
        if known.full.as_ref() == Some(expected) {
            return Some(j);
        }
    }
    None
}

fn is_tz_shift(diff_ns: i64) -> bool {
    diff_ns != 0 && diff_ns % TZ_STEP_NS == 0 && diff_ns.abs() <= TZ_MAX_NS
}

fn ns(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

pub(crate) struct FileInfo {
    quick_hash: String,
    meta: media::Metadata,
    meta_error: Option<String>,
}

pub(crate) fn read_file(parser: &mut MediaParser, path: &Path, f: &Found, quick: Option<String>) -> Result<FileInfo, String> {
    let quick_hash = match quick {
        Some(q) => q,
        None => fingerprint::quick_hash(path).map_err(|e| e.to_string())?,
    };
    let (meta, meta_error) = match media::read_metadata(parser, f.kind, path) {
        Ok(m) => (m, None),
        Err(e) => (media::Metadata::default(), Some(format!("{e:#}"))),
    };
    let after = fingerprint::stamp(path).map_err(|e| e.to_string())?;
    if after != f.stamp {
        return Err("changed while being read; will be picked up by the next scan".into());
    }
    Ok(FileInfo { quick_hash, meta, meta_error })
}

pub(crate) fn insert_file(conn: &Connection, f: &Found, folder_id: i64, info: &FileInfo) -> Result<i64> {
    let m = &info.meta;
    conn.execute(
        "INSERT INTO files (folder_id, path, path_nfc, name, kind, size, mtime_ns, created_ns, quick_hash,
                            taken, taken_offset, width, height, duration_ms, camera, meta_error, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            folder_id,
            f.rel.raw,
            f.rel.nfc,
            f.rel.name(),
            f.kind.as_str(),
            f.stamp.size as i64,
            ns(f.stamp.mtime_ns),
            f.stamp.created_ns.map(ns),
            info.quick_hash,
            m.taken,
            m.taken_offset,
            m.width,
            m.height,
            m.duration_ms.map(|d| d as i64),
            m.camera,
            info.meta_error,
            db::now(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// New content at a known path: refresh everything and drop the full hash,
/// which the hash pass recomputes.
fn update_file(conn: &Connection, id: i64, f: &Found, info: &FileInfo) -> Result<()> {
    let m = &info.meta;
    conn.execute(
        "UPDATE files SET kind = ?2, size = ?3, mtime_ns = ?4, created_ns = ?5, quick_hash = ?6, full_hash = NULL,
                taken = ?7, taken_offset = ?8, width = ?9, height = ?10, duration_ms = ?11, camera = ?12,
                meta_error = ?13, phash = NULL, verified_at = NULL
         WHERE id = ?1",
        params![
            id,
            f.kind.as_str(),
            f.stamp.size as i64,
            ns(f.stamp.mtime_ns),
            f.stamp.created_ns.map(ns),
            info.quick_hash,
            m.taken,
            m.taken_offset,
            m.width,
            m.height,
            m.duration_ms.map(|d| d as i64),
            m.camera,
            info.meta_error,
        ],
    )?;
    Ok(())
}

fn update_stamp(conn: &Connection, id: i64, mtime: i64, created: Option<i64>) -> Result<()> {
    conn.execute("UPDATE files SET mtime_ns = ?2, created_ns = ?3 WHERE id = ?1", params![id, mtime, created])?;
    Ok(())
}

/// Insert or refresh every folder (parents come first in walk order) and
/// return their ids by NFC path.
pub(crate) fn upsert_folders(conn: &Connection, folders: &[RelPath], job_id: i64) -> Result<HashMap<String, i64>> {
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut stmt = conn.prepare(
        "INSERT INTO folders (parent_id, path, path_nfc, name, event_year, event_month, event_name, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (path_nfc) DO UPDATE SET parent_id = excluded.parent_id, path = excluded.path,
             last_seen = excluded.last_seen
         RETURNING id",
    )?;
    for folder in folders {
        let parent = if folder.nfc.is_empty() { None } else { ids.get(folder.parent_nfc()).copied() };
        let event = library::parse_event(folder.name());
        let id: i64 = stmt.query_row(
            params![
                parent,
                folder.raw,
                folder.nfc,
                folder.name(),
                event.as_ref().map(|e| e.year),
                event.as_ref().map(|e| e.month),
                event.as_ref().map(|e| e.name),
                job_id,
            ],
            |r| r.get(0),
        )?;
        ids.insert(folder.nfc.clone(), id);
    }
    Ok(ids)
}

pub(crate) fn set_folder_tags(conn: &Connection, file_id: i64, rel: &RelPath) -> Result<()> {
    conn.execute("DELETE FROM file_tags WHERE file_id = ?1 AND source = 'folder'", [file_id])?;
    for tag in library::tags_for(rel) {
        let tag_id = db::tag_id(conn, tag)?;
        conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, source) VALUES (?1, ?2, 'folder')",
            params![file_id, tag_id],
        )?;
    }
    Ok(())
}

fn forget_missing(conn: &Connection, job_id: i64) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute("DELETE FROM files WHERE missing_since IS NOT NULL", [])?;
    prune(&tx, job_id)?;
    tx.commit()?;
    Ok(n as u64)
}

/// Drop folders that are gone and hold no records (deepest first), and
/// tags nothing refers to.
pub(crate) fn prune(conn: &Connection, job_id: i64) -> Result<()> {
    loop {
        let n = conn.execute(
            "DELETE FROM folders WHERE last_seen != ?1
                AND id NOT IN (SELECT folder_id FROM files)
                AND id NOT IN (SELECT parent_id FROM folders WHERE parent_id IS NOT NULL)",
            [job_id],
        )?;
        if n == 0 {
            break;
        }
    }
    conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM file_tags)", [])?;
    Ok(())
}

/// Full hashes for present files that lack one, committed as they are made.
fn hash_pending(conn: &Connection, root: &Path, stats: &mut Stats) -> Result<()> {
    let pending: Vec<(i64, String, u64, i64)> = conn
        .prepare(
            "SELECT id, path, size, mtime_ns FROM files
             WHERE full_hash IS NULL AND missing_since IS NULL ORDER BY path_nfc",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if pending.is_empty() {
        return Ok(());
    }
    let total_bytes: u64 = pending.iter().map(|p| p.2).sum();
    println!("Full hashes: {} files, {}…", pending.len(), human_bytes(total_bytes));

    let job = Job::start(conn, "hash")?;
    let started = Instant::now();
    let mut batch = Batch::begin(conn)?;
    let mut progress = Progress::new(total_bytes);
    for (id, rel, size, mtime) in &pending {
        let path = root.join(rel);
        match hash_unchanged(&path, *size, *mtime) {
            Ok(hash) => {
                conn.execute("UPDATE files SET full_hash = ?2 WHERE id = ?1", params![id, hash])?;
                stats.full_hashed += 1;
                stats.bytes_full_hashed += size;
            }
            Err(e) => stats.skipped.push(format!("{rel}: {e}")),
        }
        if batch.tick()? {
            job.progress(conn, stats.bytes_full_hashed, Some(total_bytes))?;
        }
        progress.add(*size, |done, total| {
            format!("hashed {} of {} ({})", human_bytes(done), human_bytes(total), rate(done, started.elapsed()))
        });
    }
    batch.commit()?;
    job.progress(conn, stats.bytes_full_hashed, Some(total_bytes))?;
    job.finish(conn, "done", &serde_json::json!({ "files": stats.full_hashed, "bytes": stats.bytes_full_hashed }))?;
    println!(
        "Hashed {} files ({}) in {:.0}s.",
        stats.full_hashed,
        human_bytes(stats.bytes_full_hashed),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Hash a file the index says has `size` and `mtime`, failing if it differs
/// from that before or after reading.
pub(crate) fn hash_unchanged(path: &Path, size: u64, mtime: i64) -> Result<String, String> {
    fingerprint::read_unchanged(path, size, mtime, || fingerprint::full_hash(path).map_err(|e| e.to_string()))
}

/// An open transaction that commits every few hundred writes or seconds.
pub(crate) struct Batch<'c> {
    conn: &'c Connection,
    pending: usize,
    since: Instant,
}

impl<'c> Batch<'c> {
    pub(crate) fn begin(conn: &'c Connection) -> Result<Self> {
        conn.execute_batch("BEGIN")?;
        Ok(Batch { conn, pending: 0, since: Instant::now() })
    }

    /// Count one write; returns true when it committed.
    pub(crate) fn tick(&mut self) -> Result<bool> {
        self.pending += 1;
        if self.pending >= COMMIT_EVERY || self.since.elapsed() >= COMMIT_INTERVAL {
            self.conn.execute_batch("COMMIT; BEGIN")?;
            self.pending = 0;
            self.since = Instant::now();
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn commit(self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
}

/// Prints a progress line every few seconds.
pub(crate) struct Progress {
    done: u64,
    total: u64,
    last: Instant,
}

impl Progress {
    pub(crate) fn new(total: u64) -> Self {
        Progress { done: 0, total, last: Instant::now() }
    }

    pub(crate) fn tick(&mut self, line: impl FnOnce(u64, u64) -> String) {
        self.add(1, line);
    }

    pub(crate) fn add(&mut self, n: u64, line: impl FnOnce(u64, u64) -> String) {
        self.done += n;
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            println!("  {}", line(self.done, self.total));
            self.last = Instant::now();
        }
    }
}

pub(crate) fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub(crate) fn rate(bytes: u64, d: Duration) -> String {
    format!("{}/s", human_bytes((bytes as f64 / d.as_secs_f64().max(1e-9)) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tz_shift_is_whole_quarter_hours_up_to_14h() {
        let h = 4 * TZ_STEP_NS;
        assert!(is_tz_shift(h));
        assert!(is_tz_shift(-2 * h));
        assert!(is_tz_shift(TZ_STEP_NS * 3)); // 45 min (e.g. Nepal)
        assert!(is_tz_shift(14 * h));
        assert!(!is_tz_shift(15 * h));
        assert!(!is_tz_shift(0));
        assert!(!is_tz_shift(h + 1_000_000_000));
    }
}
