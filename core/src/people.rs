//! People, groups and what the user decided about faces (phase 5c-2).
//!
//! - Everything the user says about a face is a row in `face_decisions`
//!   (`library.db`): `confirmed` (this is the person), `rejected` (this is
//!   not the person), `ignored` (a stranger nobody needs to name) or
//!   `not_face` (a false find: the box itself is wrong).
//! - A row is keyed by content and box (`quick_hash` + x/y/w/h), never by a
//!   face row id: it belongs to the detected face of that content whose box
//!   overlaps it best (IoU ≥ `MATCH_IOU`). So decisions survive moves,
//!   rescans, new `recognize` runs and model changes; one that no face
//!   matches any more is kept and shown as "face no longer found".
//! - A hand-drawn face (`manual = 1`) is the row itself, always `confirmed`
//!   with a person. A detected face that overlaps it is shown as that one
//!   face, the drawn box winning. Rejections and "not a face" are only for
//!   detected faces.
//! - A person is in at most one group (`people.group_id`, NULL: no group).
//! - Clusters and suggestions are a cache in `recognition.db`
//!   (`clusters.rs`). Everything read here is looked at through the
//!   decisions as they are now, so a decision shows at once, before the
//!   cache is recomputed.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::pets::{Space, Thresholds};
use crate::db;
use crate::faces;
use crate::library;
use crate::recognize::{self, PETS, FACES};

/// A decision belongs to the detected face whose box overlaps its box at
/// least this much (intersection over union), the best one if several do.
pub const MATCH_IOU: f64 = 0.5;
/// (For pets the numbers differ: `pets::Space::thresholds`.)
/// A face is suggested for a person when it is at least this similar
/// (cosine) to one of the person's confirmed faces. Calibrated on the real
/// drive (docs/phase5.md): the highest wrong match seen was 0.54.
pub const SUGGEST_SIM: f32 = 0.55;
/// Between this and `SUGGEST_SIM` a face is only offered as "maybe" (the
/// lowest right match seen was 0.37); below it nothing is suggested.
pub const MAYBE_SIM: f32 = 0.35;
/// Longest name of a person or group, in characters.
pub const MAX_NAME: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Confirmed,
    Rejected,
    Ignored,
    NotFace,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Confirmed => "confirmed",
            Decision::Rejected => "rejected",
            Decision::Ignored => "ignored",
            Decision::NotFace => "not_face",
        }
    }

    fn parse(s: &str) -> Option<Decision> {
        Some(match s {
            "confirmed" => Decision::Confirmed,
            "rejected" => Decision::Rejected,
            "ignored" => Decision::Ignored,
            "not_face" => Decision::NotFace,
            _ => return None,
        })
    }
}

/// The cluster numbers are stale: they changed since the list was shown.
#[derive(Debug)]
pub struct Stale;

impl std::fmt::Display for Stale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the clusters changed in the meantime; load them again")
    }
}

impl std::error::Error for Stale {}

// ---------------------------------------------------------------- matching

/// A row of `face_decisions`.
#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub id: i64,
    pub key: String,
    /// x, y, w, h as fractions of the upright picture.
    pub b: [f64; 4],
    pub person: Option<i64>,
    pub decision: Decision,
    pub manual: bool,
    pub at: i64,
    /// The kind of face it is about: `None` for a person's, `cat` or `dog`
    /// for a pet. It only ever belongs to a detected face of that kind.
    pub species: Option<String>,
}

/// A detected face (a row of `recog.faces`).
#[derive(Debug, Clone)]
pub struct Detected {
    pub id: i64,
    pub key: String,
    pub b: [f64; 4],
    pub roll: u16,
    /// Width across the face in px of the copy the worker saw.
    pub px: f64,
    pub score: f64,
    pub model: String,
    /// `cat` or `dog` for a pet, `None` for a person's face.
    pub species: Option<String>,
}

impl Detected {
    pub fn space(&self) -> Space {
        Space::of(self.species.as_deref())
    }

    /// The similarities that matter for this face (its space and model).
    pub fn thresholds(&self) -> Thresholds {
        self.space().thresholds(&self.model)
    }
}

/// What the decisions say about one detected face.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The decision saying what the face is (confirmed, ignored, not a
    /// face; the latest if several match), as an index into `decisions`.
    pub decided: Option<usize>,
    /// The people it is not.
    pub rejected: Vec<i64>,
}

/// The detected faces and the decisions, matched to each other.
pub struct Matched {
    pub faces: Vec<Detected>,
    pub decisions: Vec<DecisionRow>,
    /// Per face.
    pub states: Vec<State>,
    /// Per decision: the face it belongs to.
    pub face_of: Vec<Option<usize>>,
    /// Where the pets pass saw people, per content (fractions).
    pub bodies: HashMap<String, Vec<[f64; 4]>>,
}

pub(crate) fn table_exists(conn: &Connection, schema: &str, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(&format!("SELECT 1 FROM {schema}.sqlite_master WHERE type = 'table' AND name = ?1"), [table], |_| Ok(()))
        .optional()?
        .is_some())
}

impl Matched {
    /// All faces and decisions, or those of one content. `conn` has `recog`
    /// attached (read-only works: a `recognition.db` of phase 4 and a
    /// `library.db` before v4 too).
    pub fn load(conn: &Connection, key: Option<&str>) -> Result<Matched> {
        let version: i32 = conn.pragma_query_value(Some("recog"), "user_version", |r| r.get(0))?;
        let roll = if version >= 2 { "f.roll" } else { "0" };
        // Before v5 there are no pets.
        let species = if version >= 5 { "f.species" } else { "NULL" };
        let mut faces = Vec::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT f.id, f.key, f.x, f.y, f.w, f.h, {roll}, {px}, f.score, f.model, {species} FROM recog.faces f
                 LEFT JOIN recog.looked l ON l.key = f.key AND l.task = CASE WHEN {species} IS NULL THEN '{FACES}' ELSE '{PETS}' END
                 WHERE ?1 IS NULL OR f.key = ?1 ORDER BY f.id",
                px = faces::size_px(roll)
            ))?;
            let mut rows = stmt.query([key])?;
            while let Some(r) = rows.next()? {
                faces.push(Detected {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    b: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                    roll: r.get(6)?,
                    px: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                    score: r.get(8)?,
                    model: r.get(9)?,
                    species: r.get(10)?,
                });
            }
        }
        // A cat the detector took for a dog (or the other way round) is the
        // species the user said, for every use below.
        apply_species_overrides(conn, key, &mut faces)?;
        let decisions = if table_exists(conn, "main", "face_decisions")? { load_decisions(conn, key)? } else { Vec::new() };
        let mut m = Matched::new(faces, decisions);
        // Before v6 nobody saw people: no face is taken for a pet's.
        if version >= 6 {
            let mut stmt = conn.prepare("SELECT key, x, y, w, h FROM recog.bodies WHERE ?1 IS NULL OR key = ?1")?;
            let mut rows = stmt.query([key])?;
            while let Some(r) = rows.next()? {
                m.bodies.entry(r.get(0)?).or_default().push([r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]);
            }
        }
        Ok(m)
    }

    pub fn new(faces: Vec<Detected>, decisions: Vec<DecisionRow>) -> Matched {
        let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, f) in faces.iter().enumerate() {
            by_key.entry(f.key.as_str()).or_default().push(i);
        }
        let face_of: Vec<Option<usize>> = decisions
            .iter()
            .map(|d| {
                let candidates = by_key.get(d.key.as_str())?;
                candidates
                    .iter()
                    .filter(|&&i| crate::pets::kind_matches(d.species.as_deref(), faces[i].species.as_deref()))
                    .map(|&i| (i, recognize::iou(faces[i].b, d.b)))
                    .filter(|&(_, iou)| iou >= MATCH_IOU)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i)
            })
            .collect();
        let mut states = vec![State::default(); faces.len()];
        for (d, face) in face_of.iter().enumerate() {
            let Some(i) = *face else { continue };
            let row = &decisions[d];
            if row.decision == Decision::Rejected {
                states[i].rejected.extend(row.person);
            } else if states[i].decided.is_none_or(|j| (row.at, row.id) > (decisions[j].at, decisions[j].id)) {
                states[i].decided = Some(d);
            }
        }
        Matched { faces, decisions, states, face_of, bodies: HashMap::new() }
    }

    pub fn decided(&self, i: usize) -> Option<&DecisionRow> {
        self.states[i].decided.map(|d| &self.decisions[d])
    }

    /// Ids of the faces marked "not a face".
    pub fn not_faces(&self) -> HashSet<i64> {
        (0..self.faces.len())
            .filter(|&i| self.decided(i).is_some_and(|d| d.decision == Decision::NotFace))
            .map(|i| self.faces[i].id)
            .collect()
    }

    /// Ids of the people's faces that are really a pet's: inside a pet found
    /// in the same picture, outside any person (`pets::is_pet_face`). Only
    /// faces nobody decided on count, so drawing a face by hand or any
    /// decision settles it; pets marked "not a pet" do not count.
    pub fn pet_faces(&self) -> HashSet<i64> {
        let mut pets: HashMap<&str, Vec<[f64; 4]>> = HashMap::new();
        for (i, f) in self.faces.iter().enumerate() {
            let not_a_pet = self.decided(i).is_some_and(|d| d.decision == Decision::NotFace);
            if f.species.is_some() && !not_a_pet {
                pets.entry(f.key.as_str()).or_default().push(f.b);
            }
        }
        (0..self.faces.len())
            .filter(|&i| self.faces[i].species.is_none() && self.decided(i).is_none())
            .filter(|&i| {
                let f = &self.faces[i];
                let people = self.bodies.get(&f.key).map(Vec::as_slice).unwrap_or(&[]);
                pets.get(f.key.as_str()).is_some_and(|p| crate::pets::is_pet_face(f.b, p, people))
            })
            .map(|i| self.faces[i].id)
            .collect()
    }

    /// Decisions matched to face `i`.
    fn rows_of(&self, i: usize) -> Vec<usize> {
        (0..self.decisions.len()).filter(|&d| self.face_of[d] == Some(i)).collect()
    }
}

/// A row of `face_species`: the species the user gave a detected pet.
#[derive(Debug, Clone)]
pub struct SpeciesRow {
    pub id: i64,
    pub key: String,
    pub b: [f64; 4],
    pub species: String,
    pub at: i64,
}

fn load_species(conn: &Connection, key: Option<&str>) -> Result<Vec<SpeciesRow>> {
    // A library.db before v12 (read-only use) has none.
    if !table_exists(conn, "main", "face_species")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT id, key, x, y, w, h, species, at FROM face_species WHERE ?1 IS NULL OR key = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map([key], |r| Ok(SpeciesRow { id: r.get(0)?, key: r.get(1)?, b: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?], species: r.get(6)?, at: r.get(7)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The best overlapping pet of the entry's content (not a person's face).
fn pet_for(faces: &[Detected], key: &str, b: [f64; 4]) -> Option<usize> {
    (0..faces.len())
        .filter(|&i| faces[i].key == key && faces[i].species.is_some())
        .map(|i| (i, recognize::iou(faces[i].b, b)))
        .filter(|&(_, iou)| iou >= MATCH_IOU)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn apply_species_overrides(conn: &Connection, key: Option<&str>, faces: &mut [Detected]) -> Result<()> {
    // Oldest first, so the newest entry for a pet wins.
    for row in load_species(conn, key)? {
        if !crate::pets::is_species(&row.species) {
            continue;
        }
        if let Some(i) = pet_for(faces, &row.key, row.b) {
            faces[i].species = Some(row.species);
        }
    }
    Ok(())
}

fn load_decisions(conn: &Connection, key: Option<&str>) -> Result<Vec<DecisionRow>> {
    // A library.db before v7 (read-only use) has no species: all persons'.
    let species = if db::has_column(conn, "main", "face_decisions", "species")? { "species" } else { "NULL" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, key, x, y, w, h, person_id, decision, manual, at, {species} FROM face_decisions
         WHERE ?1 IS NULL OR key = ?1 ORDER BY id"
    ))?;
    let mut rows = stmt.query([key])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let text: String = r.get(7)?;
        let Some(decision) = Decision::parse(&text) else { continue };
        out.push(DecisionRow {
            id: r.get(0)?,
            key: r.get(1)?,
            b: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
            person: r.get(6)?,
            decision,
            manual: r.get(8)?,
            at: r.get(9)?,
            species: r.get(10)?,
        });
    }
    Ok(out)
}

/// Ids of the faces marked "not a face" (read-only works).
/// The species of every person who is a pet: the majority of their
/// confirmed faces (people's, cats', dogs', pets drawn without saying which),
/// a tie going to the people, then cats, then dogs. A dog the detector took
/// for a cat is still the dog's face once it is Layka's.
pub fn person_species(conn: &Connection) -> Result<HashMap<i64, String>> {
    if !table_exists(conn, "main", "face_decisions")? || !db::has_column(conn, "main", "face_decisions", "species")? {
        return Ok(HashMap::new());
    }
    // (people, cats, dogs, pets)
    let mut counts: HashMap<i64, [u64; 4]> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT person_id, species, count(*) FROM face_decisions
         WHERE decision = 'confirmed' AND person_id IS NOT NULL GROUP BY person_id, species",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let at = match r.get::<_, Option<String>>(1)?.as_deref() {
            None => 0,
            Some("cat") => 1,
            Some("dog") => 2,
            Some(_) => 3,
        };
        counts.entry(r.get(0)?).or_default()[at] += r.get::<_, i64>(2)? as u64;
    }
    Ok(counts
        .into_iter()
        .filter(|(_, [people, cats, dogs, pets])| cats + dogs + pets > *people)
        .map(|(id, [_, cats, dogs, pets])| {
            let species = if cats >= dogs && cats >= pets {
                "cat"
            } else if dogs >= pets {
                "dog"
            } else {
                crate::pets::PET
            };
            (id, species.to_string())
        })
        .collect())
}

/// The species to show for a pet's box: the person's when it belongs to one
/// who is a cat or a dog, else what was detected or drawn.
fn effective_species(own: Option<&str>, person: Option<i64>, of_people: &HashMap<i64, String>) -> Option<String> {
    let own = own?;
    match person.and_then(|p| of_people.get(&p)).map(String::as_str) {
        Some(s) if crate::pets::is_species(s) => Some(s.to_string()),
        _ => Some(own.to_string()),
    }
}

/// Ids of the faces taken for a pet's (`Matched::pet_faces`).
pub fn pet_face_ids(conn: &Connection) -> Result<HashSet<i64>> {
    Ok(Matched::load(conn, None)?.pet_faces())
}

pub fn not_face_ids(conn: &Connection) -> Result<HashSet<i64>> {
    Ok(Matched::load(conn, None)?.not_faces())
}

// ---------------------------------------------------------------- the view: decisions + cache

/// The cached cluster and suggestion of a face without a decision.
#[derive(Debug, Clone, Copy)]
struct Cached {
    cluster: i64,
    person: Option<i64>,
    similarity: Option<f32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PersonRef {
    pub id: i64,
    pub name: String,
}

/// A face as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct FaceItem {
    /// `recog.faces.id`; `null` for a hand-drawn face nothing was detected
    /// at, and for a decision whose face is no longer found.
    pub id: Option<i64>,
    /// The `face_decisions` id of a hand-drawn face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<i64>,
    /// A present file with this content (`null` if there is none).
    pub file: Option<i64>,
    pub kind: Option<String>,
    /// For thumbnail URLs.
    pub version: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub roll: u16,
    /// Width across the face in px of the copy the worker saw.
    pub px: f64,
    pub score: Option<f64>,
    /// Too small to cluster or suggest (`faces::MIN_CLUSTER_PX`, for
    /// pets `pets::MIN_CLUSTER_PX`).
    pub small: bool,
    /// `cat` or `dog` for a pet (its box holds the whole pet);
    /// absent for a person's face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    /// `confirmed`, `ignored` (decided), `suggested` or `maybe` (from the
    /// cache), or `null`.
    pub state: Option<&'static str>,
    /// Who it is (confirmed) or might be (suggested, maybe).
    pub person: Option<PersonRef>,
    /// Of a suggestion.
    pub similarity: Option<f32>,
    /// A decision no detected face matches any more.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
    /// People this face is not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<i64>,
}

/// Faces, decisions, present files, people and the cluster cache, read
/// together.
pub struct View {
    pub m: Matched,
    /// Content → (a present file, its kind).
    present: HashMap<String, (i64, String)>,
    cache: HashMap<i64, Cached>,
    names: HashMap<i64, String>,
    /// Content → width and height of the copy the worker saw.
    sizes: HashMap<String, (f64, f64)>,
    /// Faces taken for a pet's (`Matched::pet_faces`): not listed anywhere.
    pet_faces: HashSet<i64>,
    /// What each named person is (`cat`, `dog`, `pet`), by what their
    /// confirmed faces are; people's are left out.
    person_species: HashMap<i64, String>,
}

impl View {
    pub fn load(conn: &Connection) -> Result<View> {
        View::load_for(conn, None)
    }

    /// Everything, or what belongs to one content.
    pub fn load_for(conn: &Connection, key: Option<&str>) -> Result<View> {
        let m = Matched::load(conn, key)?;
        let mut present = HashMap::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT quick_hash, id, kind FROM ({}) WHERE ?1 IS NULL OR quick_hash = ?1",
                faces::PRESENT
            ))?;
            let mut rows = stmt.query([key])?;
            while let Some(r) = rows.next()? {
                present.insert(r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?));
            }
        }
        let mut cache = HashMap::new();
        if table_exists(conn, "recog", "clusters")? {
            let mut stmt = conn.prepare("SELECT face, cluster, person, similarity FROM recog.clusters")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                cache.insert(
                    r.get::<_, i64>(0)?,
                    Cached { cluster: r.get(1)?, person: r.get(2)?, similarity: r.get::<_, Option<f64>>(3)?.map(|s| s as f32) },
                );
            }
        }
        let names = conn
            .prepare("SELECT id, name FROM people")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut sizes = HashMap::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT key, width, height FROM recog.looked
                 WHERE task = '{FACES}' AND width IS NOT NULL AND (?1 IS NULL OR key = ?1)"
            ))?;
            let mut rows = stmt.query([key])?;
            while let Some(r) = rows.next()? {
                sizes.insert(r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?));
            }
        }
        let pet_faces = m.pet_faces();
        let person_species = person_species(conn)?;
        Ok(View { m, present, cache, names, sizes, pet_faces, person_species })
    }

    fn person(&self, id: Option<i64>) -> Option<PersonRef> {
        let id = id?;
        Some(PersonRef { id, name: self.names.get(&id)?.clone() })
    }

    /// The person suggested for face `i` (not decided, not rejected for
    /// them, similar enough), with the similarity.
    pub fn suggestion(&self, i: usize) -> Option<(i64, f32)> {
        if self.m.states[i].decided.is_some() || self.pet_faces.contains(&self.m.faces[i].id) {
            return None;
        }
        let c = self.cache.get(&self.m.faces[i].id)?;
        let (person, sim) = (c.person?, c.similarity?);
        (sim >= self.m.faces[i].thresholds().maybe
            && self.names.contains_key(&person)
            && !self.m.states[i].rejected.contains(&person))
        .then_some((person, sim))
    }

    /// The cluster of face `i` if it has no decision.
    fn cluster(&self, i: usize) -> Option<i64> {
        if self.m.states[i].decided.is_some() || self.pet_faces.contains(&self.m.faces[i].id) {
            return None;
        }
        self.cache.get(&self.m.faces[i].id).map(|c| c.cluster)
    }

    fn is_present(&self, key: &str) -> bool {
        self.present.contains_key(key)
    }

    fn file_fields(&self, key: &str) -> (Option<i64>, Option<String>, String) {
        let (file, kind) = match self.present.get(key) {
            Some((id, kind)) => (Some(*id), Some(kind.clone())),
            None => (None, None),
        };
        (file, kind, key.chars().take(8).collect())
    }

    /// Ids of the faces not matched yet: nothing decided, nobody suggested
    /// (not even as "maybe"), and not taken for a pet's. Faces of every
    /// size, so the Face check can show what recognition could not place.
    pub fn unplaced_ids(&self) -> HashSet<i64> {
        (0..self.m.faces.len())
            .filter(|&i| self.m.states[i].decided.is_none() && self.suggestion(i).is_none())
            .map(|i| self.m.faces[i].id)
            .filter(|id| !self.pet_faces.contains(id))
            .collect()
    }

    /// Detected face `i`, with what is decided or suggested.
    pub fn item(&self, i: usize) -> FaceItem {
        let f = &self.m.faces[i];
        let (file, kind, version) = self.file_fields(&f.key);
        let decided = self.m.decided(i);
        let (state, person, similarity) = match decided {
            Some(d) => (Some(d.decision.as_str()), self.person(d.person), None),
            None => match self.suggestion(i) {
                Some((p, sim)) => {
                    (Some(if sim >= f.thresholds().suggest { "suggested" } else { "maybe" }), self.person(Some(p)), Some(sim))
                }
                None => (None, None, None),
            },
        };
        // A hand-drawn box wins over the detected one it overlaps.
        let manual = decided.filter(|d| d.manual);
        let [x, y, w, h] = manual.map_or(f.b, |d| d.b);
        FaceItem {
            id: Some(f.id),
            manual: manual.map(|d| d.id),
            file,
            kind,
            version,
            x,
            y,
            w,
            h,
            roll: f.roll,
            px: f.px,
            score: Some(f.score),
            small: f.space().too_small(f.px),
            species: effective_species(f.species.as_deref(), decided.filter(|d| d.decision == Decision::Confirmed).and_then(|d| d.person), &self.person_species),
            state,
            person,
            similarity,
            lost: false,
            rejected: self.m.states[i].rejected.clone(),
        }
    }

    /// A decision no detected face matches: a hand-drawn face, or a
    /// detected face that is no longer found.
    fn unmatched_item(&self, d: usize) -> FaceItem {
        let row = &self.m.decisions[d];
        let (file, kind, version) = self.file_fields(&row.key);
        let px = self.sizes.get(&row.key).map_or(0.0, |(w, _)| row.b[2] * w);
        FaceItem {
            id: None,
            manual: row.manual.then_some(row.id),
            file,
            kind,
            version,
            x: row.b[0],
            y: row.b[1],
            w: row.b[2],
            h: row.b[3],
            roll: 0,
            px,
            score: None,
            small: false,
            species: effective_species(
                row.species.as_deref(),
                (row.decision == Decision::Confirmed).then_some(row.person).flatten(),
                &self.person_species,
            ),
            state: Some(row.decision.as_str()),
            person: self.person(row.person),
            similarity: None,
            lost: !row.manual,
            rejected: Vec::new(),
        }
    }

    /// Decisions that belong to no detected face, other than rejections.
    fn unmatched(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.m.decisions.len())
            .filter(|&d| self.m.face_of[d].is_none() && self.m.decisions[d].decision != Decision::Rejected)
    }
}

// ---------------------------------------------------------------- faces of a photo

/// A face in `/api/files/{id}`: the item plus its landmarks.
#[derive(Debug, Clone, Serialize)]
pub struct FileFace {
    #[serde(flatten)]
    pub item: FaceItem,
    pub landmarks: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileFaces {
    /// Faces to show (detected ones and hand-drawn ones); "not a face" is
    /// left out. `None` if the photo has not been looked at.
    pub faces: Option<Vec<FileFace>>,
    /// Decisions about faces that are no longer found.
    pub lost: Vec<FaceItem>,
}

/// The faces of one content, for the viewer and the info panel.
pub fn file_faces(conn: &Connection, key: &str) -> Result<FileFaces> {
    let Some(found) = recognize::faces_of(conn, key)? else {
        return Ok(FileFaces { faces: None, lost: Vec::new() });
    };
    let v = View::load_for(conn, Some(key))?;
    let landmarks: HashMap<[u64; 4], Vec<[f64; 2]>> =
        found.into_iter().map(|f| ([f.x, f.y, f.w, f.h].map(f64::to_bits), f.landmarks)).collect();
    let mut faces: Vec<FileFace> = (0..v.m.faces.len())
        .filter(|&i| v.m.decided(i).is_none_or(|d| d.decision != Decision::NotFace))
        .filter(|&i| !v.pet_faces.contains(&v.m.faces[i].id))
        .map(|i| FileFace {
            item: v.item(i),
            landmarks: landmarks.get(&v.m.faces[i].b.map(f64::to_bits)).cloned().unwrap_or_default(),
        })
        .collect();
    let mut lost = Vec::new();
    for d in v.unmatched() {
        let item = v.unmatched_item(d);
        if item.manual.is_some() {
            faces.push(FileFace { item, landmarks: Vec::new() });
        } else if item.state == Some("confirmed") {
            lost.push(item);
        }
    }
    faces.sort_by(|a, b| a.item.x.total_cmp(&b.item.x));
    Ok(FileFaces { faces: Some(faces), lost })
}

// ---------------------------------------------------------------- names

/// A name typed by the user: trimmed, NFC, not empty, no control characters.
pub fn check_name(name: &str) -> Result<String> {
    let name = library::nfc(name.trim());
    if name.is_empty() {
        bail!("the name is empty");
    }
    if name.chars().count() > MAX_NAME {
        bail!("names can have at most {MAX_NAME} characters");
    }
    if name.chars().any(char::is_control) {
        bail!("names cannot contain control characters");
    }
    Ok(name)
}

/// The id of the row in `table` with this name in any spelling (NFC,
/// ignoring case), other than `except`.
fn find_name(conn: &Connection, table: &str, name: &str, except: Option<i64>) -> Result<Option<i64>> {
    let fold = db::tag_fold(name);
    let rows: Vec<(i64, String)> = conn
        .prepare(&format!("SELECT id, name FROM {table} ORDER BY id"))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().find(|(id, n)| Some(*id) != except && db::tag_fold(n) == fold).map(|(id, _)| id))
}

fn exists(conn: &Connection, table: &str, id: i64) -> Result<bool> {
    Ok(conn.query_row(&format!("SELECT 1 FROM {table} WHERE id = ?1"), [id], |_| Ok(())).optional()?.is_some())
}

// ---------------------------------------------------------------- groups

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub position: i64,
    /// People in it (hidden ones too).
    pub people: u64,
}

pub fn groups(conn: &Connection) -> Result<Vec<Group>> {
    Ok(conn
        .prepare(
            "SELECT g.id, g.name, g.position, (SELECT count(*) FROM people p WHERE p.group_id = g.id)
             FROM groups g ORDER BY g.position, g.id",
        )?
        .query_map([], |r| Ok(Group { id: r.get(0)?, name: r.get(1)?, position: r.get(2)?, people: r.get::<_, i64>(3)? as u64 }))?
        .collect::<rusqlite::Result<_>>()?)
}

fn group(conn: &Connection, id: i64) -> Result<Group> {
    groups(conn)?.into_iter().find(|g| g.id == id).ok_or_else(|| anyhow::anyhow!("there is no group {id}"))
}

/// A new group, at the end.
pub fn create_group(conn: &Connection, name: &str) -> Result<Group> {
    let name = check_name(name)?;
    if find_name(conn, "groups", &name, None)?.is_some() {
        bail!("there is a group called {name} already");
    }
    conn.execute(
        "INSERT INTO groups (name, position) VALUES (?1, (SELECT coalesce(max(position), 0) + 1 FROM groups))",
        [&name],
    )?;
    group(conn, conn.last_insert_rowid())
}

pub fn rename_group(conn: &Connection, id: i64, name: &str) -> Result<Group> {
    let name = check_name(name)?;
    if !exists(conn, "groups", id)? {
        bail!("there is no group {id}");
    }
    if find_name(conn, "groups", &name, Some(id))?.is_some() {
        bail!("there is a group called {name} already");
    }
    conn.execute("UPDATE groups SET name = ?2 WHERE id = ?1", params![id, name])?;
    group(conn, id)
}

/// Put the groups in this order; groups not listed follow in their old order.
pub fn reorder_groups(conn: &Connection, ids: &[i64]) -> Result<Vec<Group>> {
    let tx = conn.unchecked_transaction()?;
    let old: Vec<i64> = groups(&tx)?.into_iter().map(|g| g.id).collect();
    let mut order: Vec<i64> = Vec::new();
    for id in ids {
        if !old.contains(id) {
            bail!("there is no group {id}");
        }
        if !order.contains(id) {
            order.push(*id);
        }
    }
    order.extend(old.iter().filter(|id| !order.contains(id)).copied().collect::<Vec<_>>());
    for (i, id) in order.iter().enumerate() {
        tx.execute("UPDATE groups SET position = ?2 WHERE id = ?1", params![id, i as i64 + 1])?;
    }
    tx.commit()?;
    groups(conn)
}

/// Delete a group; its people have no group afterwards.
pub fn delete_group(conn: &Connection, id: i64) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let people = tx.execute("UPDATE people SET group_id = NULL WHERE group_id = ?1", [id])? as u64;
    if tx.execute("DELETE FROM groups WHERE id = ?1", [id])? == 0 {
        bail!("there is no group {id}");
    }
    tx.commit()?;
    Ok(people)
}

// ---------------------------------------------------------------- people

#[derive(Debug, Clone, Serialize)]
pub struct Person {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub hidden: bool,
    /// Confirmed faces (hand-drawn ones and ones no longer found too).
    pub faces: u64,
    /// Present photos (one per content) with a confirmed face.
    pub photos: u64,
    /// Faces suggested for this person, and offered as "maybe".
    pub suggested: u64,
    pub maybe: u64,
    /// `cat`, `dog` or `pet` when most of the person's confirmed faces are
    /// pets (a pet); absent for everyone else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    /// The face shown for the person (`/api/faces/{id}/crop`): the chosen
    /// cover, else the largest confirmed face.
    pub cover: Option<i64>,
    /// Without a detected face to show: the largest face drawn by hand
    /// (`/api/faces/manual/{id}/crop`).
    pub cover_manual: Option<i64>,
}

struct PersonRow {
    id: i64,
    name: String,
    group_id: Option<i64>,
    hidden: bool,
    cover: Option<(String, [f64; 4])>,
    position: Option<i64>,
}

fn person_rows(conn: &Connection) -> Result<Vec<PersonRow>> {
    Ok(conn
        .prepare(
            "SELECT p.id, p.name, p.group_id, p.hidden, p.cover_key, p.cover_box, g.position
             FROM people p LEFT JOIN groups g ON g.id = p.group_id",
        )?
        .query_map([], |r| {
            let key: Option<String> = r.get(4)?;
            let b: Option<String> = r.get(5)?;
            let cover = key.zip(b.and_then(|b| serde_json::from_str::<[f64; 4]>(&b).ok()));
            Ok(PersonRow { id: r.get(0)?, name: r.get(1)?, group_id: r.get(2)?, hidden: r.get(3)?, cover, position: r.get(6)? })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// Everyone (hidden people only with `hidden`), by group order and name; no
/// group last.
pub fn people(conn: &Connection, hidden: bool) -> Result<Vec<Person>> {
    let v = View::load(conn)?;
    let mut rows = person_rows(conn)?;
    rows.retain(|p| hidden || !p.hidden);
    rows.sort_by(|a, b| {
        let order = |p: &PersonRow| (p.position.is_none(), p.position, p.name.to_lowercase(), p.id);
        order(a).cmp(&order(b))
    });
    #[derive(Default)]
    struct Counts {
        faces: u64,
        photos: HashSet<String>,
        suggested: u64,
        maybe: u64,
        largest: Option<(f64, i64)>,
        drawn: Option<(f64, i64)>,
    }
    let mut counts: HashMap<i64, Counts> = HashMap::new();
    for d in v.m.decisions.iter() {
        if let (Decision::Confirmed, Some(p)) = (d.decision, d.person) {
            let c = counts.entry(p).or_default();
            c.faces += 1;
            if v.is_present(&d.key) {
                c.photos.insert(d.key.clone());
                if d.manual && c.drawn.is_none_or(|(w, _)| d.b[2] > w) {
                    c.drawn = Some((d.b[2], d.id));
                }
            }
        }
    }
    for i in 0..v.m.faces.len() {
        let f = &v.m.faces[i];
        if let Some(d) = v.m.decided(i).filter(|d| d.decision == Decision::Confirmed)
            && let Some(p) = d.person
            && v.is_present(&f.key)
        {
            let c = counts.entry(p).or_default();
            if c.largest.is_none_or(|(px, _)| f.px > px) {
                c.largest = Some((f.px, f.id));
            }
        }
        if let Some((p, sim)) = v.suggestion(i) {
            let c = counts.entry(p).or_default();
            if sim >= f.thresholds().suggest {
                c.suggested += 1;
            } else {
                c.maybe += 1;
            }
        }
    }
    Ok(rows
        .into_iter()
        .map(|p| {
            let c = counts.remove(&p.id).unwrap_or_default();
            let chosen = p.cover.as_ref().and_then(|(key, b)| {
                (0..v.m.faces.len())
                    .filter(|&i| v.m.faces[i].key == *key && v.is_present(key))
                    .map(|i| (i, recognize::iou(v.m.faces[i].b, *b)))
                    .filter(|&(_, iou)| iou >= MATCH_IOU)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| v.m.faces[i].id)
            });
            let cover = chosen.or(c.largest.map(|(_, id)| id));
            // A hand-drawn face the person's picture was set to.
            let chosen_manual = p.cover.as_ref().and_then(|(key, b)| {
                v.m.decisions
                    .iter()
                    .find(|d| {
                        d.manual
                            && d.decision == Decision::Confirmed
                            && d.person == Some(p.id)
                            && d.key == *key
                            && v.is_present(key)
                            && recognize::iou(d.b, *b) >= MATCH_IOU
                    })
                    .map(|d| d.id)
            });
            Person {
                id: p.id,
                name: p.name,
                group_id: p.group_id,
                hidden: p.hidden,
                faces: c.faces,
                photos: c.photos.len() as u64,
                suggested: c.suggested,
                maybe: c.maybe,
                species: v.person_species.get(&p.id).cloned(),
                cover: if chosen.is_none() && chosen_manual.is_some() { None } else { cover },
                cover_manual: if chosen.is_none() && chosen_manual.is_some() {
                    chosen_manual
                } else if cover.is_none() {
                    c.drawn.map(|(_, id)| id)
                } else {
                    None
                },
            }
        })
        .collect())
}

pub fn person(conn: &Connection, id: i64) -> Result<Option<Person>> {
    Ok(people(conn, true)?.into_iter().find(|p| p.id == id))
}

fn person_or_fail(conn: &Connection, id: i64) -> Result<Person> {
    person(conn, id)?.ok_or_else(|| anyhow::anyhow!("there is no person {id}"))
}

/// A new person, optionally in a group.
pub fn create_person(conn: &Connection, name: &str, group_id: Option<i64>) -> Result<Person> {
    let name = check_name(name)?;
    if find_name(conn, "people", &name, None)?.is_some() {
        bail!("there is someone called {name} already");
    }
    if let Some(g) = group_id
        && !exists(conn, "groups", g)?
    {
        bail!("there is no group {g}");
    }
    conn.execute("INSERT INTO people (name, group_id) VALUES (?1, ?2)", params![name, group_id])?;
    person_or_fail(conn, conn.last_insert_rowid())
}

pub fn rename_person(conn: &Connection, id: i64, name: &str) -> Result<Person> {
    let name = check_name(name)?;
    if !exists(conn, "people", id)? {
        bail!("there is no person {id}");
    }
    if find_name(conn, "people", &name, Some(id))?.is_some() {
        bail!("there is someone called {name} already");
    }
    conn.execute("UPDATE people SET name = ?2 WHERE id = ?1", params![id, name])?;
    person_or_fail(conn, id)
}

/// Remove a person with no confirmed faces (a misspelled name): what
/// is left of them (rejections) goes with them. Someone with faces is
/// merged into the right person instead.
pub fn delete_person(conn: &Connection, id: i64) -> Result<()> {
    let p = person_or_fail(conn, id)?;
    if p.faces > 0 {
        bail!("{} has {} confirmed; merge them into someone instead", p.name, if p.faces == 1 { "a face".to_string() } else { format!("{} faces", p.faces) });
    }
    conn.execute("DELETE FROM people WHERE id = ?1", [id])?;
    Ok(())
}

pub fn hide_person(conn: &Connection, id: i64, hidden: bool) -> Result<Person> {
    if conn.execute("UPDATE people SET hidden = ?2 WHERE id = ?1", params![id, hidden])? == 0 {
        bail!("there is no person {id}");
    }
    person_or_fail(conn, id)
}

/// Put a person in a group, or in none.
pub fn set_group(conn: &Connection, id: i64, group_id: Option<i64>) -> Result<Person> {
    if let Some(g) = group_id
        && !exists(conn, "groups", g)?
    {
        bail!("there is no group {g}");
    }
    if conn.execute("UPDATE people SET group_id = ?2 WHERE id = ?1", params![id, group_id])? == 0 {
        bail!("there is no person {id}");
    }
    person_or_fail(conn, id)
}

/// Show this face (one of the person's confirmed faces) for the person.
pub fn set_cover(conn: &Connection, id: i64, face: i64) -> Result<Person> {
    let m = Matched::load(conn, None)?;
    let Some(i) = m.faces.iter().position(|f| f.id == face) else { bail!("there is no face {face}") };
    if !m.decided(i).is_some_and(|d| d.decision == Decision::Confirmed && d.person == Some(id)) {
        bail!("face {face} is not confirmed for this person");
    }
    let f = &m.faces[i];
    conn.execute(
        "UPDATE people SET cover_key = ?2, cover_box = ?3 WHERE id = ?1",
        params![id, f.key, serde_json::to_string(&f.b)?],
    )?;
    person_or_fail(conn, id)
}

/// Show the person's largest confirmed face in this photo (a file) for the
/// person: the right-click "Use as … picture" on a photo.
pub fn set_cover_from_file(conn: &Connection, id: i64, file: i64) -> Result<Person> {
    let key: String = conn
        .query_row("SELECT quick_hash FROM files WHERE id = ?1 AND missing_since IS NULL", [file], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("there is no file {file}"))?;
    if !exists(conn, "people", id)? {
        bail!("there is no person {id}");
    }
    let m = Matched::load(conn, Some(&key))?;
    let mut best: Option<(f64, [f64; 4])> = None;
    let mut consider = |b: [f64; 4]| {
        if best.is_none_or(|(area, _)| b[2] * b[3] > area) {
            best = Some((b[2] * b[3], b));
        }
    };
    for i in 0..m.faces.len() {
        if m.decided(i).is_some_and(|d| d.decision == Decision::Confirmed && d.person == Some(id)) {
            consider(m.faces[i].b);
        }
    }
    for (d, face) in m.decisions.iter().zip(&m.face_of) {
        if face.is_none() && d.manual && d.decision == Decision::Confirmed && d.person == Some(id) {
            consider(d.b);
        }
    }
    let Some((_, b)) = best else { bail!("this photo has no confirmed face of that person") };
    conn.execute("UPDATE people SET cover_key = ?2, cover_box = ?3 WHERE id = ?1", params![id, key, serde_json::to_string(&b)?])?;
    person_or_fail(conn, id)
}

/// Give everyone with confirmed faces a picture of their own, kept until
/// they choose another: the largest confirmed face (the face itself when
/// there is one), and a new one when the picture's face is not theirs any
/// more. Run after every change to people, so a person's picture does not
/// jump to a bigger face later. Returns how many people got a new picture.
pub fn ensure_covers(conn: &Connection) -> Result<u64> {
    if !table_exists(conn, "main", "people")? {
        return Ok(0);
    }
    let v = View::load(conn)?;
    let covers: HashMap<i64, (String, [f64; 4])> = person_rows(conn)?.into_iter().filter_map(|p| Some((p.id, p.cover?))).collect();
    let mut valid: HashSet<i64> = HashSet::new();
    let mut largest: HashMap<i64, (f64, String, [f64; 4])> = HashMap::new();
    let mut drawn: HashMap<i64, (f64, String, [f64; 4])> = HashMap::new();
    // A picture stays while its face is confirmed for the person, present or
    // not (a photo on a drive that is not plugged in must not lose it); a new
    // one is only taken from photos that are there.
    let mut note = |person: i64, key: &str, b: [f64; 4], size: f64, drawn_face: bool| {
        if covers.get(&person).is_some_and(|(k, cb)| k == key && recognize::iou(b, *cb) >= MATCH_IOU) {
            valid.insert(person);
        }
        if !v.is_present(key) {
            return;
        }
        let by = if drawn_face { &mut drawn } else { &mut largest };
        if by.get(&person).is_none_or(|(s, _, _)| size > *s) {
            by.insert(person, (size, key.to_string(), b));
        }
    };
    for i in 0..v.m.faces.len() {
        let f = &v.m.faces[i];
        if let Some(d) = v.m.decided(i).filter(|d| d.decision == Decision::Confirmed)
            && let Some(p) = d.person
        {
            note(p, &f.key, f.b, f.px, false);
        }
    }
    for d in v.unmatched() {
        let row = &v.m.decisions[d];
        if row.manual
            && row.decision == Decision::Confirmed
            && let Some(p) = row.person
        {
            note(p, &row.key, row.b, row.b[2], true);
        }
    }
    let mut changed = 0;
    for p in person_rows(conn)? {
        if valid.contains(&p.id) {
            continue;
        }
        if let Some((_, key, b)) = largest.remove(&p.id).or_else(|| drawn.remove(&p.id)) {
            conn.execute("UPDATE people SET cover_key = ?2, cover_box = ?3 WHERE id = ?1", params![p.id, key, serde_json::to_string(&b)?])?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Merge person `from` into `into`: all of `from`'s decisions become
/// `into`'s, and `from` is gone. `into` keeps its name, group and cover
/// (takes `from`'s where it has none).
pub fn merge_people(conn: &Connection, from: i64, into: i64) -> Result<Person> {
    if from == into {
        bail!("cannot merge someone with themselves");
    }
    let rows = person_rows(conn)?;
    let (Some(a), Some(b)) = (rows.iter().find(|p| p.id == from), rows.iter().find(|p| p.id == into)) else {
        bail!("there is no such person");
    };
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE face_decisions SET person_id = ?2 WHERE person_id = ?1", params![from, into])?;
    if b.group_id.is_none() && a.group_id.is_some() {
        tx.execute("UPDATE people SET group_id = ?2 WHERE id = ?1", params![into, a.group_id])?;
    }
    if b.cover.is_none() && a.cover.is_some() {
        tx.execute(
            "UPDATE people SET (cover_key, cover_box) = (SELECT cover_key, cover_box FROM people WHERE id = ?2) WHERE id = ?1",
            params![into, from],
        )?;
    }
    tx.execute("DELETE FROM people WHERE id = ?1", [from])?;
    // A face confirmed for one and rejected for the other: the
    // confirmation wins. A face rejected for both: one rejection.
    let m = Matched::load(&tx, None)?;
    for i in 0..m.faces.len() {
        let confirmed = m.decided(i).is_some_and(|d| d.decision == Decision::Confirmed && d.person == Some(into));
        let mut kept = false;
        for d in m.rows_of(i) {
            let row = &m.decisions[d];
            if row.decision == Decision::Rejected && row.person == Some(into) {
                if confirmed || kept {
                    tx.execute("DELETE FROM face_decisions WHERE id = ?1", [row.id])?;
                }
                kept = true;
            }
        }
    }
    tx.commit()?;
    person_or_fail(conn, into)
}

/// Who a request means: an existing person, or one with this name (made if
/// there is none).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Who {
    pub person_id: Option<i64>,
    pub name: Option<String>,
}

fn resolve(conn: &Connection, who: &Who) -> Result<i64> {
    match (who.person_id, who.name.as_deref()) {
        (Some(id), _) => {
            if !exists(conn, "people", id)? {
                bail!("there is no person {id}");
            }
            Ok(id)
        }
        (None, Some(name)) => {
            let name = check_name(name)?;
            match find_name(conn, "people", &name, None)? {
                Some(id) => Ok(id),
                None => {
                    conn.execute("INSERT INTO people (name) VALUES (?1)", [&name])?;
                    Ok(conn.last_insert_rowid())
                }
            }
        }
        (None, None) => bail!("who is it? (a person or a name)"),
    }
}

// ---------------------------------------------------------------- decisions

/// What to do with faces.
#[derive(Debug, Clone)]
pub enum Action {
    /// Accept the suggestion (suggested or maybe).
    Confirm,
    /// Not this person (the suggested one if `None`).
    Reject(Option<i64>),
    /// This is the person (a name makes a new person if needed).
    Assign(Who),
    /// A stranger nobody needs to name.
    Ignore,
    /// Not a face at all.
    NotFace,
    /// Forget every decision about the face.
    Undo,
    /// Forget that the face is not this person (undo a rejection only).
    Unreject(i64),
}

#[derive(Debug, Clone, Serialize)]
pub struct Decided {
    /// Faces whose decisions changed.
    pub faces: u64,
    /// The person they were assigned to, confirmed for or rejected for.
    pub person: Option<PersonRef>,
    /// After an action on some faces of a cluster: what is left of it
    /// (`null` if nothing is).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<Option<ClusterRef>>,
}

/// A cluster as an action on it leaves it.
#[derive(Debug, Clone, Serialize)]
pub struct ClusterRef {
    pub id: i64,
    pub generation: i64,
    pub size: u64,
}

/// Apply `action` to detected faces (`recog.faces` ids; unknown ids are
/// skipped) in one transaction.
pub fn decide(conn: &Connection, faces: &[i64], action: &Action) -> Result<Decided> {
    let tx = conn.unchecked_transaction()?;
    let assign_to = match action {
        Action::Assign(who) => Some(resolve(&tx, who)?),
        _ => None,
    };
    let v = View::load(&tx)?;
    let by_id: HashMap<i64, usize> = v.m.faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let mut seen = HashSet::new();
    let mut changed = 0;
    let mut person_seen = assign_to;
    let now = db::now();
    let insert = |tx: &Connection, f: &Detected, person: Option<i64>, d: Decision| -> Result<()> {
        tx.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at, species)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
            params![f.key, f.b[0], f.b[1], f.b[2], f.b[3], person, d.as_str(), now, f.species],
        )?;
        Ok(())
    };
    let delete = |tx: &Connection, rows: &[usize], keep: &dyn Fn(&DecisionRow) -> bool| -> Result<()> {
        for &d in rows {
            if !keep(&v.m.decisions[d]) {
                tx.execute("DELETE FROM face_decisions WHERE id = ?1", [v.m.decisions[d].id])?;
            }
        }
        Ok(())
    };
    for id in faces {
        let Some(&i) = by_id.get(id) else { continue };
        if !seen.insert(i) {
            continue;
        }
        let f = &v.m.faces[i];
        let rows = v.m.rows_of(i);
        let decided = v.m.decided(i);
        let is_rejection = |r: &DecisionRow| r.decision == Decision::Rejected;
        if let Some(d) = decided.filter(|d| d.manual) {
            // The face was drawn by hand over this one.
            match action {
                Action::Assign(_) => {
                    tx.execute("UPDATE face_decisions SET person_id = ?2, at = ?3 WHERE id = ?1", params![d.id, assign_to, now])?;
                }
                Action::Undo => delete(&tx, &rows, &|_| false)?,
                // Decided already: nothing to confirm; never rejected.
                Action::Confirm | Action::Unreject(_) => continue,
                _ => bail!("face {id} was drawn by hand; it can only be named again or deleted"),
            }
            changed += 1;
            continue;
        }
        match action {
            Action::Confirm | Action::Assign(_) => {
                let person = match assign_to {
                    Some(p) => p,
                    None => match v.suggestion(i) {
                        Some((p, _)) => p,
                        // Decided already: nothing to confirm.
                        None if decided.is_some() => continue,
                        None => bail!("there is nobody suggested for face {id}"),
                    },
                };
                person_seen = person_seen.or(Some(person));
                if decided.is_some_and(|d| d.decision == Decision::Confirmed && d.person == Some(person)) {
                    continue;
                }
                delete(&tx, &rows, &|r| is_rejection(r) && r.person != Some(person))?;
                insert(&tx, f, Some(person), Decision::Confirmed)?;
            }
            Action::Reject(who) => {
                let person = match who.or_else(|| v.suggestion(i).map(|s| s.0)) {
                    Some(p) => p,
                    None => bail!("there is nobody suggested for face {id}"),
                };
                if !v.names.contains_key(&person) {
                    bail!("there is no person {person}");
                }
                person_seen = person_seen.or(Some(person));
                if v.m.states[i].rejected.contains(&person) {
                    continue;
                }
                delete(&tx, &rows, &|r| !(r.decision == Decision::Confirmed && r.person == Some(person)))?;
                insert(&tx, f, Some(person), Decision::Rejected)?;
            }
            Action::Ignore => {
                if decided.is_some_and(|d| d.decision == Decision::Ignored) {
                    continue;
                }
                delete(&tx, &rows, &is_rejection)?;
                insert(&tx, f, None, Decision::Ignored)?;
            }
            Action::NotFace => {
                if decided.is_some_and(|d| d.decision == Decision::NotFace) {
                    continue;
                }
                delete(&tx, &rows, &|_| false)?;
                insert(&tx, f, None, Decision::NotFace)?;
            }
            Action::Undo => {
                if rows.is_empty() {
                    continue;
                }
                delete(&tx, &rows, &|_| false)?;
            }
            Action::Unreject(person) => {
                person_seen = person_seen.or(Some(*person));
                if !v.m.states[i].rejected.contains(person) {
                    continue;
                }
                delete(&tx, &rows, &|r| !(r.decision == Decision::Rejected && r.person == Some(*person)))?;
            }
        }
        changed += 1;
    }
    let person = match person_seen {
        Some(p) => Some(PersonRef { id: p, name: tx.query_row("SELECT name FROM people WHERE id = ?1", [p], |r| r.get(0))? }),
        None => None,
    };
    tx.commit()?;
    Ok(Decided { faces: changed, person, cluster: None })
}

/// Say what kind of pet detected faces (`recog.faces` ids; people's faces and
/// unknown ids are skipped) are, `cat` or `dog`, when the detector got it
/// wrong. Decisions already made about the pets follow, so a confirmed pet
/// stays confirmed. Returns how many pets changed.
pub fn set_species(conn: &Connection, faces: &[i64], species: &str) -> Result<u64> {
    if !crate::pets::is_species(species) {
        bail!("the species is cat or dog, not {species:?}");
    }
    let tx = conn.unchecked_transaction()?;
    let v = View::load(&tx)?;
    let by_id: HashMap<i64, usize> = v.m.faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let overrides = load_species(&tx, None)?;
    let mut seen = HashSet::new();
    let mut changed = 0;
    for id in faces {
        let Some(&i) = by_id.get(id) else { continue };
        let f = &v.m.faces[i];
        if f.species.is_none() || f.species.as_deref() == Some(species) || !seen.insert(i) {
            continue;
        }
        // Replace what was said about this pet before.
        for o in overrides.iter().filter(|o| o.key == f.key && recognize::iou(o.b, f.b) >= MATCH_IOU) {
            tx.execute("DELETE FROM face_species WHERE id = ?1", [o.id])?;
        }
        tx.execute(
            "INSERT INTO face_species (key, x, y, w, h, species, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![f.key, f.b[0], f.b[1], f.b[2], f.b[3], species, db::now()],
        )?;
        // Decisions about it (made while it was the other species) stay about it.
        for d in v.m.rows_of(i) {
            let row = &v.m.decisions[d];
            if row.species.as_deref().is_some_and(crate::pets::is_species) {
                tx.execute("UPDATE face_decisions SET species = ?2 WHERE id = ?1", params![row.id, species])?;
            }
        }
        changed += 1;
    }
    tx.commit()?;
    Ok(changed)
}

/// Add a face drawn by hand on a photo (a pet of the given species, `cat` or
/// `dog`, or `pet` for one of no species, if the user says it is one): always
/// confirmed, with a person. Returns its id (`manual` in the API).
pub fn add_manual(conn: &Connection, file: i64, b: [f64; 4], who: &Who, species: Option<&str>) -> Result<i64> {
    if let Some(s) = species {
        if !crate::pets::is_species(s) && s != crate::pets::PET {
            bail!("the species is cat or dog, not {s:?}");
        }
    }
    let ok = b.iter().all(|v| v.is_finite()) && b[0] >= 0.0 && b[1] >= 0.0 && b[2] > 0.0 && b[3] > 0.0;
    if !ok || b[0] + b[2] > 1.0 + 1e-9 || b[1] + b[3] > 1.0 + 1e-9 {
        bail!("the box must lie inside the picture (fractions of its width and height)");
    }
    let tx = conn.unchecked_transaction()?;
    let key: String = tx
        .query_row("SELECT quick_hash FROM files WHERE id = ?1 AND missing_since IS NULL", [file], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("there is no file {file}"))?;
    let person = resolve(&tx, who)?;
    tx.execute(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at, species)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'confirmed', 1, ?7, ?8)",
        params![key, b[0], b[1], b[2], b[3], person, db::now(), species],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// Delete hand-drawn faces (`manual` ids). Returns how many went.
pub fn delete_manual(conn: &Connection, ids: &[i64]) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for id in ids {
        n += tx.execute("DELETE FROM face_decisions WHERE id = ?1 AND manual = 1", [id])? as u64;
    }
    tx.commit()?;
    Ok(n)
}

// ---------------------------------------------------------------- lists

#[derive(Debug, Clone, Serialize)]
pub struct FaceList {
    pub total: u64,
    pub faces: Vec<FaceItem>,
}

fn page(mut items: Vec<FaceItem>, offset: usize, limit: usize) -> FaceList {
    let total = items.len() as u64;
    let faces = items.drain(offset.min(items.len())..).take(limit).collect();
    FaceList { total, faces }
}

/// Which faces of a person to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Which {
    /// Confirmed, hand-drawn and no longer found ones, largest first.
    Confirmed,
    /// Suggested (≥ `SUGGEST_SIM`), most similar first.
    Suggested,
    /// Offered as "maybe" (`MAYBE_SIM`–`SUGGEST_SIM`).
    Maybe,
    /// Rejected for this person (to undo).
    Rejected,
}

/// Faces of a person in present photos (`None` if there is no such person).
pub fn person_faces(conn: &Connection, id: i64, which: Which, offset: usize, limit: usize) -> Result<Option<FaceList>> {
    if !exists(conn, "people", id)? {
        return Ok(None);
    }
    let v = View::load(conn)?;
    let mut items: Vec<FaceItem> = Vec::new();
    for i in 0..v.m.faces.len() {
        if !v.is_present(&v.m.faces[i].key) {
            continue;
        }
        let take = match which {
            Which::Confirmed => v.m.decided(i).is_some_and(|d| d.decision == Decision::Confirmed && d.person == Some(id)),
            Which::Suggested => v.suggestion(i).is_some_and(|(p, s)| p == id && s >= v.m.faces[i].thresholds().suggest),
            Which::Maybe => v.suggestion(i).is_some_and(|(p, s)| p == id && s < v.m.faces[i].thresholds().suggest),
            Which::Rejected => v.m.states[i].rejected.contains(&id),
        };
        if take {
            items.push(v.item(i));
        }
    }
    match which {
        Which::Confirmed => {
            items.sort_by(|a, b| b.px.total_cmp(&a.px).then(a.id.cmp(&b.id)));
            let mut more: Vec<FaceItem> = v
                .unmatched()
                .filter(|&d| {
                    let r = &v.m.decisions[d];
                    r.decision == Decision::Confirmed && r.person == Some(id) && v.is_present(&r.key)
                })
                .map(|d| v.unmatched_item(d))
                .collect();
            more.sort_by_key(|f| f.manual.is_none());
            items.extend(more);
        }
        _ => items.sort_by(|a, b| b.similarity.unwrap_or(0.0).total_cmp(&a.similarity.unwrap_or(0.0)).then(a.id.cmp(&b.id))),
    }
    Ok(Some(page(items, offset, limit)))
}

#[derive(Debug, Clone, Serialize)]
pub struct ClusterSuggestion {
    pub person: PersonRef,
    /// Faces of the cluster suggested for them (≥ `SUGGEST_SIM`).
    pub faces: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cluster {
    /// Its number in this computation of the cache (1 is the largest);
    /// changes whenever the clusters are recomputed.
    pub id: i64,
    /// Changes only when the cluster's faces change (a number below 2^48,
    /// so JavaScript keeps it exact). Actions on the cluster pass it, so
    /// they mean the faces that were shown even after the clusters were
    /// renumbered.
    pub generation: i64,
    pub size: u64,
    /// The largest faces first.
    pub faces: Vec<FaceItem>,
    /// The person most of its faces are suggested for.
    pub suggestion: Option<ClusterSuggestion>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Clusters {
    /// Clusters (a face alone counts as one).
    pub total: u64,
    /// Faces in them: faces large enough without a decision.
    pub unnamed: u64,
    /// Faces in clusters of up to `SMALL_CLUSTER` faces (whatever `size`
    /// asked for): the ones listed one by one.
    pub small_faces: u64,
    pub clusters: Vec<Cluster>,
}

/// Faces without a decision, by cluster: (cluster, face indexes largest
/// first), largest cluster first.
fn grouped(v: &View, kind: Option<Space>) -> Vec<(i64, Vec<usize>)> {
    let mut by: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for i in 0..v.m.faces.len() {
        if kind.is_some_and(|k| k != v.m.faces[i].space()) {
            continue;
        }
        if let Some(c) = v.cluster(i)
            && v.is_present(&v.m.faces[i].key)
        {
            by.entry(c).or_default().push(i);
        }
    }
    let mut out: Vec<(i64, Vec<usize>)> = by.into_iter().collect();
    for (_, faces) in &mut out {
        faces.sort_by(|&a, &b| v.m.faces[b].px.total_cmp(&v.m.faces[a].px).then(a.cmp(&b)));
    }
    out.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    out
}

/// The generation of a cluster: a hash of its faces (ids and contents).
fn generation_of(v: &View, faces: &[usize]) -> i64 {
    let mut ids: Vec<(i64, &str)> = faces.iter().map(|&i| (v.m.faces[i].id, v.m.faces[i].key.as_str())).collect();
    ids.sort();
    let mut h = blake3::Hasher::new();
    for (id, key) in ids {
        h.update(&id.to_le_bytes());
        h.update(key.as_bytes());
        h.update(&[0]);
    }
    let b = h.finalize();
    let mut n = [0u8; 8];
    n[..6].copy_from_slice(&b.as_bytes()[..6]);
    i64::from_le_bytes(n)
}

/// Clusters of up to this many faces are "small": listed as single faces to
/// pick one by one, not as cards.
pub const SMALL_CLUSTER: usize = 2;

/// Which clusters to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    All,
    /// More than `SMALL_CLUSTER` faces.
    Large,
    /// At most `SMALL_CLUSTER` faces.
    Small,
}

/// The clusters of faces without a decision, largest first, with up to
/// `samples` faces each; only those of one space (people's faces or
/// pets) if `kind` is given. Faces and pets never share a cluster.
pub fn clusters(conn: &Connection, offset: usize, limit: usize, samples: usize, kind: Option<Space>, size: Size) -> Result<Clusters> {
    let v = View::load(conn)?;
    let mut all = grouped(&v, kind);
    let singles: u64 = all.iter().filter(|c| c.1.len() <= SMALL_CLUSTER).map(|c| c.1.len() as u64).sum();
    all.retain(|c| match size {
        Size::All => true,
        Size::Large => c.1.len() > SMALL_CLUSTER,
        Size::Small => c.1.len() <= SMALL_CLUSTER,
    });
    if size == Size::Small {
        // Largest faces first, so the ones that can be recognised come first.
        all.sort_by(|a, b| v.m.faces[b.1[0]].px.total_cmp(&v.m.faces[a.1[0]].px).then(a.0.cmp(&b.0)));
    }
    let unnamed = all.iter().map(|c| c.1.len() as u64).sum();
    let total = all.len() as u64;
    let clusters = all
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(id, faces)| {
            let mut votes: HashMap<i64, u64> = HashMap::new();
            for &i in &faces {
                if let Some((p, _)) = v.suggestion(i).filter(|s| s.1 >= v.m.faces[i].thresholds().suggest) {
                    *votes.entry(p).or_default() += 1;
                }
            }
            let suggestion = votes
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .and_then(|(p, n)| v.person(Some(p)).map(|person| ClusterSuggestion { person, faces: n }));
            Cluster {
                id,
                generation: generation_of(&v, &faces),
                size: faces.len() as u64,
                faces: faces.iter().take(samples).map(|&i| v.item(i)).collect(),
                suggestion,
            }
        })
        .collect();
    Ok(Clusters { total, unnamed, small_faces: singles, clusters })
}

/// All faces of a cluster: of the one whose faces have `generation` (its
/// number may have changed since; `Stale` if no cluster has those faces
/// any more), else of cluster number `id` (`None` if there is none).
pub fn cluster_faces(conn: &Connection, id: i64, generation: Option<i64>) -> Result<Option<Vec<FaceItem>>> {
    let v = View::load(conn)?;
    let all = grouped(&v, None);
    let found = match generation {
        Some(g) => {
            let mut matching = all.iter().filter(|c| generation_of(&v, &c.1) == g);
            let first = matching.next();
            // The same faces under the number shown, if it is still that.
            Some(all.iter().find(|c| c.0 == id && generation_of(&v, &c.1) == g).or(first).ok_or(Stale)?)
        }
        None => all.iter().find(|c| c.0 == id),
    };
    Ok(found.map(|(_, faces)| faces.iter().map(|&i| v.item(i)).collect()))
}

/// What is left of the cluster of these faces (the ones of a cluster an
/// action left undecided), as it is shown now.
pub fn cluster_of(conn: &Connection, faces: &[i64]) -> Result<Option<ClusterRef>> {
    let v = View::load(conn)?;
    Ok(grouped(&v, None)
        .into_iter()
        .find(|(_, members)| members.iter().any(|&i| faces.contains(&v.m.faces[i].id)))
        .map(|(id, members)| ClusterRef { id, generation: generation_of(&v, &members), size: members.len() as u64 }))
}

/// The face ids of a cluster for an action on it: `faces` if given (a
/// split: some of its faces), else all of them.
pub fn cluster_face_ids(conn: &Connection, id: i64, generation: Option<i64>, faces: Option<&[i64]>) -> Result<Vec<i64>> {
    let all: Vec<i64> = cluster_faces(conn, id, generation)?.unwrap_or_default().into_iter().filter_map(|f| f.id).collect();
    if all.is_empty() {
        bail!("there is no cluster {id} (any more)");
    }
    Ok(match faces {
        Some(chosen) => {
            if let Some(f) = chosen.iter().find(|f| !all.contains(f)) {
                bail!("face {f} is not in cluster {id}");
            }
            chosen.to_vec()
        }
        None => all,
    })
}

// ---------------------------------------------------------------- timeline

/// Contents with a confirmed face of any of these people (for the
/// timeline and search).
pub fn keys_of_people(conn: &Connection, ids: &[i64]) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT key FROM face_decisions WHERE person_id = ?1 AND decision = 'confirmed'")?;
    let mut keys = HashSet::new();
    for id in ids {
        keys.extend(stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?);
    }
    Ok(keys)
}

/// Contents with a pet of this species in them (`pet`: of any species), named
/// or not: detected cats and dogs (unless marked "not a pet"), and pets drawn
/// by hand. A pet that is Layka's counts as the dog Layka is, whatever the
/// detector thought. For searching; only reads, and `recog` need not be
/// attached.
pub fn keys_of_pets(conn: &Connection, species: &str) -> Result<HashSet<String>> {
    let mut keys = HashSet::new();
    let any = species == crate::pets::PET;
    let of_people = person_species(conn)?;
    let wanted = |own: Option<&str>, person: Option<i64>| {
        effective_species(own, person, &of_people).is_some_and(|s| any || s == species)
    };
    if table_exists(conn, "recog", "faces")? && db::has_column(conn, "recog", "faces", "species")? {
        let m = Matched::load(conn, None)?;
        for i in 0..m.faces.len() {
            let f = &m.faces[i];
            let decided = m.decided(i);
            if f.species.is_none() || decided.is_some_and(|d| d.decision == Decision::NotFace) {
                continue;
            }
            let person = decided.filter(|d| d.decision == Decision::Confirmed).and_then(|d| d.person);
            if wanted(f.species.as_deref(), person) {
                keys.insert(f.key.clone());
            }
        }
    }
    if table_exists(conn, "main", "face_decisions")? && db::has_column(conn, "main", "face_decisions", "species")? {
        let mut stmt = conn.prepare(
            "SELECT key, species, person_id FROM face_decisions WHERE decision = 'confirmed' AND species IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            if wanted(r.get::<_, Option<String>>(1)?.as_deref(), r.get(2)?) {
                keys.insert(r.get(0)?);
            }
        }
    }
    Ok(keys)
}

/// Every person (hidden ones too) with the contents they are confirmed on.
pub fn keys_by_person(conn: &Connection) -> Result<HashMap<i64, HashSet<String>>> {
    let mut out: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT person_id, key FROM face_decisions WHERE decision = 'confirmed' AND person_id IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        out.entry(r.get(0)?).or_default().insert(r.get(1)?);
    }
    Ok(out)
}

/// Names of people by id (unknown ids are left out).
pub fn person_names(conn: &Connection, ids: &[i64]) -> Result<Vec<PersonRef>> {
    let mut stmt = conn.prepare("SELECT name FROM people WHERE id = ?1")?;
    let mut out = Vec::new();
    for &id in ids {
        if let Some(name) = stmt.query_row([id], |r| r.get(0)).optional()? {
            out.push(PersonRef { id, name });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- userdata.json

#[derive(Debug, Serialize)]
pub struct UserGroup {
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Serialize)]
pub struct UserBox {
    pub key: String,
    #[serde(rename = "box")]
    pub b: [f64; 4],
}

#[derive(Debug, Serialize)]
pub struct UserPerson {
    pub name: String,
    pub group: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    pub cover: Option<UserBox>,
}

/// A file with a decision's content, so a decision can be matched again
/// even if the index is lost and the files moved.
#[derive(Debug, Clone, Serialize)]
pub struct UserFile {
    /// Relative to the library (NFC); where it was, for files in the trash.
    pub path: String,
    pub full_hash: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub in_trash: bool,
}

#[derive(Debug, Serialize)]
pub struct UserDecision {
    /// The person's name (`null` with `ignored` and `not_face`).
    pub person: Option<String>,
    pub decision: Decision,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub manual: bool,
    /// `quick_hash` of the content.
    pub key: String,
    /// x, y, w, h as fractions of the upright picture.
    #[serde(rename = "box")]
    pub b: [f64; 4],
    pub at: i64,
    /// `cat` or `dog` for a decision about a pet; absent for people's faces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    pub files: Vec<UserFile>,
}

#[derive(Debug, Serialize)]
pub struct UserPeople {
    pub groups: Vec<UserGroup>,
    pub people: Vec<UserPerson>,
    pub face_decisions: Vec<UserDecision>,
    /// What the user said a cat or dog the detector mistook is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pet_species: Vec<UserPetSpecies>,
}

#[derive(Debug, Serialize)]
pub struct UserPetSpecies {
    /// `quick_hash` of the content.
    pub key: String,
    #[serde(rename = "box")]
    pub b: [f64; 4],
    pub species: String,
    pub at: i64,
}

/// People, groups and decisions for `userdata.json`.
pub fn user_data(conn: &Connection) -> Result<UserPeople> {
    let groups: Vec<UserGroup> = conn
        .prepare("SELECT name, position FROM groups ORDER BY position, id")?
        .query_map([], |r| Ok(UserGroup { name: r.get(0)?, position: r.get(1)? }))?
        .collect::<rusqlite::Result<_>>()?;
    let mut people: Vec<UserPerson> = conn
        .prepare(
            "SELECT p.name, g.name, p.hidden, p.cover_key, p.cover_box FROM people p LEFT JOIN groups g ON g.id = p.group_id",
        )?
        .query_map([], |r| {
            let key: Option<String> = r.get(3)?;
            let b: Option<String> = r.get(4)?;
            let cover = key.zip(b.and_then(|b| serde_json::from_str::<[f64; 4]>(&b).ok())).map(|(key, b)| UserBox { key, b });
            Ok(UserPerson { name: r.get(0)?, group: r.get(1)?, hidden: r.get(2)?, cover })
        })?
        .collect::<rusqlite::Result<_>>()?;
    people.sort_by_key(|p| p.name.to_lowercase());

    let mut files: HashMap<String, Vec<UserFile>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT quick_hash, path_nfc, full_hash, 0 FROM files WHERE quick_hash IN (SELECT key FROM face_decisions)
             UNION ALL
             SELECT quick_hash, path_nfc, full_hash, 1 FROM trash WHERE quick_hash IN (SELECT key FROM face_decisions)
             ORDER BY 2",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            files
                .entry(r.get(0)?)
                .or_default()
                .push(UserFile { path: r.get(1)?, full_hash: r.get(2)?, in_trash: r.get(3)? });
        }
    }
    let names: HashMap<i64, String> = conn
        .prepare("SELECT id, name FROM people")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let face_decisions = load_decisions(conn, None)?
        .into_iter()
        .map(|d| UserDecision {
            person: d.person.and_then(|p| names.get(&p).cloned()),
            decision: d.decision,
            manual: d.manual,
            files: files.get(&d.key).cloned().unwrap_or_default(),
            key: d.key,
            b: d.b,
            at: d.at,
            species: d.species,
        })
        .collect();
    let pet_species = load_species(conn, None)?
        .into_iter()
        .map(|r| UserPetSpecies { key: r.key, b: r.b, species: r.species, at: r.at })
        .collect();
    Ok(UserPeople { groups, people, face_decisions, pet_species })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(id: i64, key: &str, b: [f64; 4]) -> Detected {
        Detected { id, key: key.into(), b, roll: 0, px: 100.0, score: 0.9, model: "m".into(), species: None }
    }

    fn decision(id: i64, key: &str, b: [f64; 4], d: Decision, person: Option<i64>, at: i64) -> DecisionRow {
        DecisionRow { id, key: key.into(), b, person, decision: d, manual: false, at, species: None }
    }

    /// A person's face and a pet with the very same box in one photo:
    /// each decision belongs to the face of its own kind.
    #[test]
    fn decisions_only_match_faces_of_their_own_kind() {
        let mut cat = face(2, "a", [0.1, 0.1, 0.2, 0.2]);
        cat.species = Some("cat".into());
        let faces = vec![face(1, "a", [0.1, 0.1, 0.2, 0.2]), cat];
        let mut about_cat = decision(10, "a", [0.1, 0.1, 0.2, 0.2], Decision::Confirmed, Some(7), 5);
        about_cat.species = Some("cat".into());
        let about_person = decision(11, "a", [0.1, 0.1, 0.2, 0.2], Decision::Confirmed, Some(8), 5);
        let m = Matched::new(faces, vec![about_cat, about_person]);
        assert_eq!(m.face_of, [Some(1), Some(0)]);
        assert_eq!(m.decided(0).unwrap().person, Some(8));
        assert_eq!(m.decided(1).unwrap().person, Some(7));
        // A pet decision never lands on a person's face when no pet is left.
        let mut lone = decision(12, "b", [0.1, 0.1, 0.2, 0.2], Decision::Confirmed, Some(7), 5);
        lone.species = Some("dog".into());
        let m = Matched::new(vec![face(3, "b", [0.1, 0.1, 0.2, 0.2])], vec![lone]);
        assert_eq!(m.face_of, [None]);
    }

    #[test]
    fn decisions_go_to_the_best_overlapping_face_of_their_content() {
        let faces = vec![
            face(1, "a", [0.1, 0.1, 0.2, 0.2]),
            face(2, "a", [0.5, 0.5, 0.2, 0.2]),
            face(3, "b", [0.1, 0.1, 0.2, 0.2]),
        ];
        let decisions = vec![
            // Shifted a little: still face 1.
            decision(10, "a", [0.12, 0.11, 0.2, 0.2], Decision::Confirmed, Some(7), 5),
            // Too far from any face: kept, matched to none.
            decision(11, "a", [0.3, 0.3, 0.2, 0.2], Decision::Confirmed, Some(7), 5),
            // Another content with the same box.
            decision(12, "b", [0.1, 0.1, 0.2, 0.2], Decision::Rejected, Some(7), 5),
            decision(13, "b", [0.1, 0.1, 0.2, 0.2], Decision::Ignored, None, 6),
            decision(14, "b", [0.1, 0.1, 0.2, 0.2], Decision::NotFace, None, 4),
        ];
        let m = Matched::new(faces, decisions);
        assert_eq!(m.face_of, [Some(0), None, Some(2), Some(2), Some(2)]);
        assert_eq!(m.decided(0).unwrap().id, 10);
        assert!(m.decided(1).is_none());
        // The latest of several decisions counts.
        assert_eq!(m.decided(2).unwrap().decision, Decision::Ignored);
        assert_eq!(m.states[2].rejected, [7]);
        assert!(m.not_faces().is_empty());
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  Aurelia ").unwrap(), "Aurelia");
        assert!(check_name(" ").is_err());
        assert!(check_name("a\tb").is_err());
        assert!(check_name(&"x".repeat(MAX_NAME + 1)).is_err());
    }
}
