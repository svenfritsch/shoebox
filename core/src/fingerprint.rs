//! File identity: filesystem stamps and content hashes.
//!
//! The quick hash (size + first/last 64 KiB) is cheap enough to compute for
//! every new file and narrows down move/duplicate candidates. The full hash
//! is the authoritative fingerprint used for exact duplicates and backup
//! verification.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const QUICK_CHUNK: u64 = 64 * 1024;

/// What the filesystem says about a file, without opening it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stamp {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i128,
    /// Creation ("birth") time, where the platform and filesystem report it.
    pub created_ns: Option<i128>,
}

fn to_ns(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

pub fn stamp(path: &Path) -> io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    Ok(Stamp {
        size: meta.len(),
        mtime_ns: to_ns(meta.modified()?),
        created_ns: meta.created().ok().map(to_ns),
    })
}

/// BLAKE3 over the size plus the first and last 64 KiB of the file.
pub fn quick_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());

    let mut buf = vec![0u8; QUICK_CHUNK as usize];
    let n = read_up_to(&mut file, &mut buf)?;
    hasher.update(&buf[..n]);

    if size > 2 * QUICK_CHUNK {
        file.seek(SeekFrom::End(-(QUICK_CHUNK as i64)))?;
        let n = read_up_to(&mut file, &mut buf)?;
        hasher.update(&buf[..n]);
    } else if size > QUICK_CHUNK {
        // Small file: the head already covered most of it, hash the rest.
        let n = read_up_to(&mut file, &mut buf)?;
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// BLAKE3 over the whole file.
pub fn full_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Run `read` on a file the index says has `size` and `mtime_ns`, failing
/// if the file differs from that before, or changes while it is read. This
/// is the per-file half of the guard: nothing derived from a file that moved
/// under us is stored.
pub fn read_unchanged<T>(
    path: &Path,
    size: u64,
    mtime_ns: i64,
    read: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let before = stamp(path).map_err(|e| e.to_string())?;
    if before.size != size || before.mtime_ns != mtime_ns as i128 {
        return Err("changed since the last scan".into());
    }
    let value = read()?;
    let after = stamp(path).map_err(|e| e.to_string())?;
    if after != before {
        return Err("changed while being read".into());
    }
    Ok(value)
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("shoebox-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        File::create(&path).unwrap().write_all(data).unwrap();
        path
    }

    #[test]
    fn quick_hash_sees_head_and_tail_changes() {
        let mut data = vec![7u8; 500_000];
        let a = write_temp("a.bin", &data);
        data[499_999] = 8;
        let b = write_temp("b.bin", &data);
        assert_ne!(quick_hash(&a).unwrap(), quick_hash(&b).unwrap());
    }

    #[test]
    fn identical_content_gives_identical_hashes() {
        let a = write_temp("c.bin", b"same bytes");
        let b = write_temp("d.bin", b"same bytes");
        assert_eq!(full_hash(&a).unwrap(), full_hash(&b).unwrap());
        assert_eq!(quick_hash(&a).unwrap(), quick_hash(&b).unwrap());
    }
}
