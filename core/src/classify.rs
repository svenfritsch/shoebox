//! Which files the scanner looks at, and what kind of media they are.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Jpeg,
    Png,
    Heic,
    Raw,
    Video,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Jpeg => "JPEG",
            Kind::Png => "PNG",
            Kind::Heic => "HEIC",
            Kind::Raw => "RAW",
            Kind::Video => "Video",
        }
    }

    /// Name stored in the database (same as the serde name).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Png => "png",
            Kind::Heic => "heic",
            Kind::Raw => "raw",
            Kind::Video => "video",
        }
    }
}

const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "raf", "pef", "srw",
];
const VIDEO_EXTENSIONS: &[&str] = &["mov", "mp4", "m4v", "3gp", "avi", "mts", "m2ts", "mkv", "mpg"];

/// Classify a file by its extension. `None` means "not media, ignore".
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some(Kind::Jpeg),
        "png" => Some(Kind::Png),
        "heic" | "heif" => Some(Kind::Heic),
        e if RAW_EXTENSIONS.contains(&e) => Some(Kind::Raw),
        e if VIDEO_EXTENSIONS.contains(&e) => Some(Kind::Video),
        _ => None,
    }
}

/// Names the scanner never descends into or reports: macOS bookkeeping on
/// exFAT (AppleDouble `._*` files, Spotlight, Trash, fsevents) and our own
/// `.shoebox` folder.
pub fn is_ignored(name: &str) -> bool {
    name.starts_with("._")
        || matches!(
            name,
            ".DS_Store"
                | ".Spotlight-V100"
                | ".Trashes"
                | ".fseventsd"
                | ".TemporaryItems"
                | ".shoebox"
                | "System Volume Information"
                | "$RECYCLE.BIN"
                | "Thumbs.db"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(kind_of(Path::new("a/IMG_1.JPG")), Some(Kind::Jpeg));
        assert_eq!(kind_of(Path::new("IMG_1.HEIC")), Some(Kind::Heic));
        assert_eq!(kind_of(Path::new("IMG_1.CR2")), Some(Kind::Raw));
        assert_eq!(kind_of(Path::new("IMG_1.MOV")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new("notes.txt")), None);
        assert_eq!(kind_of(Path::new("noext")), None);
    }

    #[test]
    fn ignores_macos_bookkeeping() {
        assert!(is_ignored("._IMG_1.JPG"));
        assert!(is_ignored(".DS_Store"));
        assert!(is_ignored(".shoebox"));
        assert!(!is_ignored("IMG_1.JPG"));
        assert!(!is_ignored(".hidden-but-ours"));
    }
}
