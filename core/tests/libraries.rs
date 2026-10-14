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
