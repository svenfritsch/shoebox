//! Own tags (phase 5b): tags the user adds to photos, on top of the folder
//! tags that come from where a file is.
//!
//! - Own tags are `file_tags` rows with `source = 'user'`, attached to the
//!   file id, so they survive moves and rescans (the scan only ever touches
//!   `source = 'folder'`). They are never written into originals or next to
//!   them.
//! - Folder tags cannot be removed: they would come back with the next scan.
//!   Removing only ever deletes `source = 'user'` rows.
//! - Names are trimmed and compared NFC and ignoring case (`db::tag_fold`);
//!   the first spelling wins, and an own tag with a folder tag's name is
//!   that tag. Folder tags themselves keep the folder's exact spelling.
//! - The trash keeps a file's own tags (`trash.user_tags`) and a restore
//!   puts them back.
//! - `userdata.json` next to `library.db` holds everything the user made
//!   (own tags; people, groups and face decisions since 5c-2, see
//!   `people.rs`), readable and easy to back up.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params, params_from_iter};
use serde::Serialize;

use crate::db;
use crate::library;

/// Longest tag name, in characters.
pub const MAX_NAME: usize = 100;
/// Written next to `library.db`.
pub const USERDATA_FILE: &str = "userdata.json";

/// A tag name typed by the user: trimmed, NFC, not empty, no control
/// characters.
pub fn check_name(name: &str) -> Result<String> {
    let name = library::nfc(name.trim());
    if name.is_empty() {
        bail!("the tag needs a name");
    }
    if name.chars().count() > MAX_NAME {
        bail!("tag names can have at most {MAX_NAME} characters");
    }
    if name.chars().any(char::is_control) {
        bail!("tag names cannot contain control characters");
    }
    Ok(name)
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Changed {
    /// The tag in its stored spelling (`None` when removing a name no file has).
    pub tag: Option<TagRef>,
    /// Files that got the tag, or lost it.
    pub files: u64,
    /// Removing: files among the ids that keep the tag as a folder tag.
    pub folder: u64,
}

fn tag_ref(conn: &Connection, id: i64) -> Result<TagRef> {
    Ok(TagRef { id, name: conn.query_row("SELECT name FROM tags WHERE id = ?1", [id], |r| r.get(0))? })
}

/// Give files an own tag. Unknown ids are skipped; files that have it
/// already (as an own tag) count as unchanged.
pub fn add(conn: &Connection, ids: &[i64], name: &str) -> Result<Changed> {
    let name = check_name(name)?;
    let tx = conn.unchecked_transaction()?;
    let tag = db::own_tag_id(&tx, &name)?;
    let mut files = 0;
    {
        let mut insert = tx.prepare(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, source) SELECT id, ?2, 'user' FROM files WHERE id = ?1",
        )?;
        for id in ids {
            files += insert.execute(params![id, tag])? as u64;
        }
    }
    let tag = tag_ref(&tx, tag)?;
    tx.commit()?;
    Ok(Changed { tag: Some(tag), files, folder: 0 })
}

/// Take an own tag off files. A folder tag of the same name stays.
pub fn remove(conn: &Connection, ids: &[i64], name: &str) -> Result<Changed> {
    let name = check_name(name)?;
    let tx = conn.unchecked_transaction()?;
    let Some(tag) = db::find_tag(&tx, &name)? else {
        return Ok(Changed { tag: None, files: 0, folder: 0 });
    };
    let tag_ref = tag_ref(&tx, tag)?;
    // Folder tags keep their exact spelling, so one name can have several
    // tag rows; an own tag counts as one with all of them.
    let fold = db::tag_fold(&name);
    let (mut files, mut folder) = (0, 0);
    {
        let mut delete = tx.prepare(
            "DELETE FROM file_tags WHERE file_id = ?1 AND source = 'user'
                AND tag_id IN (SELECT id FROM tags WHERE fold = ?2)",
        )?;
        let mut kept = tx.prepare(
            "SELECT count(*) > 0 FROM file_tags WHERE file_id = ?1 AND source = 'folder'
                AND tag_id IN (SELECT id FROM tags WHERE fold = ?2)",
        )?;
        for id in ids {
            files += (delete.execute(params![id, fold])? > 0) as u64;
            folder += kept.query_row(params![id, fold], |r| r.get::<_, bool>(0))? as u64;
        }
    }
    // A tag nothing refers to any more goes (the trash keeps names, not ids).
    tx.execute(
        "DELETE FROM tags WHERE fold = ?1 AND NOT EXISTS (SELECT 1 FROM file_tags WHERE tag_id = tags.id)",
        [&fold],
    )?;
    tx.commit()?;
    Ok(Changed { tag: Some(tag_ref), files, folder })
}

#[derive(Debug, Serialize)]
pub struct Counted {
    pub id: i64,
    pub name: String,
    /// How many of the given files have it.
    pub count: u64,
}

/// The own tags on any of these files, most used first ("Remove tag…" for a
/// selection).
pub fn own_tags_of(conn: &Connection, ids: &[i64]) -> Result<Vec<Counted>> {
    let mut counts: BTreeMap<i64, (String, u64)> = BTreeMap::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT t.id, t.name, count(DISTINCT ft.file_id) FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE ft.source = 'user' AND ft.file_id IN ({marks}) GROUP BY t.id"
        ))?;
        let mut rows = stmt.query(params_from_iter(chunk))?;
        while let Some(r) = rows.next()? {
            let e = counts.entry(r.get(0)?).or_insert_with(|| (String::new(), 0));
            e.0 = r.get(1)?;
            e.1 += r.get::<_, i64>(2)? as u64;
        }
    }
    let mut out: Vec<Counted> = counts.into_iter().map(|(id, (name, count))| Counted { id, name, count }).collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Own tag names of one file, for the trash (`trash.user_tags`): a JSON
/// array, or `None` if it has none.
pub fn own_names_json(conn: &Connection, file_id: i64) -> Result<Option<String>> {
    let names: Vec<String> = conn
        .prepare(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE ft.file_id = ?1 AND ft.source = 'user' ORDER BY t.name",
        )?
        .query_map([file_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(if names.is_empty() { None } else { Some(serde_json::to_string(&names)?) })
}

/// Put own tags back on a file restored from the trash.
pub fn restore_names_json(conn: &Connection, file_id: i64, json: &str) -> Result<()> {
    let names: Vec<String> = serde_json::from_str(json).context("own tags in the trash")?;
    for name in names {
        let tag = db::own_tag_id(conn, &name)?;
        conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, source) VALUES (?1, ?2, 'user')",
            params![file_id, tag],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------- userdata.json

#[derive(Debug, Serialize)]
pub struct UserData {
    pub shoebox: &'static str,
    /// Format of this file.
    pub version: u32,
    /// Unix seconds.
    pub written_at: i64,
    pub own_tags: Vec<OwnTag>,
    /// Groups, people and face decisions (version 2).
    #[serde(flatten)]
    pub people: crate::people::UserPeople,
}

#[derive(Debug, Serialize)]
pub struct OwnTag {
    pub name: String,
    pub files: Vec<TaggedFile>,
}

/// A file by path and content, so the tags can be matched again even if the
/// index is lost and the files moved.
#[derive(Debug, Serialize)]
pub struct TaggedFile {
    /// Relative to the library (NFC); where it was, for files in the trash.
    pub path: String,
    pub quick_hash: Option<String>,
    pub full_hash: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub in_trash: bool,
}

/// Everything the user made that cannot be rebuilt from the drive.
pub fn user_data(conn: &Connection) -> Result<UserData> {
    let mut tags: BTreeMap<String, OwnTag> = BTreeMap::new();
    let mut add = |name: String, file: TaggedFile| {
        tags.entry(db::tag_fold(&name)).or_insert_with(|| OwnTag { name, files: Vec::new() }).files.push(file);
    };
    {
        let mut stmt = conn.prepare(
            "SELECT t.name, f.path_nfc, f.quick_hash, f.full_hash FROM file_tags ft
             JOIN tags t ON t.id = ft.tag_id JOIN files f ON f.id = ft.file_id
             WHERE ft.source = 'user' ORDER BY t.id, f.path_nfc",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            add(
                r.get(0)?,
                TaggedFile { path: r.get(1)?, quick_hash: r.get(2)?, full_hash: r.get(3)?, in_trash: false },
            );
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT user_tags, path_nfc, quick_hash, full_hash FROM trash WHERE user_tags IS NOT NULL ORDER BY path_nfc",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let names: Vec<String> = serde_json::from_str(&r.get::<_, String>(0)?).unwrap_or_default();
            let (path, quick_hash, full_hash): (String, Option<String>, Option<String>) = (r.get(1)?, r.get(2)?, r.get(3)?);
            for name in names {
                add(
                    name,
                    TaggedFile { path: path.clone(), quick_hash: quick_hash.clone(), full_hash: full_hash.clone(), in_trash: true },
                );
            }
        }
    }
    let mut own_tags: Vec<OwnTag> = tags.into_values().collect();
    own_tags.sort_by_key(|t| t.name.to_lowercase());
    Ok(UserData {
        shoebox: env!("CARGO_PKG_VERSION"),
        version: 2,
        written_at: db::now(),
        own_tags,
        people: crate::people::user_data(conn)?,
    })
}

/// Write `userdata.json` next to the database, replacing the previous copy
/// atomically.
pub fn write_user_data(conn: &Connection, db_path: &Path) -> Result<PathBuf> {
    let dst = db_path.with_file_name(USERDATA_FILE);
    let tmp = db_path.with_file_name(format!("{USERDATA_FILE}.tmp"));
    let json = serde_json::to_vec_pretty(&user_data(conn)?)?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("write {}", tmp.display()))?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &dst).with_context(|| format!("write {}", dst.display()))?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  Europa-Park ").unwrap(), "Europa-Park");
        assert_eq!(check_name("O\u{308}sterreich").unwrap(), "\u{d6}sterreich");
        assert!(check_name("   ").is_err());
        assert!(check_name("a\nb").is_err());
        assert!(check_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert_eq!(db::tag_fold(" Europa-PARK"), db::tag_fold("europa-park"));
        assert_eq!(db::tag_fold("O\u{308}STERREICH"), db::tag_fold("\u{f6}sterreich"));
    }
}
