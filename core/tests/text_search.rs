//! Searching the text read in photos (phase 9): `text=` and `name=` in the
//! timeline, the lines of a photo, hiding a line, the limits, the summary and
//! forgetting it all, with the fake worker's text task (its header lists the
//! lines every picture "contains"). Originals stay untouched throughout.

mod common;

use std::path::PathBuf;

use common::*;
use serde_json::json;
use shoebox::recognize;

fn empty(test: &str) -> Library {
    let lib = Library::new(test);
    for entry in std::fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
    }
    lib
}

fn solid(lib: &Library, rel: &str, rgb: [u8; 3]) {
    let p = lib.path(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(200, 100, image::Rgb(rgb)).save(p).unwrap();
}

/// Read the text of every photo with the fake worker.
fn read_text(lib: &Library) {
    let stats = recognize::run(&recognize::Options {
        root: lib.root.clone(),
        db: None,
        recognizer: Some(PathBuf::from(env!("CARGO_BIN_EXE_shoebox-fake-recognizer"))),
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

fn hits(addr: std::net::SocketAddr, query: &str) -> Vec<i64> {
    let t = get(addr, &format!("/api/timeline?{query}"));
    assert_eq!(t.status, 200, "{query}");
    ids(&t.json())
}

/// Two photos that "contain" the fake's lines, and one that is dark (none).
fn library(test: &str) -> Library {
    let lib = empty(test);
    solid(&lib, "Docs/scan-2024.jpg", [200, 190, 180]);
    solid(&lib, "Docs/Foto 1.jpg", [120, 130, 140]);
    solid(&lib, "Dark/night.jpg", [5, 5, 5]);
    lib.scan_opts(true, false, false);
    read_text(&lib);
    lib
}

#[test]
fn text_search_finds_photos_by_the_words_in_them() {
    let lib = library("text-search");
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let (scan, foto, night) = (id_of(&lib, "Docs/scan-2024.jpg"), id_of(&lib, "Docs/Foto 1.jpg"), id_of(&lib, "Dark/night.jpg"));
    let both = {
        let mut v = vec![scan, foto];
        v.sort();
        v
    };
    let sorted = |mut v: Vec<i64>| {
        v.sort();
        v
    };

    // A word, a part of a word (trigram index), another case, ß as ss, digits.
    for q in ["text=Rechnung", "text=rechn", "text=RECHNUNG", "text=2024", "text=strasse", "text=Straße", "text=STRAßE"] {
        assert_eq!(sorted(hits(addr, q)), both, "{q}");
    }
    // Words shorter than three letters are looked for in the lines.
    assert_eq!(sorted(hits(addr, "text=nr")), both);
    assert!(hits(addr, "text=zz").is_empty());
    // Every word must be found (in any line); one that is not makes it empty.
    assert_eq!(sorted(hits(addr, "text=rechnung%202024")), both);
    assert_eq!(sorted(hits(addr, "text=strasse&text=2024")), both);
    assert!(hits(addr, "text=rechnung%20zebra").is_empty());
    // The dark photo has no text; lines the recogniser was unsure of were not stored.
    assert!(!hits(addr, "text=rechnung").contains(&night));
    assert!(hits(addr, "text=unsure").is_empty());
    // A line below the limit (0.7) is stored but not found.
    assert!(hits(addr, "text=faint").is_empty());
    assert!(hits(addr, "text=tiny").is_empty());
    // An empty term is no restriction.
    assert_eq!(hits(addr, "text=").len(), 3);

    // The matching line comes with the hit.
    let t = get(addr, "/api/timeline?text=strasse").json();
    let snips = t["snips"].as_array().unwrap();
    assert_eq!(snips.len(), 2);
    assert!(snips.iter().all(|s| s[1] == "Straße 12"), "{snips:?}");
    assert!(get(addr, "/api/timeline").json().get("snips").is_none(), "no snippets without a text term");

    // File names only: not folders, tags or people.
    assert_eq!(hits(addr, "name=scan"), vec![scan]);
    assert_eq!(hits(addr, "name=SCAN"), vec![scan]);
    assert_eq!(hits(addr, "name=foto%201"), vec![foto]);
    assert_eq!(hits(addr, "name=2024"), vec![scan]);
    assert!(hits(addr, "name=docs").is_empty(), "a folder is not a file name");
    assert_eq!(sorted(hits(addr, "q=docs")), both, "plain words still look at the folder");
    // They combine with everything else (and with each other).
    assert_eq!(hits(addr, "text=rechnung&name=scan"), vec![scan]);
    assert!(hits(addr, "text=rechnung&name=night").is_empty());
    let folder = lib.count("SELECT id FROM folders WHERE name = 'Docs'");
    assert_eq!(sorted(hits(addr, &format!("text=rechnung&folder={folder}"))), both);
    assert_eq!(hits(addr, "name=scan&name=2024"), vec![scan]);
    assert!(hits(addr, "name=scan&name=foto").is_empty());
    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "searching changed an original");
}

#[test]
fn the_lines_of_a_photo_can_be_hidden_and_shown_again() {
    let lib = library("text-hidden");
    let server = start(&lib, None);
    let addr = server.addr;
    let (scan, foto) = (id_of(&lib, "Docs/scan-2024.jpg"), id_of(&lib, "Docs/Foto 1.jpg"));

    let info = get(addr, &format!("/api/files/{scan}")).json();
    let lines = info["text"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "only the lines above the limits: {lines:?}");
    assert_eq!(lines[0]["text"], "Rechnung Nr. 2024");
    assert_eq!(lines[1]["text"], "Straße 12");
    assert_eq!(lines[1]["norm"], "strasse 12");
    assert_eq!(lines[0]["hidden"], false);
    // Boxes are fractions of the picture.
    let (x, y, w, h) = (lines[0]["x"].as_f64().unwrap(), lines[0]["y"].as_f64().unwrap(), lines[0]["w"].as_f64().unwrap(), lines[0]["h"].as_f64().unwrap());
    assert!((x - 0.1).abs() < 1e-9 && (y - 0.1).abs() < 1e-9 && (w - 0.5).abs() < 1e-9 && (h - 0.08).abs() < 1e-9);

    // Hide "Straße 12" of one photo: it no longer finds it, the other photo is unaffected.
    assert_eq!(post(addr, &format!("/api/files/{scan}/text-hidden"), &json!({ "norm": "strasse 12", "hidden": true })).status, 200);
    assert_eq!(hits(addr, "text=strasse"), vec![foto]);
    assert_eq!(hits(addr, "text=rechnung").len(), 2, "its other lines still find it");
    let info = get(addr, &format!("/api/files/{scan}")).json();
    assert_eq!(info["text"][1]["hidden"], true);
    assert_eq!(info["text"].as_array().unwrap().len(), 2, "a hidden line is still listed, marked");
    assert_eq!(lib.count("SELECT count(*) FROM text_hidden"), 1);
    // A line the photo does not have is refused, and so is a missing header.
    assert_eq!(post(addr, &format!("/api/files/{scan}/text-hidden"), &json!({ "norm": "no such line", "hidden": true })).status, 400);
    assert_eq!(
        bare_request(addr, "POST", &format!("/api/files/{scan}/text-hidden"), &[("Content-Type", "application/json")], br#"{"norm":"strasse 12","hidden":false}"#).status,
        403
    );
    // It is part of the user data backup.
    assert!(shoebox::tags::user_data(&lib.db()).map(|d| serde_json::to_value(d).unwrap()).unwrap()["text_hidden"][0]["norm"] == "strasse 12");
    // Show it again.
    assert_eq!(post(addr, &format!("/api/files/{scan}/text-hidden"), &json!({ "norm": "strasse 12", "hidden": false })).status, 200);
    assert_eq!(hits(addr, "text=strasse").len(), 2);
    assert_eq!(lib.count("SELECT count(*) FROM text_hidden"), 0);
    server.stop().unwrap();
}

#[test]
fn the_limits_decide_which_lines_count() {
    let lib = library("text-limits");
    let server = start(&lib, None);
    let addr = server.addr;
    let limits = get(addr, "/api/text/limits").json();
    assert_eq!((limits["min_score"].as_f64().unwrap(), limits["min_height"].as_f64().unwrap()), (0.7, 0.01));
    assert!(hits(addr, "text=faint").is_empty());

    // Lower the score: the faint line (0.6) is found, and listed in the photo.
    let set = post(addr, "/api/text/limits", &json!({ "min_score": 0.55, "min_height": 0.01 }));
    assert_eq!(set.status, 200);
    assert_eq!(set.json()["min_score"], 0.55);
    assert_eq!(hits(addr, "text=faint").len(), 2);
    let scan = id_of(&lib, "Docs/scan-2024.jpg");
    assert_eq!(get(addr, &format!("/api/files/{scan}")).json()["text"].as_array().unwrap().len(), 3);
    // Raise the size: lines of 6 % and 5 % are too small at 7 %, the 8 % one stays.
    post(addr, "/api/text/limits", &json!({ "min_score": 0.55, "min_height": 0.07 }));
    assert_eq!(hits(addr, "text=rechnung").len(), 2);
    assert!(hits(addr, "text=strasse").is_empty());
    // Out-of-range values are clamped; a missing one is its default again.
    let set = post(addr, "/api/text/limits", &json!({ "min_score": 7.0 })).json();
    assert_eq!((set["min_score"].as_f64().unwrap(), set["min_height"].as_f64().unwrap()), (1.0, 0.01));
    let set = post(addr, "/api/text/limits", &json!({})).json();
    assert_eq!((set["min_score"].as_f64().unwrap(), set["min_height"].as_f64().unwrap()), (0.7, 0.01));
    assert_eq!(hits(addr, "text=strasse").len(), 2);
    server.stop().unwrap();
}

#[test]
fn the_summary_and_forgetting_all_text() {
    let lib = library("text-stats");
    let server = start(&lib, None);
    let addr = server.addr;
    let scan = id_of(&lib, "Docs/scan-2024.jpg");
    post(addr, &format!("/api/files/{scan}/text-hidden"), &json!({ "norm": "faint", "hidden": true }));
    let s = get(addr, "/api/text/stats").json();
    assert_eq!(s["read"], 3, "{s}");
    assert_eq!(s["with_text"], 2);
    assert_eq!((s["lines"].as_i64().unwrap(), s["stored"].as_i64().unwrap()), (4, 6));
    assert_eq!(s["hidden"], 1);
    assert_eq!(s["total"], 3);
    assert_eq!(s["model"], "fake-text-1");

    // Forget it all: nothing is found, the decisions stay, a new run reads again.
    assert_eq!(post(addr, "/api/text/delete-all", &json!({})).json()["lines"], 6);
    assert!(hits(addr, "text=rechnung").is_empty());
    let s = get(addr, "/api/text/stats").json();
    assert_eq!((s["read"].as_i64().unwrap(), s["stored"].as_i64().unwrap(), s["hidden"].as_i64().unwrap()), (0, 0, 1));
    server.stop().unwrap();
    read_text(&lib);
    let server = start(&lib, None);
    assert_eq!(hits(server.addr, "text=rechnung").len(), 2);
    server.stop().unwrap();
}
