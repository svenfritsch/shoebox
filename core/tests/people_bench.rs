//! Where the time of the people endpoints goes, on a large library made by
//! `scripts/bench-people.py make` (not part of the normal test run):
//!
//!     SHOEBOX_BENCH_LIBRARY=<dir>/work cargo test --release --test people_bench -- --ignored --nocapture
//!
//! Works on a copy: `decide` and `ensure_covers` write to it.

use std::path::PathBuf;
use std::time::Instant;

use rusqlite::Connection;
use shoebox::{people, recognize};

fn library() -> Option<PathBuf> {
    std::env::var_os("SHOEBOX_BENCH_LIBRARY").map(PathBuf::from)
}

fn conn(root: &std::path::Path) -> Connection {
    let path = root.join(".shoebox/library.db");
    let conn = shoebox::db::open_shared(&path).unwrap();
    shoebox::thumbs::attach(&conn, &path).unwrap();
    recognize::attach(&conn, &path).unwrap();
    conn
}

fn time<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{what:48} {:8.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

fn count(conn: &Connection, sql: &str) -> i64 {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut n = 0;
    while rows.next().unwrap().is_some() {
        n += 1;
    }
    n
}

#[test]
#[ignore]
fn where_the_time_goes() {
    let Some(root) = library() else { return };
    let conn = conn(&root);
    println!("rows read by View::load:");
    time("  recog.faces with looked (Matched::load)", || {
        count(
            &conn,
            "SELECT f.id, f.key, f.x, f.y, f.w, f.h, f.roll, f.w * l.width, f.score, f.model, f.species FROM recog.faces f
             LEFT JOIN recog.looked l ON l.key = f.key AND l.task = CASE WHEN f.species IS NULL THEN 'faces' ELSE 'pets' END ORDER BY f.id",
        )
    });
    time("  face_decisions", || count(&conn, "SELECT * FROM face_decisions ORDER BY id"));
    time("  present files", || {
        count(&conn, "SELECT quick_hash, min(id), kind FROM files WHERE missing_since IS NULL AND kind IN ('jpeg', 'png', 'heic') GROUP BY quick_hash")
    });
    time("  recog.clusters", || count(&conn, "SELECT face, cluster, person, similarity FROM recog.clusters"));
    time("  recog.looked sizes", || count(&conn, "SELECT key, width, height FROM recog.looked WHERE task = 'faces' AND width IS NOT NULL"));
    println!("whole loads:");
    time("  Matched::load", || people::Matched::load(&conn, None).unwrap());
    let v = time("  View::load", || people::View::load(&conn).unwrap());
    let m = &v.m;
    time("  Matched::new (matching in memory)", || people::Matched::new((*m.faces).clone(), m.decisions.clone()));
    time("  pet_faces (in memory)", || m.pet_faces());
    time("  person_species", || people::person_species(&conn).unwrap());
    drop(v);
    println!("endpoints:");
    let list = time("  people::people", || people::people(&conn, false).unwrap());
    time("  serialize people", || serde_json::to_vec(&list).unwrap().len());
    let large = time("  people::clusters large 30×8", || {
        people::clusters(&conn, 0, 30, 8, Some(shoebox::pets::Space::Faces), people::Size::Large).unwrap()
    });
    time("  serialize clusters", || serde_json::to_vec(&large).unwrap().len());
    time("  people::clusters small 60×2", || {
        people::clusters(&conn, 0, 60, 2, Some(shoebox::pets::Space::Faces), people::Size::Small).unwrap()
    });
    let c = &large.clusters[0];
    time("  people::cluster_faces", || people::cluster_faces(&conn, c.id, Some(c.generation)).unwrap());
    time("  people::person", || people::person(&conn, list[0].id).unwrap());
    time("  people::person_faces", || people::person_faces(&conn, list[0].id, people::Which::Confirmed, 0, 300).unwrap());
    println!("with one view kept (what the photo app does now):");
    let v = time("  View::load", || people::View::load(&conn).unwrap());
    time("  people_in (first: counts)", || people::people_in(&conn, &v, false).unwrap());
    time("  people_in", || people::people_in(&conn, &v, false).unwrap());
    time("  clusters_in large (first: groups the faces)", || {
        people::clusters_in(&v, 0, 30, 8, Some(shoebox::pets::Space::Faces), people::Size::Large)
    });
    time("  clusters_in small", || people::clusters_in(&v, 0, 60, 2, Some(shoebox::pets::Space::Faces), people::Size::Small));
    let c0 = people::clusters_in(&v, 0, 30, 8, Some(shoebox::pets::Space::Faces), people::Size::Large).clusters.remove(0);
    time("  cluster_faces_in", || people::cluster_faces_in(&v, c0.id, Some(c0.generation)).unwrap());
    time("  person_faces_in", || people::person_faces_in(&conn, &v, list[0].id, people::Which::Confirmed, 0, 300).unwrap());
    let t = Instant::now();
    let all = people::cluster_face_ids_in(&v, c0.id, Some(c0.generation), None).unwrap();
    let who = people::Who { person_id: None, name: Some("Bench cached".into()) };
    time("  decide_in", || people::decide_in(&conn, &v, &all, &people::Action::Assign(who)).unwrap());
    let v2 = time("  refresh (library.db part)", || v.refresh(&conn).unwrap().unwrap());
    time("  ensure_covers_in", || people::ensure_covers_in(&conn, &v2).unwrap());
    time("  prune_crops_in", || shoebox::faces::prune_crops_in(&conn, &v2.m).unwrap());
    time("  clusters_in large (new view)", || {
        people::clusters_in(&v2, 0, 30, 8, Some(shoebox::pets::Space::Faces), people::Size::Large)
    });
    println!("  {:46} {:8.1} ms", "naming, all of it", t.elapsed().as_secs_f64() * 1000.0);
    time("  drop a view", || drop(v));
    drop(v2);
    println!("one photo (/api/files/{{id}}: people::file_faces):");
    let key: String = conn.query_row("SELECT key FROM face_decisions ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    time("  file_faces (first)", || people::file_faces(&conn, &key, false).unwrap());
    time("  file_faces", || people::file_faces(&conn, &key, false).unwrap());
    println!("the backup after a change (App::backup, at most once a minute):");
    let path = root.join(".shoebox/library.db");
    time("  library.db.bak", || shoebox::db::backup(&conn, &path).unwrap());
    time("  userdata.json", || shoebox::tags::write_user_data(&conn, &path).unwrap());
    println!("naming a cluster (/api/clusters/{{id}}/name), as before:");
    let large = people::clusters(&conn, 0, 30, 8, Some(shoebox::pets::Space::Faces), people::Size::Large).unwrap();
    let c = &large.clusters[0];
    let t = Instant::now();
    let all = time("  cluster_face_ids (all)", || people::cluster_face_ids(&conn, c.id, Some(c.generation), None).unwrap());
    let faces = time("  cluster_face_ids (chosen)", || people::cluster_face_ids(&conn, c.id, Some(c.generation), None).unwrap());
    let who = people::Who { person_id: None, name: Some("Bench timing".into()) };
    time("  decide", || people::decide(&conn, &faces, &people::Action::Assign(who)).unwrap());
    let left: Vec<i64> = all.into_iter().filter(|f| !faces.contains(f)).collect();
    if !left.is_empty() {
        time("  cluster_of", || people::cluster_of(&conn, &left).unwrap());
    }
    time("  ensure_covers", || people::ensure_covers(&conn).unwrap());
    time("  prune_crops", || shoebox::faces::prune_crops(&conn).unwrap());
    println!("  {:46} {:8.1} ms", "all of it", t.elapsed().as_secs_f64() * 1000.0);
}

/// Where a regrouping's time goes, phase by phase (`clusters::compute`), on
/// the same library: a first run (every suggestion searched), then after 10
/// cards named, a rejection, and an undo of an older name (only what changed):
///
///     SHOEBOX_CLUSTER_TIMES=1 SHOEBOX_BENCH_LIBRARY=<dir>/work cargo test --release --test people_bench regrouping_phases -- --ignored --nocapture
///
/// `ann::with_threads` as the photo app runs it (one core left free) with
/// `SHOEBOX_BENCH_THREADS=<n>`. Works on the copy: it names faces.
#[test]
#[ignore]
fn regrouping_phases() {
    let Some(root) = library() else { return };
    let conn = conn(&root);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = std::env::var("SHOEBOX_BENCH_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(cores);
    let run = |what: &str| {
        println!("{what}:");
        let t = Instant::now();
        let s = shoebox::ann::with_threads(threads, || shoebox::clusters::run(&conn, &|| false, &mut |_, _| {}).unwrap());
        println!(
            "{what} on {threads} threads: {:.1} s, {} without a decision in {} clusters, {} suggested; {} searched, {} compared",
            t.elapsed().as_secs_f64(),
            s.unnamed,
            s.clusters,
            s.suggested,
            s.searched,
            s.compared
        );
    };
    run("first run");
    let v = people::View::load(&conn).unwrap();
    let cards = people::clusters_in(&v, 0, 10, 1, Some(shoebox::pets::Space::Faces), people::Size::Large).clusters;
    let mut first = None;
    for (k, c) in cards.iter().enumerate() {
        let faces = people::cluster_face_ids_in(&v, c.id, Some(c.generation), None).unwrap();
        let who = people::Who { person_id: None, name: Some(format!("Bench card {k}")) };
        people::decide(&conn, &faces, &people::Action::Assign(who)).unwrap();
        first.get_or_insert(faces);
    }
    drop(v);
    run("after 10 cards named");
    let v = people::View::load(&conn).unwrap();
    let open = people::clusters_in(&v, 0, 1, 1, Some(shoebox::pets::Space::Faces), people::Size::Large).clusters.remove(0);
    let face = open.faces[0].id.unwrap();
    drop(v);
    people::decide(&conn, &[face], &people::Action::Reject(Some(1))).unwrap();
    run("after a rejection");
    people::decide(&conn, &first.unwrap()[..1], &people::Action::Undo).unwrap();
    run("after an older name undone");
}
