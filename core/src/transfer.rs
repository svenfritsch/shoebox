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
//! Two halves:
//!
//! 1. **Read** a [`TransferSet`] from a source: [`read_drive`] opens the other
//!    drive's `library.db` read-only and reads only the tables of the five
//!    boxes; [`read_userdata`] (15b) reads a `userdata.json`, the file every
//!    drive keeps next to its index, for a drive that is gone, erased or lost
//!    its index.
//! 2. **Apply** a `TransferSet` to the target for the ticked [`Boxes`]: matching,
//!    then inserts in **one transaction** on the target's `library.db`
//!    ([`apply`]). Carry over only adds: what the target already has wins, and a
//!    second run adds nothing ([`plan`] counts without writing).
//!
//! Nothing is written to the source, and no photo is read or written, so no
//! guard is needed (a file source must be a `.json`, which is never a photo). Never carried: thumbnails and recognition results (the
//! target builds them), decisions about duplicates inside the source drive
//! (they refer to its file ids), the source's trash, named places (about the
//! drive, not a photo) and the "other marks" listed in `NOT_TRANSFERRED_TABLES`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::{dates, db, geo, library, people, pets, recognize, tags};

/// Tables of `library.db` whose rows are carried (one box each, `people` with
/// `face_decisions`). A new table must be in this list or in
/// `NOT_TRANSFERRED_TABLES`: `core/tests/transfer.rs` fails otherwise, so a new kind
/// of user data is decided about on purpose.
pub const TRANSFERRED_TABLES: &[&str] = &["file_tags", "face_decisions", "people", "taken_overrides", "date_estimates", "geo_overrides", "tags", "hidden_folders"];

/// Tables that are not carried, and why: the drive's own bookkeeping, things
/// the target builds itself, things about the drive and not a photo, and the
/// "other marks" (`view_turns`, `shot_marks`, `text_hidden`, `face_species`):
/// the same kind of content-keyed rows, not offered yet. A sixth box adds one
/// of them to `TRANSFERRED_TABLES`.
pub const NOT_TRANSFERRED_TABLES: &[&str] = &[
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
    /// Hidden photos and folders (phase 20). Not ticked unless asked for: what to hide
    /// is decided per drive.
    #[serde(default)]
    pub hidden: bool,
}

fn yes() -> bool {
    true
}

impl Boxes {
    /// Every kind, hidden photos too (what a plan counts, for the check boxes).
    pub fn all() -> Boxes {
        Boxes { tags: true, hearts: true, people: true, dates: true, places: true, hidden: true }
    }
}

/// The boxes ticked when none are given: all but the hidden photos.
impl Default for Boxes {
    fn default() -> Boxes {
        Boxes { hidden: false, ..Boxes::all() }
    }
}

/// A photo's content as the source knows it. The target's file is the same
/// photo when the full hashes are equal, or, if either has none, when size and
/// quick hash are equal. `userdata.json` has no sizes; the quick hash covers
/// the size anyway.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Content {
    pub quick: String,
    pub size: Option<u64>,
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
pub struct TransferSet {
    /// The source's files by quick hash: what the content-keyed items below
    /// (decisions, dates, places) are matched by. Only keys that have an item.
    pub contents: HashMap<String, Vec<Content>>,
    /// Own tags, except the heart.
    pub tags: Vec<TagItem>,
    /// The contents with a heart.
    pub hearts: Vec<Content>,
    /// The contents hidden from the timeline by the own tag `hidden` (phase 20).
    pub hidden: Vec<Content>,
    /// The folders hidden from the timeline (NFC paths); matched by path, the target
    /// takes those it has.
    pub hidden_folders: Vec<String>,
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
    /// Photos that get the hidden tag, and folders that get the hidden flag.
    pub hidden: u64,
    pub hidden_folders: u64,
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
        self.photos == 0 && self.hidden_folders == 0
    }
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |r| r.get::<_, i64>(0))? > 0)
}

/// Read what a drive's `library.db` holds, read-only. The file is never
/// changed (and the drive may be in use by its own server meanwhile).
pub fn read_drive(db_path: &Path) -> Result<TransferSet> {
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

/// Read a `TransferSet` from an open `library.db` (read access is enough).
pub fn read(conn: &Connection) -> Result<TransferSet> {
    let mut set = TransferSet::default();

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
        let mut hidden: HashSet<Content> = HashSet::new();
        while let Some(r) = rows.next()? {
            let content = Content { quick: r.get(0)?, size: Some(r.get::<_, i64>(1)? as u64), full: r.get(2)? };
            let name: String = r.get(3)?;
            if tags::means_favorite(&name) {
                if hearted.insert(content.clone()) {
                    set.hearts.push(content);
                }
            } else if tags::is_hidden_name(&name) {
                if hidden.insert(content.clone()) {
                    set.hidden.push(content);
                }
            } else {
                set.tags.push(TagItem { content, name, at: r.get(4)? });
            }
        }
    }
    // Folders hidden from the timeline (an older index has none).
    set.hidden_folders = tags::hidden_folder_paths(conn)?;

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
            let content = Content { quick: quick.clone(), size: Some(r.get::<_, i64>(1)? as u64), full: r.get(2)? };
            let list = set.contents.entry(quick).or_default();
            if !list.contains(&content) {
                list.push(content);
            }
        }
    }
    Ok(set)
}

// ---------------------------------------------------------------- userdata.json

/// The largest `userdata.json` read. A drive with 100,000 tagged photos writes
/// some tens of MB.
pub const MAX_USERDATA_BYTES: u64 = 256 * 1024 * 1024;

/// What the dialog says about a file source.
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    /// Format version (1 to `tags::USERDATA_VERSION`).
    pub version: u32,
    /// When shoebox wrote it (Unix seconds).
    pub written_at: i64,
}

// The parts of `userdata.json` that are read. Every list may be missing (an
// older version), and anything else in the file is ignored.

#[derive(Deserialize)]
struct UdFile {
    shoebox: String,
    version: u32,
    #[serde(default)]
    written_at: i64,
    #[serde(default)]
    own_tags: Vec<UdTag>,
    #[serde(default)]
    face_decisions: Vec<UdDecision>,
    #[serde(default)]
    taken_overrides: Vec<UdTaken>,
    #[serde(default)]
    date_estimates: Vec<UdEstimate>,
    #[serde(default)]
    geo_overrides: Vec<UdGeo>,
    #[serde(default)]
    hidden_folders: Vec<String>,
}

#[derive(Deserialize)]
struct UdTag {
    name: String,
    #[serde(default)]
    files: Vec<UdTagged>,
}

#[derive(Deserialize)]
struct UdTagged {
    #[serde(default)]
    quick_hash: Option<String>,
    #[serde(default)]
    full_hash: Option<String>,
    #[serde(default)]
    in_trash: bool,
}

#[derive(Deserialize)]
struct UdDecision {
    #[serde(default)]
    person: Option<String>,
    decision: String,
    #[serde(default)]
    manual: bool,
    key: String,
    #[serde(rename = "box")]
    b: [f64; 4],
    #[serde(default)]
    at: i64,
    #[serde(default)]
    species: Option<String>,
    #[serde(default)]
    files: Vec<UdDecisionFile>,
}

#[derive(Deserialize)]
struct UdDecisionFile {
    #[serde(default)]
    full_hash: Option<String>,
    #[serde(default)]
    in_trash: bool,
}

#[derive(Deserialize)]
struct UdTaken {
    quick_hash: String,
    taken: String,
    #[serde(default)]
    taken_offset: Option<String>,
}

#[derive(Deserialize)]
struct UdEstimate {
    quick_hash: String,
    year: i32,
    #[serde(default)]
    month: Option<u32>,
    #[serde(default)]
    day: Option<u32>,
}

#[derive(Deserialize)]
struct UdGeo {
    quick_hash: String,
    lat: f64,
    lon: f64,
}

/// A hash as shoebox writes it: 64 hex digits (BLAKE3).
fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `+02:00`, `-05:30` (an offset as the scan stores it).
fn is_offset(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 6 && (b[0] == b'+' || b[0] == b'-') && b[1..3].iter().all(u8::is_ascii_digit) && b[3] == b':' && b[4..6].iter().all(u8::is_ascii_digit)
}

/// A face box: fractions of the picture.
fn is_box(b: &[f64; 4]) -> bool {
    b.iter().all(|v| v.is_finite() && (-0.5..=1.5).contains(v)) && b[2] > 0.0 && b[3] > 0.0
}

/// Read a `userdata.json` (versions 1 to `tags::USERDATA_VERSION`): the file
/// a drive keeps next to its index, or a copy of it. Only files called
/// `….json` are opened, so this never reads a photo. The file comes from
/// anywhere, so every entry is checked as if typed; what does not pass is
/// left out.
pub fn read_userdata(path: &Path) -> Result<(TransferSet, FileInfo)> {
    let is_json = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if !is_json {
        bail!("choose a userdata.json file");
    }
    let meta = std::fs::metadata(path).with_context(|| format!("{} cannot be opened", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a file", path.display());
    }
    if meta.len() > MAX_USERDATA_BYTES {
        bail!("{} is too large for a userdata.json", path.display());
    }
    let bytes = std::fs::read(path).with_context(|| format!("{} cannot be read", path.display()))?;
    parse_userdata(&bytes)
}

/// [`read_userdata`] for the file's contents.
pub fn parse_userdata(bytes: &[u8]) -> Result<(TransferSet, FileInfo)> {
    let file: UdFile = serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("this is not a userdata.json written by shoebox"))?;
    if file.shoebox.is_empty() || file.version == 0 {
        bail!("this is not a userdata.json written by shoebox");
    }
    if file.version > tags::USERDATA_VERSION {
        bail!("this userdata.json was written by a newer shoebox (version {}): use that version", file.shoebox);
    }
    let at = file.written_at.max(0);
    let mut set = TransferSet::default();
    // The full hashes known for a quick hash: what the content-keyed items
    // are matched by, as the source's index would have it.
    let mut fulls: HashMap<String, Vec<String>> = HashMap::new();
    let mut know = |quick: &str, full: &Option<String>| {
        if let Some(full) = full.as_deref().filter(|f| is_hash(f)) {
            let list = fulls.entry(quick.to_string()).or_default();
            if !list.iter().any(|f| f == full) {
                list.push(full.to_string());
            }
        }
    };

    // Own tags and hearts. Files in the source's trash are left out, as on a
    // drive (their tags are kept by its trash, not carried).
    let mut hearted: HashSet<Content> = HashSet::new();
    let mut hidden: HashSet<Content> = HashSet::new();
    for tag in &file.own_tags {
        let Ok(name) = tags::check_name(&tag.name) else { continue };
        let heart = tags::means_favorite(&name);
        let hide = tags::is_hidden_name(&name);
        for f in &tag.files {
            let Some(quick) = f.quick_hash.as_deref().filter(|q| is_hash(q)) else { continue };
            if f.in_trash {
                continue;
            }
            know(quick, &f.full_hash);
            let content = Content { quick: quick.to_string(), size: None, full: f.full_hash.clone().filter(|h| is_hash(h)) };
            if heart {
                if hearted.insert(content.clone()) {
                    set.hearts.push(content);
                }
            } else if hide {
                if hidden.insert(content.clone()) {
                    set.hidden.push(content);
                }
            } else {
                set.tags.push(TagItem { content, name: name.clone(), at: None });
            }
        }
    }
    // Hidden folders: checked like typed input, and only matched by path.
    for path in &file.hidden_folders {
        let path = library::nfc(path.trim().trim_matches('/'));
        if !path.is_empty() && !path.chars().any(char::is_control) && path.chars().count() <= 4096 && !set.hidden_folders.contains(&path) {
            set.hidden_folders.push(path);
        }
    }

    // Face decisions; a decision whose photo is only in the source's trash is
    // left out.
    for d in &file.face_decisions {
        if !is_hash(&d.key) || !is_box(&d.b) {
            continue;
        }
        if !d.files.is_empty() && d.files.iter().all(|f| f.in_trash) {
            continue;
        }
        let decision = match serde_json::from_value::<people::Decision>(serde_json::Value::String(d.decision.clone())) {
            Ok(decision) => decision,
            Err(_) => continue,
        };
        let person = match (&d.person, decision) {
            // These two never name a person.
            (_, people::Decision::Ignored | people::Decision::NotFace) => None,
            (Some(name), _) => match people::check_name(name) {
                Ok(name) => Some(name),
                Err(_) => continue,
            },
            (None, _) => None,
        };
        if d.species.as_deref().is_some_and(|s| !pets::is_species(s)) {
            continue;
        }
        for f in d.files.iter().filter(|f| !f.in_trash) {
            know(&d.key, &f.full_hash);
        }
        set.decisions.push(DecisionItem {
            key: d.key.clone(),
            b: d.b,
            person,
            decision: decision.as_str().to_string(),
            manual: d.manual,
            at: if d.at > 0 { d.at } else { at },
            species: d.species.clone(),
        });
    }

    // Dates: a corrected capture date and an estimate, by content.
    let mut dated: HashMap<String, DateItem> = HashMap::new();
    for t in &file.taken_overrides {
        let valid = chrono::NaiveDateTime::parse_from_str(&t.taken, "%Y-%m-%dT%H:%M:%S").is_ok();
        if !is_hash(&t.quick_hash) || !valid || t.taken_offset.as_deref().is_some_and(|o| !is_offset(o)) {
            continue;
        }
        let item = dated.entry(t.quick_hash.clone()).or_insert_with(|| DateItem { key: t.quick_hash.clone(), taken: None, estimate: None, at });
        item.taken = Some((t.taken.clone(), t.taken_offset.clone()));
    }
    let today = dates::today();
    for e in &file.date_estimates {
        let estimate = dates::Estimate { year: e.year, month: e.month, day: e.day };
        if !is_hash(&e.quick_hash) || estimate.checked(today).is_err() {
            continue;
        }
        let item = dated.entry(e.quick_hash.clone()).or_insert_with(|| DateItem { key: e.quick_hash.clone(), taken: None, estimate: None, at });
        item.estimate = Some((e.year as i64, e.month.map(i64::from), e.day.map(i64::from)));
    }
    let mut list: Vec<DateItem> = dated.into_values().collect();
    list.sort_by(|a, b| a.key.cmp(&b.key));
    set.dates = list;

    // Positions given by hand.
    for g in &file.geo_overrides {
        if !is_hash(&g.quick_hash) || geo::check_position(g.lat, g.lon).is_err() {
            continue;
        }
        set.places.push(PlaceItem { key: g.quick_hash.clone(), lat: g.lat, lon: g.lon, at });
    }

    // What the content-keyed items match: the full hashes the file knows for
    // their quick hash, or the quick hash alone (which covers the size).
    let keys: HashSet<String> = set
        .decisions
        .iter()
        .map(|d| d.key.clone())
        .chain(set.dates.iter().map(|d| d.key.clone()))
        .chain(set.places.iter().map(|p| p.key.clone()))
        .collect();
    for key in keys {
        let contents = match fulls.get(&key) {
            Some(list) => list.iter().map(|f| Content { quick: key.clone(), size: None, full: Some(f.clone()) }).collect(),
            None => vec![Content { quick: key.clone(), size: None, full: None }],
        };
        set.contents.insert(key, contents);
    }
    Ok((set, FileInfo { version: file.version, written_at: file.written_at }))
}

struct TargetFile {
    id: i64,
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
        by_quick.entry(quick).or_default().push(files.len());
        files.push(TargetFile { id: r.get(0)?, size: r.get::<_, i64>(2)? as u64, full: r.get(3)?, has_position: r.get::<_, i64>(4)? != 0 });
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
            c.size.is_none_or(|size| size == f.size)
                && match (&c.full, &f.full) {
                    (Some(a), Some(b)) => a == b,
                    // No full hash yet on one side: size and quick hash decide.
                    _ => true,
                }
        })
        .collect()
}

/// The target files that match any of the contents with this key.
fn matching_key(target: &Target, set: &TransferSet, key: &str) -> Vec<usize> {
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
pub fn plan(conn: &Connection, set: &TransferSet, boxes: Boxes) -> Result<Counts> {
    run(conn, set, boxes, false, &mut |_| Ok(()))
}

/// Carry `set` over to this drive: all ticked boxes in one transaction, or
/// nothing.
pub fn apply(conn: &Connection, set: &TransferSet, boxes: Boxes) -> Result<Counts> {
    apply_with(conn, set, boxes, &mut |_| Ok(()))
}

/// Like [`apply`]; `step` is called with the number of rows written so far
/// after every row, and an error from it undoes everything (the tests use it
/// to prove that a failure half way leaves the target unchanged).
pub fn apply_with(conn: &Connection, set: &TransferSet, boxes: Boxes, step: &mut dyn FnMut(u64) -> Result<()>) -> Result<Counts> {
    let tx = conn.unchecked_transaction()?;
    let counts = run(&tx, set, boxes, true, step)?;
    tx.commit()?;
    Ok(counts)
}

fn run(conn: &Connection, set: &TransferSet, boxes: Boxes, write: bool, step: &mut dyn FnMut(u64) -> Result<()>) -> Result<Counts> {
    let target = load_target(conn)?;
    let mut counts = Counts::default();
    let mut touched: HashSet<i64> = HashSet::new();
    let mut written = 0u64;

    // Tags and hearts: per file, added when the file does not have them.
    if boxes.tags || boxes.hearts || boxes.hidden {
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
        let mut hid: HashSet<i64> = HashSet::new();
        if boxes.hidden {
            for content in &set.hidden {
                for i in matching(&target, content) {
                    let file = target.files[i].id;
                    if give_tag(conn, write, &mut have, &mut ids, file, tags::HIDDEN, None)? {
                        hid.insert(file);
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
        counts.hidden = hid.len() as u64;
    }

    // Hidden folders: by path, for the folders this drive has (and not hidden yet).
    if boxes.hidden && !set.hidden_folders.is_empty() {
        let mut have: HashSet<String> = tags::hidden_folder_paths(conn)?.into_iter().collect();
        let mut folders: HashSet<String> = HashSet::new();
        {
            let mut stmt = conn.prepare("SELECT path_nfc FROM folders")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                folders.insert(r.get(0)?);
            }
        }
        for path in &set.hidden_folders {
            if !folders.contains(path) || !have.insert(path.clone()) {
                continue;
            }
            if write {
                conn.execute("INSERT OR IGNORE INTO hidden_folders (path_nfc, added_at) VALUES (?1, ?2)", params![path, db::now()])?;
                written += 1;
                step(written)?;
            }
            counts.hidden_folders += 1;
        }
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
                        // Plain SQL: `people::create_person` reads the faces
                        // back from the recognition database, which a bare
                        // connection to library.db does not have attached.
                        let Ok(name) = people::check_name(name) else { continue };
                        conn.execute("INSERT INTO people (name) VALUES (?1)", [&name])?;
                        let id = conn.last_insert_rowid();
                        person_ids.insert(fold, id);
                        counts.new_people += 1;
                        Some(id)
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
