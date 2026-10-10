//! Which drive a library folder lives on, for the drive cards: the volume's
//! name ("Extreme SSD") and the folder on it ("/Photos"). A person often keeps
//! more than photos on an external drive and scans only one folder, so the
//! folder's own name says little about which disk to plug in.
//!
//! Only the path (and, on Windows, the volume label) is looked at; nothing on
//! the drive is read or written.
//!
//! - macOS: `/Volumes/<name>/…`. The startup disk resolves to `/`, so it has no
//!   volume name here.
//! - Linux: `/media/<user>/<name>/…`, `/run/media/<user>/<name>/…` or
//!   `/mnt/<name>/…` (where the launcher looks for drives).
//! - Windows: the path only has a letter (`E:\Photos`); the label comes from the
//!   volume (`GetVolumeInformationW`), the letter itself when it has none.

use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use crate::reveal::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The volume's name, as the file manager shows it.
    pub volume: String,
    /// The library folder on it, with `/` as separator ("/Photos"; "/" for the
    /// top of the drive).
    pub folder: String,
}

/// The volume a folder is on, or `None` where it cannot be told from the path
/// (the startup disk, a network share, a plain folder in the home directory).
pub fn placement(root: &Path) -> Option<Placement> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    placement_for(Platform::current(), &root.to_string_lossy(), windows_label)
}

/// `placement` for a platform and a way to look up a Windows volume label, so
/// that every variant is testable anywhere.
pub fn placement_for(platform: Platform, path: &str, label: impl Fn(char) -> Option<String>) -> Option<Placement> {
    match platform {
        Platform::Windows => {
            // Canonical paths start with `\\?\`; a UNC share (`\\?\UNC\…`) has no letter.
            let plain = path.strip_prefix(r"\\?\").unwrap_or(path);
            let mut chars = plain.chars();
            let letter = chars.next().filter(char::is_ascii_alphabetic)?;
            let rest = plain[1..].strip_prefix(':')?;
            let name = label(letter.to_ascii_uppercase()).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("{}:", letter.to_ascii_uppercase()));
            Some(Placement { volume: name.nfc().collect(), folder: folder_of(rest.split(['\\', '/'])) })
        }
        Platform::Mac => split_under(path, &["/Volumes/"]),
        Platform::Other => {
            // /media/<user>/<name>, /run/media/<user>/<name>, /mnt/<name>
            for prefix in ["/media/", "/run/media/"] {
                if let Some(rest) = path.strip_prefix(prefix) {
                    let (_user, drive) = rest.split_once('/')?;
                    return split_under_rest(drive);
                }
            }
            split_under(path, &["/mnt/"])
        }
    }
}

fn split_under(path: &str, prefixes: &[&str]) -> Option<Placement> {
    prefixes.iter().find_map(|p| path.strip_prefix(p)).and_then(split_under_rest)
}

/// `Name/folder/…` below the directory that holds the drives.
fn split_under_rest(rest: &str) -> Option<Placement> {
    let mut parts = rest.split('/');
    let name = parts.next().filter(|n| !n.is_empty())?;
    Some(Placement { volume: name.nfc().collect(), folder: folder_of(parts) })
}

fn folder_of<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let joined: Vec<&str> = parts.filter(|p| !p.is_empty()).collect();
    format!("/{}", joined.join("/")).nfc().collect()
}

#[cfg(windows)]
fn windows_label(letter: char) -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
    }
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
    let mut buf = [0u16; 261];
    // SAFETY: `root` is NUL-terminated, `buf` is as long as stated, and the
    // other out-parameters are optional (null).
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(not(windows))]
fn windows_label(_letter: char) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(volume: &str, folder: &str) -> Option<Placement> {
        Some(Placement { volume: volume.into(), folder: folder.into() })
    }

    #[test]
    fn macos_volumes() {
        let none = |_| None;
        assert_eq!(placement_for(Platform::Mac, "/Volumes/Extreme SSD/Photos", none), p("Extreme SSD", "/Photos"));
        assert_eq!(placement_for(Platform::Mac, "/Volumes/tesselina's harddrive/Photos/Familie", none), p("tesselina's harddrive", "/Photos/Familie"));
        assert_eq!(placement_for(Platform::Mac, "/Volumes/Fotos", none), p("Fotos", "/"));
        // The startup disk resolves to /Users/…; nothing to say about it.
        assert_eq!(placement_for(Platform::Mac, "/Users/me/Pictures", none), None);
        assert_eq!(placement_for(Platform::Mac, "/Volumes/", none), None);
    }

    #[test]
    fn linux_mounts() {
        let none = |_| None;
        assert_eq!(placement_for(Platform::Other, "/media/anna/Extreme SSD/Photos", none), p("Extreme SSD", "/Photos"));
        assert_eq!(placement_for(Platform::Other, "/run/media/anna/Backup/Familie/2020", none), p("Backup", "/Familie/2020"));
        assert_eq!(placement_for(Platform::Other, "/mnt/usb/Photos", none), p("usb", "/Photos"));
        assert_eq!(placement_for(Platform::Other, "/media/anna", none), None);
        assert_eq!(placement_for(Platform::Other, "/home/anna/Photos", none), None);
    }

    #[test]
    fn windows_letters_and_labels() {
        let label = |c: char| (c == 'E').then(|| "Extreme SSD".to_string());
        assert_eq!(placement_for(Platform::Windows, r"E:\Photos", label), p("Extreme SSD", "/Photos"));
        assert_eq!(placement_for(Platform::Windows, r"\\?\E:\Photos\Familie", label), p("Extreme SSD", "/Photos/Familie"));
        assert_eq!(placement_for(Platform::Windows, r"E:\", label), p("Extreme SSD", "/"));
        // No label: the letter.
        assert_eq!(placement_for(Platform::Windows, r"\\?\F:\Photos", label), p("F:", "/Photos"));
        // A network share has no letter.
        assert_eq!(placement_for(Platform::Windows, r"\\?\UNC\server\share\Photos", label), None);
    }

    #[test]
    fn names_are_nfc() {
        let decomposed = "/Volumes/Ko\u{308}ln/Bilder";
        assert_eq!(placement_for(Platform::Mac, decomposed, |_| None), p("K\u{f6}ln", "/Bilder"));
    }
}
