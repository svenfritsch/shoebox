//! Estimated capture dates (phase 12, docs/phase12.md).
//!
//! A date the user gives a photo, in `library.db` only (`date_estimates`, keyed
//! by content like `taken_overrides` and `geo_overrides`): a year, a year and a
//! month, or a full day. It wins over every other date on the timeline, even a
//! capture date in the file, and is never written into a photo. The file's own
//! date stays in `files.taken` and comes back when the estimate is removed.
//!
//! This module also reads what the user types into the search box ("june 1987",
//! "14.6.1987", `date:jun`) and counts what such a term would find.

use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, Local, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db;

/// The first photographs are from the 1820s; an earlier year is a typing error.
pub const FIRST_YEAR: i32 = 1800;
/// Rows in a list of suggestions.
const MAX_ROWS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Year,
    Month,
    Day,
}

/// A date of the user's: the month only with a year, the day only with a month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    pub year: i32,
    #[serde(default)]
    pub month: Option<u32>,
    #[serde(default)]
    pub day: Option<u32>,
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

impl Estimate {
    pub fn precision(&self) -> Precision {
        match (self.month, self.day) {
            (Some(_), Some(_)) => Precision::Day,
            (Some(_), None) => Precision::Month,
            _ => Precision::Year,
        }
    }

    /// Refuse what cannot be: a day without a month, a month outside 1 to 12, a
    /// day the month does not have, a year before photography, a future date.
    pub fn checked(self, today: NaiveDate) -> Result<Estimate> {
        if self.day.is_some() && self.month.is_none() {
            bail!("a day needs a month");
        }
        if let Some(m) = self.month
            && !(1..=12).contains(&m)
        {
            bail!("the month must be between 1 and 12");
        }
        if self.year < FIRST_YEAR || self.year > today.year() {
            bail!("the year must be between {FIRST_YEAR} and {}", today.year());
        }
        if let (Some(m), Some(d)) = (self.month, self.day)
            && NaiveDate::from_ymd_opt(self.year, m, d).is_none()
        {
            bail!("that day does not exist in that month");
        }
        let start = NaiveDate::from_ymd_opt(self.year, self.month.unwrap_or(1), self.day.unwrap_or(1)).ok_or_else(|| anyhow!("not a date"))?;
        if start > today {
            bail!("that date is in the future");
        }
        Ok(self)
    }

    /// How the timeline sorts it: the start of its period, with 00 for what is
    /// not known (`1987-00-00` for a year, `1987-06-00` for a month), so the
    /// exact photos of a month come before those that only know the month.
    pub fn sort_key(&self) -> String {
        format!("{:04}-{:02}-{:02}T00:00:00", self.year, self.month.unwrap_or(0), self.day.unwrap_or(0))
    }
}

// ------------------------------------------------------------------ the table

/// The estimate of a file, if any.
pub fn get(conn: &Connection, id: i64) -> Result<Option<Estimate>> {
    Ok(conn
        .query_row(
            "SELECT e.year, e.month, e.day FROM date_estimates e JOIN files f ON f.quick_hash = e.key WHERE f.id = ?1",
            [id],
            |r| Ok(Estimate { year: r.get(0)?, month: r.get(1)?, day: r.get(2)? }),
        )
        .optional()?)
}

#[derive(Debug, Serialize)]
pub struct Previous {
    pub id: i64,
    pub estimate: Option<Estimate>,
}

#[derive(Debug, Default, Serialize)]
pub struct Changed {
    /// Photos whose estimate is now different.
    pub changed: usize,
    /// Of the photos asked for, those with a capture date in their file.
    pub with_exif: usize,
    /// What each had before, for Undo.
    pub previous: Vec<Previous>,
}

/// Give each photo this estimate (or take it away with `None`). Copies with the
/// same content share one, so each content is written once. Every estimate is
/// checked first; nothing is written when one is refused.
pub fn apply(conn: &Connection, items: &[(i64, Option<Estimate>)]) -> Result<Changed> {
    let tx = conn.unchecked_transaction()?;
    let out = apply_in(&tx, items)?;
    tx.commit()?;
    Ok(out)
}

/// Like `apply`, inside a transaction the caller has open (all or nothing is
/// then the caller's).
pub fn apply_in(conn: &Connection, items: &[(i64, Option<Estimate>)]) -> Result<Changed> {
    let today = today();
    for (_, e) in items {
        if let Some(e) = e {
            e.checked(today)?;
        }
    }
    // What each photo has now is read before anything is written, so Undo puts
    // back the right dates even when copies of one content are in the same batch.
    let mut rows: Vec<(i64, String, bool, Option<Estimate>)> = Vec::new();
    for &(id, _) in items {
        let (key, has_exif): (String, bool) = conn
            .query_row("SELECT quick_hash, taken IS NOT NULL FROM files WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| anyhow!("no such file"))?;
        rows.push((id, key, has_exif, get(conn, id)?));
    }
    let mut out = Changed::default();
    for (&(_, new), (id, key, has_exif, before)) in items.iter().zip(rows) {
        out.previous.push(Previous { id, estimate: before });
        out.with_exif += has_exif as usize;
        if before == new {
            continue;
        }
        match new {
            Some(e) => {
                conn.execute(
                    "INSERT INTO date_estimates (key, year, month, day, at) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(key) DO UPDATE SET year = ?2, month = ?3, day = ?4, at = ?5",
                    params![key, e.year, e.month, e.day, db::now()],
                )?;
            }
            None => {
                conn.execute("DELETE FROM date_estimates WHERE key = ?1", [&key])?;
            }
        }
        out.changed += 1;
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub total: usize,
    /// With a capture date in the file: their date stays in the file, the
    /// user's wins on the timeline.
    pub with_exif: usize,
    pub with_estimate: usize,
}

/// What a dialog for these photos should say.
pub fn check(conn: &Connection, ids: &[i64]) -> Result<Check> {
    let mut out = Check { total: 0, with_exif: 0, with_estimate: 0 };
    for &id in ids {
        let has_exif: bool = conn
            .query_row("SELECT taken IS NOT NULL FROM files WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow!("no such file"))?;
        out.total += 1;
        out.with_exif += has_exif as usize;
        out.with_estimate += get(conn, id)?.is_some() as usize;
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct DateEstimate {
    pub quick_hash: String,
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
    /// Where the content is now, to read the file by eye.
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UserDates {
    /// Estimated capture dates (userdata.json version 6).
    pub date_estimates: Vec<DateEstimate>,
}

pub fn user_data(conn: &Connection) -> Result<UserDates> {
    let mut out: Vec<DateEstimate> = conn
        .prepare("SELECT key, year, month, day FROM date_estimates ORDER BY key")?
        .query_map([], |r| Ok(DateEstimate { quick_hash: r.get(0)?, year: r.get(1)?, month: r.get(2)?, day: r.get(3)?, files: Vec::new() }))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare("SELECT path_nfc FROM files WHERE quick_hash = ?1 AND missing_since IS NULL ORDER BY path_nfc")?;
    for o in &mut out {
        o.files = stmt.query_map([&o.quick_hash], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    }
    Ok(UserDates { date_estimates: out })
}

// ------------------------------------------------------------------ the search term

/// What a date chip asks for: `1987`, `1987-06` or `1987-06-14`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Term {
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

impl Term {
    /// The `date=` parameter of the API.
    pub fn parse_param(s: &str) -> Result<Term> {
        let bad = || anyhow!("date is 1987, 1987-06 or 1987-06-14, not {s:?}");
        let parts: Vec<&str> = s.trim().split('-').collect();
        let num = |p: &str| p.parse::<u32>().ok();
        let year = parts.first().and_then(|p| (p.len() == 4).then(|| p.parse::<i32>().ok()).flatten()).ok_or_else(bad)?;
        let month = match parts.get(1) {
            Some(p) => Some(num(p).filter(|m| (1..=12).contains(m)).ok_or_else(bad)?),
            None => None,
        };
        let day = match parts.get(2) {
            Some(p) => Some(num(p).filter(|d| (1..=31).contains(d)).ok_or_else(bad)?),
            None => None,
        };
        if parts.len() > 3 {
            return Err(bad());
        }
        Ok(Term { year, month, day })
    }

    pub fn param(&self) -> String {
        match (self.month, self.day) {
            (Some(m), Some(d)) => format!("{:04}-{m:02}-{d:02}", self.year),
            (Some(m), None) => format!("{:04}-{m:02}", self.year),
            _ => format!("{:04}", self.year),
        }
    }

    /// A photo matches when its date lies completely inside the term: a photo
    /// that only knows its year matches a year, one that knows its month
    /// matches a year or that month, and only an exact day matches a day.
    pub fn matches(&self, year: i32, month: Option<u32>, day: Option<u32>) -> bool {
        year == self.year
            && match self.month {
                None => true,
                Some(m) => month == Some(m) && self.day.is_none_or(|d| day == Some(d)),
            }
    }
}

// ------------------------------------------------------------------ typed dates

const MONTH_NAMES: [(&str, &str); 12] = [
    ("january", "januar"),
    ("february", "februar"),
    ("march", "marz"),
    ("april", "april"),
    ("may", "mai"),
    ("june", "juni"),
    ("july", "juli"),
    ("august", "august"),
    ("september", "september"),
    ("october", "oktober"),
    ("november", "november"),
    ("december", "dezember"),
];

/// Lower case without the German marks (the month names are listed without them).
fn fold(s: &str) -> String {
    s.to_lowercase().replace('ä', "a").replace('ö', "o").replace('ü', "u").replace('ß', "ss")
}

/// What was understood of a typed date.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub year: Option<i32>,
    /// Three digits of a year still being typed ("198").
    pub year_prefix: Option<String>,
    /// Usually one; empty when no month was typed.
    pub months: Vec<u32>,
    pub day: Option<u32>,
}

impl Typed {
    fn set_year(&mut self, y: i32) -> Option<()> {
        if self.year.is_some() || !(FIRST_YEAR..=2200).contains(&y) {
            return None;
        }
        self.year = Some(y);
        Some(())
    }
    fn set_month(&mut self, m: u32) -> Option<()> {
        if !self.months.is_empty() || !(1..=12).contains(&m) {
            return None;
        }
        self.months = vec![m];
        Some(())
    }
    fn set_day(&mut self, d: u32) -> Option<()> {
        if self.day.is_some() || !(1..=31).contains(&d) {
            return None;
        }
        self.day = Some(d);
        Some(())
    }
}

/// `1987-06`, `14.6.1987`, `6/1987` and the like, as year, month, day.
fn compound(tok: &str) -> Option<(Option<u32>, Option<u32>, i32)> {
    if !tok.contains(['-', '.', '/']) {
        return None;
    }
    let parts: Vec<&str> = tok.trim_end_matches(['.', '-', '/']).split(['-', '.', '/']).collect();
    if parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let n: Vec<u32> = parts.iter().map(|p| p.parse().unwrap_or(0)).collect();
    let len: Vec<usize> = parts.iter().map(|p| p.len()).collect();
    let small = |l: usize| (1..=2).contains(&l);
    match (len.as_slice(), n.as_slice()) {
        ([4, a], [y, m]) if small(*a) => Some((None, Some(*m), *y as i32)),
        ([4, a, b], [y, m, d]) if small(*a) && small(*b) => Some((Some(*d), Some(*m), *y as i32)),
        ([a, 4], [m, y]) if small(*a) => Some((None, Some(*m), *y as i32)),
        ([a, b, 4], [d, m, y]) if small(*a) && small(*b) => Some((Some(*d), Some(*m), *y as i32)),
        _ => None,
    }
}

/// Read a typed date: a year, a month name (English or German, 3 letters are
/// enough) with or without a year, a day with a month, or numbers such as
/// `6/1987` and `14.6.1987` (day, month, year). `None` when it is not one.
pub fn parse_typed(text: &str) -> Option<Typed> {
    let folded = fold(text.trim());
    let mut t = Typed::default();
    let mut any = false;
    for tok in folded.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty()) {
        any = true;
        if let Some((d, m, y)) = compound(tok) {
            t.set_year(y)?;
            t.set_month(m?)?;
            if let Some(d) = d {
                t.set_day(d)?;
            }
            continue;
        }
        let bare = tok.trim_end_matches('.');
        if !bare.is_empty() && bare.chars().all(|c| c.is_ascii_digit()) {
            match bare.len() {
                4 => t.set_year(bare.parse().ok()?)?,
                3 => {
                    if t.year_prefix.is_some() {
                        return None;
                    }
                    t.year_prefix = Some(bare.to_string());
                }
                1 | 2 => t.set_day(bare.parse().ok()?)?,
                _ => return None,
            }
            continue;
        }
        if bare.chars().count() >= 3 && bare.chars().all(|c| c.is_alphabetic()) {
            let found: Vec<u32> =
                (1..=12u32).filter(|&m| { let (en, de) = MONTH_NAMES[m as usize - 1]; en.starts_with(bare) || de.starts_with(bare) }).collect();
            if found.is_empty() || !t.months.is_empty() {
                return None;
            }
            t.months = found;
            continue;
        }
        return None;
    }
    // A number of one or two digits is a day only next to a month.
    if !any || (t.day.is_some() && t.months.len() != 1) {
        return None;
    }
    Some(t)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub count: usize,
}

/// The rows of the "Date" group for what was typed. `dates` is the date of
/// every photo the search would show (year, month, day as far as each is
/// known). With the `date:` prefix a month alone lists that month of every
/// year, and a year lists its months; without it only a clear date (a year, a
/// month with a year, a day) gets a row, so "June" keeps finding June tags and
/// people. A row for nothing found is left out.
pub fn suggest(text: &str, prefix: bool, dates: &[(i32, Option<u32>, Option<u32>)]) -> Vec<Row> {
    let count = |t: Term| dates.iter().filter(|&&(y, m, d)| t.matches(y, m, d)).count();
    let row = |t: Term| {
        let n = count(t);
        (n > 0).then_some(Row { year: t.year, month: t.month, day: t.day, count: n })
    };
    let mut years: Vec<i32> = dates.iter().map(|d| d.0).collect();
    years.sort_unstable_by(|a, b| b.cmp(a));
    years.dedup();

    let mut out: Vec<Row> = Vec::new();
    if text.trim().is_empty() {
        if prefix {
            out.extend(years.iter().filter_map(|&y| row(Term { year: y, month: None, day: None })));
        }
        out.truncate(MAX_ROWS);
        return out;
    }
    let Some(t) = parse_typed(text) else { return out };
    match t.year {
        Some(y) if !t.months.is_empty() => {
            for &m in &t.months {
                out.extend(row(Term { year: y, month: Some(m), day: t.day }));
            }
        }
        Some(y) => {
            out.extend(row(Term { year: y, month: None, day: None }));
            if prefix {
                for m in (1..=12u32).rev() {
                    out.extend(row(Term { year: y, month: Some(m), day: None }));
                }
            }
        }
        None if prefix && !t.months.is_empty() => {
            for &y in &years {
                for &m in &t.months {
                    out.extend(row(Term { year: y, month: Some(m), day: None }));
                }
            }
        }
        None if prefix && t.year_prefix.is_some() => {
            let p = t.year_prefix.unwrap_or_default();
            out.extend(years.iter().filter(|y| y.to_string().starts_with(&p)).filter_map(|&y| row(Term { year: y, month: None, day: None })));
        }
        None => {}
    }
    out.truncate(MAX_ROWS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn e(year: i32, month: Option<u32>, day: Option<u32>) -> Estimate {
        Estimate { year, month, day }
    }

    #[test]
    fn estimates_are_checked() {
        let today = d(2026, 10, 10);
        assert!(e(1987, None, None).checked(today).is_ok());
        assert!(e(1987, Some(6), None).checked(today).is_ok());
        assert!(e(1987, Some(6), Some(14)).checked(today).is_ok());
        assert!(e(2026, Some(10), Some(10)).checked(today).is_ok());
        assert!(e(1987, None, Some(14)).checked(today).is_err(), "a day needs a month");
        assert!(e(1987, Some(13), None).checked(today).is_err());
        assert!(e(1987, Some(2), Some(30)).checked(today).is_err());
        assert!(e(1988, Some(2), Some(29)).checked(today).is_ok(), "leap year");
        assert!(e(1987, Some(2), Some(29)).checked(today).is_err());
        assert!(e(1799, None, None).checked(today).is_err());
        assert!(e(2026, Some(11), None).checked(today).is_err(), "future");
        assert!(e(2027, None, None).checked(today).is_err(), "future");
        assert!(e(2026, Some(10), Some(11)).checked(today).is_err(), "tomorrow");
    }

    #[test]
    fn sort_keys_put_exact_photos_first_within_a_month() {
        let day = e(1987, Some(6), Some(14)).sort_key();
        let month = e(1987, Some(6), None).sort_key();
        let year = e(1987, None, None).sort_key();
        assert_eq!(year, "1987-00-00T00:00:00");
        assert!(day > month && month > year);
        assert!(year.as_str() > "1986-12-31T23:59:59", "a year sorts after the previous year");
        assert!(year.as_str() < "1987-01-01T00:00:00", "below January of its own year");
    }

    #[test]
    fn terms_match_what_lies_inside_them() {
        let year = Term::parse_param("1987").unwrap();
        let month = Term::parse_param("1987-06").unwrap();
        let day = Term::parse_param("1987-06-14").unwrap();
        // a photo that only knows its year
        assert!(year.matches(1987, None, None) && !month.matches(1987, None, None) && !day.matches(1987, None, None));
        // a photo that knows its month
        assert!(year.matches(1987, Some(6), None) && month.matches(1987, Some(6), None) && !day.matches(1987, Some(6), None));
        // an exact day
        assert!(year.matches(1987, Some(6), Some(14)) && month.matches(1987, Some(6), Some(14)) && day.matches(1987, Some(6), Some(14)));
        assert!(!month.matches(1987, Some(7), Some(14)) && !year.matches(1986, Some(6), Some(14)));
        assert_eq!(day.param(), "1987-06-14");
        assert!(Term::parse_param("87").is_err() && Term::parse_param("1987-13").is_err() && Term::parse_param("1987-06-14-1").is_err());
    }

    #[test]
    fn typed_dates_are_read() {
        let t = |s: &str| parse_typed(s);
        assert_eq!(t("1987").unwrap().year, Some(1987));
        let june = t("june 1987").unwrap();
        assert_eq!((june.year, june.months.clone()), (Some(1987), vec![6]));
        assert_eq!(t("Juni 1987"), t("june 1987"));
        assert_eq!(t("6/1987"), t("june 1987"));
        assert_eq!(t("06.1987"), t("june 1987"));
        assert_eq!(t("1987-06"), t("june 1987"));
        let day = t("14.6.1987").unwrap();
        assert_eq!((day.year, day.months.clone(), day.day), (Some(1987), vec![6], Some(14)));
        assert_eq!(t("14 june 1987"), t("14.6.1987"));
        assert_eq!(t("june 14, 1987"), t("14.6.1987"));
        assert_eq!(t("1987-06-14"), t("14.6.1987"));
        assert_eq!(t("mär").unwrap().months, vec![3]);
        assert_eq!(t("dez").unwrap().months, vec![12]);
        assert_eq!(t("mai").unwrap().months, vec![5]);
        assert_eq!(t("198").unwrap().year_prefix.as_deref(), Some("198"));
        assert!(t("ju").is_none(), "two letters are not a month");
        assert!(t("14").is_none(), "a day needs a month");
        assert!(t("tag").is_none() && t("").is_none() && t("june june").is_none() && t("1987 1988").is_none());
    }

    #[test]
    fn suggestions_follow_the_prefix_rules() {
        let dates = vec![
            (1987, Some(6), Some(14)),
            (1987, Some(6), None),
            (1987, None, None),
            (1986, Some(6), None),
            (1985, Some(12), Some(24)),
            (2025, Some(6), Some(1)),
        ];
        let rows = |s: &str, prefix: bool| suggest(s, prefix, &dates).into_iter().map(|r| (r.year, r.month, r.day, r.count)).collect::<Vec<_>>();
        // A year alone: one row, all three photos of 1987 plus none of the others.
        assert_eq!(rows("1987", false), vec![(1987, None, None, 3)]);
        // A month with a year: month and day precision, not the year-only photo.
        assert_eq!(rows("june 1987", false), vec![(1987, Some(6), None, 2)]);
        // A month alone: nothing without the prefix, every June with it, newest first.
        assert!(rows("june", false).is_empty());
        assert_eq!(rows("june", true), vec![(2025, Some(6), None, 1), (1987, Some(6), None, 2), (1986, Some(6), None, 1)]);
        // A year with the prefix also lists its months, newest first.
        assert_eq!(rows("1987", true), vec![(1987, None, None, 3), (1987, Some(6), None, 2)]);
        // An exact day.
        assert_eq!(rows("14.6.1987", false), vec![(1987, Some(6), Some(14), 1)]);
        // Years still being typed, only with the prefix.
        assert_eq!(rows("198", true).iter().map(|r| r.0).collect::<Vec<_>>(), vec![1987, 1986, 1985]);
        assert!(rows("198", false).is_empty());
        // Nothing typed after the prefix: the years, newest first.
        assert_eq!(rows("", true).iter().map(|r| r.0).collect::<Vec<_>>(), vec![2025, 1987, 1986, 1985]);
        assert!(rows("", false).is_empty());
        // A date with no photos gets no row.
        assert!(rows("1950", false).is_empty());
    }
}
