//! Phase 16: a library registered in `libraries.json` keeps its index next to
//! the program and nothing is written into its root.
//!
//! The program folder is `$SHOEBOX_HOME`, set per child process, so the tests
//! do not touch each other (or the folder of the test binary).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use shoebox::libraries;

/// Every entry below `root` (folders too), with size and times, to compare
/// before and after.
fn tree(root: &Path) -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
    let mut out: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap())
        .map(|e| {
            let m = e.metadata().unwrap();
            (e.path().to_path_buf(), m.len(), m.modified().ok())
        })
        .collect();
    out.sort();
    out
}

fn shoebox(home: &Path, args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_shoebox")).args(args).env(libraries::HOME_ENV, home).output().unwrap()
}

#[test]
fn a_registered_library_keeps_its_index_next_to_the_program_and_writes_nothing_into_its_root() {
    let lib = Library::new("libraries-readonly");
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();

    let entry = libraries::add(&home, "Server Fotos", &lib.root).unwrap();
    let before = tree(&lib.root);
    let snapshot = lib.snapshot();

    for cmd in ["scan", "scan", "verify"] {
        let out = shoebox(&home, &[cmd.as_ref(), lib.root.as_os_str()]);
        assert!(out.status.success(), "{cmd}: {}", String::from_utf8_lossy(&out.stderr));
    }

    // The index and the previews are in libraries/<name>/ ...
    let dir = libraries::index_dir(&home, &entry);
    assert!(dir.join("library.db").is_file());
    assert!(dir.join("thumbs.db").is_file());
    // ... and the root is exactly as it was: no .shoebox, nothing changed.
    assert!(!lib.root.join(".shoebox").exists(), "a .shoebox folder was made in the root");
    assert_eq!(before, tree(&lib.root));
    let after = lib.snapshot();
    assert_eq!(snapshot.len(), after.len());
    for (path, (stamp, hash)) in &snapshot {
        assert_eq!(&after[path].0, stamp, "timestamps or size changed: {}", path.display());
        assert_eq!(&after[path].1, hash, "content changed: {}", path.display());
    }

    // The same root is found again however it is spelled.
    let reg = libraries::load(&home).unwrap();
    assert_eq!(reg.find_root(&lib.root.join(".").join("")).map(|e| e.name.as_str()), Some("Server Fotos"));

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn a_root_that_is_not_registered_still_gets_its_own_shoebox_folder() {
    let lib = Library::new("libraries-unregistered");
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-empty-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();

    let out = shoebox(&home, &["scan".as_ref(), lib.root.as_os_str()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(lib.root.join(".shoebox").join("library.db").is_file());
    assert!(!home.join("libraries").exists());

    let _ = fs::remove_dir_all(&home);
}

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
    shoebox(home, &args)
}

fn text(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn the_libraries_command_lists_adds_scans_renames_moves_removes_and_deletes() {
    let lib = Library::new("libraries-cli");
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-cli-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let root = lib.root.display().to_string();
    let before = tree(&lib.root);

    // Nothing yet; the list says how to add one, and is valid JSON when asked.
    let none = run(&home, &["libraries"]);
    assert!(none.status.success() && text(&none).contains("No libraries"), "{}", text(&none));
    let json: serde_json::Value = serde_json::from_slice(&run(&home, &["libraries", "--json"]).stdout).unwrap();
    assert_eq!(json["libraries"].as_array().unwrap().len(), 0);

    // `scan --index-next-to-program` registers the folder (named after it) and scans it; nothing is written into it.
    let scanned = run(&home, &["scan", &root, "--index-next-to-program", "--name", "Server Fotos", "--no-thumbs"]);
    assert!(scanned.status.success(), "{}", text(&scanned));
    assert!(!lib.root.join(".shoebox").exists());
    assert_eq!(before, tree(&lib.root));
    assert!(home.join("libraries/Server Fotos/library.db").is_file());
    // A second scan, without the flag, finds the index by the registry.
    assert!(run(&home, &["scan", &root, "--no-thumbs"]).status.success());
    assert!(run(&home, &["verify", &root]).status.success());
    assert!(!lib.root.join(".shoebox").exists());
    let listed: serde_json::Value = serde_json::from_slice(&run(&home, &["libraries", "--json"]).stdout).unwrap();
    assert_eq!(listed["libraries"][0]["name"], "Server Fotos");
    assert_eq!(listed["libraries"][0]["online"], true);
    assert!(listed["libraries"][0]["photos"].as_u64().unwrap() >= 5, "{listed}");

    // --db and --index-next-to-program exclude each other, --name needs the flag.
    assert!(!run(&home, &["scan", &root, "--db", "x.db", "--index-next-to-program"]).status.success());
    assert!(!run(&home, &["scan", &root, "--name", "X"]).status.success());
    // The same folder again, by add or by scan, is not a second library.
    assert!(!run(&home, &["libraries", "add", &root]).status.success());
    let again = run(&home, &["scan", &root, "--index-next-to-program", "--no-thumbs"]);
    assert!(again.status.success() && text(&again).contains("already a library"), "{}", text(&again));
    assert_eq!(libraries::load(&home).unwrap().libraries.len(), 1);

    // Rename moves the index folder; a bad name is refused.
    assert!(run(&home, &["libraries", "rename", "Server Fotos", "Büro"]).status.success());
    assert!(home.join("libraries/Büro/library.db").is_file() && !home.join("libraries/Server Fotos").exists());
    assert!(!run(&home, &["libraries", "rename", "Büro", "a/b"]).status.success());

    // Another folder: a folder without the photos is refused (unless --force), the right one is taken.
    let wrong = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-cli-wrong", std::process::id()));
    fs::create_dir_all(&wrong).unwrap();
    let w = wrong.display().to_string();
    let refused = run(&home, &["libraries", "root", "Büro", &w]);
    assert!(!refused.status.success() && text(&refused).contains("none of the"), "{}", text(&refused));
    assert!(run(&home, &["libraries", "root", "Büro", &root]).status.success());
    assert!(run(&home, &["libraries", "root", "Büro", &w, "--force"]).status.success());
    // Back to the real folder needs no --force: the indexed photos are all there.
    let back = run(&home, &["libraries", "root", "Büro", &root]);
    assert!(back.status.success() && text(&back).contains("were found there"), "{}", text(&back));

    // Remove keeps the index folder; delete needs --yes and removes it.
    assert!(run(&home, &["libraries", "remove", "Büro"]).status.success());
    assert!(home.join("libraries/Büro/library.db").is_file());
    assert!(run(&home, &["libraries", "add", &root, "--name", "Zwei"]).status.success());
    let no = run(&home, &["libraries", "delete", "Zwei"]);
    assert!(!no.status.success() && text(&no).contains("--yes"), "{}", text(&no));
    assert!(libraries::load(&home).unwrap().find_name("Zwei").is_some());
    assert!(run(&home, &["libraries", "delete", "Zwei", "--yes"]).status.success());
    assert!(!home.join("libraries/Zwei").exists() && libraries::load(&home).unwrap().libraries.is_empty());
    assert!(!run(&home, &["libraries", "delete", "Nope", "--yes"]).status.success());

    // All along, nothing in the library folder changed.
    assert_eq!(before, tree(&lib.root));
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&wrong);
}

#[test]
fn a_library_that_moved_keeps_its_index_and_an_offline_one_is_listed_not_connected() {
    let lib = Library::new("libraries-moved");
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-moved-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let root = lib.root.display().to_string();
    assert!(run(&home, &["scan", &root, "--index-next-to-program", "--name", "Srv", "--no-thumbs"]).status.success());

    // The share is now somewhere else (a copy with the same relative paths and the same times).
    let moved = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-moved-new", std::process::id()));
    let _ = fs::remove_dir_all(&moved);
    copy_dir(&lib.root, &moved);
    for e in walkdir::WalkDir::new(&lib.root).into_iter().map(|e| e.unwrap()).filter(|e| e.file_type().is_file()) {
        let rel = e.path().strip_prefix(&lib.root).unwrap();
        set_mtime(&moved.join(rel), shoebox::fingerprint::stamp(e.path()).unwrap().mtime_ns as i128);
    }
    let new_root = moved.display().to_string();
    let pointed = run(&home, &["libraries", "root", "Srv", &new_root]);
    assert!(pointed.status.success(), "{}", text(&pointed));

    // Nothing is found new, changed, moved or missing: the index followed the folder.
    let scan = run(&home, &["scan", &new_root, "--no-thumbs", "--json"]);
    assert!(scan.status.success(), "{}", text(&scan));
    let stats: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    for key in ["added", "changed", "moved", "missing"] {
        assert_eq!(stats[key], 0, "{key}: {stats}");
    }
    assert!(stats["unchanged"].as_u64().unwrap() >= 5, "{stats}");
    assert!(!moved.join(".shoebox").exists());
    assert!(run(&home, &["verify", &new_root]).status.success());

    // The share goes offline: listed as not connected, the photo count stays, a scan stops cleanly.
    let gone = moved.with_extension("gone");
    let _ = fs::remove_dir_all(&gone);
    fs::rename(&moved, &gone).unwrap();
    let list: serde_json::Value = serde_json::from_slice(&run(&home, &["libraries", "--json"]).stdout).unwrap();
    assert_eq!(list["libraries"][0]["online"], false, "{list}");
    assert!(list["libraries"][0]["photos"].as_u64().unwrap() >= 5, "{list}");
    let offline = run(&home, &["scan", &new_root, "--no-thumbs"]);
    assert!(!offline.status.success());
    assert!(!moved.exists(), "a scan of an offline share made its folder");
    // It comes back: all as before.
    fs::rename(&gone, &moved).unwrap();
    let list: serde_json::Value = serde_json::from_slice(&run(&home, &["libraries", "--json"]).stdout).unwrap();
    assert_eq!(list["libraries"][0]["online"], true);

    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&moved);
}

#[test]
fn a_backup_check_between_two_libraries_kept_next_to_the_program_only_reads() {
    let original = Library::new("libraries-backup-original");
    let backup = Library::new("libraries-backup-copy");
    let home = std::env::temp_dir().join(format!("shoebox-it-{}-libraries-backup-home", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let (a, b) = (original.root.display().to_string(), backup.root.display().to_string());
    for (root, name) in [(&a, "Original"), (&b, "Copy")] {
        let scanned = run(&home, &["scan", root, "--index-next-to-program", "--name", name, "--no-thumbs"]);
        assert!(scanned.status.success(), "{}", text(&scanned));
    }
    let (before_a, before_b) = (tree(&original.root), tree(&backup.root));
    let check = run(&home, &["backup", &a, &b, "--json"]);
    assert!(check.status.success(), "{}", text(&check));
    let report: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!((before_a, before_b), (tree(&original.root), tree(&backup.root)));
    assert!(!original.root.join(".shoebox").exists() && !backup.root.join(".shoebox").exists());
    let _ = fs::remove_dir_all(&home);
}
