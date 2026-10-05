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
//!   neighbours end up in one cluster. Faces with a decision (confirmed,
//!   ignored, not a face) never take part.
//! - A cluster of more than `MAX_CLUSTER` faces is split with a stricter
//!   similarity (0.62, 0.64, …) until every piece fits.
//! - Suggestions: every face without a decision is compared with the
//!   confirmed faces of every person (best match, so confirmed faces from
//!   several ages bridge the gap a single reference cannot), never with a
//!   person it was rejected for: from `people::SUGGEST_SIM` suggested,
//!   from `people::MAYBE_SIM` offered as "maybe". Faces drawn by hand count
//!   as confirmed faces when the worker found landmarks in their box
//!   (`recog.drawn.aligned`); a plain crop's embedding is too unreliable.
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
    pub seconds: f64,
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
            println!("Clusters: looking up the neighbours of {total} faces…");
            progress = crate::scan::Progress::new(total);
        }
        progress.add(done - last, |done, total| format!("neighbours {done}/{total}"));
        last = done;
    })?;
    println!(
        "Clusters: {} faces large enough, {} without a decision in {} clusters; {} suggested, {} maybe ({:.1} s).",
        s.faces, s.unnamed, s.clusters, s.suggested, s.maybe, s.seconds
    );
    if let Some(a) = &s.pets {
        println!(
            "Clusters: {} cats and dogs large enough, {} without a decision in {} clusters; {} suggested, {} maybe.",
            a.faces, a.unnamed, a.clusters, a.suggested, a.maybe
        );
    }
    Ok(s)
}

/// Recompute the clusters and suggestions; resumes neighbour lists a
/// stopped run left. `stop` is asked between chunks (Ctrl-C, the server
/// stopping); `progress` gets (done, total) neighbour lists. `conn` has
/// `recog` attached.
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

fn eligible(conn: &Connection, space: Space, model: &str) -> Result<Eligible> {
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
        out.ids.push(r.get(0)?);
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
}

fn compute(conn: &Connection, job: &Job, stop: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, u64)) -> Result<Summary> {
    let started = Instant::now();
    let mut parts: Vec<Part> = Vec::new();
    for space in [Space::Faces, Space::Pets] {
        let Some(model) = current_model_of(conn, space)? else { continue };
        let all = eligible(conn, space, &model)?;
        let th = space.thresholds(&model);
        parts.push(Part { space, model, th, all, todo: Vec::new() });
    }

    // Neighbour lists: kept from earlier runs, computed for the others.
    conn.execute("DELETE FROM recog.neighbours WHERE face NOT IN (SELECT id FROM recog.faces)", [])?;
    let have: HashSet<i64> =
        conn.prepare("SELECT face FROM recog.neighbours")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for part in &mut parts {
        part.todo = (0..part.all.ids.len()).filter(|&i| !have.contains(&part.all.ids[i])).collect();
    }
    let total: u64 = parts.iter().map(|p| p.todo.len() as u64).sum();
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

    // What the user decided, as it is now.
    let m = Matched::load(conn, None)?;
    let state_of: HashMap<i64, usize> = m.faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();

    let mut summaries: Vec<Summary> = Vec::new();
    // (face, cluster, person, similarity) of every face without a decision.
    let mut rows: Vec<(i64, i64, Option<i64>, Option<f64>)> = Vec::new();
    let mut numbered = 0i64;
    for part in &parts {
        let analysis = analyse(conn, &m, &state_of, part)?;
        let mut summary = Summary {
            faces: part.all.ids.len() as u64,
            listed: part.todo.len() as u64,
            clusters: analysis.groups.len() as u64,
            ..Summary::default()
        };
        for (c, group) in analysis.groups.iter().enumerate() {
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

    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM recog.clusters", [])?;
    {
        let mut insert = tx.prepare("INSERT INTO recog.clusters (face, cluster, person, similarity) VALUES (?1, ?2, ?3, ?4)")?;
        for (face, cluster, person, similarity) in &rows {
            insert.execute(params![face, cluster, person, similarity])?;
        }
    }
    tx.commit()?;
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
fn analyse(conn: &Connection, m: &Matched, state_of: &HashMap<i64, usize>, part: &Part) -> Result<Analysis> {
    let all = &part.all;
    let n = all.ids.len();
    let pos: HashMap<i64, usize> = all.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let decided = |i: usize| state_of.get(&all.ids[i]).and_then(|&s| m.decided(s));
    let open: Vec<bool> = (0..n).map(|i| decided(i).is_none()).collect();

    // Clusters: neighbours without a decision, joined. Similarities are
    // computed again (cheap), so a list can never join faces that are not
    // close.
    let mut parent: Vec<usize> = (0..n).collect();
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
                if sim >= part.th.cluster {
                    edges.push((i, j, sim));
                    let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
            }
        }
    }
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in (0..n).filter(|&i| open[i]) {
        members.entry(root(&mut parent, i)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut big_edges: HashMap<usize, Vec<(usize, usize, f32)>> = HashMap::new();
    if members.values().any(|m| m.len() > MAX_CLUSTER) {
        for &(i, j, sim) in &edges {
            let r = root(&mut parent, i);
            if members[&r].len() > MAX_CLUSTER {
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

    // Suggestions: the most similar confirmed faces of every person (of
    // this space), and the faces or pets drawn by hand that were aligned.
    let mut ref_person: Vec<i64> = Vec::new();
    let mut ref_data: Vec<f32> = Vec::new();
    for i in 0..n {
        if let Some(p) = decided(i).filter(|d| d.decision == Decision::Confirmed).and_then(|d| d.person) {
            ref_person.push(p);
            ref_data.extend_from_slice(all.row(i));
        }
    }
    for (p, emb) in drawn_references(conn, m, part.space, &part.model, all.dim)? {
        ref_person.push(p);
        ref_data.extend(emb);
    }
    let ref_index = Index::build(&ref_data, all.dim.max(1));
    let open_list: Vec<usize> = (0..n).filter(|&i| open[i]).collect();
    let suggestions: Vec<Option<(i64, f32)>> = if ref_person.is_empty() {
        vec![None; open_list.len()]
    } else {
        ann::parallel(open_list.len(), |k| {
            let i = open_list[k];
            let rejected = state_of.get(&all.ids[i]).map(|&s| m.states[s].rejected.as_slice()).unwrap_or(&[]);
            ref_index
                .search(all.row(i), REFERENCES, part.th.maybe, None)
                .into_iter()
                .map(|(r, sim)| (ref_person[r], sim))
                .find(|(p, _)| !rejected.contains(p))
        })
    };
    let suggestion: HashMap<usize, (i64, f32)> =
        open_list.iter().zip(suggestions).filter_map(|(&i, s)| s.map(|s| (i, s))).collect();
    Ok(Analysis { groups, suggestion })
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
fn drawn_references(conn: &Connection, m: &Matched, space: Space, model: &str, dim: usize) -> Result<Vec<(i64, Vec<f32>)>> {
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
            out.push((person, emb));
        }
    }
    Ok(out)
}

/// Where the clustering stands, for `/api/info`.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    /// A clustering job is running (in any process).
    pub running: bool,
    /// Neighbour lists of the latest job: computed and to compute.
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
