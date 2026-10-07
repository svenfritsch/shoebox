//! Library rules that depend only on paths: relative path strings, Unicode
//! normalisation, event folders and folder tags.

use std::path::{Component, Path};

use unicode_normalization::UnicodeNormalization;

/// A path relative to the library root, `/`-separated, exactly as found on
/// disk (for opening the file), plus its NFC form (for comparing and search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelPath {
    pub raw: String,
    pub nfc: String,
}

impl RelPath {
    /// `None` for paths outside `root` or with non-UTF-8 names.
    pub fn new(root: &Path, path: &Path) -> Option<RelPath> {
        let rel = path.strip_prefix(root).ok()?;
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str()?),
                _ => return None,
            }
        }
        let raw = parts.join("/");
        let nfc = nfc(&raw);
        Some(RelPath { raw, nfc })
    }

    /// Last component (NFC); empty for the root.
    pub fn name(&self) -> &str {
        self.nfc.rsplit('/').next().unwrap_or("")
    }

    /// Parent folder (NFC); `""` is the root.
    pub fn parent_nfc(&self) -> &str {
        self.nfc.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
    }
}

pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// A folder named `YYYY-MM Name`, e.g. `2020-07 Urlaub Griechenland`. The
/// scanner reads every form of `EventPattern`, whatever the library's
/// setting is, so old folders and other drives keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event<'a> {
    pub year: u16,
    pub month: u8,
    pub name: &'a str,
}

/// Year and month at the start of an event folder's name, then the name.
/// Year first, two or four digits (two are 20YY); `.` or `-` between year and
/// month; between date and name a space, `_`, `.`, `-` or nothing. (`/` cannot
/// be used: it would make a subfolder.) A name after punctuation or nothing
/// may not start with a digit, so `2020-07-15 Foo` (a day) is no event folder.
pub fn parse_event(folder_name: &str) -> Option<Event<'_>> {
    let b = folder_name.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits != 2 && digits != 4 {
        return None;
    }
    if !matches!(b.get(digits), Some(b'-' | b'.')) {
        return None;
    }
    let m = digits + 1;
    if b.len() < m + 2 || !b[m..m + 2].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year: u16 = folder_name[..digits].parse().ok()?;
    let year = if digits == 2 { 2000 + year } else { year };
    let month: u8 = folder_name[m..m + 2].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let rest = &folder_name[m + 2..];
    // After spaces any name will do; after a punctuation mark or nothing it
    // may not start with a digit (that would be a day, `2020-07-15`).
    let (name, digit_ok) = if rest.starts_with(char::is_whitespace) {
        (rest.trim_start(), true)
    } else if let Some(n) = rest.strip_prefix(['_', '.', '-']) {
        (n.trim_start(), n.starts_with(char::is_whitespace))
    } else {
        (rest, false)
    };
    if name.is_empty() || (!digit_ok && name.starts_with(|c: char| c.is_ascii_digit())) {
        return None;
    }
    Some(Event { year, month, name })
}

/// How the import dialog and "rename folder" name an event folder. Kept per
/// library (setting `event_pattern`, text like `YYYY-MM Name`); it only
/// decides what gets created, never how folders are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventPattern {
    /// 2 or 4 digits.
    pub year_digits: u8,
    /// `.` or `-`.
    pub date_sep: char,
    /// Between date and name: a space, `_`, `.`, `-` or nothing.
    pub gap: Option<char>,
}

impl Default for EventPattern {
    fn default() -> Self {
        EventPattern { year_digits: 4, date_sep: '-', gap: Some(' ') }
    }
}

impl EventPattern {
    /// `YYYY-MM Name`, `YY.MM_Name`, `YYYY.MMName` ...
    pub fn parse(text: &str) -> Option<EventPattern> {
        let (year_digits, rest) = if let Some(r) = text.strip_prefix("YYYY") {
            (4, r)
        } else {
            (2, text.strip_prefix("YY")?)
        };
        let mut chars = rest.chars();
        let date_sep = chars.next().filter(|c| matches!(c, '.' | '-'))?;
        let rest = chars.as_str().strip_prefix("MM")?;
        let gap = rest.strip_suffix("Name")?;
        let gap = match gap {
            "" => None,
            g if g.chars().count() == 1 && matches!(g.chars().next(), Some(' ' | '_' | '.' | '-')) => g.chars().next(),
            _ => return None,
        };
        Some(EventPattern { year_digits, date_sep, gap })
    }

    pub fn format(&self) -> String {
        let year = if self.year_digits == 4 { "YYYY" } else { "YY" };
        format!("{year}{}MM{}Name", self.date_sep, self.gap.map(String::from).unwrap_or_default())
    }

    /// The folder name for a year (four digits, four-digit years only fit
    /// the two-digit form from 2000 to 2099), month and event name.
    pub fn folder_name(&self, year: i32, month: u32, name: &str) -> Option<String> {
        let y = if self.year_digits == 4 {
            format!("{year:04}")
        } else if (2000..2100).contains(&year) {
            format!("{:02}", year - 2000)
        } else {
            return None;
        };
        let gap = self.gap.map(String::from).unwrap_or_default();
        Some(format!("{y}{}{month:02}{gap}{name}", self.date_sep))
    }
}

/// The tag a folder contributes to every file below it: the event name for
/// event folders, the folder name otherwise.
pub fn folder_tag(folder_name: &str) -> &str {
    parse_event(folder_name).map(|e| e.name).unwrap_or(folder_name)
}

/// Tags of a file: one per folder level between the root and the file.
pub fn tags_for(file: &RelPath) -> Vec<&str> {
    let parent = file.parent_nfc();
    if parent.is_empty() {
        return Vec::new();
    }
    let mut tags: Vec<&str> = parent.split('/').map(folder_tag).collect();
    tags.sort_unstable();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_folders() {
        assert_eq!(
            parse_event("2020-07 Urlaub Griechenland"),
            Some(Event { year: 2020, month: 7, name: "Urlaub Griechenland" })
        );
        assert_eq!(parse_event("2020-12\tX").map(|e| e.month), Some(12));
        assert_eq!(parse_event("2020-13 Nope"), None);
        assert_eq!(parse_event("2020-00 Nope"), None);
        assert_eq!(parse_event("2020-07Urlaub").map(|e| e.name), Some("Urlaub"));
        assert_eq!(parse_event("2020-0710 Fotos"), None);
        assert_eq!(parse_event("2020-07 "), None);
        assert_eq!(parse_event("Familie"), None);
        // Two-digit years are 20YY; a dot works as the separator.
        assert_eq!(parse_event("20-07 Kurz"), Some(Event { year: 2020, month: 7, name: "Kurz" }));
        assert_eq!(parse_event("98.08 Urlaub").map(|e| (e.year, e.month)), Some((2098, 8)));
        assert_eq!(parse_event("2020.07 Urlaub").map(|e| (e.year, e.month)), Some((2020, 7)));
        assert_eq!(parse_event("2020/07 Urlaub"), None);
        assert_eq!(parse_event("202-07 Urlaub"), None);
        assert_eq!(parse_event("20207-07 Urlaub"), None);
        assert_eq!(parse_event("2020.13 Urlaub"), None);
        assert_eq!(parse_event("20.07Urlaub").map(|e| e.name), Some("Urlaub"));
        // Other gaps between date and name.
        assert_eq!(parse_event("20.07.Foo").map(|e| (e.year, e.month, e.name)), Some((2020, 7, "Foo")));
        assert_eq!(parse_event("20.07_Foo").map(|e| e.name), Some("Foo"));
        assert_eq!(parse_event("2020-07-Foo").map(|e| e.name), Some("Foo"));
        assert_eq!(parse_event("2020-07 - Foo").map(|e| e.name), Some("- Foo"));
        assert_eq!(parse_event("2020-07 2019 Reise").map(|e| e.name), Some("2019 Reise"));
        assert_eq!(parse_event("2020-07-15 Foo"), None); // a day, not an event folder
        assert_eq!(parse_event("2020-07_"), None);
        assert_eq!(parse_event("2020-07."), None);
    }

    #[test]
    fn event_patterns() {
        let p = |t: &str| EventPattern::parse(t).unwrap();
        assert_eq!(EventPattern::default().format(), "YYYY-MM Name");
        assert_eq!(p("YY.MM_Name").format(), "YY.MM_Name");
        assert_eq!(p("YYYY.MMName").gap, None);
        assert_eq!(p("YYYY-MM Name").folder_name(2021, 3, "Ausflug").unwrap(), "2021-03 Ausflug");
        assert_eq!(p("YY.MM_Name").folder_name(2020, 3, "X").unwrap(), "20.03_X");
        assert_eq!(p("YY.MM.Name").folder_name(2007, 11, "X").unwrap(), "07.11.X");
        assert_eq!(p("YY.MM Name").folder_name(1998, 8, "X"), None); // 20YY only
        for bad in ["MM-YYYY Name", "YYYY/MM Name", "YYYY-MM  Name", "YYY-MM Name", "YYYY-MM", "YYYY MM Name", "YYYY-MMxName"] {
            assert_eq!(EventPattern::parse(bad), None, "{bad}");
        }
        // Whatever the pattern makes, the scanner reads back.
        for gap in [Some(' '), Some('_'), Some('.'), Some('-'), None] {
            for (digits, sep) in [(2, '.'), (4, '-'), (4, '.'), (2, '-')] {
                let pat = EventPattern { year_digits: digits, date_sep: sep, gap };
                assert_eq!(EventPattern::parse(&pat.format()), Some(pat));
                let name = pat.folder_name(2021, 3, "Ausflug 2").unwrap();
                let e = parse_event(&name).unwrap_or_else(|| panic!("{name}"));
                assert_eq!((e.year, e.month, e.name), (2021, 3, "Ausflug 2"), "{name}");
            }
        }
    }

    #[test]
    fn rel_paths_are_nfc_normalised() {
        let root = Path::new("/lib");
        let p = RelPath::new(root, Path::new("/lib/2019-08 O\u{308}sterreich/IMG_1.JPG")).unwrap();
        assert_eq!(p.raw, "2019-08 O\u{308}sterreich/IMG_1.JPG");
        assert_eq!(p.nfc, "2019-08 \u{d6}sterreich/IMG_1.JPG");
        assert_eq!(p.name(), "IMG_1.JPG");
        assert_eq!(p.parent_nfc(), "2019-08 \u{d6}sterreich");
        assert_eq!(RelPath::new(root, root).unwrap().raw, "");
        assert!(RelPath::new(root, Path::new("/other/x.jpg")).is_none());
    }

    #[test]
    fn every_folder_level_is_a_tag() {
        let root = Path::new("/lib");
        let p = RelPath::new(root, Path::new("/lib/Familie/Weihnachten/a.jpg")).unwrap();
        assert_eq!(tags_for(&p), ["Familie", "Weihnachten"]);
        let p = RelPath::new(root, Path::new("/lib/2020-07 Urlaub/a.jpg")).unwrap();
        assert_eq!(tags_for(&p), ["Urlaub"]);
        let p = RelPath::new(root, Path::new("/lib/a.jpg")).unwrap();
        assert!(tags_for(&p).is_empty());
    }
}
