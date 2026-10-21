//! Clusters and suggestions (phase 5c-2): a cache in `recognition.db`,
//! recomputed from the faces and the user's decisions after every
//! `shoebox recognize` run and in the background in `shoebox serve`.
//!
//! - Only faces of present photos take part, found with the current model
//!   (the one of the latest upright result), at least
//!   `faces::MIN_CLUSTER_PX` wide. Faces found by the rotated pass take
//!   part like the others.
//! - Cats and dogs (`pets.rs`) are another **space**: their embeddings
//!   come from another model, so they are neighbours, clusters and
//!   suggestions among themselves only, with thresholds of their own
//!   (`Space::thresholds`) and at least `pets::MIN_CLUSTER_PX` wide.
//!   Everything below is done once per space; a person is suggested for
//!   faces of a space only through their confirmed faces in that space. The
//!   clusters of the pets are numbered after those of the faces.
//! - Neighbours, not all pairs: every such face looks up its nearest
//!   neighbours (`ann::Index`, similarity ≥ `CLUSTER_SIM`) once; the lists
//!   are kept in `recog.neighbours`, so a run that is stopped resumes, and
//!   after a `recognize` run only the new faces need theirs.
//! - Clusters, from scratch every time: faces without a decision that are
//!   neighbours end up in one cluster (`join`, the closest pairs first).
//!   Faces with a decision (confirmed, ignored, not a face) never take part.
//!   Two exceptions: faces suggested for two different people are never
//!   joined, and two parts are joined only when they are similar on average
//!   (`Thresholds::cohesion`), so look-alikes do not chain into one big card.
//! - A cluster of more than `MAX_CLUSTER` faces is split with a stricter
//!   similarity (0.62, 0.64, …) until every piece fits.
//! - Suggestions, before the clusters: every face without a decision is
//!   compared with the confirmed faces of every person (best match, so
//!   confirmed faces from several ages bridge the gap a single reference
//!   cannot), never with a person it was rejected for: from
//!   `people::SUGGEST_SIM` suggested, from `people::MAYBE_SIM` offered as
//!   "maybe". Faces drawn by hand count as confirmed faces when the worker
//!   found landmarks in their box (`recog.drawn.aligned`); a plain crop's
//!   embedding is too unreliable.
//!
//! Nothing here is user data: deleting `recognition.db` loses no decision.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::pets::{Space, Thresholds};
use crate::ann::{self, Index};
use crate::db::{self, Job};
use crate::faces;
use crate::people::{Decision, Matched};
use crate::recognize::{Interrupted, KINDS};

/// Faces at least this similar (cosine) are neighbours, and neighbours
/// end up in one cluster. Stricter than `people::SUGGEST_SIM`: a cluster
/// grows from neighbour to neighbour, and on the real drive everything at
/// 0.60 and above was the same person.
pub const CLUSTER_SIM: f32 = 0.60;
/// … and two parts of a cluster of faces are joined only when they are at
/// least this similar on average (`Thresholds::cohesion`), so a chain of
/// look-alikes (a mother, her children, other fair children) does not end up
/// on one card. The suggest similarity: on the real drive the highest wrong
/// match seen was 0.54. `SHOEBOX_FACE_COHESION` sets another (a number, or
/// `off` for the chains of before), to compare on a real library.
pub const COHESION_SIM: f32 = 0.55;
/// No card holds more faces than this: nobody can look through 2,500 faces
/// to find the odd one out. A larger cluster (one that chained from
/// neighbour to neighbour, say a mother, her children and other fair
/// children) is split by raising the similarity in `SPLIT_STEP`s until its
/// pieces fit; faces that stay joined even at 1.0 are cut into pieces of
/// this size.
pub const MAX_CLUSTER: usize = 100;
const SPLIT_STEP: f32 = 0.02;
/// Neighbours kept per face.
pub const NEIGHBOURS: usize = 24;
/// Confirmed faces looked at per face for its suggestion (the most similar).
const REFERENCES: usize = 32;
/// Neighbour lists computed (and committed) at a time.
const CHUNK: usize = 500;
/// Suggestions searched between two reports of progress.
const SEARCH_CHUNK: usize = 2000;
/// A `running` clustering job that has not reported progress for this long
/// is dead.
const JOB_ALIVE_SECS: i64 = 120;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    /// Faces taking part (large enough, current model, present photos).
    pub faces: u64,
    /// The same for the cats and dogs, if any were looked at: its own space,
    /// with its own thresholds. The numbers above are the faces' only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pets: Option<Box<Summary>>,
    /// Neighbour lists computed this time (the others were kept).
    pub listed: u64,
    /// Of `faces`, without a decision; and the clusters they form.
    pub unnamed: u64,
    pub clusters: u64,
    /// Faces suggested for someone, and offered as "maybe".
    pub suggested: u64,
    pub maybe: u64,
    /// Of `unnamed`: faces whose suggestion was searched among all confirmed
    /// faces, and those only compared with the faces confirmed since the last
    /// run (their earlier suggestion kept otherwise).
    pub searched: u64,
    pub compared: u64,
    /// The clusters by size, to judge the thresholds on a real library.
    pub sizes: Sizes,
    pub seconds: f64,
}

/// Clusters of 1, 2, 3–5, 6–20 and more faces.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sizes {
    pub one: u64,
    pub two: u64,
    pub three_to_five: u64,
    pub six_to_twenty: u64,
    pub more: u64,
}

impl Sizes {
    fn add(&mut self, len: usize) {
        match len {
            0 => {}
            1 => self.one += 1,
            2 => self.two += 1,
            3..=5 => self.three_to_five += 1,
            6..=20 => self.six_to_twenty += 1,
            _ => self.more += 1,
        }
    }
}

impl std::fmt::Display for Sizes {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "of 1 face {}, 2 faces {}, 3-5 {}, 6-20 {}, more {}",
            self.one, self.two, self.three_to_five, self.six_to_twenty, self.more
        )
    }
}

/// `COHESION_SIM`, or what `SHOEBOX_FACE_COHESION` says (`off`: none).
fn face_cohesion(default: Option<f32>) -> Option<f32> {
    match std::env::var("SHOEBOX_FACE_COHESION") {
        Ok(v) if v.trim() == "off" => None,
        Ok(v) => v.trim().parse().ok().or(default),
        Err(_) => default,
    }
}

/// Whether a clustering job is running right now (in any process).
pub fn running(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM recog.jobs WHERE kind = 'clusters' AND state = 'running' AND updated_at > ?1",
        [db::now() - JOB_ALIVE_SECS],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The model of the latest upright result: embeddings of other models
/// cannot be compared with its.
pub fn current_model(conn: &Connection) -> Result<Option<String>> {
    current_model_of(conn, Space::Faces)
}

/// The model of the latest result of a space's pass.
pub fn current_model_of(conn: &Connection, space: Space) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT model FROM recog.looked WHERE task = ?1 AND error IS NULL ORDER BY done_at DESC LIMIT 1",
            [space.task()],
            |r| r.get(0),
        )
        .optional()?)
}

/// `run`, printing progress and the result (for `shoebox recognize`).
pub fn run_printing(conn: &Connection, stop: &dyn Fn() -> bool) -> Result<Summary> {
    let mut progress = crate::scan::Progress::new(0);
    let mut last = 0;
    let s = run(conn, stop, &mut |done, total| {
        if last == 0 {
            println!("Clusters: neighbours and suggestions, {total} steps…");
            progress = crate::scan::Progress::new(total);
        }
        progress.add(done - last, |done, total| format!("clusters {done}/{total}"));
        last = done;
    })?;
    println!(
        "Clusters: {} faces large enough, {} without a decision in {} clusters; {} suggested, {} maybe ({:.1} s).",
        s.faces, s.unnamed, s.clusters, s.suggested, s.maybe, s.seconds
    );
    println!("Clusters of faces: {}.", s.sizes);
    if let Some(a) = &s.pets {
        println!(
            "Clusters: {} cats and dogs large enough, {} without a decision in {} clusters; {} suggested, {} maybe.",
            a.faces, a.unnamed, a.clusters, a.suggested, a.maybe
        );
        println!("Clusters of pets: {}.", a.sizes);
    }
    Ok(s)
}

/// Recompute the clusters and suggestions; resumes neighbour lists a
/// stopped run left. `stop` is asked between chunks (Ctrl-C, the server
/// stopping); `progress` gets (done, total) steps: neighbour lists computed
/// and faces whose suggestion was searched (nearly all of the time once the
/// lists are there). `conn` has `recog` attached.
pub fn run(conn: &Connection, stop: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, u64)) -> Result<Summary> {
    let job = Job::start_in(conn, "recog.jobs", "clusters")?;
    let result = compute(conn, &job, stop, progress);
    match &result {
        Ok(summary) => job.finish(conn, "done", summary)?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            let state = if e.is::<Interrupted>() { "interrupted" } else { "failed" };
            let _ = job.finish(conn, state, &format!("{e:#}"));
        }
    }
    result
}

/// The faces taking part: ids and embeddings (`dim` floats each).
struct Eligible {
    ids: Vec<i64>,
    data: Vec<f32>,
    dim: usize,
}

impl Eligible {
    fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }
}

fn eligible(conn: &Connection, space: Space, model: &str, hidden: &HashSet<i64>) -> Result<Eligible> {
    let mut stmt = conn.prepare(&format!(
        "SELECT f.id, f.emb FROM recog.faces f
         JOIN recog.looked l ON l.key = f.key AND l.task = '{task}'
         WHERE f.model = ?1 AND {filter} AND {px} >= ?2
           AND f.key IN (SELECT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS}))
         ORDER BY f.id",
        task = space.task(),
        filter = space.filter(),
        px = faces::size_px("f.roll")
    ))?;
    let mut rows = stmt.query(params![model, space.min_px()])?;
    let mut out = Eligible { ids: Vec::new(), data: Vec::new(), dim: 0 };
    while let Some(r) = rows.next()? {
        let bytes: Vec<u8> = r.get(1)?;
        let emb: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if out.dim == 0 {
            out.dim = emb.len();
        }
        if emb.is_empty() || emb.len() != out.dim {
            continue;
        }
        let id: i64 = r.get(0)?;
        if hidden.contains(&id) {
            continue;
        }
        out.ids.push(id);
        out.data.extend(emb);
    }
    Ok(out)
}

fn encode(list: &[(i64, f32)]) -> Vec<u8> {
    list.iter().flat_map(|(id, sim)| id.to_le_bytes().into_iter().chain(sim.to_le_bytes())).collect()
}

fn decode(bytes: &[u8]) -> impl Iterator<Item = i64> + '_ {
    bytes.chunks_exact(12).map(|c| i64::from_le_bytes(c[..8].try_into().unwrap()))
}

/// One space's faces taking part, with what it is compared by.
struct Part {
    space: Space,
    model: String,
    th: Thresholds,
    all: Eligible,
    /// Positions in `all` that still need a neighbour list.
    todo: Vec<usize>,
}

/// What the analysis of one space found: clusters of positions in `all`,
/// and the person suggested for a position.
struct Analysis {
    groups: Vec<Vec<usize>>,
    suggestion: HashMap<usize, (i64, f32)>,
    /// What the suggestions came from, for the next run (`Previous`):
    /// (ref, person), and the faces without a decision with their rejections.
    refs: Vec<(i64, i64)>,
    open: Vec<(i64, String)>,
    searched: u64,
    compared: u64,
}

/// What the last run worked the suggestions out from (`recog.suggest_*`,
/// written with `recog.clusters`). A face's suggestion depends only on its
/// embedding, the confirmed faces (the references) and its rejections, so
/// the next run searches all references only for faces where one of those
/// changed, and compares the others with the references added since.
#[derive(Default)]
struct Previous {
    /// Space (`Space::task`) → its model and its references (ref → person).
    refs: HashMap<String, (String, HashMap<i64, i64>)>,
    /// The faces without a decision then: their suggestion.
    rows: HashMap<i64, Option<(i64, f32)>>,
    /// Their space and rejections (`rejections`).
    open: HashMap<i64, (String, String)>,
}

/// A face's rejections as `recog.suggest_open` keeps them.
fn rejections(rejected: &[i64]) -> String {
    let mut ids = rejected.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

impl Previous {
    fn load(conn: &Connection) -> Result<Previous> {
        let mut prev = Previous::default();
        let runs: Vec<(String, String)> =
            conn.prepare("SELECT space, model FROM recog.suggest_runs")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT ref, person FROM recog.suggest_refs WHERE space = ?1")?;
        for (space, model) in runs {
            let refs: HashMap<i64, i64> = stmt.query_map([&space], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            prev.refs.insert(space, (model, refs));
        }
        let mut stmt = conn.prepare("SELECT face, person, similarity FROM recog.clusters")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let (face, person, sim): (i64, Option<i64>, Option<f64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
            prev.rows.insert(face, person.zip(sim).map(|(p, s)| (p, s as f32)));
        }
        let mut stmt = conn.prepare("SELECT face, space, rejected FROM recog.suggest_open")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            prev.open.insert(r.get(0)?, (r.get(1)?, r.get(2)?));
        }
        Ok(prev)
    }
}

/// How long each phase of a run took, printed (to stderr) when
/// `SHOEBOX_CLUSTER_TIMES` is set: `regrouping_phases` in
/// `core/tests/people_bench.rs`.
struct Laps {
    last: Instant,
    on: bool,
}

impl Laps {
    fn new() -> Laps {
        Laps { last: Instant::now(), on: std::env::var_os("SHOEBOX_CLUSTER_TIMES").is_some() }
    }

    /// The phase since the last lap is done.
    fn lap(&mut self, what: impl FnOnce() -> String) {
        if self.on {
            eprintln!("  {:52} {:9.1} ms", what(), self.last.elapsed().as_secs_f64() * 1000.0);
        }
        self.last = Instant::now();
    }
}

fn compute(conn: &Connection, job: &Job, stop: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, u64)) -> Result<Summary> {
    let started = Instant::now();
    let mut laps = Laps::new();
    let mut parts: Vec<Part> = Vec::new();
    // A dog's face found by the face detector belongs to the pet, not here.
    let hidden = crate::people::pet_face_ids(conn)?;
    laps.lap(|| "pet_face_ids".into());
    for space in [Space::Faces, Space::Pets] {
        let Some(model) = current_model_of(conn, space)? else { continue };
        let all = eligible(conn, space, &model, &hidden)?;
        laps.lap(|| format!("eligible {space:?} ({} faces)", all.ids.len()));
        let mut th = space.thresholds(&model);
        if space == Space::Faces {
            th.cohesion = face_cohesion(th.cohesion);
        }
        parts.push(Part { space, model, th, all, todo: Vec::new() });
    }

    // What the user decided, as it is now. Decisions made while this runs
    // are seen through by the people view and count towards the next run.
    let m = Matched::load(conn, None)?;
    let state_of: HashMap<i64, usize> = m.faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let searches: u64 = parts
        .iter()
        .map(|p| p.all.ids.iter().filter(|id| state_of.get(id).and_then(|&s| m.decided(s)).is_none()).count() as u64)
        .sum();
    laps.lap(|| "decisions (Matched::load)".into());

    // Neighbour lists: kept from earlier runs, computed for the others.
    conn.execute("DELETE FROM recog.neighbours WHERE face NOT IN (SELECT id FROM recog.faces)", [])?;
    let have: HashSet<i64> =
        conn.prepare("SELECT face FROM recog.neighbours")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for part in &mut parts {
        part.todo = (0..part.all.ids.len()).filter(|&i| !have.contains(&part.all.ids[i])).collect();
    }
    let total: u64 = parts.iter().map(|p| p.todo.len() as u64).sum::<u64>() + searches;
    laps.lap(|| "neighbour lists kept".into());
    job.progress(conn, 0, Some(total))?;
    let mut done = 0;
    for part in &parts {
        if part.todo.is_empty() {
            continue;
        }
        let all = &part.all;
        let index = Index::build(&all.data, all.dim);
        for chunk in part.todo.chunks(CHUNK) {
            if stop() {
                return Err(Interrupted.into());
            }
            let lists = ann::parallel(chunk.len(), |c| index.search(all.row(chunk[c]), NEIGHBOURS, part.th.cluster, Some(chunk[c])));
            let tx = conn.unchecked_transaction()?;
            {
                let mut insert = tx.prepare("INSERT OR REPLACE INTO recog.neighbours (face, list) VALUES (?1, ?2)")?;
                for (&i, list) in chunk.iter().zip(&lists) {
                    let list: Vec<(i64, f32)> = list.iter().map(|&(j, sim)| (all.ids[j], sim)).collect();
                    insert.execute(params![all.ids[i], encode(&list)])?;
                }
            }
            tx.commit()?;
            done += chunk.len() as u64;
            job.progress(conn, done, Some(total))?;
            progress(done, total);
        }
    }


    laps.lap(|| format!("neighbour lists computed ({})", total - searches));
    let previous = Previous::load(conn)?;
    laps.lap(|| format!("the last run's suggestions ({} faces)", previous.rows.len()));
    let mut summaries: Vec<Summary> = Vec::new();
    // What this run's suggestions come from, per space.
    let mut sources: Vec<(&'static str, &str, Vec<(i64, i64)>, Vec<(i64, String)>)> = Vec::new();
    // (face, cluster, person, similarity) of every face without a decision.
    let mut rows: Vec<(i64, i64, Option<i64>, Option<f64>)> = Vec::new();
    let mut numbered = 0i64;
    for part in &parts {
        let mut analysis = analyse(conn, &m, &state_of, part, &previous, &mut laps, stop, &mut |n| {
            done += n;
            job.progress(conn, done, Some(total))?;
            progress(done, total);
            Ok(())
        })?;
        let mut summary = Summary {
            faces: part.all.ids.len() as u64,
            listed: part.todo.len() as u64,
            clusters: analysis.groups.len() as u64,
            searched: analysis.searched,
            compared: analysis.compared,
            ..Summary::default()
        };
        sources.push((part.space.task(), &part.model, std::mem::take(&mut analysis.refs), std::mem::take(&mut analysis.open)));
        for (c, group) in analysis.groups.iter().enumerate() {
            summary.sizes.add(group.len());
            for &i in group {
                let s = analysis.suggestion.get(&i);
                rows.push((part.all.ids[i], numbered + c as i64 + 1, s.map(|s| s.0), s.map(|s| s.1 as f64)));
                summary.unnamed += 1;
                match s {
                    Some(&(_, sim)) if sim >= part.th.suggest => summary.suggested += 1,
                    Some(_) => summary.maybe += 1,
                    None => {}
                }
            }
        }
        numbered += analysis.groups.len() as i64;
        summaries.push(summary);
    }

    laps.lap(|| "summaries".into());
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM recog.clusters", [])?;
    {
        let mut insert = tx.prepare("INSERT INTO recog.clusters (face, cluster, person, similarity) VALUES (?1, ?2, ?3, ?4)")?;
        for (face, cluster, person, similarity) in &rows {
            insert.execute(params![face, cluster, person, similarity])?;
        }
    }
    tx.execute_batch("DELETE FROM recog.suggest_runs; DELETE FROM recog.suggest_refs; DELETE FROM recog.suggest_open;")?;
    {
        let mut run = tx.prepare("INSERT INTO recog.suggest_runs (space, model) VALUES (?1, ?2)")?;
        let mut source = tx.prepare("INSERT INTO recog.suggest_refs (space, ref, person) VALUES (?1, ?2, ?3)")?;
        let mut open = tx.prepare("INSERT OR REPLACE INTO recog.suggest_open (face, space, rejected) VALUES (?1, ?2, ?3)")?;
        for (space, model, refs, faces) in &sources {
            run.execute(params![space, model])?;
            for (r, person) in refs {
                source.execute(params![space, r, person])?;
            }
            for (face, rejected) in faces {
                open.execute(params![face, space, rejected])?;
            }
        }
    }
    tx.commit()?;
    laps.lap(|| format!("write recog.clusters ({} rows)", rows.len()));
    let mut summaries = summaries.into_iter();
    let mut out = Summary::default();
    for part in &parts {
        let s = summaries.next().expect("one summary per part");
        match part.space {
            Space::Faces => out = s,
            Space::Pets => out.pets = Some(Box::new(s)),
        }
    }
    out.seconds = started.elapsed().as_secs_f64();
    Ok(out)
}

/// Clusters and suggestions of one space.
/// `searched` is told of suggestions searched (`SEARCH_CHUNK` at a time);
/// `stop` is asked between those.
fn analyse(
    conn: &Connection,
    m: &Matched,
    state_of: &HashMap<i64, usize>,
    part: &Part,
    previous: &Previous,
    laps: &mut Laps,
    stop: &dyn Fn() -> bool,
    searched: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<Analysis> {
    let space = part.space;
    let all = &part.all;
    let n = all.ids.len();
    let pos: HashMap<i64, usize> = all.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let decided = |i: usize| state_of.get(&all.ids[i]).and_then(|&s| m.decided(s));
    let open: Vec<bool> = (0..n).map(|i| decided(i).is_none()).collect();

    // Suggestions: the most similar confirmed faces of every person (of
    // this space), and the faces or pets drawn by hand that were aligned.
    // First, because a cluster never joins faces suggested for two people.
    // A reference is a face's id, or minus the decision's for one drawn by hand.
    let mut ref_id: Vec<i64> = Vec::new();
    let mut ref_person: Vec<i64> = Vec::new();
    let mut ref_data: Vec<f32> = Vec::new();
    for i in 0..n {
        if let Some(p) = decided(i).filter(|d| d.decision == Decision::Confirmed).and_then(|d| d.person) {
            ref_id.push(all.ids[i]);
            ref_person.push(p);
            ref_data.extend_from_slice(all.row(i));
        }
    }
    for (d, p, emb) in drawn_references(conn, m, part.space, &part.model, all.dim)? {
        ref_id.push(-d);
        ref_person.push(p);
        ref_data.extend(emb);
    }
    laps.lap(|| format!("{space:?}: references ({})", ref_person.len()));
    let open_list: Vec<usize> = (0..n).filter(|&i| open[i]).collect();
    let rejected_of = |i: usize| state_of.get(&all.ids[i]).map(|&s| m.states[s].rejected.as_slice()).unwrap_or(&[]);

    // What changed since the last run: references added (or now someone
    // else's), and the people who lost one (their suggestions may have come
    // from it). Without a last run of this model, or with more references
    // added than a search compares a face with, every face is searched.
    let mut added: Vec<usize> = Vec::new();
    let mut dirty: HashSet<i64> = HashSet::new();
    let search_all = match previous.refs.get(space.task()).filter(|(model, _)| *model == part.model) {
        None => true,
        Some((_, before)) => {
            let now: HashMap<i64, i64> = ref_id.iter().copied().zip(ref_person.iter().copied()).collect();
            dirty.extend(before.iter().filter(|(r, p)| now.get(r) != Some(p)).map(|(_, p)| *p));
            added.extend((0..ref_id.len()).filter(|&k| before.get(&ref_id[k]) != Some(&ref_person[k])));
            added.len() > Index::compared_per_search(ref_id.len())
        }
    };
    // A face is searched again when it had no decision at the last run, or
    // not in this space (new, a decision undone, a pet's species corrected),
    // its rejections changed, or the person it was suggested for lost a
    // reference.
    let rejected_now: Vec<String> = open_list.iter().map(|&i| rejections(rejected_of(i))).collect();
    let search: Vec<bool> = open_list
        .iter()
        .zip(&rejected_now)
        .map(|(&i, rejected)| {
            let id = all.ids[i];
            search_all
                || match (previous.open.get(&id), previous.rows.get(&id)) {
                    (Some((was, before_rejected)), Some(before)) => {
                        was != space.task() || before_rejected != rejected || before.is_some_and(|(p, _)| dirty.contains(&p))
                    }
                    _ => true,
                }
        })
        .collect();
    let searched_n = search.iter().filter(|&&s| s).count() as u64;
    let ref_index = (!ref_person.is_empty() && searched_n > 0).then(|| Index::build(&ref_data, all.dim.max(1)));
    laps.lap(|| format!("{space:?}: reference index"));
    let dim = all.dim.max(1);
    let new_person: Vec<i64> = added.iter().map(|&k| ref_person[k]).collect();
    let new_data: Vec<f32> = added.iter().flat_map(|&k| ref_data[k * dim..(k + 1) * dim].iter().copied()).collect();
    let mut suggestions: Vec<Option<(i64, f32)>> = Vec::with_capacity(open_list.len());
    for (chunk, wanted) in open_list.chunks(SEARCH_CHUNK).zip(search.chunks(SEARCH_CHUNK)) {
        if stop() {
            return Err(Interrupted.into());
        }
        suggestions.extend(ann::parallel(chunk.len(), |k| {
            let i = chunk[k];
            let rejected = rejected_of(i);
            if wanted[k] {
                return ref_index.as_ref().and_then(|index| {
                    index
                        .search(all.row(i), REFERENCES, part.th.maybe, None)
                        .into_iter()
                        .map(|(r, sim)| (ref_person[r], sim))
                        .find(|(p, _)| !rejected.contains(p))
                });
            }
            // The earlier suggestion, unless one of the new references is closer.
            let mut best = previous.rows.get(&all.ids[i]).copied().flatten();
            for (p, emb) in new_person.iter().zip(new_data.chunks_exact(dim)) {
                let sim = ann::dot(all.row(i), emb);
                if sim >= part.th.maybe && !rejected.contains(p) && best.is_none_or(|b| sim > b.1) {
                    best = Some((*p, sim));
                }
            }
            best
        }));
        searched(chunk.len() as u64)?;
    }
    let suggestion: HashMap<usize, (i64, f32)> =
        open_list.iter().zip(suggestions).filter_map(|(&i, s)| s.map(|s| (i, s))).collect();
    let refs: Vec<(i64, i64)> = ref_id.into_iter().zip(ref_person).collect();
    let open_faces: Vec<(i64, String)> = open_list.iter().map(|&i| all.ids[i]).zip(rejected_now).collect();
    let compared = open_list.len() as u64 - searched_n;
    laps.lap(|| format!("{space:?}: suggestions ({searched_n} searched, {compared} compared with {} new)", new_person.len()));

    // Clusters: neighbours without a decision, joined. Similarities are
    // computed again (cheap), so a list can never join faces that are not
    // close.
    let mut edges: Vec<(usize, usize, f32)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT face, list FROM recog.neighbours")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let Some(&i) = pos.get(&r.get::<_, i64>(0)?) else { continue };
            if !open[i] {
                continue;
            }
            let bytes: Vec<u8> = r.get(1)?;
            for other in decode(&bytes) {
                let Some(&j) = pos.get(&other) else { continue };
                let sim = if open[j] { ann::dot(all.row(i), all.row(j)) } else { 0.0 };
                if sim >= part.th.cluster && i != j {
                    edges.push((i.min(j), i.max(j), sim));
                }
            }
        }
    }
    laps.lap(|| format!("{space:?}: neighbours read, edges ({})", edges.len()));
    let label: Vec<Option<i64>> =
        (0..n).map(|i| suggestion.get(&i).filter(|s| s.1 >= part.th.suggest).map(|s| s.0)).collect();
    let cohesion = part.th.cohesion.map(|min| (all.data.as_slice(), all.dim, min));
    let mut parent = join(n, &mut edges, &label, cohesion);
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in (0..n).filter(|&i| open[i]) {
        members.entry(root(&mut parent, i)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut big_edges: HashMap<usize, Vec<(usize, usize, f32)>> = HashMap::new();
    if members.values().any(|m| m.len() > MAX_CLUSTER) {
        for &(i, j, sim) in &edges {
            let r = root(&mut parent, i);
            // An edge that was not taken may lead into another cluster.
            if r == root(&mut parent, j) && members[&r].len() > MAX_CLUSTER {
                big_edges.entry(r).or_default().push((i, j, sim));
            }
        }
    }
    for (r, m) in members {
        if m.len() > MAX_CLUSTER {
            split(m, big_edges.remove(&r).unwrap_or_default(), part.th.cluster, &mut groups);
        } else {
            groups.push(m);
        }
    }
    drop(edges);
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    laps.lap(|| format!("{space:?}: join and split ({} cards)", groups.len()));
    Ok(Analysis { groups, suggestion, refs, open: open_faces, searched: searched_n, compared })
}

/// Join `n` faces along `edges` (pairs at least the cluster similarity
/// apart), the most similar pair first, and return the union-find parents.
/// Two parts are not joined when
/// - faces in them are suggested (`label`, at least the suggest similarity)
///   for two different people: two pets that look alike to the model end up
///   on two cards, not on one card that says "Looks like Spucky (44 of 82)"
///   while Judy is in it as well;
/// - with `cohesion` (embeddings, their length, the least average), the
///   average similarity of every face of one part with every face of the
///   other is below it. For L2-normalised embeddings that average is the dot
///   product of the parts' sums divided by both sizes, so it costs one
///   product per edge. A chain of look-alikes (each close to the next, the
///   ends far apart) stops growing where the parts no longer fit together.
///
/// Sorts `edges` (and drops the doubles).
fn join(n: usize, edges: &mut Vec<(usize, usize, f32)>, label: &[Option<i64>], cohesion: Option<(&[f32], usize, f32)>) -> Vec<usize> {
    edges.sort_by(|a, b| b.2.total_cmp(&a.2).then((a.0, a.1).cmp(&(b.0, b.1))));
    edges.dedup_by(|a, b| (a.0, a.1) == (b.0, b.1));
    let mut parent: Vec<usize> = (0..n).collect();
    let mut label = label.to_vec();
    let mut size = vec![1usize; n];
    let (mut sum, dim, least) = match cohesion {
        Some((data, dim, least)) => (data.to_vec(), dim, least),
        None => (Vec::new(), 0, f32::MIN),
    };
    for &(i, j, _) in edges.iter() {
        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
        if a == b {
            continue;
        }
        if matches!((label[a], label[b]), (Some(p), Some(q)) if p != q) {
            continue;
        }
        let (keep, gone) = (a.min(b), a.max(b));
        if dim > 0 {
            let average = ann::dot(&sum[a * dim..(a + 1) * dim], &sum[b * dim..(b + 1) * dim]) / (size[a] * size[b]) as f32;
            if average < least {
                continue;
            }
            let (low, high) = sum.split_at_mut(gone * dim);
            for (s, v) in low[keep * dim..(keep + 1) * dim].iter_mut().zip(&high[..dim]) {
                *s += v;
            }
        }
        parent[gone] = keep;
        size[keep] += size[gone];
        label[keep] = label[keep].or(label[gone]);
    }
    parent
}

/// Union-find: the root of `i`, flattening the path.
fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Cut a cluster of more than `MAX_CLUSTER` faces into pieces that fit:
/// join its faces again with a similarity `SPLIT_STEP` higher, and do the
/// same with every piece still too large.
fn split(members: Vec<usize>, edges: Vec<(usize, usize, f32)>, sim: f32, out: &mut Vec<Vec<usize>>) {
    if members.len() <= MAX_CLUSTER {
        out.push(members);
        return;
    }
    let sim = sim + SPLIT_STEP;
    if sim > 1.0 {
        out.extend(members.chunks(MAX_CLUSTER).map(<[usize]>::to_vec));
        return;
    }
    let at: HashMap<usize, usize> = members.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut parent: Vec<usize> = (0..members.len()).collect();
    let edges: Vec<(usize, usize, f32)> = edges.into_iter().filter(|e| e.2 >= sim).collect();
    for &(i, j, _) in &edges {
        let (a, b) = (root(&mut parent, at[&i]), root(&mut parent, at[&j]));
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut pieces: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, &i) in members.iter().enumerate() {
        pieces.entry(root(&mut parent, k)).or_default().push(i);
    }
    let mut piece_edges: HashMap<usize, Vec<(usize, usize, f32)>> = HashMap::new();
    for e in edges {
        let r = root(&mut parent, at[&e.0]);
        if pieces[&r].len() > MAX_CLUSTER {
            piece_edges.entry(r).or_default().push(e);
        }
    }
    for (r, m) in pieces {
        split(m, piece_edges.remove(&r).unwrap_or_default(), sim, out);
    }
}

/// Faces (or, in the pets' space, pets) drawn by hand that serve as
/// references: confirmed for a person, over no detected one of their kind
/// (that one's own embedding counts then), of a present photo, aligned
/// (landmarks found; a pet's box always is), embedded with `model`, at
/// least the space's minimum wide.
/// (decision id, person, embedding) of the faces drawn by hand that serve
/// as references.
fn drawn_references(conn: &Connection, m: &Matched, space: Space, model: &str, dim: usize) -> Result<Vec<(i64, i64, Vec<f32>)>> {
    // The width of the copy the worker saw is the same in every pass.
    let mut stmt = conn.prepare(&format!(
        "SELECT e.emb, (SELECT l.width FROM recog.looked l WHERE l.key = e.key AND l.width IS NOT NULL LIMIT 1)
         FROM recog.drawn e
         WHERE e.key = ?1 AND e.x = ?2 AND e.y = ?3 AND e.w = ?4 AND e.h = ?5 AND e.model = ?6 AND e.aligned = 1
           AND e.emb IS NOT NULL
           AND e.key IN (SELECT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS}))"
    ))?;
    let mut out = Vec::new();
    for (d, row) in m.decisions.iter().enumerate() {
        let Some(person) = row.person else { continue };
        if !row.manual || row.decision != Decision::Confirmed || m.face_of[d].is_some() {
            continue;
        }
        if Space::of(row.species.as_deref()) != space {
            continue;
        }
        let found: Option<(Vec<u8>, Option<f64>)> = stmt
            .query_row(params![row.key, row.b[0], row.b[1], row.b[2], row.b[3], model], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((bytes, width)) = found else { continue };
        if space.too_small(row.b[2] * width.unwrap_or(0.0)) {
            continue;
        }
        let emb: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if dim > 0 && emb.len() == dim {
            out.push((row.id, person, emb));
        }
    }
    Ok(out)
}

/// Where the clustering stands, for `/api/info`.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    /// A clustering job is running (in any process).
    pub running: bool,
    /// Steps of the latest job (neighbour lists, suggestions searched):
    /// done and to do.
    pub done: i64,
    pub total: Option<i64>,
    /// The latest job: `running`, `done`, `failed`, `interrupted`.
    pub state: Option<String>,
    pub finished_at: Option<i64>,
    /// In the cache: clusters and the faces in them, faces suggested for
    /// someone (≥ `people::SUGGEST_SIM`).
    pub clusters: u64,
    pub unnamed: u64,
    pub suggested: u64,
    /// People named so far.
    pub people: u64,
}

pub fn overview(conn: &Connection) -> Result<Overview> {
    let latest: Option<(i64, Option<i64>, String, Option<i64>)> = conn
        .query_row(
            "SELECT done, total, state, finished_at FROM recog.jobs WHERE kind = 'clusters' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (clusters, unnamed): (i64, i64) =
        conn.query_row("SELECT count(DISTINCT cluster), count(*) FROM recog.clusters", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    // Suggested: at least as similar as the face's own space asks for.
    let mut suggested = 0i64;
    {
        let mut stmt = conn.prepare(
            "SELECT f.species, f.model, c.similarity FROM recog.clusters c JOIN recog.faces f ON f.id = c.face
             WHERE c.similarity IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let (species, model, sim): (Option<String>, String, f64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            suggested += (sim as f32 >= Space::of(species.as_deref()).thresholds(&model).suggest) as i64;
        }
    }
    let people: i64 = conn.query_row("SELECT count(*) FROM people", [], |r| r.get(0))?;
    let (done, total, state, finished_at) = match latest {
        Some((d, t, s, f)) => (d, t, Some(s), f),
        None => (0, None, None, None),
    };
    Ok(Overview {
        running: running(conn)?,
        done,
        total,
        state,
        finished_at,
        clusters: clusters as u64,
        unnamed: unnamed as u64,
        suggested: suggested as u64,
        people: people as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(mut out: Vec<Vec<usize>>) -> Vec<usize> {
        out.sort_by_key(|g| std::cmp::Reverse(g.len()));
        out.iter().map(Vec::len).collect()
    }

    fn clusters(n: usize, mut edges: Vec<(usize, usize, f32)>, label: &[Option<i64>], cohesion: Option<(&[f32], usize, f32)>) -> Vec<Vec<usize>> {
        let mut parent = join(n, &mut edges, label, cohesion);
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            groups.entry(root(&mut parent, i)).or_default().push(i);
        }
        let mut out: Vec<Vec<usize>> = groups.into_values().collect();
        out.sort();
        out
    }

    /// Without suggestions or cohesion (faces), neighbours chain as before.
    #[test]
    fn neighbours_chain_into_one_cluster() {
        let edges = vec![(0, 1, 0.7), (1, 2, 0.65), (2, 3, 0.61), (4, 5, 0.9)];
        assert_eq!(clusters(6, edges, &[None; 6], None), vec![vec![0, 1, 2, 3], vec![4, 5]]);
    }

    /// Two pets suggested for Spucky and Judy, linked by a look-alike: the
    /// strongest links win and the two never share a card.
    #[test]
    fn faces_suggested_for_two_people_are_never_joined() {
        let label = [Some(1), None, None, Some(2), None];
        // 0 (Spucky) - 1 - 2 - 3 (Judy), and 4 close to Judy.
        let edges = vec![(0, 1, 0.95), (1, 2, 0.91), (2, 3, 0.93), (3, 4, 0.92), (1, 4, 0.90)];
        let out = clusters(5, edges, &label, None);
        assert_eq!(out, vec![vec![0, 1], vec![2, 3, 4]]);
        // The same without suggestions: one chain.
        let edges = vec![(0, 1, 0.95), (1, 2, 0.91), (2, 3, 0.93), (3, 4, 0.92), (1, 4, 0.90)];
        assert_eq!(clusters(5, edges, &[None; 5], None).len(), 1);
    }

    /// A unit vector at `deg` degrees in the plane: neighbours on a circle
    /// are close, the ends of an arc far apart.
    fn at(deg: f32) -> [f32; 2] {
        let r = deg.to_radians();
        [r.cos(), r.sin()]
    }

    /// 82 look-alikes in a chain (each 0.95 to the next, the ends opposite):
    /// joined link by link they are one card; with cohesion they fall into
    /// pieces of a few that look alike as a whole.
    #[test]
    fn a_chain_of_look_alikes_falls_into_small_clusters_with_cohesion() {
        let step = 0.95f32.acos().to_degrees();
        let data: Vec<f32> = (0..82).flat_map(|k| at(k as f32 * step)).collect();
        let edges: Vec<_> = (0..81).map(|k| (k, k + 1, ann::dot(&data[k * 2..k * 2 + 2], &data[k * 2 + 2..k * 2 + 4]))).collect();
        assert_eq!(clusters(82, edges.clone(), &[None; 82], None).len(), 1);
        let out = clusters(82, edges, &[None; 82], Some((&data, 2, 0.875)));
        assert!(out.len() >= 10, "{} clusters", out.len());
        // Cut greedily, a chain may leave one at an end on its own.
        assert!(out.iter().filter(|g| g.len() == 1).count() <= 2, "{out:?}");
        for g in &out {
            assert!(g.len() <= 8, "{g:?}");
            // Every pair in a piece at least the cohesion similar on average.
            let mut total = 0.0;
            for &i in g {
                for &j in g {
                    total += ann::dot(&data[i * 2..i * 2 + 2], &data[j * 2..j * 2 + 2]);
                }
            }
            assert!(total / (g.len() * g.len()) as f32 >= 0.875, "{g:?}");
        }
    }

    /// Three tight groups of 60 joined by weak links make one chained
    /// cluster of 180: it falls apart into the three.
    #[test]
    fn a_chained_cluster_is_split_where_the_links_are_weak() {
        let mut edges = Vec::new();
        for g in 0..3 {
            for k in 1..60 {
                edges.push((g * 60, g * 60 + k, 0.9));
            }
        }
        edges.push((0, 60, 0.62));
        edges.push((60, 120, 0.64));
        let mut out = Vec::new();
        split((0..180).collect(), edges, CLUSTER_SIM, &mut out);
        assert_eq!(sizes(out), vec![60, 60, 60]);
    }

    /// Faces that stay joined at every similarity are cut into pieces.
    #[test]
    fn identical_faces_are_cut_into_pieces() {
        let edges: Vec<_> = (1..250).map(|k| (0, k, 1.0)).collect();
        let mut out = Vec::new();
        split((0..250).collect(), edges, CLUSTER_SIM, &mut out);
        assert_eq!(sizes(out), vec![100, 100, 50]);
    }
}
