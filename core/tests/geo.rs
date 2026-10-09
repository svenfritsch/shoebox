//! Integration tests for locations (phase 10): positions read from EXIF GPS,
//! positions the user gives, named places on the map, the maps setting, the
//! user data backup, and the guard around all of it.

mod common;

use std::fs;

use common::*;
use serde_json::{Value, json};

/// A JPEG with an EXIF GPS block for 48°7'30"N 11°30'E (48.125, 11.5).
fn jpeg_with_gps(lib: &Library, rel: &str, seed: u8) {
    lib.jpeg(rel, seed);
    let p = lib.path(rel);
    let bytes = fs::read(&p).unwrap();
    assert_eq!(&bytes[..2], [0xFF, 0xD8]);
    let be16 = |v: u16| v.to_be_bytes().to_vec();
    let be32 = |v: u32| v.to_be_bytes().to_vec();
    let entry = |tag: u16, typ: u16, count: u32, value: Vec<u8>| {
        let mut e = be16(tag);
        e.extend(be16(typ));
        e.extend(be32(count));
        let mut v = value;
        v.resize(4, 0);
        e.extend(v);
        e
    };
    let mut tiff = b"MM\0*".to_vec();
    tiff.extend(be32(8));
    // IFD0 at 8: one entry, the pointer to the GPS IFD at 26.
    tiff.extend(be16(1));
    tiff.extend(entry(0x8825, 4, 1, be32(26)));
    tiff.extend(be32(0));
    // GPS IFD at 26: 4 entries (54 bytes), values from 80.
    tiff.extend(be16(4));
    tiff.extend(entry(1, 2, 2, b"N\0".to_vec()));
    tiff.extend(entry(2, 5, 3, be32(80)));
    tiff.extend(entry(3, 2, 2, b"E\0".to_vec()));
    tiff.extend(entry(4, 5, 3, be32(104)));
    tiff.extend(be32(0));
    assert_eq!(tiff.len(), 80);
    for (n, d) in [(48u32, 1u32), (7, 1), (30, 1), (11, 1), (30, 1), (0, 1)] {
        tiff.extend(be32(n));
        tiff.extend(be32(d));
    }
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(be16(app1.len() as u16 + 2));
    out.extend(app1);
    out.extend(&bytes[2..]);
    fs::write(p, out).unwrap();
}

const GPS_PHOTO: &str = "Familie/Insel/GPS_0001.jpg";
const PLAIN: &str = "2020-07 Urlaub Griechenland/IMG_0001.JPG";

fn details(addr: std::net::SocketAddr, id: i64) -> Value {
    get(addr, &format!("/api/files/{id}")).json()
}

fn area_ids(addr: std::net::SocketAddr, q: &str) -> Vec<i64> {
    let r = get(addr, &format!("/api/timeline?{q}"));
    assert_eq!(r.status, 200, "{q}");
    ids(&r.json())
}

#[test]
fn positions_come_from_the_file_or_from_the_user() {
    let lib = Library::new("geo-positions");
    jpeg_with_gps(&lib, GPS_PHOTO, 21);
    lib.scan();
    let before = lib.snapshot();
    let gps = id_of(&lib, GPS_PHOTO);
    let plain = id_of(&lib, PLAIN);
    let (lat, lon): (f64, f64) = lib.db().query_row("SELECT lat, lon FROM files WHERE id = ?1", [gps], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!((lat - 48.125).abs() < 1e-6 && (lon - 11.5).abs() < 1e-6, "{lat} {lon}");
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE lat IS NOT NULL"), 1);

    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(details(addr, gps)["position"]["source"], "file");
    assert!(details(addr, plain)["position"].is_null());

    // Maps are a setting, off until asked for.
    assert_eq!(get(addr, "/api/maps").json()["on"], false);
    assert_eq!(post(addr, "/api/maps", &json!({ "on": true })).json()["on"], true);
    assert_eq!(get(addr, "/api/maps").json()["on"], true);

    // A photo with a position in its file keeps it.
    let r = post(addr, &format!("/api/files/{gps}/position"), &json!({ "lat": 1.0, "lon": 2.0 }));
    assert_eq!(r.status, 400);
    // Bad numbers are refused, nothing is stored.
    for (lat, lon) in [(91.0, 0.0), (-90.5, 0.0), (0.0, 180.5), (0.0, -181.0)] {
        let r = post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": lat, "lon": lon }));
        assert_eq!(r.status, 400, "{lat} {lon}");
    }
    assert_eq!(post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": 1.0 })).status, 400);
    assert_eq!(lib.count("SELECT count(*) FROM geo_overrides"), 0);

    // The user gives one; it is found, changeable, and not in the file.
    let r = post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": 37.97, "lon": 23.72 }));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let d = details(addr, plain);
    assert_eq!((d["position"]["source"].as_str(), d["position"]["lat"].as_f64()), (Some("user"), Some(37.97)));
    assert_eq!(area_ids(addr, "area=37,23,38,24"), [plain]);
    assert!(area_ids(addr, "area=40,23,41,24").is_empty());
    let r = post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": 38.5, "lon": 23.72 }));
    assert_eq!(r.status, 200);
    assert!(area_ids(addr, "area=37,23,38,24").is_empty());
    assert_eq!(area_ids(addr, "area=38,23,39,24"), [plain]);

    // Both are on the map.
    let pts = get(addr, "/api/geo/points").json();
    let mut got: Vec<i64> = pts["ids"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    got.sort();
    let mut want = vec![gps, plain];
    want.sort();
    assert_eq!(got, want);

    // Forgetting only drops the user's position.
    assert_eq!(post(addr, &format!("/api/files/{plain}/position"), &json!({ "clear": true })).status, 200);
    assert!(details(addr, plain)["position"].is_null());
    assert_eq!(post(addr, &format!("/api/files/{gps}/position"), &json!({ "clear": true })).status, 200);
    assert_eq!(details(addr, gps)["position"]["source"], "file");

    // Every change needs the header.
    let body = json!({ "lat": 1.0, "lon": 2.0 }).to_string();
    let r = bare_request(addr, "POST", &format!("/api/files/{plain}/position"), &[("Content-Type", "application/json")], body.as_bytes());
    assert_eq!(r.status, 403);

    server.stop().unwrap();
    assert_eq!(lib.snapshot(), before, "originals untouched");
}

#[test]
fn places_are_rectangles_that_collect_their_photos() {
    let lib = Library::new("geo-places");
    jpeg_with_gps(&lib, GPS_PHOTO, 21);
    lib.scan();
    let before = lib.snapshot();
    let gps = id_of(&lib, GPS_PHOTO);
    let plain = id_of(&lib, PLAIN);
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": 48.2, "lon": 11.6 })).status, 200);

    let create = |name: &str, s: f64, w: f64, n: f64, e: f64| post(addr, "/api/places", &json!({ "name": name, "south": s, "west": w, "north": n, "east": e }));
    let home = create(" Zuhause ", 48.0, 11.0, 48.5, 12.0);
    assert_eq!(home.status, 200, "{}", String::from_utf8_lossy(&home.body));
    let home_id = home.json()["id"].as_i64().unwrap();
    assert_eq!(home.json()["name"], "Zuhause");
    let island = create("Insel XY", 10.0, 10.0, 11.0, 11.0).json()["id"].as_i64().unwrap();

    // Bad names and areas.
    assert_eq!(create("zuhause", 1.0, 1.0, 2.0, 2.0).status, 400, "same name, other case");
    assert_eq!(create("   ", 1.0, 1.0, 2.0, 2.0).status, 400);
    assert_eq!(create("Flat", 2.0, 1.0, 2.0, 2.0).status, 400);
    assert_eq!(create("Backwards", 1.0, 3.0, 2.0, 2.0).status, 400);
    assert_eq!(create("Off", -91.0, 1.0, 2.0, 2.0).status, 400);

    // The photos inside are found without any bookkeeping.
    let list = get(addr, "/api/places").json();
    let counts: Vec<(&str, i64)> = list.as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap(), p["count"].as_i64().unwrap())).collect();
    assert_eq!(counts, [("Zuhause", 2), ("Insel XY", 0)]);
    let mut in_home = area_ids(addr, &format!("place={home_id}"));
    in_home.sort();
    let mut want = vec![gps, plain];
    want.sort();
    assert_eq!(in_home, want);
    assert!(area_ids(addr, &format!("place={island}")).is_empty());
    assert_eq!(get(addr, "/api/timeline?place=9999").status, 400);
    assert_eq!(get(addr, "/api/timeline?area=1,2,3").status, 400);

    // Rename, redraw (the new rectangle decides), delete.
    let r = post(addr, &format!("/api/places/{island}/rename"), &json!({ "name": "Zuhause" }));
    assert_eq!(r.status, 400, "name taken");
    assert_eq!(post(addr, &format!("/api/places/{island}/rename"), &json!({ "name": "Kreta" })).json()["name"], "Kreta");
    let r = post(addr, &format!("/api/places/{home_id}/area"), &json!({ "south": 48.15, "west": 11.0, "north": 48.5, "east": 12.0 }));
    assert_eq!(r.status, 200);
    assert_eq!(area_ids(addr, &format!("place={home_id}")), [plain]);
    assert_eq!(post(addr, &format!("/api/places/{home_id}/area"), &json!({ "south": 5.0, "west": 1.0, "north": 4.0, "east": 2.0 })).status, 400);
    assert_eq!(area_ids(addr, &format!("place={home_id}")), [plain], "a refused redraw changes nothing");
    assert_eq!(post(addr, &format!("/api/places/{island}/delete"), &json!({})).status, 200);
    assert_eq!(get(addr, "/api/places").json().as_array().unwrap().len(), 1);

    // The backup of what the user made.
    server.stop().unwrap();
    let data: Value = serde_json::from_slice(&fs::read(lib.path(".shoebox/userdata.json")).unwrap()).unwrap();
    assert_eq!(data["version"], 5);
    assert_eq!(data["places"][0]["name"], "Zuhause");
    assert_eq!(data["geo_overrides"][0]["files"][0], PLAIN);
    assert_eq!(lib.snapshot(), before, "originals untouched");
}

#[test]
fn an_older_index_reads_positions_with_the_next_scan() {
    let lib = Library::new("geo-reread");
    jpeg_with_gps(&lib, GPS_PHOTO, 21);
    lib.scan();
    // As after the update from schema 9: columns empty, "not read yet".
    lib.db().execute("UPDATE files SET lat = NULL, lon = NULL, geo_done = 0", []).unwrap();
    let before = lib.snapshot();
    let stats = lib.scan();
    assert_eq!(stats.added, 0);
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE lat IS NOT NULL"), 1);
    assert_eq!(lib.count("SELECT count(*) FROM files WHERE geo_done = 0"), 0);
    assert_eq!(lib.snapshot(), before, "originals untouched");
}

#[test]
fn a_place_combines_with_the_other_filters() {
    let lib = Library::new("geo-combine");
    jpeg_with_gps(&lib, GPS_PHOTO, 21);
    lib.scan();
    let gps = id_of(&lib, GPS_PHOTO);
    let plain = id_of(&lib, PLAIN);
    let server = start(&lib, None);
    let addr = server.addr;
    assert_eq!(post(addr, &format!("/api/files/{plain}/position"), &json!({ "lat": 48.2, "lon": 11.6 })).status, 200);
    let home = post(addr, "/api/places", &json!({ "name": "Zuhause", "south": 48.0, "west": 11.0, "north": 48.5, "east": 12.0 })).json()["id"].as_i64().unwrap();
    // Folder tags of the photos in the place, and the tags found when the place is part of the search.
    let tags = |q: &str| -> Vec<String> {
        get(addr, &format!("/api/tags?{q}")).json().as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    };
    assert!(tags("").contains(&"Weihnachten".to_string()), "all tags without a filter");
    let inside = tags(&format!("place={home}"));
    assert!(inside.contains(&"Insel".to_string()) && inside.contains(&"Urlaub Griechenland".to_string()), "{inside:?}");
    assert!(!inside.contains(&"Weihnachten".to_string()), "a tag of photos outside the place: {inside:?}");
    // AND with a tag.
    let insel = get(addr, "/api/tags?q=Insel").json()[0]["id"].as_i64().unwrap();
    assert_eq!(area_ids(addr, &format!("place={home}&tag={insel}")), [gps]);
    assert_eq!(area_ids(addr, &format!("place={home}&q=Griechenland")), [plain]);
    // An unknown place is the client's mistake, wherever it is asked about.
    for path in ["tags", "people/search", "pets/search", "timeline"] {
        assert_eq!(get(addr, &format!("/api/{path}?place=9999")).status, 400, "{path}");
    }
    server.stop().unwrap();
}
