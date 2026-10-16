//! Live Photos: a phone takes a still (JPEG or HEIC) and a short video at the
//! same moment and gives both the same name (`IMG_1234.HEIC`, `IMG_1234.MOV`).
//!
//! The name alone is not enough: cameras and phones count their files the same
//! way, so `IMG_0139.JPG` from a Canon and `IMG_0139.MOV` from an iPhone can
//! meet in one folder, years apart. A still and a video are a Live Photo only
//! if they share folder and name (the caller checks that), the video is short,
//! and both were taken at the same moment. The timeline, the export and moving
//! or trashing a photo use this one rule.

use chrono::{FixedOffset, NaiveDateTime};

use crate::classify::Kind;

/// The video of a Live Photo is at most this long.
pub const MAX_MS: i64 = 6_000;
/// How far apart still and video may be. Both carry the moment the shutter
/// was pressed; a video whose date comes from the container (not Apple's own
/// tag) may be a few seconds off.
const TOLERANCE_S: i64 = 10;
/// When one side does not say its time zone (a JPEG without
/// `OffsetTimeOriginal`), the two may differ by the zone: whole quarter hours
/// up to this far.
const MAX_ZONE_S: i64 = 14 * 3600;
const ZONE_STEP_S: i64 = 15 * 60;

/// When a file was taken, as the file itself says (not a date the user gave it).
#[derive(Debug, Clone, Copy)]
pub struct Moment<'a> {
    /// `files.taken`: `YYYY-MM-DDTHH:MM:SS`, wall-clock time.
    pub taken: Option<&'a str>,
    /// `files.taken_offset` (`+02:00`), when the file records one.
    pub offset: Option<&'a str>,
    /// Modification time; stands in when a file has no capture date.
    pub mtime_ns: i64,
}

/// A file that can be the still of a Live Photo.
pub fn is_still(kind: Kind) -> bool {
    matches!(kind, Kind::Jpeg | Kind::Heic)
}

/// A file that can be the video of a Live Photo.
pub fn is_clip(kind: Kind, duration_ms: Option<i64>) -> bool {
    kind == Kind::Video && duration_ms.is_some_and(|d| d <= MAX_MS)
}

/// Whether a still and a video (same folder, same name) were taken together.
pub fn same_moment(still: Moment, video: Moment) -> bool {
    match (seconds(still), seconds(video)) {
        (Some((a, Some(oa))), Some((b, Some(ob)))) => ((a - oa) - (b - ob)).abs() <= TOLERANCE_S,
        (Some((a, _)), Some((b, _))) => {
            let d = a - b;
            if d.abs() > MAX_ZONE_S + TOLERANCE_S {
                return false;
            }
            let rest = d.rem_euclid(ZONE_STEP_S);
            rest <= TOLERANCE_S || ZONE_STEP_S - rest <= TOLERANCE_S
        }
        // No capture date on one side: the modification times, which a copy
        // in the file manager keeps.
        _ => (still.mtime_ns - video.mtime_ns).abs() <= TOLERANCE_S * 1_000_000_000,
    }
}

/// Wall-clock seconds and, if known, the zone's offset from UTC in seconds.
fn seconds(m: Moment) -> Option<(i64, Option<i64>)> {
    let t = NaiveDateTime::parse_from_str(m.taken?, "%Y-%m-%dT%H:%M:%S").ok()?;
    let offset = m.offset.and_then(|o| o.parse::<FixedOffset>().ok()).map(|o| o.local_minus_utc() as i64);
    Some((t.and_utc().timestamp(), offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(taken: &'static str, offset: Option<&'static str>) -> Moment<'static> {
        Moment { taken: Some(taken), offset, mtime_ns: 0 }
    }

    #[test]
    fn same_second_or_a_few_apart() {
        assert!(same_moment(at("2021-03-14T15:27:01", None), at("2021-03-14T15:27:01", None)));
        assert!(same_moment(at("2021-03-14T15:27:01", None), at("2021-03-14T15:27:04", None)));
        assert!(!same_moment(at("2021-03-14T15:27:01", None), at("2021-03-14T15:28:01", None)));
    }

    #[test]
    fn another_day_is_another_photo() {
        // A Canon IMG_0139.JPG and an iPhone IMG_0139.MOV in one folder.
        assert!(!same_moment(at("2021-03-14T15:27:00", None), at("2019-08-02T10:11:12", Some("+02:00"))));
        assert!(!same_moment(at("2021-03-14T15:27:00", None), at("2021-03-15T15:27:00", None)));
    }

    #[test]
    fn time_zones() {
        // Both zones known: the same instant.
        assert!(same_moment(at("2021-03-14T15:27:00", Some("+01:00")), at("2021-03-14T14:27:02", Some("+00:00"))));
        assert!(!same_moment(at("2021-03-14T15:27:00", Some("+01:00")), at("2021-03-14T15:27:00", Some("+00:00"))));
        // One zone unknown: whole quarter hours apart are fine.
        assert!(same_moment(at("2021-03-14T15:27:00", None), at("2021-03-14T14:27:01", Some("+00:00"))));
        assert!(same_moment(at("2021-03-14T15:27:00", None), at("2021-03-14T09:57:00", Some("+00:00"))));
        assert!(!same_moment(at("2021-03-14T15:27:00", None), at("2021-03-14T14:20:00", Some("+00:00"))));
    }

    #[test]
    fn without_a_date_the_modification_times() {
        let still = Moment { taken: None, offset: None, mtime_ns: 1_600_000_000_000_000_000 };
        let near = Moment { taken: Some("2020-09-13T12:26:40"), offset: None, mtime_ns: still.mtime_ns + 2_000_000_000 };
        let far = Moment { mtime_ns: still.mtime_ns + 3600 * 1_000_000_000, ..near };
        assert!(same_moment(still, near));
        assert!(!same_moment(still, far));
    }

    #[test]
    fn kinds() {
        assert!(is_still(Kind::Jpeg) && is_still(Kind::Heic) && !is_still(Kind::Png));
        assert!(is_clip(Kind::Video, Some(3_000)) && !is_clip(Kind::Video, Some(60_000)) && !is_clip(Kind::Video, None));
    }
}
