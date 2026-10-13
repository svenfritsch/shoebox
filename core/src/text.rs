//! Text in photos (phase 9, `docs/plan.md`): what the worker's `text` task
//! returns is cleaned and filtered here, in one place, before anything is
//! stored: only German and English characters, no icons or specks, nothing
//! the recogniser is unsure of. The search folds text the same way
//! ([`index_form`]), so a query and the stored lines always meet.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::db;
use crate::fingerprint;
use crate::media;
use crate::thumbs::{self, Source};

/// Lines the recogniser is less sure of than this are not even stored: they
/// are noise. What the search and the viewer show is decided by the limits
/// below, which the user can move on the Text check page.
pub const STORE_MIN_SCORE: f64 = 0.5;
/// Lines lower than this fraction of the photo's height are not even stored.
pub const STORE_MIN_HEIGHT: f64 = 0.004;
/// The limits the search and the viewer start with: lines the recogniser is
/// sure of (0.7), and not smaller than 1 % of the photo's height (the smallest
/// print is mostly wrong, and specks and icons are read as text).
pub const DEFAULT_MIN_SCORE: f64 = 0.7;
pub const DEFAULT_MIN_HEIGHT: f64 = 0.010;

/// A line of text in a photo; the box is in fractions of the upright photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    pub text: String,
}

/// Letters of the German and English alphabet.
fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || "ÄÖÜäöüß".contains(c)
}

/// Characters a line may keep: the Basic Latin letters, digits and
/// punctuation, the German umlauts and ß, typographic quotes and dashes, and a
/// few symbols (€ § ° ² ³ µ £). Everything else (other scripts, other
/// accents) is removed from the line.
fn allowed(c: char) -> bool {
    c.is_ascii_graphic() || c == ' ' || "ÄÖÜäöüß„“”‘’«»–—·•€§°²³µ£".contains(c)
}

/// The line as it is kept, or `None` if nothing worth keeping is left: `β`
/// (which the model prints for `ß`) becomes `ß`, characters outside the
/// allowed set are removed, and a line with fewer than two letters or digits
/// is dropped (an icon read as "X", a lone symbol).
pub fn clean(text: &str) -> Option<String> {
    let kept: String = text.chars().map(|c| if c == 'β' { 'ß' } else { c }).filter(|&c| allowed(c)).collect();
    let kept = kept.trim().to_string();
    (kept.chars().filter(|&c| is_letter(c) || c.is_ascii_digit()).count() >= 2).then_some(kept)
}

/// A line of the worker's result as it is stored, or `None` if it is
/// filtered out ([`STORE_MIN_SCORE`], [`STORE_MIN_HEIGHT`], [`clean`]).
pub fn keep(line: Line) -> Option<Line> {
    if !(line.score >= STORE_MIN_SCORE) || !(line.h >= STORE_MIN_HEIGHT) {
        return None;
    }
    let text = clean(&line.text)?;
    Some(Line { text, ..line })
}

/// The text folded for searching: NFC, lower case, accents dropped (`ü` is
/// `u`), `ß` is `ss`, everything that is not a letter or a digit is a space,
/// spaces are collapsed. "Straße", "STRASSE" and "strasse" fold alike.
pub fn fold(text: &str) -> String {
    let lower: String = text.nfc().flat_map(char::to_lowercase).collect();
    let mut out = String::with_capacity(lower.len());
    let mut space = true;
    for c in lower.replace('ß', "ss").nfd().filter(|&c| !is_combining_mark(c)) {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// A capital `B` right after a lower-case letter is read as `ß` ("strasse"
/// that came back as "straBe"): the repaired text, for [`index_form`].
pub fn repair_eszett(text: &str) -> String {
    let mut prev_lower = false;
    text.chars()
        .map(|c| {
            let out = if c == 'B' && prev_lower { 'ß' } else { c };
            prev_lower = c.is_lowercase();
            out
        })
        .collect()
}

/// What goes into the search index for a line: the folded text, and, when the
/// `B`/`ß` repair makes a difference ("eBay" stays findable as "ebay" next to
/// "eßay"), the folded repaired text after it.
pub fn index_form(text: &str) -> String {
    let plain = fold(text);
    let repaired = fold(&repair_eszett(text));
    if repaired == plain {
        plain
    } else {
        format!("{plain} {repaired}")
    }
}

// ------------------------------------------------------------------ the search

/// What the search and the viewer show of the stored lines: only lines of at
/// least this confidence and (as a fraction of the photo's height) this size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Limits {
    pub min_score: f64,
    pub min_height: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { min_score: DEFAULT_MIN_SCORE, min_height: DEFAULT_MIN_HEIGHT }
    }
}

const SETTING_SCORE: &str = "text_min_score";
const SETTING_HEIGHT: &str = "text_min_height";

/// The limits of this library (`library.db` settings), the defaults where
/// the user has not moved them.
pub fn limits(conn: &Connection) -> Limits {
    let get = |key: &str, default: f64, lo: f64, hi: f64| {
        db::setting(conn, key).ok().flatten().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).map_or(default, |v| v.clamp(lo, hi))
    };
    Limits {
        min_score: get(SETTING_SCORE, DEFAULT_MIN_SCORE, STORE_MIN_SCORE, 1.0),
        min_height: get(SETTING_HEIGHT, DEFAULT_MIN_HEIGHT, STORE_MIN_HEIGHT, 0.2),
    }
}

/// Move the limits (`None` puts one back to its default).
pub fn set_limits(conn: &Connection, min_score: Option<f64>, min_height: Option<f64>) -> Result<Limits> {
    for (key, v) in [(SETTING_SCORE, min_score), (SETTING_HEIGHT, min_height)] {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            db::set_setting(conn, key, Some(&format!("{v}")))?;
        }
    }
    if min_score.is_none() {
        db::set_setting(conn, SETTING_SCORE, None)?;
    }
    if min_height.is_none() {
        db::set_setting(conn, SETTING_HEIGHT, None)?;
    }
    Ok(limits(conn))
}

/// Whether `recognition.db` is attached and has the text tables.
pub fn ready(conn: &Connection) -> bool {
    crate::people::table_exists(conn, "recog", "text_lines").unwrap_or(false)
}

/// The words of a search term, folded ([`fold`]); empty if nothing is left.
pub fn words_of(term: &str) -> Vec<String> {
    fold(term).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The contents (quick hashes) with a shown line that contains `word` (already
/// folded). Three letters or more go through the trigram index, shorter ones
/// scan the lines.
fn keys_with_word(conn: &Connection, word: &str, limits: &Limits) -> Result<HashSet<String>> {
    let shown = "t.score >= ?2 AND t.h >= ?3
         AND NOT EXISTS (SELECT 1 FROM main.text_hidden h WHERE h.key = t.key AND h.norm = t.text_norm)";
    let mut out = HashSet::new();
    if word.chars().count() >= 3 {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT DISTINCT t.key FROM recog.text_lines t
             WHERE t.id IN (SELECT rowid FROM recog.text_fts WHERE text_fts MATCH ?1) AND {shown}"
        ))?;
        let phrase = format!("\"{}\"", word.replace('"', "\"\""));
        for key in stmt.query_map(params![phrase, limits.min_score, limits.min_height], |r| r.get::<_, String>(0))? {
            out.insert(key?);
        }
    } else {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT DISTINCT t.key FROM recog.text_lines t WHERE t.text_norm LIKE ?1 AND {shown}"
        ))?;
        for key in stmt.query_map(params![format!("%{word}%"), limits.min_score, limits.min_height], |r| r.get::<_, String>(0))? {
            out.insert(key?);
        }
    }
    Ok(out)
}

/// The contents in which every word of every term is found, each word in some
/// shown line (so "check out" finds a photo whose lines read "check" and
/// "out" or "checkout"). `None` for no usable words: no restriction.
pub fn keys_matching(conn: &Connection, terms: &[String]) -> Result<Option<HashSet<String>>> {
    let words: Vec<String> = terms.iter().flat_map(|t| words_of(t)).collect();
    if words.is_empty() {
        return Ok(None);
    }
    if !ready(conn) {
        return Ok(Some(HashSet::new()));
    }
    let limits = limits(conn);
    let mut keys: Option<HashSet<String>> = None;
    for w in &words {
        let found = keys_with_word(conn, w, &limits)?;
        keys = Some(match keys {
            Some(k) => k.intersection(&found).cloned().collect(),
            None => found,
        });
        if keys.as_ref().is_some_and(HashSet::is_empty) {
            break;
        }
    }
    Ok(keys)
}

/// A line of text in a photo, for the viewer.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LineOut {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    pub text: String,
    /// The folded text, which a hidden line is known by.
    pub norm: String,
    /// The user hid it: it is shown struck through and is not searched.
    pub hidden: bool,
}

/// The lines of a content that the limits show, top to bottom, hidden ones
/// included (marked).
pub fn lines_of(conn: &Connection, key: &str) -> Result<Vec<LineOut>> {
    if !ready(conn) {
        return Ok(Vec::new());
    }
    let limits = limits(conn);
    let mut stmt = conn.prepare_cached(
        "SELECT t.x, t.y, t.w, t.h, t.score, t.text, t.text_norm,
                EXISTS (SELECT 1 FROM main.text_hidden h WHERE h.key = t.key AND h.norm = t.text_norm)
         FROM recog.text_lines t WHERE t.key = ?1 AND t.score >= ?2 AND t.h >= ?3 ORDER BY t.y, t.x, t.id",
    )?;
    let rows = stmt.query_map(params![key, limits.min_score, limits.min_height], |r| {
        Ok(LineOut { x: r.get(0)?, y: r.get(1)?, w: r.get(2)?, h: r.get(3)?, score: r.get(4)?, text: r.get(5)?, norm: r.get(6)?, hidden: r.get::<_, i64>(7)? != 0 })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Hide a line of a content from the search (or show it again). `norm` is
/// the line's folded text, as in [`LineOut`]; a line the content does not have
/// is refused.
pub fn set_hidden(conn: &Connection, key: &str, norm: &str, hidden: bool) -> Result<()> {
    let has: Option<i64> = conn
        .query_row("SELECT 1 FROM recog.text_lines WHERE key = ?1 AND text_norm = ?2 LIMIT 1", params![key, norm], |r| r.get(0))
        .optional()?;
    if has.is_none() {
        anyhow::bail!("this photo has no such line of text");
    }
    if hidden {
        conn.execute("INSERT OR IGNORE INTO text_hidden (key, norm, at) VALUES (?1, ?2, ?3)", params![key, norm, db::now()])?;
    } else {
        conn.execute("DELETE FROM text_hidden WHERE key = ?1 AND norm = ?2", params![key, norm])?;
    }
    Ok(())
}

/// For each photo (by id) in `ids`, the first shown line that contains one
/// of the words of `terms`: what a text search found in it.
pub fn snippets(conn: &Connection, ids: &HashSet<i64>, terms: &[String]) -> Result<HashMap<i64, String>> {
    let words: Vec<String> = terms.iter().flat_map(|t| words_of(t)).collect();
    let mut out = HashMap::new();
    if words.is_empty() || ids.is_empty() || !ready(conn) {
        return Ok(out);
    }
    let limits = limits(conn);
    let mut stmt = conn.prepare_cached(
        "SELECT f.id, t.text, t.text_norm FROM files f JOIN recog.text_lines t ON t.key = f.quick_hash
         WHERE f.missing_since IS NULL AND t.score >= ?1 AND t.h >= ?2
           AND NOT EXISTS (SELECT 1 FROM main.text_hidden h WHERE h.key = t.key AND h.norm = t.text_norm)
         ORDER BY t.y, t.x, t.id",
    )?;
    let mut rows = stmt.query(params![limits.min_score, limits.min_height])?;
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        if !ids.contains(&id) || out.contains_key(&id) {
            continue;
        }
        let norm: String = r.get(2)?;
        if words.iter().any(|w| norm.contains(w.as_str())) {
            out.insert(id, r.get(1)?);
        }
    }
    Ok(out)
}

/// What has been read, for `shoebox text stats` and the status line.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    /// Contents the text pass has been through (with its current model).
    pub read: u64,
    /// … of those, with at least one shown line.
    pub with_text: u64,
    /// Shown lines (above the limits) and stored lines (all).
    pub lines: u64,
    pub stored: u64,
    pub hidden: u64,
    /// Contents waiting for the text pass (all photos, so a rough count).
    pub total: u64,
    pub model: Option<String>,
    pub limits: Option<Limits>,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    if !ready(conn) {
        return Ok(Stats::default());
    }
    let limits = limits(conn);
    let one = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<u64> { Ok(conn.query_row(sql, p, |r| r.get::<_, i64>(0))? as u64) };
    let kinds = crate::recognize::KINDS;
    Ok(Stats {
        read: one("SELECT count(*) FROM recog.looked WHERE task = 'text' AND error IS NULL", &[])?,
        with_text: one(
            "SELECT count(DISTINCT key) FROM recog.text_lines WHERE score >= ?1 AND h >= ?2",
            &[&limits.min_score, &limits.min_height],
        )?,
        lines: one("SELECT count(*) FROM recog.text_lines WHERE score >= ?1 AND h >= ?2", &[&limits.min_score, &limits.min_height])?,
        stored: one("SELECT count(*) FROM recog.text_lines", &[])?,
        hidden: if crate::people::table_exists(conn, "main", "text_hidden")? { one("SELECT count(*) FROM text_hidden", &[])? } else { 0 },
        total: one(&format!("SELECT count(DISTINCT quick_hash) FROM files WHERE missing_since IS NULL AND kind IN ({kinds})"), &[])?,
        model: conn
            .query_row("SELECT model FROM recog.looked WHERE task = 'text' ORDER BY done_at DESC LIMIT 1", [], |r| r.get(0))
            .optional()?,
        limits: Some(limits),
    })
}

/// `shoebox text stats`: what has been read, in words.
pub fn format_stats(s: &Stats) -> String {
    let Some(l) = s.limits else { return "No text read yet: run `shoebox recognize --text-only` first.".into() };
    if s.read == 0 && s.stored == 0 {
        return "No text read yet: run `shoebox recognize --text-only` first.".into();
    }
    let mut out = format!(
        "Text read in {} of {} photos with {}.\n  {} photos have text a search finds ({} lines)\n  {} lines stored in all, {} hidden by you\n  A line counts from a confidence of {:.2} and a height of {:.1} % of the photo",
        s.read,
        s.total,
        s.model.as_deref().unwrap_or("an unknown model"),
        s.with_text,
        s.lines,
        s.stored,
        s.hidden,
        l.min_score,
        l.min_height * 100.0
    );
    if s.read < s.total {
        out.push_str(&format!("\n  {} photos still to read", s.total - s.read));
    }
    out
}

/// `shoebox text stats`. Only reads.
pub fn print_stats(root: &std::path::Path, db: Option<&std::path::Path>) -> Result<Option<Stats>> {
    let Some(conn) = crate::faces::open_readonly(root, db)? else {
        crate::say!("No text read yet: run `shoebox recognize --text-only` first.");
        return Ok(None);
    };
    let stats = stats(&conn)?;
    crate::say!("{}", format_stats(&stats));
    Ok(Some(stats))
}

/// A line close to the limits, for the Text check page.
#[derive(Debug, Clone, Serialize)]
pub struct CheckLine {
    /// A present photo with this content.
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    pub text: String,
    /// The limits let it count.
    pub shown: bool,
}

/// Lines near the limits (a little above and a little below), the nearest
/// first, at most `n`: what moving a limit would take in or leave out.
pub fn check_lines(conn: &Connection, n: usize) -> Result<Vec<CheckLine>> {
    if !ready(conn) {
        return Ok(Vec::new());
    }
    let l = limits(conn);
    let mut stmt = conn.prepare(
        "SELECT (SELECT min(f.id) FROM files f WHERE f.quick_hash = t.key AND f.missing_since IS NULL) AS fid,
                t.x, t.y, t.w, t.h, t.score, t.text
         FROM recog.text_lines t
         WHERE (t.score BETWEEN ?1 - 0.2 AND ?1 + 0.12 OR t.h BETWEEN ?2 * 0.5 AND ?2 * 1.6)
           AND NOT EXISTS (SELECT 1 FROM main.text_hidden h WHERE h.key = t.key AND h.norm = t.text_norm)
         ORDER BY max(abs(t.score - ?1) / 0.1, abs(t.h - ?2) / (?2 * 0.5)), t.key, t.y",
    )?;
    let mut out = Vec::new();
    let mut rows = stmt.query(params![l.min_score, l.min_height])?;
    while let Some(r) = rows.next()? {
        let Some(id) = r.get::<_, Option<i64>>(0)? else { continue };
        let (score, h): (f64, f64) = (r.get(5)?, r.get(4)?);
        out.push(CheckLine { id, x: r.get(1)?, y: r.get(2)?, w: r.get(3)?, h, score, text: r.get(6)?, shown: score >= l.min_score && h >= l.min_height });
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}

/// The widest side of a line's crop.
const CROP_EDGE: u32 = 640;
const CROP_QUALITY: u8 = 82;

/// A crop of one line of text (its box with some room) from the photo, made
/// under the guard and never stored.
pub fn render_crop(lib_heif: &libheif_rs::LibHeif, src: &Source, b: [f64; 4]) -> Result<Vec<u8>, String> {
    fingerprint::read_unchanged(&src.path, src.size, src.mtime_ns, || {
        let img = media::decode_image(lib_heif, src.kind, &src.path, crate::recognize::EDGE).map_err(|e| format!("{e:#}"))?;
        let img = thumbs::shrink(img, crate::recognize::EDGE);
        let (iw, ih) = (img.width() as f64, img.height() as f64);
        let [x, y, w, h] = b;
        let (mx, my) = (w * 0.04 + 0.004, h * 0.35);
        let (x0, y0) = (((x - mx) * iw).max(0.0), ((y - my) * ih).max(0.0));
        let (x1, y1) = (((x + w + mx) * iw).min(iw), ((y + h + my) * ih).min(ih));
        let (cw, ch) = (((x1 - x0).round() as u32).max(1), ((y1 - y0).round() as u32).max(1));
        let crop = img.crop_imm(x0.round() as u32, y0.round() as u32, cw.min(img.width()), ch.min(img.height()));
        media::encode_jpeg(&thumbs::shrink(crop, CROP_EDGE), CROP_QUALITY).map_err(|e| format!("{e:#}"))
    })
}

/// How far the text pass is, for the status line and the search box.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Progress {
    /// Photos (contents) the text pass has been through.
    pub done: u64,
    pub total: u64,
    /// Lines the limits show: 0 means a text search has nothing to find.
    pub lines: u64,
}

pub fn progress(conn: &Connection) -> Result<Progress> {
    if !ready(conn) {
        return Ok(Progress::default());
    }
    let limits = limits(conn);
    let kinds = crate::recognize::KINDS;
    let (total, done): (i64, i64) = conn.query_row(
        &format!(
            "SELECT count(*), count(l.key) FROM
               (SELECT DISTINCT quick_hash FROM files WHERE missing_since IS NULL AND kind IN ({kinds})) f
             LEFT JOIN recog.looked l ON l.key = f.quick_hash AND l.task = 'text' AND l.error IS NULL"
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let lines: i64 = conn.query_row("SELECT count(*) FROM recog.text_lines WHERE score >= ?1 AND h >= ?2", params![limits.min_score, limits.min_height], |r| r.get(0))?;
    Ok(Progress { done: done as u64, total: total as u64, lines: lines as u64 })
}

/// Forget everything that was read (the lines and the memory of having read
/// each photo); the user's hidden lines stay, they are decisions. A photo
/// that is read again gets its lines back.
pub fn delete_all(conn: &Connection) -> Result<u64> {
    if !ready(conn) {
        return Ok(0);
    }
    let n = conn.execute("DELETE FROM recog.text_lines", [])? as u64;
    conn.execute("DELETE FROM recog.looked WHERE task = 'text'", [])?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, score: f64, h: f64) -> Line {
        Line { x: 0.1, y: 0.1, w: 0.5, h, score, text: text.into() }
    }

    #[test]
    fn other_scripts_and_icons_are_dropped() {
        assert_eq!(clean("田"), None);
        assert_eq!(clean("门田"), None);
        assert_eq!(clean("X"), None);
        assert_eq!(clean("="), None);
        assert_eq!(clean("你好 Hello"), Some("Hello".into()));
        assert_eq!(clean("11:33"), Some("11:33".into()));
        assert_eq!(clean("€ 24,99"), Some("€ 24,99".into()));
    }

    #[test]
    fn german_marks_stay_and_other_accents_go() {
        assert_eq!(clean("Größe Äpfel über"), Some("Größe Äpfel über".into()));
        assert_eq!(clean("Straβe"), Some("Straße".into()), "the model prints a Greek beta for ß");
        assert_eq!(clean("FOR SOMIÉ"), Some("FOR SOMI".into()));
        assert_eq!(clean("Crème"), Some("Crme".into()));
    }

    #[test]
    fn lines_are_kept_by_score_and_size() {
        assert!(keep(line("Rechnung", 0.97, 0.05)).is_some());
        assert!(keep(line("Rechnung", 0.4, 0.05)).is_none(), "unsure");
        assert!(keep(line("Rechnung", 0.6, 0.05)).is_some(), "stored, shown only above the user's limit");
        assert!(keep(line("Rechnung", 0.97, 0.003)).is_none(), "tiny");
        assert!(keep(line("Rechnung", f64::NAN, 0.05)).is_none());
        assert!(keep(line("田", 0.99, 0.05)).is_none(), "nothing left after cleaning");
        assert_eq!(keep(line("Straβe 4", 0.9, 0.03)).unwrap().text, "Straße 4");
    }

    #[test]
    fn folding_ignores_case_accents_eszett_and_unicode_form() {
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("STRASSE"), "strasse");
        assert_eq!(fold("Müller"), "muller");
        // NFD input (u + combining diaeresis) folds like NFC.
        assert_eq!(fold("Mu\u{308}ller"), "muller");
        assert_eq!(fold("  Rechnung Nr. 2024-0412 "), "rechnung nr 2024 0412");
        assert_eq!(fold("Haupt-Straße, 12"), "haupt strasse 12");
    }

    #[test]
    fn the_eszett_repair_is_indexed_next_to_the_plain_text() {
        assert_eq!(repair_eszett("HaunstetterstraBe"), "Haunstetterstraße");
        assert_eq!(repair_eszett("GroBe BASE"), "Große BASE", "capitals after capitals stay");
        assert_eq!(index_form("HaunstetterstraBe"), "haunstetterstrabe haunstetterstrasse");
        assert_eq!(index_form("eBay"), "ebay essay");
        assert_eq!(index_form("Rechnung"), "rechnung", "no repair, no duplicate");
    }
}
