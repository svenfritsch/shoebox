//! Carry over data between drives (phase 15a): photos are matched by content,
//! what the target has wins, everything is added in one transaction, and
//! nothing changes on the source drive.

mod common;

use std::fs;

use common::*;
use serde_json::json;
use shoebox::transfer::{self, Boxes, Counts};
use shoebox::tags;

/// A library with only what the test writes itself.
fn bare(test: &str) -> Library {
    let lib = Library::new(test);
    for entry in fs::read_dir(&lib.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fs::remove_dir_all(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
    }
    lib
}

fn open(lib: &Library) -> rusqlite::Connection {
    shoebox::db::open_shared(&lib.path(".shoebox/library.db")).unwrap()
}

fn db_path(lib: &Library) -> std::path::PathBuf {
    lib.path(".shoebox/library.db")
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

const FJORD: &str = "Norwegen/fjord.jpg";
const PAPA: &str = "Norwegen/papa.jpg";
const DROHNE: &str = "Norwegen/drohne.jpg";

/// The travel drive: three photos with data, one only here.
fn source(test: &str) -> Library {
    let a = bare(test);
    a.jpeg(FJORD, 20);
    a.jpeg(PAPA, 21);
    a.jpeg(DROHNE, 22);
    a.jpeg("Solo/nur-a.jpg", 23);
    a.scan_opts(true, false, false);
    a
}

/// The family drive: the same photos under other names and folders (one of
/// them twice), and one only here.
fn target(test: &str) -> Library {
    let b = bare(test);
    b.jpeg("Familie/2026/IMG_1.jpg", 20);
    b.jpeg("Familie/2026/IMG_2.jpg", 21);
    b.jpeg("Familie/2026/IMG_2 Kopie.jpg", 21);
    b.jpeg("Andere/nur-b.jpg", 30);
    b.scan_opts(true, false, false);
    b
}

/// Tags, a heart, people, dates and a place made on the source drive.
fn make_data(a: &Library) {
    let conn = open(a);
    let (fjord, papa, drohne) = (id_of(a, FJORD), id_of(a, PAPA), id_of(a, DROHNE));
    tags::add(&conn, &[fjord], "Fjord").unwrap();
    tags::add(&conn, &[papa], "Papa").unwrap();
    tags::add(&conn, &[drohne], "Drohne").unwrap();
    tags::set_favorite(&conn, &[fjord], true).unwrap();
    conn.execute("INSERT INTO people (name) VALUES ('Anna')", []).unwrap();
    let key = |p: &str| a.record_key(p);
    conn.execute(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
         VALUES (?1, 0.1, 0.1, 0.2, 0.2, (SELECT id FROM people WHERE name = 'Anna'), 'confirmed', 1, 5)",
        [key(FJORD)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at) VALUES (?1, 0.5, 0.5, 0.1, 0.1, NULL, 'ignored', 0, 6)",
        [key(PAPA)],
    )
    .unwrap();
    // A decision about a photo that is not on the other drive.
    conn.execute(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
         VALUES (?1, 0.1, 0.1, 0.2, 0.2, (SELECT id FROM people WHERE name = 'Anna'), 'confirmed', 1, 7)",
        [key("Solo/nur-a.jpg")],
    )
    .unwrap();
    conn.execute("INSERT INTO taken_overrides (key, taken, taken_offset, at) VALUES (?1, '1999-12-31T23:59:00', NULL, 1)", [key(FJORD)]).unwrap();
    conn.execute("INSERT INTO date_estimates (key, year, month, day, at) VALUES (?1, 1987, 6, NULL, 2)", [key(PAPA)]).unwrap();
    conn.execute("INSERT INTO geo_overrides (key, lat, lon, at) VALUES (?1, 69.6, 18.9, 3)", [key(FJORD)]).unwrap();
}

fn own_tag_rows(lib: &Library, fold: &str, path: Option<&str>) -> i64 {
    let filter = path.map(|p| format!(" AND f.path_nfc = '{p}'")).unwrap_or_default();
    lib.count(&format!(
        "SELECT count(*) FROM file_tags ft JOIN tags t ON t.id = ft.tag_id JOIN files f ON f.id = ft.file_id
         WHERE ft.source = 'user' AND t.fold = '{fold}'{filter}"
    ))
}

#[test]
fn data_is_carried_by_content_to_the_same_photos_under_other_names() {
    let a = source("carry-basic-a");
    let b = target("carry-basic-b");
    make_data(&a);
    let before_db = fs::read(db_path(&a)).unwrap();
    let before_files = a.snapshot();

    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let conn = open(&b);
    // The plan says what will happen, without doing it.
    let expected = Counts { tags: 3, hearts: 1, people: 2, new_people: 1, dates: 2, places: 1, photos: 3 };
    assert_eq!(transfer::plan(&conn, &set, Boxes::all()).unwrap(), expected);
    assert_eq!(b.count("SELECT count(*) FROM people"), 0);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions"), 0);

    assert_eq!(transfer::apply(&conn, &set, Boxes::all()).unwrap(), expected);

    // Tags: "Papa" on both identical copies; the photo only on the source gets nothing.
    assert_eq!(own_tag_rows(&b, "fjord", Some("Familie/2026/IMG_1.jpg")), 1);
    assert_eq!(own_tag_rows(&b, "papa", None), 2);
    assert_eq!(b.count("SELECT count(*) FROM tags WHERE fold = 'drohne'"), 0, "a photo that is not here brings no tag");
    // Folder tags come from where a file lies: never carried.
    assert_eq!(own_tag_rows(&b, "norwegen", None), 0);
    // The heart is separate from the tags.
    assert_eq!(own_tag_rows(&b, "favorite", Some("Familie/2026/IMG_1.jpg")), 1);
    // People: Anna is created without group, the decisions are copied by content and box.
    assert_eq!(b.count("SELECT count(*) FROM people WHERE name = 'Anna' AND group_id IS NULL AND cover_key IS NULL AND hidden = 0"), 1);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions"), 2);
    assert_eq!(
        b.count(&format!(
            "SELECT count(*) FROM face_decisions WHERE key = '{}' AND decision = 'confirmed' AND manual = 1 AND person_id = (SELECT id FROM people WHERE name = 'Anna')",
            b.record_key("Familie/2026/IMG_1.jpg")
        )),
        1
    );
    assert_eq!(b.count("SELECT count(*) FROM face_decisions WHERE decision = 'ignored' AND person_id IS NULL"), 1);
    // Dates and places by content.
    assert_eq!(b.count(&format!("SELECT count(*) FROM taken_overrides WHERE key = '{}' AND taken = '1999-12-31T23:59:00'", b.record_key("Familie/2026/IMG_1.jpg"))), 1);
    assert_eq!(b.count(&format!("SELECT count(*) FROM date_estimates WHERE key = '{}' AND year = 1987 AND month = 6", b.record_key("Familie/2026/IMG_2.jpg"))), 1);
    assert_eq!(b.count("SELECT count(*) FROM geo_overrides WHERE lat = 69.6 AND lon = 18.9"), 1);

    // A second run reports nothing and writes nothing.
    let rows = |lib: &Library| {
        lib.count("SELECT (SELECT count(*) FROM file_tags) + (SELECT count(*) FROM face_decisions) + (SELECT count(*) FROM taken_overrides) + (SELECT count(*) FROM date_estimates) + (SELECT count(*) FROM geo_overrides) + (SELECT count(*) FROM people) + (SELECT count(*) FROM tags)")
    };
    let rows_before = rows(&b);
    let again = transfer::apply(&conn, &set, Boxes::all()).unwrap();
    assert_eq!(again, Counts::default());
    assert!(again.is_nothing());
    assert_eq!(rows(&b), rows_before);

    // Only reading: neither the source's index nor a photo changed.
    assert_eq!(fs::read(db_path(&a)).unwrap(), before_db, "the source's library.db is byte-identical");
    assert_eq!(a.snapshot(), before_files);
}

#[test]
fn a_box_that_is_not_ticked_changes_nothing() {
    let a = source("carry-boxes-a");
    let b = target("carry-boxes-b");
    make_data(&a);
    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let conn = open(&b);

    let only_hearts = Boxes { tags: false, hearts: true, people: false, dates: false, places: false };
    let done = transfer::apply(&conn, &set, only_hearts).unwrap();
    assert_eq!(done, Counts { hearts: 1, photos: 1, ..Counts::default() });
    assert_eq!(own_tag_rows(&b, "favorite", None), 1);
    assert_eq!(own_tag_rows(&b, "fjord", None) + own_tag_rows(&b, "papa", None), 0);
    assert_eq!(b.count("SELECT count(*) FROM people"), 0);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions"), 0);
    assert_eq!(b.count("SELECT count(*) FROM taken_overrides") + b.count("SELECT count(*) FROM date_estimates") + b.count("SELECT count(*) FROM geo_overrides"), 0);

    // The rest follows when ticked; the heart is not counted twice.
    let rest = Boxes { hearts: false, ..Boxes::all() };
    let done = transfer::apply(&conn, &set, rest).unwrap();
    assert_eq!((done.tags, done.hearts, done.people, done.dates, done.places), (3, 0, 2, 2, 1));
}

#[test]
fn what_the_target_already_has_wins() {
    let a = source("carry-wins-a");
    let b = target("carry-wins-b");
    make_data(&a);
    {
        let conn = open(&b);
        let img1 = id_of(&b, "Familie/2026/IMG_1.jpg");
        // The same tag in another spelling: not added twice.
        tags::add(&conn, &[img1], "FJORD").unwrap();
        // A date of its own for the same content.
        conn.execute("INSERT INTO taken_overrides (key, taken, taken_offset, at) VALUES (?1, '2001-01-01T00:00:00', NULL, 9)", [b.record_key("Familie/2026/IMG_1.jpg")]).unwrap();
        // A decision of its own whose box overlaps the one on the source: it stays, and Anna is not even created for it.
        conn.execute("INSERT INTO people (name) VALUES ('Bea')", []).unwrap();
        conn.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
             VALUES (?1, 0.12, 0.1, 0.2, 0.2, (SELECT id FROM people WHERE name = 'Bea'), 'confirmed', 1, 9)",
            [b.record_key("Familie/2026/IMG_1.jpg")],
        )
        .unwrap();
    }
    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let conn = open(&b);
    let done = transfer::apply(&conn, &set, Boxes::all()).unwrap();
    // Tags: only "Papa" on the two copies; dates: only the estimate; people: only the ignored face.
    assert_eq!((done.tags, done.hearts, done.people, done.new_people, done.dates, done.places), (2, 1, 1, 0, 1, 1));
    assert_eq!(own_tag_rows(&b, "fjord", None), 1);
    assert_eq!(b.count("SELECT count(*) FROM taken_overrides WHERE taken = '2001-01-01T00:00:00'"), 1);
    assert_eq!(b.count("SELECT count(*) FROM taken_overrides"), 1, "the source's corrected date was left out");
    assert_eq!(b.count("SELECT count(*) FROM people WHERE name = 'Anna'"), 0);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions WHERE person_id = (SELECT id FROM people WHERE name = 'Bea')"), 1);
}

#[test]
fn people_are_found_by_name_and_a_person_of_another_spelling_is_the_same() {
    let a = source("carry-names-a");
    let b = target("carry-names-b");
    make_data(&a);
    // The target knows Anna as "ANNA", with a group and a picture of its own.
    {
        let conn = open(&b);
        conn.execute("INSERT INTO groups (name, position) VALUES ('Familie', 1)", []).unwrap();
        conn.execute("INSERT INTO people (name, group_id) VALUES ('ANNA', (SELECT id FROM groups WHERE name = 'Familie'))", []).unwrap();
    }
    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let done = transfer::apply(&open(&b), &set, Boxes::all()).unwrap();
    assert_eq!(done.new_people, 0);
    assert_eq!(b.count("SELECT count(*) FROM people"), 1);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions WHERE person_id = (SELECT id FROM people WHERE name = 'ANNA')"), 1);
    assert_eq!(b.count("SELECT count(*) FROM people WHERE group_id IS NOT NULL"), 1, "the target's group stays");
}

#[test]
fn photos_are_matched_by_content_not_by_name_missing_files_count_on_the_source_only() {
    let a = bare("carry-match-a");
    a.jpeg("Reise/gleich.jpg", 40);
    a.jpeg("Reise/geloescht.jpg", 41);
    a.jpeg("Reise/bearbeitet.jpg", 42);
    a.jpeg("Reise/weg-auf-b.jpg", 43);
    a.scan_opts(true, false, false);
    let b = bare("carry-match-b");
    b.jpeg("Neu/anderer-name.jpg", 40);
    b.jpeg("Neu/auch-da.jpg", 41);
    // The same name as on the source, but edited: another content.
    b.jpeg("Reise/bearbeitet.jpg", 44);
    b.jpeg("Neu/war-da.jpg", 43);
    b.scan_opts(true, false, false);
    {
        let conn = open(&a);
        for (path, tag) in [("Reise/gleich.jpg", "Gleich"), ("Reise/geloescht.jpg", "Geloescht"), ("Reise/bearbeitet.jpg", "Bearbeitet"), ("Reise/weg-auf-b.jpg", "Weg")] {
            tags::add(&conn, &[id_of(&a, path)], tag).unwrap();
        }
    }
    // The source's file is gone from its drive: its record is still used.
    fs::remove_file(a.path("Reise/geloescht.jpg")).unwrap();
    a.scan_opts(true, false, false);
    assert!(a.record("Reise/geloescht.jpg").unwrap().2, "the source records it as missing");
    // The target's file is gone: nothing is given to a file that is not there.
    fs::remove_file(b.path("Neu/war-da.jpg")).unwrap();
    b.scan_opts(true, false, false);

    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let done = transfer::apply(&open(&b), &set, Boxes::all()).unwrap();
    assert_eq!(done.tags, 2);
    assert_eq!(own_tag_rows(&b, "gleich", Some("Neu/anderer-name.jpg")), 1);
    assert_eq!(own_tag_rows(&b, "geloescht", Some("Neu/auch-da.jpg")), 1);
    assert_eq!(own_tag_rows(&b, "bearbeitet", None), 0, "edited content is another photo");
    assert_eq!(own_tag_rows(&b, "weg", None), 0, "a missing file on the target receives nothing");
}

#[test]
fn a_photo_without_a_full_hash_is_matched_by_size_and_quick_hash() {
    let a = source("carry-quick-a");
    make_data(&a);
    let b = bare("carry-quick-b");
    b.jpeg("Familie/IMG_1.jpg", 20);
    b.scan_opts(false, false, false);
    assert!(b.record("Familie/IMG_1.jpg").unwrap().1.is_none(), "no full hash yet");
    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let done = transfer::apply(&open(&b), &set, Boxes::all()).unwrap();
    assert_eq!(done.tags, 1);
    assert_eq!(own_tag_rows(&b, "fjord", Some("Familie/IMG_1.jpg")), 1);
}

#[test]
fn a_failure_half_way_leaves_the_target_unchanged() {
    let a = source("carry-atomic-a");
    let b = target("carry-atomic-b");
    make_data(&a);
    let set = transfer::read_drive(&db_path(&a)).unwrap();
    let tables = ["file_tags", "tags", "people", "face_decisions", "taken_overrides", "date_estimates", "geo_overrides"];
    let counts = |lib: &Library| tables.iter().map(|t| lib.count(&format!("SELECT count(*) FROM {t}"))).collect::<Vec<_>>();
    let before = counts(&b);
    let conn = open(&b);
    let result = transfer::apply_with(&conn, &set, Boxes::all(), &mut |written| {
        if written >= 4 {
            anyhow::bail!("the drive was unplugged");
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(counts(&b), before, "all or nothing");
    // Run again: it works, because nothing was left half done.
    assert_eq!(transfer::apply(&conn, &set, Boxes::all()).unwrap().photos, 3);
}

/// A new table in `library.db` must be decided about: carried, or listed as
/// not carried. This fails until it is in `transfer::CARRIED_TABLES` or
/// `transfer::NOT_CARRIED_TABLES`.
#[test]
fn every_table_of_the_index_is_carried_or_listed_as_not_carried() {
    let lib = bare("carry-schema");
    lib.scan_opts(true, false, false);
    let conn = lib.db();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(names.len() > 10, "{names:?}");
    for name in &names {
        let carried = transfer::CARRIED_TABLES.contains(&name.as_str());
        let listed = transfer::NOT_CARRIED_TABLES.contains(&name.as_str());
        assert!(carried || listed, "table {name} is neither carried nor listed as not carried (core/src/transfer.rs)");
        assert!(!(carried && listed), "table {name} is in both lists");
    }
    for name in transfer::CARRIED_TABLES.iter().chain(transfer::NOT_CARRIED_TABLES) {
        assert!(names.iter().any(|n| n == name), "{name} is listed but there is no such table");
    }
}

#[test]
fn the_routes_give_hints_plans_and_carry_over_and_refuse_backups() {
    let a = source("carry-api-a");
    let b = target("carry-api-b");
    make_data(&a);
    let before = (a.snapshot(), b.snapshot());
    let gone = std::env::temp_dir().join(format!("shoebox-it-{}-carry-api-unplugged", std::process::id()));
    let server = start_many(&[&a, &b], &[gone.as_path()]);
    let addr = server.addr;
    let (ia, ib) = (lib_id(addr, &a), lib_id(addr, &b));

    // Hints: one pair, from the travel drive to the family drive, nothing back.
    let hints = get(addr, "/raw/api/all/carry").json();
    let hints = hints.as_array().unwrap();
    assert_eq!(hints.len(), 1, "{hints:?}");
    assert_eq!((hints[0]["from"].as_str().unwrap(), hints[0]["to"].as_str().unwrap()), (ia.as_str(), ib.as_str()));
    assert_eq!(hints[0]["counts"]["photos"], 3);
    assert_eq!(hints[0]["counts"]["tags"], 3);

    // The plan for the dialog; nothing is written yet.
    let plan = post(addr, "/raw/api/all/carry/plan", &json!({ "from": ia, "to": ib }));
    assert_eq!(plan.status, 200, "{}", String::from_utf8_lossy(&plan.body));
    let plan = plan.json();
    assert_eq!(plan["counts"]["hearts"], 1);
    assert_eq!(plan["counts"]["people"], 2);
    assert_eq!(plan["source_emptied"], false);
    assert_eq!(own_tag_rows(&b, "fjord", None), 0);

    // Changes need the X-Shoebox header like every other change.
    let body = format!(r#"{{"from":"{ia}","to":"{ib}"}}"#);
    let json_ct = ("Content-Type", "application/json");
    assert_eq!(bare_request(addr, "POST", "/raw/api/all/carry", &[json_ct], body.as_bytes()).status, 403);

    // Carry over (all boxes when none are named).
    let done = post(addr, "/raw/api/all/carry", &json!({ "from": ia, "to": ib }));
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    assert_eq!(done.json()["counts"]["photos"], 3);
    assert_eq!(own_tag_rows(&b, "fjord", None), 1);
    assert_eq!(own_tag_rows(&b, "papa", None), 2);
    // Once done there is nothing left to do, and no hint.
    assert_eq!(post(addr, "/raw/api/all/carry", &json!({ "from": ia, "to": ib })).json()["counts"]["photos"], 0);
    assert_eq!(get(addr, "/raw/api/all/carry").json().as_array().unwrap().len(), 0);
    // The new person shows in All drives.
    let people = get(addr, "/raw/api/all/people").json();
    assert!(people["people"].as_array().unwrap().iter().any(|p| p["name"] == "Anna"));

    // The same drive twice, an unknown drive and a drive that is not plugged in.
    assert_eq!(post(addr, "/raw/api/all/carry", &json!({ "from": ia, "to": ia })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/carry", &json!({ "from": ia, "to": "00000000" })).status, 404);
    let drives = get(addr, "/raw/api/all/drives").json();
    let off = drives.as_array().unwrap().iter().find(|d| d["online"] == false).unwrap()["library"].as_str().unwrap().to_string();
    assert_eq!(post(addr, "/raw/api/all/carry", &json!({ "from": off, "to": ib })).status, 503);
    assert_eq!(post(addr, "/raw/api/all/carry/plan", &json!({ "from": ia, "to": off })).status, 503);
    assert!(!gone.exists(), "nothing was created where the missing drive should be");

    // A backup is refused, as source and as target.
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": ib, "role": "backup" })).status, 200);
    let refused = post(addr, "/raw/api/all/carry", &json!({ "from": ia, "to": ib }));
    assert_eq!(refused.status, 400);
    assert!(refused.json()["error"].as_str().unwrap().contains("backup"));
    assert_eq!(post(addr, "/raw/api/all/carry", &json!({ "from": ib, "to": ia })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/carry/plan", &json!({ "from": ia, "to": ib })).status, 400);
    server.stop().unwrap();

    // Photos did not change on either drive.
    assert_eq!((a.snapshot(), b.snapshot()), before);
}

#[test]
fn a_drive_that_may_be_a_backup_is_refused_until_the_user_decides() {
    // Two drives with exactly the same content: one of them is a backup, and
    // only the user can tell which.
    let one = bare("carry-undecided-a");
    let two = bare("carry-undecided-b");
    for lib in [&one, &two] {
        lib.jpeg("Bilder/eins.jpg", 60);
        lib.jpeg("Bilder/zwei.jpg", 61);
        lib.scan_opts(true, false, false);
    }
    tags::add(&open(&one), &[id_of(&one, "Bilder/eins.jpg")], "Urlaub").unwrap();
    let server = start_many(&[&one, &two], &[]);
    let addr = server.addr;
    let (i1, i2) = (lib_id(addr, &one), lib_id(addr, &two));
    assert_eq!(get(addr, "/raw/api/all/carry").json().as_array().unwrap().len(), 0, "no hints for drives that may be backups");
    let refused = post(addr, "/raw/api/all/carry", &json!({ "from": i1, "to": i2 }));
    assert_eq!(refused.status, 400);
    assert!(refused.json()["error"].as_str().unwrap().contains("Decide first"));
    // Said "separate" for both: they take part.
    for id in [&i1, &i2] {
        assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": id, "role": "separate" })).status, 200);
    }
    let done = post(addr, "/raw/api/all/carry", &json!({ "from": i1, "to": i2 }));
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    assert_eq!(done.json()["counts"]["tags"], 1);
    server.stop().unwrap();
}

#[test]
fn a_drive_that_was_emptied_is_recognised_so_the_dialog_can_warn() {
    let a = source("carry-emptied-a");
    assert!(!transfer::looks_emptied(&db_path(&a)).unwrap());
    for p in [FJORD, PAPA, DROHNE] {
        fs::remove_file(a.path(p)).unwrap();
    }
    a.scan_opts(true, false, false);
    assert!(transfer::looks_emptied(&db_path(&a)).unwrap(), "3 of 4 files are gone");
}
