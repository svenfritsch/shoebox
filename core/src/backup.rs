//! `shoebox backup <original drive> <backup drive>`: is the backup complete?
//!
//! shoebox does not copy; rsync or Carbon Copy Cloner do. Both drives keep
//! their own index, so the comparison is one query over the two
//! `library.db` files (`multi::backup_report`, by full hash): what is new
//! since the last backup, what is different on the backup, what only the
//! backup has. Neither drive is written to. With `--deep` the backup drive's
//! own files are then re-read and compared with its index (`shoebox verify`
//! under the guard), which finds bit rot.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::multi::{self, BackupReport};
use crate::say;
use crate::{db, report, verify};

pub struct Options {
    /// The drive that is copied (scanned, with full hashes).
    pub primary: PathBuf,
    /// The copy (scanned, with full hashes).
    pub backup: PathBuf,
    /// Also re-read the backup's files and check them against its index.
    pub deep: bool,
    /// How many files of each kind to list.
    pub limit: usize,
}

#[derive(Serialize)]
pub struct Check {
    pub report: BackupReport,
    /// Only with `--deep`.
    pub verify: Option<verify::Report>,
    /// Everything is in order.
    pub ok: bool,
}

fn index_of(root: &std::path::Path) -> Result<(PathBuf, String)> {
    let root = root.canonicalize().with_context(|| format!("cannot open {}", root.display()))?;
    let db = db::default_path(&root);
    if !db.is_file() {
        bail!("no index at {} (run `shoebox scan` on that drive first)", db.display());
    }
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
    Ok((db, name))
}

fn days_ago(unix: Option<i64>) -> String {
    match unix {
        None => "never".into(),
        Some(t) => match (db::now() - t).max(0) / 86_400 {
            0 => "today".into(),
            1 => "1 day ago".into(),
            n => format!("{n} days ago"),
        },
    }
}

pub fn run(opts: &Options) -> Result<Check> {
    let (pdb, pname) = index_of(&opts.primary)?;
    let (bdb, bname) = index_of(&opts.backup)?;
    if pdb == bdb {
        bail!("the backup must be another drive than the one it copies");
    }
    say!("Comparing {bname} (backup) with {pname}…");
    let r = multi::backup_report(&pdb, &pname, &bdb, &bname, opts.limit)?;
    say!("{} files of {pname} compared by content: {} are on the backup.", r.compared, r.covered);
    if r.unhashed > 0 {
        say!("{} files of {pname} have no full hash yet (run `shoebox scan` on it); they are not compared.", r.unhashed);
    }
    let list = |title: &str, n: u64, files: &[multi::BackupFile], failed: bool, note: &str| {
        if n == 0 {
            return;
        }
        say!("{title}: {n}");
        for f in files {
            say!("  {}", f.path);
            if failed {
                report::file(&f.path, false, note);
            }
        }
        if n as usize > files.len() {
            say!("  … {} more", n as usize - files.len());
        }
    };
    list("Not on the backup yet (new since the last backup)", r.missing, &r.missing_files, true, "not on the backup yet");
    list("Different on the backup (same path, other content)", r.different, &r.different_files, true, "other content on the backup");
    list(&format!("Only on the backup (gone or changed on {pname} since)"), r.extra, &r.extra_files, false, "");
    say!(
        "Backup drive: last scanned {}, files last arrived {}.",
        days_ago(r.backup_last_scan),
        days_ago(r.backup_last_new_files)
    );
    if r.up_to_date {
        say!("The backup holds everything {pname} has.");
    }

    let verify = if opts.deep {
        say!("Checking the files on {bname} against its index (this reads them)…");
        Some(verify::run(&verify::Options { root: opts.backup.clone(), db: None, quick: false, limit: None })?)
    } else {
        None
    };
    let ok = r.up_to_date && verify.as_ref().is_none_or(|v| v.is_clean());
    Ok(Check { report: r, verify, ok })
}
