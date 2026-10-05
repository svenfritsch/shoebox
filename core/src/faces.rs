//! Did recognition work? (phase 5c-1) The faces `shoebox recognize` stored,
//! looked at as a whole: `shoebox faces stats`, and for the web UI's face
//! check page a sorted list, face crops and nearest neighbours.
//!
//! Everything here only reads `recognition.db` (and the face decisions in
//! `library.db`: faces marked "not a face" are counted, and listed only when
//! asked for, see `people.rs`). The one thing written is the crop cache in `thumbs.db` (`thumbs.faces`, keyed by content and
//! box like the faces), and crops are made from the original under the
//! guard (`fingerprint::read_unchanged`), like thumbnails.

use std::path::Path;

use anyhow::{Context, Result};
use image::DynamicImage;
use libheif_rs::LibHeif;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::fingerprint;
use crate::media;
use crate::recognize::{self, FACES, FACES_ROT, KINDS};
use crate::thumbs::{self, Source};

/// Faces narrower than this (px in the ≤1600 px copy the worker saw) are
/// listed but too small to cluster or suggest. Decided on the real drive
/// (docs/phase5.md): the faces of 30–40 px all were real, recognisable people.
pub const MIN_CLUSTER_PX: f64 = 30.0;
/// Longer edge of a face crop.
pub const CROP_EDGE: u32 = 160;
const CROP_QUALITY: u8 = 85;
/// Room around the face in a crop, as a share of the face's longer side.
const CROP_MARGIN: f64 = 0.25;

const WIDTH_BUCKETS: [f64; 4] = [30.0, 40.0, 60.0, 120.0];
/// YuNet's scores of stored faces lie between 0.85 (the recognizer's cut-off)
/// and ~0.96 (the real drive: none higher), so the buckets split that range.
const SCORE_BUCKETS: [f64; 4] = [0.88, 0.90, 0.92, 0.94];

/// Too small to take part in clustering (`MIN_CLUSTER_PX`).
pub fn too_small(px: f64) -> bool {
    px < MIN_CLUSTER_PX
}

/// SQL for a face's width in px of the copy the worker saw (`f` a row of
/// `recog.faces`, `l` its `faces` row in `recog.looked`), measured across
/// the face: one found turned 90° lies sideways in the picture, so its width
/// is the box's height. `roll` is the column (or `0` in a v1 database).
pub(crate) fn size_px(roll: &str) -> String {
    format!("(CASE WHEN {roll} % 180 = 90 THEN f.h * l.height ELSE f.w * l.width END)")
}

/// Contents of present photos, one file each (the lowest id).
pub(crate) const PRESENT: &str = "SELECT quick_hash, min(id) AS id, kind FROM files
                       WHERE missing_since IS NULL AND kind IN ('jpeg', 'png', 'heic') GROUP BY quick_hash";

// ---------------------------------------------------------------- stats

#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    /// `from <= value < to`; `None` is open-ended.
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub count: u64,
    /// Of `count`, marked "not a face" (false finds).
    pub not_face: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorCount {
    pub message: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub kind: String,
    pub state: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub done: i64,
    pub total: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    /// Photos on the drive (one per content).
    pub photos: u64,
    /// Of those, looked at by the upright pass (successfully or not).
    pub looked: u64,
    pub failed: u64,
    /// Failures grouped by message, most common first.
    pub errors: Vec<ErrorCount>,
    /// Looked at by the rotated pass (`--rotated`), and failed there.
    pub rotated_looked: u64,
    pub rotated_failed: u64,
    pub faces: u64,
    /// Of `faces`, found by the rotated pass.
    pub rotated_faces: u64,
    /// Face width in px of the ≤1600 px copy, in `WIDTH_BUCKETS`.
    pub widths: Vec<Bucket>,
    pub scores: Vec<Bucket>,
    /// Faces under `min_cluster_px`.
    pub small: u64,
    /// Of `faces`, marked "not a face" by the user (false finds).
    pub not_faces: u64,
    pub min_cluster_px: f64,
    /// The last runs of `shoebox recognize`, newest first.
    pub runs: Vec<Run>,
}

/// The stats over present photos. `conn` has `recog` attached; a database
/// still at schema v1 (no `roll`) works too, so this can run read-only.
pub fn stats(conn: &Connection) -> Result<Stats> {
    let version: i32 = conn.pragma_query_value(Some("recog"), "user_version", |r| r.get(0))?;
    let roll = if version >= 2 { "f.roll" } else { "0" };
    let present = format!("(SELECT DISTINCT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS}))");

    let photos: i64 = conn.query_row(&format!("SELECT count(*) FROM {present}"), [], |r| r.get(0))?;
    let looked = |task: &str| -> Result<(i64, i64)> {
        Ok(conn.query_row(
            &format!("SELECT count(*), count(error) FROM recog.looked WHERE task = ?1 AND key IN {present}"),
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    };
    let (looked_n, failed) = looked(FACES)?;
    let (rotated_looked, rotated_failed) = looked(FACES_ROT)?;
    let errors = conn
        .prepare(&format!(
            "SELECT error, count(*) AS n FROM recog.looked WHERE task = ?1 AND error IS NOT NULL AND key IN {present}
             GROUP BY error ORDER BY n DESC, error"
        ))?
        .query_map([FACES], |r| Ok(ErrorCount { message: r.get(0)?, count: r.get::<_, i64>(1)? as u64 }))?
        .collect::<rusqlite::Result<_>>()?;

    let mut widths = buckets(&WIDTH_BUCKETS);
    let mut scores = buckets(&SCORE_BUCKETS);
    let (mut faces, mut rotated_faces, mut small, mut not_faces) = (0, 0, 0, 0);
    let marked = crate::people::not_face_ids(conn)?;
    let mut rows = conn.prepare(&format!(
        "SELECT {}, f.score, {roll}, f.id FROM recog.faces f
         JOIN recog.looked l ON l.key = f.key AND l.task = '{FACES}'
         WHERE f.key IN {present}",
        size_px(roll)
    ))?;
    let mut rows = rows.query([])?;
    while let Some(r) = rows.next()? {
        let (px, score, roll): (Option<f64>, f64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let px = px.unwrap_or(0.0);
        let not_face = marked.contains(&r.get(3)?);
        faces += 1;
        rotated_faces += (roll != 0) as u64;
        small += too_small(px) as u64;
        not_faces += not_face as u64;
        count_into(&mut widths, px, not_face);
        count_into(&mut scores, score, not_face);
    }

    let runs = conn
        .prepare(
            "SELECT kind, state, started_at, finished_at, done, total FROM recog.jobs
             WHERE kind IN ('faces', 'faces-rot') ORDER BY id DESC LIMIT 5",
        )?
        .query_map([], |r| {
            Ok(Run {
                kind: r.get(0)?,
                state: r.get(1)?,
                started_at: r.get(2)?,
                finished_at: r.get(3)?,
                done: r.get(4)?,
                total: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    Ok(Stats {
        photos: photos as u64,
        looked: looked_n as u64,
        failed: failed as u64,
        errors,
        rotated_looked: rotated_looked as u64,
        rotated_failed: rotated_failed as u64,
        faces,
        rotated_faces,
        widths,
        scores,
        small,
        not_faces,
        min_cluster_px: MIN_CLUSTER_PX,
        runs,
    })
}

fn buckets(edges: &[f64]) -> Vec<Bucket> {
    let mut out = Vec::with_capacity(edges.len() + 1);
    let mut from = None;
    for &e in edges {
        out.push(Bucket { from, to: Some(e), count: 0, not_face: 0 });
        from = Some(e);
    }
    out.push(Bucket { from, to: None, count: 0, not_face: 0 });
    out
}

fn count_into(buckets: &mut [Bucket], value: f64, not_face: bool) {
    if let Some(b) = buckets.iter_mut().find(|b| b.to.is_none_or(|to| value < to)) {
        b.count += 1;
        b.not_face += not_face as u64;
    }
}

/// Open the index and `recognition.db` read-only (nothing is created or
/// migrated); `None` if there is no `recognition.db` yet.
pub fn open_readonly(root: &Path, db: Option<&Path>) -> Result<Option<Connection>> {
    let db_path = match db {
        Some(p) => std::path::absolute(p)?,
        None => db::default_path(root),
    };
    if !db_path.is_file() {
        anyhow::bail!("no index at {} (run `shoebox scan` first)", db_path.display());
    }
    let recog_path = recognize::path_for(&db_path);
    if !recog_path.is_file() {
        return Ok(None);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&db_path, flags).with_context(|| format!("open {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    // Attached databases are opened with the connection's flags: read-only too.
    let recog_str = recog_path.to_str().context("recognition.db path is not valid UTF-8")?;
    conn.execute("ATTACH DATABASE ?1 AS recog", [recog_str])
        .with_context(|| format!("open {}", recog_path.display()))?;
    Ok(Some(conn))
}

/// The stats of `shoebox faces stats`, `None` before the first
/// `shoebox recognize`. Only reads.
pub fn stats_for(root: &Path, db: Option<&Path>) -> Result<Option<Stats>> {
    match open_readonly(root, db)? {
        Some(conn) => Ok(Some(stats(&conn)?)),
        None => Ok(None),
    }
}

/// `shoebox faces stats`: print the stats. Only reads.
pub fn print_stats(root: &Path, db: Option<&Path>) -> Result<Option<Stats>> {
    let stats = stats_for(root, db)?;
    match &stats {
        Some(s) => crate::say!("{}", format_stats(s).trim_end_matches('\n')),
        None => crate::say!("No faces yet: run `shoebox recognize` first."),
    }
    Ok(stats)
}

fn percent(n: u64, of: u64) -> f64 {
    if of == 0 { 0.0 } else { 100.0 * n as f64 / of as f64 }
}

fn bucket_label(b: &Bucket, unit: &str, decimals: usize) -> String {
    match (b.from, b.to) {
        (None, Some(to)) => format!("under {to:.decimals$}{unit}"),
        (Some(from), Some(to)) => format!("{from:.decimals$}–{to:.decimals$}{unit}"),
        (Some(from), None) => format!("{from:.decimals$}{unit} and more"),
        (None, None) => "all".into(),
    }
}

fn format_buckets(out: &mut String, buckets: &[Bucket], total: u64, unit: &str, decimals: usize) {
    let max = buckets.iter().map(|b| b.count).max().unwrap_or(0).max(1);
    let marked = buckets.iter().any(|b| b.not_face > 0);
    for b in buckets {
        let bar = "█".repeat(((b.count * 30).div_ceil(max)) as usize);
        let not_face = if marked { format!(" {:>6} not a face", b.not_face) } else { String::new() };
        out.push_str(&format!(
            "  {:<16} {:>7} {:>5.1}%{not_face}  {bar}\n",
            bucket_label(b, unit, decimals),
            b.count,
            percent(b.count, total)
        ));
    }
}

fn format_time(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn format_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", (s + 30) / 60),
        s => format!("{} h {} min", s / 3600, (s % 3600 + 30) / 60),
    }
}

pub fn format_stats(s: &Stats) -> String {
    let mut out = String::new();
    let waiting = s.photos.saturating_sub(s.looked);
    out.push_str(&format!("Photos:  {} looked at of {}", s.looked, s.photos));
    if waiting > 0 {
        out.push_str(&format!(" ({waiting} still to look at)"));
    }
    out.push('\n');
    out.push_str(&format!("Failed:  {}\n", s.failed));
    for e in s.errors.iter().take(15) {
        out.push_str(&format!("  {:>6}  {}\n", e.count, e.message));
    }
    if s.errors.len() > 15 {
        out.push_str(&format!("  … {} other messages\n", s.errors.len() - 15));
    }
    let ok = s.looked - s.failed;
    out.push_str(&format!(
        "Faces:   {} ({:.2} per photo looked at)\n",
        s.faces,
        if ok == 0 { 0.0 } else { s.faces as f64 / ok as f64 }
    ));
    if s.rotated_looked > 0 {
        out.push_str(&format!(
            "Turned:  {} photos looked at turned 90° and 270° ({} failed): {} faces added\n",
            s.rotated_looked, s.rotated_failed, s.rotated_faces
        ));
    } else {
        out.push_str("Turned:  not looked at yet (`shoebox recognize --rotated` finds faces of people lying down)\n");
    }
    if s.faces > 0 {
        out.push_str("\nFace width in the ≤1600 px copy:\n");
        format_buckets(&mut out, &s.widths, s.faces, " px", 0);
        out.push_str(&format!(
            "  Under {:.0} px (listed, too small for clustering): {} ({:.1}%)\n",
            s.min_cluster_px,
            s.small,
            percent(s.small, s.faces)
        ));
        if s.not_faces > 0 {
            out.push_str(&format!(
                "  Marked \"not a face\" (false finds): {} ({:.1}%)\n",
                s.not_faces,
                percent(s.not_faces, s.faces)
            ));
        }
        out.push_str("\nScore:\n");
        format_buckets(&mut out, &s.scores, s.faces, "", 2);
    }
    if !s.runs.is_empty() {
        out.push_str("\nLast runs:\n");
        for r in &s.runs {
            let took = r.finished_at.map(|f| format_duration(f - r.started_at)).unwrap_or_default();
            let total = r.total.map(|t| format!("/{t}")).unwrap_or_default();
            out.push_str(&format!(
                "  {}  {:<9} {:<11} {:>13}  {took}\n",
                format_time(r.started_at),
                r.kind,
                r.state,
                format!("{}{total}", r.done)
            ));
        }
    }
    out
}

// ---------------------------------------------------------------- the face list

/// How the face check page asks for faces.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ListQuery {
    /// `size` (default) or `score`.
    pub sort: Option<String>,
    /// Largest / best first.
    #[serde(default)]
    pub desc: bool,
    /// Only faces at least this wide (px) …
    pub min_px: Option<f64>,
    /// … and narrower than this.
    pub max_px: Option<f64>,
    /// Only faces found by the rotated pass.
    #[serde(default)]
    pub rotated: bool,
    /// Only faces marked "not a face" (to undo); they are left out
    /// otherwise.
    #[serde(default)]
    pub not_face: bool,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// `recog.faces.id`
    pub id: i64,
    /// A present file with this content.
    pub file: i64,
    /// `jpeg`, `png` or `heic`.
    pub kind: String,
    /// For thumbnail URLs (as in the timeline).
    pub version: String,
    /// Width in px of the ≤1600 px copy.
    pub px: f64,
    pub score: f64,
    pub roll: u16,
    /// Under `MIN_CLUSTER_PX`: listed, but not for clustering.
    pub small: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct List {
    /// Faces matching the filter (before offset and limit).
    pub total: u64,
    pub faces: Vec<Item>,
    pub min_cluster_px: f64,
}

const DEFAULT_LIMIT: usize = 300;
const MAX_LIMIT: usize = 2000;

/// `SELECT` of an `Item` (plus `extra` columns at the end) over faces of
/// present photos; `f` is the face, `p` its file.
fn item_sql(extra: &str, filter: &str) -> String {
    format!(
        "SELECT f.id, p.id, p.kind, p.quick_hash, {px} AS px, f.score, f.roll{extra}
         FROM recog.faces f
         JOIN recog.looked l ON l.key = f.key AND l.task = '{FACES}'
         JOIN ({PRESENT}) p ON p.quick_hash = f.key
         WHERE {filter}",
        px = size_px("f.roll")
    )
}

fn item(r: &rusqlite::Row) -> rusqlite::Result<Item> {
    let key: String = r.get(3)?;
    let px: f64 = r.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
    Ok(Item {
        id: r.get(0)?,
        file: r.get(1)?,
        kind: r.get(2)?,
        version: key.chars().take(8).collect(),
        px,
        score: r.get(5)?,
        roll: r.get(6)?,
        small: too_small(px),
    })
}

/// Faces of present photos, filtered and sorted for the face check page.
/// `conn` has `recog` attached (at the current schema).
pub fn list(conn: &Connection, q: &ListQuery) -> Result<List> {
    let order = match q.sort.as_deref() {
        Some("score") => "f.score",
        _ => "px",
    };
    let dir = if q.desc { "DESC" } else { "ASC" };
    // Ids are numbers from the database, so they can go into the SQL.
    let marked: Vec<String> = crate::people::not_face_ids(conn)?.iter().map(i64::to_string).collect();
    let filter = format!(
        "px >= ?1 AND px < ?2{} AND f.id {} ({})",
        if q.rotated { " AND f.roll != 0" } else { "" },
        if q.not_face { "IN" } else { "NOT IN" },
        marked.join(",")
    );
    let sql = format!(
        "{} ORDER BY {order} {dir}, f.id LIMIT ?3 OFFSET ?4",
        item_sql(", count(*) OVER ()", &filter)
    );
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let (min, max) = (q.min_px.unwrap_or(0.0), q.max_px.unwrap_or(f64::MAX));
    let mut total = 0;
    let faces = conn
        .prepare(&sql)?
        .query_map(params![min, max, limit as i64, q.offset as i64], |r| {
            total = r.get::<_, i64>(7)? as u64;
            item(r)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if faces.is_empty() && q.offset > 0 {
        // Past the end: count without the page.
        total = conn.query_row(&format!("SELECT count(*) FROM ({})", item_sql("", &filter)), params![min, max], |r| {
            r.get::<_, i64>(0)
        })? as u64;
    }
    Ok(List { total, faces, min_cluster_px: MIN_CLUSTER_PX })
}

#[derive(Debug, Clone, Serialize)]
pub struct Neighbour {
    #[serde(flatten)]
    pub face: Item,
    /// Cosine similarity of the embeddings (1: identical).
    pub similarity: f32,
}

/// The faces (of present photos, same model, not marked "not a face") whose
/// embeddings are closest to face `id`'s, best first; `None` if there is no
/// such face. Only reads: a look at which threshold separates people.
pub fn similar(conn: &Connection, id: i64, limit: usize) -> Result<Option<Vec<Neighbour>>> {
    let target: Option<(String, Vec<u8>)> = conn
        .query_row("SELECT model, emb FROM recog.faces WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((model, emb)) = target else { return Ok(None) };
    let target = floats(&emb);
    let marked = crate::people::not_face_ids(conn)?;
    let mut stmt = conn.prepare(&item_sql(", f.emb", "f.model = ?1 AND f.id != ?2"))?;
    let mut rows = stmt.query(params![model, id])?;
    let mut all = Vec::new();
    while let Some(r) = rows.next()? {
        let emb: Vec<u8> = r.get(7)?;
        let other = floats(&emb);
        if other.len() != target.len() || marked.contains(&r.get(0)?) {
            continue;
        }
        let similarity = target.iter().zip(&other).map(|(a, b)| a * b).sum::<f32>();
        all.push(Neighbour { face: item(r)?, similarity });
    }
    all.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    all.truncate(limit);
    Ok(Some(all))
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

// ---------------------------------------------------------------- crops

/// A face's box, as stored (fractions of the upright picture).
#[derive(Debug, Clone, PartialEq)]
pub struct FaceBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub roll: u16,
}

/// Content key and box of face `id`, and a present file with that content
/// (`None` when the face's photo is gone or in the trash).
pub fn face(conn: &Connection, id: i64) -> Result<Option<(String, FaceBox, Option<i64>)>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT f.key, f.x, f.y, f.w, f.h, f.roll, p.id FROM recog.faces f
                 LEFT JOIN ({PRESENT}) p ON p.quick_hash = f.key WHERE f.id = ?1"
            ),
            [id],
            |r| {
                let b = FaceBox { x: r.get(1)?, y: r.get(2)?, w: r.get(3)?, h: r.get(4)?, roll: r.get(5)? };
                Ok((r.get(0)?, b, r.get(6)?))
            },
        )
        .optional()?)
}

/// Content key and box of the face drawn by hand with this `manual` id
/// (the decision row), and a present file with that content.
pub fn drawn_face(conn: &Connection, id: i64) -> Result<Option<(String, FaceBox, Option<i64>)>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT d.key, d.x, d.y, d.w, d.h, p.id FROM face_decisions d
                 LEFT JOIN ({PRESENT}) p ON p.quick_hash = d.key WHERE d.id = ?1 AND d.manual = 1"
            ),
            [id],
            |r| {
                let b = FaceBox { x: r.get(1)?, y: r.get(2)?, w: r.get(3)?, h: r.get(4)?, roll: 0 };
                Ok((r.get(0)?, b, r.get(5)?))
            },
        )
        .optional()?)
}

/// All faces of a content: detected ones, then ones drawn by hand.
pub fn boxes_of(conn: &Connection, key: &str) -> Result<Vec<FaceBox>> {
    let mut boxes: Vec<FaceBox> = conn
        .prepare("SELECT x, y, w, h, roll FROM recog.faces WHERE key = ?1 ORDER BY id")?
        .query_map([key], |r| Ok(FaceBox { x: r.get(0)?, y: r.get(1)?, w: r.get(2)?, h: r.get(3)?, roll: r.get(4)? }))?
        .collect::<rusqlite::Result<_>>()?;
    if crate::people::table_exists(conn, "main", "face_decisions")? {
        let drawn = conn
            .prepare("SELECT x, y, w, h FROM face_decisions WHERE key = ?1 AND manual = 1 ORDER BY id")?
            .query_map([key], |r| Ok(FaceBox { x: r.get(0)?, y: r.get(1)?, w: r.get(2)?, h: r.get(3)?, roll: 0 }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for d in drawn {
            if !boxes.contains(&d) {
                boxes.push(d);
            }
        }
    }
    Ok(boxes)
}

/// A stored crop: `Ok(jpeg)`, `Err(reason)` if the photo could not be read,
/// `None` if it has not been made. `conn` has `thumbs` attached.
pub fn load_crop(conn: &Connection, key: &str, b: &FaceBox) -> Result<Option<Result<Vec<u8>, String>>> {
    let row: Option<(Option<Vec<u8>>, Option<String>)> = conn
        .query_row(
            "SELECT jpeg, error FROM thumbs.faces WHERE key = ?1 AND x = ?2 AND y = ?3 AND w = ?4 AND h = ?5",
            params![key, b.x, b.y, b.w, b.h],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(jpeg, error)| jpeg.ok_or_else(|| error.unwrap_or_default())))
}

/// Crops of `boxes` from one decode of the photo, under the guard. Faces the
/// rotated pass found are turned upright.
pub fn render_crops(lib_heif: &LibHeif, src: &Source, boxes: &[FaceBox]) -> Result<Vec<Vec<u8>>, String> {
    fingerprint::read_unchanged(&src.path, src.size, src.mtime_ns, || {
        let img = media::decode_image(lib_heif, src.kind, &src.path, recognize::EDGE).map_err(|e| format!("{e:#}"))?;
        let img = thumbs::shrink(img, recognize::EDGE);
        boxes.iter().map(|b| media::encode_jpeg(&crop(&img, b), CROP_QUALITY).map_err(|e| format!("{e:#}"))).collect()
    })
}

/// A square around the face with some room, kept inside the picture,
/// turned upright and shrunk to `CROP_EDGE`.
fn crop(img: &DynamicImage, b: &FaceBox) -> DynamicImage {
    let (iw, ih) = (img.width() as f64, img.height() as f64);
    let (fw, fh) = (b.w * iw, b.h * ih);
    let side = (fw.max(fh) * (1.0 + 2.0 * CROP_MARGIN)).min(iw).min(ih).max(1.0);
    let cx = (b.x + b.w / 2.0) * iw;
    let cy = (b.y + b.h / 2.0) * ih;
    let x0 = (cx - side / 2.0).clamp(0.0, (iw - side).max(0.0));
    let y0 = (cy - side / 2.0).clamp(0.0, (ih - side).max(0.0));
    let s = side.round().max(1.0) as u32;
    let mut c = img.crop_imm(x0.round() as u32, y0.round() as u32, s, s);
    c = match b.roll {
        90 => c.rotate90(),
        180 => c.rotate180(),
        270 => c.rotate270(),
        _ => c,
    };
    thumbs::shrink(c, CROP_EDGE)
}

/// Replace the crops stored for a content (or store why there are none).
pub fn store_crops(conn: &Connection, key: &str, boxes: &[FaceBox], result: &Result<Vec<Vec<u8>>, String>) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM thumbs.faces WHERE key = ?1", [key])?;
    {
        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO thumbs.faces (key, x, y, w, h, jpeg, error, made_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (i, b) in boxes.iter().enumerate() {
            let (jpeg, error) = match result {
                Ok(crops) => (crops.get(i), None),
                Err(e) => (None, Some(e.as_str())),
            };
            insert.execute(params![key, b.x, b.y, b.w, b.h, jpeg, error, db::now()])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_land_in_their_bucket() {
        let mut b = buckets(&WIDTH_BUCKETS);
        for v in [10.0, 29.9, 30.0, 39.0, 40.0, 59.0, 60.0, 119.0, 120.0, 900.0] {
            count_into(&mut b, v, v < 20.0);
        }
        assert_eq!(b.iter().map(|b| b.count).collect::<Vec<_>>(), [2, 2, 2, 2, 2]);
        assert_eq!(b.iter().map(|b| b.not_face).collect::<Vec<_>>(), [1, 0, 0, 0, 0]);
        assert_eq!(bucket_label(&b[0], " px", 0), "under 30 px");
        assert_eq!(bucket_label(&b[2], " px", 0), "40–60 px");
        assert_eq!(bucket_label(&b[4], " px", 0), "120 px and more");
        assert!(too_small(29.9) && !too_small(30.0));
    }

    #[test]
    fn crops_stay_inside_and_turn_upright() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(400, 200));
        let c = crop(&img, &FaceBox { x: 0.9, y: 0.0, w: 0.1, h: 0.2, roll: 0 });
        assert_eq!((c.width(), c.height()), (60, 60));
        // Bigger than the picture allows: the picture's shorter side.
        let c = crop(&img, &FaceBox { x: 0.0, y: 0.0, w: 1.0, h: 1.0, roll: 90 });
        assert_eq!((c.width(), c.height()), (CROP_EDGE, CROP_EDGE));
    }
}
