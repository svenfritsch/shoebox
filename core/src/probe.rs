//! `shoebox probe`: the phase-0 check that this machine and this drive can
//! do everything shoebox needs, without changing a single original.
//!
//! 1. walk the folder (skipping macOS bookkeeping files)
//! 2. record size, timestamps and full hash of every media file
//! 3. read metadata and render a preview of every file
//! 4. record everything again and require it to be identical

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use libheif_rs::LibHeif;
use nom_exif::MediaParser;
use serde::Serialize;
use unicode_normalization::{UnicodeNormalization, is_nfc};
use walkdir::WalkDir;

use crate::classify::{self, Kind};
use crate::fingerprint::{self, Stamp};
use crate::fsinfo;
use crate::media::{self, Metadata};

const THUMB_EDGE: u32 = 256;

pub struct Options {
    pub root: PathBuf,
    pub thumbs: PathBuf,
    pub report: PathBuf,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
struct FileResult {
    path: String,
    kind: Kind,
    size: u64,
    quick_hash: String,
    full_hash: String,
    metadata: Option<Metadata>,
    metadata_error: Option<String>,
    metadata_ms: f64,
    preview: Option<String>,
    preview_error: Option<String>,
    preview_ms: f64,
    warning: Option<String>,
}

#[derive(Serialize)]
struct GuardViolation {
    path: String,
    before: Option<Stamp>,
    after: Option<Stamp>,
    hash_changed: bool,
}

#[derive(Serialize)]
struct Report {
    shoebox_version: &'static str,
    os: String,
    root: String,
    filesystem: Option<String>,
    ffmpeg: Option<String>,
    files: Vec<FileResult>,
    ignored_entries: usize,
    non_nfc_names: Vec<String>,
    raw_pairs: usize,
    live_photo_pairs: usize,
    bytes_hashed: u64,
    hash_seconds: f64,
    guard_violations: Vec<GuardViolation>,
}

pub fn run(opts: Options) -> Result<bool> {
    let root = opts.root.canonicalize().with_context(|| format!("cannot open {}", opts.root.display()))?;
    std::fs::create_dir_all(&opts.thumbs)?;
    let thumbs = opts.thumbs.canonicalize()?;
    if thumbs.starts_with(&root) {
        bail!("the preview folder must not be inside the photo folder (it would add files there)");
    }

    println!("shoebox probe {}", env!("CARGO_PKG_VERSION"));
    println!("  system:      {}", fsinfo::os_description());
    let filesystem = fsinfo::filesystem_type(&root);
    println!("  folder:      {}", root.display());
    println!("  filesystem:  {}", filesystem.as_deref().unwrap_or("unknown"));
    let ffmpeg = media::find_ffmpeg();
    println!(
        "  ffmpeg:      {}",
        ffmpeg.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not found (video previews skipped)".into())
    );
    println!();

    // 1. Walk.
    let (paths, ignored_entries, non_nfc_names) = walk(&root, opts.limit)?;
    println!("Found {} media files ({} system/hidden entries skipped).", paths.len(), ignored_entries);

    // 2. Snapshot before.
    print!("Hashing… ");
    let started = Instant::now();
    let mut before = Vec::with_capacity(paths.len());
    let mut bytes_hashed = 0;
    for (path, _) in &paths {
        let stamp = fingerprint::stamp(path)?;
        bytes_hashed += stamp.size;
        let quick = fingerprint::quick_hash(path)?;
        let full = fingerprint::full_hash(path)?;
        before.push((stamp, quick, full));
    }
    let hash_seconds = started.elapsed().as_secs_f64();
    println!("{} in {:.1}s ({})", human_bytes(bytes_hashed), hash_seconds, throughput(bytes_hashed, started.elapsed()));

    // 3. Metadata and previews.
    println!("Reading metadata and rendering previews…");
    let lib_heif = LibHeif::new();
    let mut parser = MediaParser::new();
    let mut files = Vec::with_capacity(paths.len());
    for (i, ((path, kind), (stamp, quick, full))) in paths.iter().zip(&before).enumerate() {
        let rel = rel_path(&root, path);

        let t = Instant::now();
        let (metadata, metadata_error) = match media::read_metadata(&mut parser, *kind, path) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let metadata_ms = ms(t.elapsed());

        let thumb_path = thumbs.join(format!("{i:05}.jpg"));
        let t = Instant::now();
        let preview_result = match kind {
            Kind::Raw => Err(None), // RAW files are indexed but never shown
            Kind::Video => match &ffmpeg {
                Some(ff) => {
                    let duration = metadata.as_ref().and_then(|m| m.duration_ms);
                    media::write_video_poster(ff, path, &thumb_path, duration, THUMB_EDGE).map_err(Some)
                }
                None => Err(None),
            },
            _ => media::write_image_thumb(&lib_heif, *kind, path, &thumb_path, THUMB_EDGE).map_err(Some),
        };
        let preview_ms = ms(t.elapsed());
        let (preview, preview_error) = match preview_result {
            Ok(()) => (Some(thumb_path.display().to_string()), None),
            Err(Some(e)) => (None, Some(format!("{e:#}"))),
            Err(None) => (None, None),
        };

        let warning = match kind {
            Kind::Jpeg if media::jpeg_missing_end_marker(path).unwrap_or(false) => {
                Some("no JPEG end marker: file may be truncated or damaged".to_string())
            }
            _ => None,
        };

        files.push(FileResult {
            path: rel,
            kind: *kind,
            size: stamp.size,
            quick_hash: quick.clone(),
            full_hash: full.clone(),
            metadata,
            metadata_error,
            metadata_ms,
            preview,
            preview_error,
            preview_ms,
            warning,
        });
    }

    // 4. Snapshot after: nothing may have changed.
    print!("Checking that no original was modified… ");
    let mut guard_violations = Vec::new();
    for ((path, _), (stamp_before, _, hash_before)) in paths.iter().zip(&before) {
        let stamp_after = fingerprint::stamp(path).ok();
        let hash_after = fingerprint::full_hash(path).ok();
        let hash_changed = hash_after.as_deref() != Some(hash_before.as_str());
        if stamp_after.as_ref() != Some(stamp_before) || hash_changed {
            guard_violations.push(GuardViolation {
                path: rel_path(&root, path),
                before: Some(stamp_before.clone()),
                after: stamp_after,
                hash_changed,
            });
        }
    }
    println!("{}", if guard_violations.is_empty() { "OK" } else { "FAILED" });

    let (raw_pairs, live_photo_pairs) = count_pairs(&paths);

    let report = Report {
        shoebox_version: env!("CARGO_PKG_VERSION"),
        os: fsinfo::os_description(),
        root: root.display().to_string(),
        filesystem,
        ffmpeg: ffmpeg.map(|p| p.display().to_string()),
        files,
        ignored_entries,
        non_nfc_names,
        raw_pairs,
        live_photo_pairs,
        bytes_hashed,
        hash_seconds,
        guard_violations,
    };
    print_summary(&report);

    std::fs::write(&opts.report, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("write report to {}", opts.report.display()))?;
    println!("\nFull report: {}", opts.report.display());
    println!("Previews:    {}", thumbs.display());

    Ok(report.guard_violations.is_empty())
}

type Walked = (Vec<(PathBuf, Kind)>, usize, Vec<String>);

fn walk(root: &Path, limit: Option<usize>) -> Result<Walked> {
    let mut paths = Vec::new();
    let mut ignored = 0;
    let mut non_nfc = Vec::new();
    let walker = WalkDir::new(root).sort_by_file_name().into_iter().filter_entry(|e| {
        e.depth() == 0 || !classify::is_ignored(&e.file_name().to_string_lossy())
    });
    for entry in walker {
        let entry = entry?;
        if entry.depth() > 0 {
            let name = entry.file_name().to_string_lossy();
            if !is_nfc(&name) {
                non_nfc.push(rel_path(root, entry.path()));
            }
        }
        if !entry.file_type().is_file() {
            continue;
        }
        match classify::kind_of(entry.path()) {
            Some(kind) => paths.push((entry.into_path(), kind)),
            None => {}
        }
        if limit.is_some_and(|l| paths.len() >= l) {
            break;
        }
    }
    // Count what the filter skipped, separately, so the walk above stays simple.
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if entry.depth() > 0 && classify::is_ignored(&entry.file_name().to_string_lossy()) {
            ignored += 1;
        }
    }
    Ok((paths, ignored, non_nfc))
}

/// RAW+JPEG/HEIC pairs and Live Photos (still + short MOV) share a folder and a
/// file stem.
fn count_pairs(paths: &[(PathBuf, Kind)]) -> (usize, usize) {
    let mut groups: HashMap<(PathBuf, String), Vec<Kind>> = HashMap::new();
    for (path, kind) in paths {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path.file_stem().map(|s| s.to_string_lossy().nfc().collect::<String>().to_lowercase());
        groups.entry((dir, stem.unwrap_or_default())).or_default().push(*kind);
    }
    let still = |k: &Kind| matches!(k, Kind::Jpeg | Kind::Heic);
    let raw = groups.values().filter(|ks| ks.contains(&Kind::Raw) && ks.iter().any(still)).count();
    let live = groups.values().filter(|ks| ks.contains(&Kind::Video) && ks.iter().any(still)).count();
    (raw, live)
}

fn print_summary(r: &Report) {
    #[derive(Default)]
    struct Row {
        files: usize,
        meta_ok: usize,
        dated: usize,
        previews: usize,
        preview_ms: f64,
    }
    let mut rows: BTreeMap<Kind, Row> = BTreeMap::new();
    for f in &r.files {
        let row = rows.entry(f.kind).or_default();
        row.files += 1;
        if f.metadata.is_some() {
            row.meta_ok += 1;
        }
        if f.metadata.as_ref().is_some_and(|m| m.taken.is_some()) {
            row.dated += 1;
        }
        if f.preview.is_some() {
            row.previews += 1;
            row.preview_ms += f.preview_ms;
        }
    }

    println!();
    println!("{:<7} {:>6} {:>9} {:>11} {:>9} {:>12}", "Kind", "Files", "Metadata", "Date found", "Preview", "ms/preview");
    for (kind, row) in &rows {
        let preview = if *kind == Kind::Raw { "–".to_string() } else { row.previews.to_string() };
        let avg = if row.previews > 0 { format!("{:.0}", row.preview_ms / row.previews as f64) } else { "–".into() };
        println!(
            "{:<7} {:>6} {:>9} {:>11} {:>9} {:>12}",
            kind.label(),
            row.files,
            row.meta_ok,
            row.dated,
            preview,
            avg
        );
    }
    println!();
    println!("RAW+JPEG/HEIC pairs:  {}", r.raw_pairs);
    println!("Live Photo pairs:     {}", r.live_photo_pairs);
    println!("Non-NFC file names:   {} (umlauts stored decomposed; handled by normalizing)", r.non_nfc_names.len());

    let warnings: Vec<_> = r.files.iter().filter_map(|f| f.warning.as_ref().map(|w| format!("{}: {w}", f.path))).collect();
    if !warnings.is_empty() {
        println!("\nWarnings ({}):", warnings.len());
        for w in &warnings {
            println!("  {w}");
        }
    }

    let errors: Vec<_> = r
        .files
        .iter()
        .flat_map(|f| {
            f.metadata_error
                .iter()
                .map(move |e| format!("{}: metadata: {e}", f.path))
                .chain(f.preview_error.iter().map(move |e| format!("{}: preview: {e}", f.path)))
        })
        .collect();
    if !errors.is_empty() {
        println!("\nProblems ({}):", errors.len());
        for e in errors.iter().take(15) {
            println!("  {e}");
        }
        if errors.len() > 15 {
            println!("  … {} more in the report", errors.len() - 15);
        }
    }

    println!();
    if r.guard_violations.is_empty() {
        println!("GUARD OK: all {} originals unchanged (size, modified, created, content).", r.files.len());
    } else {
        println!("GUARD FAILED: {} originals changed!", r.guard_violations.len());
        for v in &r.guard_violations {
            println!("  {} (content changed: {})", v.path, v.hash_changed);
        }
    }
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

fn throughput(bytes: u64, d: Duration) -> String {
    let secs = d.as_secs_f64().max(1e-9);
    format!("{}/s", human_bytes((bytes as f64 / secs) as u64))
}
