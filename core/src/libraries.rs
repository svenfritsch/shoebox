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
}
