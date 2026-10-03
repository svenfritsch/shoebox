//! The SQLite index in `.shoebox/library.db`.
//!
//! exFAT has no journaling, so the database runs with `synchronous=FULL` and
//! a rollback journal (no WAL: its shared-memory file is a risk on external
//! drives), and a copy is written after every scan.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

pub const DIR: &str = ".shoebox";
pub const FILE: &str = "library.db";
const BACKUP_SUFFIX: &str = ".bak";

/// Bump when the schema changes and add a step to `migrate`.
const SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE folders (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES folders(id),
    path        TEXT NOT NULL,          -- relative to the root, as found on disk
    path_nfc    TEXT NOT NULL UNIQUE,   -- '' is the root
    name        TEXT NOT NULL,          -- NFC
    event_year  INTEGER,                -- set for 'YYYY-MM Name' folders
    event_month INTEGER,
    event_name  TEXT,
    last_seen   INTEGER NOT NULL        -- id of the scan job that last saw it
);

CREATE TABLE files (
    id            INTEGER PRIMARY KEY,
    folder_id     INTEGER NOT NULL REFERENCES folders(id),
    path          TEXT NOT NULL,        -- relative to the root, as found on disk
    path_nfc      TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,        -- NFC
    kind          TEXT NOT NULL,        -- jpeg, png, heic, raw, video
    size          INTEGER NOT NULL,
    mtime_ns      INTEGER NOT NULL,
    created_ns    INTEGER,
    quick_hash    TEXT NOT NULL,        -- BLAKE3 of size + first/last 64 KiB
    full_hash     TEXT,                 -- BLAKE3 of the content; NULL until hashed
    taken         TEXT,                 -- local capture time, YYYY-MM-DDTHH:MM:SS
    taken_offset  TEXT,                 -- e.g. +03:00, when known
    width         INTEGER,
    height        INTEGER,
    duration_ms   INTEGER,
    camera        TEXT,
    phash         TEXT,                 -- perceptual hash (computed with thumbnails)
    meta_error    TEXT,
    added_at      INTEGER NOT NULL,     -- Unix seconds
    missing_since INTEGER,              -- Unix seconds; NULL while the file exists
    verified_at   INTEGER               -- last successful `shoebox verify`
);
CREATE INDEX files_folder ON files(folder_id);
CREATE INDEX files_size ON files(size);
CREATE INDEX files_full_hash ON files(full_hash);
CREATE INDEX files_taken ON files(taken);

CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE file_tags (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id),
    source  TEXT NOT NULL,              -- 'folder' for now
    PRIMARY KEY (file_id, tag_id, source)
) WITHOUT ROWID;
CREATE INDEX file_tags_tag ON file_tags(tag_id);

CREATE TABLE jobs (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,          -- scan, thumbs, hash, verify
    state       TEXT NOT NULL,          -- running, done, failed, interrupted
    started_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    finished_at INTEGER,
    done        INTEGER NOT NULL DEFAULT 0,
    total       INTEGER,
    detail      TEXT                    -- JSON summary
);
";

/// Default database location for a library root.
pub fn default_path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// Open for a command that owns the index (scan, verify): also marks jobs
/// left `running` by a killed process as interrupted.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = open_shared(path)?;
    // Jobs still marked running belong to a process that did not finish.
    conn.execute(
        "UPDATE jobs SET state = 'interrupted', finished_at = updated_at WHERE state = 'running'",
        [],
    )?;
    Ok(conn)
}

/// Open alongside other processes (the web server runs while a scan may be
/// writing), leaving their `running` jobs alone.
pub fn open_shared(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("library.db has schema version {version}; this shoebox only knows {SCHEMA_VERSION} (update shoebox)");
    }
    if version < 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Copy the database next to itself (`library.db.bak`), atomically replacing
/// the previous copy.
pub fn backup(conn: &Connection, db_path: &Path) -> Result<PathBuf> {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    let dst = db_path.with_file_name(&name);
    name.push(".tmp");
    let tmp = db_path.with_file_name(name);
    let _ = std::fs::remove_file(&tmp);
    {
        let mut out = Connection::open(&tmp)?;
        rusqlite::backup::Backup::new(conn, &mut out)?.run_to_completion(256, std::time::Duration::ZERO, None)?;
        out.pragma_update(None, "journal_mode", "DELETE")?;
    }
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, &dst).with_context(|| format!("write {}", dst.display()))?;
    Ok(dst)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A row in `jobs`, so the UI can show progress and history.
pub struct Job {
    pub id: i64,
}

impl Job {
    pub fn start(conn: &Connection, kind: &str) -> Result<Job> {
        let t = now();
        conn.execute(
            "INSERT INTO jobs (kind, state, started_at, updated_at) VALUES (?1, 'running', ?2, ?2)",
            params![kind, t],
        )?;
        Ok(Job { id: conn.last_insert_rowid() })
    }

    pub fn progress(&self, conn: &Connection, done: u64, total: Option<u64>) -> Result<()> {
        conn.execute(
            "UPDATE jobs SET done = ?2, total = ?3, updated_at = ?4 WHERE id = ?1",
            params![self.id, done as i64, total.map(|t| t as i64), now()],
        )?;
        Ok(())
    }

    pub fn finish(&self, conn: &Connection, state: &str, detail: &impl serde::Serialize) -> Result<()> {
        let t = now();
        conn.execute(
            "UPDATE jobs SET state = ?2, detail = ?3, updated_at = ?4, finished_at = ?4 WHERE id = ?1",
            params![self.id, state, serde_json::to_string(detail)?, t],
        )?;
        Ok(())
    }
}

/// Id of a tag, creating it if needed.
pub fn tag_id(conn: &Connection, name: &str) -> Result<i64> {
    if let Some(id) = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    conn.execute("INSERT INTO tags (name) VALUES (?1)", [name])?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_schema_and_backup() {
        let dir = std::env::temp_dir().join(format!("shoebox-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(DIR).join(FILE);
        let conn = open(&path).unwrap();
        let job = Job::start(&conn, "scan").unwrap();
        drop(conn);

        // Reopening marks the unfinished job as interrupted.
        let conn = open(&path).unwrap();
        let state: String = conn.query_row("SELECT state FROM jobs WHERE id = ?1", [job.id], |r| r.get(0)).unwrap();
        assert_eq!(state, "interrupted");

        let bak = backup(&conn, &path).unwrap();
        let copy = Connection::open(&bak).unwrap();
        let n: i64 = copy.query_row("SELECT count(*) FROM jobs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
