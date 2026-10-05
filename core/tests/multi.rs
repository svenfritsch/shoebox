//! Several drives: duplicates across drives (never against a backup), people
//! matched by name, and the roles of the drives.

mod common;

use common::*;
use serde_json::json;

/// A library of its own: four photos that exist nowhere else, two shared with `Library::new` copies.
fn own_library(test: &str, seed: u8) -> Library {
    let lib = Library::new(test);
    let _ = std::fs::remove_dir_all(lib.path("fixtures"));
    lib.jpeg("2020-07 Urlaub Griechenland/IMG_0002.JPG", seed);
    lib.jpeg("Familie/Weihnachten/DSC_2001.jpg", seed.wrapping_add(1));
    lib.jpeg(&format!("{NFD_DIR}/IMG_0100.JPG"), seed.wrapping_add(2));
    lib.jpeg("Ordner mit Leerzeichen/Bild 1.jpeg", seed.wrapping_add(3));
    // IMG_0001.JPG (seed 1) is the same content as on every other `Library::new`.
    lib.scan_opts(true, false, false);
    lib
}

/// A copy of `Library::new` without fixtures: all contents equal to another such library.
fn copy_library(test: &str) -> Library {
    let lib = Library::new(test);
    let _ = std::fs::remove_dir_all(lib.path("fixtures"));
    lib.scan_opts(true, false, false);
    lib
}

fn lib_id(addr: std::net::SocketAddr, lib: &Library) -> String {
    let name = lib.root.file_name().unwrap().to_str().unwrap().to_string();
    get(addr, "/api/libraries")
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == name.as_str())
        .unwrap_or_else(|| panic!("{name} not listed"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn duplicates_across_separate_drives_but_never_against_a_backup() {
    let a = own_library("multi-dup-a", 10);
    let b = own_library("multi-dup-b", 100);
    let backup = copy_library("multi-dup-copy");
    let c = copy_library("multi-dup-orig");
    let before = (a.snapshot(), b.snapshot(), backup.snapshot(), c.snapshot());

    // a and b share one photo; backup and c are the same content.
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let dup = get(addr, "/raw/api/all/duplicates").json();
    // IMG_0001.JPG and Screenshot.png are the same on both.
    assert_eq!(dup["total_groups"], 2, "{dup}");
    for group in dup["groups"].as_array().unwrap() {
        let files = group["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], files[1]["path"]);
        assert_ne!(files[0]["library"], files[1]["library"]);
    }
    assert_eq!(dup["excluded"].as_array().unwrap().len(), 0);
    server.stop().unwrap();

    // c and its copy hold exactly the same: one of them is the backup, which
    // only the user can tell. Until then neither gets duplicate suggestions.
    let server = start_many(&[&c, &backup], &[]);
    let addr = server.addr;
    let (id_c, id_copy) = (lib_id(addr, &c), lib_id(addr, &backup));
    let drives = get(addr, "/raw/api/all/drives").json();
    assert!(drives.as_array().unwrap().iter().all(|d| d["online"] == true && d["role"] == "unknown" && d["suggested_backup_of"].is_string()));
    let dup = get(addr, "/raw/api/all/duplicates").json();
    assert_eq!(dup["total_groups"], 0);
    assert_eq!(dup["excluded"].as_array().unwrap().len(), 2);
    // The user marks one as the backup: it takes no part, and the other one is
    // no longer suspected.
    let mark = post(addr, "/raw/api/all/role", &json!({ "library": id_copy, "role": "backup" }));
    assert_eq!(mark.status, 200, "{}", String::from_utf8_lossy(&mark.body));
    let dup = get(addr, "/raw/api/all/duplicates").json();
    assert_eq!(dup["total_groups"], 0);
    assert_eq!(dup["excluded"].as_array().unwrap().len(), 1);
    assert_eq!(dup["excluded"][0]["library"], id_copy.as_str());
    assert!(dup["excluded"][0]["reason"].as_str().unwrap().contains("backup"));
    // Said "separate" instead: compared again, so everything is a duplicate.
    post(addr, "/raw/api/all/role", &json!({ "library": id_copy, "role": "separate" }));
    assert_eq!(get(addr, "/raw/api/all/duplicates").json()["total_groups"], 6);
    // The role lives in that drive's own library.db and survives a restart.
    post(addr, "/raw/api/all/role", &json!({ "library": id_copy, "role": "backup" }));
    assert_eq!(backup.count("SELECT count(*) FROM settings WHERE key = 'role' AND value = 'backup'"), 1);
    assert_eq!(c.count("SELECT count(*) FROM settings WHERE key = 'role'"), 0);
    let bad = post(addr, "/raw/api/all/role", &json!({ "library": id_c, "role": "nonsense" }));
    assert_eq!(bad.status, 400);
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": "00000000", "role": "backup" })).status, 404);
    server.stop().unwrap();
    let server = start_many(&[&c, &backup], &[]);
    let drives = get(server.addr, "/raw/api/all/drives").json();
    assert!(drives.as_array().unwrap().iter().any(|d| d["role"] == "backup"));
    server.stop().unwrap();

    // Only comparing happened: no photo changed on any drive.
    assert_eq!((a.snapshot(), b.snapshot(), backup.snapshot(), c.snapshot()), before);
}

#[test]
fn a_drive_that_holds_nothing_new_is_suggested_as_a_backup() {
    let main = own_library("multi-bk-main", 10);
    // The backup has everything of main, and main has more.
    let copy = own_library("multi-bk-copy", 10);
    main.jpeg("Neu/extra1.jpg", 200);
    main.jpeg("Neu/extra2.jpg", 201);
    main.jpeg("Neu/extra3.jpg", 202);
    main.jpeg("Neu/extra4.jpg", 203);
    main.jpeg("Neu/extra5.jpg", 204);
    main.jpeg("Neu/extra6.jpg", 205);
    main.jpeg("Neu/extra7.jpg", 206);
    main.jpeg("Neu/extra8.jpg", 207);
    main.jpeg("Neu/extra9.jpg", 208);
    main.jpeg("Neu/extra10.jpg", 209);
    main.scan_opts(true, false, false);

    let server = start_many(&[&main, &copy], &[]);
    let addr = server.addr;
    let (id_main, id_copy) = (lib_id(addr, &main), lib_id(addr, &copy));
    let drives = get(addr, "/raw/api/all/drives").json();
    let state = |id: &str| drives.as_array().unwrap().iter().find(|d| d["library"] == id).unwrap().clone();
    assert_eq!(state(&id_copy)["suggested_backup_of"], main.root.file_name().unwrap().to_str().unwrap());
    assert!(state(&id_main)["suggested_backup_of"].is_null());
    // Until the user decides, the probable backup gets no duplicate suggestions.
    let dup = get(addr, "/raw/api/all/duplicates").json();
    assert_eq!(dup["total_groups"], 0, "{dup}");
    assert!(dup["excluded"][0]["reason"].as_str().unwrap().contains("looks like a backup"));
    // "Separate" overrides the suggestion.
    post(addr, "/raw/api/all/role", &json!({ "library": id_copy, "role": "separate" }));
    assert_eq!(get(addr, "/raw/api/all/duplicates").json()["total_groups"], 6);
    server.stop().unwrap();
}

#[test]
fn people_with_the_same_name_on_different_drives_are_one_person() {
    let a = own_library("multi-people-a", 10);
    let b = own_library("multi-people-b", 100);
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let (ia, ib) = (lib_id(addr, &a), lib_id(addr, &b));
    let mk = |id: &str, name: &str| {
        let r = post(addr, &format!("/raw/api/lib/{id}/people"), &json!({ "name": name }));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    };
    mk(&ia, "Anna");
    mk(&ia, "Ben");
    mk(&ib, "anna"); // same person: case does not matter
    mk(&ib, "Zo\u{eb}"); // NFC
    mk(&ia, "Zoe\u{308}"); // NFD, the same name after normalising

    let all = get(addr, "/raw/api/all/people").json();
    let people = all["people"].as_array().unwrap();
    let names: Vec<_> = people.iter().map(|p| p["name"].as_str().unwrap().to_lowercase()).collect();
    assert_eq!(people.len(), 3, "{names:?}");
    let anna = people.iter().find(|p| p["name"].as_str().unwrap().eq_ignore_ascii_case("anna")).unwrap();
    assert_eq!(anna["libraries"].as_array().unwrap().len(), 2, "Anna is known on both drives");
    let ben = people.iter().find(|p| p["name"] == "Ben").unwrap();
    assert_eq!(ben["libraries"].as_array().unwrap().len(), 1);
    let zoe = people.iter().find(|p| p["name"] == "Zo\u{eb}").unwrap();
    assert_eq!(zoe["libraries"].as_array().unwrap().len(), 2);
    assert_eq!(all["offline"].as_array().unwrap().len(), 0);

    // Everything stays in each drive's own database.
    assert_eq!(a.count("SELECT count(*) FROM people"), 3);
    assert_eq!(b.count("SELECT count(*) FROM people"), 2);
    server.stop().unwrap();
}

#[test]
fn offline_drives_are_listed_and_the_cross_drive_routes_need_the_same_safety() {
    let a = own_library("multi-safe-a", 10);
    let b = own_library("multi-safe-b", 100);
    let gone = std::env::temp_dir().join(format!("shoebox-it-{}-multi-safe-unplugged", std::process::id()));
    let server = start_many(&[&a, &b], &[gone.as_path()]);
    let addr = server.addr;
    let all = get(addr, "/raw/api/all/people").json();
    assert_eq!(all["offline"].as_array().unwrap().len(), 1);
    let drives = get(addr, "/raw/api/all/drives").json();
    assert_eq!(drives.as_array().unwrap().iter().filter(|d| d["online"] == false).count(), 1);
    let json_ct = ("Content-Type", "application/json");
    // Changes need X-Shoebox.
    let id = lib_id(addr, &a);
    let body = format!(r#"{{"library":"{id}","role":"backup"}}"#);
    assert_eq!(bare_request(addr, "POST", "/raw/api/all/role", &[json_ct], body.as_bytes()).status, 403);
    // The offline drive cannot be changed.
    let off = drives.as_array().unwrap().iter().find(|d| d["online"] == false).unwrap()["library"].as_str().unwrap().to_string();
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": off, "role": "backup" })).status, 503);
    // Nothing was created where the missing drive should be.
    assert!(!gone.exists());
    server.stop().unwrap();
}
