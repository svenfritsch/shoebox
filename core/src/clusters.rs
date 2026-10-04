//! Clusters and suggestions (phase 5c-2): a cache in `recognition.db`,
//! recomputed from the faces and the user's decisions after every
//! `shoebox recognize` run and in the background in `shoebox serve`.
//!
//! - Only faces of present photos take part, found with the current model
//!   (the one of the latest upright result), at least
//!   `faces::MIN_CLUSTER_PX` wide. Faces found by the rotated pass take
//!   part like the others.
//! - Neighbours, not all pairs: every such face looks up its nearest
//!   neighbours (`ann::Index`, similarity ≥ `CLUSTER_SIM`) once; the lists
//!   are kept in `recog.neighbours`, so a run that is stopped resumes, and
//!   after a `recognize` run only the new faces need theirs.
//! - Clusters, from scratch every time: faces without a decision that are
//!   neighbours end up in one cluster. Faces with a decision (confirmed,
//!   ignored, not a face) never take part.
//! - Suggestions: every face without a decision is compared with the
//!   confirmed faces of every person (best match, so confirmed faces from
//!   several ages bridge the gap a single reference cannot), never with a
//!   person it was rejected for: from `people::SUGGEST_SIM` suggested,
//!   from `people::MAYBE_SIM` offered as "maybe".
//!
//! Nothing here is user data: deleting `recognition.db` loses no decision.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::ann::{self, Index};
use crate::db::{self, Job};
use crate::faces::{self, MIN_CLUSTER_PX};
use crate::people::{self, Decision, Matched};
use crate::recognize::{FACES, Interrupted, KINDS};

/// Faces at least this similar (cosine) are neighbours, and neighbours
/// end up in one cluster. Stricter than `people::SUGGEST_SIM`: a cluster
/// grows from neighbour to neighbour, and on the real drive everything at
/// 0.60 and above was the same person.
pub const CLUSTER_SIM: f32 = 0.60;
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
    Ok(conn
        .query_row(
            &format!("SELECT model FROM recog.looked WHERE task = '{FACES}' AND error IS NULL ORDER BY done_at DESC LIMIT 1"),
            [],
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

fn eligible(conn: &Connection, model: &str) -> Result<Eligible> {
    let mut stmt = conn.prepare(&format!(
        "SELECT f.id, f.emb FROM recog.faces f
         JOIN recog.looked l ON l.key = f.key AND l.task = '{FACES}'
         WHERE f.model = ?1 AND {px} >= ?2
           AND f.key IN (SELECT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({KINDS}))
         ORDER BY f.id",
        px = faces::size_px("f.roll")
    ))?;
    let mut rows = stmt.query(params![model, MIN_CLUSTER_PX])?;
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

fn compute(conn: &Connection, job: &Job, stop: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, u64)) -> Result<Summary> {
    let started = Instant::now();
    let model = current_model(conn)?;
    let all = match &model {
        Some(m) => eligible(conn, m)?,
        None => Eligible { ids: Vec::new(), data: Vec::new(), dim: 0 },
    };
    let n = all.ids.len();
    let pos: HashMap<i64, usize> = all.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    // Neighbour lists: kept from earlier runs, computed for the others.
    conn.execute("DELETE FROM recog.neighbours WHERE face NOT IN (SELECT id FROM recog.faces)", [])?;
    let have: HashSet<i64> =
        conn.prepare("SELECT face FROM recog.neighbours")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let todo: Vec<usize> = (0..n).filter(|&i| !have.contains(&all.ids[i])).collect();
    let total = todo.len() as u64;
    job.progress(conn, 0, Some(total))?;
    if !todo.is_empty() {
        let index = Index::build(&all.data, all.dim);
        let mut done = 0;
        for chunk in todo.chunks(CHUNK) {
            if stop() {
                return Err(Interrupted.into());
            }
            let lists = ann::parallel(chunk.len(), |c| index.search(all.row(chunk[c]), NEIGHBOURS, CLUSTER_SIM, Some(chunk[c])));
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
    let decided = |i: usize| state_of.get(&all.ids[i]).and_then(|&s| m.decided(s));
    let open: Vec<bool> = (0..n).map(|i| decided(i).is_none()).collect();

    // Clusters: neighbours without a decision, joined. Similarities are
    // computed again (cheap), so a list can never join faces that are not
    // close.
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
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
                if open[j] && ann::dot(all.row(i), all.row(j)) >= CLUSTER_SIM {
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
    let mut groups: Vec<Vec<usize>> = members.into_values().collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));

    // Suggestions: the most similar confirmed faces of every person.
    let refs: Vec<(usize, i64)> = (0..n)
        .filter_map(|i| decided(i).filter(|d| d.decision == Decision::Confirmed).and_then(|d| d.person).map(|p| (i, p)))
        .collect();
    let ref_data: Vec<f32> = refs.iter().flat_map(|&(i, _)| all.row(i).iter().copied()).collect();
    let ref_index = Index::build(&ref_data, all.dim.max(1));
    let open_list: Vec<usize> = (0..n).filter(|&i| open[i]).collect();
    let suggestions: Vec<Option<(i64, f32)>> = if refs.is_empty() {
        vec![None; open_list.len()]
    } else {
        ann::parallel(open_list.len(), |k| {
            let i = open_list[k];
            let rejected = state_of.get(&all.ids[i]).map(|&s| m.states[s].rejected.as_slice()).unwrap_or(&[]);
            ref_index
                .search(all.row(i), REFERENCES, people::MAYBE_SIM, None)
                .into_iter()
                .map(|(r, sim)| (refs[r].1, sim))
                .find(|(p, _)| !rejected.contains(p))
        })
    };
    let suggestion: HashMap<usize, (i64, f32)> =
        open_list.iter().zip(suggestions).filter_map(|(&i, s)| s.map(|s| (i, s))).collect();

    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM recog.clusters", [])?;
    let mut summary = Summary { faces: n as u64, listed: total, clusters: groups.len() as u64, ..Summary::default() };
    {
        let mut insert = tx.prepare("INSERT INTO recog.clusters (face, cluster, person, similarity) VALUES (?1, ?2, ?3, ?4)")?;
        for (c, faces) in groups.iter().enumerate() {
            for &i in faces {
                let s = suggestion.get(&i);
                insert.execute(params![all.ids[i], c as i64 + 1, s.map(|s| s.0), s.map(|s| s.1 as f64)])?;
                summary.unnamed += 1;
                match s {
                    Some(&(_, sim)) if sim >= people::SUGGEST_SIM => summary.suggested += 1,
                    Some(_) => summary.maybe += 1,
                    None => {}
                }
            }
        }
    }
    tx.commit()?;
    summary.seconds = started.elapsed().as_secs_f64();
    Ok(summary)
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
    let (clusters, unnamed, suggested): (i64, i64, i64) = conn.query_row(
        "SELECT count(DISTINCT cluster), count(*), count(*) FILTER (WHERE similarity >= ?1) FROM recog.clusters",
        [people::SUGGEST_SIM as f64],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
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
