//! Video posters from AVFoundation, the system's own video framework on
//! macOS (what QuickTime and Photos use): H.264, HEVC and ProRes in
//! `.mov`, `.mp4`, `.m4v`, `.3gp`, turned upright, without ffmpeg. The video
//! is only read; callers wrap this in `fingerprint::read_unchanged`.

use std::path::Path;

use image::{DynamicImage, RgbaImage};
use objc2::rc::autoreleasepool;
use objc2_av_foundation::{AVAssetImageGenerator, AVURLAsset};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, kCGColorSpaceSRGB,
};
use objc2_core_media::CMTime;
use objc2_foundation::{NSString, NSURL};

/// One frame about 10% into the video (the first is often black), at most
/// `edge` pixels on its longer side.
pub fn frame(path: &Path, duration_ms: Option<u64>, edge: u32) -> Result<DynamicImage, String> {
    let path = path.to_str().ok_or("AVFoundation: the path is not UTF-8")?;
    autoreleasepool(|_| {
        // SAFETY: plain calls into AVFoundation with valid, retained objects;
        // the generator and asset live for the whole block.
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
            // Portrait videos are stored lying down with a turn in the track.
            generator.setAppliesPreferredTrackTransform(true);
            generator.setMaximumSize(CGSize::new(edge as f64, edge as f64));
            let seconds = match duration_ms {
                Some(ms) => ms as f64 / 10_000.0,
                None => {
                    let d = asset.duration().seconds();
                    if d.is_finite() && d > 0.0 { d / 10.0 } else { 0.0 }
                }
            };
            let grab = |seconds: f64| {
                #[allow(deprecated)] // the replacement needs macOS 13; we run on 10.13
                generator
                    .copyCGImageAtTime_actualTime_error(CMTime::with_seconds(seconds, 600), std::ptr::null_mut())
                    .map_err(|e| format!("AVFoundation: {}", e.localizedDescription()))
            };
            // Seeking past the end of a very short clip yields no frame.
            let image = grab(seconds).or_else(|e| if seconds > 0.0 { grab(0.0) } else { Err(e) })?;
            to_rgba(&image)
        }
    })
}

/// Draw a CGImage into an sRGB bitmap of our own (any pixel format, colour
/// space or HDR in the video comes out as 8-bit RGBA).
fn to_rgba(image: &CGImage) -> Result<DynamicImage, String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("AVFoundation: unexpected frame size {w}×{h}"));
    }
    let mut pixels = vec![0u8; w * h * 4];
    // SAFETY: `pixels` holds h rows of w×4 bytes and outlives the context,
    // which is dropped at the end of this block.
    unsafe {
        let srgb = CGColorSpace::with_name(Some(kCGColorSpaceSRGB)).ok_or("AVFoundation: no sRGB colour space")?;
        let context: objc2_core_foundation::CFRetained<CGContext> = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            Some(&srgb),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
        .ok_or("AVFoundation: no bitmap context")?;
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
        CGContext::draw_image(Some(&context), rect, Some(image));
    }
    let rgba = RgbaImage::from_raw(w as u32, h as u32, pixels).ok_or("AVFoundation: bad frame buffer")?;
    // Video frames are opaque, so premultiplied alpha changes nothing.
    Ok(DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).to_rgb8()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Clips are made with ffmpeg (on the CI Mac from Homebrew); without it
    /// there is nothing to test with.
    fn ffmpeg(args: &[&str]) -> bool {
        let Some(ff) = crate::media::find_ffmpeg() else { return false };
        Command::new(ff).args(["-v", "error", "-nostdin", "-y"]).args(args).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn frames_come_small_and_upright() {
        let dir = std::env::temp_dir().join(format!("shoebox-avposter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let land = dir.join("land scape.mp4");
        let land_s = land.to_str().unwrap();
        let lavfi = "testsrc2=size=1280x720:rate=30:duration=3";
        if !ffmpeg(&["-f", "lavfi", "-i", lavfi, "-c:v", "libx264", "-pix_fmt", "yuv420p", land_s]) {
            eprintln!("no ffmpeg with libx264: skipped");
            return;
        }
        let img = frame(&land, None, 384).unwrap();
        assert_eq!((img.width(), img.height()), (384, 216));
        // A known duration picks the time without asking the file.
        let img = frame(&land, Some(3000), 384).unwrap();
        assert_eq!((img.width(), img.height()), (384, 216));

        // Stored lying down with a quarter turn in the track, as phones store
        // portrait videos: the poster comes upright.
        let turned = dir.join("turned.mov");
        if ffmpeg(&["-display_rotation", "90", "-i", land_s, "-c", "copy", turned.to_str().unwrap()]) {
            let img = frame(&turned, None, 384).unwrap();
            assert_eq!((img.width(), img.height()), (216, 384));
        } else {
            eprintln!("this ffmpeg cannot set a display rotation: turned clip skipped");
        }

        let junk = dir.join("junk.mov");
        std::fs::write(&junk, b"not a video").unwrap();
        assert!(frame(&junk, None, 384).unwrap_err().starts_with("AVFoundation: "));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
