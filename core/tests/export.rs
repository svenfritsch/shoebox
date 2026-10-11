//! Export (phase 12): copies into a new folder under the guard, numbering,
//! RAW and Live Photo partners, merging into a folder that exists, and that
//! only this computer may export.

mod common;

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use shoebox::fingerprint;

const DAY_NS: i128 = 86_400 * 1_000_000_000;
const BASE_NS: i128 = 1_600_000_000 * 1_000_000_000;

/// A library of photos with known capture order (by modified time): the two
/// Greece photos with a RAW and a Live Photo video for the first, then a
/// Christmas photo. Returns the ids in the order they were taken.
fn library(name: &str) -> (Library, Vec<i64>) {
    let lib = Library::new(name);
    // Only what these tests name: no fixtures and no other samples in the way.
    for rel in [
        "Familie/Screenshot.png",
        "Familie/notes.txt",
        "Ordner mit Leerzeichen/Bild 1.jpeg",
    ] {
        fs::remove_file(lib.path(rel)).unwrap();
    }
    fs::remove_dir_all(lib.path(NFD_DIR)).unwrap();
    lib.write("2020-07 Urlaub Griechenland/IMG_0001.CR2", b"not really a raw file, but named like one");
    lib.write("2020-07 Urlaub Griechenland/IMG_0001.MOV", b"not really a video, but named like one");
    set_times(&lib, &[
        ("2020-07 Urlaub Griechenland/IMG_0001.JPG", 1),
        ("2020-07 Urlaub Griechenland/IMG_0001.CR2", 1),
        ("2020-07 Urlaub Griechenland/IMG_0001.MOV", 1),
        ("2020-07 Urlaub Griechenland/IMG_0002.JPG", 2),
        ("Familie/Weihnachten/DSC_2001.jpg", 3),
    ]);
    lib.scan_opts(true, false, false);
    // A Live Photo video is a short one; the index has no duration for this stand-in.
    lib.db().execute("UPDATE files SET duration_ms = 2000 WHERE name = 'IMG_0001.MOV'", []).unwrap();
    let ids = [
        "2020-07 Urlaub Griechenland/IMG_0001.JPG",
        "2020-07 Urlaub Griechenland/IMG_0002.JPG",
        "Familie/Weihnachten/DSC_2001.jpg",
    ]
    .map(|p| id_of(&lib, p))
    .to_vec();
    (lib, ids)
}

fn set_times(lib: &Library, files: &[(&str, i128)]) {
    for (rel, day) in files {
        set_mtime(&lib.path(rel), BASE_NS + day * DAY_NS);
    }
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shoebox-export-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn body(parent: &Path, name: &str, ids: &[i64], options: Value) -> Value {
    json!({ "ids": ids, "parent": parent.display().to_string(), "name": name, "options": options })
}

fn all_options() -> Value {
    json!({ "number": true, "raw": true, "live": true, "heic_to_jpeg": false })
}

/// Start an export and wait for it to end; the final status.
fn export(addr: SocketAddr, req: &Value) -> Value {
    let r = post(addr, "/api/export/start", req);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let s = get(addr, "/api/export/status").json();
        if s["state"] != "running" {
            return s;
        }
        assert!(Instant::now() < end, "the export does not end");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn copies_are_identical_numbered_in_capture_order_and_originals_stay() {
    let (lib, ids) = library("export-copy");
    let before = lib.snapshot();
    let server = start(&lib, None);
    let out = out_dir("copy");

    // The dialog's numbers: three photos, one RAW, one Live Photo video.
    let summary = post(server.addr, "/api/export/summary", &json!({ "ids": ids })).json();
    assert_eq!((summary["photos"].as_u64(), summary["videos"].as_u64()), (Some(3), Some(0)));
    assert_eq!((summary["raw"].as_u64(), summary["live"].as_u64()), (Some(1), Some(1)));

    let req = body(&out, "print-march", &ids, all_options());
    let check = post(server.addr, "/api/export/check", &req).json();
    assert_eq!(check["exists"], false);
    assert_eq!(check["counts"]["new"], 5, "{check}");
    assert_eq!(check["shoebox"], Value::Null);
    assert!(!out.join("print-march").exists(), "the check writes nothing");

    let status = export(server.addr, &req);
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(status["copied"], 5);
    assert_eq!(status["failed"], json!([]));

    let dest = out.join("print-march");
    assert_eq!(
        names(&dest),
        ["001_IMG_0001.CR2", "001_IMG_0001.JPG", "001_IMG_0001.MOV", "002_IMG_0002.JPG", "003_DSC_2001.jpg"]
    );
    // Byte for byte, with the original's modified time.
    for (copy, original) in [
        ("001_IMG_0001.JPG", "2020-07 Urlaub Griechenland/IMG_0001.JPG"),
        ("001_IMG_0001.CR2", "2020-07 Urlaub Griechenland/IMG_0001.CR2"),
        ("003_DSC_2001.jpg", "Familie/Weihnachten/DSC_2001.jpg"),
    ] {
        assert_eq!(fs::read(dest.join(copy)).unwrap(), fs::read(lib.path(original)).unwrap(), "{copy}");
        assert_eq!(fingerprint::stamp(&dest.join(copy)).unwrap().mtime_ns, fingerprint::stamp(&lib.path(original)).unwrap().mtime_ns);
    }
    assert!(names(&dest).iter().all(|n| !n.ends_with(".part")));
    // The guard: nothing in the library changed.
    assert_eq!(lib.snapshot(), before);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn raw_and_live_come_only_when_asked_and_numbers_can_be_left_off() {
    let (lib, ids) = library("export-options");
    let server = start(&lib, None);
    let out = out_dir("options");

    let status = export(server.addr, &body(&out, "plain", &ids, json!({ "number": false })));
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(names(&out.join("plain")), ["DSC_2001.jpg", "IMG_0001.JPG", "IMG_0002.JPG"]);

    // Only the RAW file.
    let status = export(server.addr, &body(&out, "raw", &ids, json!({ "number": true, "raw": true })));
    assert_eq!(status["state"], "done");
    assert_eq!(names(&out.join("raw")), ["001_IMG_0001.CR2", "001_IMG_0001.JPG", "002_IMG_0002.JPG", "003_DSC_2001.jpg"]);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn exporting_into_a_folder_that_exists_adds_renumbers_and_leaves_other_files() {
    let (lib, ids) = library("export-merge");
    let server = start(&lib, None);
    let out = out_dir("merge");
    let options = json!({ "number": true, "raw": false, "live": false });
    assert_eq!(export(server.addr, &body(&out, "album", &ids, options.clone()))["state"], "done");
    let dest = out.join("album");
    assert_eq!(names(&dest), ["001_IMG_0001.JPG", "002_IMG_0002.JPG", "003_DSC_2001.jpg"]);
    // Something that is not ours, even one that looks a bit like ours.
    fs::write(dest.join("notes.txt"), b"mine").unwrap();
    fs::write(dest.join("004_other.jpg"), b"mine too").unwrap();

    // A new photo taken before all the others.
    lib.jpeg("Familie/IMG_0000.JPG", 9);
    set_times(&lib, &[("Familie/IMG_0000.JPG", 0)]);
    lib.scan_opts(true, false, false);
    let mut all = ids.clone();
    all.push(id_of(&lib, "Familie/IMG_0000.JPG"));

    let req = body(&out, "album", &all, options);
    let check = post(server.addr, "/api/export/check", &req).json();
    assert_eq!(check["exists"], true);
    assert_eq!(check["counts"]["new"], 1, "{check}");
    assert_eq!(check["counts"]["already"], 3);
    assert_eq!(check["counts"]["renamed"], 3);

    let status = export(server.addr, &req);
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!((status["copied"].as_u64(), status["already"].as_u64(), status["renamed"].as_u64()), (Some(1), Some(3), Some(3)));
    assert_eq!(
        names(&dest),
        ["001_IMG_0000.JPG", "002_IMG_0001.JPG", "003_IMG_0002.JPG", "004_DSC_2001.jpg", "004_other.jpg", "notes.txt"]
    );
    // The files moved with their content.
    assert_eq!(fs::read(dest.join("004_DSC_2001.jpg")).unwrap(), fs::read(lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap());
    assert_eq!(fs::read(dest.join("004_other.jpg")).unwrap(), b"mine too");
    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"mine");

    // Again: nothing to do.
    let status = export(server.addr, &req);
    assert_eq!((status["copied"].as_u64(), status["already"].as_u64(), status["renamed"].as_u64()), (Some(0), Some(4), Some(0)));
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn same_names_get_a_suffix_only_without_numbers_and_only_when_the_size_differs() {
    let (lib, mut ids) = library("export-clash");
    // Another IMG_0001.JPG from another folder: a different photo.
    lib.jpeg("Familie/IMG_0001.JPG", 77);
    set_times(&lib, &[("Familie/IMG_0001.JPG", 4)]);
    lib.scan_opts(true, false, false);
    ids.push(id_of(&lib, "Familie/IMG_0001.JPG"));
    let server = start(&lib, None);
    let out = out_dir("clash");

    let off = json!({ "number": false });
    assert_eq!(export(server.addr, &body(&out, "x", &ids, off.clone()))["state"], "done");
    assert_eq!(names(&out.join("x")), ["DSC_2001.jpg", "IMG_0001.JPG", "IMG_0001_2.JPG", "IMG_0002.JPG"]);
    // The same again: all four are there already, the second one by its suffix.
    let status = export(server.addr, &body(&out, "x", &ids, off));
    assert_eq!(status["state"], "done");
    assert_eq!(status["copied"], 0, "{status}");

    // With numbers every name is its own.
    assert_eq!(export(server.addr, &body(&out, "y", &ids, json!({ "number": true })))["state"], "done");
    assert_eq!(names(&out.join("y")), ["001_IMG_0001.JPG", "002_IMG_0002.JPG", "003_DSC_2001.jpg", "004_IMG_0001.JPG"]);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn a_file_that_changed_since_the_scan_is_not_copied() {
    let (lib, ids) = library("export-guard");
    let server = start(&lib, None);
    let out = out_dir("guard");
    // Same size, new content and a new modified time: the index is out of date.
    let path = lib.path("Familie/Weihnachten/DSC_2001.jpg");
    let mut data = fs::read(&path).unwrap();
    let last = data.len() - 3;
    data[last] ^= 0xFF;
    fs::write(&path, data).unwrap();
    set_mtime(&path, BASE_NS + 99 * DAY_NS);

    let status = export(server.addr, &body(&out, "g", &ids, json!({ "number": true })));
    assert_eq!(status["state"], "done");
    assert_eq!(status["copied"], 2, "{status}");
    assert_eq!(status["failed"].as_array().unwrap().len(), 1);
    assert!(status["failed"][0].as_str().unwrap().contains("DSC_2001.jpg"), "{status}");
    assert!(names(&out.join("g")).iter().all(|n| !n.contains("DSC_2001") && !n.ends_with(".part")));
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn a_drive_with_a_library_is_reported_and_folder_names_are_checked() {
    let (lib, ids) = library("export-shoebox");
    let server = start(&lib, None);

    // Inside the drive the photos are on.
    let inside = post(server.addr, "/api/export/check", &body(&lib.path("Familie"), "copies", &ids, json!({}))).json();
    assert_eq!(inside["shoebox"]["same_drive"], true, "{inside}");
    // Somewhere else: nothing to say.
    let out = out_dir("shoebox");
    let outside = post(server.addr, "/api/export/check", &body(&out, "copies", &ids, json!({}))).json();
    assert_eq!(outside["shoebox"], Value::Null);
    // Another drive with a library of its own.
    fs::create_dir_all(out.join("other/.shoebox")).unwrap();
    let other = post(server.addr, "/api/export/check", &body(&out.join("other"), "copies", &ids, json!({}))).json();
    assert_eq!(other["shoebox"]["same_drive"], false, "{other}");

    for bad in ["", "a/b", "a:b", "..", "end."] {
        let r = post(server.addr, "/api/export/start", &body(&out, bad, &ids, json!({})));
        assert_eq!(r.status, 400, "{bad:?}");
    }
    let r = post(server.addr, "/api/export/start", &body(&out.join("missing"), "x", &ids, json!({})));
    assert_eq!(r.status, 400);
    let r = post(server.addr, "/api/export/start", &body(&out, "x", &[], json!({})));
    assert_eq!(r.status, 400, "nothing to export");
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn the_picker_lists_folders_and_only_this_computer_may_export() {
    let (lib, ids) = library("export-local");
    let server = start(&lib, None);
    let out = out_dir("local");
    fs::create_dir_all(out.join("Zebra")).unwrap();
    fs::create_dir_all(out.join("apfel")).unwrap();
    fs::create_dir_all(out.join(".hidden")).unwrap();
    fs::write(out.join("file.txt"), b"x").unwrap();

    let list = get(server.addr, &format!("/api/export/folders?path={}", encode(&out.display().to_string()))).json();
    let folders: Vec<&str> = list["folders"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(folders, ["apfel", "Zebra"], "no files, no hidden folders, sorted without regard to case");
    assert_eq!(list["crumbs"].as_array().unwrap().last().unwrap()["path"], list["path"]);
    assert_eq!(get(server.addr, "/api/export/folders?path=relative/path").status, 400);
    assert_eq!(get(server.addr, "/api/export/places").status, 200);

    assert_eq!(get(server.addr, "/api/info").json()["export"], true);
    server.stop().unwrap();

    // A device on the network, logged in with the PIN, gets no export: it
    // would save into this computer's folders.
    let server = start(&lib, Some("4711"));
    let addr = server.addr;
    let evil = ("Host", "photos.evil.example");
    let json = ("Content-Type", "application/json");
    let login = request(addr, "POST", "/api/login", &[evil, json], br#"{"pin":"4711"}"#);
    let cookie = login.header("set-cookie").unwrap().split(';').next().unwrap().to_string();
    let lan = [evil, ("Cookie", cookie.as_str()), json];
    assert_eq!(request(addr, "GET", "/api/info", &lan, b"").json()["export"], false);
    for path in ["/api/export/places", "/api/export/status", "/api/export/folders?path=%2F"] {
        assert_eq!(request(addr, "GET", path, &lan, b"").status, 403, "{path}");
    }
    let req = body(&out, "nope", &ids, json!({}));
    for path in ["/api/export/summary", "/api/export/check", "/api/export/start", "/api/export/stop", "/api/export/reveal"] {
        assert_eq!(request(addr, "POST", path, &lan, req.to_string().as_bytes()).status, 403, "{path}");
    }
    assert!(!out.join("nope").exists());
    let _ = fs::remove_dir_all(&out);
}

/// Needs the HEIC and the Live Photo video of `scripts/make-fixtures.sh`.
#[test]
fn a_heic_becomes_a_jpeg_with_its_exif_and_stays_there_on_the_second_export() {
    if std::env::var_os("SHOEBOX_FIXTURES").is_none_or(|f| f.is_empty()) {
        eprintln!("SHOEBOX_FIXTURES is not set: skipping the HEIC export test");
        return;
    }
    let lib = Library::new("export-heic");
    lib.scan_opts(true, false, false);
    let before = lib.snapshot();
    let heic: (i64, String) = lib
        .db()
        .query_row("SELECT id, path FROM files WHERE kind = 'heic' AND missing_since IS NULL", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let server = start(&lib, None);
    let out = out_dir("heic");
    let ids = [heic.0];

    let summary = post(server.addr, "/api/export/summary", &json!({ "ids": ids })).json();
    assert_eq!((summary["heic"].as_u64(), summary["live"].as_u64()), (Some(1), Some(1)), "{summary}");

    let options = json!({ "number": true, "live": true, "heic_to_jpeg": true });
    let req = body(&out, "jpegs", &ids, options.clone());
    let status = export(server.addr, &req);
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!((status["converted"].as_u64(), status["no_exif"].as_u64()), (Some(1), Some(0)), "{status}");
    let dest = out.join("jpegs");
    assert_eq!(names(&dest), ["001_IMG_0002.MOV", "001_IMG_0002.jpg"]);

    let jpeg = fs::read(dest.join("001_IMG_0002.jpg")).unwrap();
    let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
    assert_eq!((img.width(), img.height()), (4032, 3024));
    // The capture date and the camera made it across, and the picture is upright.
    let has = |needle: &[u8]| jpeg.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"2020:07:15 09:10:11"), "capture date");
    assert!(has(b"iPhone 12"), "camera model");
    // (No Orientation tag at all also means upright.)
    if let Ok(slot) = shoebox::orientation::find(&jpeg) {
        assert_eq!(slot.value, 1);
    }
    // The video came as it is.
    let mov = lib.root.join(Path::new(&heic.1).with_extension("MOV"));
    assert_eq!(fs::read(dest.join("001_IMG_0002.MOV")).unwrap(), fs::read(mov).unwrap());

    // Again: the .jpg counts as the HEIC, nothing is written.
    let status = export(server.addr, &req);
    assert_eq!((status["copied"].as_u64(), status["already"].as_u64()), (Some(0), Some(2)), "{status}");
    assert_eq!(lib.snapshot(), before);
    let _ = fs::remove_dir_all(&out);
}
