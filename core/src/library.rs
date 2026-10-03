//! Library rules that depend only on paths: relative path strings, Unicode
//! normalisation, event folders and folder tags.

use std::path::{Component, Path};

use unicode_normalization::UnicodeNormalization;

/// A path relative to the library root, `/`-separated, exactly as found on
/// disk (for opening the file), plus its NFC form (for comparing and search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelPath {
    pub raw: String,
    pub nfc: String,
}

impl RelPath {
    /// `None` for paths outside `root` or with non-UTF-8 names.
    pub fn new(root: &Path, path: &Path) -> Option<RelPath> {
        let rel = path.strip_prefix(root).ok()?;
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str()?),
                _ => return None,
            }
        }
        let raw = parts.join("/");
        let nfc = nfc(&raw);
        Some(RelPath { raw, nfc })
    }

    /// Last component (NFC); empty for the root.
    pub fn name(&self) -> &str {
        self.nfc.rsplit('/').next().unwrap_or("")
    }

    /// Parent folder (NFC); `""` is the root.
    pub fn parent_nfc(&self) -> &str {
        self.nfc.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
    }
}

pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// A folder named `YYYY-MM Name`, e.g. `2020-07 Urlaub Griechenland`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event<'a> {
    pub year: u16,
    pub month: u8,
    pub name: &'a str,
}

/// Matches `^(\d{4})-(0[1-9]|1[0-2])\s+(.+)$`.
pub fn parse_event(folder_name: &str) -> Option<Event<'_>> {
    let b = folder_name.as_bytes();
    if b.len() < 9 || !b[..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' {
        return None;
    }
    if !b[5..7].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year = folder_name[..4].parse().ok()?;
    let month: u8 = folder_name[5..7].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let rest = &folder_name[7..];
    let name = rest.trim_start();
    if name.len() == rest.len() || name.is_empty() {
        return None; // needs at least one whitespace, then a name
    }
    Some(Event { year, month, name })
}

/// The tag a folder contributes to every file below it: the event name for
/// event folders, the folder name otherwise.
pub fn folder_tag(folder_name: &str) -> &str {
    parse_event(folder_name).map(|e| e.name).unwrap_or(folder_name)
}

/// Tags of a file: one per folder level between the root and the file.
pub fn tags_for(file: &RelPath) -> Vec<&str> {
    let parent = file.parent_nfc();
    if parent.is_empty() {
        return Vec::new();
    }
    let mut tags: Vec<&str> = parent.split('/').map(folder_tag).collect();
    tags.sort_unstable();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_folders() {
        assert_eq!(
            parse_event("2020-07 Urlaub Griechenland"),
            Some(Event { year: 2020, month: 7, name: "Urlaub Griechenland" })
        );
        assert_eq!(parse_event("2020-12\tX").map(|e| e.month), Some(12));
        assert_eq!(parse_event("2020-13 Nope"), None);
        assert_eq!(parse_event("2020-00 Nope"), None);
        assert_eq!(parse_event("2020-07Urlaub"), None);
        assert_eq!(parse_event("2020-07 "), None);
        assert_eq!(parse_event("Familie"), None);
        assert_eq!(parse_event("20-07 Kurz"), None);
    }

    #[test]
    fn rel_paths_are_nfc_normalised() {
        let root = Path::new("/lib");
        let p = RelPath::new(root, Path::new("/lib/2019-08 O\u{308}sterreich/IMG_1.JPG")).unwrap();
        assert_eq!(p.raw, "2019-08 O\u{308}sterreich/IMG_1.JPG");
        assert_eq!(p.nfc, "2019-08 \u{d6}sterreich/IMG_1.JPG");
        assert_eq!(p.name(), "IMG_1.JPG");
        assert_eq!(p.parent_nfc(), "2019-08 \u{d6}sterreich");
        assert_eq!(RelPath::new(root, root).unwrap().raw, "");
        assert!(RelPath::new(root, Path::new("/other/x.jpg")).is_none());
    }

    #[test]
    fn every_folder_level_is_a_tag() {
        let root = Path::new("/lib");
        let p = RelPath::new(root, Path::new("/lib/Familie/Weihnachten/a.jpg")).unwrap();
        assert_eq!(tags_for(&p), ["Familie", "Weihnachten"]);
        let p = RelPath::new(root, Path::new("/lib/2020-07 Urlaub/a.jpg")).unwrap();
        assert_eq!(tags_for(&p), ["Urlaub"]);
        let p = RelPath::new(root, Path::new("/lib/a.jpg")).unwrap();
        assert!(tags_for(&p).is_empty());
    }
}
