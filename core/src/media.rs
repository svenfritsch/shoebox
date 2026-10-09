//! Reading metadata and producing preview images. Originals are only ever
//! opened for reading.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use image::{DynamicImage, ImageDecoder, RgbImage};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use nom_exif::{EntryValue, ExifDateTime, ExifTag, MediaKind, MediaParser, MediaSource, TrackInfoTag};
use serde::Serialize;

use crate::classify::Kind;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Metadata {
    /// Capture time as local wall-clock time, `YYYY-MM-DDTHH:MM:SS`: EXIF
    /// `DateTimeOriginal` for images, container creation date (UTC) for videos.
    pub taken: Option<String>,
    /// UTC offset of `taken` (`+03:00`), when the file records one.
    pub taken_offset: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub camera: Option<String>,
    /// Whether the file carries an embedded EXIF preview image.
    pub has_embedded_thumb: bool,
    /// Where it was taken, decimal degrees (EXIF GPS, video location).
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// A position from a file, or None for one that cannot be real: out of range
/// or exactly 0,0 (what cameras without a fix write).
pub fn valid_position(lat: Option<f64>, lon: Option<f64>) -> Option<(f64, f64)> {
    let (lat, lon) = (lat?, lon?);
    let ok = lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon);
    (ok && !(lat == 0.0 && lon == 0.0)).then_some((lat, lon))
}

fn apply_gps(meta: &mut Metadata, gps: Option<&nom_exif::GPSInfo>) {
    if let Some((lat, lon)) = gps.and_then(|g| valid_position(g.latitude_decimal(), g.longitude_decimal())) {
        (meta.lat, meta.lon) = (Some(lat), Some(lon));
    }
}

/// Read capture date, dimensions and camera. A file without EXIF (e.g. a PNG
/// screenshot) is not an error; it simply has no capture date.
pub fn read_metadata(parser: &mut MediaParser, kind: Kind, path: &Path) -> Result<Metadata> {
    let kind = content_kind(kind, path);
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
            (meta.taken, meta.taken_offset) = split_date(track.get(TrackInfoTag::CreateDate));
            meta.width = track.get(TrackInfoTag::Width).and_then(|v| v.as_u32());
            meta.height = track.get(TrackInfoTag::Height).and_then(|v| v.as_u32());
            meta.duration_ms = track.get(TrackInfoTag::DurationMs).and_then(|v| v.as_u64());
            meta.camera = track.get(TrackInfoTag::Model).map(|v| v.to_string());
            apply_gps(&mut meta, track.gps_info());
        }
    }
    if meta.width.is_none() && matches!(kind, Kind::Jpeg | Kind::Png) {
        // Not every writer records the size in EXIF; the image header has it.
        let dims = image::ImageReader::open(path)?.with_guessed_format()?.into_dimensions();
        if let Ok((w, h)) = dims {
            (meta.width, meta.height) = (Some(w), Some(h));
        }
    }
    Ok(meta)
}

fn apply_exif(meta: &mut Metadata, exif: &nom_exif::Exif) {
    (meta.taken, meta.taken_offset) =
        split_date(exif.get(ExifTag::DateTimeOriginal).or_else(|| exif.get(ExifTag::CreateDate)));
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
    apply_gps(meta, exif.gps_info());
    meta.has_embedded_thumb = exif
        .entries()
        .any(|e| e.tag().tag() == Some(ExifTag::ThumbnailLength));
}

/// Normalise a date value into sortable local time plus optional offset.
fn split_date(value: Option<&EntryValue>) -> (Option<String>, Option<String>) {
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
    match value.and_then(EntryValue::as_datetime) {
        Some(ExifDateTime::Aware(dt)) => (Some(dt.format(FORMAT).to_string()), Some(dt.offset().to_string())),
        Some(ExifDateTime::Naive(dt)) => (Some(dt.format(FORMAT).to_string()), None),
        None => (None, None),
    }
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

/// What an image file really is, going by its first bytes rather than its
/// name: some exports keep a `.HEIC` name on a JPEG, or the other way round.
/// Videos and RAW files, and images that start like none of the formats
/// below, keep the kind their extension gave them.
pub fn content_kind(kind: Kind, path: &Path) -> Kind {
    use std::io::Read;
    if !matches!(kind, Kind::Jpeg | Kind::Png | Kind::Heic) {
        return kind;
    }
    let mut head = Vec::with_capacity(12);
    let read = std::fs::File::open(path).and_then(|f| f.take(12).read_to_end(&mut head));
    if read.is_err() {
        return kind;
    }
    sniff(&head).unwrap_or(kind)
}

fn sniff(head: &[u8]) -> Option<Kind> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    // ISO base media: size, `ftyp`, major brand. Only the HEIF brands; an
    // MP4 or MOV is left to its extension.
    match head.get(4..12) {
        Some([b'f', b't', b'y', b'p', brand @ ..])
            if matches!(brand, b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1") =>
        {
            Some(Kind::Heic)
        }
        _ => None,
    }
}

/// Decode an image and write a JPEG preview whose longer edge is `edge` px.
pub fn write_image_thumb(lib_heif: &LibHeif, kind: Kind, src: &Path, dst: &Path, edge: u32) -> Result<()> {
    decode_image(lib_heif, kind, src, edge)?.thumbnail(edge, edge).to_rgb8().save(dst)?;
    Ok(())
}

/// Decode an image upright (EXIF orientation applied) at a size whose longer
/// edge is at least `edge` px where the decoder can scale cheaply (JPEG,
/// HEIC); callers shrink the result to the exact size.
pub fn decode_image(lib_heif: &LibHeif, kind: Kind, src: &Path, edge: u32) -> Result<DynamicImage> {
    match content_kind(kind, src) {
        // jpeg-decoder rejects a few exotic files that `image` reads.
        Kind::Jpeg => decode_jpeg_scaled(src, edge).or_else(|_| decode_oriented(src)),
        Kind::Png => decode_oriented(src),
        // libheif applies the HEIF rotation and mirroring itself.
        Kind::Heic => decode_heic(lib_heif, src, edge),
        other => bail!("no image decoder for {}", other.label()),
    }
}

fn decode_oriented(src: &Path) -> Result<DynamicImage> {
    let mut decoder = image::ImageReader::open(src)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// JPEG with DCT scaling (1/2, 1/4 or 1/8 straight from the IDCT), which is
/// several times faster than decoding a 12-megapixel photo in full.
fn decode_jpeg_scaled(src: &Path, edge: u32) -> Result<DynamicImage> {
    use jpeg_decoder::{Decoder, PixelFormat};
    let mut decoder = Decoder::new(std::io::BufReader::new(std::fs::File::open(src)?));
    let edge = edge.min(u16::MAX as u32) as u16;
    decoder.scale(edge, edge)?;
    let pixels = decoder.decode()?;
    let info = decoder.info().ok_or_else(|| anyhow!("no JPEG header"))?;
    let (w, h) = (info.width as u32, info.height as u32);
    let bad = || anyhow!("JPEG pixel buffer does not match its size");
    let mut img = match info.pixel_format {
        PixelFormat::RGB24 => DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, pixels).ok_or_else(bad)?),
        PixelFormat::L8 => DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, pixels).ok_or_else(bad)?),
        PixelFormat::L16 => {
            let px = pixels.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            DynamicImage::ImageLuma16(image::ImageBuffer::from_raw(w, h, px).ok_or_else(bad)?)
        }
        // Rare (print workflows); `image` gets the colour conversion right.
        PixelFormat::CMYK32 => bail!("CMYK JPEG"),
    };
    if let Some(o) = decoder.exif_data().and_then(image::metadata::Orientation::from_exif_chunk) {
        img.apply_orientation(o);
    }
    Ok(img)
}

/// Encode a preview as JPEG.
/// A JPEG turned by `quarters` quarter turns clockwise and encoded again.
pub fn turn_jpeg(bytes: &[u8], quarters: i32, quality: u8) -> Result<Vec<u8>> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)?;
    let img = match quarters.rem_euclid(4) {
        1 => img.rotate90(),
        2 => img.rotate180(),
        3 => img.rotate270(),
        _ => img,
    };
    encode_jpeg(&img, quality)
}

pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&img.to_rgb8())?;
    Ok(out)
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
    std::fs::write(dst, video_poster(ffmpeg, src, duration_ms, edge, 3)?)?;
    Ok(())
}

/// One frame at ~10% into the video, as JPEG bytes (`quality` is ffmpeg's
/// `-q:v`, 2 = best). ffmpeg only reads the video and writes to a pipe.
pub fn video_poster(ffmpeg: &Path, src: &Path, duration_ms: Option<u64>, edge: u32, quality: u8) -> Result<Vec<u8>> {
    let at = duration_ms.map(|d| d as f64 / 10_000.0).unwrap_or(1.0);
    let poster = |at: f64| -> Result<Vec<u8>> {
        let output = Command::new(ffmpeg)
            .args(["-v", "error", "-nostdin", "-ss", &format!("{at:.3}"), "-i"])
            .arg(src)
            .args([
                "-frames:v",
                "1",
                "-vf",
                // Shrink the longer edge to `edge`; never enlarge.
                &format!("scale='if(gt(iw,ih),min({edge},iw),-2)':'if(gt(iw,ih),-2,min({edge},ih))'"),
                "-q:v",
                &quality.to_string(),
                "-f",
                "image2pipe",
                "-c:v",
                "mjpeg",
                "-",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .context("run ffmpeg")?;
        if !output.status.success() || output.stdout.is_empty() {
            bail!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(output.stdout)
    };
    // Seeking past the end of a very short clip yields no frame.
    poster(at).or_else(|e| if at > 0.0 { poster(0.0) } else { Err(e) })
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
