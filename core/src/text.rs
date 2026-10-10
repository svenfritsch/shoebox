//! Text in photos (phase 9, `docs/plan.md`): what the worker's `text` task
//! returns is cleaned and filtered here, in one place, before anything is
//! stored: only German and English characters, no icons or specks, nothing
//! the recogniser is unsure of. The search folds text the same way
//! ([`index_form`]), so a query and the stored lines always meet.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Lines the recogniser is less sure of than this are not kept.
pub const MIN_SCORE: f64 = 0.7;
/// Lines lower than this fraction of the photo's height are not kept: the
/// smallest print is mostly wrong, and specks and icons are read as text.
pub const MIN_HEIGHT: f64 = 0.010;

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
/// filtered out ([`MIN_SCORE`], [`MIN_HEIGHT`], [`clean`]).
pub fn keep(line: Line) -> Option<Line> {
    if !(line.score >= MIN_SCORE) || !(line.h >= MIN_HEIGHT) {
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
        assert!(keep(line("Rechnung", 0.5, 0.05)).is_none(), "unsure");
        assert!(keep(line("Rechnung", 0.97, 0.004)).is_none(), "tiny");
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
