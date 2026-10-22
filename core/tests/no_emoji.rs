//! The pages show our own line icons, never emojis (CLAUDE.md, "Icons"): a
//! picture character in the page's code or texts brings a different look on
//! every system. Text symbols (✓ ✗ ✕ ♥ ☰ ↺ ⇄) stay. These read the files only.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The characters that are drawn as colour pictures (or as a picture symbol
/// we replaced by an icon): the emoji blocks, the warning sign and the pencil (the draw-again square ⬚ stays).
fn is_picture(c: char) -> bool {
    matches!(c, '\u{1F000}'..='\u{1FAFF}' | '\u{26A0}' | '\u{270E}')
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "vendor") {
                files(&path, out);
            }
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("js" | "css" | "html" | "json")) {
            out.push(path);
        }
    }
}

#[test]
fn no_emoji_in_the_pages() {
    let mut all = Vec::new();
    for dir in ["web", "launcher-web", "i18n"] {
        files(&root().join(dir), &mut all);
    }
    assert!(all.len() > 10, "the page's files were not found");
    let mut found = Vec::new();
    for path in all {
        let text = fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            if let Some(c) = line.chars().find(|c| is_picture(*c)) {
                found.push(format!("{}:{}: {c} (U+{:04X})", path.strip_prefix(root()).unwrap().display(), n + 1, c as u32));
            }
        }
    }
    assert!(found.is_empty(), "emojis in the pages, draw an icon instead (lineIcon in web/app.js):\n{}", found.join("\n"));
}
