//! Export (phase 13): copies of chosen photos and videos in a new folder
//! somewhere else, for a print shop, a layout program or another person.
//!
//! Originals are only read, and every read is under the guard: the file must
//! still match the index (size, modified time), must not change while it is
//! read, is hashed while it is copied and the copy is hashed again from the
//! destination. Nothing is ever written next to an original, in the library or
//! in its index.
//!
//! What goes into the folder, in capture order:
//!
//! - the photos and videos asked for, each as a byte-identical copy with the
//!   original's modified time; a HEIC can become a JPEG (with its EXIF and
//!   colour profile) for programs that cannot read HEIC;
//! - with the options: the RAW file and the video part of a Live Photo that
//!   belong to a photo, under the number and name of that photo;
//! - names are `001_IMG_3457.jpg` (the number can be switched off).
//!
//! An existing folder is merged, never replaced or emptied: what is already
//! there (same name without the number, same size) is skipped, the numbers
//! of the files that are there are changed to the new order, and files that
//! are not ours stay as they are.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use libheif_rs::{HeifContext, LibHeif};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::classify::Kind;
use crate::{db, fingerprint, library, media, organize};

/// A Live Photo's video is at most this long (as in the timeline).
const LIVE_MAX_MS: i64 = 6_000;
/// Quality of a JPEG made from a HEIC.
const JPEG_QUALITY: u8 = 95;
/// Copy in pieces of this size, so Stop is quick.
const CHUNK: usize = 1024 * 1024;
const PART: &str = ".part";
const TEMP: &str = ".shoebox-renaming";

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct Options {
    /// `001_` in front of every name.
    #[serde(default)]
    pub number: bool,
    /// Take the RAW file of a photo along.
    #[serde(default)]
    pub raw: bool,
    /// Take the video part of a Live Photo along.
    #[serde(default)]
    pub live: bool,
    /// Make JPEGs of HEIC photos.
    #[serde(default)]
    pub heic_to_jpeg: bool,
}

// ---------------------------------------------------------------- what is asked for

#[derive(Debug, Clone)]
struct Src {
    rel: String,
    name: String,
    kind: Kind,
    size: u64,
    mtime_ns: i64,
    full_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    main: Src,
    /// Quarter turns clockwise the user gave a HEIC or PNG in shoebox only.
    turn: i32,
    raw: Option<Src>,
    live: Option<Src>,
}

fn src_of(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Option<(Src, Option<i64>)>> {
    let Some(kind) = Kind::parse(&r.get::<_, String>(at + 2)?) else { return Ok(None) };
    Ok(Some((
        Src {
            rel: r.get(at)?,
            name: r.get(at + 1)?,
            kind,
            size: r.get::<_, i64>(at + 3)? as u64,
            mtime_ns: r.get(at + 4)?,
            full_hash: r.get(at + 5)?,
        },
        r.get(at + 6)?,
    )))
}

fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// The files for `ids`, in capture order, with their RAW and Live Photo partners.
/// Ids that are not (or no longer) in the index are listed in `skipped`.
fn load(conn: &Connection, ids: &[i64]) -> Result<(Vec<Entry>, Vec<String>)> {
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    let mut found: Vec<(String, String, Entry, i64)> = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT path, name, kind, size, mtime_ns, full_hash, duration_ms,
                coalesce({taken}, strftime('%Y-%m-%dT%H:%M:%S', mtime_ns / 1000000000, 'unixepoch', 'localtime')),
                folder_id, lower(path_nfc),
                coalesce((SELECT quarters FROM view_turns v WHERE v.key = files.quick_hash), 0)
         FROM files WHERE id = ?1 AND missing_since IS NULL AND kind != 'raw'",
        taken = db::TAKEN
    ))?;
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        let row = stmt.query_row([id], |r| {
            let Some((main, _)) = src_of(r, 0)? else { return Ok(None) };
            Ok(Some((main, r.get::<_, String>(7)?, r.get::<_, i64>(8)?, r.get::<_, String>(9)?, r.get::<_, i32>(10)?)))
        });
        match row {
            Ok(Some((main, taken, folder, path_lower, turn))) => {
                found.push((taken, path_lower, Entry { main, turn, raw: None, live: None }, folder));
            }
            Ok(None) | Err(rusqlite::Error::QueryReturnedNoRows) => skipped.push(format!("#{id}: not in the index")),
            Err(e) => return Err(e.into()),
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    // Partners: same folder, same stem. One look at each folder.
    let folders: HashSet<i64> = found.iter().map(|f| f.3).collect();
    let mut partners: HashMap<(i64, String), (Option<Src>, Option<Src>)> = HashMap::new();
    let mut by_folder = conn.prepare(
        "SELECT path, name, kind, size, mtime_ns, full_hash, duration_ms FROM files
         WHERE folder_id = ?1 AND missing_since IS NULL AND kind IN ('raw', 'video')",
    )?;
    for folder in folders {
        let mut rows = by_folder.query([folder])?;
        while let Some(r) = rows.next()? {
            let Some((src, duration)) = src_of(r, 0)? else { continue };
            let slot = partners.entry((folder, organize::fold(stem_of(&src.name)))).or_default();
            match src.kind {
                Kind::Raw if slot.0.is_none() => slot.0 = Some(src),
                Kind::Video if slot.1.is_none() && duration.is_some_and(|d| d <= LIVE_MAX_MS) => slot.1 = Some(src),
                _ => {}
            }
        }
    }
    let entries = found
        .into_iter()
        .map(|(_, _, mut e, folder)| {
            if let Some((raw, live)) = partners.get(&(folder, organize::fold(stem_of(&e.main.name)))) {
                e.raw = raw.clone();
                // Only a still has a Live Photo video.
                if matches!(e.main.kind, Kind::Jpeg | Kind::Heic) {
                    e.live = live.clone();
                }
            }
            e
        })
        .collect();
    Ok((entries, skipped))
}

/// What the dialog shows before anything is chosen: counts and sizes.
#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub photos: u64,
    pub videos: u64,
    pub heic: u64,
    pub raw: u64,
    pub live: u64,
    /// The photos and videos themselves, without RAW and Live Photo videos.
    pub bytes: u64,
    pub raw_bytes: u64,
    pub live_bytes: u64,
    pub skipped: Vec<String>,
}

pub fn summarize(conn: &Connection, ids: &[i64]) -> Result<Summary> {
    let (entries, skipped) = load(conn, ids)?;
    let mut s = Summary { skipped, ..Summary::default() };
    for e in &entries {
        if e.main.kind == Kind::Video {
            s.videos += 1;
        } else {
            s.photos += 1;
        }
        if e.main.kind == Kind::Heic {
            s.heic += 1;
        }
        s.bytes += e.main.size;
        if let Some(raw) = &e.raw {
            s.raw += 1;
            s.raw_bytes += raw.size;
        }
        if let Some(live) = &e.live {
            s.live += 1;
            s.live_bytes += live.size;
        }
    }
    Ok(s)
}

// ---------------------------------------------------------------- names

/// Characters the common file systems (exFAT, FAT32, NTFS) do not allow.
fn bad_char(c: char) -> bool {
    c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// A file name that every common file system takes: forbidden characters
/// become `_`, no trailing dots or spaces, at most 200 characters.
pub fn safe_name(name: &str) -> String {
    let mut s: String = library::nfc(name).chars().map(|c| if bad_char(c) { '_' } else { c }).collect();
    if s.chars().count() > 200 {
        let (stem, ext) = match s.rsplit_once('.') {
            Some((a, b)) if b.chars().count() <= 10 => (a.to_string(), format!(".{b}")),
            _ => (s.clone(), String::new()),
        };
        let keep = 200usize.saturating_sub(ext.chars().count());
        s = stem.chars().take(keep).collect::<String>() + &ext;
    }
    let trimmed = s.trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() { "_".to_string() } else { trimmed }
}

/// The name of the new folder, as typed: refused (not changed) when a file
/// system would not take it.
pub fn check_folder_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the new folder a name");
    }
    if name == "." || name == ".." || name.chars().any(bad_char) || name.ends_with('.') {
        bail!("the folder name cannot have these characters: \\ / : * ? \" < > | (or end with a dot)");
    }
    if name.chars().count() > 200 {
        bail!("the folder name is too long");
    }
    Ok(library::nfc(name))
}

fn with_suffix(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}_{n}.{ext}"),
        _ => format!("{name}_{n}"),
    }
}

/// `001_` and so on: at least three digits.
fn prefix(n: usize, total: usize) -> String {
    let width = total.to_string().len().max(3);
    format!("{n:0width$}_")
}

/// The name without a leading `001_`.
fn without_number(name: &str) -> Option<&str> {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    (digits >= 3 && name.as_bytes().get(digits) == Some(&b'_') && name.len() > digits + 1).then(|| &name[digits + 1..])
}

// ---------------------------------------------------------------- the plan

#[derive(Debug, Clone)]
struct Task {
    src: Src,
    turn: i32,
    /// Name in the destination folder.
    target: String,
    convert: bool,
    /// Already in the folder: this file, which may need a new number.
    existing: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    /// Photos and videos asked for (partners not counted).
    pub total: usize,
    /// Files to be written.
    pub new: usize,
    /// Files that are in the folder already.
    pub already: usize,
    /// Of the new ones, HEIC made JPEG.
    pub converted: usize,
    /// Files in the folder whose number changes.
    pub renamed: usize,
    /// Bytes to be written (the size of the originals).
    pub bytes: u64,
    /// Files the folder has now.
    pub existing: usize,
}

pub struct Plan {
    tasks: Vec<Task>,
    /// (current name, new name) for files already in the folder.
    renames: Vec<(String, String)>,
    pub counts: Counts,
    pub skipped: Vec<String>,
}

/// The files directly in `dir` (name, size); nothing when it does not exist.
fn listing(dir: &Path) -> Result<Vec<(String, u64)>> {
    let Ok(read) = fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut files = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(PART) {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            files.push((library::nfc(&name), meta.len()));
        }
    }
    files.sort();
    Ok(files)
}

/// Work out what an export of `ids` into `dest` would do, without doing it.
pub fn plan(conn: &Connection, ids: &[i64], opts: Options, dest: &Path) -> Result<Plan> {
    let (entries, skipped) = load(conn, ids)?;
    let total = entries.len();
    let present = listing(dest)?;
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (name, _)) in present.iter().enumerate() {
        by_name.entry(organize::fold(name)).or_default().push(i);
        if let Some(rest) = without_number(name) {
            by_name.entry(organize::fold(rest)).or_default().push(i);
        }
    }
    let mut used = vec![false; present.len()];
    // Every name that is taken or will be: a new file never gets one of these.
    let mut taken: HashSet<String> = present.iter().map(|(n, _)| organize::fold(n)).collect();

    let mut tasks: Vec<Task> = Vec::new();
    let mut counts = Counts { total, existing: present.len(), ..Counts::default() };
    for (i, e) in entries.iter().enumerate() {
        let number = if opts.number { prefix(i + 1, total) } else { String::new() };
        let mut parts: Vec<(&Src, bool)> = vec![(&e.main, opts.heic_to_jpeg && e.main.kind == Kind::Heic)];
        if opts.raw && let Some(raw) = &e.raw {
            parts.push((raw, false));
        }
        if opts.live && let Some(live) = &e.live {
            parts.push((live, false));
        }
        for (src, convert) in parts {
            let name = safe_name(&src.name);
            let plain = if convert { format!("{}.jpg", stem_of(&name)) } else { name.clone() };
            // Is it there already? The same name without the number and the
            // same size; a converted HEIC only by its name (as .jpg or .jpeg).
            let mut keys: Vec<String> = if convert {
                vec![organize::fold(&plain), organize::fold(&format!("{}.jpeg", stem_of(&plain)))]
            } else {
                vec![organize::fold(&plain)]
            };
            // Without numbers, a second photo of the same name got a suffix the last time.
            if !convert && !opts.number {
                keys.extend((2..=20).map(|n| organize::fold(&with_suffix(&plain, n))));
            }
            let hit = keys.iter().find_map(|k| {
                by_name.get(k)?.iter().copied().find(|&j| !used[j] && (convert || present[j].1 == src.size))
            });
            let target = if let Some(j) = hit {
                used[j] = true;
                if opts.number { format!("{number}{plain}") } else { present[j].0.clone() }
            } else {
                let mut target = format!("{number}{plain}");
                let mut n = 2;
                while taken.contains(&organize::fold(&target)) {
                    target = format!("{number}{}", with_suffix(&plain, n));
                    n += 1;
                }
                target
            };
            taken.insert(organize::fold(&target));
            let existing = hit.map(|j| present[j].0.clone());
            match &existing {
                Some(_) => counts.already += 1,
                None => {
                    counts.new += 1;
                    counts.bytes += src.size;
                    counts.converted += usize::from(convert);
                }
            }
            tasks.push(Task { src: src.clone(), turn: e.turn, target, convert, existing });
        }
    }

    // New numbers for what is there: only where the name is free after the
    // others have moved; a file whose place is held by something else stays.
    let mut renames: Vec<(String, String)> = Vec::new();
    let moving: HashSet<String> = tasks
        .iter()
        .filter_map(|t| t.existing.as_ref().filter(|old| **old != t.target).map(|old| organize::fold(old)))
        .collect();
    let mut occupied: HashSet<String> = present.iter().map(|(n, _)| organize::fold(n)).filter(|n| !moving.contains(n)).collect();
    for t in &mut tasks {
        let Some(old) = t.existing.clone() else { continue };
        if old == t.target {
            continue;
        }
        let want = organize::fold(&t.target);
        if occupied.contains(&want) {
            t.target = old; // stays as it is
        } else {
            occupied.insert(want);
            renames.push((old, t.target.clone()));
        }
    }
    counts.renamed = renames.len();
    Ok(Plan { tasks, renames, counts, skipped })
}

// ---------------------------------------------------------------- where to

/// The drive with a shoebox library that `path` is on (the folder that has a
/// `.shoebox` folder), if any: copies there would be found by its next scan.
pub fn shoebox_drive(path: &Path) -> Option<PathBuf> {
    // The new folder does not exist yet: resolve the nearest folder that does
    // (symlinks such as /var -> /private/var on a Mac), so the answer can be
    // compared with `drive_of_index`.
    let mut at = path.to_path_buf();
    let resolved = loop {
        if let Ok(real) = at.canonicalize() {
            break real;
        }
        at.file_name()?;
        if !at.pop() {
            return None;
        }
    };
    let mut at = resolved;
    loop {
        if at.join(db::DIR).is_dir() {
            return Some(at);
        }
        if !at.pop() {
            return None;
        }
    }
}

/// The drive a library's index is on, to tell "copies back onto the drive
/// they came from" from "onto another drive with a library".
pub fn drive_of_index(db_path: &Path) -> Option<PathBuf> {
    let drive = db_path.parent()?.parent()?;
    Some(drive.canonicalize().unwrap_or_else(|_| drive.to_path_buf()))
}

#[derive(Debug, Serialize)]
pub struct Place {
    /// downloads, desktop, documents, pictures, home
    pub kind: &'static str,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DriveInfo {
    pub name: String,
    pub path: String,
    /// Has a library of its own.
    pub library: bool,
    pub free: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Places {
    pub places: Vec<Place>,
    pub drives: Vec<DriveInfo>,
}

fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_dir())
}

/// `XDG_DOWNLOAD_DIR="$HOME/Downloads"` and the like from `user-dirs.dirs`.
fn xdg_dir(home: &Path, key: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(home.join(".config/user-dirs.dirs")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix(&format!("{key}=")))?;
    let value = line.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => PathBuf::from(format!("{}{rest}", home.display())),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path.is_dir()).then_some(path)
}

/// The folders people save into, and the drives. A place that does not exist
/// is left out. On Linux the places come from the desktop's own settings
/// (`user-dirs.dirs`), elsewhere they are the usual folders in the home folder.
pub fn places() -> Places {
    let mut places = Vec::new();
    if let Some(home) = home() {
        for (kind, folder, xdg) in [
            ("downloads", "Downloads", "XDG_DOWNLOAD_DIR"),
            ("desktop", "Desktop", "XDG_DESKTOP_DIR"),
            ("documents", "Documents", "XDG_DOCUMENTS_DIR"),
            ("pictures", "Pictures", "XDG_PICTURES_DIR"),
        ] {
            let path = if cfg!(target_os = "linux") { xdg_dir(&home, xdg) } else { None }
                .unwrap_or_else(|| home.join(folder));
            if path.is_dir() {
                places.push(Place { kind, path: path.display().to_string() });
            }
        }
        places.push(Place { kind: "home", path: home.display().to_string() });
    }
    let drives = crate::launcher::detect_drives()
        .into_iter()
        .map(|d| DriveInfo { free: free_space(Path::new(&d.path)), name: d.name, path: d.path, library: d.library })
        .collect();
    Places { places, drives }
}

#[cfg(unix)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(not(unix))]
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}

#[derive(Debug, Serialize)]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct Folder {
    pub name: String,
    pub path: String,
    /// Has a `.shoebox` folder.
    pub library: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    pub crumbs: Vec<Crumb>,
    pub folders: Vec<Folder>,
    /// The drive with a library this folder is on, if any.
    pub shoebox: Option<String>,
    pub free: Option<u64>,
}

/// The sub-folders of `path` (no files, no hidden folders).
pub fn list_folders(path: &Path) -> Result<Listing> {
    if !path.is_absolute() {
        bail!("not a full path");
    }
    let meta = fs::metadata(path).with_context(|| format!("{} cannot be opened", path.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    let mut folders = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("{} cannot be read", path.display()))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let p = entry.path();
        folders.push(Folder { name: library::nfc(&name), library: p.join(db::DIR).is_dir(), path: p.display().to_string() });
    }
    folders.sort_by_key(|f| f.name.to_lowercase());
    let mut crumbs: Vec<Crumb> = path
        .ancestors()
        .map(|a| Crumb {
            name: a.file_name().map(|n| library::nfc(&n.to_string_lossy())).unwrap_or_else(|| a.display().to_string()),
            path: a.display().to_string(),
        })
        .collect();
    crumbs.reverse();
    Ok(Listing {
        path: path.display().to_string(),
        crumbs,
        folders,
        shoebox: shoebox_drive(path).map(|d| d.display().to_string()),
        free: free_space(path),
    })
}

/// The new folder inside `parent`; `parent` must exist.
pub fn destination(parent: &str, name: &str) -> Result<PathBuf> {
    let name = check_folder_name(name)?;
    let parent = Path::new(parent);
    if !parent.is_absolute() || !parent.is_dir() {
        bail!("choose the folder to save in");
    }
    // A library whose index is kept next to the program is only read: no copies into its folder.
    if let Some(lib) = crate::libraries::read_only_root_of(parent) {
        bail!("{} is read-only for shoebox (its index is kept next to the program), nothing can be saved in it: choose a folder outside it", lib.name);
    }
    Ok(parent.join(name))
}

// ---------------------------------------------------------------- running

#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    /// idle, running, done, stopped, failed
    pub state: &'static str,
    pub dest: String,
    pub total: usize,
    pub done: usize,
    pub current: String,
    pub copied: usize,
    pub converted: usize,
    pub already: usize,
    pub renamed: usize,
    /// Written, but without EXIF (it did not fit or could not be read).
    pub no_exif: usize,
    pub failed: Vec<String>,
    pub error: Option<String>,
}

/// What the running export reports and what asks it to stop.
#[derive(Default)]
pub struct Shared {
    pub progress: Mutex<Progress>,
    pub stop: AtomicBool,
}

impl Shared {
    pub fn running(&self) -> bool {
        self.progress.lock().unwrap().state == "running"
    }

    /// Mark an export as started (before its thread runs).
    pub fn begin(&self, dest: &Path) {
        self.stop.store(false, Ordering::SeqCst);
        *self.progress.lock().unwrap() = Progress { state: "running", dest: dest.display().to_string(), ..Progress::default() };
    }

    fn update(&self, f: impl FnOnce(&mut Progress)) {
        f(&mut self.progress.lock().unwrap());
    }
}

/// Do an export. Returns when done, stopped or failed; `shared` has the outcome.
pub fn run(root: &Path, plan: Plan, dest: &Path, shared: &Shared) {
    let heif = LibHeif::new();
    shared.update(|p| {
        *p = Progress { state: "running", dest: dest.display().to_string(), total: plan.tasks.len(), ..Progress::default() };
    });
    let outcome = (|| -> Result<bool> {
        fs::create_dir_all(dest).with_context(|| format!("{} cannot be made", dest.display()))?;
        renumber(dest, &plan.renames)?;
        shared.update(|p| p.renamed = plan.renames.len());
        for task in &plan.tasks {
            if shared.stop.load(Ordering::SeqCst) {
                return Ok(false);
            }
            shared.update(|p| p.current = task.target.clone());
            if task.existing.is_some() {
                shared.update(|p| {
                    p.already += 1;
                    p.done += 1;
                });
                continue;
            }
            match export_one(&heif, root, dest, task, shared) {
                Ok(Written::Stopped) => return Ok(false),
                Ok(Written::Done { no_exif }) => shared.update(|p| {
                    p.copied += 1;
                    p.converted += usize::from(task.convert);
                    p.no_exif += usize::from(no_exif);
                    p.done += 1;
                }),
                Err(e) => shared.update(|p| {
                    p.failed.push(format!("{}: {e}", task.src.name));
                    p.done += 1;
                }),
            }
        }
        Ok(true)
    })();
    shared.update(|p| {
        p.current.clear();
        match outcome {
            Ok(true) => p.state = "done",
            Ok(false) => p.state = "stopped",
            Err(e) => {
                p.state = "failed";
                p.error = Some(format!("{e:#}"));
            }
        }
    });
}

/// Give the files that are in the folder their new names, through temporary
/// names so that two files can swap numbers. Never replaces a file.
fn renumber(dir: &Path, renames: &[(String, String)]) -> Result<()> {
    // The files are listed NFC; on disk they may be decomposed.
    let mut on_disk: HashMap<String, String> = HashMap::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let raw = entry.file_name().to_string_lossy().into_owned();
        on_disk.insert(library::nfc(&raw), raw);
    }
    let mut stage: Vec<(PathBuf, String)> = Vec::new();
    for (i, (old, new)) in renames.iter().enumerate() {
        let Some(raw) = on_disk.get(old) else { continue };
        let temp = dir.join(format!("{TEMP}-{i}"));
        organize::rename_noreplace(&dir.join(raw), &temp).with_context(|| format!("{old} cannot be renamed"))?;
        stage.push((temp, new.clone()));
    }
    for (temp, new) in stage {
        let to = dir.join(&new);
        if to.exists() {
            bail!("{new} is in the way; the files in the folder keep temporary names starting with {TEMP}");
        }
        organize::rename_noreplace(&temp, &to).with_context(|| format!("{new} cannot be written"))?;
    }
    Ok(())
}

enum Written {
    Done { no_exif: bool },
    Stopped,
}

fn export_one(heif: &LibHeif, root: &Path, dest: &Path, task: &Task, shared: &Shared) -> Result<Written, String> {
    // Stored paths are relative and plain; refuse anything else outright.
    if !Path::new(&task.src.rel).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return Err("not a path inside the library".into());
    }
    let from = root.join(&task.src.rel);
    let to = dest.join(&task.target);
    if to.exists() {
        return Err(format!("{} is in the folder already", task.target));
    }
    let part = dest.join(format!("{}{PART}", task.target));
    let result = fingerprint::read_unchanged(&from, task.src.size, task.src.mtime_ns, || {
        let modified = fs::metadata(&from).and_then(|m| m.modified()).map_err(|e| e.to_string())?;
        let written = if task.convert {
            convert_heic(heif, &from, &part, task.turn, modified).map_err(|e| format!("{e:#}"))?
        } else {
            match copy_checked(&from, &part, task.src.full_hash.as_deref(), modified, &shared.stop)? {
                None => return Ok(Written::Stopped),
                Some(()) => Written::Done { no_exif: false },
            }
        };
        Ok(written)
    });
    match result {
        Ok(Written::Done { no_exif }) => {
            organize::rename_noreplace(&part, &to).map_err(|e| format!("{} cannot be written: {e}", task.target))?;
            Ok(Written::Done { no_exif })
        }
        Ok(Written::Stopped) => {
            let _ = fs::remove_file(&part);
            Ok(Written::Stopped)
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Copy `from` to `to` (which must not exist), hashing what is read, then
/// read the copy back and compare. `None` when stopped.
fn copy_checked(from: &Path, to: &Path, index_hash: Option<&str>, modified: SystemTime, stop: &AtomicBool) -> Result<Option<()>, String> {
    let io = |e: std::io::Error| e.to_string();
    let mut src = File::open(from).map_err(io)?;
    let mut dst = OpenOptions::new().write(true).create_new(true).open(to).map_err(io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let n = src.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).map_err(io)?;
    }
    let read = hasher.finalize().to_hex().to_string();
    if index_hash.is_some_and(|h| h != read) {
        return Err("changed since the last scan".into());
    }
    dst.sync_all().map_err(io)?;
    dst.set_modified(modified).map_err(io)?;
    drop(dst);
    let copy = fingerprint::full_hash(to).map_err(io)?;
    if copy != read {
        return Err("the copy is not identical to the original".into());
    }
    Ok(Some(()))
}

// ---------------------------------------------------------------- HEIC to JPEG

/// Decode a HEIC (upright, as shown), turn it as the user turned it in
/// shoebox, encode a JPEG with the EXIF and colour profile of the original
/// and write it to `to`.
fn convert_heic(heif: &LibHeif, from: &Path, to: &Path, turn: i32, modified: SystemTime) -> Result<Written> {
    let img = media::decode_image(heif, Kind::Heic, from, u32::MAX / 4)?;
    let img = match turn.rem_euclid(4) {
        1 => img.rotate90(),
        2 => img.rotate180(),
        3 => img.rotate270(),
        _ => img,
    };
    let jpeg = media::encode_jpeg(&img, JPEG_QUALITY)?;
    let (exif, icc) = heic_metadata(from);
    let mut no_exif = exif.is_none();
    let exif = exif.and_then(|e| app1(&e));
    no_exif |= exif.is_none();
    let jpeg = with_metadata(&jpeg, exif.as_deref(), icc.as_deref());
    let mut out = OpenOptions::new().write(true).create_new(true).open(to)?;
    out.write_all(&jpeg)?;
    out.sync_all()?;
    out.set_modified(modified)?;
    Ok(Written::Done { no_exif })
}

/// The EXIF (TIFF part) and the ICC profile of a HEIC.
fn heic_metadata(path: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let Some(path) = path.to_str() else { return (None, None) };
    let Ok(ctx) = HeifContext::read_from_file(path) else { return (None, None) };
    let Ok(handle) = ctx.primary_image_handle() else { return (None, None) };
    let mut ids = vec![0; handle.number_of_metadata_blocks(b"Exif").max(0) as usize];
    let n = handle.metadata_block_ids(&mut ids, b"Exif");
    let exif = ids.iter().take(n).find_map(|&id| handle.metadata(id).ok()).and_then(|block| {
        // The block starts with the offset to the TIFF header (4 bytes).
        let skip = u32::from_be_bytes(block.get(..4)?.try_into().ok()?) as usize;
        let tiff = block.get(4 + skip..)?;
        (tiff.len() > 8).then(|| tiff.to_vec())
    });
    let icc = handle.color_profile_raw().map(|p| p.data);
    (exif, icc)
}

/// An APP1 segment (marker and length included) for a TIFF block, with the
/// Orientation set to "upright": the pixels are upright already.
fn app1(tiff: &[u8]) -> Option<Vec<u8>> {
    let mut tiff = tiff.to_vec();
    // Let `orientation::find` locate the tag in a minimal JPEG around the block.
    let len = (2 + 6 + tiff.len()).min(0xFFFF) as u16;
    let fake = [&[0xFF, 0xD8, 0xFF, 0xE1][..], &len.to_be_bytes(), b"Exif\0\0", &tiff[..]].concat();
    if let Ok(slot) = crate::orientation::find(&fake) {
        let at = slot.offset - 12; // SOI, marker, length and "Exif\0\0" come first
        let bytes = slot.bytes(1);
        tiff[at] = bytes[0];
        tiff[at + 1] = bytes[1];
    }
    let len = 2 + 6 + tiff.len();
    if len > 0xFFFF {
        return None; // the segment cannot be longer
    }
    let mut seg = vec![0xFF, 0xE1];
    seg.extend((len as u16).to_be_bytes());
    seg.extend(b"Exif\0\0");
    seg.extend(tiff);
    Some(seg)
}

/// APP2 segments for an ICC profile (`ICC_PROFILE`, in pieces if needed).
fn icc_segments(profile: &[u8]) -> Vec<u8> {
    const MAX: usize = 0xFFFF - 2 - 14;
    let pieces: Vec<&[u8]> = profile.chunks(MAX).collect();
    let mut out = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        out.extend([0xFF, 0xE2]);
        out.extend(((2 + 14 + piece.len()) as u16).to_be_bytes());
        out.extend(b"ICC_PROFILE\0");
        out.push(i as u8 + 1);
        out.push(pieces.len() as u8);
        out.extend(*piece);
    }
    out
}

/// The JPEG with the segments put right after its JFIF header.
fn with_metadata(jpeg: &[u8], exif: Option<&[u8]>, icc: Option<&[u8]>) -> Vec<u8> {
    let mut at = 2; // after SOI
    if jpeg.get(2..4) == Some(&[0xFF, 0xE0]) {
        if let Some(len) = jpeg.get(4..6).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) {
            at = (4 + len).min(jpeg.len());
        }
    }
    let mut out = Vec::with_capacity(jpeg.len() + 4096);
    out.extend(&jpeg[..at]);
    if let Some(e) = exif {
        out.extend(e);
    }
    if let Some(p) = icc.filter(|p| !p.is_empty() && p.len() < 0xFF * (0xFFFF - 16)) {
        out.extend(icc_segments(p));
    }
    out.extend(&jpeg[at..]);
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_three_digits_or_more() {
        assert_eq!(prefix(1, 42), "001_");
        assert_eq!(prefix(7, 1500), "0007_");
        assert_eq!(without_number("001_IMG_1.jpg"), Some("IMG_1.jpg"));
        assert_eq!(without_number("0007_IMG_1.jpg"), Some("IMG_1.jpg"));
        assert_eq!(without_number("12_IMG.jpg"), None);
        assert_eq!(without_number("IMG_001_x.jpg"), None);
        assert_eq!(without_number("001_"), None);
    }

    #[test]
    fn names_are_made_safe_for_every_file_system() {
        assert_eq!(safe_name("a:b?c.jpg"), "a_b_c.jpg");
        assert_eq!(safe_name("name. "), "name");
        assert_eq!(safe_name("..."), "_");
        let long = format!("{}.jpg", "x".repeat(300));
        let s = safe_name(&long);
        assert_eq!(s.chars().count(), 200);
        assert!(s.ends_with(".jpg"));
        assert_eq!(with_suffix("IMG_1.jpg", 2), "IMG_1_2.jpg");
        assert_eq!(with_suffix("noext", 3), "noext_3");
    }

    #[test]
    fn folder_names_are_checked_not_changed() {
        assert_eq!(check_folder_name("  print-march ").unwrap(), "print-march");
        assert!(check_folder_name("").is_err());
        assert!(check_folder_name("a/b").is_err());
        assert!(check_folder_name("a:b").is_err());
        assert!(check_folder_name("..").is_err());
        assert!(check_folder_name("end.").is_err());
    }

    #[test]
    fn exif_orientation_is_reset_and_jpeg_gets_its_segments() {
        // A TIFF block (little endian) with one entry: Orientation = 6.
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        tiff.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend([0, 0, 0, 0]);
        let seg = app1(&tiff).unwrap();
        assert_eq!(&seg[..2], &[0xFF, 0xE1]);
        assert_eq!(&seg[4..10], b"Exif\0\0");
        let file = [&[0xFF, 0xD8][..], &seg, &[0xFF, 0xD9]].concat();
        assert_eq!(crate::orientation::find(&file).unwrap().value, 1);

        let jpeg = [&[0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2][..], &[0xFF, 0xD9]].concat();
        let out = with_metadata(&jpeg, Some(&seg), Some(&[1, 2, 3]));
        assert_eq!(&out[..8], &jpeg[..8]); // SOI and the JFIF header stay first
        assert_eq!(&out[8..10], &[0xFF, 0xE1]);
        assert!(out.windows(12).any(|w| w == b"ICC_PROFILE\0"));
        assert_eq!(&out[out.len() - 2..], &[0xFF, 0xD9]);
    }
}
