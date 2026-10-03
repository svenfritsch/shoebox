//! Reading metadata and producing preview images. Originals are only ever
//! opened for reading.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use image::{DynamicImage, RgbImage};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use nom_exif::{ExifTag, MediaKind, MediaParser, MediaSource, TrackInfoTag};
use serde::Serialize;

use crate::classify::Kind;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Metadata {
    /// EXIF `DateTimeOriginal` for images, container creation date for videos.
    pub taken: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub camera: Option<String>,
    /// Whether the file carries an embedded EXIF preview image.
    pub has_embedded_thumb: bool,
}

/// Read capture date, dimensions and camera. A file without EXIF (e.g. a PNG
/// screenshot) is not an error; it simply has no capture date.
pub fn read_metadata(parser: &mut MediaParser, kind: Kind, path: &Path) -> Result<Metadata> {
    if kind == Kind::Heic {
        return read_heic_metadata(parser, path);
    }
    let source = MediaSource::open(path).context("open")?;
    let mut meta = Metadata::default();
    match source.kind() {
        MediaKind::Image => match parser.parse_exif(source) {
            Ok(iter) => apply_exif(&mut meta, &iter.into()),
            Err(nom_exif::Error::ExifNotFound) => {}
            Err(e) => return Err(e.into()),
        },
        MediaKind::Track => {
            let track = parser.parse_track(source)?;
            meta.taken = track.get(TrackInfoTag::CreateDate).map(|v| v.to_string());
            meta.width = track.get(TrackInfoTag::Width).and_then(|v| v.as_u32());
            meta.height = track.get(TrackInfoTag::Height).and_then(|v| v.as_u32());
            meta.duration_ms = track.get(TrackInfoTag::DurationMs).and_then(|v| v.as_u64());
            meta.camera = track.get(TrackInfoTag::Model).map(|v| v.to_string());
        }
    }
    Ok(meta)
}

fn apply_exif(meta: &mut Metadata, exif: &nom_exif::Exif) {
    meta.taken = exif
        .get(ExifTag::DateTimeOriginal)
        .or_else(|| exif.get(ExifTag::CreateDate))
        .map(|v| v.to_string());
    meta.width = meta.width.or_else(|| {
        exif.get(ExifTag::ExifImageWidth)
            .or_else(|| exif.get(ExifTag::ImageWidth))
            .and_then(|v| v.as_u32())
    });
    meta.height = meta.height.or_else(|| {
        exif.get(ExifTag::ExifImageHeight)
            .or_else(|| exif.get(ExifTag::ImageHeight))
            .and_then(|v| v.as_u32())
    });
    meta.camera = exif.get(ExifTag::Model).map(|v| v.to_string());
    meta.has_embedded_thumb = exif
        .entries()
        .any(|e| e.tag().tag() == Some(ExifTag::ThumbnailLength));
}

/// HEIC containers vary a lot between writers, so we let libheif (the
/// reference implementation) locate the EXIF block and only parse the TIFF
/// payload with nom-exif.
fn read_heic_metadata(parser: &mut MediaParser, path: &Path) -> Result<Metadata> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("non-UTF-8 path"))?;
    let ctx = HeifContext::read_from_file(path_str)?;
    let handle = ctx.primary_image_handle()?;
    let mut meta = Metadata {
        width: Some(handle.width()),
        height: Some(handle.height()),
        ..Metadata::default()
    };

    let mut ids = [0; 1];
    if handle.metadata_block_ids(&mut ids, b"Exif") == 0 {
        return Ok(meta);
    }
    let block = handle.metadata(ids[0])?;
    // The block starts with a 4-byte big-endian offset to the TIFF header.
    let offset = block
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| anyhow!("EXIF block too short"))?;
    let tiff = block.get(4 + offset..).ok_or_else(|| anyhow!("bad EXIF offset"))?.to_vec();
    let exif: nom_exif::Exif = parser.parse_exif(MediaSource::from_memory(tiff)?)?.into();
    apply_exif(&mut meta, &exif);
    Ok(meta)
}

/// Decode an image and write a JPEG preview whose longer edge is `edge` px.
pub fn write_image_thumb(lib_heif: &LibHeif, kind: Kind, src: &Path, dst: &Path, edge: u32) -> Result<()> {
    let img = match kind {
        Kind::Jpeg | Kind::Png => image::open(src)?,
        Kind::Heic => decode_heic(lib_heif, src, edge)?,
        other => bail!("no image decoder for {}", other.label()),
    };
    img.thumbnail(edge, edge).to_rgb8().save(dst)?;
    Ok(())
}

fn decode_heic(lib_heif: &LibHeif, src: &Path, edge: u32) -> Result<DynamicImage> {
    let path = src.to_str().ok_or_else(|| anyhow!("non-UTF-8 path"))?;
    let ctx = HeifContext::read_from_file(path)?;
    let handle = ctx.primary_image_handle()?;
    let decoded = lib_heif.decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)?;

    // Let libheif do the downscale; it is much cheaper than converting the
    // full-resolution frame first.
    let (w, h) = (decoded.width(), decoded.height());
    let scale = (edge as f64 / w.max(h) as f64).min(1.0);
    let (tw, th) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    let small = decoded.scale(tw, th, None)?;

    let planes = small.planes();
    let plane = planes.interleaved.ok_or_else(|| anyhow!("no interleaved plane"))?;
    let mut rgb = Vec::with_capacity((plane.width * plane.height * 3) as usize);
    for row in 0..plane.height as usize {
        let start = row * plane.stride;
        rgb.extend_from_slice(&plane.data[start..start + plane.width as usize * 3]);
    }
    let buf = RgbImage::from_raw(plane.width, plane.height, rgb).ok_or_else(|| anyhow!("bad plane size"))?;
    Ok(DynamicImage::ImageRgb8(buf))
}

/// Find an `ffmpeg` binary: next to our own executable first, then on PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Grab one frame at ~10% into the video as a JPEG preview.
pub fn write_video_poster(ffmpeg: &Path, src: &Path, dst: &Path, duration_ms: Option<u64>, edge: u32) -> Result<()> {
    let at = duration_ms.map(|d| d as f64 / 10_000.0).unwrap_or(1.0);
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-nostdin", "-ss", &format!("{at:.3}"), "-i"])
        .arg(src)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale='min({edge},iw)':-2"),
            "-f",
            "image2",
            "-y",
        ])
        .arg(dst)
        .output()
        .context("run ffmpeg")?;
    if !output.status.success() || !dst.is_file() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// A JPEG must end with the EOI marker (FF D9). Decoders happily render a
/// cut-off file with a grey bottom, so this is our hint for damaged files.
/// Some phones append data after EOI (e.g. Motion Photos); those show up as
/// false positives and are only reported as a warning.
pub fn jpeg_missing_end_marker(path: &Path) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() < 2 {
        return Ok(true);
    }
    file.seek(SeekFrom::End(-2))?;
    let mut tail = [0u8; 2];
    file.read_exact(&mut tail)?;
    Ok(tail != [0xFF, 0xD9])
}
