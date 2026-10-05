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
use crate::organize;
use crate::phash;

/// Same threshold as the plan: copies stay within it, different photos land
/// around 32 bits apart.
pub const NEAR_BITS: u32 = 8;

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
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    /// All files have identical content.
    pub exact: bool,
    pub files: Vec<DupFile>,
}

struct Row {
    file: DupFile,
    full_hash: Option<String>,
    phash: Option<u64>,
}

/// What the search needs from the database, loaded in one go so the
/// comparing can happen without holding the connection.
pub struct Candidates {
    rows: Vec<Row>,
    decided: HashSet<(i64, i64)>,
}

/// The files in `shown` (what the timeline shows: present, no RAW, no Live
/// Photo videos) and the decisions taken so far.
pub fn load(conn: &Connection, shown: &HashSet<i64>) -> Result<Candidates> {
    let rows: Vec<Row> = conn
        .prepare(
            &format!(
                "SELECT id, name, path_nfc, folder_id, kind, size, width, height, {}, quick_hash, full_hash, phash
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
                },
                full_hash: r.get(10)?,
                phash: r.get::<_, Option<String>>(11)?.as_deref().and_then(phash::from_hex),
            })
        })?
        .filter(|r| r.as_ref().map_or(true, |r| shown.contains(&r.file.id)))
        .collect::<rusqlite::Result<_>>()?;
    let decided: HashSet<(i64, i64)> = conn
        .prepare("SELECT a, b FROM dup_decisions")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Candidates { rows, decided })
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
            for i in order {
                let mut f = rows[i].file.clone();
                if let Some(h) = rows[i].full_hash.as_deref().filter(|h| copies[h] > 1) {
                    let next = numbers.len() as u32 + 1;
                    f.same = Some(*numbers.entry(h).or_insert(next));
                }
                files.push(f);
            }
            let newest = files.iter().filter_map(|f| f.taken.clone()).max().unwrap_or_default();
            (Group { exact: !near_roots.contains(&root), files }, newest)
        })
        .collect();
    groups.sort_by(|(a, ta), (b, tb)| b.exact.cmp(&a.exact).then(tb.cmp(ta)).then(a.files[0].path.cmp(&b.files[0].path)));
    groups.into_iter().map(|(g, _)| g).collect()
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

    let ids: Vec<i64> = gone.iter().map(|g| g.id).collect();
    let trashed = organize::trash_files(conn, root, &ids)?;
    let mut out = Removed { trashed, ..Default::default() };
    let done: HashSet<String> = out.trashed.files.iter().cloned().collect();
    let went = |c: &Carry| done.contains(&c.path);
    let tx = conn.unchecked_transaction()?;
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
