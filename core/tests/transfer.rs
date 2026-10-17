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
    let a = source("transfer-basic-a");
    let b = target("transfer-basic-b");
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
    let a = source("transfer-boxes-a");
    let b = target("transfer-boxes-b");
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
    let a = source("transfer-wins-a");
    let b = target("transfer-wins-b");
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
    let a = source("transfer-names-a");
    let b = target("transfer-names-b");
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
    let a = bare("transfer-match-a");
    a.jpeg("Reise/gleich.jpg", 40);
    a.jpeg("Reise/geloescht.jpg", 41);
    a.jpeg("Reise/bearbeitet.jpg", 42);
    a.jpeg("Reise/weg-auf-b.jpg", 43);
    a.scan_opts(true, false, false);
    let b = bare("transfer-match-b");
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
    let a = source("transfer-quick-a");
    make_data(&a);
    let b = bare("transfer-quick-b");
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
    let a = source("transfer-atomic-a");
    let b = target("transfer-atomic-b");
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
/// not carried. This fails until it is in `transfer::TRANSFERRED_TABLES` or
/// `transfer::NOT_TRANSFERRED_TABLES`.
#[test]
fn every_table_of_the_index_is_carried_or_listed_as_not_carried() {
    let lib = bare("transfer-schema");
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
        let carried = transfer::TRANSFERRED_TABLES.contains(&name.as_str());
        let listed = transfer::NOT_TRANSFERRED_TABLES.contains(&name.as_str());
        assert!(carried || listed, "table {name} is neither carried nor listed as not carried (core/src/transfer.rs)");
        assert!(!(carried && listed), "table {name} is in both lists");
    }
    for name in transfer::TRANSFERRED_TABLES.iter().chain(transfer::NOT_TRANSFERRED_TABLES) {
        assert!(names.iter().any(|n| n == name), "{name} is listed but there is no such table");
    }
}

#[test]
fn the_routes_give_hints_plans_and_transfer_and_refuse_backups() {
    let a = source("transfer-api-a");
    let b = target("transfer-api-b");
    make_data(&a);
    let before = (a.snapshot(), b.snapshot());
    let gone = std::env::temp_dir().join(format!("shoebox-it-{}-transfer-api-unplugged", std::process::id()));
    let server = start_many(&[&a, &b], &[gone.as_path()]);
    let addr = server.addr;
    let (ia, ib) = (lib_id(addr, &a), lib_id(addr, &b));

    // Hints: one pair, from the travel drive to the family drive, nothing back.
    let hints = get(addr, "/raw/api/all/transfer").json();
    let hints = hints.as_array().unwrap();
    assert_eq!(hints.len(), 1, "{hints:?}");
    assert_eq!((hints[0]["from"].as_str().unwrap(), hints[0]["to"].as_str().unwrap()), (ia.as_str(), ib.as_str()));
    assert_eq!(hints[0]["counts"]["photos"], 3);
    assert_eq!(hints[0]["counts"]["tags"], 3);

    // The plan for the dialog; nothing is written yet.
    let plan = post(addr, "/raw/api/all/transfer/plan", &json!({ "from": ia, "to": ib }));
    assert_eq!(plan.status, 200, "{}", String::from_utf8_lossy(&plan.body));
    let plan = plan.json();
    assert_eq!(plan["counts"]["hearts"], 1);
    assert_eq!(plan["counts"]["people"], 2);
    assert_eq!(plan["source_emptied"], false);
    assert_eq!(own_tag_rows(&b, "fjord", None), 0);

    // Changes need the X-Shoebox header like every other change.
    let body = format!(r#"{{"from":"{ia}","to":"{ib}"}}"#);
    let json_ct = ("Content-Type", "application/json");
    assert_eq!(bare_request(addr, "POST", "/raw/api/all/transfer", &[json_ct], body.as_bytes()).status, 403);

    // Carry over (all boxes when none are named).
    let done = post(addr, "/raw/api/all/transfer", &json!({ "from": ia, "to": ib }));
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    assert_eq!(done.json()["counts"]["photos"], 3);
    assert_eq!(own_tag_rows(&b, "fjord", None), 1);
    assert_eq!(own_tag_rows(&b, "papa", None), 2);
    // Once done there is nothing left to do, and no hint.
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "from": ia, "to": ib })).json()["counts"]["photos"], 0);
    assert_eq!(get(addr, "/raw/api/all/transfer").json().as_array().unwrap().len(), 0);
    // The new person shows in All drives.
    let people = get(addr, "/raw/api/all/people").json();
    assert!(people["people"].as_array().unwrap().iter().any(|p| p["name"] == "Anna"));

    // The same drive twice, an unknown drive and a drive that is not plugged in.
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "from": ia, "to": ia })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "from": ia, "to": "00000000" })).status, 404);
    let drives = get(addr, "/raw/api/all/drives").json();
    let off = drives.as_array().unwrap().iter().find(|d| d["online"] == false).unwrap()["library"].as_str().unwrap().to_string();
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "from": off, "to": ib })).status, 503);
    assert_eq!(post(addr, "/raw/api/all/transfer/plan", &json!({ "from": ia, "to": off })).status, 503);
    assert!(!gone.exists(), "nothing was created where the missing drive should be");

    // A backup is refused, as source and as target.
    assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": ib, "role": "backup" })).status, 200);
    let refused = post(addr, "/raw/api/all/transfer", &json!({ "from": ia, "to": ib }));
    assert_eq!(refused.status, 400);
    assert!(refused.json()["error"].as_str().unwrap().contains("backup"));
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "from": ib, "to": ia })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/transfer/plan", &json!({ "from": ia, "to": ib })).status, 400);
    server.stop().unwrap();

    // Photos did not change on either drive.
    assert_eq!((a.snapshot(), b.snapshot()), before);
}

#[test]
fn a_drive_that_may_be_a_backup_is_refused_until_the_user_decides() {
    // Two drives with exactly the same content: one of them is a backup, and
    // only the user can tell which.
    let one = bare("transfer-undecided-a");
    let two = bare("transfer-undecided-b");
    for lib in [&one, &two] {
        lib.jpeg("Bilder/eins.jpg", 60);
        lib.jpeg("Bilder/zwei.jpg", 61);
        lib.scan_opts(true, false, false);
    }
    tags::add(&open(&one), &[id_of(&one, "Bilder/eins.jpg")], "Urlaub").unwrap();
    let server = start_many(&[&one, &two], &[]);
    let addr = server.addr;
    let (i1, i2) = (lib_id(addr, &one), lib_id(addr, &two));
    assert_eq!(get(addr, "/raw/api/all/transfer").json().as_array().unwrap().len(), 0, "no hints for drives that may be backups");
    let refused = post(addr, "/raw/api/all/transfer", &json!({ "from": i1, "to": i2 }));
    assert_eq!(refused.status, 400);
    assert!(refused.json()["error"].as_str().unwrap().contains("Decide first"));
    // Said "separate" for both: they take part.
    for id in [&i1, &i2] {
        assert_eq!(post(addr, "/raw/api/all/role", &json!({ "library": id, "role": "separate" })).status, 200);
    }
    let done = post(addr, "/raw/api/all/transfer", &json!({ "from": i1, "to": i2 }));
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    assert_eq!(done.json()["counts"]["tags"], 1);
    server.stop().unwrap();
}

#[test]
fn a_drive_that_was_emptied_is_recognised_so_the_dialog_can_warn() {
    let a = source("transfer-emptied-a");
    assert!(!transfer::looks_emptied(&db_path(&a)).unwrap());
    for p in [FJORD, PAPA, DROHNE] {
        fs::remove_file(a.path(p)).unwrap();
    }
    a.scan_opts(true, false, false);
    assert!(transfer::looks_emptied(&db_path(&a)).unwrap(), "3 of 4 files are gone");
}

// ---------------------------------------------------------------- from a file (phase 15b)

/// Where a copy of a `userdata.json` is kept, outside every library.
fn kept_aside(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shoebox-it-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The source drive writes its `userdata.json`, as the server does after every change.
fn write_userdata(a: &Library) -> std::path::PathBuf {
    tags::write_user_data(&open(a), &db_path(a)).unwrap()
}

#[test]
fn a_userdata_json_carries_the_same_as_its_drive() {
    let a = source("transfer-file-a");
    let b = target("transfer-file-b");
    make_data(&a);
    let file = write_userdata(&a);
    let before = fs::read(&file).unwrap();

    let (set, info) = transfer::read_userdata(&file).unwrap();
    assert_eq!(info.version, tags::USERDATA_VERSION);
    assert!(info.written_at > 0);
    let conn = open(&b);
    let expected = Counts { tags: 3, hearts: 1, people: 2, new_people: 1, dates: 2, places: 1, photos: 3 };
    assert_eq!(transfer::plan(&conn, &set, Boxes::all()).unwrap(), expected);
    assert_eq!(transfer::apply(&conn, &set, Boxes::all()).unwrap(), expected);

    assert_eq!(own_tag_rows(&b, "fjord", Some("Familie/2026/IMG_1.jpg")), 1);
    assert_eq!(own_tag_rows(&b, "papa", None), 2, "both identical copies");
    assert_eq!(own_tag_rows(&b, "favorite", Some("Familie/2026/IMG_1.jpg")), 1);
    assert_eq!(b.count("SELECT count(*) FROM people WHERE name = 'Anna' AND group_id IS NULL"), 1);
    assert_eq!(
        b.count(&format!(
            "SELECT count(*) FROM face_decisions WHERE key = '{}' AND decision = 'confirmed' AND manual = 1 AND at = 5 AND person_id = (SELECT id FROM people WHERE name = 'Anna')",
            b.record_key("Familie/2026/IMG_1.jpg")
        )),
        1
    );
    assert_eq!(b.count("SELECT count(*) FROM face_decisions WHERE decision = 'ignored' AND person_id IS NULL"), 1);
    assert_eq!(b.count("SELECT count(*) FROM taken_overrides WHERE taken = '1999-12-31T23:59:00'"), 1);
    assert_eq!(b.count("SELECT count(*) FROM date_estimates WHERE year = 1987 AND month = 6 AND day IS NULL"), 1);
    assert_eq!(b.count("SELECT count(*) FROM geo_overrides WHERE lat = 69.6 AND lon = 18.9"), 1);

    // A second run adds nothing; the file is only read.
    assert!(transfer::apply(&conn, &set, Boxes::all()).unwrap().is_nothing());
    assert_eq!(fs::read(&file).unwrap(), before);
}

#[test]
fn a_lost_index_gets_its_data_back_from_a_copy_of_its_userdata_json() {
    let a = source("transfer-restore");
    make_data(&a);
    let aside = kept_aside("transfer-restore-copy");
    let copy = aside.join("userdata.json");
    fs::copy(write_userdata(&a), &copy).unwrap();
    let before = a.snapshot();

    // The index is lost; a scan makes a new one, without the user's data.
    for name in ["library.db", "library.db.bak", "userdata.json"] {
        let _ = fs::remove_file(a.path(&format!(".shoebox/{name}")));
    }
    a.scan_opts(true, false, false);
    assert_eq!(own_tag_rows(&a, "fjord", None), 0);

    let (set, _) = transfer::read_userdata(&copy).unwrap();
    let done = transfer::apply(&open(&a), &set, Boxes::all()).unwrap();
    // Everything is back, the photo that was only on this drive included.
    assert_eq!((done.tags, done.hearts, done.people, done.dates, done.places), (3, 1, 3, 2, 1));
    assert_eq!(own_tag_rows(&a, "drohne", Some(DROHNE)), 1);
    assert_eq!(a.count(&format!("SELECT count(*) FROM face_decisions WHERE key = '{}'", a.record_key("Solo/nur-a.jpg"))), 1);
    assert_eq!(a.snapshot(), before, "no photo changed");
    let _ = fs::remove_dir_all(&aside);
}

#[test]
fn every_version_of_the_file_is_read_and_what_does_not_pass_is_left_out() {
    let b = target("transfer-file-versions");
    let k1 = b.record_key("Familie/2026/IMG_1.jpg");
    let k2 = b.record_key("Familie/2026/IMG_2.jpg");
    let full1 = b.record("Familie/2026/IMG_1.jpg").unwrap().1.unwrap();
    let other = "f".repeat(64);

    // Version 1: own tags only, no lists of the later versions.
    let v1 = json!({
        "shoebox": "0.1.0", "version": 1, "written_at": 1_700_000_000,
        "own_tags": [
            { "name": "Fjord", "files": [{ "path": "a.jpg", "quick_hash": k1, "full_hash": full1 }] },
            // Another content under the same quick hash: not this photo.
            { "name": "Bearbeitet", "files": [{ "path": "b.jpg", "quick_hash": k1, "full_hash": other }] },
            // In the source's trash: not carried.
            { "name": "Weg", "files": [{ "path": "c.jpg", "quick_hash": k2, "full_hash": null, "in_trash": true }] },
            { "name": "Favoriten", "files": [{ "path": "a.jpg", "quick_hash": k1, "full_hash": null }] },
            { "name": "   ", "files": [{ "path": "a.jpg", "quick_hash": k1 }] },
            { "name": "Kaputt", "files": [{ "path": "a.jpg", "quick_hash": "../../etc" }] }
        ]
    });
    let (set, info) = transfer::parse_userdata(v1.to_string().as_bytes()).unwrap();
    assert_eq!((info.version, info.written_at), (1, 1_700_000_000));
    assert_eq!(set.tags.len(), 2, "the blank name and the bad hash are left out");
    assert_eq!(set.hearts.len(), 1, "Favoriten is the heart");
    let done = transfer::apply(&open(&b), &set, Boxes::all()).unwrap();
    assert_eq!((done.tags, done.hearts, done.photos), (1, 1, 1));
    assert_eq!(own_tag_rows(&b, "fjord", Some("Familie/2026/IMG_1.jpg")), 1);
    assert_eq!(own_tag_rows(&b, "bearbeitet", None) + own_tag_rows(&b, "weg", None), 0);

    // Version 6 with entries that cannot be right: each is left out alone.
    let v6 = json!({
        "shoebox": "0.9.0", "version": 6, "written_at": 1_700_000_100,
        "groups": [], "people": [], "places": [], "view_turns": [], "shot_marks": [], "text_hidden": [],
        "face_decisions": [
            { "person": "Bea", "decision": "confirmed", "key": k2, "box": [0.1, 0.1, 0.2, 0.2], "at": 0, "files": [] },
            { "person": "Cleo", "decision": "maybe", "key": k2, "box": [0.5, 0.5, 0.2, 0.2], "at": 7, "files": [] },
            { "person": "Dora", "decision": "confirmed", "key": k2, "box": [0.5, 0.5, 0.0, 0.2], "at": 7, "files": [] },
            { "person": "Ella", "decision": "confirmed", "key": k2, "box": [0.7, 0.7, 0.1, 0.1], "at": 7, "species": "horse", "files": [] },
            { "person": "Finn", "decision": "confirmed", "key": k2, "box": [0.7, 0.1, 0.1, 0.1], "at": 7, "files": [{ "path": "x.jpg", "full_hash": null, "in_trash": true }] },
            { "person": "Gabi", "decision": "ignored", "key": k1, "box": [0.3, 0.3, 0.1, 0.1], "at": 8, "files": [] }
        ],
        "taken_overrides": [
            { "quick_hash": k1, "taken": "2001-02-03T04:05:06", "taken_offset": "+01:00", "files": [] },
            { "quick_hash": k2, "taken": "gestern", "taken_offset": null, "files": [] }
        ],
        "date_estimates": [
            { "quick_hash": k2, "year": 3000, "month": null, "day": null, "files": [] }
        ],
        "geo_overrides": [
            { "quick_hash": k1, "lat": 48.1, "lon": 11.5, "files": [] },
            { "quick_hash": k2, "lat": 200.0, "lon": 11.5, "files": [] }
        ],
        "something_new": { "ignored": true }
    });
    let (set, _) = transfer::parse_userdata(v6.to_string().as_bytes()).unwrap();
    assert_eq!(set.decisions.len(), 2, "Bea and the ignored face");
    assert!(set.decisions.iter().all(|d| d.person.as_deref() != Some("Gabi")), "an ignored face names nobody");
    assert_eq!(set.decisions.iter().find(|d| d.person.as_deref() == Some("Bea")).unwrap().at, 1_700_000_100, "no time: when the file was written");
    assert_eq!((set.dates.len(), set.places.len()), (1, 1));
    let done = transfer::apply(&open(&b), &set, Boxes::all()).unwrap();
    assert_eq!((done.people, done.new_people, done.dates, done.places), (2, 1, 1, 1));
    assert_eq!(b.count("SELECT count(*) FROM people"), 1);
    assert_eq!(b.count(&format!("SELECT count(*) FROM taken_overrides WHERE key = '{k1}' AND taken_offset = '+01:00'")), 1);

    // Not a userdata.json, or one from a newer shoebox.
    for bad in [&b"not json"[..], br#"{"version": 1}"#, br#"{"shoebox": "x", "version": 0}"#, br#"[1, 2]"#] {
        assert!(transfer::parse_userdata(bad).is_err(), "{}", String::from_utf8_lossy(bad));
    }
    let newer = json!({ "shoebox": "9.0.0", "version": tags::USERDATA_VERSION + 1, "written_at": 1 });
    let err = transfer::parse_userdata(newer.to_string().as_bytes()).unwrap_err().to_string();
    assert!(err.contains("newer"), "{err}");
}

#[test]
fn only_a_json_file_is_opened() {
    let b = target("transfer-file-kind");
    // A photo is never opened as a source, whatever it holds.
    let err = transfer::read_userdata(&b.path("Familie/2026/IMG_1.jpg")).unwrap_err().to_string();
    assert!(err.contains("userdata.json"), "{err}");
    assert!(transfer::read_userdata(&b.path("missing.json")).is_err());
    assert!(transfer::read_userdata(&b.path(".shoebox")).is_err(), "a folder is not a file");
}

#[test]
fn the_routes_take_a_file_from_this_computer_only() {
    let a = source("transfer-file-api-a");
    let b = target("transfer-file-api-b");
    make_data(&a);
    let aside = kept_aside("transfer-file-api-copy");
    let copy = aside.join("userdata (Reise).json");
    fs::copy(write_userdata(&a), &copy).unwrap();
    fs::write(aside.join("notes.txt"), b"x").unwrap();
    let file = copy.display().to_string();

    // One drive is enough: the data comes from the file.
    let server = start(&b, Some("4711"));
    let addr = server.addr;
    let ib = lib_id(addr, &b);

    // The picker lists the .json files and the .shoebox folders.
    let list = get(addr, &format!("/api/export/folders?path={}&files=json", encode(&aside.display().to_string()))).json();
    let files: Vec<&str> = list["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(files, ["userdata (Reise).json"]);
    let root = get(addr, &format!("/api/export/folders?path={}&files=json", encode(&b.root.display().to_string()))).json();
    assert!(root["folders"].as_array().unwrap().iter().any(|f| f["name"] == ".shoebox"));
    let plain = get(addr, &format!("/api/export/folders?path={}", encode(&b.root.display().to_string()))).json();
    assert!(plain["folders"].as_array().unwrap().iter().all(|f| f["name"] != ".shoebox"), "the export's picker is unchanged");
    assert_eq!(plain["files"].as_array().unwrap().len(), 0);

    let plan = post(addr, "/raw/api/all/transfer/plan", &json!({ "file": file, "to": ib }));
    assert_eq!(plan.status, 200, "{}", String::from_utf8_lossy(&plan.body));
    let plan = plan.json();
    assert_eq!(plan["counts"]["photos"], 3);
    assert_eq!(plan["file_name"], "userdata (Reise).json");
    assert_eq!(plan["version"], tags::USERDATA_VERSION);
    assert_eq!(own_tag_rows(&b, "fjord", None), 0, "the plan writes nothing");

    // Neither or both sources, a relative path, a file that is not one.
    assert_eq!(post(addr, "/raw/api/all/transfer/plan", &json!({ "to": ib })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/transfer/plan", &json!({ "from": ib, "file": file, "to": ib })).status, 400);
    assert_eq!(post(addr, "/raw/api/all/transfer/plan", &json!({ "file": "userdata.json", "to": ib })).status, 400);
    let photo = b.path("Familie/2026/IMG_1.jpg").display().to_string();
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "file": photo, "to": ib })).status, 400);
    let txt = aside.join("notes.txt").display().to_string();
    assert_eq!(post(addr, "/raw/api/all/transfer", &json!({ "file": txt, "to": ib })).status, 400);

    // A device on the network may not have this computer's files read.
    let evil = ("Host", "photos.evil.example");
    let ct = ("Content-Type", "application/json");
    let login = request(addr, "POST", "/raw/api/login", &[evil, ct], br#"{"pin":"4711"}"#);
    let cookie = login.header("set-cookie").unwrap().split(';').next().unwrap().to_string();
    let lan = [evil, ("Cookie", cookie.as_str()), ct];
    let body = json!({ "file": file, "to": ib }).to_string();
    for path in ["/raw/api/all/transfer/plan", "/raw/api/all/transfer"] {
        assert_eq!(request(addr, "POST", path, &lan, body.as_bytes()).status, 403, "{path}");
    }
    assert_eq!(own_tag_rows(&b, "fjord", None), 0);

    let done = post(addr, "/raw/api/all/transfer", &json!({ "file": file, "to": ib, "boxes": { "people": false } }));
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    let done = done.json();
    assert_eq!(done["counts"]["tags"], 3);
    assert_eq!(done["counts"]["people"], 0);
    assert_eq!(own_tag_rows(&b, "papa", None), 2);
    assert_eq!(b.count("SELECT count(*) FROM face_decisions"), 0, "people were not ticked");
    server.stop().unwrap();
    let _ = fs::remove_dir_all(&aside);
}
