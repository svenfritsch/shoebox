//! Several drives, each with its own library: what they have in common.
//!
//! Every drive keeps its own `.shoebox` (index, thumbnails, faces, names).
//! Nothing is merged on disk. Comparing two drives opens one `library.db`
//! read-only and attaches the other one (`ATTACH`), so a single query matches
//! full hashes without reading any photo.
//!
//! A backup drive holds the same files as another drive on purpose, so it
//! must never produce duplicate suggestions. A drive is a backup when the
//! user says so; when they have not, a drive that holds (almost) nothing the
//! other drive does not is *suggested* as a backup and left out of the
//! duplicates until the user confirms or says "separate".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::db;

/// A drive counts as a probable backup when at least this share of its files
/// (by content) is also on the other drive.
pub const BACKUP_OVERLAP: f64 = 0.9;

const ROLE_KEY: &str = "role";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The user said: this drive is a copy of another one.
    Backup,
    /// The user said: this drive has its own photos.
    Separate,
    /// Not decided yet.
    Unknown,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "backup" => Some(Role::Backup),
            "separate" => Some(Role::Separate),
            "unknown" => Some(Role::Unknown),
            _ => None,
        }
    }
}

pub fn role(conn: &Connection) -> Result<Role> {
    Ok(db::setting(conn, ROLE_KEY)?.as_deref().and_then(Role::parse).unwrap_or(Role::Unknown))
}

pub fn set_role(conn: &Connection, role: Role) -> Result<()> {
    let value = match role {
        Role::Backup => Some("backup"),
        Role::Separate => Some("separate"),
        Role::Unknown => None,
    };
    db::set_setting(conn, ROLE_KEY, value)
}

/// One drive as the comparisons see it.
#[derive(Debug, Clone)]
pub struct Drive {
    pub id: String,
    pub name: String,
    pub db: PathBuf,
    pub role: Role,
}

/// `a` as main database and `b` attached as `other`, both read-only.
fn open_pair(a: &Path, b: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(a, flags).with_context(|| format!("open {}", a.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let b_str = b.to_str().context("library.db path is not valid UTF-8")?;
    conn.execute("ATTACH DATABASE ?1 AS other", [b_str]).with_context(|| format!("open {}", b.display()))?;
    Ok(conn)
}

/// How much two drives have in common, by content.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Overlap {
    /// Different contents on each drive (present files with a full hash).
    pub a_total: u64,
    pub b_total: u64,
    /// Contents on both.
    pub shared: u64,
}

impl Overlap {
    pub fn share_of_a(&self) -> f64 {
        if self.a_total == 0 { 0.0 } else { self.shared as f64 / self.a_total as f64 }
    }
    pub fn share_of_b(&self) -> f64 {
        if self.b_total == 0 { 0.0 } else { self.shared as f64 / self.b_total as f64 }
    }
}

pub fn overlap(a: &Path, b: &Path) -> Result<Overlap> {
    let conn = open_pair(a, b)?;
    let count = |sql: &str| -> Result<u64> { Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
    Ok(Overlap {
        a_total: count("SELECT count(DISTINCT full_hash) FROM main.files WHERE missing_since IS NULL AND full_hash IS NOT NULL")?,
        b_total: count("SELECT count(DISTINCT full_hash) FROM other.files WHERE missing_since IS NULL AND full_hash IS NOT NULL")?,
        shared: count(
            "SELECT count(DISTINCT a.full_hash) FROM main.files a
             WHERE a.missing_since IS NULL AND a.full_hash IS NOT NULL
               AND EXISTS (SELECT 1 FROM other.files b
                           WHERE b.full_hash = a.full_hash AND b.missing_since IS NULL)",
        )?,
    })
}

/// Which of two drives looks like the backup of the other, if any: the one
/// whose contents are (almost) all on the other drive and which is not the
/// bigger one. Two drives that mirror each other are `Mirror`: one of them
/// is the backup, and only the user can say which.
pub fn probable_backup(o: &Overlap) -> Option<Side> {
    if o.shared == 0 {
        return None;
    }
    let a_in_b = o.share_of_a() >= BACKUP_OVERLAP;
    let b_in_a = o.share_of_b() >= BACKUP_OVERLAP;
    match (a_in_b, b_in_a) {
        (true, true) if o.a_total == o.b_total => Some(Side::Mirror),
        (true, _) if o.a_total <= o.b_total => Some(Side::A),
        (_, true) if o.b_total <= o.a_total => Some(Side::B),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The first drive looks like the backup of the second.
    A,
    B,
    /// Both hold the same: either could be the backup.
    Mirror,
}

/// Why a drive is left out of the duplicate search.
#[derive(Debug, Clone, Serialize)]
pub struct Excluded {
    pub library: String,
    pub name: String,
    pub reason: String,
}

/// A drive with what the user said and what the contents suggest.
#[derive(Debug, Clone, Serialize)]
pub struct DriveRole {
    pub library: String,
    pub name: String,
    pub role: Role,
    /// Looks like a backup of this drive (name), though the user has not said so.
    pub suggested_backup_of: Option<String>,
}

/// The roles of all drives, with a suggestion where the user has not decided.
pub fn roles(drives: &[Drive]) -> Result<Vec<DriveRole>> {
    let mut out: Vec<DriveRole> = drives
        .iter()
        .map(|d| DriveRole { library: d.id.clone(), name: d.name.clone(), role: d.role, suggested_backup_of: None })
        .collect();
    for i in 0..drives.len() {
        for j in i + 1..drives.len() {
            let o = overlap(&drives[i].db, &drives[j].db)?;
            let pairs: &[(usize, usize)] = match probable_backup(&o) {
                Some(Side::A) => &[(i, j)],
                Some(Side::B) => &[(j, i)],
                // Once the user has decided about one of two mirrors, the
                // other one is not suspected on its own.
                Some(Side::Mirror) if drives[i].role == Role::Unknown && drives[j].role == Role::Unknown => &[(i, j), (j, i)],
                _ => continue,
            };
            for &(backup, of) in pairs {
                // Only for drives the user has not decided about, and not
                // when the other one is a backup itself (it is already out).
                if drives[backup].role == Role::Unknown
                    && drives[of].role != Role::Backup
                    && out[backup].suggested_backup_of.is_none()
                {
                    out[backup].suggested_backup_of = Some(drives[of].name.clone());
                }
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateFile {
    pub library: String,
    pub name: String,
    pub id: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub files: Vec<DuplicateFile>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CrossDuplicates {
    /// Contents that are on at least two drives (up to the limit asked for).
    pub groups: Vec<DuplicateGroup>,
    pub total_groups: u64,
    /// Drives that took part.
    pub compared: Vec<String>,
    /// Backups, and drives that may be one (not decided): no suggestions.
    pub excluded: Vec<Excluded>,
}

/// Contents that exist on two or more drives that are not backups of each
/// other. Offline drives are not passed in.
pub fn cross_duplicates(drives: &[Drive], limit: usize) -> Result<CrossDuplicates> {
    let suggestions = roles(drives)?;
    let mut result = CrossDuplicates::default();
    let mut eligible: Vec<&Drive> = Vec::new();
    for (d, s) in drives.iter().zip(&suggestions) {
        match (d.role, &s.suggested_backup_of) {
            (Role::Backup, _) => result.excluded.push(Excluded {
                library: d.id.clone(),
                name: d.name.clone(),
                reason: "marked as a backup".into(),
            }),
            (Role::Unknown, Some(of)) => result.excluded.push(Excluded {
                library: d.id.clone(),
                name: d.name.clone(),
                reason: format!("looks like a backup of {of}; confirm it or mark it as separate"),
            }),
            _ => {
                result.compared.push(d.name.clone());
                eligible.push(d);
            }
        }
    }

    // hash → (size, files), files unique by (drive, id)
    let mut groups: BTreeMap<String, (u64, BTreeSet<(usize, i64, String)>)> = BTreeMap::new();
    for i in 0..eligible.len() {
        for j in i + 1..eligible.len() {
            let conn = open_pair(&eligible[i].db, &eligible[j].db)?;
            let mut stmt = conn.prepare(
                "SELECT a.full_hash, a.size, a.id, a.path, b.id, b.path
                 FROM main.files a JOIN other.files b ON b.full_hash = a.full_hash
                 WHERE a.full_hash IS NOT NULL AND a.missing_since IS NULL AND b.missing_since IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, String>(5)?))
            })?;
            for row in rows {
                let (hash, size, a_id, a_path, b_id, b_path) = row?;
                let entry = groups.entry(hash).or_insert_with(|| (size, BTreeSet::new()));
                entry.1.insert((i, a_id, a_path));
                entry.1.insert((j, b_id, b_path));
            }
        }
    }
    result.total_groups = groups.len() as u64;
    result.groups = groups
        .into_iter()
        .take(limit)
        .map(|(hash, (size, files))| DuplicateGroup {
            hash,
            size,
            files: files
                .into_iter()
                .map(|(d, id, path)| DuplicateFile { library: eligible[d].id.clone(), name: eligible[d].name.clone(), id, path })
                .collect(),
        })
        .collect();
    Ok(result)
}

/// A person by name over all drives.
#[derive(Debug, Clone, Serialize)]
pub struct MergedPerson {
    pub name: String,
    /// The name of the person's group (the first drive that has one).
    pub group: Option<String>,
    pub faces: u64,
    pub photos: u64,
    pub hidden: bool,
    /// Where this person is known: the same name on a drive is the same person.
    pub libraries: Vec<PersonOnDrive>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PersonOnDrive {
    pub library: String,
    pub name: String,
    /// The person's id in that drive's database.
    pub person: i64,
    pub faces: u64,
    pub photos: u64,
    pub cover: Option<i64>,
    pub cover_manual: Option<i64>,
}

/// The same name (NFC, ignoring case) on different drives is the same
/// person. Names, groups and decisions stay in each drive's own database;
/// this only reads them.
pub fn merge_people(per_drive: &[(String, String, Vec<crate::people::Person>, Vec<crate::people::Group>)]) -> Vec<MergedPerson> {
    let mut merged: BTreeMap<String, MergedPerson> = BTreeMap::new();
    for (lib, lib_name, people, groups) in per_drive {
        for p in people {
            let key = crate::library::nfc(p.name.trim()).to_lowercase();
            let group = p.group_id.and_then(|g| groups.iter().find(|x| x.id == g)).map(|g| g.name.clone());
            let entry = merged.entry(key).or_insert_with(|| MergedPerson {
                name: crate::library::nfc(p.name.trim()),
                group: None,
                faces: 0,
                photos: 0,
                hidden: true,
                libraries: Vec::new(),
            });
            entry.group = entry.group.take().or(group);
            entry.faces += p.faces;
            entry.photos += p.photos;
            entry.hidden &= p.hidden;
            entry.libraries.push(PersonOnDrive {
                library: lib.clone(),
                name: lib_name.clone(),
                person: p.id,
                faces: p.faces,
                photos: p.photos,
                cover: p.cover,
                cover_manual: p.cover_manual,
            });
        }
    }
    let mut list: Vec<MergedPerson> = merged.into_values().collect();
    list.sort_by(|a, b| b.faces.cmp(&a.faces).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    list
}
