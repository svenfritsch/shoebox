//! Live Photos are a still and a short video with the same name in one
//! folder, taken at the same moment. A camera's `IMG_0139.JPG` next to a
//! phone's `IMG_0139.MOV` from another day is no Live Photo: the timeline
//! shows both, the export leaves the video out, and moving or trashing one
//! leaves the other where it is.

mod common;

use std::fs;

use common::*;
use shoebox::{browse, export, organize};

const CANON: &str = "Familie/Aurelia/IMG_0139.JPG";
const PHONE: &str = "Familie/Aurelia/IMG_0139.MOV";
const STILL: &str = "2020-07 Urlaub Griechenland/IMG_0002.JPG";
const CLIP: &str = "2020-07 Urlaub Griechenland/IMG_0002.MOV";

/// The stand-ins carry no metadata; the index gets what real files would say.
fn library(name: &str) -> Library {
    let lib = Library::new(name);
    lib.jpeg(CANON, 7);
    lib.write("Familie/Aurelia/IMG_0139.XMP", b"<x:xmpmeta/>");
    lib.write(PHONE, b"not really a video, but named like one");
    lib.write(CLIP, b"not really a video either");
    lib.scan_opts(true, false, false);
    let set = |path: &str, taken: &str, offset: Option<&str>, duration: Option<i64>| {
        lib.db()
            .execute(
                "UPDATE files SET taken = ?2, taken_offset = ?3, duration_ms = ?4 WHERE path_nfc = ?1",
                rusqlite::params![path, taken, offset, duration],
            )
            .unwrap();
    };
    // A Canon writes no time zone; the iPhone video is from another day.
    set(CANON, "2021-03-14T15:27:00", None, None);
    set(PHONE, "2019-08-02T10:11:12", Some("+02:00"), Some(2_500));
    // A real Live Photo: the video's container date is in UTC.
    set(STILL, "2020-07-15T09:10:11", Some("+03:00"), None);
    set(CLIP, "2020-07-15T06:10:12", Some("+00:00"), Some(2_000));
    lib
}

#[test]
fn the_timeline_pairs_only_what_was_taken_together() {
    let lib = library("live-timeline");
    let snapshot = browse::Snapshot::load(&lib.db()).unwrap();
    let item = |path: &str| snapshot.items.iter().find(|it| it.id == id_of(&lib, path));

    assert_eq!(item(STILL).unwrap().live, Some(id_of(&lib, CLIP)));
    assert!(item(CLIP).is_none(), "a Live Photo's video is folded into its still");

    assert_eq!(item(CANON).unwrap().live, None, "another day's video is not this photo's");
    assert!(item(PHONE).is_some(), "the video stays on the timeline on its own");
}

#[test]
fn the_export_takes_only_the_real_live_video() {
    let lib = library("live-export");
    let summary = export::summarize(&lib.db(), &[id_of(&lib, CANON), id_of(&lib, STILL)]).unwrap();
    assert_eq!((summary.photos, summary.live), (2, 1));
}

#[test]
fn moving_and_trashing_keep_strangers_apart() {
    let lib = library("live-organize");
    let conn = lib.db();

    // Trashing the phone's video leaves the Canon photo and its sidecar.
    let trashed = organize::trash_files(&conn, &lib.root, &[id_of(&lib, PHONE)]).unwrap();
    assert!(trashed.skipped.is_empty(), "{:?}", trashed.skipped);
    assert_eq!(trashed.files, vec![PHONE.to_string()]);
    assert_eq!(trashed.sidecars, 0);
    assert!(lib.path(CANON).exists() && lib.path("Familie/Aurelia/IMG_0139.XMP").exists());

    // A Live Photo moves with its video.
    let moved = organize::move_files(&conn, &lib.root, &[id_of(&lib, STILL)], "Urlaub").unwrap();
    assert!(moved.skipped.is_empty(), "{:?}", moved.skipped);
    let mut files = moved.files.clone();
    files.sort();
    assert_eq!(files, vec!["Urlaub/IMG_0002.JPG".to_string(), "Urlaub/IMG_0002.MOV".to_string()]);

    // The Canon photo moves with its sidecar.
    let moved = organize::move_files(&conn, &lib.root, &[id_of(&lib, CANON)], "Aurelia").unwrap();
    assert_eq!((moved.files.len(), moved.sidecars), (1, 1));
    assert!(fs::read_dir(lib.path("Aurelia")).unwrap().count() == 2);
}

#[test]
fn a_video_with_a_photo_of_its_own_moment_still_takes_it_along() {
    let lib = library("live-from-video");
    let conn = lib.db();
    // Moving the clip itself (by id) takes its still along, as before.
    let moved = organize::move_files(&conn, &lib.root, &[id_of(&lib, CLIP)], "Urlaub").unwrap();
    assert_eq!(moved.files.len(), 2, "{:?}", moved.files);
    assert!(lib.path("Urlaub/IMG_0002.JPG").exists());
}
