//! Duplicates: exact (same full hash) and near (perceptual hashes at most
//! `NEAR_BITS` apart: resized, re-encoded or lightly edited copies, bursts).
//!
//! Files are grouped by the pairs between them, so a group may also hold a
//! chain of similar shots. Per group the user decides: different photos
//! (`distinct`), versions of one photo (`linked`), or one of them goes to
//! the trash. Decided pairs are not offered again; ids survive moves.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::db;
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
            "SELECT id, name, path_nfc, folder_id, kind, size, width, height, taken, quick_hash, full_hash, phash
             FROM files WHERE missing_since IS NULL AND kind != 'raw'",
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
