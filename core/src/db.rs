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
const SCHEMA_VERSION: i32 = 4;

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
    source  TEXT NOT NULL,              -- 'folder' (from the path) or 'user' (own tags, v3)
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

/// Phase 3: decisions about duplicate pairs, and the trash.
const SCHEMA_V2: &str = "
CREATE TABLE dup_decisions (
    a          INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,  -- a < b
    b          INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    decision   TEXT NOT NULL,       -- distinct (different photos), linked (versions of one photo)
    decided_at INTEGER NOT NULL,
    PRIMARY KEY (a, b)
) WITHOUT ROWID;
CREATE INDEX dup_decisions_b ON dup_decisions(b);

CREATE TABLE trash (
    id         INTEGER PRIMARY KEY,
    batch      INTEGER NOT NULL,     -- one delete: a photo with its RAW, Live Photo and sidecar files
    path       TEXT NOT NULL,        -- where it was, relative to the root, as found on disk
    path_nfc   TEXT NOT NULL,
    stored     TEXT NOT NULL,        -- where it is now, relative to .shoebox/trash
    kind       TEXT,                 -- NULL for sidecar files (XMP, AAE), which are not indexed
    size       INTEGER NOT NULL,
    mtime_ns   INTEGER NOT NULL,
    quick_hash TEXT,
    full_hash  TEXT,
    deleted_at INTEGER NOT NULL      -- Unix seconds
);
CREATE INDEX trash_batch ON trash(batch);
";

/// Phase 5b: own tags. The trash keeps a file's own tags, and own tags are
/// looked up by their folded name (`tag_fold`), so "Europa-Park" and
/// "europa-park" are one tag. `fold` is filled in by `migrate`.
const SCHEMA_V3: &str = "
ALTER TABLE trash ADD COLUMN user_tags TEXT;  -- JSON array of the file's own tag names; NULL if none
ALTER TABLE tags ADD COLUMN fold TEXT;        -- tag_fold(name)
CREATE INDEX tags_fold ON tags(fold);
";

/// Phase 5c-2: people, groups and what the user decided about faces
/// (`people.rs`). User data like own tags: `recognition.db` stays a cache.
const SCHEMA_V4: &str = "
CREATE TABLE IF NOT EXISTS groups (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS people (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL UNIQUE,
    group_id  INTEGER REFERENCES groups(id) ON DELETE SET NULL,  -- one group at most; NULL: no group
    cover_key TEXT,                 -- quick_hash + box of the face shown for this person
    cover_box TEXT,                 -- JSON [x, y, w, h]
    hidden    INTEGER NOT NULL DEFAULT 0
);
-- What the user decided about a face, for detected and hand-drawn faces
-- alike. Keyed by content and box, so it survives moves, rescans and model
-- changes: a detected face takes over the decision whose box it overlaps
-- best (IoU >= 0.5).
CREATE TABLE IF NOT EXISTS face_decisions (
    id        INTEGER PRIMARY KEY,
    key       TEXT NOT NULL,        -- files.quick_hash
    x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,
    person_id INTEGER REFERENCES people(id) ON DELETE CASCADE,  -- NULL with 'ignored' and 'not_face'
    decision  TEXT NOT NULL,        -- confirmed, rejected (not this person), ignored (stranger), not_face (false find)
    manual    INTEGER NOT NULL DEFAULT 0,  -- 1: the box was drawn by hand, not detected
    at        INTEGER NOT NULL      -- Unix seconds
);
CREATE INDEX IF NOT EXISTS face_decisions_key ON face_decisions(key);
CREATE INDEX IF NOT EXISTS face_decisions_person ON face_decisions(person_id);
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
    if version < 2 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    if version < 3 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V3)?;
        let names: Vec<(i64, String)> =
            tx.prepare("SELECT id, name FROM tags")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (id, name) in names {
            tx.execute("UPDATE tags SET fold = ?2 WHERE id = ?1", params![id, tag_fold(&name)])?;
        }
        tx.pragma_update(None, "user_version", 3)?;
        tx.commit()?;
    }
    if version < 4 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V4)?;
        tx.pragma_update(None, "user_version", 4)?;
        tx.commit()?;
    }
    Ok(())
}

/// Copy the database next to itself (`library.db.bak`), atomically replacing
/// the previous copy.
pub fn backup(conn: &Connection, db_path: &Path) -> Result<PathBuf> {
    backup_schema(conn, "main", db_path)
}

/// `backup` for a database attached as `schema` (whose file is `db_path`).
pub fn backup_schema(conn: &Connection, schema: &str, db_path: &Path) -> Result<PathBuf> {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    let dst = db_path.with_file_name(&name);
    name.push(".tmp");
    let tmp = db_path.with_file_name(name);
    let _ = std::fs::remove_file(&tmp);
    {
        let mut out = Connection::open(&tmp)?;
        rusqlite::backup::Backup::new_with_names(conn, schema, &mut out, "main")?.run_to_completion(256, std::time::Duration::ZERO, None)?;
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
    /// `jobs`, or `<schema>.jobs` of an attached database with the same table.
    table: &'static str,
}

impl Job {
    pub fn start(conn: &Connection, kind: &str) -> Result<Job> {
        Job::start_in(conn, "jobs", kind)
    }

    /// A job recorded in another database's `jobs` table (`recog.jobs`), so
    /// its progress does not count as a change to the index.
    pub fn start_in(conn: &Connection, table: &'static str, kind: &str) -> Result<Job> {
        let t = now();
        conn.execute(
            &format!("INSERT INTO {table} (kind, state, started_at, updated_at) VALUES (?1, 'running', ?2, ?2)"),
            params![kind, t],
        )?;
        Ok(Job { id: conn.last_insert_rowid(), table })
    }

    pub fn progress(&self, conn: &Connection, done: u64, total: Option<u64>) -> Result<()> {
        conn.execute(
            &format!("UPDATE {} SET done = ?2, total = ?3, updated_at = ?4 WHERE id = ?1", self.table),
            params![self.id, done as i64, total.map(|t| t as i64), now()],
        )?;
        Ok(())
    }

    pub fn finish(&self, conn: &Connection, state: &str, detail: &impl serde::Serialize) -> Result<()> {
        let t = now();
        conn.execute(
            &format!(
                "UPDATE {} SET state = ?2, detail = ?3, updated_at = ?4, finished_at = ?4 WHERE id = ?1",
                self.table
            ),
            params![self.id, state, serde_json::to_string(detail)?, t],
        )?;
        Ok(())
    }
}

/// How tag names are compared: trimmed, NFC, ignoring case.
pub fn tag_fold(name: &str) -> String {
    crate::library::nfc(name.trim()).to_lowercase()
}

/// Id of the tag with this name in any spelling (the first one wins).
pub fn find_tag(conn: &Connection, name: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM tags WHERE fold = ?1 ORDER BY id LIMIT 1", [tag_fold(name)], |r| r.get(0))
        .optional()?)
}

/// Id of a folder tag, creating it if needed. Spelled exactly as the
/// folder, so a case-only rename of a folder changes its tag too.
pub fn tag_id(conn: &Connection, name: &str) -> Result<i64> {
    if let Some(id) = conn.query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    conn.execute("INSERT INTO tags (name, fold) VALUES (?1, ?2)", params![name, tag_fold(name)])?;
    Ok(conn.last_insert_rowid())
}

/// Id of an own tag: an existing tag in any spelling (a folder tag too), or
/// a new one spelled as given.
pub fn own_tag_id(conn: &Connection, name: &str) -> Result<i64> {
    match find_tag(conn, name)? {
        Some(id) => Ok(id),
        None => tag_id(conn, name),
    }
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

    #[test]
    fn v3_folds_existing_tags() {
        let dir = std::env::temp_dir().join(format!("shoebox-db-v3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(SCHEMA_V1).unwrap();
            old.execute_batch(SCHEMA_V2).unwrap();
            old.pragma_update(None, "user_version", 2).unwrap();
            old.execute("INSERT INTO tags (name) VALUES ('Europa-Park'), ('O\u{308}sterreich')", []).unwrap();
        }
        let conn = open(&path).unwrap();
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // Own tags in other spellings find the existing tags; folder tags
        // keep their exact spelling.
        assert_eq!(own_tag_id(&conn, "europa-park").unwrap(), 1);
        assert_eq!(own_tag_id(&conn, "\u{d6}STERREICH").unwrap(), 2);
        assert_eq!(own_tag_id(&conn, "Neu").unwrap(), 3);
        assert_eq!(find_tag(&conn, "NEU ").unwrap(), Some(3));
        assert_eq!(tag_id(&conn, "neu").unwrap(), 4);
        conn.execute("UPDATE trash SET user_tags = '[]' WHERE 0", []).unwrap();
        // v4 on top: people and face decisions, and a re-run changes nothing.
        conn.execute("INSERT INTO people (name) VALUES ('Aurelia')", []).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);
        let conn = open(&path).unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM people", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        conn.execute(
            "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, at) VALUES ('k', 0, 0, 1, 1, 1, 'confirmed', 0)",
            [],
        )
        .unwrap();
        // Deleting a group leaves its people without one; deleting a person
        // takes their decisions along.
        conn.execute("INSERT INTO groups (name, position) VALUES ('Familie', 1)", []).unwrap();
        conn.execute("UPDATE people SET group_id = 1", []).unwrap();
        conn.execute("DELETE FROM groups", []).unwrap();
        let group: Option<i64> = conn.query_row("SELECT group_id FROM people", [], |r| r.get(0)).unwrap();
        assert_eq!(group, None);
        conn.execute("DELETE FROM people", []).unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM face_decisions", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
