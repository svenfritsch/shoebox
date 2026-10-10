//! The translation files (phase 8): German has every key English has, with
//! the same placeholders; every key the UI code asks for exists; no key is
//! left unused. These read the files only and touch no photos.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn messages(lang: &str) -> BTreeMap<String, String> {
    let text = fs::read_to_string(root().join("i18n").join(format!("{lang}.json"))).unwrap();
    let Value::Object(map) = serde_json::from_str(&text).unwrap() else { panic!("{lang}.json is not an object") };
    map.into_iter()
        .map(|(k, v)| {
            let Value::String(s) = v else { panic!("{lang}.json: {k} is not a string") };
            (k, s)
        })
        .collect()
}

/// `{name}` placeholders of a message.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        out.insert(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

/// The UI sources: scripts and pages of the photo app and the launcher.
fn sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in ["web", "launcher-web"] {
        for entry in fs::read_dir(root().join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if matches!(path.extension().and_then(|e| e.to_str()), Some("js" | "html")) {
                let text = fs::read_to_string(&path).unwrap();
                out.push((path, text));
            }
        }
    }
    out
}

/// Keys named in the code: every quoted `area.name` literal whose first
/// segment is an area of the messages (so also `tr(cond ? 'a.b' : 'a.c')` and
/// `data-i18n="a.b"`). A key ending in a dot is a prefix, completed at run time.
fn used_keys() -> BTreeSet<(String, PathBuf)> {
    let areas: BTreeSet<String> = messages("en").keys().filter_map(|k| k.split('.').next().map(str::to_string)).collect();
    let mut out = BTreeSet::new();
    for (path, text) in sources() {
        for line in text.lines() {
            let mut rest = line;
            while let Some(open) = rest.find(['\'', '"']) {
                let quote = rest.as_bytes()[open] as char;
                let after = &rest[open + 1..];
                let Some(len) = after.find(quote) else { break };
                let piece = &after[..len];
                let chars_ok = piece.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
                let file_name = [".css", ".js", ".json", ".html", ".svg"].iter().any(|e| piece.ends_with(e));
                if chars_ok && piece.contains('.') && !file_name && areas.contains(piece.split('.').next().unwrap()) {
                    out.insert((piece.to_string(), path.clone()));
                    rest = &after[len + 1..];
                } else {
                    // An apostrophe in a comment or text must not swallow what follows.
                    rest = after;
                }
            }
        }
    }
    out
}

/// The key a plural message is asked for by: `x.one` and `x.other` are both `x`.
fn base(key: &str) -> &str {
    for form in [".zero", ".one", ".two", ".few", ".many", ".other"] {
        if let Some(b) = key.strip_suffix(form) {
            return b;
        }
    }
    key
}

#[test]
fn german_has_every_key_english_has() {
    let (en, de) = (messages("en"), messages("de"));
    let missing: Vec<_> = en.keys().filter(|k| !de.contains_key(*k)).collect();
    let extra: Vec<_> = de.keys().filter(|k| !en.contains_key(*k)).collect();
    assert!(missing.is_empty(), "missing in de.json: {missing:?}");
    assert!(extra.is_empty(), "only in de.json: {extra:?}");
}

#[test]
fn placeholders_match_between_languages() {
    let (en, de) = (messages("en"), messages("de"));
    for (key, text) in &en {
        let Some(other) = de.get(key) else { continue };
        assert_eq!(placeholders(text), placeholders(other), "placeholders of {key} differ: {text:?} vs {other:?}");
    }
}

#[test]
fn plural_messages_have_an_other_form() {
    for lang in ["en", "de"] {
        let all = messages(lang);
        for key in all.keys() {
            if base(key) != key {
                assert!(all.contains_key(&format!("{}.other", base(key))), "{lang}: {key} needs {}.other", base(key));
            }
        }
    }
}

#[test]
fn every_key_the_code_asks_for_exists() {
    let en = messages("en");
    for (key, path) in used_keys() {
        let found = if key.ends_with('.') {
            en.keys().any(|k| k.starts_with(&key))
        } else {
            en.contains_key(&key) || en.contains_key(&format!("{key}.other"))
        };
        assert!(found, "{}: no message {key:?} in en.json", path.strip_prefix(root()).unwrap_or(Path::new("?")).display());
    }
}

#[test]
fn no_message_is_left_unused() {
    let used: BTreeSet<String> = used_keys().into_iter().map(|(k, _)| k).collect();
    let unused: Vec<_> = messages("en")
        .into_keys()
        .filter(|k| !used.contains(base(k)) && !used.iter().any(|u| u.ends_with('.') && k.starts_with(u.as_str())))
        .collect();
    assert!(unused.is_empty(), "keys in en.json that no code uses: {unused:?}");
}
