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

const SPAN: i64 = 1 << 40;

fn recognize_all(lib: &Library) {
    let stats = shoebox::recognize::run(&shoebox::recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(env!("CARGO_BIN_EXE_shoebox-fake-recognizer").into()),
        limit: None,
        retry_failed: false,
        rotated: false,
        pets: false,
        text: false,
        text_only: false,
        timeouts: Default::default(),
    })
    .unwrap();
    assert_eq!(stats.failed, 0, "{:?}", stats.errors);
}

#[test]
fn one_timeline_over_the_drives_by_tag_person_and_text_but_without_backups() {
    let a = own_library("multi-tl-a", 10);
    let b = own_library("multi-tl-b", 100);
    let backup = copy_library("multi-tl-backup");
    let main = copy_library("multi-tl-main");
    // Anna is on both drives (a plain picture of one colour is one person to the fake).
    for (lib, n) in [(&a, 1u8), (&b, 2u8)] {
        let mut img = image::RgbImage::from_pixel(200, 120, image::Rgb([200, 150, 120]));
        img.put_pixel(0, 0, image::Rgb([n, n, n]));
        std::fs::create_dir_all(lib.path("Anna")).unwrap();
        img.save(lib.path(format!("Anna/anna{n}.png").as_str())).unwrap();
        lib.scan_opts(true, false, false);
        recognize_all(lib);
    }
    let before = (a.snapshot(), b.snapshot(), backup.snapshot(), main.snapshot());
    let server = start_many(&[&a, &b, &backup, &main], &[]);
    let addr = server.addr;
    let (ia, ib, i_backup) = (lib_id(addr, &a), lib_id(addr, &b), lib_id(addr, &backup));
    for (id, lib, n) in [(&ia, &a, 1u8), (&ib, &b, 2u8)] {
        let file = id_of(lib, &format!("Anna/anna{n}.png"));
        let info = get(addr, &format!("/raw/api/lib/{id}/files/{file}")).json();
        let face = info["faces"][0]["id"].as_i64().expect("the fake found a face");
        let r = post(addr, &format!("/raw/api/lib/{id}/faces/assign"), &json!({ "faces": [face], "name": "Anna" }));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }
    // The copy of `main` is a backup; `a` and `b` are separate drives.
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": i_backup, "role": "backup" })).status, 200);

    // No filter: everything on the drives that take part, newest first.
    let all = get(addr, "/raw/api/all/timeline").json();
    let libs: Vec<String> = all["libs"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(libs.len(), 3, "{libs:?}: the backup is left out");
    assert!(!libs.contains(&backup.root.file_name().unwrap().to_str().unwrap().to_string()));
    assert_eq!(all["left_out"][0]["library"], i_backup.as_str());
    let days: Vec<i64> = all["days"].as_array().unwrap().iter().map(|d| d.as_i64().unwrap()).collect();
    assert!(days.windows(2).all(|w| w[0] >= w[1]), "one list, newest first: {days:?}");
    let ids = ids(&all);
    assert_eq!(ids.len(), all["count"].as_u64().unwrap() as usize);
    let drives_seen: std::collections::BTreeSet<i64> = ids.iter().map(|g| g / SPAN).collect();
    assert_eq!(drives_seen.len(), 3, "photos of every drive in one list");

    // A person by name: photos of Anna from both drives, in one list.
    let anna = get(addr, "/raw/api/all/timeline?person=anna").json();
    let from: Vec<i64> = ids_of(&anna).iter().map(|g| g / SPAN).collect();
    assert_eq!(from.len(), 2, "{anna}");
    assert_ne!(from[0], from[1], "one from each drive");
    // A name only one drive knows: only that drive's photos (all the filters must match).
    assert_eq!(get(addr, "/raw/api/all/timeline?person=Anna&person=Nobody").json()["count"], 0);
    // A tag by name (the folder `Familie/Weihnachten` exists on every drive).
    let xmas = get(addr, "/raw/api/all/timeline?tag=weihnachten").json();
    assert_eq!(xmas["count"], 3);
    assert_eq!(get(addr, "/raw/api/all/timeline?tag=weihnachten&person=Anna").json()["count"], 0);
    // Free text.
    assert_eq!(get(addr, "/raw/api/all/timeline?q=anna").json()["count"].as_u64().unwrap(), 2);

    // The id says where a file is: thumbnails and originals come from its own drive.
    let g = ids_of(&anna)[0];
    let (drive, local) = ((g / SPAN) as usize, g % SPAN);
    let lib_of = anna["libs"][drive]["id"].as_str().unwrap();
    assert_eq!(get(addr, &format!("/raw/api/lib/{lib_of}/files/{local}/thumb")).status, 200);

    // Suggestions: tags by name over the drives that take part.
    let tags = get(addr, "/raw/api/all/tags?q=weih").json();
    assert_eq!(tags[0]["name"], "Weihnachten");
    assert_eq!(tags[0]["count"], 3);
    server.stop().unwrap();
    assert_eq!((a.snapshot(), b.snapshot(), backup.snapshot(), main.snapshot()), before, "only reading");
}

fn ids_of(timeline: &serde_json::Value) -> Vec<i64> {
    ids(timeline)
}

/// A pet by its species over the drives that take part (`pet=` is not a
/// name: it means the same on every drive), also as typed words.
#[test]
fn pet_terms_work_over_the_drives() {
    let make = |test: &str, pictures: &[(&str, [u8; 3], u8)]| {
        let lib = Library::new(test);
        for entry in std::fs::read_dir(&lib.root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
        }
        for (rel, rgb, n) in pictures {
            let mut img = image::RgbImage::from_pixel(200, 120, image::Rgb(*rgb));
            img.put_pixel(0, 0, image::Rgb([*n, *n, *n]));
            std::fs::create_dir_all(lib.path(rel).parent().unwrap()).unwrap();
            img.save(lib.path(rel)).unwrap();
        }
        lib.scan_opts(true, false, false);
        let stats = shoebox::recognize::run(&shoebox::recognize::Options {
            root: lib.root.clone(),
            db: None,
            recognizer: Some(env!("CARGO_BIN_EXE_shoebox-fake-recognizer").into()),
            limit: None,
            retry_failed: false,
            rotated: false,
            pets: true,
            text: false,
            text_only: false,
            timeouts: Default::default(),
        })
        .unwrap();
        assert_eq!(stats.failed, 0, "{:?}", stats.errors);
        lib
    };
    // A red picture is a cat to the fake, a blue one a dog.
    let a = make("multi-pets-a", &[("Fotos/miez.png", [200, 60, 40], 1)]);
    let b = make("multi-pets-b", &[("Fotos/mieze.png", [200, 60, 40], 2), ("Fotos/bello.png", [40, 60, 200], 1)]);
    let before = (a.snapshot(), b.snapshot());
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let count = |path: &str| get(addr, path).json()["count"].as_u64().unwrap();
    assert_eq!(count("/raw/api/all/timeline?pet=cat"), 2);
    assert_eq!(count("/raw/api/all/timeline?pet=dog"), 1);
    assert_eq!(count("/raw/api/all/timeline?pet=pet"), 3);
    assert_eq!(count("/raw/api/all/timeline?pet=cat&pet=dog"), 0);
    assert_eq!(count("/raw/api/all/timeline?q=katze"), 2);
    assert_eq!(count("/raw/api/all/timeline?q=hund"), 1);
    let cats = get(addr, "/raw/api/all/timeline?pet=cat").json();
    let drives: std::collections::BTreeSet<i64> = ids_of(&cats).iter().map(|g| g / SPAN).collect();
    assert_eq!(drives.len(), 2, "cats of both drives in one list");
    assert_eq!(get(addr, "/raw/api/all/timeline?pet=cow").status, 400);
    // The suggestions are per drive: each answers for itself.
    let (ia, ib) = (lib_id(addr, &a), lib_id(addr, &b));
    assert_eq!(get(addr, &format!("/raw/api/lib/{ia}/pets/search")).json().as_array().unwrap().len(), 2, "any pet and cats here");
    assert_eq!(get(addr, &format!("/raw/api/lib/{ib}/pets/search")).json().as_array().unwrap().len(), 3);
    assert_eq!((a.snapshot(), b.snapshot()), before);
    server.stop().unwrap();
}

fn read_text_of(lib: &Library) {
    let stats = shoebox::recognize::run(&shoebox::recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(env!("CARGO_BIN_EXE_shoebox-fake-recognizer").into()),
        limit: None,
        retry_failed: false,
        rotated: false,
        pets: false,
        text: true,
        text_only: true,
        timeouts: Default::default(),
    })
    .unwrap();
    assert_eq!(stats.text.unwrap().failed, 0);
}

/// The words in the photos and the file names are searched on every drive that
/// takes part, and the answer is one list with the matching line of each hit.
#[test]
fn text_and_name_terms_work_over_the_drives() {
    let a = own_library("multi-text-a", 10);
    let b = own_library("multi-text-b", 100);
    read_text_of(&a);
    read_text_of(&b);
    let before = (a.snapshot(), b.snapshot());
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;

    let all = get(addr, "/raw/api/all/timeline").json();
    let every = all["count"].as_u64().unwrap();
    assert!(every >= 8);
    let text = get(addr, "/raw/api/all/timeline?text=rechnung").json();
    let drives: std::collections::BTreeSet<i64> = ids_of(&text).iter().map(|g| g / SPAN).collect();
    assert_eq!(drives.len(), 2, "hits on both drives");
    assert!(text["count"].as_u64().unwrap() > 0 && text["count"].as_u64().unwrap() <= every);
    let snips = text["snips"].as_array().unwrap();
    assert_eq!(snips.len() as u64, text["count"].as_u64().unwrap(), "a snippet for every hit");
    assert!(snips.iter().all(|s| s[1] == "Rechnung Nr. 2024"));
    let listed: std::collections::BTreeSet<i64> = ids_of(&text).into_iter().collect();
    assert!(snips.iter().all(|s| listed.contains(&s[0].as_i64().unwrap())), "snippets are for ids of the list");
    assert_eq!(get(addr, "/raw/api/all/timeline?text=zebra").json()["count"], 0);
    assert_eq!(get(addr, "/raw/api/all/timeline?text=rechnung&text=strasse").json()["count"], text["count"]);

    // File names: this one is on drive a and b alike (the same name in each).
    let named = get(addr, "/raw/api/all/timeline?name=img_0002").json();
    assert_eq!(named["count"], 2, "{named}");
    assert!(get(addr, "/raw/api/all/timeline?name=urlaub").json()["count"] == 0, "folder names are not file names");
    server.stop().unwrap();
    assert_eq!((a.snapshot(), b.snapshot()), before, "searching changed an original");
}

#[test]
fn the_common_timeline_shows_dates_the_user_gave_and_filters_by_them() {
    let a = own_library("multi-dates-a", 10);
    let b = own_library("multi-dates-b", 90);
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let id_a = lib_id(addr, &a);
    let photo = id_of(&a, "Familie/Weihnachten/DSC_2001.jpg");
    let r = post(addr, &format!("/raw/api/lib/{id_a}/files/dates"), &json!({ "ids": [photo], "year": 1987, "month": 6 }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));

    // Only that photo is dated 1987, and it carries the mark of a date the user gave.
    let found = get(addr, "/raw/api/all/timeline?date=1987").json();
    assert_eq!(found["count"], 1);
    let est = found["est"].as_array().unwrap();
    assert_eq!((est.len(), est[0][1].as_str()), (1, Some("m")));
    assert_eq!(found["days"][0], 19870600);
    // Photos that need a date are the others; a bad date is refused.
    let needs = get(addr, "/raw/api/all/timeline?nodate=1").json();
    assert!(needs["count"].as_i64().unwrap() >= 1);
    assert!(!needs["ids"].as_array().unwrap().contains(&found["ids"][0]));
    assert_eq!(get(addr, "/raw/api/all/timeline?date=87").status, 400);
    server.stop().unwrap();
}

/// A library with only the photos the test writes itself.
fn bare_library(test: &str) -> Library {
    let lib = Library::new(test);
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
    }
    lib
}

fn tag_counts(addr: std::net::SocketAddr, query: &str) -> std::collections::BTreeMap<String, u64> {
    get(addr, &format!("/raw/api/all/tags?own=1&limit=100{query}"))
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["count"].as_u64().unwrap()))
        .collect()
}

fn person_photos(addr: std::net::SocketAddr, query: &str, name: &str) -> Option<u64> {
    get(addr, &format!("/raw/api/all/people{query}"))
        .json()["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .map(|p| p["photos"].as_u64().unwrap())
}

/// Phase 15a: in All drives the suggestions narrow with the filter, as on a
/// single drive: counts are the photos the list then shows, and tags and
/// people that would show nothing are left out.
#[test]
fn suggestions_over_all_drives_narrow_with_the_filter() {
    let a = bare_library("multi-narrow-a");
    let b = bare_library("multi-narrow-b");
    for (lib, files) in [(&a, [("x/a1.jpg", 40u8), ("x/a2.jpg", 41), ("x/a3.jpg", 42)].as_slice()), (&b, [("y/b1.jpg", 50), ("y/b2.jpg", 51)].as_slice())] {
        for (path, seed) in files {
            lib.jpeg(path, *seed);
        }
        lib.scan_opts(true, false, false);
    }
    let tag = |lib: &Library, path: &str, name: &str| {
        let conn = shoebox::db::open_shared(&lib.path(".shoebox/library.db")).unwrap();
        shoebox::tags::add(&conn, &[id_of(lib, path)], name).unwrap();
    };
    for (path, name) in [("x/a1.jpg", "Ski"), ("x/a1.jpg", "Winter"), ("x/a2.jpg", "Winter"), ("x/a3.jpg", "Sommer")] {
        tag(&a, path, name);
    }
    for (path, name) in [("y/b1.jpg", "Ski"), ("y/b1.jpg", "Winter"), ("y/b2.jpg", "Strand")] {
        tag(&b, path, name);
    }
    // Anna is on one photo of each drive.
    for (lib, path) in [(&a, "x/a1.jpg"), (&b, "y/b2.jpg")] {
        let conn = lib.db();
        conn.execute("INSERT INTO people (name) VALUES ('Anna')", []).unwrap();
        conn.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
             VALUES (?1, 0.1, 0.1, 0.2, 0.2, (SELECT id FROM people WHERE name = 'Anna'), 'confirmed', 1, 1)",
            [lib.record_key(path)],
        )
        .unwrap();
    }
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let count_of = |q: &str| get(addr, &format!("/raw/api/all/timeline{q}")).json()["count"].as_u64().unwrap();

    // No filter: every tag counted over both drives.
    let all = tag_counts(addr, "");
    assert_eq!((all["Winter"], all["Ski"], all["Sommer"], all["Strand"]), (3, 2, 1, 1));
    assert_eq!(person_photos(addr, "", "Anna"), Some(2));

    // With a tag: only the photos that have it, and only tags that appear on them.
    let ski = tag_counts(addr, "&tag=Ski");
    assert_eq!(ski.into_iter().collect::<Vec<_>>(), vec![("Winter".to_string(), 2)], "the tag itself, Sommer and Strand are left out");
    // Anna is on a1 (Ski) on one drive, and on b2 (no Ski) on the other: only the first counts.
    assert_eq!(person_photos(addr, "?tag=Ski", "Anna"), Some(1));
    assert_eq!(person_photos(addr, "?tag=Sommer", "Anna"), None, "Anna is on no photo with Sommer");
    // With a person: a drive that has her contributes its photos of her.
    let anna = tag_counts(addr, "&person=Anna");
    assert_eq!(anna.into_iter().collect::<Vec<_>>(), vec![("Ski".to_string(), 1), ("Strand".to_string(), 1), ("Winter".to_string(), 1)]);
    // A tag only one drive has: the other contributes nothing.
    assert_eq!(person_photos(addr, "?tag=Strand", "Anna"), Some(1));
    assert!(tag_counts(addr, "&tag=Strand").is_empty(), "nothing else on that photo");
    assert!(tag_counts(addr, "&tag=Sommer").is_empty());
    // The typed word still narrows by name.
    assert_eq!(tag_counts(addr, "&q=win").into_iter().collect::<Vec<_>>(), vec![("Winter".to_string(), 3)]);

    // A count always equals what the list then shows.
    for filter in ["", "&tag=Ski", "&tag=Winter", "&person=Anna", "&tag=Winter&person=Anna"] {
        let base = filter.trim_start_matches('&');
        for (name, n) in tag_counts(addr, filter) {
            let q = if base.is_empty() { format!("?tag={name}") } else { format!("?{base}&tag={name}") };
            assert_eq!(count_of(&q), n, "{name} within {filter:?}");
        }
    }
    assert_eq!(person_photos(addr, "?tag=Winter", "Anna"), Some(count_of("?tag=Winter&person=Anna")));
    assert_eq!(person_photos(addr, "?tag=Strand", "Anna"), Some(count_of("?tag=Strand&person=Anna")));
    server.stop().unwrap();
}

/// Two drives that share five photos in folders that are named alike on one
/// drive only, on both, or not at all (seed → folders of the copies on A and B):
/// 1 `Noah/2025-07 Kroatien` and `Lena/Urlaub Kroatien`, 2 `Gemeinsam/Festival`
/// on both, 3 `Downloads` and `Lena/Handy 2025`, 4 `Downloads` and
/// `2025-07 Kroatien`, 5 `O\u{308}sa` (NFD) and `Other`. Each has photos of its own.
fn shared_folders(test: &str) -> (Library, Library) {
    let (a, b) = (bare_library(&format!("{test}-a")), bare_library(&format!("{test}-b")));
    a.jpeg("Noah/2025-07 Kroatien/a.jpg", 1);
    b.jpeg("Lena/Urlaub Kroatien/a.jpg", 1);
    a.jpeg("Gemeinsam/Festival/b.jpg", 2);
    b.jpeg("Gemeinsam/Festival/b.jpg", 2);
    a.jpeg("Downloads/c.jpg", 3);
    b.jpeg("Lena/Handy 2025/c.jpg", 3);
    a.jpeg("Downloads/d.jpg", 4);
    b.jpeg("2025-07 Kroatien/d.jpg", 4);
    a.jpeg("O\u{308}sa/e.jpg", 5);
    b.jpeg("Other/e.jpg", 5);
    for seed in 20..25 {
        a.jpeg(&format!("Eigene/a{seed}.jpg"), seed);
        b.jpeg(&format!("Eigene/b{seed}.jpg"), seed + 20);
    }
    a.scan_opts(true, false, false);
    b.scan_opts(true, false, false);
    (a, b)
}

fn ignore_query(names: &[&str]) -> String {
    names.iter().map(|n| format!("&ignore={}", encode(n))).collect()
}

#[test]
fn a_photo_with_a_copy_in_an_ignored_folder_is_left_out_with_all_its_copies() {
    let (a, b) = shared_folders("multi-ignore");
    let before = (a.snapshot(), b.snapshot());
    let server = start_many(&[&a, &b], &[]);
    let addr = server.addr;
    let dups = |names: &[&str]| get(addr, &format!("/raw/api/all/duplicates?limit=200{}", ignore_query(names))).json();

    // Nothing ignored: all five, as before.
    let all = dups(&[]);
    assert_eq!((all["total_groups"].as_u64(), all["hidden_groups"].as_u64()), (Some(5), Some(0)), "{all}");
    assert_eq!(all["groups"].as_array().unwrap().len(), 5);

    // Seeds 1 and 4: one copy lies in `2025-07 Kroatien`, the other copy's folder is named otherwise.
    let kroatien = dups(&["2025-07 Kroatien"]);
    assert_eq!((kroatien["total_groups"].as_u64(), kroatien["hidden_groups"].as_u64()), (Some(3), Some(2)), "{kroatien}");
    let paths: Vec<&str> = kroatien["groups"].as_array().unwrap().iter().flat_map(|g| g["files"].as_array().unwrap()).map(|f| f["path"].as_str().unwrap()).collect();
    assert!(paths.iter().all(|p| !p.ends_with("/a.jpg") && !p.ends_with("/d.jpg")), "{paths:?}");

    // The name of the other drive's folder hides the same photo; a folder above the photo counts.
    assert_eq!(dups(&["Urlaub Kroatien"])["hidden_groups"], 1);
    assert_eq!(dups(&["Noah"])["hidden_groups"], 1);
    // Several names, in any case and form: `Downloads` has seeds 3 and 4, `O\u{308}sa` seed 5.
    let some = dups(&["downloads", "\u{d6}SA", "  "]);
    assert_eq!((some["total_groups"].as_u64(), some["hidden_groups"].as_u64()), (Some(2), Some(3)), "{some}");
    // A name that is no folder, a file's name and a part of a path hide nothing.
    for name in ["Festiva", "a.jpg", "Gemeinsam/Festival", "Eigene2"] {
        assert_eq!(dups(&[name])["hidden_groups"], 0, "{name}");
    }
    // The limit counts what is left.
    let few = dups(&["2025-07 Kroatien"]);
    assert_eq!(few["groups"].as_array().unwrap().len(), 3);
    let one = get(addr, &format!("/raw/api/all/duplicates?limit=1{}", ignore_query(&["2025-07 Kroatien"]))).json();
    assert_eq!((one["groups"].as_array().unwrap().len(), one["total_groups"].as_u64()), (1, Some(3)));
    server.stop().unwrap();

    // It is a filter: no photo and no index changed.
    assert_eq!((a.snapshot(), b.snapshot()), before);
}

#[test]
fn folder_names_are_offered_with_the_photos_they_would_hide() {
    let (a, b) = shared_folders("multi-ignore-names");
    let backup = bare_library("multi-ignore-names-bk");
    backup.jpeg("Sicherung/a.jpg", 1);
    backup.scan_opts(true, false, false);
    let server = start_many(&[&a, &b, &backup], &[]);
    let addr = server.addr;
    let (ia, ib, ic) = (lib_id(addr, &a), lib_id(addr, &b), lib_id(addr, &backup));
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": ic, "role": "backup" })).status, 200);
    let folders = |query: &str| -> Vec<(String, u64, Vec<String>)> {
        get(addr, &format!("/raw/api/all/duplicates/folders?{query}"))
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let drives = f["drives"].as_array().unwrap().iter().map(|d| d["library"].as_str().unwrap().to_string()).collect();
                (f["name"].as_str().unwrap().to_string(), f["photos"].as_u64().unwrap(), drives)
            })
            .collect()
    };
    let names = |rows: &[(String, u64, Vec<String>)]| rows.iter().map(|r| r.0.clone()).collect::<Vec<_>>();

    // Names on both drives first, then more photos, then alphabetical. A backup adds nothing.
    let all = folders("limit=50");
    assert_eq!(
        names(&all),
        ["2025-07 Kroatien", "Festival", "Gemeinsam", "Downloads", "Lena", "Handy 2025", "Noah", "\u{d6}sa", "Other", "Urlaub Kroatien"],
        "{all:?}"
    );
    let row = |name: &str| all.iter().find(|r| r.0 == name).unwrap().clone();
    let mut both = vec![ia.clone(), ib.clone()];
    both.sort();
    assert_eq!((row("2025-07 Kroatien").1, row("2025-07 Kroatien").2), (2, both));
    assert_eq!((row("Downloads").1, row("Downloads").2), (2, vec![ia.clone()]));
    assert_eq!((row("Lena").1, row("Lena").2), (2, vec![ib.clone()]));
    // The default is 8 rows, `limit` asks for fewer.
    assert_eq!(folders("").len(), 8);
    assert_eq!(names(&folders("limit=2")), ["2025-07 Kroatien", "Festival"]);

    // Names already ignored are not offered, and the counts are what is still visible.
    let rest = folders(&format!("limit=50{}", ignore_query(&["2025-07 Kroatien"])));
    assert_eq!(names(&rest), ["Festival", "Gemeinsam", "Downloads", "Handy 2025", "Lena", "\u{d6}sa", "Other"], "{rest:?}");
    assert_eq!(rest.iter().find(|r| r.0 == "Lena").unwrap().1, 1);

    // What was typed: names that start with it first, then names that contain it; accents, case and form do not matter.
    assert_eq!(names(&folders("limit=3&q=o")), ["\u{d6}sa", "Other", "2025-07 Kroatien"]);
    assert_eq!(names(&folders("q=OSA")), ["\u{d6}sa"]);
    assert_eq!(names(&folders(&format!("q={}", encode("o\u{308}sa")))), ["\u{d6}sa"]);
    assert_eq!(names(&folders("q=kroatien")), ["2025-07 Kroatien", "Urlaub Kroatien"]);
    assert!(folders("q=nothing+like+it").is_empty());
    server.stop().unwrap();
}
