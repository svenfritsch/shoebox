//! Phase 16: libraries whose index lives next to the program.
//!
//! A library on a drive keeps its index in `<root>/.shoebox/`. A folder on a
//! server (or any folder the user does not want to write into) can instead
//! keep it in `<program folder>/libraries/<name>/`; `libraries.json` next to
//! the program says which root belongs to which name. Nothing is ever written
//! into the root of such a library (see docs/phase16.md).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::{db, organize};

/// The registry file, in the program folder.
pub const FILE: &str = "libraries.json";
/// One folder per library below this one, in the program folder.
pub const DIR: &str = "libraries";
/// Test and portable override of the program folder.
pub const HOME_ENV: &str = "SHOEBOX_HOME";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The folder name under `libraries/` and the label in the Control Panel.
    pub name: String,
    /// Where the photos are, as the user gave it (a mapped letter or a UNC
    /// path). Files are stored relative to it, so it can be changed.
    pub root: String,
    pub added: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub libraries: Vec<Entry>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry { version: 1, libraries: Vec::new() }
    }
}

/// The program folder: the one that holds `shoebox` / `shoebox.exe` (or
/// `$SHOEBOX_HOME`).
pub fn home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os(HOME_ENV).filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// The registry in `home`; empty if there is no file yet.
pub fn load(home: &Path) -> Result<Registry> {
    let path = home.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::default()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let reg: Registry = serde_json::from_str(&text).with_context(|| format!("{} is not valid", path.display()))?;
    if reg.version != 1 {
        bail!("{} has version {}, this shoebox reads version 1", path.display(), reg.version);
    }
    Ok(reg)
}

/// Write the registry to a temporary file and rename it over the old one.
pub fn save(home: &Path, reg: &Registry) -> Result<()> {
    std::fs::create_dir_all(home).with_context(|| format!("create {}", home.display()))?;
    let path = home.join(FILE);
    let tmp = home.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(reg)?).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// A path as text for comparing two spellings of the same place: NFC, no
/// `\\?\` prefix (`\\?\UNC\s\x` is `\\s\x`), `/` and `\` alike and no trailing
/// separator on Windows, case-insensitive there.
pub fn norm_root(path: &str) -> String {
    let nfc: String = path.nfc().collect();
    let plain = if let Some(rest) = nfc.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        nfc.strip_prefix(r"\\?\").unwrap_or(&nfc).to_string()
    };
    let windows_like = plain.contains('\\') || plain.chars().nth(1) == Some(':');
    if windows_like {
        plain.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    } else if plain.len() > 1 {
        plain.trim_end_matches('/').to_string()
    } else {
        plain
    }
}

/// The same place, as far as it can be told: canonical forms when the paths
/// exist (a share that is down is compared as typed).
fn same_root(a: &str, b: &str) -> bool {
    let canon = |s: &str| std::fs::canonicalize(s).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| s.to_string());
    norm_root(a) == norm_root(b) || norm_root(&canon(a)) == norm_root(&canon(b))
}

impl Registry {
    pub fn find_root(&self, root: &Path) -> Option<&Entry> {
        let root = root.to_string_lossy();
        self.libraries.iter().find(|e| same_root(&e.root, &root))
    }

    pub fn find_name(&self, name: &str) -> Option<&Entry> {
        let key = name_key(name);
        self.libraries.iter().find(|e| name_key(&e.name) == key)
    }
}

fn name_key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// Where the files of a library are: `<home>/libraries/<name>/`.
pub fn index_dir(home: &Path, entry: &Entry) -> PathBuf {
    home.join(DIR).join(&entry.name)
}

/// A name for a root that no entry has yet: the root's last folder, then
/// " 2", " 3", ...
pub fn suggest_name(reg: &Registry, root: &Path) -> String {
    let base = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .and_then(|n| organize::check_name(&n).ok())
        .unwrap_or_else(|| "Library".to_string());
    let mut name = base.clone();
    let mut n = 2;
    while reg.find_name(&name).is_some() {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}

/// Add a library to the registry of `home` (the index folder is made by the
/// first scan, not here). Refuses a name or a root that is already there.
pub fn add(home: &Path, name: &str, root: &Path) -> Result<Entry> {
    let name = organize::check_name(name)?;
    let mut reg = load(home)?;
    if reg.find_name(&name).is_some() {
        bail!("there is already a library called {name}");
    }
    if let Some(other) = reg.find_root(root) {
        bail!("{} is already a library ({})", root.display(), other.name);
    }
    if !root.is_dir() {
        bail!("{} is not a folder (is the drive or share connected?)", root.display());
    }
    let entry = Entry { name, root: root.display().to_string(), added: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string() };
    reg.libraries.push(entry.clone());
    save(home, &reg)?;
    Ok(entry)
}

/// The entry that has this root, in the registry of this program's folder.
pub fn entry_for(root: &Path) -> Option<(PathBuf, Entry)> {
    let home = home()?;
    let reg = load(&home).ok()?;
    let entry = reg.find_root(root)?.clone();
    Some((home, entry))
}

/// The index database of a library registered next to the program, if
/// `root` is one.
pub fn index_db(root: &Path) -> Option<PathBuf> {
    let (home, entry) = entry_for(root)?;
    Some(index_dir(&home, &entry).join(db::FILE))
}

/// Registered next to the program: shoebox never changes anything in such a
/// library's root (no move, trash, rotate or rename).
pub fn is_read_only(root: &Path) -> bool {
    entry_for(root).is_some()
}

// ---------------------------------------------------------------- management
// What the Control Panel's "Manage…" panel does (docs/phase16.md). Each takes
// the program folder, so tests need no environment.

/// The program folder can hold `libraries/`: made if needed and written to once.
pub fn ensure_home_writable(home: &Path) -> Result<()> {
    let dir = home.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("shoebox cannot write to {} (move the shoebox folder out of Program Files, or keep the index on the drive)", home.display()))?;
    let probe = dir.join(".write-test");
    std::fs::write(&probe, b"").with_context(|| format!("shoebox cannot write to {} (move the shoebox folder out of Program Files, or keep the index on the drive)", home.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub name: String,
    pub root: String,
    /// The root is a folder that can be read now (a share may be down).
    pub online: bool,
    /// Photos in the index (not missing); `None` before the first scan.
    pub photos: Option<u64>,
}

/// Photos in the index at `db`, read-only; `None` if there is none.
fn count_photos(db: &Path) -> Option<u64> {
    if !db.is_file() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row("SELECT count(*) FROM files WHERE missing_since IS NULL", [], |r| r.get::<_, i64>(0)).ok().map(|n| n as u64)
}

/// Every registered library with its state.
pub fn list(home: &Path) -> Result<Vec<Status>> {
    let reg = load(home)?;
    Ok(reg
        .libraries
        .iter()
        .map(|e| Status {
            name: e.name.clone(),
            root: e.root.clone(),
            online: Path::new(&e.root).is_dir(),
            photos: count_photos(&index_dir(home, e).join(db::FILE)),
        })
        .collect())
}

fn entry_named<'a>(reg: &'a mut Registry, name: &str) -> Result<&'a mut Entry> {
    let key = name_key(name);
    reg.libraries.iter_mut().find(|e| name_key(&e.name) == key).with_context(|| format!("there is no library called {name}"))
}

/// A new name for a library: its folder under `libraries/` is renamed too.
pub fn rename(home: &Path, name: &str, new_name: &str) -> Result<Entry> {
    let new_name = organize::check_name(new_name)?;
    let mut reg = load(home)?;
    let old = entry_named(&mut reg, name)?.clone();
    if name_key(&old.name) != name_key(&new_name) && reg.find_name(&new_name).is_some() {
        bail!("there is already a library called {new_name}");
    }
    let from = index_dir(home, &old);
    let to = home.join(DIR).join(&new_name);
    if from.is_dir() {
        // A name that differs only in case goes through a temporary name (case-insensitive file systems).
        if name_key(&old.name) == name_key(&new_name) {
            let tmp = home.join(DIR).join(format!(".renaming-{}", old.name));
            std::fs::rename(&from, &tmp)?;
            std::fs::rename(&tmp, &to)?;
        } else if to.exists() {
            bail!("{} is already there", to.display());
        } else {
            std::fs::rename(&from, &to).with_context(|| format!("rename {}", from.display()))?;
        }
    }
    let entry = entry_named(&mut reg, name)?;
    entry.name = new_name;
    let out = entry.clone();
    save(home, &reg)?;
    Ok(out)
}

/// How many of the indexed files a folder holds, checked on a sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootCheck {
    pub checked: u64,
    pub found: u64,
    /// Photos in the index (not missing).
    pub total: u64,
}

const CHECK_SAMPLE: i64 = 1000;

/// Look for a sample of the library's files under `root` (read only).
pub fn check_root(home: &Path, name: &str, root: &Path) -> Result<RootCheck> {
    let mut reg = load(home)?;
    let entry = entry_named(&mut reg, name)?.clone();
    if !root.is_dir() {
        bail!("{} is not a folder (is the drive or share connected?)", root.display());
    }
    let db = index_dir(home, &entry).join(db::FILE);
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| format!("no index for {} yet (scan it first)", entry.name))?;
    let total: i64 = conn.query_row("SELECT count(*) FROM files WHERE missing_since IS NULL", [], |r| r.get(0))?;
    let mut stmt = conn.prepare("SELECT path FROM files WHERE missing_since IS NULL ORDER BY random() LIMIT ?1")?;
    let paths = stmt.query_map([CHECK_SAMPLE], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let found = paths.iter().filter(|p| root.join(p).is_file()).count();
    Ok(RootCheck { checked: paths.len() as u64, found: found as u64, total: total as u64 })
}

/// Point a library at another folder. The index is kept: files are stored
/// relative to the root. With `force` false a folder that holds none of the
/// sampled files is refused (it is probably the wrong one).
pub fn set_root(home: &Path, name: &str, root: &Path, force: bool) -> Result<RootCheck> {
    let check = check_root(home, name, root).or_else(|e| {
        // A library that was never scanned has nothing to compare with.
        if e.to_string().starts_with("no index") { Ok(RootCheck { checked: 0, found: 0, total: 0 }) } else { Err(e) }
    })?;
    if check.checked > 0 && check.found == 0 && !force {
        bail!("none of the {} photos checked is in {}: is this the right folder?", check.checked, root.display());
    }
    let mut reg = load(home)?;
    if let Some(other) = reg.find_root(root) {
        if name_key(&other.name) != name_key(name) {
            bail!("{} is already a library ({})", root.display(), other.name);
        }
    }
    entry_named(&mut reg, name)?.root = root.display().to_string();
    save(home, &reg)?;
    Ok(check)
}

/// Take a library off the list; its index folder stays.
pub fn remove(home: &Path, name: &str) -> Result<()> {
    let mut reg = load(home)?;
    let key = name_key(&entry_named(&mut reg, name)?.name.clone());
    reg.libraries.retain(|e| name_key(&e.name) != key);
    save(home, &reg)
}

/// Take a library off the list and delete its index folder `libraries/<name>/`
/// (the photos are not touched; tags, dates, names and previews go with it).
pub fn delete_index(home: &Path, name: &str) -> Result<()> {
    let mut reg = load(home)?;
    let entry = entry_named(&mut reg, name)?.clone();
    // The name was checked when it was added; check again before deleting a folder.
    organize::check_name(&entry.name)?;
    let dir = index_dir(home, &entry);
    reg.libraries.retain(|e| name_key(&e.name) != name_key(&entry.name));
    save(home, &reg)?;
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("delete {}", dir.display()))?;
    }
    Ok(())
}

/// Called first by everything that would change an original (move, rename,
/// trash, restore, rotate, remove copies): a library registered next to the
/// program is never changed.
pub fn ensure_writable(root: &Path) -> Result<()> {
    match entry_for(root) {
        Some((_, e)) => bail!("{} is read-only for shoebox (its index is kept next to the program), nothing in it is changed", e.name),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let d = std::env::temp_dir().join(format!("shoebox-libs-{}-{}", std::process::id(), blake3::hash(format!("{:?}", std::time::Instant::now()).as_bytes()).to_hex()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn spellings_of_one_place() {
        assert_eq!(norm_root(r"\\?\UNC\Server\Share\Fotos"), norm_root(r"\\server\share\fotos\"));
        assert_eq!(norm_root(r"\\?\E:\Photos"), norm_root("e:/photos"));
        assert_ne!(norm_root(r"\\server\share\a"), norm_root(r"\\server\share\b"));
        assert_eq!(norm_root("/Volumes/K\u{f6}ln/"), norm_root("/Volumes/Ko\u{308}ln"));
        // Unix paths stay case-sensitive.
        assert_ne!(norm_root("/mnt/Fotos"), norm_root("/mnt/fotos"));
    }

    #[test]
    fn registry_round_trip_and_refusals() {
        let home = home();
        let photos = home.join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        assert_eq!(load(&home).unwrap(), Registry::default());
        let e = add(&home, " Server Fotos ", &photos).unwrap();
        assert_eq!(e.name, "Server Fotos");
        assert_eq!(load(&home).unwrap().libraries, vec![e.clone()]);
        // Same name (any case), same root, a root that is not there, a bad name.
        assert!(add(&home, "server fotos", &home).is_err());
        assert!(add(&home, "Other", &photos).is_err());
        assert!(add(&home, "Gone", &home.join("nope")).is_err());
        assert!(add(&home, "a/b", &home).is_err());
        assert_eq!(index_dir(&home, &e), home.join("libraries").join("Server Fotos"));
        assert!(load(&home).unwrap().find_root(&photos.join("..").join("photos")).is_some());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn suggested_names_are_unique() {
        let mut reg = Registry::default();
        let root = Path::new("/mnt/Fotos");
        assert_eq!(suggest_name(&reg, root), "Fotos");
        reg.libraries.push(Entry { name: "Fotos".into(), root: "/x".into(), added: String::new() });
        assert_eq!(suggest_name(&reg, root), "Fotos 2");
    }

    #[test]
    fn broken_or_newer_file_is_an_error() {
        let home = home();
        std::fs::write(home.join(FILE), "{ nope").unwrap();
        assert!(load(&home).is_err());
        std::fs::write(home.join(FILE), r#"{"version":2,"libraries":[]}"#).unwrap();
        assert!(load(&home).is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn lib_with_index(home: &Path, name: &str, files: &[&str], root: &Path) {
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }
        let e = add(home, name, root).unwrap();
        let dir = index_dir(home, &e);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = db::open(&dir.join(db::FILE)).unwrap();
        let folder = crate::organize::ensure_folder(&conn, root, "").map(|f| f.0).unwrap_or(1);
        for (i, f) in files.iter().enumerate() {
            conn.execute(
                "INSERT INTO files (folder_id, path, path_nfc, name, kind, size, mtime_ns, quick_hash, added_at) VALUES (?1, ?2, ?2, ?3, 'jpeg', 1, 0, ?4, 0)",
                rusqlite::params![folder, f, f.rsplit('/').next().unwrap(), format!("q{i}")],
            )
            .unwrap();
        }
    }

    #[test]
    fn list_rename_set_root_remove_and_delete() {
        let home = home();
        let a = home.join("a");
        lib_with_index(&home, "Fotos", &["x/1.jpg", "x/2.jpg", "3.jpg"], &a);
        let l = list(&home).unwrap();
        assert_eq!((l[0].name.as_str(), l[0].online, l[0].photos), ("Fotos", true, Some(3)));

        // A rename moves the index folder, refuses a taken name.
        add(&home, "Other", &home).unwrap();
        assert!(rename(&home, "Fotos", "other").is_err());
        rename(&home, "Fotos", "Server Fotos").unwrap();
        assert!(home.join("libraries/Server Fotos/library.db").is_file());
        assert!(!home.join("libraries/Fotos").exists());
        rename(&home, "Server Fotos", "server fotos").unwrap(); // case only
        assert!(home.join("libraries/server fotos/library.db").is_file());

        // Another folder: refused when it holds none of the files, taken when it holds them.
        let wrong = home.join("wrong");
        std::fs::create_dir_all(&wrong).unwrap();
        assert!(set_root(&home, "server fotos", &wrong, false).is_err());
        let b = home.join("b");
        for f in ["x/1.jpg", "x/2.jpg", "3.jpg"] {
            std::fs::create_dir_all(b.join(f).parent().unwrap()).unwrap();
            std::fs::write(b.join(f), b"x").unwrap();
        }
        let c = set_root(&home, "server fotos", &b, false).unwrap();
        assert_eq!((c.checked, c.found, c.total), (3, 3, 3));
        assert_eq!(load(&home).unwrap().find_name("server fotos").unwrap().root, b.display().to_string());
        // The same folder for two libraries is refused.
        assert!(set_root(&home, "Other", &b, true).is_err());

        // Removing keeps the index folder, deleting does not.
        remove(&home, "Other").unwrap();
        assert_eq!(load(&home).unwrap().libraries.len(), 1);
        delete_index(&home, "server fotos").unwrap();
        assert!(!home.join("libraries/server fotos").exists());
        assert!(load(&home).unwrap().libraries.is_empty());
        assert!(delete_index(&home, "nope").is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_read_only_program_folder_is_reported() {
        let home = home();
        // `libraries` is a file: nothing can be made there.
        std::fs::write(home.join(DIR), b"").unwrap();
        assert!(ensure_home_writable(&home).is_err());
        std::fs::remove_dir_all(&home).unwrap();
        let ok = self::home();
        assert!(ensure_home_writable(&ok).is_ok());
        std::fs::remove_dir_all(&ok).unwrap();
    }
}
