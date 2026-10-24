//! The search box's one request (`/api/suggest`): every section in one reply,
//! each counted within the rest of the search so that a number is what the
//! list shows once it is picked, the prefixes read by the server, and the
//! counts it keeps between letters following every change. Only reading.
//! (Each section against its own endpoint: people.rs, dates.rs,
//! text_search.rs; over all drives: multi.rs.)

mod common;

use common::*;
use serde_json::{Value, json};

const KEYS: [&str; 7] = ["tags", "people", "pets", "groups", "dates", "text", "name"];

/// The sample library with own tags, a heart, people (one hidden), groups and
/// a cat, written straight into its index (no recognizer needed).
fn library(test: &str) -> Library {
    let lib = Library::new(test);
    lib.scan();
    let conn = lib.db();
    let id = |p: &str| id_of(&lib, p);
    let (g1, g2) = (id("2020-07 Urlaub Griechenland/IMG_0001.JPG"), id("2020-07 Urlaub Griechenland/IMG_0002.JPG"));
    let (xmas, bild) = (id("Familie/Weihnachten/DSC_2001.jpg"), id("Ordner mit Leerzeichen/Bild 1.jpeg"));
    for (ids, name) in [(vec![g1, g2], "Strand"), (vec![g1], "Sonne"), (vec![xmas], "Schnee"), (vec![g1], "favorite")] {
        shoebox::tags::add(&conn, &ids, name).unwrap();
    }
    // Ten tags with the same word, on one photo each: the list shows eight.
    for i in 0..10 {
        shoebox::tags::add(&conn, &[[g1, g2, xmas, bild][i % 4]], &format!("Serie {i}")).unwrap();
    }
    let face = |person: &str, path: &str, n: i64| {
        conn.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at)
             VALUES (?1, 0.1 * ?3, 0.1, 0.05, 0.05, (SELECT id FROM people WHERE name = ?2), 'confirmed', 1, 1)",
            rusqlite::params![lib.record_key(path), person, n],
        )
        .unwrap();
    };
    for (name, hidden) in [("Anna", 0), ("Ben", 0), ("Carla Hidden", 1)] {
        conn.execute("INSERT INTO people (name, hidden) VALUES (?1, ?2)", rusqlite::params![name, hidden]).unwrap();
    }
    face("Anna", "2020-07 Urlaub Griechenland/IMG_0001.JPG", 1);
    face("Anna", "Familie/Weihnachten/DSC_2001.jpg", 1);
    face("Ben", "2020-07 Urlaub Griechenland/IMG_0002.JPG", 1);
    face("Carla Hidden", "2020-07 Urlaub Griechenland/IMG_0001.JPG", 2);
    // Eight people with the same word: the list shows six; each in a group of
    // its own ("Gruppe …", four of five are listed).
    for i in 0..8 {
        conn.execute("INSERT INTO people (name) VALUES (?1)", [format!("Person {i}")]).unwrap();
        face(&format!("Person {i}"), "Ordner mit Leerzeichen/Bild 1.jpeg", i + 1);
    }
    conn.execute("INSERT INTO groups (name, position) VALUES ('Familie Gruppe', 0)", []).unwrap();
    conn.execute("UPDATE people SET group_id = (SELECT id FROM groups WHERE name = 'Familie Gruppe') WHERE name IN ('Anna', 'Ben')", []).unwrap();
    for i in 0..5 {
        conn.execute("INSERT INTO groups (name, position) VALUES (?1, ?2)", rusqlite::params![format!("Gruppe {i}"), i + 1]).unwrap();
        conn.execute(
            "UPDATE people SET group_id = (SELECT id FROM groups WHERE name = ?1) WHERE name = ?2",
            [format!("Gruppe {i}"), format!("Person {i}")],
        )
        .unwrap();
    }
    // A cat on the screenshot, drawn by hand.
    conn.execute(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at, species)
         VALUES (?1, 0.2, 0.2, 0.5, 0.5, NULL, 'confirmed', 1, 1, 'cat')",
        [lib.record_key("Familie/Screenshot.png")],
    )
    .unwrap();
    lib
}

fn suggest(addr: std::net::SocketAddr, query: &str) -> Value {
    let r = get(addr, &format!("/api/suggest?{query}"));
    assert_eq!(r.status, 200, "{query}: {}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn names(list: &Value, key: &str) -> Vec<String> {
    list.as_array().unwrap().iter().map(|x| x[key].as_str().unwrap().to_string()).collect()
}

fn count(addr: std::net::SocketAddr, query: &str) -> u64 {
    get(addr, &format!("/api/timeline?{query}")).json()["count"].as_u64().unwrap()
}

#[test]
fn every_section_in_one_reply() {
    let lib = library("suggest-every-section");
    let before = lib.snapshot();
    let server = start(&lib, None);
    let addr = server.addr;

    // Always the same keys, whatever is typed.
    for q in ["q=", "q=s", "q=date:2020", "q=text:x", "q=name:img", "q=%C3%A4%C3%B6%C3%BC"] {
        let s = suggest(addr, q);
        let mut keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        let mut want = KEYS.to_vec();
        want.sort();
        assert_eq!(keys, want, "{q}");
    }

    // Own tags only (a folder's name is offered as a folder by the page),
    // most photos first, eight at most; the heart is a tag like the others
    // (the page shows it as ♥).
    let s = suggest(addr, "q=s");
    let tags = names(&s["tags"], "name");
    assert_eq!(tags.len(), 8, "{tags:?}");
    assert_eq!(tags[0], "Strand", "two photos: first");
    assert!(!tags.iter().any(|t| t.contains("Urlaub") || t == "Familie"), "no folder tags: {tags:?}");
    assert!(names(&suggest(addr, "q=urlaub")["tags"], "name").is_empty(), "only a folder has that name");
    assert_eq!(names(&suggest(addr, "q=SERIE+3")["tags"], "name"), ["Serie 3"], "the case does not matter");

    // People by name: hidden ones left out, six at most, most photos first.
    let people = names(&suggest(addr, "q=")["people"], "name");
    assert_eq!(people.len(), 6, "{people:?}");
    assert_eq!(people[0], "Anna");
    assert!(names(&suggest(addr, "q=carla")["people"], "name").is_empty(), "a hidden person is not offered");
    assert_eq!(suggest(addr, "q=anna")["people"][0]["photos"], 2);
    // Groups by name, four at most.
    assert_eq!(names(&suggest(addr, "q=gruppe")["groups"], "name").len(), 4);
    assert_eq!(suggest(addr, "q=familie+g")["groups"][0]["name"], "Familie Gruppe");
    assert_eq!(suggest(addr, "q=familie+g")["groups"][0]["photos"], 3, "Anna's two and Ben's one");
    // Pets by the words for them, in English and German.
    for q in ["q=cat", "q=katze", "q=pets"] {
        let pets = &suggest(addr, q)["pets"];
        assert_eq!(pets.as_array().unwrap().len(), 1, "{q}: {pets}");
        assert_eq!(pets[0]["photos"], 1, "{q}");
    }
    // Dates: a year that is typed (the event folder's July 2020).
    let dates = suggest(addr, "q=2020")["dates"].clone();
    assert_eq!((dates[0]["year"].as_i64(), dates[0]["month"].as_i64()), (Some(2020), None), "{dates}");
    assert!(suggest(addr, "q=july")["dates"].as_array().unwrap().is_empty(), "a month alone needs date:");
    // Nothing matches: every list empty; the text row is still counted (0).
    let s = suggest(addr, "q=zzzz");
    for k in ["tags", "people", "pets", "groups", "dates"] {
        assert_eq!(s[k], json!([]), "{k}");
    }
    assert_eq!(s["text"], json!({ "term": "zzzz", "count": 0 }));
    assert!(suggest(addr, "q=z")["text"].is_null(), "one letter has no text row");

    // The prefixes, as the page reads them (any case, spaces around them).
    let only = |s: &Value, key: &str| KEYS.iter().filter(|k| **k != key).all(|k| s[*k] == json!([]) || s[*k].is_null());
    for q in ["q=date:2020", "q=+Datum:+2020", "q=DATE:july", "q=date:"] {
        let s = suggest(addr, q);
        assert!(only(&s, "dates") && !s["dates"].as_array().unwrap().is_empty(), "{q}: {s}");
    }
    let s = suggest(addr, "q=name:img_000");
    assert!(only(&s, "name"), "{s}");
    // As many as the list then shows (with SHOEBOX_FIXTURES the library has more such files).
    assert_eq!(s["name"]["term"], "img_000");
    assert!(s["name"]["count"].as_u64().unwrap() >= 2, "{s}");
    assert_eq!(s["name"]["count"].as_u64(), Some(count(addr, "name=img_000")));
    assert_eq!(suggest(addr, "q=Name:+bild")["name"]["count"].as_u64(), Some(count(addr, "name=bild")));
    assert!(suggest(addr, "q=Name:+bild")["name"]["count"].as_u64().unwrap() >= 1);
    let s = suggest(addr, "q=text:");
    assert!(KEYS.iter().all(|k| s[*k] == json!([]) || s[*k].is_null()), "nothing after the prefix: {s}");
    // Not a prefix: a word with a colon later on, or another word before it.
    assert!(suggest(addr, "q=strand:x")["text"]["term"] == "strand:x");
    assert!(suggest(addr, "q=photo+date:2020")["dates"].as_array().unwrap().is_empty());

    // A filter's own terms are not offered again.
    let anna = suggest(addr, "q=anna")["people"][0]["id"].as_i64().unwrap();
    assert!(names(&suggest(addr, &format!("q=anna&person={anna}"))["people"], "name").is_empty());
    let family = suggest(addr, "q=familie+g")["groups"][0]["id"].as_i64().unwrap();
    assert!(names(&suggest(addr, &format!("q=familie&group={family}"))["groups"], "name").is_empty());
    assert!(suggest(addr, "q=cat&pet=cat")["pets"].as_array().unwrap().is_empty());
    // A broken parameter is refused, as by the timeline.
    assert_eq!(get(addr, "/api/suggest?q=a&tag=x").status, 400);
    assert_eq!(get(addr, "/api/suggest?q=a&date=87").status, 400);

    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "suggesting changed an original");
}

/// Every number in the reply is what the list shows once that suggestion is
/// picked, within every kind of chip (dates, groups, type, heart too).
#[test]
fn every_number_is_what_the_list_then_shows() {
    let lib = library("suggest-every-number");
    let server = start(&lib, None);
    let addr = server.addr;
    let first = |q: &str, key: &str| suggest(addr, q)[key][0]["id"].as_i64().unwrap();
    let (anna, strand) = (first("q=anna", "people"), first("q=strand", "tags"));
    let family = first("q=familie+g", "groups");
    let filters = [
        String::new(),
        format!("&tag={strand}"),
        format!("&person={anna}"),
        format!("&group={family}"),
        "&date=2020".to_string(),
        "&nodate=1".to_string(),
        "&type=photo".to_string(),
        "&type=screenshot".to_string(),
        "&fav=1".to_string(),
        "&name=img".to_string(),
        format!("&date=2020&person={anna}"),
    ];
    let mut checked = 0;
    for filter in &filters {
        for typed in ["", "s", "a", "familie", "gruppe", "person", "cat", "2020", "img"] {
            let s = suggest(addr, &format!("q={typed}{filter}"));
            let mut picked = |key: &str, param: &str, value: String, n: &Value| {
                let n = n.as_u64().unwrap();
                assert!(n > 0, "{key} {value} with nothing to show, within {filter:?}");
                assert_eq!(count(addr, &format!("{param}={value}{filter}")), n, "{key} {value} within {filter:?}");
                checked += 1;
            };
            for t in s["tags"].as_array().unwrap() {
                picked("tag", "tag", t["id"].to_string(), &t["count"]);
            }
            for p in s["people"].as_array().unwrap() {
                picked("person", "person", p["id"].to_string(), &p["photos"]);
            }
            for g in s["groups"].as_array().unwrap() {
                picked("group", "group", g["id"].to_string(), &g["photos"]);
            }
            for p in s["pets"].as_array().unwrap() {
                picked("pet", "pet", p["species"].as_str().unwrap().to_string(), &p["photos"]);
            }
            for d in s["dates"].as_array().unwrap() {
                let value = match (d["month"].as_u64(), d["day"].as_u64()) {
                    (Some(m), Some(day)) => format!("{}-{m:02}-{day:02}", d["year"]),
                    (Some(m), None) => format!("{}-{m:02}", d["year"]),
                    _ => d["year"].to_string(),
                };
                picked("date", "date", value, &d["count"]);
            }
            let name = suggest(addr, &format!("q=name:{typed}{filter}"));
            if let Some(n) = name["name"]["count"].as_u64() {
                assert_eq!(count(addr, &format!("name={typed}{filter}")), n, "name:{typed} within {filter:?}");
            }
        }
    }
    assert!(checked > 100, "only {checked} numbers checked");
    server.stop().unwrap();
}

/// The server keeps the tags' and people's counts between letters; a change
/// in between counts at once (an own tag, a name, a hidden person).
#[test]
fn kept_counts_follow_every_change() {
    let lib = library("suggest-kept-counts");
    let server = start(&lib, None);
    let addr = server.addr;
    let photos_of = |q: &str, key: &str, name: &str| -> Option<u64> {
        suggest(addr, q)[key].as_array().unwrap().iter().find(|x| x["name"] == name).map(|x| x[if key == "tags" { "count" } else { "photos" }].as_u64().unwrap())
    };
    assert_eq!(photos_of("q=sonne", "tags", "Sonne"), Some(1));
    let id = |p: &str| id_of(&lib, p);
    let r = post(addr, "/api/tags/add", &json!({ "ids": [id("Familie/Weihnachten/DSC_2001.jpg")], "name": "Sonne" }));
    assert_eq!(r.status, 200);
    assert_eq!(photos_of("q=sonne", "tags", "Sonne"), Some(2), "a tag given in between");
    post(addr, "/api/tags/remove", &json!({ "ids": [id("2020-07 Urlaub Griechenland/IMG_0001.JPG")], "name": "Sonne" }));
    assert_eq!(photos_of("q=sonn", "tags", "Sonne"), Some(1), "a tag taken off in between");

    assert_eq!(photos_of("q=ben", "people", "Ben"), Some(1));
    let ben = suggest(addr, "q=ben")["people"][0]["id"].as_i64().unwrap();
    let r = post(addr, &format!("/api/people/{ben}/rename"), &json!({ "name": "Benno" }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(photos_of("q=ben", "people", "Benno"), Some(1), "renamed in between");
    let r = post(addr, &format!("/api/people/{ben}/hide"), &json!({ "hidden": true }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(photos_of("q=ben", "people", "Benno"), None, "hidden in between");
    // The groups' and pets' counts are kept the same way: Ben leaving his group
    // and a dog drawn on a photo count at once, with the same filter.
    let group_photos = |q: &str| suggest(addr, q)["groups"].as_array().unwrap().iter().find(|g| g["name"] == "Familie Gruppe").map(|g| g["photos"].as_u64().unwrap());
    assert_eq!(group_photos("q=familie+g"), Some(3), "Anna's two and Ben's one");
    let r = post(addr, &format!("/api/people/{ben}/group"), &json!({ "group_id": null }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(group_photos("q=familie+g"), Some(2), "Ben left his group in between");
    assert_eq!(suggest(addr, "q=dog")["pets"], json!([]));
    lib.db()
        .execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at, species)
             VALUES (?1, 0.2, 0.2, 0.5, 0.5, NULL, 'confirmed', 1, 1, 'dog')",
            [lib.record_key("Ordner mit Leerzeichen/Bild 1.jpeg")],
        )
        .unwrap();
    assert_eq!(suggest(addr, "q=dog")["pets"], json!([{ "species": "dog", "photos": 1 }]), "a dog drawn in between");
    assert_eq!(suggest(addr, "q=pets")["pets"][0]["photos"], 2, "any pet: the cat and the dog");
    server.stop().unwrap();
}
