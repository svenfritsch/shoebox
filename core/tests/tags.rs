//! Integration tests for own tags (phase 5b): adding and removing them, many
//! photos at once, names folded, folder tags that cannot be removed, tags
//! that survive moves, rescans and the trash, the user data backup, and the
//! guard around every endpoint.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use shoebox::fingerprint::Stamp;

/// Own tags of a file (names, sorted).
fn own_tags(lib: &Library, id: i64) -> Vec<String> {
    tags_by_source(lib, id, "user")
}

fn folder_tags(lib: &Library, id: i64) -> Vec<String> {
    tags_by_source(lib, id, "folder")
}

fn tags_by_source(lib: &Library, id: i64, source: &str) -> Vec<String> {
    lib.db()
        .prepare(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE ft.file_id = ?1 AND ft.source = ?2 ORDER BY t.name",
        )
        .unwrap()
        .query_map(rusqlite::params![id, source], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Every own tag row as (file path, tag name).
fn user_rows(lib: &Library) -> Vec<(String, String)> {
    lib.db()
        .prepare(
            "SELECT f.path_nfc, t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id JOIN files f ON f.id = ft.file_id
             WHERE ft.source = 'user' ORDER BY f.path_nfc, t.name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn add(addr: std::net::SocketAddr, ids: &[i64], name: &str) -> Value {
    let r = post(addr, "/api/tags/add", &json!({ "ids": ids, "name": name }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn remove(addr: std::net::SocketAddr, ids: &[i64], name: &str) -> Value {
    let r = post(addr, "/api/tags/remove", &json!({ "ids": ids, "name": name }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn tag_list(addr: std::net::SocketAddr, query: &str) -> Vec<Value> {
    get(addr, &format!("/api/tags{query}")).json().as_array().unwrap().clone()
}

fn find<'a>(tags: &'a [Value], name: &str) -> Option<&'a Value> {
    tags.iter().find(|t| t["name"] == name)
}

fn search(addr: std::net::SocketAddr, q: &str) -> Vec<i64> {
    ids(&get(addr, &format!("/api/timeline?q={}", encode(q))).json())
}

fn assert_untouched(before: &BTreeMap<PathBuf, (Stamp, String)>, after: &BTreeMap<PathBuf, (Stamp, String)>) {
    assert_eq!(before.len(), after.len(), "files were added or removed outside .shoebox");
    for (path, (stamp, hash)) in before {
        let (stamp_after, hash_after) = &after[path];
        assert_eq!(stamp, stamp_after, "timestamps or size changed: {}", path.display());
        assert_eq!(hash, hash_after, "content changed: {}", path.display());
    }
}

#[test]
fn own_tags_add_remove_many_at_once_and_fold_names() {
    let lib = Library::new("tags-api");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;

    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let img2 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let png = id_of(&lib, "Familie/Screenshot.png");

    // Add to several photos at once; the first spelling wins, other
    // spellings and spaces around the name are the same tag.
    let r = add(addr, &[img1, img2], "Europa-Park");
    assert_eq!((r["files"].as_i64(), r["tag"]["name"].as_str()), (Some(2), Some("Europa-Park")));
    let r = add(addr, &[img1, img2, dsc, 999_999], "  europa-PARK ");
    assert_eq!((r["files"].as_i64(), r["tag"]["name"].as_str()), (Some(1), Some("Europa-Park")), "{r}");
    assert_eq!(own_tags(&lib, dsc), ["Europa-Park"]);
    // NFC: a decomposed umlaut is the same tag as a composed one.
    add(addr, &[img1], "O\u{308}sterreich");
    let r = add(addr, &[img2], "\u{f6}sterreich");
    assert_eq!(r["tag"]["name"], "\u{d6}sterreich");
    assert_eq!(own_tags(&lib, img2), ["Europa-Park", "\u{d6}sterreich"]);

    // An own tag with a folder tag's name is that tag ("both").
    let r = add(addr, &[img1], "familie");
    assert_eq!(r["tag"]["name"], "Familie");
    let tags = tag_list(addr, "");
    assert_eq!(find(&tags, "Europa-Park").unwrap()["kind"], "own");
    assert_eq!(find(&tags, "Europa-Park").unwrap()["count"], 3);
    assert_eq!(find(&tags, "Familie").unwrap()["kind"], "both");
    assert_eq!(find(&tags, "Weihnachten").unwrap()["kind"], "folder");
    let own: Vec<String> = tag_list(addr, "?own=1").iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(own.contains(&"Europa-Park".to_string()) && own.contains(&"Familie".to_string()), "{own:?}");
    assert!(!own.contains(&"Weihnachten".to_string()), "{own:?}");
    assert_eq!(tag_list(addr, "?q=EUROPA").len(), 1);

    // The info panel gets folder tags first, then own tags, with their source.
    let details = get(addr, &format!("/api/files/{img1}")).json();
    let listed: Vec<(String, String)> = details["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["source"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        listed,
        [
            ("Urlaub Griechenland".into(), "folder".into()),
            ("Europa-Park".into(), "user".into()),
            ("Familie".into(), "user".into()),
            ("\u{d6}sterreich".into(), "user".into()),
        ]
    );

    // Search finds own tags like folder tags, also together with a folder name.
    let mut found = search(addr, "europa-park");
    found.sort();
    assert_eq!(found, {
        let mut v = vec![img1, img2, dsc];
        v.sort();
        v
    });
    assert_eq!(search(addr, "Weihnachten Europa-Park"), [dsc]);
    let tag_id = find(&tags, "Europa-Park").unwrap()["id"].as_i64().unwrap();
    assert_eq!(ids(&get(addr, &format!("/api/timeline?tag={tag_id}")).json()).len(), 3);

    // The own tags on a selection, for "Remove tag…".
    let sel = post(addr, "/api/tags/selection", &json!({ "ids": [img1, img2, dsc, png] })).json();
    let sel: Vec<(String, i64)> = sel
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(sel, [("Europa-Park".into(), 3), ("\u{d6}sterreich".into(), 2), ("Familie".into(), 1)]);

    // Folder tags cannot be removed: only the own row goes.
    let r = remove(addr, &[png], "Familie");
    assert_eq!((r["files"].as_i64(), r["folder"].as_i64()), (Some(0), Some(1)));
    assert_eq!(folder_tags(&lib, png), ["Familie"]);
    let r = remove(addr, &[img1], "FAMILIE");
    assert_eq!(r["files"], 1);
    assert_eq!(own_tags(&lib, img1), ["Europa-Park", "\u{d6}sterreich"]);
    assert_eq!(find(&tag_list(addr, ""), "Familie").unwrap()["kind"], "folder");
    let r = remove(addr, &[dsc], "Weihnachten");
    assert_eq!((r["files"].as_i64(), r["folder"].as_i64()), (Some(0), Some(1)));
    assert_eq!(folder_tags(&lib, dsc), ["Familie", "Weihnachten"]);

    // Remove from many at once; a tag nobody has any more is gone.
    let r = remove(addr, &[img1, img2, dsc], "europa-park");
    assert_eq!(r["files"], 3);
    assert!(find(&tag_list(addr, ""), "Europa-Park").is_none());
    assert_eq!(lib.count("SELECT count(*) FROM tags WHERE name = 'Europa-Park'"), 0);
    assert!(search(addr, "europa-park").is_empty());
    let r = remove(addr, &[img1], "never there");
    assert_eq!((r["files"].as_i64(), r["tag"].is_null()), (Some(0), true));

    // Bad names, and the header every change needs.
    for bad in ["", "   ", "a\u{7}b", &"x".repeat(101)] {
        let r = post(addr, "/api/tags/add", &json!({ "ids": [img1], "name": bad }));
        assert_eq!(r.status, 400, "{bad:?}");
    }
    let body = json!({ "ids": [img1], "name": "Sneaky" }).to_string();
    for path in ["/api/tags/add", "/api/tags/remove"] {
        let r = bare_request(addr, "POST", path, &[("Content-Type", "application/json")], body.as_bytes());
        assert_eq!(r.status, 403, "{path}");
    }
    assert_eq!(lib.count("SELECT count(*) FROM tags WHERE name = 'Sneaky'"), 0);

    // The user data backup: right after the first change, and when stopping.
    let userdata = lib.path(".shoebox/userdata.json");
    let started = Instant::now();
    while !userdata.exists() {
        assert!(started.elapsed() < Duration::from_secs(10), "no userdata.json");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.stop().unwrap();
    let data: Value = serde_json::from_slice(&fs::read(&userdata).unwrap()).unwrap();
    assert_eq!(data["version"], 7);
    let names: Vec<&str> = data["own_tags"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["\u{d6}sterreich"]);
    let files = data["own_tags"][0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    assert!(files[0]["full_hash"].is_string() && files[0]["quick_hash"].is_string());
    let bak = rusqlite::Connection::open(lib.path(".shoebox/library.db.bak")).unwrap();
    let n: i64 = bak.query_row("SELECT count(*) FROM file_tags WHERE source = 'user'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);

    assert_untouched(&before, &lib.snapshot());
}

#[test]
fn own_tags_survive_moves_rescans_and_the_trash() {
    let lib = Library::new("tags-keep");
    lib.scan();
    let before = lib.snapshot();
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let img2 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");

    let server = start(&lib, None);
    let addr = server.addr;
    add(addr, &[img1, img2, dsc], "Aurelia");
    add(addr, &[dsc], "Weihnachtsmarkt");
    let rows = user_rows(&lib);
    assert_eq!(rows.len(), 4);

    // Move: folder tags follow the path, own tags stay.
    let r = post(addr, "/api/move", &json!({ "ids": [dsc], "folder": "Familie/Advent" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(folder_tags(&lib, dsc), ["Advent", "Familie"]);
    assert_eq!(own_tags(&lib, dsc), ["Aurelia", "Weihnachtsmarkt"]);

    // Case-only rename of a folder.
    let folder: i64 = lib
        .db()
        .query_row("SELECT id FROM folders WHERE path_nfc = 'Familie/Advent'", [], |r| r.get(0))
        .unwrap();
    let r = post(addr, &format!("/api/folders/{folder}/rename"), &json!({ "path": "Familie/advent" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(own_tags(&lib, dsc), ["Aurelia", "Weihnachtsmarkt"]);

    // Trash and restore: the record is new, its own tags come back.
    let r = post(addr, "/api/trash", &json!({ "ids": [img2] }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let batch = r.json()["batches"][0].as_i64().unwrap();
    let in_trash: String = lib.db().query_row("SELECT user_tags FROM trash WHERE batch = ?1", [batch], |r| r.get(0)).unwrap();
    assert_eq!(in_trash, r#"["Aurelia"]"#);
    let r = post(addr, &format!("/api/trash/{batch}/restore"), &json!({}));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let img2 = r.json()["ids"][0].as_i64().unwrap();
    assert_eq!(own_tags(&lib, img2), ["Aurelia"]);
    assert_eq!(folder_tags(&lib, img2), ["Urlaub Griechenland"]);

    // A tag only a trashed photo had comes back with it too.
    add(addr, &[img1], "Nur hier");
    let r = post(addr, "/api/trash", &json!({ "ids": [img1] })).json();
    let batch = r["batches"][0].as_i64().unwrap();
    assert!(find(&tag_list(addr, ""), "Nur hier").is_none());
    server.stop().unwrap();
    // userdata.json lists it while it is in the trash.
    let data: Value = serde_json::from_slice(&fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    let nur = data["own_tags"].as_array().unwrap().iter().find(|t| t["name"] == "Nur hier").unwrap().clone();
    assert_eq!(nur["files"][0]["in_trash"], true);

    // Rescans keep own tags (the scan touches only folder tags), also when
    // they prune tags nothing refers to.
    lib.scan();
    let server = start(&lib, None);
    let r = post(server.addr, &format!("/api/trash/{batch}/restore"), &json!({}));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let img1 = r.json()["ids"][0].as_i64().unwrap();
    assert_eq!(own_tags(&lib, img1), ["Aurelia", "Nur hier"]);
    server.stop().unwrap();
    let rows = user_rows(&lib);
    lib.scan();
    assert_eq!(user_rows(&lib), rows);

    // Moved behind shoebox's back (a folder renamed in the Finder, a file
    // moved into another folder): the scan finds them by content and keeps
    // their own tags.
    fs::rename(lib.path("Familie"), lib.path("Familie 2012")).unwrap();
    fs::create_dir_all(lib.path("Sortiert")).unwrap();
    fs::rename(lib.path("2020-07 Urlaub Griechenland/IMG_0002.JPG"), lib.path("Sortiert/IMG_0002.JPG")).unwrap();
    let stats = lib.scan();
    assert!(stats.moved >= 2, "{stats:?}");
    assert_eq!(id_of(&lib, "Familie 2012/advent/DSC_2001.jpg"), dsc);
    assert_eq!(own_tags(&lib, dsc), ["Aurelia", "Weihnachtsmarkt"]);
    assert_eq!(folder_tags(&lib, dsc), ["Familie 2012", "advent"]);
    assert_eq!(id_of(&lib, "Sortiert/IMG_0002.JPG"), img2);
    assert_eq!(own_tags(&lib, img2), ["Aurelia"]);
    assert_eq!(folder_tags(&lib, img2), ["Sortiert"]);
    assert_eq!(user_rows(&lib).len(), 5);

    // Moved back: the originals are where they were, unchanged.
    fs::rename(lib.path("Familie 2012"), lib.path("Familie")).unwrap();
    fs::create_dir_all(lib.path("Familie/Weihnachten")).unwrap();
    fs::rename(lib.path("Familie/advent/DSC_2001.jpg"), lib.path("Familie/Weihnachten/DSC_2001.jpg")).unwrap();
    fs::remove_dir(lib.path("Familie/advent")).unwrap();
    fs::rename(lib.path("Sortiert/IMG_0002.JPG"), lib.path("2020-07 Urlaub Griechenland/IMG_0002.JPG")).unwrap();
    fs::remove_dir(lib.path("Sortiert")).unwrap();
    lib.scan();
    assert_eq!(own_tags(&lib, dsc), ["Aurelia", "Weihnachtsmarkt"]);
    assert_untouched(&before, &lib.snapshot());
}

#[test]
fn move_always_keeps_own_tags_and_can_keep_folder_tags() {
    let lib = Library::new("tags-nokeep");
    lib.scan();
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let img2 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let server = start(&lib, None);
    let addr = server.addr;
    add(addr, &[img1, img2], "Aurelia");
    add(addr, &[img1], "Nur eins");
    let old_folder = folder_tags(&lib, img1);
    assert!(!old_folder.is_empty());

    // Default: own tags move along, the old folder's tags are not kept.
    let r = post(addr, "/api/move", &json!({ "ids": [img1], "folder": "Sortiert" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(own_tags(&lib, img1), ["Aurelia", "Nur eins"]);
    assert_eq!(folder_tags(&lib, img1), ["Sortiert"]);

    // keep_folder_tags: the old folder's tags stay as own tags, next to the
    // own tags; folder tags follow the path.
    let r = post(addr, "/api/move", &json!({ "ids": [img2], "folder": "Neu", "keep_folder_tags": true }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(folder_tags(&lib, img2), ["Neu"]);
    let mut expected: Vec<String> = old_folder.iter().cloned().chain(["Aurelia".to_string()]).collect();
    expected.sort();
    let mut own = own_tags(&lib, img2);
    own.sort();
    assert_eq!(own, expected);
    server.stop().unwrap();
}

#[test]
fn guard_every_tag_endpoint_leaves_originals_untouched() {
    let lib = Library::new("tags-guard");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let all = ids(&get(addr, "/api/timeline").json());
    assert!(!all.is_empty());

    add(addr, &all, "Alle");
    assert_eq!(post(addr, "/api/tags/selection", &json!({ "ids": all })).status, 200);
    for path in ["/api/tags", "/api/tags?own=1", "/api/tags?q=alle", "/api/timeline?q=alle"] {
        assert_eq!(get(addr, path).status, 200, "{path}");
    }
    for id in &all {
        assert_eq!(get(addr, &format!("/api/files/{id}")).status, 200);
    }
    remove(addr, &all, "Alle");
    remove(addr, &all, "Familie");
    server.stop().unwrap();

    assert_untouched(&before, &lib.snapshot());
    lib.scan();
    let report = lib.verify(false);
    assert!(report.missing.is_empty() && report.changed.is_empty() && report.damaged.is_empty());
}

fn timeline_ids(addr: std::net::SocketAddr, query: &str) -> Vec<i64> {
    let r = get(addr, &format!("/api/timeline{query}"));
    assert_eq!(r.status, 200, "{query}: {}", String::from_utf8_lossy(&r.body));
    let mut v = ids(&r.json());
    v.sort();
    v
}

fn sorted(mut v: Vec<i64>) -> Vec<i64> {
    v.sort();
    v
}

#[test]
fn search_by_several_tags_at_once() {
    let lib = Library::new("tags-search");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let img2 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let png = id_of(&lib, "Familie/Screenshot.png");
    let id = |r: Value| r["tag"]["id"].as_i64().unwrap();

    let spielplatz = id(add(addr, &[img1, img2, dsc], "Spielplatz"));
    let winter = id(add(addr, &[img2, dsc, png], "Winter"));
    let schnee = id(add(addr, &[dsc], "Schnee"));

    // All tags must match.
    assert_eq!(timeline_ids(addr, &format!("?tag={spielplatz}")), sorted(vec![img1, img2, dsc]));
    assert_eq!(timeline_ids(addr, &format!("?tag={spielplatz}&tag={winter}")), sorted(vec![img2, dsc]));
    assert_eq!(timeline_ids(addr, &format!("?tag={spielplatz}&tag={winter}&tag={schnee}")), [dsc]);
    assert_eq!(timeline_ids(addr, &format!("?tag={winter}&tag={spielplatz}&tag={winter}")), sorted(vec![img2, dsc]));
    // The response names the tags of the filter, for the chips.
    let t = get(addr, &format!("/api/timeline?tag={spielplatz}&tag={winter}")).json();
    let names: Vec<&str> = t["tags"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Spielplatz", "Winter"]);

    // A folder tag and an own tag together; then with a folder and text.
    let familie = find(&tag_list(addr, ""), "Familie").unwrap()["id"].as_i64().unwrap();
    assert_eq!(timeline_ids(addr, &format!("?tag={familie}&tag={winter}")), sorted(vec![dsc, png]));
    let folder: i64 = lib.db().query_row("SELECT id FROM folders WHERE path_nfc = 'Familie'", [], |r| r.get(0)).unwrap();
    assert_eq!(timeline_ids(addr, &format!("?folder={folder}&tag={winter}")), sorted(vec![dsc, png]));
    assert_eq!(timeline_ids(addr, &format!("?folder={folder}&tag={winter}&q=dsc")), [dsc]);

    // A tag is the exact tag, not every tag containing its name (free text
    // still matches parts of words).
    let fall = id(add(addr, &[img1], "Fall"));
    add(addr, &[img2], "Fallschirm");
    assert_eq!(timeline_ids(addr, &format!("?tag={fall}")), [img1]);
    assert_eq!(timeline_ids(addr, "?q=fall"), sorted(vec![img1, img2]));

    // Suggestions within a search: counted over the photos it shows, without
    // the tags already in it, and nothing that would leave no photo.
    let within = tag_list(addr, &format!("?tag={schnee}"));
    let counts: BTreeMap<&str, i64> =
        within.iter().map(|t| (t["name"].as_str().unwrap(), t["count"].as_i64().unwrap())).collect();
    assert_eq!(counts.get("Spielplatz"), Some(&1));
    assert_eq!(counts.get("Winter"), Some(&1));
    assert_eq!(counts.get("Weihnachten"), Some(&1));
    assert!(!counts.contains_key("Schnee") && !counts.contains_key("Fall"), "{counts:?}");
    let within = tag_list(addr, &format!("?tag={spielplatz}&q=WIN"));
    assert_eq!(within.len(), 1);
    assert_eq!((within[0]["name"].as_str(), within[0]["count"].as_i64()), (Some("Winter"), Some(2)));
    assert_eq!(find(&tag_list(addr, "?q=winter"), "Winter").unwrap()["count"], 3);

    // An unknown tag finds nothing; a malformed one is refused.
    assert!(timeline_ids(addr, &format!("?tag={spielplatz}&tag=999999")).is_empty());
    assert_eq!(get(addr, "/api/timeline?tag=abc").status, 400);

    server.stop().unwrap();
    assert_untouched(&before, &lib.snapshot());
}

#[test]
fn favorites_are_the_own_tag_favorite_and_found_by_heart_or_word() {
    let lib = Library::new("favorites");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let heart = |ids: &[i64], on: bool| {
        let r = post(addr, "/api/favorites", &json!({ "ids": ids, "on": on }));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()
    };

    // The database only gets an own tag called "favorite".
    assert_eq!(heart(&[img1, dsc], true)["files"], 2);
    assert_eq!(own_tags(&lib, img1), vec!["favorite"]);
    assert_eq!(heart(&[img1], true)["files"], 0, "already there");
    // Typed as a tag, the other words are the heart too.
    assert_eq!(add(addr, &[img1], "Favoriten")["tag"]["name"], "favorite");
    assert_eq!(remove(addr, &[dsc], "favorit")["files"], 1);
    assert_eq!(heart(&[dsc], true)["files"], 1);

    // The timeline says which of its photos have a heart; `fav=1` and the
    // words in both languages find them.
    let all = get(addr, "/api/timeline").json();
    assert_eq!(sorted(all["favs"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect()), sorted(vec![img1, dsc]));
    let both = sorted(vec![img1, dsc]);
    for q in ["?fav=1", "?q=favorite", "?q=favorit", "?q=Favoriten", "?q=favorites"] {
        assert_eq!(timeline_ids(addr, q), both, "{q}");
    }
    assert_eq!(timeline_ids(addr, "?fav=1&q=weihnachten"), vec![dsc]);
    assert!(timeline_ids(addr, "?q=favoriten%20nichtda").is_empty());
    assert_eq!(timeline_ids(addr, "?fav=0").len(), ids(&all).len());

    // Taking the heart off removes the tag again.
    assert_eq!(heart(&[img1], false)["files"], 1);
    assert_eq!(timeline_ids(addr, "?fav=1"), vec![dsc]);
    heart(&[dsc], false);
    assert!(timeline_ids(addr, "?fav=1").is_empty());
    assert!(user_rows(&lib).is_empty());
    server.stop().unwrap();

    assert_untouched(&before, &lib.snapshot());
}

#[test]
fn tags_remember_when_they_were_last_used() {
    let lib = Library::new("tags-last-used");
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    let img = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");

    let last_used = |name: &str| -> Option<i64> {
        let tags = tag_list(addr, "?own=1");
        find(&tags, name).and_then(|t| t["last_used"].as_i64())
    };

    add(addr, &[img], "alt");
    add(addr, &[img], "neu");
    // Tags from before the time was recorded have none.
    lib.db().execute("UPDATE file_tags SET added_at = NULL WHERE source = 'user'", []).unwrap();
    assert_eq!((last_used("alt"), last_used("neu")), (None, None));

    // A tag counts as used when a photo is given it, even if that photo had
    // it already.
    lib.db().execute("UPDATE file_tags SET added_at = 1000 WHERE source = 'user'", []).unwrap();
    let r = add(addr, &[img], "alt");
    assert_eq!(r["files"], 0, "{r}");
    assert!(last_used("alt").unwrap() > 1000);
    assert_eq!(last_used("neu"), Some(1000));
    // The folder tag of the same photo has none (it is not used by hand).
    let tags = tag_list(addr, "");
    assert!(tags.iter().filter(|t| t["kind"] == "folder").all(|t| t.get("last_used").is_none()));
}

#[test]
fn manual_tags_only_leaves_out_what_a_folder_name_gives() {
    let lib = Library::new("tags-manual");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let png = id_of(&lib, "Familie/Screenshot.png");

    // "Familie" is a folder tag of everything under it; one photo outside gets
    // it by hand, and so does one inside (which has it both ways).
    add(addr, &[img1, png], "Familie");
    let tags = tag_list(addr, "");
    let familie = find(&tags, "Familie").unwrap();
    let id = familie["id"].as_i64().unwrap();
    assert_eq!(familie["kind"], "both");
    let all = timeline_ids(addr, &format!("?tag={id}"));
    assert!(all.contains(&img1) && all.contains(&dsc) && all.contains(&png), "{all:?}");
    assert_eq!(familie["count"].as_u64().unwrap(), all.len() as u64);
    // The count of the manual ones, and the filter that shows them.
    assert_eq!(familie["manual"], 2);
    assert_eq!(timeline_ids(addr, &format!("?tag={id}&manual=1")), sorted(vec![img1, png]));
    assert_eq!(timeline_ids(addr, &format!("?tag={id}&manual=0")), all);
    // Within another search the same: counted over what it shows.
    let within = tag_list(addr, &format!("?tag={id}&manual=1"));
    assert!(find(&within, "Weihnachten").is_none(), "{within:?}");
    // The suggestions (own=1) keep a tag that is also given by hand, even inside
    // a search whose photos only have it from their folder, and leave out a tag
    // that is only a folder name.
    let weihnachten: i64 = lib.db().query_row("SELECT id FROM folders WHERE path_nfc = 'Familie/Weihnachten'", [], |r| r.get(0)).unwrap();
    let narrowed = tag_list(addr, &format!("?folder={weihnachten}&own=1"));
    assert!(find(&narrowed, "Familie").is_some(), "{narrowed:?}");
    assert!(find(&narrowed, "Weihnachten").is_none(), "{narrowed:?}");
    let plain = tag_list(addr, "?own=1");
    assert!(find(&plain, "Familie").is_some() && find(&plain, "Weihnachten").is_none(), "{plain:?}");
    // A tag that is only a folder name has none by hand.
    assert_eq!(find(&tags, "Weihnachten").unwrap()["manual"], 0);

    server.stop().unwrap();
    assert_untouched(&before, &lib.snapshot());
}

fn hide(addr: std::net::SocketAddr, ids: &[i64], on: bool) -> Value {
    let r = post(addr, "/api/hidden", &json!({ "ids": ids, "on": on }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn hide_folder(addr: std::net::SocketAddr, path: &str, on: bool) -> Value {
    let r = post(addr, "/api/hidden/folder", &json!({ "path": path, "on": on }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn folder_row(addr: std::net::SocketAddr, path: &str) -> Value {
    let list = get(addr, "/api/folders").json();
    list.as_array().unwrap().iter().find(|f| f["path"] == path).unwrap_or_else(|| panic!("no folder {path}")).clone()
}

fn hids(timeline: &Value) -> Vec<i64> {
    sorted(timeline["hids"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect())
}

#[test]
fn hidden_is_the_own_tag_hidden_and_a_folder_flag_is_inherited() {
    let lib = Library::new("hidden");
    lib.scan();
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let img2 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0002.JPG");
    let dsc = id_of(&lib, "Familie/Weihnachten/DSC_2001.jpg");
    let shot = id_of(&lib, "Familie/Screenshot.png");
    let all = timeline_ids(addr, "");
    let without = |hidden: &[i64]| sorted(all.iter().copied().filter(|i| !hidden.contains(i)).collect());

    // The photo: the database only gets an own tag called "hidden".
    assert_eq!(hide(addr, &[img1], true)["files"], 1);
    assert_eq!(own_tags(&lib, img1), vec!["hidden"]);
    assert_eq!(hide(addr, &[img1], true)["files"], 0, "already there");
    // Typed as a tag name, the other words are the same tag.
    for name in ["Versteckt", "verborgen", "Ausgeblendet"] {
        let r = add(addr, &[img1], name);
        assert_eq!((r["tag"]["name"].as_str(), r["files"].as_i64()), (Some("hidden"), Some(0)), "{name}");
    }
    assert_eq!(own_tags(&lib, img1), vec!["hidden"]);

    // Every list leaves it out: the timeline, a folder, a tag, a type, a date, a search.
    assert_eq!(timeline_ids(addr, ""), without(&[img1]));
    assert_eq!(hids(&get(addr, "/api/timeline").json()), Vec::<i64>::new());
    let urlaub = folder_row(addr, "2020-07 Urlaub Griechenland");
    assert_eq!(urlaub["count"], 1, "the folder counts what is shown");
    let urlaub_id = urlaub["id"].as_i64().unwrap();
    assert_eq!(timeline_ids(addr, &format!("?folder={urlaub_id}")), vec![img2], "a hidden photo is not shown by clicking its folder");
    assert_eq!(timeline_ids(addr, "?type=photo"), without(&[img1, shot]));
    assert_eq!(timeline_ids(addr, "?q=griechenland"), vec![img2]);
    // The Hidden chip, and the whole word typed as text, show it; a start of the word does not.
    assert_eq!(timeline_ids(addr, "?hidden=1"), vec![img1]);
    assert_eq!(hids(&get(addr, "/api/timeline?hidden=1").json()), vec![img1]);
    for q in ["hidden", "versteckt", "Verborgen", "ausgeblendet"] {
        assert!(timeline_ids(addr, &format!("?q={q}")).contains(&img1), "{q}");
    }
    assert_eq!(timeline_ids(addr, "?q=hidden%20griechenland"), vec![img1], "only the hidden one has the word");
    for q in ["hid", "vers", "ausg"] {
        assert_eq!(timeline_ids(addr, &format!("?q={q}")), Vec::<i64>::new(), "{q} is only the start of a word");
    }
    // With the chip the other filters still apply (AND).
    assert_eq!(timeline_ids(addr, "?hidden=1&type=video"), Vec::<i64>::new());
    assert_eq!(timeline_ids(addr, "?hidden=1&q=griechenland"), vec![img1]);
    // The photo's details say why it is hidden.
    let info = get(addr, &format!("/api/files/{img1}")).json();
    assert_eq!((info["hidden"].as_bool(), info["hidden_tag"].as_bool(), info["hidden_folder"].as_str()), (Some(true), Some(true), None));
    // Tag counts and the tag list leave it out too, and the hidden tag itself is a tag like any other
    // in the list (the page does not show it).
    let tags = tag_list(addr, "");
    assert_eq!(find(&tags, "Urlaub Griechenland").unwrap()["count"], 1);

    // The folder: everything in it and below is hidden, also what comes later.
    assert_eq!(hide_folder(addr, "Familie", true)["changed"], true);
    assert_eq!(hide_folder(addr, "Familie", true)["changed"], false, "already hidden");
    assert_eq!(timeline_ids(addr, ""), without(&[img1, dsc, shot]));
    assert_eq!(timeline_ids(addr, "?hidden=1"), sorted(vec![img1, dsc, shot]));
    let familie = folder_row(addr, "Familie");
    let weihnachten = folder_row(addr, "Familie/Weihnachten");
    assert_eq!((familie["hidden"].as_bool(), familie["hidden_own"].as_bool()), (Some(true), Some(true)));
    assert_eq!((weihnachten["hidden"].as_bool(), weihnachten["hidden_own"].as_bool()), (Some(true), Some(false)), "hidden by the folder above");
    assert_eq!(familie["count"], 2, "a hidden folder counts its own photos");
    assert_eq!(folder_row(addr, "")["count"], all.len() as i64 - 3, "the folders above do not");
    // Clicking a hidden folder (or one below it) shows its photos; they are not in the timeline.
    assert_eq!(timeline_ids(addr, &format!("?folder={}", familie["id"])), sorted(vec![dsc, shot]));
    assert_eq!(timeline_ids(addr, &format!("?folder={}", weihnachten["id"])), vec![dsc]);
    assert_eq!(hids(&get(addr, &format!("/api/timeline?folder={}", familie["id"])).json()), sorted(vec![dsc, shot]));
    // Tags: folder tags of hidden photos are not counted, a tag with nothing left is not offered.
    let tags = tag_list(addr, "");
    assert!(find(&tags, "Weihnachten").is_none() && find(&tags, "Familie").is_none(), "{tags:?}");
    let info = get(addr, &format!("/api/files/{dsc}")).json();
    assert_eq!((info["hidden"].as_bool(), info["hidden_tag"].as_bool(), info["hidden_folder"].as_str()), (Some(true), Some(false), Some("Familie")));

    // A photo moved into a hidden folder is hidden; moved out of it, it is shown again.
    let r = post(addr, "/api/move", &json!({ "ids": [img2], "folder": "Familie/Weihnachten" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(timeline_ids(addr, ""), without(&[img1, dsc, shot, img2]));
    let r = post(addr, "/api/move", &json!({ "ids": [dsc], "folder": "Sortiert" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(timeline_ids(addr, ""), without(&[img1, shot, img2]));
    // A renamed hidden folder stays hidden, with the folders below it.
    let r = post(addr, &format!("/api/folders/{}/rename", familie["id"]), &json!({ "path": "Familie 2" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(timeline_ids(addr, ""), without(&[img1, shot, img2]));
    assert_eq!(folder_row(addr, "Familie 2/Weihnachten")["hidden"], true);
    assert_eq!(lib.count("SELECT count(*) FROM hidden_folders WHERE path_nfc = 'Familie 2'"), 1);
    assert_eq!(lib.count("SELECT count(*) FROM hidden_folders WHERE path_nfc = 'Familie'"), 0);

    // Broken requests are refused.
    for body in [json!({ "path": "Nicht da", "on": true }), json!({ "path": "", "on": true }), json!({ "path": "/", "on": true })] {
        assert_eq!(post(addr, "/api/hidden/folder", &body).status, 400, "{body}");
    }
    assert_eq!(post(addr, "/api/hidden/folder", &json!({ "path": "Familie 2" })).status, 422, "on is needed");
    assert_eq!(post(addr, "/api/hidden", &json!({ "ids": "x", "on": true })).status, 422);
    // Showing a folder that is not hidden is not an error and changes nothing.
    assert_eq!(hide_folder(addr, "Sortiert", false)["changed"], false);

    // The backup of what the user made says which folders are hidden.
    server.stop().unwrap();
    let data: Value = serde_json::from_slice(&fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["hidden_folders"], json!(["Familie 2"]));
    assert!(data["own_tags"].as_array().unwrap().iter().any(|t| t["name"] == "hidden"));

    // A rescan keeps the flags and a photo copied into a hidden folder is hidden at once.
    lib.jpeg("Familie 2/Neu.jpg", 77);
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    let neu = id_of(&lib, "Familie 2/Neu.jpg");
    assert!(!timeline_ids(addr, "").contains(&neu));
    assert!(timeline_ids(addr, "?hidden=1").contains(&neu));
    // Everything shown again.
    assert_eq!(hide_folder(addr, "Familie 2", false)["changed"], true);
    assert_eq!(hide(addr, &[img1], false)["files"], 1);
    assert_eq!(timeline_ids(addr, "?hidden=1"), Vec::<i64>::new());
    assert!(timeline_ids(addr, "").contains(&neu));
    assert!(timeline_ids(addr, "").contains(&img1));
    assert_eq!(folder_row(addr, "Familie 2")["hidden"], false);
    server.stop().unwrap();

    // Nothing was written next to an original: only what the moves and the new file changed.
    let after = lib.snapshot();
    assert!(after.len() >= before.len());
}

#[test]
fn a_folder_called_hidden_does_not_hide_its_photos_nor_the_ones_that_keep_its_tag() {
    let lib = Library::new("hidden-name");
    lib.scan();
    let server = start(&lib, None);
    let addr = server.addr;
    let img1 = id_of(&lib, "2020-07 Urlaub Griechenland/IMG_0001.JPG");
    let all = timeline_ids(addr, "");
    let r = post(addr, "/api/move", &json!({ "ids": [img1], "folder": "Hidden" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(timeline_ids(addr, ""), all, "a folder's name hides nothing, only the flag does");
    assert_eq!(folder_row(addr, "Hidden")["hidden"], false);
    // Moving out with the folder's tags kept: they become own tags, but "hidden" is not one of them.
    let r = post(addr, "/api/move", &json!({ "ids": [img1], "folder": "Sortiert", "keep_folder_tags": true }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(own_tags(&lib, img1).iter().all(|t| !t.eq_ignore_ascii_case("hidden")), "{:?}", own_tags(&lib, img1));
    assert_eq!(timeline_ids(addr, ""), all);
    server.stop().unwrap();
}
