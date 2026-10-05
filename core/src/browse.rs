//! What the web UI shows: the timeline (sorted, filtered), the folder tree
//! and tags. Read-only over the index; the server keeps one `Snapshot` until
//! another process (a scan) commits to the database.
//!
//! Sorting: newest first by capture date. A file without one takes the
//! month of its nearest `YYYY-MM Name` folder, and failing that its
//! modification date. RAW files are not shown (their JPEG/HEIC twin is), and
//! the short video of a Live Photo is folded into its still.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{Local, TimeZone};
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use serde::Serialize;

use crate::classify::Kind;
use crate::library;

/// Videos at most this long that share folder and name with a still are the
/// motion part of a Live Photo.
const LIVE_MAX_MS: i64 = 6_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DateSource {
    /// Capture date from the file (EXIF, video container).
    File,
    /// Month of the event folder.
    Folder,
    /// Modification date.
    Modified,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: i64,
    pub kind: Kind,
    pub folder_id: i64,
    /// `YYYY-MM-DDTHH:MM:SS`, local time.
    pub sort: String,
    pub date_source: DateSource,
    /// First characters of the quick hash: changes when the content does,
    /// so thumbnail URLs can be cached for good.
    pub version: String,
    /// Lowercased NFC path, for text search.
    pub path_lower: String,
    /// The video of a Live Photo.
    pub live: Option<i64>,
}

impl Item {
    /// `YYYYMMDD` as a number, for the client.
    pub fn day(&self) -> u32 {
        let d: String = self.sort.chars().take(10).filter(char::is_ascii_digit).collect();
        d.parse().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    pub id: i64,
    pub parent: Option<i64>,
    pub name: String,
    pub path: String,
    /// Shown files in this folder and below.
    pub count: u64,
}

pub struct Snapshot {
    /// Newest first.
    pub items: Vec<Item>,
    pub folders: Vec<Folder>,
    children: HashMap<i64, Vec<i64>>,
}

#[derive(Debug, Default, Clone)]
pub struct Query {
    pub folder: Option<i64>,
    /// Every one of these tags (AND); a tag also matches the other spellings
    /// of its name (folder tags keep their folder's exact spelling).
    pub tags: Vec<i64>,
    pub text: Option<String>,
    /// Only photos with a confirmed face of every one of these people
    /// (AND, 5c-3).
    pub people: Vec<i64>,
}

impl Snapshot {
    pub fn load(conn: &Connection) -> Result<Snapshot> {
        struct FolderRow {
            parent: Option<i64>,
            event: Option<(i64, i64)>,
        }
        let mut folder_rows: HashMap<i64, FolderRow> = HashMap::new();
        let mut folders = Vec::new();
        {
            let mut stmt =
                conn.prepare("SELECT id, parent_id, name, path_nfc, event_year, event_month FROM folders")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                let parent: Option<i64> = r.get(1)?;
                let event = match (r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?) {
                    (Some(y), Some(m)) => Some((y, m)),
                    _ => None,
                };
                folder_rows.insert(id, FolderRow { parent, event });
                folders.push(Folder { id, parent, name: r.get(2)?, path: r.get(3)?, count: 0 });
            }
        }
        // Nearest event folder (the folder itself or an ancestor).
        let mut event_of: HashMap<i64, Option<(i64, i64)>> = HashMap::new();
        for &id in folder_rows.keys() {
            let mut cur = Some(id);
            let mut found = None;
            let mut depth = 0;
            while let Some(f) = cur.and_then(|c| folder_rows.get(&c)) {
                if let Some(e) = f.event {
                    found = Some(e);
                    break;
                }
                cur = f.parent;
                depth += 1;
                if depth > 1000 {
                    break; // a cycle would be a bug; do not hang on it
                }
            }
            event_of.insert(id, found);
        }

        struct Row {
            id: i64,
            kind: Kind,
            folder_id: i64,
            stem: String,
            path_lower: String,
            taken: Option<String>,
            mtime_ns: i64,
            duration_ms: Option<i64>,
            quick_hash: String,
        }
        let mut rows = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, kind, folder_id, name, path_nfc, taken, mtime_ns, duration_ms, quick_hash
                 FROM files WHERE missing_since IS NULL AND kind != 'raw'",
            )?;
            let mut q = stmt.query([])?;
            while let Some(r) = q.next()? {
                let Some(kind) = Kind::parse(&r.get::<_, String>(1)?) else { continue };
                let name: String = r.get(3)?;
                let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name).to_lowercase();
                rows.push(Row {
                    id: r.get(0)?,
                    kind,
                    folder_id: r.get(2)?,
                    stem,
                    path_lower: r.get::<_, String>(4)?.to_lowercase(),
                    taken: r.get(5)?,
                    mtime_ns: r.get(6)?,
                    duration_ms: r.get(7)?,
                    quick_hash: r.get(8)?,
                });
            }
        }

        // Live Photos: a still and a short video with the same folder and stem.
        let mut stills: HashMap<(i64, &str), i64> = HashMap::new();
        for r in &rows {
            if matches!(r.kind, Kind::Jpeg | Kind::Heic) {
                stills.insert((r.folder_id, r.stem.as_str()), r.id);
            }
        }
        let mut live: HashMap<i64, i64> = HashMap::new();
        let mut hidden: HashSet<i64> = HashSet::new();
        for r in &rows {
            if r.kind == Kind::Video
                && r.duration_ms.is_some_and(|d| d <= LIVE_MAX_MS)
                && let Some(&still) = stills.get(&(r.folder_id, r.stem.as_str()))
            {
                live.entry(still).or_insert(r.id);
                hidden.insert(r.id);
            }
        }

        let mut items: Vec<Item> = rows
            .iter()
            .filter(|r| !hidden.contains(&r.id))
            .map(|r| {
                let (sort, date_source) = match (&r.taken, event_of.get(&r.folder_id).copied().flatten()) {
                    (Some(t), _) => (t.clone(), DateSource::File),
                    (None, Some((y, m))) => (format!("{y:04}-{m:02}-01T00:00:00"), DateSource::Folder),
                    (None, None) => (local_time(r.mtime_ns), DateSource::Modified),
                };
                Item {
                    id: r.id,
                    kind: r.kind,
                    folder_id: r.folder_id,
                    sort,
                    date_source,
                    version: r.quick_hash.chars().take(8).collect(),
                    path_lower: r.path_lower.clone(),
                    live: live.get(&r.id).copied(),
                }
            })
            .collect();
        items.sort_by(|a, b| b.sort.cmp(&a.sort).then_with(|| a.path_lower.cmp(&b.path_lower)));

        // Folder counts, summed up the tree.
        let mut direct: HashMap<i64, u64> = HashMap::new();
        for it in &items {
            *direct.entry(it.folder_id).or_default() += 1;
        }
        let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
        for f in &folders {
            if let Some(p) = f.parent {
                children.entry(p).or_default().push(f.id);
            }
        }
        let mut snapshot = Snapshot { items, folders, children };
        let totals: HashMap<i64, u64> = snapshot
            .folders
            .iter()
            .map(|f| (f.id, snapshot.subtree(f.id).iter().map(|id| direct.get(id).copied().unwrap_or(0)).sum()))
            .collect();
        for f in &mut snapshot.folders {
            f.count = totals[&f.id];
        }
        snapshot.folders.sort_by_key(|f| f.path.to_lowercase());
        Ok(snapshot)
    }

    /// A folder and every folder below it.
    pub fn subtree(&self, root: i64) -> HashSet<i64> {
        let mut out = HashSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if out.insert(id) {
                stack.extend(self.children.get(&id).into_iter().flatten());
            }
        }
        out
    }

    /// Items matching every part of the query, newest first.
    pub fn query(&self, conn: &Connection, q: &Query) -> Result<Vec<&Item>> {
        let folders = q.folder.map(|f| self.subtree(f));
        let mut tagged: Option<HashSet<i64>> = None;
        for &tag in &q.tags {
            let ids = file_ids_with_tag_name(conn, tag)?;
            tagged = Some(match tagged {
                Some(t) => t.intersection(&ids).copied().collect(),
                None => ids,
            });
        }
        // Each word must appear in the path, in one of the file's tags or in
        // the name of someone confirmed on it.
        let mut words: Vec<(String, HashSet<i64>)> = Vec::new();
        let text = q.text.as_deref().unwrap_or("");
        let (tag_names, people_names): (Vec<(i64, String)>, Vec<(i64, String)>) = if text.trim().is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let names = |sql: &str| -> Result<Vec<(i64, String)>> {
                Ok(conn
                    .prepare(sql)?
                    .query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?.to_lowercase())))?
                    .collect::<rusqlite::Result<_>>()?)
            };
            let people = if crate::people::table_exists(conn, "main", "people")? {
                names("SELECT id, name FROM people")?
            } else {
                Vec::new()
            };
            (names("SELECT id, name FROM tags")?, people)
        };
        let mut file_keys: Option<HashMap<String, Vec<i64>>> = None;
        let mut files_of_people = |people: &[i64]| -> Result<HashSet<i64>> {
            if people.is_empty() {
                return Ok(HashSet::new());
            }
            if file_keys.is_none() {
                let mut by_key: HashMap<String, Vec<i64>> = HashMap::new();
                let mut stmt = conn.prepare("SELECT id, quick_hash FROM files WHERE missing_since IS NULL")?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    by_key.entry(r.get(1)?).or_default().push(r.get(0)?);
                }
                file_keys = Some(by_key);
            }
            let by_key = file_keys.as_ref().unwrap();
            let mut ids = HashSet::new();
            for key in crate::people::keys_of_people(conn, people)? {
                ids.extend(by_key.get(&key).into_iter().flatten());
            }
            Ok(ids)
        };
        for word in text.split_whitespace() {
            let word = library::nfc(word).to_lowercase();
            let tags: Vec<i64> =
                tag_names.iter().filter(|(_, name)| name.contains(&word)).map(|(id, _)| *id).collect();
            let mut ids = file_ids_with_tags(conn, &tags)?;
            let people: Vec<i64> =
                people_names.iter().filter(|(_, name)| name.contains(&word)).map(|(id, _)| *id).collect();
            ids.extend(files_of_people(&people)?);
            words.push((word, ids));
        }
        let mut person: Option<HashSet<i64>> = None;
        for &p in &q.people {
            let ids = files_of_people(&[p])?;
            person = Some(match person {
                Some(have) => have.intersection(&ids).copied().collect(),
                None => ids,
            });
        }
        Ok(self
            .items
            .iter()
            .filter(|it| person.as_ref().is_none_or(|p| p.contains(&it.id)))
            .filter(|it| folders.as_ref().is_none_or(|f| f.contains(&it.folder_id)))
            .filter(|it| tagged.as_ref().is_none_or(|t| t.contains(&it.id)))
            .filter(|it| words.iter().all(|(w, ids)| ids.contains(&it.id) || it.path_lower.contains(w.as_str())))
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TagKind {
    /// Only from folder names.
    Folder,
    /// Only added by the user.
    Own,
    /// Both: a folder name that the user also added to other photos.
    Both,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub count: u64,
    pub kind: TagKind,
}

/// Every tag with the number of present files carrying it, most used first.
pub fn all_tags(conn: &Connection) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, count(DISTINCT f.id) AS n, max(ft.source = 'folder'), max(ft.source = 'user') FROM tags t
         JOIN file_tags ft ON ft.tag_id = t.id
         JOIN files f ON f.id = ft.file_id AND f.missing_since IS NULL AND f.kind != 'raw'
         GROUP BY t.id ORDER BY n DESC, t.name",
    )?;
    let tags = stmt
        .query_map([], |r| {
            let kind = match (r.get::<_, bool>(3)?, r.get::<_, bool>(4)?) {
                (true, true) => TagKind::Both,
                (false, true) => TagKind::Own,
                _ => TagKind::Folder,
            };
            Ok(Tag { id: r.get(0)?, name: r.get(1)?, count: r.get::<_, i64>(2)? as u64, kind })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tags)
}

/// A tag of one file and where it comes from: `folder` (its path) or `user`
/// (an own tag). A tag can be both; then it is listed twice.
#[derive(Debug, Clone, Serialize)]
pub struct FileTag {
    pub id: i64,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagName {
    pub id: i64,
    pub name: String,
}

/// Names of tags by id (unknown ids are left out).
pub fn tag_names(conn: &Connection, ids: &[i64]) -> Result<Vec<TagName>> {
    let mut stmt = conn.prepare("SELECT name FROM tags WHERE id = ?1")?;
    let mut out = Vec::new();
    for &id in ids {
        if let Some(name) = stmt.query_row([id], |r| r.get(0)).optional()? {
            out.push(TagName { id, name });
        }
    }
    Ok(out)
}

/// Files with this tag or another spelling of its name.
fn file_ids_with_tag_name(conn: &Connection, tag: i64) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT ft.file_id FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
         WHERE t.fold = (SELECT fold FROM tags WHERE id = ?1)",
    )?;
    let ids = stmt.query_map([tag], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Tags of the given files with how many of them carry each, most used
/// first (suggestions that narrow a search). Tags in `skip` are left out.
pub fn tags_within(conn: &Connection, files: &HashSet<i64>, skip: &[i64]) -> Result<Vec<Tag>> {
    let mut counts: HashMap<i64, (String, HashSet<i64>, bool, bool)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT ft.file_id, t.id, t.name, ft.source FROM file_tags ft JOIN tags t ON t.id = ft.tag_id")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let file: i64 = r.get(0)?;
            let tag: i64 = r.get(1)?;
            if !files.contains(&file) || skip.contains(&tag) {
                continue;
            }
            let e = counts.entry(tag).or_insert_with(|| (String::new(), HashSet::new(), false, false));
            if e.0.is_empty() {
                e.0 = r.get(2)?;
            }
            e.1.insert(file);
            match r.get_ref(3)?.as_str()? {
                "folder" => e.2 = true,
                _ => e.3 = true,
            }
        }
    }
    let mut tags: Vec<Tag> = counts
        .into_iter()
        .map(|(id, (name, files, folder, own))| Tag {
            id,
            name,
            count: files.len() as u64,
            kind: match (folder, own) {
                (true, true) => TagKind::Both,
                (false, true) => TagKind::Own,
                _ => TagKind::Folder,
            },
        })
        .collect();
    tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    Ok(tags)
}

fn file_ids_with_tags(conn: &Connection, tags: &[i64]) -> Result<HashSet<i64>> {
    if tags.is_empty() {
        return Ok(HashSet::new());
    }
    let marks = vec!["?"; tags.len()].join(",");
    let mut stmt = conn.prepare(&format!("SELECT DISTINCT file_id FROM file_tags WHERE tag_id IN ({marks})"))?;
    let ids = stmt.query_map(params_from_iter(tags), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

fn local_time(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    match Local.timestamp_opt(secs, 0).single() {
        Some(t) => t.format("%Y-%m-%dT%H:%M:%S").to_string(),
        None => "0000-01-01T00:00:00".into(),
    }
}

/// Everything the info panel shows about one file.
#[derive(Debug, Serialize)]
pub struct Details {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub folder_id: i64,
    pub kind: String,
    pub size: u64,
    pub taken: Option<String>,
    pub taken_offset: Option<String>,
    pub modified: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub camera: Option<String>,
    /// Folder tags first, then own tags.
    pub tags: Vec<FileTag>,
    pub missing: bool,
}

pub fn details(conn: &Connection, id: i64) -> Result<Option<Details>> {
    let d = conn
        .query_row(
            "SELECT id, name, path_nfc, folder_id, kind, size, taken, taken_offset, mtime_ns, width, height,
                    duration_ms, camera, missing_since IS NOT NULL
             FROM files WHERE id = ?1",
            [id],
            |r| {
                Ok(Details {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    folder_id: r.get(3)?,
                    kind: r.get(4)?,
                    size: r.get::<_, i64>(5)? as u64,
                    taken: r.get(6)?,
                    taken_offset: r.get(7)?,
                    modified: local_time(r.get(8)?),
                    width: r.get(9)?,
                    height: r.get(10)?,
                    duration_ms: r.get::<_, Option<i64>>(11)?.map(|d| d as u64),
                    camera: r.get(12)?,
                    tags: Vec::new(),
                    missing: r.get(13)?,
                })
            },
        )
        .optional()?;
    let Some(mut d) = d else { return Ok(None) };
    d.tags = conn
        .prepare(
            "SELECT t.id, t.name, ft.source FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE ft.file_id = ?1
             ORDER BY ft.source != 'folder', t.name",
        )?
        .query_map([id], |r| Ok(FileTag { id: r.get(0)?, name: r.get(1)?, source: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(d))
}
