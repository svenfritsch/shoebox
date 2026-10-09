//! Where photos were taken (phase 10).
//!
//! - A position comes from the file (`files.lat/lon`, read by the scan from
//!   EXIF GPS or a video's location) or, for a photo without one, from the
//!   user (`geo_overrides`, keyed by content like `taken_overrides`). The
//!   file's own position always wins; nothing is ever written to a photo.
//! - A place is a named rectangle on the map (`places`). The photos inside
//!   are found by comparing coordinates, so a new photo or a redrawn
//!   rectangle needs no bookkeeping.
//! - Maps are drawn by the browser (Leaflet, OpenStreetMap tiles). The server
//!   never fetches or stores a tile; the whole map feature is a setting that
//!   is off by default (`maps`).

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db;

/// Setting `maps`: show maps (needs the internet; tiles come from
/// OpenStreetMap). Off unless the user turned it on.
pub const MAPS_KEY: &str = "maps";

const MAX_NAME: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

impl Area {
    /// A rectangle that lies on the globe and has a size. One across the
    /// date line is not supported: west must be left of east.
    pub fn checked(self) -> Result<Area> {
        let all = [self.south, self.west, self.north, self.east];
        if all.iter().any(|v| !v.is_finite()) {
            bail!("the area must be numbers");
        }
        if self.south < -90.0 || self.north > 90.0 || self.west < -180.0 || self.east > 180.0 {
            bail!("the area lies outside the map");
        }
        if self.south >= self.north || self.west >= self.east {
            bail!("the area needs a size (south below north, west left of east)");
        }
        Ok(self)
    }

    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        (self.south..=self.north).contains(&lat) && (self.west..=self.east).contains(&lon)
    }
}

/// A position typed or clicked by the user: latitude -90 to 90, longitude
/// -180 to 180.
pub fn check_position(lat: f64, lon: f64) -> Result<(f64, f64)> {
    if !lat.is_finite() || !lon.is_finite() {
        bail!("latitude and longitude must be numbers");
    }
    if !(-90.0..=90.0).contains(&lat) {
        bail!("latitude must be between -90 and 90");
    }
    if !(-180.0..=180.0).contains(&lon) {
        bail!("longitude must be between -180 and 180");
    }
    Ok((lat, lon))
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    /// `file` (in the photo) or `user` (given in shoebox).
    pub source: &'static str,
}

/// Where a file was taken, if known.
pub fn position_of(conn: &Connection, id: i64) -> Result<Option<Position>> {
    let row: Option<(Option<f64>, Option<f64>, Option<f64>, Option<f64>)> = conn
        .query_row(
            "SELECT files.lat, files.lon,
                    (SELECT o.lat FROM geo_overrides o WHERE o.key = files.quick_hash),
                    (SELECT o.lon FROM geo_overrides o WHERE o.key = files.quick_hash)
             FROM files WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(match row {
        Some((Some(lat), Some(lon), _, _)) => Some(Position { lat, lon, source: "file" }),
        Some((_, _, Some(lat), Some(lon))) => Some(Position { lat, lon, source: "user" }),
        _ => None,
    })
}

/// Give a photo without a position one (stored for its content, so copies
/// share it), or change the one the user gave before. A photo whose file has
/// a position keeps it.
pub fn set_position(conn: &Connection, id: i64, lat: f64, lon: f64) -> Result<Position> {
    let (lat, lon) = check_position(lat, lon)?;
    let (key, own): (String, bool) = conn
        .query_row("SELECT quick_hash, lat IS NOT NULL AND lon IS NOT NULL FROM files WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no such file"))?;
    if own {
        bail!("this photo has a position in its file; shoebox does not change it");
    }
    conn.execute(
        "INSERT INTO geo_overrides (key, lat, lon, at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET lat = ?2, lon = ?3, at = ?4",
        params![key, lat, lon, db::now()],
    )?;
    Ok(Position { lat, lon, source: "user" })
}

/// Forget a position the user gave. The file's own is not touched.
pub fn clear_position(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM geo_overrides WHERE key = (SELECT quick_hash FROM files WHERE id = ?1)",
        [id],
    )?;
    Ok(())
}

/// Every file with a position: id, latitude, longitude. Missing files are
/// left out; the caller keeps the ones it shows (no RAW, no Live Photo clip).
pub fn positions(conn: &Connection) -> Result<Vec<(i64, f64, f64)>> {
    let sql = format!(
        "SELECT id, {lat}, {lon} FROM files WHERE missing_since IS NULL AND {lat} IS NOT NULL AND {lon} IS NOT NULL",
        lat = db::LAT,
        lon = db::LON
    );
    Ok(conn.prepare(&sql)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?)
}

/// Ids of the files inside an area.
pub fn files_in(conn: &Connection, area: &Area) -> Result<std::collections::HashSet<i64>> {
    Ok(positions(conn)?.into_iter().filter(|&(_, lat, lon)| area.contains(lat, lon)).map(|(id, _, _)| id).collect())
}

// ---------------------------------------------------------------- places

#[derive(Debug, Clone, Serialize)]
pub struct Place {
    pub id: i64,
    pub name: String,
    #[serde(flatten)]
    pub area: Area,
}

pub fn check_name(name: &str) -> Result<String> {
    let name = crate::library::nfc(name.trim());
    if name.is_empty() {
        bail!("a place needs a name");
    }
    if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        bail!("that name is not allowed");
    }
    Ok(name)
}

pub fn places(conn: &Connection) -> Result<Vec<Place>> {
    Ok(conn
        .prepare("SELECT id, name, south, west, north, east FROM places ORDER BY position, id")?
        .query_map([], |r| {
            Ok(Place {
                id: r.get(0)?,
                name: r.get(1)?,
                area: Area { south: r.get(2)?, west: r.get(3)?, north: r.get(4)?, east: r.get(5)? },
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn place(conn: &Connection, id: i64) -> Result<Option<Place>> {
    Ok(places(conn)?.into_iter().find(|p| p.id == id))
}

fn name_taken(conn: &Connection, name: &str, except: Option<i64>) -> Result<bool> {
    let fold = db::tag_fold(name);
    Ok(places(conn)?.iter().any(|p| Some(p.id) != except && db::tag_fold(&p.name) == fold))
}

pub fn create_place(conn: &Connection, name: &str, area: Area) -> Result<Place> {
    let name = check_name(name)?;
    let area = area.checked()?;
    if name_taken(conn, &name, None)? {
        bail!("there is already a place called {name:?}");
    }
    let position: i64 = conn.query_row("SELECT coalesce(max(position), 0) + 1 FROM places", [], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO places (name, south, west, north, east, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![name, area.south, area.west, area.north, area.east, position],
    )?;
    Ok(Place { id: conn.last_insert_rowid(), name, area })
}

pub fn rename_place(conn: &Connection, id: i64, name: &str) -> Result<Place> {
    let name = check_name(name)?;
    if name_taken(conn, &name, Some(id))? {
        bail!("there is already a place called {name:?}");
    }
    if conn.execute("UPDATE places SET name = ?2 WHERE id = ?1", params![id, name])? == 0 {
        bail!("no such place");
    }
    place(conn, id)?.ok_or_else(|| anyhow::anyhow!("no such place"))
}

pub fn redraw_place(conn: &Connection, id: i64, area: Area) -> Result<Place> {
    let area = area.checked()?;
    if conn.execute(
        "UPDATE places SET south = ?2, west = ?3, north = ?4, east = ?5 WHERE id = ?1",
        params![id, area.south, area.west, area.north, area.east],
    )? == 0
    {
        bail!("no such place");
    }
    place(conn, id)?.ok_or_else(|| anyhow::anyhow!("no such place"))
}

pub fn delete_place(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM places WHERE id = ?1", [id])?;
    Ok(())
}

// ---------------------------------------------------------------- userdata.json

#[derive(Debug, Serialize)]
pub struct UserGeo {
    /// Positions given to photos without one (version 5).
    pub geo_overrides: Vec<GeoOverride>,
    /// Named rectangles (version 5).
    pub places: Vec<Place>,
}

#[derive(Debug, Serialize)]
pub struct GeoOverride {
    pub quick_hash: String,
    pub lat: f64,
    pub lon: f64,
    /// Where the content is now, to read the file by eye.
    pub files: Vec<String>,
}

pub fn user_data(conn: &Connection) -> Result<UserGeo> {
    let mut out: Vec<GeoOverride> = conn
        .prepare("SELECT key, lat, lon FROM geo_overrides ORDER BY key")?
        .query_map([], |r| Ok(GeoOverride { quick_hash: r.get(0)?, lat: r.get(1)?, lon: r.get(2)?, files: Vec::new() }))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare("SELECT path_nfc FROM files WHERE quick_hash = ?1 AND missing_since IS NULL ORDER BY path_nfc")?;
    for o in &mut out {
        o.files = stmt.query_map([&o.quick_hash], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    }
    Ok(UserGeo { geo_overrides: out, places: places(conn)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_and_positions_are_checked() {
        let a = Area { south: 10.0, west: 20.0, north: 12.0, east: 22.0 };
        assert!(a.checked().is_ok() && a.contains(11.0, 21.0) && !a.contains(13.0, 21.0));
        assert!(Area { south: 12.0, ..a }.checked().is_err());
        assert!(Area { north: 91.0, ..a }.checked().is_err());
        assert!(Area { west: f64::NAN, ..a }.checked().is_err());
        assert!(check_position(48.1, 11.5).is_ok() && check_position(0.0, 0.0).is_ok());
        assert!(check_position(91.0, 0.0).is_err() && check_position(0.0, 181.0).is_err());
        assert!(check_position(f64::INFINITY, 0.0).is_err());
        assert!(check_name("  ").is_err());
        assert_eq!(check_name(" Zuhause ").unwrap(), "Zuhause");
    }
}
