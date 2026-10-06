//! Duplicates: exact (same full hash) and near (perceptual hashes at most
//! `NEAR_BITS` apart: resized, re-encoded or lightly edited copies, bursts).
//!
//! Files are grouped by the pairs between them, so a group may also hold a
//! chain of similar shots. Per group the user decides: different photos
//! (`distinct`), versions of one photo (`linked`), or one of them goes to
//! the trash. Decided pairs are not offered again; ids survive moves.

use std::collections::{HashMap, HashSet};

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db;
use crate::library;
use crate::organize;
use crate::phash;

/// Same threshold as the plan: copies stay within it, different photos land
/// around 32 bits apart.
pub const NEAR_BITS: u32 = 8;

/// "Surely the same photo": at most this many bits apart (the similar
/// groups allow `NEAR_BITS`, which also catches bursts), and the same shape
/// and capture time (see `same_photo`).
pub const SURE_BITS: u32 = 4;

/// Allowed when one of the two lost its capture date (what a messenger
/// strips): heavy recompression moves the hash further. Two undated
/// pictures (screenshots, say) get no such leeway.
pub const SURE_BITS_STRIPPED: u32 = 6;

/// Looser limit for an original and its edit (same folder, same camera
/// number): a portrait blur or a filter moves the hash.
pub const EDIT_BITS: u32 = 20;

pub const DECISIONS: &[&str] = &["distinct", "linked"];

#[derive(Debug, Clone, Serialize)]
pub struct DupFile {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub folder_id: i64,
    pub kind: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken: Option<String>,
    /// Thumbnail URL version (first characters of the quick hash).
    pub version: String,
    /// Files in the group with the same number have identical content.
    pub same: Option<u32>,
    /// Folder tags and own tags (filled in by `add_tags`, after `find`).
    pub tags: Vec<DupTag>,
    /// Files with the same row are the same photo in different quality
    /// (a resized or re-sent copy) or identical copies; the page shows them
    /// side by side with one thumbnail.
    pub row: u32,
    /// Lies in a folder the user named as a place for copies (InDesign's
    /// "Links", say): never the one to keep, and ticked when it is the same
    /// photo as one that lies elsewhere.
    pub in_copies: bool,
    /// Surely the same photo as the best file of its row, only worse (fewer
    /// pixels, or without the metadata the best one has): the id of that
    /// best file, which is the one to keep. The page ticks these.
    pub keeper: Option<i64>,
    /// The file of its row to keep: the best quality, then the one without
    /// a copy's name ("IMG_1 (2)", "IMG_1 - Copy"), then the earliest. The
    /// page ticks the other files of a row.
    pub pick: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DupTag {
    pub name: String,
    /// An own tag (removable); otherwise it comes from the folder.
    pub own: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    /// All files have identical content.
    pub exact: bool,
    /// `identical` (same content), `resolution` (surely the same photo,
    /// differing in size, quality or name), `edited` (an iPhone original
    /// and its "E" edit) or `similar` (different shots that look alike: a
    /// series, repeated clicks).
    pub kind: &'static str,
    pub files: Vec<DupFile>,
}

struct Row {
    file: DupFile,
    added_at: i64,
    full_hash: Option<String>,
    phash: Option<u64>,
}

/// What the search needs from the database, loaded in one go so the
/// comparing can happen without holding the connection.
pub struct Candidates {
    rows: Vec<Row>,
    decided: HashSet<(i64, i64)>,
}

/// Folder names (any level of a path) where copies live, not originals: a
/// packaged InDesign project keeps the photos it uses in a "Links" folder.
/// Kept per library as the setting `dup_copy_folders` (a JSON list).
pub const COPY_FOLDERS_KEY: &str = "dup_copy_folders";

/// Names are compared NFC-normalised and case-sensitively: "Link" and "link"
/// are two names (list both to match both).
fn fold_name(name: &str) -> String {
    library::nfc(name.trim())
}

pub fn copy_folders(conn: &Connection) -> Result<Vec<String>> {
    let names: Vec<String> = db::setting(conn, COPY_FOLDERS_KEY)?.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    Ok(names)
}

pub fn set_copy_folders(conn: &Connection, names: &[String]) -> Result<Vec<String>> {
    let mut clean: Vec<String> = Vec::new();
    for n in names {
        let n = library::nfc(n.trim());
        if n.is_empty() || n.contains('/') || n.chars().any(char::is_control) {
            bail!("a folder name is one name, without “/” (got “{n}”)");
        }
        if !clean.iter().any(|c| fold_name(c) == fold_name(&n)) {
            clean.push(n);
        }
    }
    db::set_setting(conn, COPY_FOLDERS_KEY, (!clean.is_empty()).then(|| serde_json::to_string(&clean).unwrap()).as_deref())?;
    Ok(clean)
}

/// Does a folder of the path (not the file's name) carry one of the names?
fn in_copy_folder(path: &str, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    let folded: Vec<String> = names.iter().map(|n| fold_name(n)).collect();
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts.iter().any(|p| folded.iter().any(|n| *n == fold_name(p)))
}

/// The files in `shown` (what the timeline shows: present, no RAW, no Live
/// Photo videos) and the decisions taken so far.
pub fn load(conn: &Connection, shown: &HashSet<i64>) -> Result<Candidates> {
    let rows: Vec<Row> = conn
        .prepare(
            &format!(
                "SELECT id, name, path_nfc, folder_id, kind, size, width, height, {}, quick_hash, full_hash, phash, added_at
                 FROM files WHERE missing_since IS NULL AND kind != 'raw'",
                db::TAKEN
            ),
        )?
        .query_map([], |r| {
            Ok(Row {
                file: DupFile {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    folder_id: r.get(3)?,
                    kind: r.get(4)?,
                    size: r.get::<_, i64>(5)? as u64,
                    width: r.get(6)?,
                    height: r.get(7)?,
                    taken: r.get(8)?,
                    version: r.get::<_, String>(9)?.chars().take(8).collect(),
                    same: None,
                    tags: Vec::new(),
                    row: 0,
                    keeper: None,
                    in_copies: false,
                    pick: false,
                },
                full_hash: r.get(10)?,
                phash: r.get::<_, Option<String>>(11)?.as_deref().and_then(phash::from_hex),
                added_at: r.get(12)?,
            })
        })?
        .filter(|r| r.as_ref().map_or(true, |r| shown.contains(&r.file.id)))
        .collect::<rusqlite::Result<_>>()?;
    let copy_folders = copy_folders(conn)?;
    let mut rows = rows;
    for r in &mut rows {
        r.file.in_copies = in_copy_folder(&r.file.path, &copy_folders);
    }
    let decided: HashSet<(i64, i64)> = conn
        .prepare("SELECT a, b FROM dup_decisions")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Candidates { rows, decided })
}

/// Words for "copy" in the languages a Finder, Explorer or file manager
/// may speak.
const COPY_WORDS: &[&str] = &["copy", "kopie", "copia", "copie", "kopia", "cópia", "kopi", "копия", "копія"];

/// "copy", "another copy", "3rd copy", "Max's conflicted copy 2024-01-01".
fn is_copy_phrase(inner: &str) -> bool {
    if inner.contains("conflicted copy") {
        return true;
    }
    let inner = inner.strip_prefix("another ").unwrap_or(inner);
    let inner = match inner.split_once(' ') {
        Some((first, rest)) if first.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => inner,
    };
    COPY_WORDS.contains(&inner)
}

/// Take one copy marker off the end of a (lowercase) name, if there is one:
/// Windows "x - Copy", "x - Copy (2)" ("x - Kopie"), macOS "x copy",
/// "x copy 2", "x 2", browsers and Explorer imports "x (1)", "x(1)", GNOME
/// "x (copy)", "x (another copy)", "x (3rd copy)", Dropbox "x (Name's
/// conflicted copy 2024-01-01)", and "x-1", "x_2" (Image Capture and
/// friends).
fn strip_copy_marker(l: &str) -> Option<String> {
    let l = l.trim_end();
    if let Some(open) = l.strip_suffix(')').and_then(|r| r.rfind('(').map(|i| (r, i))) {
        let (r, i) = open;
        let inner = r[i + 1..].trim();
        let numbered = !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit());
        let word = is_copy_phrase(inner);
        if numbered || word {
            let base = r[..i].trim_end();
            return (!base.is_empty()).then(|| base.to_string());
        }
    }
    for w in COPY_WORDS {
        if let Some(rest) = l.strip_suffix(w) {
            let base = rest.trim_end_matches([' ', '-', '_']);
            if !base.is_empty() && base.len() < rest.len() {
                return Some(base.to_string());
            }
        }
    }
    let digits = l.len() - l.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 && digits < l.len() {
        let rest = &l[..l.len() - digits];
        if let Some(base) = rest.strip_suffix([' ', '-', '_']) {
            let base = base.trim_end();
            return (!base.is_empty()).then(|| base.to_string());
        }
    }
    None
}

/// Which of these file names are a copy's name of another one in the list
/// ("IMG_1 (2)", "IMG_1 - Copy", "IMG_1 copy 2" next to "IMG_1"). The
/// extension does not matter. The one to keep is the one without it.
pub fn copy_named(names: &[&str]) -> Vec<bool> {
    let stem = |n: &str| {
        let stem = n.rsplit_once('.').map_or(n, |(s, _)| s);
        library::nfc(stem).to_lowercase()
    };
    let stems: Vec<String> = names.iter().map(|n| stem(n)).collect();
    let all: HashSet<&str> = stems.iter().map(String::as_str).collect();
    stems
        .iter()
        .map(|s| {
            let mut cur = s.clone();
            for _ in 0..4 {
                match strip_copy_marker(&cur) {
                    Some(base) => {
                        if base != *s && all.contains(base.as_str()) {
                            return true;
                        }
                        cur = base;
                    }
                    None => return false,
                }
            }
            false
        })
        .collect()
}

/// A camera's file name: `IMG_6621.HEIC`, or `IMG_E6621.HEIC` for the
/// edited version an iPhone writes next to the original. Only the first part
/// counts: a copy's suffix after it ("IMG_6621 1", "IMG_6621 (2)",
/// "IMG_6621 - Copy") is ignored, so a copy still carries its shot's number.
/// Returns the lowercase prefix, whether it is the edited one, and the
/// number. Names whose number runs on into something else (a messenger's
/// "IMG-20250726-WA0001", a Pixel's "PXL_20250101_120000123") do not parse.
fn camera_name(name: &str) -> Option<(String, bool, u64)> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let l = library::nfc(stem).to_lowercase();
    let l = l.trim_start_matches('_');
    let letters = l.len() - l.trim_start_matches(|c: char| c.is_ascii_lowercase()).len();
    let (prefix, rest) = l.split_at(letters);
    if !matches!(prefix, "img" | "dsc" | "dscn" | "dscf" | "pxl" | "mvimg" | "p" | "dji" | "gopr" | "sam" | "pict" | "cimg" | "image") {
        return None;
    }
    let rest = rest.strip_prefix(['_', '-', ' ']).unwrap_or(rest);
    let (edited, rest) = match rest.strip_prefix('e') {
        Some(d) if d.starts_with(|c: char| c.is_ascii_digit()) => (true, d),
        _ => (false, rest),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = &rest[digits.len()..];
    // Whatever follows the number must be a copy's suffix, not more name.
    if !(3..=6).contains(&digits.len()) || !(after.is_empty() || after.starts_with([' ', '('])) {
        return None;
    }
    Some((prefix.to_string(), edited, digits.parse().ok()?))
}

/// Camera names with different numbers are different shots, whatever the
/// pictures look like.
fn names_differ(a: &str, b: &str) -> bool {
    matches!((camera_name(a), camera_name(b)), (Some(x), Some(y)) if x.2 != y.2)
}

/// An original and its iPhone edit ("IMG_6616" and "IMG_E6616").
fn original_and_edit(a: &str, b: &str) -> bool {
    matches!((camera_name(a), camera_name(b)), (Some(x), Some(y)) if x.0 == y.0 && x.2 == y.2 && x.1 != y.1)
}

fn pixels(r: &Row) -> u64 {
    r.file.width.unwrap_or(0) as u64 * r.file.height.unwrap_or(0) as u64
}

fn secs(taken: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(taken, "%Y-%m-%dT%H:%M:%S").ok().map(|d| d.and_utc().timestamp())
}

/// Two pictures that are the same photo for certain, differing in size,
/// quality or metadata only: identical content, or the same picture (hash at
/// most `SURE_BITS` apart) in the same shape (a turned copy counts) and with
/// no capture time that disagrees. Shots of a burst or a re-taken photo fail
/// this: their capture times differ.
fn same_photo(a: &Row, b: &Row) -> bool {
    if a.file.kind == "video" || b.file.kind == "video" {
        return false;
    }
    if a.full_hash.is_some() && a.full_hash == b.full_hash {
        return true;
    }
    // Shots with different camera numbers, and an original next to its edit,
    // are never "the same photo in another size".
    if names_differ(&a.file.name, &b.file.name) || original_and_edit(&a.file.name, &b.file.name) {
        return false;
    }
    let (Some(pa), Some(pb)) = (a.phash, b.phash) else { return false };
    let stripped = a.file.taken.is_some() != b.file.taken.is_some();
    if phash::distance(pa, pb) > if stripped { SURE_BITS_STRIPPED } else { SURE_BITS } {
        return false;
    }
    let shape = |r: &Row| match (r.file.width, r.file.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(w.max(h) as f64 / w.min(h) as f64),
        _ => None,
    };
    let (Some(sa), Some(sb)) = (shape(a), shape(b)) else { return false };
    if (sa - sb).abs() / sa.max(sb) > 0.02 {
        return false;
    }
    // Two files with a capture date, the same size and different bytes are
    // two shots of a series (a burst puts several in one second), not one
    // photo twice: a copy differs in size or has lost its metadata.
    let size = |r: &Row| (r.file.width.unwrap_or(0).max(r.file.height.unwrap_or(0)), r.file.width.unwrap_or(0).min(r.file.height.unwrap_or(0)));
    if a.file.taken.is_some() && b.file.taken.is_some() && size(a) == size(b) {
        return false;
    }
    match (&a.file.taken, &b.file.taken) {
        (Some(x), Some(y)) => matches!((secs(x), secs(y)), (Some(x), Some(y)) if (x - y).abs() <= 2),
        _ => true,
    }
}

/// `a` is better than `b`: more pixels, or as many and the metadata (a
/// capture date) that `b` lost.
fn better(a: &Row, b: &Row) -> bool {
    pixels(a) > pixels(b) || (pixels(a) == pixels(b) && a.file.taken.is_some() && b.file.taken.is_none())
}

/// For the files of one group, in `order`: the row each belongs to, and the
/// best file to keep where it is surely the same photo and worse.
fn photo_rows(rows: &[Row], order: &[usize], open: &dyn Fn(usize, usize) -> bool) -> Vec<(u32, Option<i64>, bool)> {
    let mut sets = UnionFind::new(order.len());
    for (x, &i) in order.iter().enumerate() {
        for (y, &j) in order.iter().enumerate().skip(x + 1) {
            if open(i, j) && same_photo(&rows[i], &rows[j]) {
                sets.union(x, y);
            }
        }
    }
    let mut numbers: HashMap<usize, u32> = HashMap::new();
    let ids: Vec<u32> = (0..order.len())
        .map(|x| {
            let root = sets.find(x);
            let next = numbers.len() as u32;
            *numbers.entry(root).or_insert(next)
        })
        .collect();
    // The best of each component: most pixels, then a capture date, then
    // size, then the earliest record and first path. Sameness is not
    // transitive (a messenger copy without a date fits two shots a day
    // apart), so a file joins the best one's row only if it is the same
    // photo as the best itself; any other gets a row of its own.
    let copy_name: HashMap<usize, bool> = {
        let names: Vec<&str> = order.iter().map(|&i| rows[i].file.name.as_str()).collect();
        order.iter().copied().zip(copy_named(&names)).collect()
    };
    let key = |i: usize| {
        let r = &rows[i];
        (!r.file.in_copies, pixels(r), r.file.taken.is_some(), !copy_name[&i], r.file.size, std::cmp::Reverse(r.added_at), std::cmp::Reverse(r.file.path.clone()))
    };
    let mut best: HashMap<u32, usize> = HashMap::new();
    for (x, &i) in order.iter().enumerate() {
        let e = best.entry(ids[x]).or_insert(i);
        if key(i) > key(*e) {
            *e = i;
        }
    }
    let mut next = numbers.len() as u32;
    order
        .iter()
        .enumerate()
        .map(|(x, &i)| {
            let b = best[&ids[x]];
            if b == i {
                return (ids[x], None, true);
            }
            if open(b, i) && same_photo(&rows[b], &rows[i]) {
                let worse = better(&rows[b], &rows[i]);
                return (ids[x], worse.then(|| rows[b].file.id), false);
            }
            next += 1;
            (next - 1, None, true)
        })
        .collect()
}

/// Every group of undecided duplicates: identical copies first, then
/// similar ones; newest first within each.
pub fn find(candidates: &Candidates) -> Vec<Group> {
    let Candidates { rows, decided } = candidates;
    let open = |i: usize, j: usize| {
        let (a, b) = (rows[i].file.id, rows[j].file.id);
        !decided.contains(&(a.min(b), a.max(b)))
    };
    let same_content = |i: usize, j: usize| rows[i].full_hash.is_some() && rows[i].full_hash == rows[j].full_hash;

    let mut sets = UnionFind::new(rows.len());
    let mut near_edges = Vec::new();

    let mut by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some(h) = &r.full_hash {
            by_hash.entry(h).or_default().push(i);
        }
    }
    for members in by_hash.values().filter(|m| m.len() > 1) {
        for (k, &i) in members.iter().enumerate() {
            for &j in &members[k + 1..] {
                if open(i, j) {
                    sets.union(i, j);
                }
            }
        }
    }

    let mut by_phash: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some(p) = r.phash {
            by_phash.entry(p).or_default().push(i);
        }
    }
    let values: Vec<u64> = by_phash.keys().copied().collect();
    let mut bucket_pairs: Vec<(usize, usize)> = (0..values.len()).map(|v| (v, v)).collect();
    bucket_pairs.extend(phash::near_pairs(&values, NEAR_BITS));
    for (x, y) in bucket_pairs {
        let (a, b) = (&by_phash[&values[x]], &by_phash[&values[y]]);
        for (k, &i) in a.iter().enumerate() {
            let others = if x == y { &b[k + 1..] } else { &b[..] };
            for &j in others {
                if !same_content(i, j) && open(i, j) {
                    sets.union(i, j);
                    near_edges.push(i);
                }
            }
        }
    }

    // An iPhone edit can look quite different from its original (a portrait
    // blur); same camera number plus a loose hash match joins them.
    let mut by_number: HashMap<(String, u64), Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some((prefix, _, n)) = camera_name(&r.file.name) {
            by_number.entry((prefix, n)).or_default().push(i);
        }
    }
    for members in by_number.values().filter(|m| m.len() > 1) {
        for (k, &i) in members.iter().enumerate() {
            for &j in &members[k + 1..] {
                let close = matches!((rows[i].phash, rows[j].phash), (Some(a), Some(b)) if phash::distance(a, b) <= EDIT_BITS);
                if close
                    && !same_content(i, j)
                    && original_and_edit(&rows[i].file.name, &rows[j].file.name)
                    && rows[i].file.folder_id == rows[j].file.folder_id
                    && open(i, j)
                {
                    sets.union(i, j);
                    near_edges.push(i);
                }
            }
        }
    }

    let near_roots: HashSet<usize> = near_edges.into_iter().map(|i| sets.find(i)).collect();
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..rows.len() {
        members.entry(sets.find(i)).or_default().push(i);
    }
    let mut groups: Vec<(Group, String)> = members
        .into_iter()
        .filter(|(_, m)| m.len() > 1)
        .map(|(root, m)| {
            let mut copies: HashMap<&str, u32> = HashMap::new();
            for &i in &m {
                if let Some(h) = &rows[i].full_hash {
                    *copies.entry(h).or_default() += 1;
                }
            }
            let mut numbers: HashMap<&str, u32> = HashMap::new();
            let mut files: Vec<DupFile> = Vec::new();
            let mut order: Vec<usize> = m.clone();
            order.sort_by(|&i, &j| rows[j].file.taken.cmp(&rows[i].file.taken).then(rows[i].file.path.cmp(&rows[j].file.path)));
            let info = photo_rows(rows, &order, &open);
            for (&i, &(row, keeper, pick)) in order.iter().zip(&info) {
                let mut f = rows[i].file.clone();
                f.row = row;
                f.keeper = keeper;
                f.pick = pick;
                if let Some(h) = rows[i].full_hash.as_deref().filter(|h| copies[h] > 1) {
                    let next = numbers.len() as u32 + 1;
                    f.same = Some(*numbers.entry(h).or_insert(next));
                }
                files.push(f);
            }
            let newest = files.iter().filter_map(|f| f.taken.clone()).max().unwrap_or_default();
            let rows_in_group: HashSet<u32> = files.iter().map(|f| f.row).collect();
            let exact = !near_roots.contains(&root);
            let edited = files.iter().enumerate().any(|(x, a)| files[x + 1..].iter().any(|b| original_and_edit(&a.name, &b.name)));
            let kind = match (rows_in_group.len(), exact) {
                _ if edited => "edited",
                (1, true) => "identical",
                (1, false) => "resolution",
                _ => "similar",
            };
            (Group { exact, kind, files }, newest)
        })
        .collect();
    groups.sort_by(|(a, ta), (b, tb)| b.exact.cmp(&a.exact).then(tb.cmp(ta)).then(a.files[0].path.cmp(&b.files[0].path)));
    groups.into_iter().map(|(g, _)| g).collect()
}

/// Put the tags on the files of the groups (they are only needed there).
pub fn add_tags(conn: &Connection, groups: &mut [Group]) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT t.name, ft.source = 'user' FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1 ORDER BY ft.source = 'user', t.name",
    )?;
    for f in groups.iter_mut().flat_map(|g| g.files.iter_mut()) {
        f.tags = stmt.query_map([f.id], |r| Ok(DupTag { name: r.get(0)?, own: r.get(1)? }))?.collect::<rusqlite::Result<_>>()?;
    }
    Ok(())
}

/// Exact duplicates (same full hash) that lie in the same folder: per
/// content and folder one file stays (the highest resolution, then the
/// earliest record, then the first path) and the others go. Near duplicates
/// are never part of it, and a copy the user already decided about with the
/// keeper (`distinct`, `linked`) stays too. Returns `(keep, remove)`.
pub fn same_folder_plan(candidates: &Candidates) -> Vec<(i64, Vec<i64>)> {
    let Candidates { rows, decided } = candidates;
    let mut parts: HashMap<(&str, i64), Vec<&Row>> = HashMap::new();
    for r in rows {
        if let Some(h) = &r.full_hash {
            parts.entry((h, r.file.folder_id)).or_default().push(r);
        }
    }
    let mut plan: Vec<(i64, Vec<i64>)> = parts
        .into_values()
        .filter(|p| p.len() > 1)
        .filter_map(|mut p| {
            let pixels = |r: &Row| r.file.width.unwrap_or(0) as u64 * r.file.height.unwrap_or(0) as u64;
            let copies = copy_named(&p.iter().map(|r| r.file.name.as_str()).collect::<Vec<_>>());
            let flag: HashMap<i64, bool> = p.iter().map(|r| r.file.id).zip(copies).collect();
            p.sort_by(|a, b| {
                a.file
                    .in_copies
                    .cmp(&b.file.in_copies)
                    .then(pixels(b).cmp(&pixels(a)))
                    .then(flag[&a.file.id].cmp(&flag[&b.file.id]))
                    .then(a.added_at.cmp(&b.added_at))
                    .then(a.file.path.cmp(&b.file.path))
            });
            let keep = p[0].file.id;
            let gone: Vec<i64> = p[1..]
                .iter()
                .map(|r| r.file.id)
                .filter(|&id| !decided.contains(&(keep.min(id), keep.max(id))))
                .collect();
            (!gone.is_empty()).then_some((keep, gone))
        })
        .collect();
    plan.sort();
    plan
}

/// Copies that are surely the same photo as a better one (see `same_photo`
/// and `better`: a smaller or re-sent version, say from WhatsApp), without
/// review: the better file stays. Returns `(keep, remove)`; pairs the user
/// decided about never count.
pub fn lower_quality_plan(candidates: &Candidates) -> Vec<(i64, Vec<i64>)> {
    let mut by_keeper: HashMap<i64, Vec<i64>> = HashMap::new();
    for g in find(candidates) {
        for f in &g.files {
            if let Some(k) = f.keeper {
                by_keeper.entry(k).or_default().push(f.id);
            }
        }
    }
    let mut plan: Vec<(i64, Vec<i64>)> = by_keeper.into_iter().collect();
    for (_, gone) in &mut plan {
        gone.sort_unstable();
    }
    plan.sort();
    plan
}

/// What `remove_planned` did.
#[derive(Debug, Default, Serialize)]
pub struct BulkRemoved {
    /// Contents that lost copies.
    pub groups: u64,
    /// Files moved to the trash.
    pub removed: u64,
    pub tags_added: u64,
    pub dates_set: u64,
    pub skipped: Vec<String>,
}

/// Carry out `same_folder_plan` or `lower_quality_plan`; each group like
/// `remove_copies`.
pub fn remove_planned(conn: &Connection, root: &Path, plan: &[(i64, Vec<i64>)]) -> Result<BulkRemoved> {
    let mut out = BulkRemoved::default();
    for (keep, gone) in plan {
        match remove_copies(conn, root, &[*keep], gone, &HashMap::new()) {
            Ok(r) if r.conflicts.is_empty() => {
                out.groups += 1;
                out.removed += r.trashed.files.len() as u64;
                out.tags_added += r.tags_added;
                out.dates_set += r.dates_set;
                out.skipped.extend(r.trashed.skipped);
            }
            Ok(r) => out.skipped.extend(r.conflicts.iter().map(|c| format!("{}: capture dates conflict", c.path))),
            Err(e) => out.skipped.push(format!("{e:#}")),
        }
    }
    Ok(out)
}

/// Record a decision for every pair among `ids`, or forget it (`None`).
pub fn decide(conn: &Connection, ids: &[i64], decision: Option<&str>) -> Result<u64> {
    if let Some(d) = decision
        && !DECISIONS.contains(&d)
    {
        bail!("unknown decision {d}");
    }
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 {
        bail!("a decision needs at least two files");
    }
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for (k, &a) in ids.iter().enumerate() {
        for &b in &ids[k + 1..] {
            n += match decision {
                Some(d) => tx.execute(
                    "INSERT OR REPLACE INTO dup_decisions (a, b, decision, decided_at) VALUES (?1, ?2, ?3, ?4)",
                    params![a, b, d, db::now()],
                )?,
                None => tx.execute("DELETE FROM dup_decisions WHERE a = ?1 AND b = ?2", params![a, b])?,
            } as u64;
        }
    }
    tx.commit()?;
    Ok(n)
}

#[derive(Debug, Clone, Serialize)]
pub struct Linked {
    pub id: i64,
    pub path: String,
    pub missing: bool,
}

/// Other versions of a photo (`linked` decisions).
pub fn linked(conn: &Connection, id: i64) -> Result<Vec<Linked>> {
    let rows = conn
        .prepare(
            "SELECT f.id, f.path_nfc, f.missing_since IS NOT NULL FROM dup_decisions d
             JOIN files f ON f.id = CASE WHEN d.a = ?1 THEN d.b ELSE d.a END
             WHERE (d.a = ?1 OR d.b = ?1) AND d.decision = 'linked' ORDER BY f.path_nfc",
        )?
        .query_map([id], |r| Ok(Linked { id: r.get(0)?, path: r.get(1)?, missing: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

// ------------------------------------------------------------ removing copies

/// What the user's choice of copies to delete did.
#[derive(Debug, Default, Serialize)]
pub struct Removed {
    pub trashed: organize::Trashed,
    /// Tags put on surviving files as own tags (a file can get several).
    pub tags_added: u64,
    /// Survivors that got another capture date (an override in `library.db`).
    pub dates_set: u64,
    /// Capture dates that cannot be merged cleanly: nothing was done, the
    /// request is repeated with `dates` chosen.
    pub conflicts: Vec<DateConflict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DateConflict {
    pub keep: i64,
    pub path: String,
    pub dates: Vec<String>,
}

/// Dates further apart than this are a real conflict, not rounding or a
/// time zone.
const SAME_DAY_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq)]
struct Date {
    taken: String,
    offset: Option<String>,
}

impl Date {
    fn secs(&self) -> Option<i64> {
        chrono::NaiveDateTime::parse_from_str(&self.taken, "%Y-%m-%dT%H:%M:%S").ok().map(|d| d.and_utc().timestamp())
    }
}

#[derive(Debug, PartialEq)]
enum Merge {
    /// The survivor's date stays.
    Keep,
    Set(Date),
    Conflict(Vec<String>),
}

/// The capture date a survivor ends up with. It keeps its own when no copy
/// differs; the oldest date of all wins when they are less than a day
/// apart or the survivor has none; otherwise the user chooses.
fn merge_dates(own: &Option<Date>, others: &[Date], choice: Option<&str>) -> Merge {
    let mut all: Vec<&Date> = own.iter().chain(others).collect();
    let distinct: BTreeSet<&str> = all.iter().map(|d| d.taken.as_str()).collect();
    if distinct.len() <= 1 && (own.is_some() || all.is_empty()) {
        return Merge::Keep;
    }
    all.sort_by_key(|d| d.taken.clone());
    let spread = match (all.first().and_then(|d| d.secs()), all.last().and_then(|d| d.secs())) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    let pick = |taken: &str| all.iter().find(|d| d.taken == taken).map(|d| (*d).clone());
    let chosen = match choice {
        Some(c) => pick(c),
        None if spread.is_some_and(|s| s < SAME_DAY_SECS) || (own.is_none() && distinct.len() == 1) => all.first().map(|d| (*d).clone()),
        None => None,
    };
    match chosen {
        Some(d) if own.as_ref() == Some(&d) => Merge::Keep,
        Some(d) => Merge::Set(d),
        None => Merge::Conflict(distinct.iter().map(|d| d.to_string()).collect()),
    }
}

fn date_of(conn: &Connection, id: i64) -> Result<(Option<Date>, String)> {
    Ok(conn.query_row(
        &format!("SELECT {}, {}, quick_hash FROM files WHERE id = ?1", db::TAKEN, db::TAKEN_OFFSET),
        [id],
        |r| Ok((r.get::<_, Option<String>>(0)?.map(|taken| Date { taken, offset: r.get(1).ok().flatten() }), r.get(2)?)),
    )?)
}

/// Store the survivor's new date; if it is what the file says anyway, the
/// override goes instead.
fn set_date(conn: &Connection, id: i64, date: &Date) -> Result<()> {
    let (key, native): (String, Option<String>) =
        conn.query_row("SELECT quick_hash, taken FROM files WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if native.as_deref() == Some(date.taken.as_str()) {
        conn.execute("DELETE FROM taken_overrides WHERE key = ?1", [&key])?;
    } else {
        conn.execute(
            "INSERT OR REPLACE INTO taken_overrides (key, taken, taken_offset, at) VALUES (?1, ?2, ?3, ?4)",
            params![key, date.taken, date.offset, db::now()],
        )?;
    }
    Ok(())
}

struct Copy {
    id: i64,
    path: String,
    full_hash: Option<String>,
    phash: Option<u64>,
    pixels: u64,
    added_at: i64,
}

fn copy_of(conn: &Connection, id: i64) -> Result<Copy> {
    let row = conn
        .query_row(
            "SELECT id, path_nfc, full_hash, phash, coalesce(width, 0) * coalesce(height, 0), added_at
             FROM files WHERE id = ?1 AND missing_since IS NULL",
            [id],
            |r| {
                Ok(Copy {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    full_hash: r.get(2)?,
                    phash: r.get::<_, Option<String>>(3)?.as_deref().and_then(phash::from_hex),
                    pixels: r.get::<_, i64>(4)? as u64,
                    added_at: r.get(5)?,
                })
            },
        )
        .optional()?;
    row.ok_or_else(|| anyhow::anyhow!("file {id} is not in the library (any more)"))
}

/// The best of `keep` to inherit from `gone`: identical content first, else
/// a similar photo; the highest resolution, then the earliest record.
fn heir<'a>(gone: &Copy, keep: &'a [Copy]) -> Option<&'a Copy> {
    let same = |k: &&Copy| gone.full_hash.is_some() && k.full_hash == gone.full_hash;
    let similar = |k: &&Copy| match (gone.phash, k.phash) {
        (Some(a), Some(b)) => phash::distance(a, b) <= NEAR_BITS,
        _ => false,
    };
    let best = |mut c: Vec<&'a Copy>| {
        c.sort_by(|a, b| b.pixels.cmp(&a.pixels).then(a.added_at.cmp(&b.added_at)).then(a.path.cmp(&b.path)));
        c.into_iter().next()
    };
    best(keep.iter().filter(same).collect()).or_else(|| best(keep.iter().filter(similar).collect()))
}

/// Names of a file's tags of one source (`folder` or `user`).
fn tag_names(conn: &Connection, id: i64, source: &str) -> Result<Vec<String>> {
    Ok(conn
        .prepare(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE ft.file_id = ?1 AND ft.source = ?2 ORDER BY t.name",
        )?
        .query_map(params![id, source], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Give `heir` the tags of a copy that goes away (its folder tags and own
/// tags) as own tags, unless the heir has the tag as a folder tag already.
/// Returns how many it got.
fn carry_names(conn: &Connection, heir: i64, names: &[String]) -> Result<u64> {
    let mut have: HashSet<String> = tag_names(conn, heir, "folder")?.iter().map(|n| db::tag_fold(n)).collect();
    let mut added = 0;
    for name in names {
        if !have.insert(db::tag_fold(name)) {
            continue;
        }
        let tag = db::own_tag_id(conn, name)?;
        added += conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, source) VALUES (?1, ?2, 'user')",
            params![heir, tag],
        )? as u64;
    }
    Ok(added)
}

/// Move `remove` to the trash while `keep` stays. At least one file must
/// stay, and each removed file must be a duplicate of one that stays (same
/// content, or a similar photo), so this cannot delete anything else. What
/// would be lost goes to the survivor first (see `heir`): the folder tags
/// and own tags of a removed copy become own tags of the heir, and a capture
/// date it lacks (or the oldest one) becomes its date, stored as an override
/// in the database, never in the file. Dates that really conflict
/// (`DateConflict`) stop everything until `dates` (heir id → chosen date)
/// says which to take.
pub fn remove_copies(
    conn: &Connection,
    root: &Path,
    keep: &[i64],
    remove: &[i64],
    dates: &HashMap<i64, String>,
) -> Result<Removed> {
    if keep.is_empty() {
        bail!("at least one copy has to stay");
    }
    if remove.is_empty() {
        bail!("nothing to delete");
    }
    if remove.iter().any(|id| keep.contains(id)) {
        bail!("a copy cannot both stay and go");
    }
    let keep: Vec<Copy> = keep.iter().map(|&id| copy_of(conn, id)).collect::<Result<_>>()?;
    let gone: Vec<Copy> = remove.iter().map(|&id| copy_of(conn, id)).collect::<Result<_>>()?;
    // Everything that would be lost is read before the records go.
    struct Carry {
        path: String,
        heir: i64,
        names: Vec<String>,
        date: Option<Date>,
    }
    let mut carries = Vec::new();
    for g in &gone {
        let Some(h) = heir(g, &keep) else { bail!("{} is not a duplicate of a file that stays", g.path) };
        let mut names = tag_names(conn, g.id, "folder")?;
        names.extend(tag_names(conn, g.id, "user")?);
        carries.push(Carry { path: g.path.clone(), heir: h.id, names, date: date_of(conn, g.id)?.0 });
    }
    let merge_for = |heir: i64, done: &dyn Fn(&Carry) -> bool| -> Result<Merge> {
        let own = date_of(conn, heir)?.0;
        let others: Vec<Date> = carries.iter().filter(|c| c.heir == heir && done(c)).filter_map(|c| c.date.clone()).collect();
        Ok(merge_dates(&own, &others, dates.get(&heir).map(String::as_str)))
    };
    let heirs: BTreeSet<i64> = carries.iter().map(|c| c.heir).collect();
    let mut conflicts = Vec::new();
    for &h in &heirs {
        if let Merge::Conflict(options) = merge_for(h, &|_| true)? {
            let path = keep.iter().find(|k| k.id == h).map(|k| k.path.clone()).unwrap_or_default();
            conflicts.push(DateConflict { keep: h, path, dates: options });
        }
    }
    if !conflicts.is_empty() {
        return Ok(Removed { conflicts, ..Default::default() });
    }

    // What a backup check needs later, read before the records go: the removed
    // copy's content and the content of the copy that stays.
    let mut remembered = Vec::new();
    for (g, c) in gone.iter().zip(&carries) {
        let kept_hash = keep.iter().find(|k| k.id == c.heir).and_then(|k| k.full_hash.clone());
        if let (Some(full_hash), Some(kept_hash)) = (g.full_hash.clone(), kept_hash) {
            let size: i64 = conn.query_row("SELECT size FROM files WHERE id = ?1", [g.id], |r| r.get(0))?;
            remembered.push((g.path.clone(), size, full_hash, kept_hash));
        }
    }
    let ids: Vec<i64> = gone.iter().map(|g| g.id).collect();
    let trashed = organize::trash_files(conn, root, &ids)?;
    let mut out = Removed { trashed, ..Default::default() };
    let done: HashSet<String> = out.trashed.files.iter().cloned().collect();
    let went = |c: &Carry| done.contains(&c.path);
    let tx = conn.unchecked_transaction()?;
    for (path, size, full_hash, kept_hash) in remembered.iter().filter(|r| done.contains(&r.0)) {
        tx.execute(
            "INSERT INTO removed_copies (path_nfc, size, full_hash, kept_hash, removed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![path, size, full_hash, kept_hash, db::now()],
        )?;
    }
    for c in carries.iter().filter(|c| went(c)) {
        out.tags_added += carry_names(&tx, c.heir, &c.names)?;
    }
    for &h in &heirs {
        if let Merge::Set(d) = merge_for(h, &went)? {
            set_date(&tx, h, &d)?;
            out.dates_set += 1;
        }
    }
    tx.commit()?;
    Ok(out)
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect() }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(names: &[&str]) -> Vec<bool> {
        copy_named(names)
    }

    #[test]
    fn copy_names_next_to_their_original() {
        for copy in [
            "IMG_1 (2).jpg",
            "IMG_1(1).jpg",
            "IMG_1 - Copy.jpg",
            "IMG_1 - Copy (2).jpg",
            "IMG_1 - Kopie (3).JPG",
            "IMG_1 copy.jpg",
            "IMG_1 copy 2.jpg",
            "IMG_1 Kopie.jpg",
            "IMG_1 2.jpg",
            "IMG_1-1.jpg",
            "IMG_1_2.jpg",
            "IMG_1 (copy).jpg",
            "IMG_1 (another copy).jpg",
            "IMG_1 (3rd copy).jpg",
            "IMG_1 (Max's conflicted copy 2024-01-01).jpg",
            "img_1 (2).jpeg",
        ] {
            assert_eq!(flags(&["IMG_1.jpg", copy]), [false, true], "{copy}");
        }
    }

    #[test]
    fn names_that_are_not_copies_stay() {
        // No file with the base name: nothing is a copy.
        assert_eq!(flags(&["IMG_1 (2).jpg", "IMG_3.jpg"]), [false, false]);
        assert_eq!(flags(&["Bild 1.jpeg"]), [false]);
        // Different names in a group.
        assert_eq!(flags(&["IMG_0412.jpg", "IMG-20240812-WA0007.jpg"]), [false, false]);
        // A copy of a copy points at the original.
        assert_eq!(flags(&["a.jpg", "a (2).jpg", "a (2) (2).jpg"]), [false, true, true]);
    }

    #[test]
    fn camera_numbers_tell_shots_apart() {
        assert!(names_differ("IMG_6621.HEIC", "IMG_6620.HEIC"));
        assert!(!names_differ("IMG_6621.HEIC", "IMG_6621.JPG"));
        assert!(!names_differ("IMG_6621.HEIC", "IMG-20250726-WA0001.jpg"));
        // A copy's suffix is ignored: it still belongs to its own shot.
        assert!(names_differ("IMG_6621 (2).HEIC", "IMG_6620.HEIC"));
        assert!(names_differ("IMG_4285 1.JPG", "IMG_4284 1.JPG"));
        assert!(names_differ("IMG_4285 - Copy.JPG", "IMG_4284.JPG"));
        assert!(!names_differ("IMG_4284 1.JPG", "IMG_4284.JPG"));
        assert!(!names_differ("IMG_6621 (2).HEIC", "IMG_6621.HEIC"));
        assert!(!names_differ("PXL_20250101_120000123.jpg", "PXL_20250101_120000456.jpg"));
        assert!(names_differ("_DSC1235.NEF", "_DSC1234.JPG"));
    }

    #[test]
    fn iphone_edits_pair_with_their_original() {
        assert!(original_and_edit("IMG_6616.HEIC", "IMG_E6616.HEIC"));
        assert!(original_and_edit("IMG_E6616.JPG", "img_6616.heic"));
        assert!(!original_and_edit("IMG_6616.HEIC", "IMG_6616.JPG"));
        assert!(!original_and_edit("IMG_6616.HEIC", "IMG_E6617.HEIC"));
    }
}
