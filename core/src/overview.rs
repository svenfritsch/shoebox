//! "Recognition stats": how every kind of recognition went on a drive, one
//! short block each: faces (with the turned-photos pass), cats and dogs, and
//! the words in photos. The Control Panel button "Recognition stats" and
//! `shoebox stats` print it; the detailed pages stay `shoebox faces stats
//! [--pets]` and `shoebox text stats`. Only reads.

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::faces;
use crate::pets::Space;
use crate::text;

/// One kind of recognition on one drive.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Part {
    /// Photos on the drive (one per content).
    pub photos: u64,
    /// Looked at, and of those failed.
    pub looked: u64,
    pub failed: u64,
    /// Still to look at.
    pub to_do: u64,
    /// Faces, pets or shown lines of text found.
    pub finds: u64,
    /// Photos with at least one find.
    pub with_finds: u64,
    /// Faces: the turned pass (looked at, found, failed).
    pub rotated_looked: u64,
    pub rotated_found: u64,
    pub rotated_failed: u64,
    /// Marked "not a face" by the user; not matched to anybody yet.
    pub not_faces: u64,
    pub unmatched: u64,
    /// Named people (or pets) with at least one confirmed face.
    pub named: u64,
    /// The most common error and how often.
    pub error: Option<(String, u64)>,
    /// Text: hidden lines, lines stored in all.
    pub hidden: u64,
    pub stored: u64,
    pub model: Option<String>,
    /// Unix seconds of the last run.
    pub last_run: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Overview {
    /// `false` before the first recognition run on this drive.
    pub started: bool,
    pub faces: Part,
    pub pets: Part,
    pub text: Part,
}

fn from_faces(s: &faces::Stats, with_finds: u64, named: u64) -> Part {
    Part {
        photos: s.photos,
        looked: s.looked,
        failed: s.failed,
        to_do: s.photos.saturating_sub(s.looked),
        finds: s.faces,
        with_finds,
        rotated_looked: s.rotated_looked,
        rotated_found: s.rotated_faces,
        rotated_failed: s.rotated_failed,
        not_faces: s.not_faces,
        unmatched: s.unmatched,
        named,
        error: s.errors.first().map(|e| (e.message.clone(), e.count)),
        last_run: s.runs.iter().filter(|r| r.kind == "faces" || r.kind == "pets").map(|r| r.started_at).max(),
        ..Part::default()
    }
}

/// People and pets with a name and at least one confirmed face.
fn named(conn: &Connection) -> Result<(u64, u64)> {
    if !crate::people::table_exists(conn, "main", "face_decisions")? {
        return Ok((0, 0));
    }
    let pet_persons = crate::people::person_species(conn)?;
    let mut stmt = conn.prepare("SELECT DISTINCT person_id FROM face_decisions WHERE decision = 'confirmed' AND person_id IS NOT NULL")?;
    let ids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let pets = ids.iter().filter(|i| pet_persons.contains_key(i)).count() as u64;
    Ok((ids.len() as u64 - pets, pets))
}

fn photos_with(conn: &Connection, space: Space) -> Result<u64> {
    Ok(conn.query_row(
        &format!("SELECT count(DISTINCT f.key) FROM recog.faces f WHERE {}", space.filter()),
        [],
        |r| r.get::<_, i64>(0),
    )? as u64)
}

/// The overview from an open connection with `recog` attached.
pub fn overview(conn: &Connection) -> Result<Overview> {
    let (people, pets) = named(conn)?;
    let fs = faces::stats_of(conn, Space::Faces)?;
    let ps = faces::stats_of(conn, Space::Pets)?;
    let ts = text::stats(conn)?;
    let text_run: Option<i64> = conn
        .query_row("SELECT max(started_at) FROM recog.jobs WHERE kind = 'text'", [], |r| r.get(0))
        .unwrap_or(None);
    let mut faces_part = from_faces(&fs, photos_with(conn, Space::Faces)?, people);
    faces_part.model = conn
        .query_row("SELECT model FROM recog.looked WHERE task = 'faces' ORDER BY done_at DESC LIMIT 1", [], |r| r.get(0))
        .ok();
    let mut pets_part = from_faces(&ps, photos_with(conn, Space::Pets)?, pets);
    pets_part.model = conn
        .query_row("SELECT model FROM recog.looked WHERE task = 'pets' ORDER BY done_at DESC LIMIT 1", [], |r| r.get(0))
        .ok();
    let failed_text: u64 = conn.query_row("SELECT count(*) FROM recog.looked WHERE task = 'text' AND error IS NOT NULL", [], |r| r.get::<_, i64>(0))? as u64;
    let text_part = Part {
        photos: ts.total,
        looked: ts.read + failed_text,
        failed: failed_text,
        to_do: ts.total.saturating_sub(ts.read + failed_text),
        finds: ts.lines,
        with_finds: ts.with_text,
        hidden: ts.hidden,
        stored: ts.stored,
        model: ts.model,
        last_run: text_run,
        ..Part::default()
    };
    let started = fs.looked + ps.looked + fs.rotated_looked + text_part.looked > 0;
    Ok(Overview { started, faces: faces_part, pets: pets_part, text: text_part })
}

/// The overview of a drive, `None` before the first run. Only reads.
pub fn overview_for(root: &Path, db: Option<&Path>) -> Result<Option<Overview>> {
    match faces::open_readonly(root, db)? {
        Some(conn) => Ok(Some(overview(&conn)?)),
        None => Ok(None),
    }
}

fn when(ts: Option<i64>) -> String {
    use chrono::TimeZone;
    ts.and_then(|t| chrono::Local.timestamp_opt(t, 0).single())
        .map(|t| format!(", last run {}", t.format("%Y-%m-%d %H:%M")))
        .unwrap_or_default()
}

fn block(out: &mut String, title: &str, noun: &str, p: &Part, how: &str) {
    if p.looked == 0 {
        out.push_str(&format!("{title}: not run yet ({how}).\n"));
        return;
    }
    let state = if p.to_do == 0 { "all photos done".to_string() } else { format!("{} still to do", p.to_do) };
    out.push_str(&format!(
        "{title}: {} of {} photos looked at, {state}{}{}\n",
        p.looked,
        p.photos,
        if p.failed > 0 { format!(", {} failed", p.failed) } else { String::new() },
        when(p.last_run)
    ));
    out.push_str(&format!("  {} {noun} in {} photos", p.finds, p.with_finds));
    if let Some(m) = &p.model {
        out.push_str(&format!(" (model {m})"));
    }
    out.push('\n');
}

pub fn format_overview(o: &Overview) -> String {
    if !o.started {
        return "Nothing recognized yet: run `shoebox recognize` (faces), `--pets` or `--text-only`.".into();
    }
    let mut out = String::new();
    block(&mut out, "Faces", "faces", &o.faces, "shoebox recognize");
    if o.faces.looked > 0 {
        if o.faces.rotated_looked > 0 {
            out.push_str(&format!(
                "  turned photos: {} looked at, {} more faces found{}\n",
                o.faces.rotated_looked,
                o.faces.rotated_found,
                if o.faces.rotated_failed > 0 { format!(", {} failed", o.faces.rotated_failed) } else { String::new() }
            ));
        }
        out.push_str(&format!(
            "  {} people named, {} faces not matched to anybody yet, {} marked \"not a face\"\n",
            o.faces.named, o.faces.unmatched, o.faces.not_faces
        ));
    }
    block(&mut out, "Pets", "cats and dogs", &o.pets, "shoebox recognize --pets");
    if o.pets.looked > 0 {
        out.push_str(&format!("  {} pets named, {} not matched yet\n", o.pets.named, o.pets.unmatched));
    }
    block(&mut out, "Text", "lines of text", &o.text, "shoebox recognize --text-only");
    if o.text.looked > 0 {
        out.push_str(&format!("  {} lines stored in all, {} hidden by you\n", o.text.stored, o.text.hidden));
    }
    for (title, p) in [("Faces", &o.faces), ("Pets", &o.pets), ("Text", &o.text)] {
        if let Some((message, n)) = &p.error {
            out.push_str(&format!("{title}: most common error ({n}×): {message}\n"));
        }
    }
    out.push_str("Details: `shoebox faces stats [--pets]` and `shoebox text stats`.");
    out
}

/// `shoebox stats`: print it. Only reads.
pub fn print_overview(root: &Path, db: Option<&Path>) -> Result<Overview> {
    let o = overview_for(root, db)?.unwrap_or_default();
    crate::say!("{}", format_overview(&o));
    Ok(o)
}
