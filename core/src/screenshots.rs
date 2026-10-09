//! Is this picture a screenshot? Phase 11 ([docs/phase11.md](../../docs/phase11.md)).
//!
//! No model: a score from what the index knows (kind, name, size, camera) and
//! one number from the pixels of the thumbnail (how much of the picture is
//! flat colour). The score is a guess; the user's own decision
//! (`shot_marks`) always wins over it. Nothing is read from or written to an
//! original here.

use image::DynamicImage;

use crate::classify::Kind;

/// From this score on a picture counts as a screenshot. Set from numbers on
/// the real drive (docs/phase11.md, step 1), not by the code.
pub const THRESHOLD: u32 = 60;

/// Parts of the name that mean a screenshot, lower case and NFC.
const NAME_WORDS: &[&str] =
    &["screenshot", "screen shot", "screen-shot", "screen_shot", "bildschirmfoto", "bildschirmaufnahme", "bildschirmkopie"];

/// Pixel sizes of phone, tablet and computer displays (either way round).
const DISPLAYS: &[(u32, u32)] = &[
    // iPhone
    (640, 1136), (750, 1334), (828, 1792), (1080, 1920), (1125, 2436), (1170, 2532), (1179, 2556),
    (1206, 2622), (1242, 2208), (1242, 2688), (1284, 2778), (1290, 2796), (1320, 2868),
    // Android
    (720, 1280), (720, 1600), (1080, 2160), (1080, 2240), (1080, 2340), (1080, 2400), (1080, 2412),
    (1344, 2992), (1440, 2960), (1440, 3040), (1440, 3120), (1440, 3200),
    // iPad
    (1488, 2266), (1536, 2048), (1620, 2160), (1640, 2360), (1668, 2388), (2048, 2732),
    // Mac and monitors
    (1280, 800), (1366, 768), (1440, 900), (1536, 864), (1680, 1050), (1920, 1080), (1920, 1200),
    (2560, 1440), (2560, 1600), (2560, 1664), (2880, 1800), (2880, 1864), (3024, 1964),
    (3440, 1440), (3456, 2234), (3840, 2160), (5120, 2880),
];

/// How flat the picture is, 0 (a photograph) to 100 (a user interface): the
/// share of neighbouring pixels (right and below) with exactly the same
/// colour, from 0.40 (0) to 0.80 (100). Photos have sensor noise and soft
/// gradients; screens have large areas of one colour. Measured on the
/// thumbnail as it is stored (JPEG), so a backfill from stored thumbnails and
/// a fresh one agree.
pub fn pixel_score(img: &DynamicImage) -> u8 {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w < 8 || h < 8 {
        return 0;
    }
    let (mut same, mut total) = (0u64, 0u64);
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let p = rgb.get_pixel(x, y);
            total += 2;
            same += (rgb.get_pixel(x + 1, y) == p) as u64 + (rgb.get_pixel(x, y + 1) == p) as u64;
        }
    }
    let flat = same as f64 / total as f64;
    (((flat - 0.40) / 0.40) * 100.0).clamp(0.0, 100.0).round() as u8
}

pub fn name_says_screenshot(name: &str) -> bool {
    let name = name.to_lowercase();
    NAME_WORDS.iter().any(|w| name.contains(w))
}

pub fn is_display_size(width: u32, height: u32) -> bool {
    DISPLAYS.iter().any(|&(a, b)| (width, height) == (a, b) || (width, height) == (b, a))
}

/// The score, 0 to 100. `pixels` is `None` until the thumbnail exists.
/// A picture with a camera model is never more than 40 (a camera made it);
/// HEIC, RAW and videos are never screenshots.
pub fn score(name: &str, kind: Kind, width: Option<u32>, height: Option<u32>, camera: Option<&str>, pixels: Option<u8>) -> u32 {
    if !matches!(kind, Kind::Png | Kind::Jpeg) {
        return 0;
    }
    let has_camera = camera.is_some_and(|c| !c.trim().is_empty());
    let mut s = 0u32;
    if name_says_screenshot(name) {
        s += 50;
    }
    s += if kind == Kind::Png { 15 } else { 5 };
    if !has_camera {
        s += 10;
    }
    if let (Some(w), Some(h)) = (width, height)
        && is_display_size(w, h)
    {
        s += 25;
    }
    s += pixels.unwrap_or(0) as u32 / 2;
    if has_camera {
        s = s.min(40);
    }
    s.min(100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A made-up phone screen: white background, grey bars, a few coloured
    /// buttons and some "text" lines.
    fn interface(w: u32, h: u32) -> DynamicImage {
        let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        for y in 0..h / 10 {
            for x in 0..w {
                img.put_pixel(x, y, Rgb([240, 240, 245]));
            }
        }
        for row in 0..12 {
            let y0 = h / 8 + row * (h / 16);
            for y in y0..y0 + 3 {
                for x in w / 10..w * 7 / 10 {
                    if (x / 3 + row) % 4 != 0 && y < h {
                        img.put_pixel(x, y, Rgb([30, 30, 30]));
                    }
                }
            }
        }
        for y in h * 85 / 100..h * 92 / 100 {
            for x in w / 5..w * 4 / 5 {
                img.put_pixel(x, y, Rgb([0, 122, 255]));
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// A made-up photograph: a smooth gradient with sensor noise.
    fn photograph(w: u32, h: u32) -> DynamicImage {
        let mut seed = 12345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 24) % 9) as i32 - 4
        };
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let base = [(x * 255 / w) as i32, (y * 255 / h) as i32, 128];
                img.put_pixel(x, y, Rgb(base.map(|c| (c + noise()).clamp(0, 255) as u8)));
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    fn through_jpeg(img: &DynamicImage) -> DynamicImage {
        let bytes = crate::media::encode_jpeg(img, 80).unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[test]
    fn flatness_tells_screens_from_photos() {
        assert!(pixel_score(&through_jpeg(&interface(234, 384))) >= 70);
        assert_eq!(pixel_score(&through_jpeg(&photograph(384, 288))), 0);
        // Too small to say anything.
        assert_eq!(pixel_score(&DynamicImage::ImageRgb8(RgbImage::new(4, 4))), 0);
    }

    #[test]
    fn names() {
        for n in ["Screenshot 2024-03-02 at 10.11.12.png", "Bildschirmfoto 2024-03-02 um 10.11.12.png", "screenshot_20240302.jpg", "Screen Shot 2019.png", "IMG_1.PNG Bildschirmaufnahme"] {
            assert!(name_says_screenshot(n), "{n}");
        }
        assert!(!name_says_screenshot("IMG_1234.PNG") && !name_says_screenshot("Urlaub am Strand.jpg"));
    }

    #[test]
    fn sizes_either_way_round() {
        assert!(is_display_size(1170, 2532) && is_display_size(2532, 1170) && is_display_size(2880, 1800));
        assert!(!is_display_size(4032, 3024) && !is_display_size(1171, 2532));
    }

    #[test]
    fn score_combines_the_signals() {
        let png = Kind::Png;
        // A named PNG without camera is one, even before its thumbnail exists.
        assert!(score("Screenshot 1.png", png, None, None, None, None) >= THRESHOLD);
        // iPhone screenshot: PNG, no camera, phone size, flat.
        assert!(score("IMG_1234.PNG", png, Some(1170), Some(2532), None, Some(90)) >= THRESHOLD);
        // The same without a flat picture is a scan or an export, not one.
        assert!(score("IMG_1234.PNG", png, Some(1170), Some(2532), None, Some(0)) < THRESHOLD);
        // A forwarded JPEG that lost everything but its pixels.
        assert!(score("IMG-20240302-WA0001.jpg", Kind::Jpeg, Some(1080), Some(2400), None, Some(90)) >= THRESHOLD);
        // A camera made it: never, whatever it looks like.
        assert!(score("Screenshot.jpg", Kind::Jpeg, Some(1170), Some(2532), Some("iPhone 15"), Some(100)) < THRESHOLD);
        // A flat PNG of the wrong size and name (a drawing).
        assert!(score("zeichnung.png", png, Some(900), Some(700), None, Some(40)) < THRESHOLD);
        // Never HEIC, RAW, video.
        for k in [Kind::Heic, Kind::Raw, Kind::Video] {
            assert_eq!(score("Screenshot.mov", k, Some(1170), Some(2532), None, Some(100)), 0);
        }
    }
}
