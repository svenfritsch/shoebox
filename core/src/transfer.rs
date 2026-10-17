//! Carry over data between drives (phase 15a, docs/phase15.md).
//!
//! What a person made in shoebox (own tags, hearts, people and what was decided
//! about faces, corrected and estimated dates, positions given by hand) lives in
//! the `library.db` of the drive where it was made. When the same photos are
//! copied to another drive in the file manager, that data stays behind. This
//! module brings it to the same photos on another drive, matched **by content**
//! (the full hash; the size and quick hash while a full hash is missing), never
//! by name or folder.
//!
//! Two halves, so a later reader of `userdata.json` (15b) can reuse the second:
//!
//! 1. **Read** a [`CarrySet`] from a source: [`read_drive`] opens the other
//!    drive's `library.db` read-only and reads only the tables of the five
//!    boxes.
//! 2. **Apply** a `CarrySet` to the target for the ticked [`Boxes`]: matching,
//!    then inserts in **one transaction** on the target's `library.db`
//!    ([`apply`]). Carry over only adds: what the target already has wins, and a
//!    second run adds nothing ([`plan`] counts without writing).
//!
//! Nothing is written to the source, and no photo is read or written, so no
//! guard is needed. Never carried: thumbnails and recognition results (the
//! target builds them), decisions about duplicates inside the source drive
//! (they refer to its file ids), the source's trash, named places (about the
//! drive, not a photo) and the "other marks" listed in `NOT_CARRIED_TABLES`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::{db, people, recognize, tags};

/// Tables of `library.db` whose rows are carried (one box each, `people` with
/// `face_decisions`). A new table must be in this list or in
/// `NOT_CARRIED_TABLES`: `core/tests/transfer.rs` fails otherwise, so a new kind
/// of user data is decided about on purpose.
pub const CARRIED_TABLES: &[&str] = &["file_tags", "face_decisions", "people", "taken_overrides", "date_estimates", "geo_overrides", "tags"];

/// Tables that are not carried, and why: the drive's own bookkeeping, things
/// the target builds itself, things about the drive and not a photo, and the
/// "other marks" (`view_turns`, `shot_marks`, `text_hidden`, `face_species`):
/// the same kind of content-keyed rows, not offered yet. A sixth box adds one
/// of them to `CARRIED_TABLES`.
pub const NOT_CARRIED_TABLES: &[&str] = &[
    "folders",
    "files",
    "jobs",
    "dup_decisions",
    "trash",
    "groups",
    "settings",
    "removed_copies",
    "places",
    "view_turns",
    "shot_marks",
    "text_hidden",
    "face_species",
];

/// Which kinds of data to carry; the dialog's check boxes (all ticked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boxes {
    #[serde(default = "yes")]
    pub tags: bool,
    #[serde(default = "yes")]
    pub hearts: bool,
    #[serde(default = "yes")]
    pub people: bool,
    #[serde(default = "yes")]
    pub dates: bool,
    #[serde(default = "yes")]
    pub places: bool,
}

fn yes() -> bool {
    true
}

impl Boxes {
    pub fn all() -> Boxes {
        Boxes { tags: true, hearts: true, people: true, dates: true, places: true }
    }
}

impl Default for Boxes {
    fn default() -> Boxes {
        Boxes::all()
    }
}

/// A photo's content as the source's index knows it. The target's file is the
/// same photo when the full hashes are equal, or, if either has none yet, when
/// size and quick hash are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Content {
    pub quick: String,
    pub size: u64,
    pub full: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TagItem {
    pub content: Content,
    pub name: String,
    /// When the tag was given (Unix seconds), if the source recorded it.
    pub at: Option<i64>,
}

/// What the user decided about one face, with the person by name.
#[derive(Debug, Clone)]
pub struct DecisionItem {
    /// The content's quick hash, as `face_decisions.key`.
    pub key: String,
    pub b: [f64; 4],
    pub person: Option<String>,
    pub decision: String,
    pub manual: bool,
    pub at: i64,
    pub species: Option<String>,
}

/// A date the user gave a content: a corrected capture date (`taken`, with its
/// offset) and/or an estimate (year, month, day).
#[derive(Debug, Clone)]
pub struct DateItem {
    pub key: String,
    pub taken: Option<(String, Option<String>)>,
    pub estimate: Option<(i64, Option<i64>, Option<i64>)>,
    pub at: i64,
}

#[derive(Debug, Clone)]
pub struct PlaceItem {
    pub key: String,
    pub lat: f64,
    pub lon: f64,
    pub at: i64,
}

/// Everything of the five boxes that a source holds.
#[derive(Debug, Clone, Default)]
pub struct CarrySet {
    /// The source's files by quick hash: what the content-keyed items below
    /// (decisions, dates, places) are matched by. Only keys that have an item.
    pub contents: HashMap<String, Vec<Content>>,
    /// Own tags, except the heart.
    pub tags: Vec<TagItem>,
    /// The contents with a heart.
    pub hearts: Vec<Content>,
    pub decisions: Vec<DecisionItem>,
    pub dates: Vec<DateItem>,
    pub places: Vec<PlaceItem>,
}

/// What a carry over does (or, for a plan, would do) on the target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Photos that get an own tag, photos that get a heart.
    pub tags: u64,
    pub hearts: u64,
    /// Faces that get a decision, and people created for them.
    pub people: u64,
    pub new_people: u64,
    /// Contents that get a corrected or estimated date, or a position.
    pub dates: u64,
    pub places: u64,
    /// Photos of the target that get something, of any kind.
    pub photos: u64,
}

impl Counts {
    pub fn is_nothing(&self) -> bool {
        self.photos == 0
    }
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |r| r.get::<_, i64>(0))? > 0)
}

/// Read what a drive's `library.db` holds, read-only. The file is never
/// changed (and the drive may be in use by its own server meanwhile).
pub fn read_drive(db_path: &Path) -> Result<CarrySet> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags).with_context(|| format!("open {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    read(&conn)
}

/// How many files a drive's index lists, and how many of them are missing
/// (gone from the drive since the last scan). More than half missing means the
/// drive was emptied: carry over *before* wiping it.
pub fn looks_emptied(db_path: &Path) -> Result<bool> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags).with_context(|| format!("open {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let (total, missing): (i64, i64) =
        conn.query_row("SELECT count(*), coalesce(sum(missing_since IS NOT NULL), 0) FROM files", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(total > 0 && missing * 2 > total)
}

/// Read a `CarrySet` from an open `library.db` (read access is enough).
pub fn read(conn: &Connection) -> Result<CarrySet> {
    let mut set = CarrySet::default();

    // Own tags and hearts: by the content of the file that carries them. The
    // source's missing files count (their records are kept until forgotten).
    {
        // An index from before the "given at" column is read all the same.
        let added = if db::has_column(conn, "main", "file_tags", "added_at")? { "ft.added_at" } else { "NULL" };
        let mut stmt = conn.prepare(&format!(
            "SELECT f.quick_hash, f.size, f.full_hash, t.name, {added}
             FROM file_tags ft JOIN tags t ON t.id = ft.tag_id JOIN files f ON f.id = ft.file_id
             WHERE ft.source = 'user' ORDER BY f.path_nfc, t.id"
        ))?;
        let mut rows = stmt.query([])?;
        let mut hearted: HashSet<Content> = HashSet::new();
        while let Some(r) = rows.next()? {
            let content = Content { quick: r.get(0)?, size: r.get::<_, i64>(1)? as u64, full: r.get(2)? };
            let name: String = r.get(3)?;
            if tags::means_favorite(&name) {
                if hearted.insert(content.clone()) {
                    set.hearts.push(content);
                }
            } else {
                set.tags.push(TagItem { content, name, at: r.get(4)? });
            }
        }
    }

    // What is keyed by content (the quick hash): decisions, dates, positions.
    let mut keys: HashSet<String> = HashSet::new();
    if has_table(conn, "face_decisions")? {
        // Before schema v7 a decision has no species: all are people's.
        let species = if db::has_column(conn, "main", "face_decisions", "species")? { "d.species" } else { "NULL" };
        let mut stmt = conn.prepare(&format!(
            "SELECT d.key, d.x, d.y, d.w, d.h, p.name, d.decision, d.manual, d.at, {species}
             FROM face_decisions d LEFT JOIN people p ON p.id = d.person_id ORDER BY d.id"
        ))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            keys.insert(key.clone());
            set.decisions.push(DecisionItem {
                key,
                b: [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?],
                person: r.get(5)?,
                decision: r.get(6)?,
                manual: r.get::<_, i64>(7)? != 0,
                at: r.get(8)?,
                species: r.get(9)?,
            });
        }
    }
    {
        let mut dates: HashMap<String, DateItem> = HashMap::new();
        if has_table(conn, "taken_overrides")? {
            let mut stmt = conn.prepare("SELECT key, taken, taken_offset, at FROM taken_overrides ORDER BY key")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let key: String = r.get(0)?;
                let item = dates.entry(key.clone()).or_insert_with(|| DateItem { key, taken: None, estimate: None, at: 0 });
                item.taken = Some((r.get(1)?, r.get(2)?));
                item.at = item.at.max(r.get(3)?);
            }
        }
        if has_table(conn, "date_estimates")? {
            let mut stmt = conn.prepare("SELECT key, year, month, day, at FROM date_estimates ORDER BY key")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let key: String = r.get(0)?;
                let item = dates.entry(key.clone()).or_insert_with(|| DateItem { key, taken: None, estimate: None, at: 0 });
                item.estimate = Some((r.get(1)?, r.get(2)?, r.get(3)?));
                item.at = item.at.max(r.get(4)?);
            }
        }
        let mut list: Vec<DateItem> = dates.into_values().collect();
        list.sort_by(|a, b| a.key.cmp(&b.key));
        keys.extend(list.iter().map(|d| d.key.clone()));
        set.dates = list;
    }
    if has_table(conn, "geo_overrides")? {
        let mut stmt = conn.prepare("SELECT key, lat, lon, at FROM geo_overrides ORDER BY key")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            keys.insert(key.clone());
            set.places.push(PlaceItem { key, lat: r.get(1)?, lon: r.get(2)?, at: r.get(3)? });
        }
    }
    // One pass over the files, keeping only the contents some item refers to.
    if !keys.is_empty() {
        let mut stmt = conn.prepare("SELECT quick_hash, size, full_hash FROM files")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let quick: String = r.get(0)?;
            if !keys.contains(&quick) {
                continue;
            }
            let content = Content { quick: quick.clone(), size: r.get::<_, i64>(1)? as u64, full: r.get(2)? };
            let list = set.contents.entry(quick).or_default();
            if !list.contains(&content) {
                list.push(content);
            }
        }
    }
    Ok(set)
}

struct TargetFile {
    id: i64,
    quick: String,
    size: u64,
    full: Option<String>,
    /// The file itself has a position (EXIF), which wins over one given by hand.
    has_position: bool,
}

struct Target {
    files: Vec<TargetFile>,
    by_quick: HashMap<String, Vec<usize>>,
}

/// The target's present files. Missing ones receive nothing.
fn load_target(conn: &Connection) -> Result<Target> {
    let mut files = Vec::new();
    let mut by_quick: HashMap<String, Vec<usize>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, quick_hash, size, full_hash, lat IS NOT NULL AND lon IS NOT NULL FROM files WHERE missing_since IS NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let quick: String = r.get(1)?;
        by_quick.entry(quick.clone()).or_default().push(files.len());
        files.push(TargetFile { id: r.get(0)?, quick, size: r.get::<_, i64>(2)? as u64, full: r.get(3)?, has_position: r.get::<_, i64>(4)? != 0 });
    }
    Ok(Target { files, by_quick })
}

/// The target files that are the same photo as `c`.
fn matching(target: &Target, c: &Content) -> Vec<usize> {
    target
        .by_quick
        .get(&c.quick)
        .into_iter()
        .flatten()
        .copied()
        .filter(|&i| {
            let f = &target.files[i];
            f.size == c.size
                && match (&c.full, &f.full) {
                    (Some(a), Some(b)) => a == b,
                    // No full hash yet on one side: size and quick hash decide.
                    _ => true,
                }
        })
        .collect()
}

/// The target files that match any of the contents with this key.
fn matching_key(target: &Target, set: &CarrySet, key: &str) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for c in set.contents.get(key).into_iter().flatten() {
        for i in matching(target, c) {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

/// Give a file an own tag unless it has it (in any spelling). Returns whether
/// it was new. Without `write` nothing is stored, but the answer is the same.
fn give_tag(
    conn: &Connection,
    write: bool,
    have: &mut HashMap<i64, HashSet<String>>,
    ids: &mut HashMap<String, i64>,
    file: i64,
    name: &str,
    at: Option<i64>,
) -> Result<bool> {
    let fold = db::tag_fold(name);
    if !have.entry(file).or_default().insert(fold.clone()) {
        return Ok(false);
    }
    if write {
        let known = ids.get(&fold).copied();
        let tag = match known {
            Some(t) => t,
            None => {
                let t = db::own_tag_id(conn, name)?;
                ids.insert(fold, t);
                t
            }
        };
        conn.execute("INSERT OR IGNORE INTO file_tags (file_id, tag_id, source, added_at) VALUES (?1, ?2, 'user', ?3)", params![file, tag, at])?;
    }
    Ok(true)
}

/// Count what carrying `set` over would do on this drive, without writing.
pub fn plan(conn: &Connection, set: &CarrySet, boxes: Boxes) -> Result<Counts> {
    run(conn, set, boxes, false, &mut |_| Ok(()))
}

/// Carry `set` over to this drive: all ticked boxes in one transaction, or
/// nothing.
pub fn apply(conn: &Connection, set: &CarrySet, boxes: Boxes) -> Result<Counts> {
    apply_with(conn, set, boxes, &mut |_| Ok(()))
}

/// Like [`apply`]; `step` is called with the number of rows written so far
/// after every row, and an error from it undoes everything (the tests use it
/// to prove that a failure half way leaves the target unchanged).
pub fn apply_with(conn: &Connection, set: &CarrySet, boxes: Boxes, step: &mut dyn FnMut(u64) -> Result<()>) -> Result<Counts> {
    let tx = conn.unchecked_transaction()?;
    let counts = run(&tx, set, boxes, true, step)?;
    tx.commit()?;
    Ok(counts)
}

fn run(conn: &Connection, set: &CarrySet, boxes: Boxes, write: bool, step: &mut dyn FnMut(u64) -> Result<()>) -> Result<Counts> {
    let target = load_target(conn)?;
    let mut counts = Counts::default();
    let mut touched: HashSet<i64> = HashSet::new();
    let mut written = 0u64;

    // Tags and hearts: per file, added when the file does not have them.
    if boxes.tags || boxes.hearts {
        let mut have: HashMap<i64, HashSet<String>> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT ft.file_id, t.fold FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE ft.source = 'user'")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                if let Some(fold) = r.get::<_, Option<String>>(1)? {
                    have.entry(r.get(0)?).or_default().insert(fold);
                }
            }
        }
        let mut ids: HashMap<String, i64> = HashMap::new();
        let mut tagged: HashSet<i64> = HashSet::new();
        let mut hearted: HashSet<i64> = HashSet::new();
        if boxes.tags {
            for item in &set.tags {
                let Ok(name) = tags::check_name(&item.name) else { continue };
                for i in matching(&target, &item.content) {
                    let file = target.files[i].id;
                    if give_tag(conn, write, &mut have, &mut ids, file, &name, item.at)? {
                        tagged.insert(file);
                        touched.insert(file);
                        if write {
                            written += 1;
                            step(written)?;
                        }
                    }
                }
            }
        }
        if boxes.hearts {
            for content in &set.hearts {
                for i in matching(&target, content) {
                    let file = target.files[i].id;
                    if give_tag(conn, write, &mut have, &mut ids, file, tags::FAVORITE, None)? {
                        hearted.insert(file);
                        touched.insert(file);
                        if write {
                            written += 1;
                            step(written)?;
                        }
                    }
                }
            }
        }
        counts.tags = tagged.len() as u64;
        counts.hearts = hearted.len() as u64;
    }

    // People: a decision is copied unless the target has one whose box overlaps
    // it (the rule that matches a detected face with a decision); a person is
    // looked up by name and created, without group and picture, if missing.
    if boxes.people {
        let mut existing: HashMap<String, Vec<([f64; 4], Option<String>)>> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT key, x, y, w, h, species FROM face_decisions")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                existing.entry(r.get(0)?).or_default().push(([r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?], r.get(5)?));
            }
        }
        let mut person_ids: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, name FROM people")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                person_ids.insert(db::tag_fold(&r.get::<_, String>(1)?), r.get(0)?);
            }
        }
        let mut would_create: HashSet<String> = HashSet::new();
        for d in &set.decisions {
            let here = matching_key(&target, set, &d.key);
            if here.is_empty() {
                continue;
            }
            let have = existing.entry(d.key.clone()).or_default();
            if have.iter().any(|(b, species)| *species == d.species && recognize::iou(*b, d.b) >= people::MATCH_IOU) {
                continue;
            }
            let person: Option<i64> = match &d.person {
                None => None,
                Some(name) => {
                    let fold = db::tag_fold(name);
                    if let Some(&id) = person_ids.get(&fold) {
                        Some(id)
                    } else if write {
                        match people::create_person(conn, name, None) {
                            Ok(p) => {
                                person_ids.insert(fold, p.id);
                                counts.new_people += 1;
                                Some(p.id)
                            }
                            Err(_) => continue,
                        }
                    } else {
                        if would_create.insert(fold) {
                            counts.new_people += 1;
                        }
                        None
                    }
                }
            };
            if write {
                conn.execute(
                    "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at, species)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![d.key, d.b[0], d.b[1], d.b[2], d.b[3], person, d.decision, d.manual as i64, d.at, d.species],
                )?;
                written += 1;
                step(written)?;
            }
            have.push((d.b, d.species.clone()));
            counts.people += 1;
            touched.extend(here.iter().map(|&i| target.files[i].id));
        }
    }

    // Dates: only for a content that has no corrected or estimated date here.
    if boxes.dates {
        let mut dated: HashSet<String> = HashSet::new();
        for sql in ["SELECT key FROM taken_overrides", "SELECT key FROM date_estimates"] {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                dated.insert(r.get(0)?);
            }
        }
        for d in &set.dates {
            let here = matching_key(&target, set, &d.key);
            if here.is_empty() || !dated.insert(d.key.clone()) {
                continue;
            }
            if write {
                if let Some((taken, offset)) = &d.taken {
                    conn.execute("INSERT OR IGNORE INTO taken_overrides (key, taken, taken_offset, at) VALUES (?1, ?2, ?3, ?4)", params![d.key, taken, offset, d.at])?;
                }
                if let Some((year, month, day)) = d.estimate {
                    conn.execute("INSERT OR IGNORE INTO date_estimates (key, year, month, day, at) VALUES (?1, ?2, ?3, ?4, ?5)", params![d.key, year, month, day, d.at])?;
                }
                written += 1;
                step(written)?;
            }
            counts.dates += 1;
            touched.extend(here.iter().map(|&i| target.files[i].id));
        }
    }

    // Places: a position given by hand, for a content that has none here (a
    // file with a position of its own keeps it).
    if boxes.places {
        let mut positioned: HashSet<String> = HashSet::new();
        {
            let mut stmt = conn.prepare("SELECT key FROM geo_overrides")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                positioned.insert(r.get(0)?);
            }
        }
        for p in &set.places {
            let here: Vec<usize> = matching_key(&target, set, &p.key).into_iter().filter(|&i| !target.files[i].has_position).collect();
            if here.is_empty() || !positioned.insert(p.key.clone()) {
                continue;
            }
            if write {
                conn.execute("INSERT OR IGNORE INTO geo_overrides (key, lat, lon, at) VALUES (?1, ?2, ?3, ?4)", params![p.key, p.lat, p.lon, p.at])?;
                written += 1;
                step(written)?;
            }
            counts.places += 1;
            touched.extend(here.iter().map(|&i| target.files[i].id));
        }
    }

    counts.photos = touched.len() as u64;
    Ok(counts)
}
