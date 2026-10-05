//! Changes the user asks for: moving photos (with their RAW, Live Photo and
//! sidecar companions), renaming folders, and the trash.
//!
//! - Files are only ever renamed. On one drive `rename` keeps the content
//!   and every timestamp; nothing is copied, rewritten or replaced.
//! - A file moves only while it matches the index (size and modification
//!   time); anything else has to be looked at by a scan first.
//! - Names are compared NFC-normalised and ignoring case, as exFAT and APFS
//!   do, so nothing lands on an existing name in another spelling.
//!   Case-only renames go through a temporary name.
//! - A photo moves together with the files of the same name in its folder
//!   (RAW, Live Photo video, XMP/AAE sidecars), all or none.
//! - The disk changes first, then the index. If shoebox stops in between,
//!   the next scan recognises the move.
//! - "Deleting" moves files into `.shoebox/trash/` on the same drive; only
//!   emptying the trash removes them.
//! - Turning a JPEG is the one change that writes into an original, and only
//!   the two bytes of its EXIF Orientation tag, in place: nothing is copied
//!   or replaced, the picture data and the capture and creation dates stay.
//!   It must match the index first, and what is on disk afterwards must be
//!   exactly the old file with those two bytes changed (full hash).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use nom_exif::MediaParser;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::classify;
use crate::db;
use crate::fingerprint::{self, Stamp};
use crate::library::{self, RelPath};
use crate::media;
use crate::orientation;
use crate::scan::{self, Found};
use crate::tags;

/// Below `.shoebox/`.
pub const TRASH_DIR: &str = "trash";

/// Files that belong to a photo without being indexed: XMP (Lightroom,
/// darktable) and AAE (iOS edits) sidecars.
const SIDECAR_EXTENSIONS: &[&str] = &["xmp", "aae"];

/// Characters that exFAT, Windows or the Finder do not allow in names.
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

// ---------------------------------------------------------------- names

/// A file or folder name typed by the user, trimmed and NFC-normalised.
/// Refuses names that would be hidden, skipped by the scanner, or invalid
/// on exFAT.
pub fn check_name(name: &str) -> Result<String> {
    let name = library::nfc(name.trim());
    if name.is_empty() {
        bail!("the name is empty");
    }
    if name.len() > 255 {
        bail!("the name is too long");
    }
    if let Some(c) = name.chars().find(|c| c.is_control() || FORBIDDEN.contains(c)) {
        bail!("names cannot contain {c:?}");
    }
    if name.starts_with('.') {
        bail!("names cannot start with a dot");
    }
    if name.ends_with('.') {
        bail!("names cannot end with a dot");
    }
    if classify::is_ignored(&name) {
        bail!("{name} is a reserved name");
    }
    Ok(name)
}

/// A folder path relative to the root, `/`-separated; `""` is the root.
pub fn check_folder_path(path: &str) -> Result<String> {
    let parts: Vec<String> =
        path.split('/').filter(|p| !p.trim().is_empty()).map(check_name).collect::<Result<_>>()?;
    Ok(parts.join("/"))
}

/// How exFAT and APFS compare names: ignoring case and Unicode form.
pub fn fold(name: &str) -> String {
    library::nfc(name).to_lowercase()
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// The entries of one folder by folded name, to find what a new name would
/// collide with.
struct Names {
    by_fold: HashMap<String, String>,
}

impl Names {
    fn load(dir: &Path) -> io::Result<Names> {
        let mut by_fold = HashMap::new();
        for entry in fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            by_fold.insert(fold(&name), name);
        }
        Ok(Names { by_fold })
    }

    /// The existing entry that `name` collides with.
    fn get(&self, name: &str) -> Option<&str> {
        self.by_fold.get(&fold(name)).map(String::as_str)
    }

    fn insert(&mut self, name: &str) {
        self.by_fold.insert(fold(name), name.to_string());
    }

    fn remove(&mut self, name: &str) {
        self.by_fold.remove(&fold(name));
    }
}

/// `name`, or `stem (2).ext`, `stem (3).ext`, … if that is taken in `dir`.
pub(crate) fn free_name(dir: &Path, name: &str) -> Result<String> {
    let names = Names::load(dir)?;
    if names.get(name).is_none() {
        return Ok(name.to_string());
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    (2..10_000)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| names.get(candidate).is_none())
        .ok_or_else(|| anyhow!("no free name for {name}"))
}

// ---------------------------------------------------------------- renaming

/// `rename` that fails instead of replacing an existing `to`. Atomic where
/// the platform and filesystem support it (Linux `renameat2`, macOS
/// `renamex_np`); otherwise checked just before renaming. Callers check
/// for collisions in other spellings themselves (`Names`).
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let f = CString::new(from.as_os_str().as_bytes())?;
        let t = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both arguments are valid NUL-terminated strings.
        #[cfg(target_os = "linux")]
        let r = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) };
        #[cfg(target_os = "macos")]
        let r = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // Anything but "not supported here" is a real answer.
        // (ENOTSUP and EOPNOTSUPP are the same on Linux, not on macOS.)
        let unsupported = [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOSYS];
        if !e.raw_os_error().is_some_and(|c| unsupported.contains(&c)) {
            return Err(e);
        }
    }
    if to.symlink_metadata().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", to.display())));
    }
    fs::rename(from, to)
}

/// Rename `from` to `dir/name`, where `names` lists `dir`. A name that
/// differs from `from`'s only in case or Unicode form (same folder) goes
/// through a temporary name, as case-insensitive filesystems may otherwise
/// ignore the change.
fn rename_into(from: &Path, dir: &Path, name: &str, names: &mut Names) -> Result<PathBuf> {
    let to = dir.join(name);
    let from_name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match names.get(name).map(str::to_string) {
        Some(existing) if from.parent() == Some(dir) && existing == from_name => {
            if existing == name {
                return Ok(to); // nothing to do
            }
            let tmp = dir.join(format!(".shoebox-rename-{}", std::process::id()));
            rename_noreplace(from, &tmp).with_context(|| format!("rename {from_name}"))?;
            if let Err(e) = rename_noreplace(&tmp, &to) {
                let _ = fs::rename(&tmp, from);
                return Err(e).with_context(|| format!("rename {from_name} to {name}"));
            }
            names.remove(&existing);
        }
        Some(existing) => bail!("{existing} already exists there"),
        None => rename_noreplace(from, &to).with_context(|| format!("move {from_name}"))?,
    }
    names.insert(name);
    Ok(to)
}

// ---------------------------------------------------------------- folders

fn last_scan(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT coalesce(max(id), 0) FROM jobs WHERE kind = 'scan'", [], |r| r.get(0))?)
}

/// The folder at an NFC path, created on disk and in the index where
/// needed. A level that already exists in another spelling (case, Unicode
/// form) is used as it is. Returns the folder's id and its path as on disk.
pub fn ensure_folder(conn: &Connection, root: &Path, path_nfc: &str) -> Result<(i64, String)> {
    let mut raw = String::new();
    let mut chain = vec![RelPath { raw: String::new(), nfc: String::new() }];
    for part in path_nfc.split('/').filter(|p| !p.is_empty()) {
        let dir = root.join(&raw);
        let name = match Names::load(&dir)?.get(part) {
            Some(existing) => {
                if !dir.join(existing).is_dir() {
                    bail!("{} is a file, not a folder", join(&raw, existing));
                }
                existing.to_string()
            }
            None => {
                fs::create_dir(dir.join(part)).with_context(|| format!("create folder {}", join(&raw, part)))?;
                part.to_string()
            }
        };
        raw = join(&raw, &name);
        chain.push(RelPath { nfc: library::nfc(&raw), raw: raw.clone() });
    }
    let ids = scan::upsert_folders(conn, &chain, last_scan(conn)?)?;
    Ok((ids[&chain.last().expect("root is in the chain").nfc], raw))
}

#[derive(Debug, Serialize)]
pub struct RenamedFolder {
    pub id: i64,
    pub path: String,
    /// Indexed files below it, whose paths changed.
    pub files: u64,
}

/// Rename or move a folder (with everything in it) to a new NFC path.
pub fn rename_folder(conn: &Connection, root: &Path, folder_id: i64, new_path: &str) -> Result<RenamedFolder> {
    let (old_raw, old_nfc): (String, String) = conn
        .query_row("SELECT path, path_nfc FROM folders WHERE id = ?1", [folder_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .context("no such folder")?;
    if old_nfc.is_empty() {
        bail!("the library itself cannot be renamed");
    }
    let target = check_folder_path(new_path)?;
    if target.is_empty() {
        bail!("the name is empty");
    }
    if target == old_nfc {
        return Ok(RenamedFolder { id: folder_id, path: old_nfc, files: 0 });
    }
    if fold(&target).starts_with(&format!("{}/", fold(&old_nfc))) {
        bail!("a folder cannot be moved into itself");
    }
    let from = root.join(&old_raw);
    if !from.is_dir() {
        bail!("{old_nfc} is not on the drive any more; a scan will look for it");
    }
    let (parent_nfc, name) = target.rsplit_once('/').unwrap_or(("", &target));

    // Index rows already at the new place belong to folders and files that
    // are gone from the drive (the name is free there). Records of present
    // files mean the index is out of date.
    let folders: Vec<(i64, String, String)> = conn
        .prepare("SELECT id, path, path_nfc FROM folders")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let below = |p: &str, base: &str| p == base || p.starts_with(&format!("{base}/"));

    let (parent_id, parent_raw) = ensure_folder(conn, root, parent_nfc)?;
    let dir = root.join(&parent_raw);
    let new_raw = join(&parent_raw, name);
    let new_nfc = library::nfc(&new_raw);
    let mut stale: Vec<&(i64, String, String)> =
        folders.iter().filter(|f| below(&f.2, &new_nfc) && !below(&f.2, &old_nfc)).collect();
    for f in &stale {
        let present: i64 = conn.query_row(
            "SELECT count(*) FROM files WHERE folder_id = ?1 AND missing_since IS NULL",
            [f.0],
            |r| r.get(0),
        )?;
        if present > 0 {
            bail!("the index still lists files in {}; scan the library first", f.2);
        }
    }

    let mut names = Names::load(&dir)?;
    rename_into(&from, &dir, name, &mut names)?;

    let tx = conn.unchecked_transaction()?;
    stale.sort_by_key(|f| std::cmp::Reverse(f.2.len())); // children first
    for f in stale {
        tx.execute("DELETE FROM files WHERE folder_id = ?1", [f.0])?;
        tx.execute("DELETE FROM folders WHERE id = ?1", [f.0])?;
    }
    let moved: Vec<&(i64, String, String)> = folders.iter().filter(|f| below(&f.2, &old_nfc)).collect();
    let rebase = |raw: &str| -> Result<String> {
        let rest = raw.strip_prefix(old_raw.as_str()).context("folder paths in the index are inconsistent")?;
        Ok(format!("{new_raw}{rest}"))
    };
    for (id, raw, _) in &moved {
        let raw = rebase(raw)?;
        tx.execute("UPDATE folders SET path = ?2, path_nfc = ?3 WHERE id = ?1", params![id, raw, library::nfc(&raw)])?;
    }
    let event = library::parse_event(name);
    tx.execute(
        "UPDATE folders SET parent_id = ?2, name = ?3, event_year = ?4, event_month = ?5, event_name = ?6 WHERE id = ?1",
        params![
            folder_id,
            parent_id,
            library::nfc(name),
            event.as_ref().map(|e| e.year),
            event.as_ref().map(|e| e.month),
            event.as_ref().map(|e| e.name),
        ],
    )?;
    let ids: HashSet<i64> = moved.iter().map(|f| f.0).collect();
    let files: Vec<(i64, i64, String)> = tx
        .prepare("SELECT id, folder_id, path FROM files")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter(|row| row.as_ref().map_or(true, |(_, folder, _)| ids.contains(folder)))
        .collect::<rusqlite::Result<_>>()?;
    for (id, _, raw) in &files {
        let raw = rebase(raw)?;
        let rel = RelPath { nfc: library::nfc(&raw), raw };
        tx.execute("UPDATE files SET path = ?2, path_nfc = ?3 WHERE id = ?1", params![id, rel.raw, rel.nfc])?;
        scan::set_folder_tags(&tx, *id, &rel)?;
    }
    tx.commit()?;
    Ok(RenamedFolder { id: folder_id, path: new_nfc, files: files.len() as u64 })
}

// ---------------------------------------------------------------- files

/// The parts of a `files` row needed to move it.
#[derive(Debug, Clone)]
struct Rec {
    id: i64,
    path: String,
    path_nfc: String,
    folder_id: i64,
    kind: String,
    size: u64,
    mtime_ns: i64,
    quick_hash: String,
    full_hash: Option<String>,
    missing: bool,
}

const REC_COLUMNS: &str =
    "id, path, path_nfc, folder_id, kind, size, mtime_ns, quick_hash, full_hash, missing_since IS NOT NULL";

fn rec(r: &rusqlite::Row) -> rusqlite::Result<Rec> {
    Ok(Rec {
        id: r.get(0)?,
        path: r.get(1)?,
        path_nfc: r.get(2)?,
        folder_id: r.get(3)?,
        kind: r.get(4)?,
        size: r.get::<_, i64>(5)? as u64,
        mtime_ns: r.get(6)?,
        quick_hash: r.get(7)?,
        full_hash: r.get(8)?,
        missing: r.get(9)?,
    })
}

fn load(conn: &Connection, id: i64) -> Result<Option<Rec>> {
    Ok(conn.query_row(&format!("SELECT {REC_COLUMNS} FROM files WHERE id = ?1"), [id], rec).optional()?)
}

/// A photo and what belongs to it: indexed files with the same name stem in
/// the same folder (RAW, Live Photo video, other formats), and sidecars.
struct Group {
    /// The chosen file first.
    files: Vec<Rec>,
    /// Paths as on disk.
    sidecars: Vec<String>,
}

fn group_of(conn: &Connection, root: &Path, first: Rec) -> Result<Group> {
    let want = fold(stem(file_name(&first.path)));
    let mut files: Vec<Rec> = conn
        .prepare(&format!("SELECT {REC_COLUMNS} FROM files WHERE folder_id = ?1 AND id != ?2 AND missing_since IS NULL"))?
        .query_map(params![first.folder_id, first.id], rec)?
        .filter(|r| r.as_ref().map_or(true, |r| fold(stem(file_name(&r.path))) == want))
        .collect::<rusqlite::Result<_>>()?;
    files.sort_by(|a, b| a.path_nfc.cmp(&b.path_nfc));
    files.insert(0, first);

    let dir_raw = parent(&files[0].path).to_string();
    let mut sidecars = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join(&dir_raw)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            if SIDECAR_EXTENSIONS.contains(&ext.as_str())
                && fold(stem(&name)) == want
                && !classify::is_ignored(&name)
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                sidecars.push(join(&dir_raw, &name));
            }
        }
    }
    sidecars.sort();
    Ok(Group { files, sidecars })
}

impl Group {
    /// Check that every file is still what the index says, before anything
    /// is touched.
    fn check(&self, root: &Path) -> Result<()> {
        for f in &self.files {
            if f.missing {
                bail!("{} is missing from the drive", f.path_nfc);
            }
            let stamp = match fingerprint::stamp(&root.join(&f.path)) {
                Ok(s) => s,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    bail!("{} is not on the drive any more; a scan will look for it", f.path_nfc)
                }
                Err(e) => return Err(e).with_context(|| f.path_nfc.clone()),
            };
            if stamp.size != f.size || stamp.mtime_ns != f.mtime_ns as i128 {
                bail!("{} changed since the last scan; scan the library first", f.path_nfc);
            }
        }
        Ok(())
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|f| file_name(&f.path)).chain(self.sidecars.iter().map(|s| file_name(s)))
    }

    /// Load the group of each id (once per group, in the order given).
    fn all(conn: &Connection, root: &Path, ids: &[i64], skipped: &mut Vec<String>) -> Result<Vec<Group>> {
        let mut seen = HashSet::new();
        let mut groups = Vec::new();
        for &id in ids {
            if seen.contains(&id) {
                continue;
            }
            let Some(first) = load(conn, id)? else {
                skipped.push(format!("#{id}: not in the index"));
                continue;
            };
            let group = group_of(conn, root, first)?;
            seen.extend(group.files.iter().map(|f| f.id));
            groups.push(group);
        }
        Ok(groups)
    }
}

/// Point a record at its new path (and the folder tags that come with it).
fn set_path(conn: &Connection, id: i64, rel: &RelPath, folder_id: i64) -> Result<()> {
    // The record of a missing file may still hold the name; the file there
    // now is another one.
    conn.execute("DELETE FROM files WHERE path_nfc = ?1 AND id != ?2 AND missing_since IS NOT NULL", params![rel.nfc, id])?;
    conn.execute(
        "UPDATE files SET path = ?2, path_nfc = ?3, name = ?4, folder_id = ?5 WHERE id = ?1",
        params![id, rel.raw, rel.nfc, rel.name(), folder_id],
    )?;
    scan::set_folder_tags(conn, id, rel)
}

#[derive(Debug, Default, Serialize)]
pub struct Moved {
    /// New NFC paths of the indexed files that moved (companions included).
    pub files: Vec<String>,
    pub sidecars: u64,
    /// Photos that stayed where they were, with the reason.
    pub skipped: Vec<String>,
    /// Where they went.
    pub folder_id: i64,
    pub folder: String,
}

/// Move photos into a folder (NFC path, created if needed), each with its
/// companions. A photo whose group cannot move completely stays put.
pub fn move_files(conn: &Connection, root: &Path, ids: &[i64], folder: &str) -> Result<Moved> {
    move_files_with(conn, root, ids, folder, true)
}

/// As `move_files`; without `keep_tags` the photos' own tags are dropped
/// (folder tags always follow the new folder).
pub fn move_files_with(conn: &Connection, root: &Path, ids: &[i64], folder: &str, keep_tags: bool) -> Result<Moved> {
    let folder = check_folder_path(folder)?;
    let (folder_id, folder_raw) = ensure_folder(conn, root, &folder)?;
    let dir = root.join(&folder_raw);
    let mut names = Names::load(&dir)?;
    let mut out = Moved { folder_id, folder: library::nfc(&folder_raw), ..Default::default() };
    for group in Group::all(conn, root, ids, &mut out.skipped)? {
        if group.files[0].folder_id == folder_id {
            continue; // already there
        }
        let result = group.check(root).and_then(|()| {
            match group.names().find_map(|n| names.get(n)) {
                Some(existing) => bail!("{} already exists in {}", existing, out.folder),
                None => Ok(()),
            }
        });
        if let Err(e) = result.and_then(|()| move_group(conn, root, &group, folder_id, &folder_raw, keep_tags, &mut names, &mut out)) {
            out.skipped.push(format!("{}: {e:#}", group.files[0].path_nfc));
        }
    }
    Ok(out)
}

fn move_group(
    conn: &Connection,
    root: &Path,
    group: &Group,
    folder_id: i64,
    folder_raw: &str,
    keep_tags: bool,
    names: &mut Names,
    out: &mut Moved,
) -> Result<()> {
    let dir = root.join(folder_raw);
    let tx = conn.unchecked_transaction()?;
    // Whatever happened on disk is committed, even if a later step fails.
    let result = (|| -> Result<()> {
        for f in &group.files {
            let name = file_name(&f.path);
            let from = root.join(&f.path);
            let before = fingerprint::stamp(&from)?;
            let to = rename_into(&from, &dir, name, names)?;
            let raw = join(folder_raw, name);
            let rel = RelPath { nfc: library::nfc(&raw), raw };
            set_path(&tx, f.id, &rel, folder_id)?;
            if !keep_tags {
                crate::tags::drop_own(&tx, f.id)?;
            }
            out.files.push(rel.nfc);
            check_kept(&before, &to)?;
        }
        for s in &group.sidecars {
            rename_into(&root.join(s), &dir, file_name(s), names)?;
            out.sidecars += 1;
        }
        Ok(())
    })();
    tx.commit()?;
    result
}

/// After a rename: size and timestamps must be what they were.
fn check_kept(before: &Stamp, path: &Path) -> Result<()> {
    let after = fingerprint::stamp(path)?;
    if &after != before {
        bail!("{}: the filesystem changed its timestamps while moving it", path.display());
    }
    Ok(())
}

/// Index a file that just appeared (an import or a restore from the trash),
/// with its full hash when the caller already knows it.
pub(crate) fn index_file(conn: &Connection, root: &Path, raw: &str, full_hash: Option<&str>) -> Result<i64> {
    let path = root.join(raw);
    let rel = RelPath::new(root, &path).context("not inside the library")?;
    let kind = classify::kind_of(&path).context("not a photo or video")?;
    let stamp = fingerprint::stamp(&path)?;
    let (folder_id, _) = ensure_folder(conn, root, rel.parent_nfc())?;
    let found = Found { rel, kind, stamp };
    let info = scan::read_file(&mut MediaParser::new(), &path, &found, None).map_err(|e| anyhow!("{raw}: {e}"))?;
    conn.execute("DELETE FROM files WHERE path_nfc = ?1 AND missing_since IS NOT NULL", [&found.rel.nfc])?;
    let id = scan::insert_file(conn, &found, folder_id, &info)?;
    if let Some(h) = full_hash {
        conn.execute("UPDATE files SET full_hash = ?2 WHERE id = ?1", params![id, h])?;
    }
    scan::set_folder_tags(conn, id, &found.rel)?;
    Ok(id)
}

// ---------------------------------------------------------------- rotate

/// What turning a photo changed.
#[derive(Debug, Serialize)]
pub struct Rotated {
    pub id: i64,
    /// The first 8 characters of the new quick hash: what the web page puts
    /// on the picture addresses.
    pub version: String,
    /// The EXIF orientation stored now (1 to 8).
    pub orientation: u8,
}

/// Larger files are not read into memory to be checked.
const ROTATE_MAX_BYTES: u64 = 256 << 20;

/// Turn a JPEG by `quarters` quarter turns clockwise (negative:
/// counter-clockwise) by changing its EXIF Orientation tag in place.
///
/// Like the Finder's Quick Look this is lossless and quick, and it is the
/// only time shoebox writes into an original: two bytes, no copy, no
/// replacement. The file must still match the index, carry an Orientation
/// tag already (adding one would move every byte after it), and afterwards
/// hash to exactly the old content with those two bytes changed; if not, the
/// old bytes are put back. The modification time is the file system's own
/// doing, as in the Finder; the creation time must not change.
pub fn rotate(conn: &Connection, root: &Path, id: i64, quarters: i32) -> Result<Rotated> {
    let quarters = quarters.rem_euclid(4);
    if quarters == 0 {
        bail!("nothing to turn");
    }
    let rec = load(conn, id)?.ok_or_else(|| anyhow!("not in the index"))?;
    let name = rec.path_nfc.as_str();
    if rec.missing {
        bail!("{name} is missing from the drive");
    }
    let kind = classify::Kind::parse(&rec.kind).ok_or_else(|| anyhow!("{name}: unknown kind"))?;
    let path = root.join(&rec.path);
    // A JPEG called `.HEIC` is a JPEG; a HEIC, PNG, RAW file or video has no
    // Orientation tag that is safe to change in place.
    if media::content_kind(kind, &path) != classify::Kind::Jpeg {
        bail!("{name}: only JPEG photos can be turned for now");
    }
    if rec.size > ROTATE_MAX_BYTES {
        bail!("{name} is too large to turn");
    }

    let before = fingerprint::stamp(&path).with_context(|| name.to_string())?;
    let bytes = fingerprint::read_unchanged(&path, rec.size, rec.mtime_ns, || fs::read(&path).map_err(|e| e.to_string()))
        .map_err(|e| anyhow!("{name}: {e}; scan the library first"))?;
    if let Some(stored) = &rec.full_hash
        && blake3::hash(&bytes).to_hex().as_str() != stored
    {
        bail!("{name} differs from the index (hash); check it before changing it");
    }

    let slot = orientation::find(&bytes).map_err(|e| anyhow!("{name}: {e:#}"))?;
    let value = orientation::turned(slot.value, quarters);
    let old_bytes = [bytes[slot.offset], bytes[slot.offset + 1]];
    let new_bytes = slot.bytes(value);
    let mut expected = bytes;
    expected[slot.offset..slot.offset + 2].copy_from_slice(&new_bytes);
    let expected_hash = blake3::hash(&expected).to_hex().to_string();
    drop(expected);

    patch(&path, slot.offset as u64, &new_bytes).with_context(|| format!("{name}: could not write"))?;
    let verified = (|| -> Result<fingerprint::Stamp> {
        let after = fingerprint::stamp(&path)?;
        if after.size != before.size || after.created_ns != before.created_ns {
            bail!("the drive changed the size or the creation date");
        }
        if fingerprint::full_hash(&path)? != expected_hash {
            bail!("the file reads differently from what was written");
        }
        Ok(after)
    })();
    let stamp = match verified {
        Ok(s) => s,
        Err(e) => {
            let restored = patch(&path, slot.offset as u64, &old_bytes);
            bail!("{name}: {e:#}; {}", if restored.is_ok() { "the old bytes were put back" } else { "putting the old bytes back failed too" });
        }
    };

    let rel = RelPath::new(root, &path).context("not inside the library")?;
    let found = Found { rel, kind, stamp };
    let info = scan::read_file(&mut MediaParser::new(), &path, &found, None).map_err(|e| anyhow!("{name}: {e}"))?;
    let tx = conn.unchecked_transaction()?;
    scan::update_file(&tx, id, &found, &info)?;
    tx.execute("UPDATE files SET full_hash = ?2 WHERE id = ?1", params![id, expected_hash])?;
    let key: String = tx.query_row("SELECT quick_hash FROM files WHERE id = ?1", [id], |r| r.get(0))?;
    follow_content(&tx, &rec.quick_hash, &key, quarters)?;
    tx.commit()?;
    Ok(Rotated { id, version: key.chars().take(8).collect(), orientation: value })
}

/// Overwrite `bytes.len()` bytes at `offset`, in place.
fn patch(path: &Path, offset: u64, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// A box (x, y, w, h as fractions of the upright picture) after the picture
/// was turned by `quarters` quarter turns clockwise.
fn turn_box(b: [f64; 4], quarters: i32) -> [f64; 4] {
    let [x, y, w, h] = b;
    match quarters.rem_euclid(4) {
        1 => [1.0 - y - h, x, h, w],
        2 => [1.0 - x - w, 1.0 - y - h, w, h],
        3 => [y, 1.0 - x - w, h, w],
        _ => b,
    }
}

/// Things the user decided are keyed by the content's quick hash, which
/// changed with the tag: the capture date they set and who is where on the
/// photo (turned with the picture) follow the photo to its new key. What the
/// other copies of the old content still use stays with them. What is only
/// cached (thumbnails, detected faces) is made again: the next view and the
/// next `shoebox recognize` do that, and prune the old.
fn follow_content(conn: &Connection, old: &str, new: &str, quarters: i32) -> Result<()> {
    let has_table = |name: &str| -> Result<bool> {
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [name], |r| r.get(0))?)
    };
    let still_used: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE quick_hash = ?1) OR EXISTS(SELECT 1 FROM trash WHERE quick_hash = ?1)",
        [old],
        |r| r.get(0),
    )?;
    let same = old == new;

    if has_table("face_decisions")? {
        let rows: Vec<(i64, [f64; 4])> = conn
            .prepare("SELECT id, x, y, w, h FROM face_decisions WHERE key = ?1")?
            .query_map([old], |r| Ok((r.get(0)?, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?])))?
            .collect::<rusqlite::Result<_>>()?;
        for (row, b) in rows {
            let [x, y, w, h] = turn_box(b, quarters);
            if same {
                conn.execute("UPDATE face_decisions SET x = ?2, y = ?3, w = ?4, h = ?5 WHERE id = ?1", params![row, x, y, w, h])?;
            } else {
                conn.execute(
                    "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
                     SELECT ?2, ?3, ?4, ?5, ?6, person_id, decision, manual, at FROM face_decisions WHERE id = ?1",
                    params![row, new, x, y, w, h],
                )?;
            }
        }
        if !same && !still_used {
            conn.execute("DELETE FROM face_decisions WHERE key = ?1 AND id NOT IN (SELECT id FROM face_decisions WHERE key = ?2)", params![old, new])?;
        }
    }
    if !same && has_table("taken_overrides")? {
        conn.execute(
            "INSERT OR REPLACE INTO taken_overrides (key, taken, taken_offset, at)
             SELECT ?2, taken, taken_offset, at FROM taken_overrides WHERE key = ?1",
            params![old, new],
        )?;
        if !still_used {
            conn.execute("DELETE FROM taken_overrides WHERE key = ?1", [old])?;
        }
    }
    if !same && !still_used && has_table("people")? {
        let covers: Vec<(i64, String)> = conn
            .prepare("SELECT id, cover_box FROM people WHERE cover_key = ?1 AND cover_box IS NOT NULL")?
            .query_map([old], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (person, json) in covers {
            let Ok(b) = serde_json::from_str::<[f64; 4]>(&json) else { continue };
            conn.execute(
                "UPDATE people SET cover_key = ?2, cover_box = ?3 WHERE id = ?1",
                params![person, new, serde_json::to_string(&turn_box(b, quarters))?],
            )?;
        }
    }
    // The cached picture of the old content, if nothing uses it any more, or
    // of the new one when the key did not change although the picture did.
    // (Not every connection has the thumbnail database attached.)
    if same || !still_used {
        let _ = conn.execute("DELETE FROM thumbs.thumbs WHERE key = ?1", [old]);
    }
    Ok(())
}

// ---------------------------------------------------------------- trash

pub fn trash_dir(root: &Path) -> PathBuf {
    root.join(db::DIR).join(TRASH_DIR)
}

#[derive(Debug, Default, Serialize)]
pub struct Trashed {
    /// One batch per photo (with its companions).
    pub batches: Vec<i64>,
    /// Indexed files that went to the trash.
    pub files: Vec<String>,
    pub sidecars: u64,
    pub skipped: Vec<String>,
}

/// Move photos and their companions into `.shoebox/trash/<batch>/`. Their
/// records go (with tags and duplicate decisions); the trash table keeps
/// what is needed to put them back, own tags included.
pub fn trash_files(conn: &Connection, root: &Path, ids: &[i64]) -> Result<Trashed> {
    let mut out = Trashed::default();
    for group in Group::all(conn, root, ids, &mut out.skipped)? {
        let first = group.files[0].path_nfc.clone();
        if let Err(e) = group.check(root).and_then(|()| trash_group(conn, root, &group, &mut out)) {
            out.skipped.push(format!("{first}: {e:#}"));
        }
    }
    Ok(out)
}

fn trash_group(conn: &Connection, root: &Path, group: &Group, out: &mut Trashed) -> Result<()> {
    // Skip numbers whose folder is left over (an earlier attempt that failed).
    let mut batch: i64 = conn.query_row("SELECT coalesce(max(batch), 0) + 1 FROM trash", [], |r| r.get(0))?;
    while trash_dir(root).join(batch.to_string()).exists() {
        batch += 1;
    }
    let dir = trash_dir(root).join(batch.to_string());
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let mut names = Names::load(&dir)?;
    let tx = conn.unchecked_transaction()?;
    let result = (|| -> Result<()> {
        for f in &group.files {
            let name = file_name(&f.path);
            let from = root.join(&f.path);
            let before = fingerprint::stamp(&from)?;
            let to = rename_into(&from, &dir, name, &mut names)?;
            tx.execute(
                "INSERT INTO trash (batch, path, path_nfc, stored, kind, size, mtime_ns, quick_hash, full_hash, deleted_at, user_tags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    batch,
                    f.path,
                    f.path_nfc,
                    format!("{batch}/{name}"),
                    f.kind,
                    f.size as i64,
                    f.mtime_ns,
                    f.quick_hash,
                    f.full_hash,
                    db::now(),
                    tags::own_names_json(&tx, f.id)?
                ],
            )?;
            tx.execute("DELETE FROM file_tags WHERE file_id = ?1", [f.id])?;
            tx.execute("DELETE FROM dup_decisions WHERE a = ?1 OR b = ?1", [f.id])?;
            tx.execute("DELETE FROM files WHERE id = ?1", [f.id])?;
            out.files.push(f.path_nfc.clone());
            check_kept(&before, &to)?;
        }
        for s in &group.sidecars {
            let from = root.join(s);
            let stamp = fingerprint::stamp(&from)?;
            let name = file_name(s);
            rename_into(&from, &dir, name, &mut names)?;
            tx.execute(
                "INSERT INTO trash (batch, path, path_nfc, stored, size, mtime_ns, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![batch, s, library::nfc(s), format!("{batch}/{name}"), stamp.size as i64, ns(stamp.mtime_ns), db::now()],
            )?;
            out.sidecars += 1;
        }
        Ok(())
    })();
    tx.commit()?;
    out.batches.push(batch);
    result
}

fn ns(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashItem {
    pub id: i64,
    pub batch: i64,
    /// Where it was (NFC).
    pub path: String,
    /// `None` for sidecar files.
    pub kind: Option<String>,
    pub size: u64,
    pub deleted_at: i64,
    #[serde(skip)]
    pub quick_hash: Option<String>,
}

/// Everything in the trash, most recently deleted first.
pub fn trash_list(conn: &Connection) -> Result<Vec<TrashItem>> {
    let items = conn
        .prepare("SELECT id, batch, path_nfc, kind, size, deleted_at, quick_hash FROM trash ORDER BY batch DESC, kind IS NULL, path_nfc")?
        .query_map([], |r| {
            Ok(TrashItem {
                id: r.get(0)?,
                batch: r.get(1)?,
                path: r.get(2)?,
                kind: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                deleted_at: r.get(5)?,
                quick_hash: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(items)
}

struct TrashRow {
    id: i64,
    path: String,
    path_nfc: String,
    stored: String,
    kind: Option<String>,
    size: u64,
    mtime_ns: i64,
    full_hash: Option<String>,
    user_tags: Option<String>,
}

fn trash_rows(conn: &Connection, batch: Option<i64>) -> Result<Vec<TrashRow>> {
    let rows = conn
        .prepare(
            "SELECT id, path, path_nfc, stored, kind, size, mtime_ns, full_hash, user_tags FROM trash
             WHERE ?1 IS NULL OR batch = ?1 ORDER BY kind IS NULL, path_nfc",
        )?
        .query_map([batch], |r| {
            Ok(TrashRow {
                id: r.get(0)?,
                path: r.get(1)?,
                path_nfc: r.get(2)?,
                stored: r.get(3)?,
                kind: r.get(4)?,
                size: r.get::<_, i64>(5)? as u64,
                mtime_ns: r.get(6)?,
                full_hash: r.get(7)?,
                user_tags: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Stored trash paths are `<batch>/<name>`; refuse anything else.
fn stored_path(root: &Path, stored: &str) -> Result<PathBuf> {
    let ok = Path::new(stored).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !ok {
        bail!("bad trash entry {stored}");
    }
    Ok(trash_dir(root).join(stored))
}

#[derive(Debug, Default, Serialize)]
pub struct Restored {
    /// NFC paths of the files back in the library (sidecars included).
    pub files: Vec<String>,
    /// Ids of the indexed files (new records).
    pub ids: Vec<i64>,
}

/// Put one batch back where it was, with the files' own tags. Fails without
/// moving anything if one of the places is taken by now.
pub fn restore(conn: &Connection, root: &Path, batch: i64) -> Result<Restored> {
    let rows = trash_rows(conn, Some(batch))?;
    if rows.is_empty() {
        bail!("nothing in the trash with that number");
    }
    let mut targets = Vec::new();
    for r in &rows {
        let stored = stored_path(root, &r.stored)?;
        let stamp = fingerprint::stamp(&stored).with_context(|| format!("{} is gone from the trash folder", r.path_nfc))?;
        let (_, dir_raw) = ensure_folder(conn, root, parent(&r.path_nfc))?;
        let names = Names::load(&root.join(&dir_raw))?;
        if let Some(existing) = names.get(file_name(&r.path)) {
            bail!("{} is taken by another file now", join(&dir_raw, existing));
        }
        let unchanged = stamp.size == r.size && stamp.mtime_ns == r.mtime_ns as i128;
        targets.push((stored, dir_raw, unchanged));
    }
    let mut out = Restored::default();
    let tx = conn.unchecked_transaction()?;
    let result = (|| -> Result<()> {
        for (r, (stored, dir_raw, unchanged)) in rows.iter().zip(&targets) {
            let raw = join(dir_raw, file_name(&r.path));
            rename_noreplace(stored, &root.join(&raw)).with_context(|| format!("restore {}", r.path_nfc))?;
            tx.execute("DELETE FROM trash WHERE id = ?1", [r.id])?;
            if r.kind.is_some() {
                let hash = r.full_hash.as_deref().filter(|_| *unchanged);
                let id = index_file(&tx, root, &raw, hash)?;
                if let Some(names) = &r.user_tags {
                    tags::restore_names_json(&tx, id, names)?;
                }
                out.ids.push(id);
            }
            out.files.push(library::nfc(&raw));
        }
        Ok(())
    })();
    tx.commit()?;
    let _ = fs::remove_dir(trash_dir(root).join(batch.to_string()));
    result.map(|()| out)
}

/// Delete for good: one batch, or everything in the trash.
pub fn empty_trash(conn: &Connection, root: &Path, batch: Option<i64>) -> Result<u64> {
    let rows = trash_rows(conn, batch)?;
    let mut batches = HashSet::new();
    let tx = conn.unchecked_transaction()?;
    for r in &rows {
        match fs::remove_file(stored_path(root, &r.stored)?) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("delete {}", r.stored)),
        }
        tx.execute("DELETE FROM trash WHERE id = ?1", [r.id])?;
        batches.insert(r.stored.split('/').next().unwrap_or_default().to_string());
    }
    tx.commit()?;
    for b in batches {
        // Only what we put there, plus Finder bookkeeping (.DS_Store).
        let dir = trash_dir(root).join(&b);
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                if classify::is_ignored(&e.file_name().to_string_lossy()) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        let _ = fs::remove_dir(&dir);
    }
    Ok(rows.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  Urlaub ").unwrap(), "Urlaub");
        assert_eq!(check_name("O\u{308}sterreich").unwrap(), "\u{d6}sterreich");
        for bad in ["", " ", "a/b", "a:b", ".hidden", "dots.", "._x", ".shoebox", "Thumbs.db", "tab\there"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check_folder_path("/Familie / 2020-07 Urlaub/").unwrap(), "Familie/2020-07 Urlaub");
        assert_eq!(check_folder_path("").unwrap(), "");
        assert!(check_folder_path("a/../b").is_err());
    }

    #[test]
    fn folding_ignores_case_and_unicode_form() {
        assert_eq!(fold("O\u{308}STERREICH"), fold("\u{f6}sterreich"));
        assert_ne!(fold("a"), fold("b"));
    }

    #[test]
    fn free_names_get_a_number() {
        let dir = std::env::temp_dir().join(format!("shoebox-names-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_name(&dir, "IMG_1.JPG").unwrap(), "IMG_1.JPG");
        fs::write(dir.join("img_1.jpg"), b"x").unwrap();
        assert_eq!(free_name(&dir, "IMG_1.JPG").unwrap(), "IMG_1 (2).JPG");
        fs::write(dir.join("IMG_1 (2).JPG"), b"x").unwrap();
        assert_eq!(free_name(&dir, "IMG_1.JPG").unwrap(), "IMG_1 (3).JPG");

        // No replacing, whatever the platform.
        fs::write(dir.join("a"), b"a").unwrap();
        fs::write(dir.join("b"), b"b").unwrap();
        assert!(rename_noreplace(&dir.join("a"), &dir.join("b")).is_err());
        assert_eq!(fs::read(dir.join("b")).unwrap(), b"b");
        fs::remove_dir_all(&dir).unwrap();
    }
}
