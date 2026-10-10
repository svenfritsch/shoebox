//! Integration tests for estimated capture dates (phase 12): a date the user
//! gives a photo (year, month or day) wins on the timeline, keeps the file's own
//! date, sorts and groups by its precision, is found by date terms, and is
//! carried with the content. Nothing is ever written to a photo.

mod common;

use std::fs;

use common::*;
use serde_json::{Value, json};

const FOLDER_A: &str = "2020-07 Urlaub Griechenland/IMG_0001.JPG";
const FOLDER_B: &str = "2020-07 Urlaub Griechenland/IMG_0002.JPG";
const LOOSE: &str = "Familie/Weihnachten/DSC_2001.jpg";
const SPACES: &str = "Ordner mit Leerzeichen/Bild 1.jpeg";

fn details(addr: std::net::SocketAddr, id: i64) -> Value {
    get(addr, &format!("/api/files/{id}")).json()
}

fn set(addr: std::net::SocketAddr, ids: &[i64], body: Value) -> common::Response {
    let mut b = body;
    b["ids"] = json!(ids);
    post(addr, "/api/files/dates", &b)
}

fn timeline(addr: std::net::SocketAddr, q: &str) -> Value {
    let r = get(addr, &format!("/api/timeline?{q}"));
    assert_eq!(r.status, 200, "{q}");
    r.json()
}

fn sorted(mut v: Vec<i64>) -> Vec<i64> {
    v.sort();
    v
}

fn set_taken(lib: &Library, id: i64, taken: Option<&str>) {
    lib.db().execute("UPDATE files SET taken = ?2, taken_offset = NULL WHERE id = ?1", rusqlite::params![id, taken]).unwrap();
}

#[test]
fn the_users_date_wins_and_the_files_own_comes_back() {
    let lib = Library::new("dates-wins");
    lib.scan();
    let (folder, loose, exif) = (id_of(&lib, FOLDER_A), id_of(&lib, LOOSE), id_of(&lib, SPACES));
    set_taken(&lib, exif, Some("2026-10-03T16:12:00"));
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;

    // The order today: file date, event folder, created or modified.
    assert_eq!(details(addr, exif)["date_source"], "file");
    assert_eq!(details(addr, folder)["date_source"], "folder");
    assert!(["created", "modified"].contains(&details(addr, loose)["date_source"].as_str().unwrap()));
    assert!(details(addr, folder)["estimate"].is_null() && details(addr, exif)["exif"]["taken"] == "2026-10-03T16:12:00");

    // A photo without a capture date takes the user's month.
    let r = set(addr, &[folder], json!({ "year": 1987, "month": 6 }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!((r.json()["changed"].as_i64(), r.json()["with_exif"].as_i64()), (Some(1), Some(0)));
    assert!(r.json()["previous"][0]["estimate"].is_null());
    let d = details(addr, folder);
    assert_eq!(d["date_source"], "estimate");
    assert_eq!(d["estimate"], json!({ "year": 1987, "month": 6, "day": null }));
    assert_eq!(d["sort_date"], "1987-06-00T00:00:00");
    assert!(d["exif"].is_null());

    // A photo that has a capture date takes it too: the user's date wins, the
    // file's stays visible.
    let r = set(addr, &[exif], json!({ "year": 1987, "month": 6, "day": 14 }));
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["with_exif"], 1, "the answer says it had one");
    let d = details(addr, exif);
    assert_eq!(d["date_source"], "estimate");
    assert_eq!(d["sort_date"], "1987-06-14T00:00:00");
    assert_eq!((d["exif"]["taken"].as_str(), d["taken"].as_str()), (Some("2026-10-03T16:12:00"), Some("2026-10-03T16:12:00")));

    // Taking it away brings the file's date back; a rescan changes nothing.
    assert_eq!(set(addr, &[exif], json!({ "clear": true })).status, 200);
    let d = details(addr, exif);
    assert_eq!((d["date_source"].as_str(), d["sort_date"].as_str()), (Some("file"), Some("2026-10-03T16:12:00")));
    assert!(d["estimate"].is_null());
    assert_eq!(set(addr, &[folder], json!({ "clear": true })).status, 200);
    assert_eq!(details(addr, folder)["date_source"], "folder");
    lib.scan();
    assert_eq!(lib.count("SELECT count(*) FROM date_estimates"), 0);

    // The same date twice changes nothing.
    assert_eq!(set(addr, &[folder], json!({ "year": 1990 })).json()["changed"], 1);
    assert_eq!(set(addr, &[folder], json!({ "year": 1990 })).json()["changed"], 0);

    // Dates that cannot be are refused, and nothing is stored.
    assert_eq!(set(addr, &[loose], json!({ "year": 1990, "day": 5 })).status, 400, "a day needs a month");
    for bad in [
        json!({ "year": 1990, "month": 13 }),
        json!({ "year": 1990, "month": 0 }),
        json!({ "year": 1990, "month": 2, "day": 30 }),
        json!({ "year": 1987, "month": 2, "day": 29 }),
        json!({ "year": 1799 }),
        json!({ "year": 2999 }),
        json!({ "month": 6 }),
    ] {
        let r = set(addr, &[loose], bad.clone());
        assert_eq!(r.status, 400, "{bad}");
    }
    assert_eq!(set(addr, &[loose, 999_999], json!({ "year": 1990 })).status, 400, "an unknown photo refuses all");
    assert!(details(addr, loose)["estimate"].is_null());
    assert_eq!(set(addr, &[loose], json!({ "year": 1988, "month": 2, "day": 29 })).status, 200, "a leap day");

    // Changes need the header.
    let body = json!({ "ids": [loose], "year": 1990 }).to_string();
    let r = bare_request(addr, "POST", "/api/files/dates", &[("Content-Type", "application/json")], body.as_bytes());
    assert_eq!(r.status, 403);

    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "originals untouched");
    assert!(lib.verify(false).is_clean());
}

#[test]
fn years_months_and_days_sort_into_the_timeline_with_a_group_for_a_year() {
    let lib = Library::new("dates-sort");
    lib.scan();
    let (year, month, day, folder, exif) = (id_of(&lib, LOOSE), id_of(&lib, SPACES), id_of(&lib, FOLDER_B), id_of(&lib, FOLDER_A), id_of(&lib, "Familie/Screenshot.png"));
    set_taken(&lib, exif, Some("2019-12-24T18:00:00"));
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(set(addr, &[year], json!({ "year": 1987 })).status, 200);
    assert_eq!(set(addr, &[month], json!({ "year": 1987, "month": 6 })).status, 200);
    assert_eq!(set(addr, &[day], json!({ "year": 1987, "month": 6, "day": 14 })).status, 200);

    let t = timeline(addr, "");
    let order = ids(&t);
    let pos = |id: i64| order.iter().position(|&x| x == id).unwrap();
    // Newest first: the day, then the month of that month, then the year alone,
    // all below the photos of 2019 and 2020.
    assert!(pos(day) < pos(month) && pos(month) < pos(year), "{order:?}");
    assert!(pos(exif) < pos(day) && pos(folder) < pos(day));
    let days: Vec<i64> = t["days"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(days[pos(day)], 19870614);
    assert_eq!(days[pos(month)], 19870600, "a month group, after the exact days");
    assert_eq!(days[pos(year)], 19870000, "the year alone: a group of its own, month 0");

    // The "~": the user's dates are `m`, the folder's month is `f`; a capture
    // date in the file has none.
    let est: Vec<(i64, String)> = t["est"].as_array().unwrap().iter().map(|e| (e[0].as_i64().unwrap(), e[1].as_str().unwrap().to_string())).collect();
    let code = |id: i64| est.iter().find(|e| e.0 == id).map(|e| e.1.as_str());
    assert_eq!((code(day), code(month), code(year)), (Some("m"), Some("m"), Some("m")));
    assert_eq!(code(folder), Some("f"));
    assert_eq!(code(exif), None);

    server.stop().unwrap();
}

#[test]
fn date_terms_find_what_lies_inside_them() {
    let lib = Library::new("dates-filter");
    lib.scan();
    let (year, month, day) = (id_of(&lib, LOOSE), id_of(&lib, SPACES), id_of(&lib, FOLDER_B));
    let (folder, exif) = (id_of(&lib, FOLDER_A), id_of(&lib, "Familie/Screenshot.png"));
    set_taken(&lib, exif, Some("1987-06-14T09:00:00"));
    let server = start(&lib, None);
    let addr = server.addr;
    set(addr, &[year], json!({ "year": 1987 }));
    set(addr, &[month], json!({ "year": 1987, "month": 6 }));
    set(addr, &[day], json!({ "year": 1987, "month": 6, "day": 14 }));

    let found = |q: &str| sorted(ids(&timeline(addr, q)));
    // A year finds every precision; a month finds a month and a day; a day only a day.
    assert_eq!(found("date=1987"), sorted(vec![year, month, day, exif]));
    assert_eq!(found("date=1987-06"), sorted(vec![month, day, exif]));
    assert_eq!(found("date=1987-06-14"), sorted(vec![day, exif]));
    assert!(found("date=1987-07").is_empty() && found("date=1986").is_empty());
    // A date from an event folder is a month, never an exact day.
    assert_eq!(found("date=2020-07"), sorted(vec![folder]));
    assert!(found("date=2020-07-01").is_empty());
    // Terms combine (AND), and with the other filters.
    assert_eq!(found("date=1987&date=1987-06"), sorted(vec![month, day, exif]));
    assert_eq!(found("date=1987&date=2020"), Vec::<i64>::new());
    assert_eq!(get(addr, "/api/timeline?date=87").status, 400);
    assert_eq!(get(addr, "/api/timeline?date=1987-13").status, 400);

    // "Needs a date": no capture date in the file and none of the user's.
    let needs = found("nodate=1");
    assert!(needs.contains(&folder));
    for id in [year, month, day, exif] {
        assert!(!needs.contains(&id), "{id}");
    }
    assert_eq!(get(addr, "/api/info").json()["needs_date"], needs.len());
    set(addr, &[folder], json!({ "year": 2001 }));
    assert!(!found("nodate=1").contains(&folder), "a date of the user's takes it off the list");
    assert_eq!(get(addr, "/api/info").json()["needs_date"], needs.len() - 1);

    // The suggestions of the "Date" group.
    let rows = |q: &str| -> Vec<(i64, Option<i64>, Option<i64>, i64)> {
        let r = get(addr, &format!("/api/dates?{q}"));
        assert_eq!(r.status, 200, "{q}");
        r.json().as_array().unwrap().iter().map(|r| (r["year"].as_i64().unwrap(), r["month"].as_i64(), r["day"].as_i64(), r["count"].as_i64().unwrap())).collect()
    };
    assert_eq!(rows("q=1987"), vec![(1987, None, None, 4)]);
    assert_eq!(rows("q=june+1987"), vec![(1987, Some(6), None, 3)]);
    assert_eq!(rows("q=14.6.1987"), vec![(1987, Some(6), Some(14), 2)]);
    assert!(rows("q=june").is_empty(), "a month alone gets no row without the prefix");
    assert_eq!(rows("q=juni&prefix=1"), vec![(1987, Some(6), None, 3)], "with the prefix, in German too");
    assert_eq!(rows("q=1987&prefix=1"), vec![(1987, None, None, 4), (1987, Some(6), None, 3)]);
    let years: Vec<i64> = rows("q=&prefix=1").iter().map(|r| r.0).collect();
    assert!(years.windows(2).all(|w| w[0] > w[1]), "the years, newest first: {years:?}");
    assert!(years.contains(&2001) && years.contains(&1987));
    assert!(rows("q=1950").is_empty());
    assert!(rows("q=tag").is_empty());
    // Counted within the rest of the filter: no videos here.
    assert!(rows("q=1987&type=video").is_empty());

    server.stop().unwrap();
}

#[test]
fn copies_share_a_date_and_undo_puts_back_what_was_there() {
    let lib = Library::new("dates-undo");
    // An exact copy: same content, so one date.
    fs::create_dir_all(lib.path("Kopie")).unwrap();
    fs::copy(lib.path(FOLDER_A), lib.path("Kopie/IMG_0001.JPG")).unwrap();
    lib.scan();
    let (a, copy, b) = (id_of(&lib, FOLDER_A), id_of(&lib, "Kopie/IMG_0001.JPG"), id_of(&lib, FOLDER_B));
    let server = start(&lib, None);
    let addr = server.addr;

    set(addr, &[a], json!({ "year": 1987 }));
    assert_eq!(details(addr, copy)["estimate"]["year"], 1987, "the copy has the same content");
    assert_eq!(lib.count("SELECT count(*) FROM date_estimates"), 1);

    // A batch changes two photos; the answer says what each had.
    let r = set(addr, &[a, b], json!({ "year": 1990, "month": 5 }));
    assert_eq!(r.status, 200);
    let prev = r.json()["previous"].clone();
    assert_eq!(prev[0], json!({ "id": a, "estimate": { "year": 1987, "month": null, "day": null } }));
    assert_eq!(prev[1], json!({ "id": b, "estimate": null }));
    assert_eq!(details(addr, b)["estimate"]["month"], 5);

    // Two copies of one content in the same batch: Undo still restores what each had.
    let r = set(addr, &[a, copy], json!({ "year": 1999 }));
    assert_eq!(r.json()["previous"][0]["estimate"]["year"], 1990);
    assert_eq!(r.json()["previous"][1]["estimate"]["year"], 1990, "read before anything was written");
    let r = post(addr, "/api/files/dates", &json!({ "restore": r.json()["previous"] }));
    assert_eq!(r.status, 200);
    assert_eq!(details(addr, copy)["estimate"]["year"], 1990);

    // Undo: the earlier dates come back, a photo that had none has none again.
    let r = post(addr, "/api/files/dates", &json!({ "restore": prev }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(details(addr, a)["estimate"], json!({ "year": 1987, "month": null, "day": null }));
    assert!(details(addr, b)["estimate"].is_null());

    // What a dialog for a selection says.
    set_taken(&lib, b, Some("2019-01-01T00:00:00"));
    let c = post(addr, "/api/files/dates/check", &json!({ "ids": [a, b, copy] })).json();
    assert_eq!((c["total"].as_i64(), c["with_exif"].as_i64(), c["with_estimate"].as_i64()), (Some(3), Some(1), Some(2)));
    server.stop().unwrap();
}

fn rings(path: &std::path::Path, w: u32, h: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(w, h, |x, y| {
        let (cx, cy) = (x as f32 * 1600.0 / w as f32, y as f32 * 1200.0 / h as f32);
        let d = ((cx - 1000.0).powi(2) + (cy - 500.0).powi(2)).sqrt();
        image::Rgb([((d / 25.0).sin() * 100.0 + 128.0) as u8, (cy / 5.0) as u8, 90])
    })
    .save(path)
    .unwrap();
}

#[test]
fn removing_a_copy_hands_its_date_to_the_survivor_and_the_backup_has_it() {
    let lib = Library::new("dates-dupes");
    let original = "Ringe/gross.jpg";
    rings(&lib.path(original), 1600, 1200);
    rings(&lib.path("A/klein.jpg"), 800, 600);
    lib.scan();
    let before = lib.snapshot();
    let (orig, small) = (id_of(&lib, original), id_of(&lib, "A/klein.jpg"));
    let server = start(&lib, None);
    let addr = server.addr;

    // The copy that goes has a date of the user's, the survivor has none.
    set(addr, &[small], json!({ "year": 1987, "month": 8 }));
    assert!(details(addr, orig)["estimate"].is_null());
    let r = post(addr, "/api/duplicates/remove", &json!({ "keep": [orig], "remove": [small] }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(details(addr, orig)["estimate"], json!({ "year": 1987, "month": 8, "day": null }));

    // A survivor that has its own keeps it.
    rings(&lib.path("B/klein.jpg"), 720, 540);
    lib.scan();
    let other = id_of(&lib, "B/klein.jpg");
    set(addr, &[other], json!({ "year": 1999 }));
    let r = post(addr, "/api/duplicates/remove", &json!({ "keep": [orig], "remove": [other] }));
    assert_eq!(r.status, 200);
    assert_eq!(details(addr, orig)["estimate"]["year"], 1987);

    // The backup of what the user made.
    server.stop().unwrap();
    let data: Value = serde_json::from_slice(&fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["version"], 6);
    let mine = data["date_estimates"].as_array().unwrap().iter().find(|e| e["files"][0] == original).expect("the survivor's date");
    assert_eq!((mine["year"].as_i64(), mine["month"].as_i64()), (Some(1987), Some(8)));
    assert_eq!(before[&lib.path(original)], lib.snapshot()[&lib.path(original)], "the survivor is untouched");
}

#[test]
fn an_older_index_gets_the_table_without_losing_anything() {
    let lib = Library::new("dates-migrate");
    lib.scan();
    let db = lib.db();
    db.execute_batch("DROP TABLE date_estimates; PRAGMA user_version = 12;").unwrap();
    drop(db);
    lib.scan();
    assert_eq!(lib.count("SELECT count(*) FROM date_estimates"), 0);
    let v: i64 = lib.db().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 13);
}
