//! Forgetting what recognition found: the faces, or the cats and dogs
//! (`recognition.db`), and on request what the user decided about them
//! (`library.db`: confirmed, rejected, ignored, "not a face", hand-drawn
//! boxes, names).
//!
//! Two levels, each for one space (`pets::Space`):
//!
//! - **Automatic finds**: the boxes and embeddings the recognizer found, the
//!   memory of having looked at each photo, the clusters built from them.
//!   Nothing the user did is touched: decisions and names are kept by content
//!   and box in `library.db`, so they take hold again when "Recognize" reads
//!   the photos again. Until then the finds are gone, so the people pages are
//!   empty and nothing is suggested.
//! - **Everything**: that and the user's decisions in the space, the people
//!   who have none left, and the groups that lose all their people. This
//!   cannot be undone: the photos hold none of it.
//!
//! No photo is read or changed.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::pets::Space;
use crate::recognize::{EMBED, EMBED_PETS, FACES, FACES_ROT, PETS};

/// What forgetting a space takes, or took.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Forgot {
    /// Faces (or pets) the recognizer found.
    pub finds: u64,
    /// Photos looked at (the memory of it, so they are read again).
    pub photos: u64,
    /// What the user decided about boxes in this space (`everything` only).
    pub decisions: u64,
    /// People (or pets) with a name who would be left with nothing
    /// (`everything` only).
    pub people: u64,
}

fn tasks(space: Space) -> &'static [&'static str] {
    match space {
        Space::Faces => &[FACES, FACES_ROT, EMBED],
        Space::Pets => &[PETS, EMBED_PETS],
    }
}

fn species_clause(space: Space) -> &'static str {
    match space {
        Space::Faces => "species IS NULL",
        Space::Pets => "species IS NOT NULL",
    }
}

/// The decisions of a space, with the person each names: a decision is a pet's
/// when it carries a species, or when its person is a cat or a dog.
fn decisions_of(conn: &Connection, space: Space) -> Result<Vec<(i64, Option<i64>)>> {
    if !crate::people::table_exists(conn, "main", "face_decisions")? {
        return Ok(Vec::new());
    }
    let pet_persons = crate::people::person_species(conn)?;
    let has_species = crate::db::has_column(conn, "main", "face_decisions", "species")?;
    let sql = if has_species {
        "SELECT id, person_id, species FROM face_decisions"
    } else {
        "SELECT id, person_id, NULL FROM face_decisions"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let person: Option<i64> = r.get(1)?;
        let species: Option<String> = r.get(2)?;
        let is_pet = species.is_some() || person.is_some_and(|p| pet_persons.contains_key(&p));
        if is_pet == (space == Space::Pets) {
            out.push((r.get(0)?, person));
        }
    }
    Ok(out)
}

/// People who are left with no decision at all once `gone` are deleted.
fn people_left_empty(conn: &Connection, gone: &[(i64, Option<i64>)]) -> Result<Vec<i64>> {
    let gone_ids: HashSet<i64> = gone.iter().map(|d| d.0).collect();
    let named: HashSet<i64> = gone.iter().filter_map(|d| d.1).collect();
    let mut remaining: HashMap<i64, u64> = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, person_id FROM face_decisions WHERE person_id IS NOT NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (id, person): (i64, i64) = (r.get(0)?, r.get(1)?);
        if !gone_ids.contains(&id) {
            *remaining.entry(person).or_default() += 1;
        }
    }
    let mut empty: Vec<i64> = named.into_iter().filter(|p| !remaining.contains_key(p)).collect();
    empty.sort_unstable();
    Ok(empty)
}

fn recog_ready(conn: &Connection) -> bool {
    crate::people::table_exists(conn, "recog", "faces").unwrap_or(false)
}

/// What forgetting would take, to say so before asking.
pub fn counts(conn: &Connection, space: Space) -> Result<Forgot> {
    let mut f = Forgot::default();
    if recog_ready(conn) {
        let sp = species_clause(space);
        f.finds = conn.query_row(&format!("SELECT count(*) FROM recog.faces WHERE {sp}"), [], |r| r.get::<_, i64>(0))? as u64;
        let list = tasks(space).iter().map(|t| format!("'{t}'")).collect::<Vec<_>>().join(",");
        f.photos = conn.query_row(
            &format!("SELECT count(DISTINCT key) FROM recog.looked WHERE task IN ({list})"),
            [],
            |r| r.get::<_, i64>(0),
        )? as u64;
    }
    let gone = decisions_of(conn, space)?;
    f.decisions = gone.len() as u64;
    f.people = if gone.is_empty() { 0 } else { people_left_empty(conn, &gone)?.len() as u64 };
    Ok(f)
}

/// Forget the automatic finds of one space, and with `everything` the user's
/// decisions in it as well (see the module's text).
pub fn forget(conn: &Connection, space: Space, everything: bool) -> Result<Forgot> {
    let before = counts(conn, space)?;
    let tx = conn.unchecked_transaction()?;
    if recog_ready(conn) {
        let sp = species_clause(space);
        for table in ["neighbours", "clusters"] {
            if crate::people::table_exists(conn, "recog", table)? {
                tx.execute(&format!("DELETE FROM recog.{table} WHERE face IN (SELECT id FROM recog.faces WHERE {sp})"), [])?;
            }
        }
        tx.execute(&format!("DELETE FROM recog.faces WHERE {sp}"), [])?;
        if space == Space::Pets && crate::people::table_exists(conn, "recog", "bodies")? {
            tx.execute("DELETE FROM recog.bodies", [])?;
        }
        if crate::people::table_exists(conn, "recog", "drawn")? {
            let embed = if space == Space::Faces { EMBED } else { EMBED_PETS };
            tx.execute("DELETE FROM recog.drawn WHERE key IN (SELECT key FROM recog.looked WHERE task = ?1)", params![embed])?;
        }
        for t in tasks(space) {
            tx.execute("DELETE FROM recog.looked WHERE task = ?1", params![t])?;
        }
    }
    if everything {
        let gone = decisions_of(conn, space)?;
        let empty = people_left_empty(conn, &gone)?;
        for (id, _) in &gone {
            tx.execute("DELETE FROM face_decisions WHERE id = ?1", params![id])?;
        }
        if space == Space::Pets && crate::people::table_exists(conn, "main", "face_species")? {
            tx.execute("DELETE FROM face_species", [])?;
        }
        let mut groups: HashSet<i64> = HashSet::new();
        for p in &empty {
            if let Some(g) = tx.query_row("SELECT group_id FROM people WHERE id = ?1", params![p], |r| r.get::<_, Option<i64>>(0)).ok().flatten() {
                groups.insert(g);
            }
            tx.execute("DELETE FROM people WHERE id = ?1", params![p])?;
        }
        for g in groups {
            tx.execute("DELETE FROM groups WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM people WHERE group_id = ?1)", params![g])?;
        }
    }
    tx.commit()?;
    if recog_ready(conn) {
        // The embeddings were most of the file.
        let _ = conn.execute_batch("VACUUM recog");
    }
    Ok(if everything { before } else { Forgot { decisions: 0, people: 0, ..before } })
}
