//! Perceptual hash for near-duplicate detection.
//!
//! The classic DCT hash: shrink to 32×32 greyscale, take the 2-D DCT, keep
//! the 8×8 lowest frequencies and set one bit per coefficient that lies above
//! their median. Resized, re-encoded or slightly edited copies stay within a
//! Hamming distance of about 8; different photos land near 32.
//!
//! It is computed from the thumbnail, so every image is decoded only once.

use std::sync::OnceLock;

use image::DynamicImage;
use image::imageops::FilterType;

const N: usize = 32;
const K: usize = 8;

/// Hash of an image (any size; thumbnails are plenty).
pub fn of(img: &DynamicImage) -> u64 {
    let small = img.resize_exact(N as u32, N as u32, FilterType::Triangle).to_luma32f();
    let px: Vec<f32> = small.into_raw();
    let cos = cos_table();

    // Separable DCT-II, only the K lowest frequencies in each direction.
    let mut rows = [[0f32; K]; N]; // rows[y][u]
    for y in 0..N {
        for u in 0..K {
            rows[y][u] = (0..N).map(|x| px[y * N + x] * cos[u][x]).sum();
        }
    }
    let mut coeffs = [0f32; K * K];
    for v in 0..K {
        for u in 0..K {
            coeffs[v * K + u] = (0..N).map(|y| rows[y][u] * cos[v][y]).sum();
        }
    }

    // The DC term only reflects overall brightness; leave it out of the median.
    let mut sorted = coeffs[1..].to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0;
    coeffs.iter().enumerate().fold(0u64, |h, (i, &c)| if c > median { h | 1 << i } else { h })
}

pub fn to_hex(h: u64) -> String {
    format!("{h:016x}")
}

pub fn from_hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s, 16).ok()
}

pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Index pairs `(i, j)`, `i < j`, of hashes at most `max` bits apart. Every
/// pair is compared, on all cores: about a second for 100,000 distinct
/// hashes on an older laptop.
pub fn near_pairs(hashes: &[u64], max: u32) -> Vec<(usize, usize)> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 16);
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    // Interleaved rows, so every thread gets long and short ones.
                    let mut out = Vec::new();
                    for i in (t..hashes.len()).step_by(threads) {
                        near_in_row(hashes, i, max, &mut out);
                    }
                    out
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().expect("phash worker")).collect()
    })
}

fn near_in_row(hashes: &[u64], i: usize, max: u32, out: &mut Vec<(usize, usize)>) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("popcnt") {
        // SAFETY: the CPU has POPCNT.
        unsafe { near_in_row_popcnt(hashes, i, max, out) };
        return;
    }
    near_in_row_generic(hashes, i, max, out);
}

/// The baseline x86_64 target has no POPCNT instruction; every Intel Mac does.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
unsafe fn near_in_row_popcnt(hashes: &[u64], i: usize, max: u32, out: &mut Vec<(usize, usize)>) {
    near_in_row_generic(hashes, i, max, out);
}

#[inline(always)]
fn near_in_row_generic(hashes: &[u64], i: usize, max: u32, out: &mut Vec<(usize, usize)>) {
    let h = hashes[i];
    for (k, &other) in hashes[i + 1..].iter().enumerate() {
        if (h ^ other).count_ones() <= max {
            out.push((i, i + 1 + k));
        }
    }
}

fn cos_table() -> &'static [[f32; N]; K] {
    static TABLE: OnceLock<[[f32; N]; K]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [[0f32; N]; K];
        for (u, row) in t.iter_mut().enumerate() {
            for (x, c) in row.iter_mut().enumerate() {
                *c = ((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / (2 * N) as f32).cos();
            }
        }
        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// Blocks of pseudo-random brightness with a soft gradient: enough
    /// structure for the low frequencies to mean something.
    fn scene(seed: u32, w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let (bx, by) = (x * 6 / w, y * 5 / h);
            let n = (bx * 7919 + by * 104_729 + seed * 1_299_709).wrapping_mul(2_654_435_761) >> 24;
            let v = (n as f32 * 0.7 + (x as f32 / w as f32) * 70.0) as u8;
            Rgb([v, v / 2 + 40, 255 - v])
        }))
    }

    #[test]
    fn resized_copies_are_close_and_different_images_are_not() {
        let a = of(&scene(1, 640, 480));
        let a_small = of(&scene(1, 640, 480).thumbnail(160, 160));
        let mut darker = scene(1, 640, 480).to_rgb8();
        darker.pixels_mut().for_each(|p| p.0.iter_mut().for_each(|c| *c = (*c as f32 * 0.8) as u8));
        let a_dark = of(&DynamicImage::ImageRgb8(darker));
        let rings = of(&DynamicImage::ImageRgb8(RgbImage::from_fn(640, 480, |x, y| {
            let d = ((x as f32 - 400.0).powi(2) + (y as f32 - 200.0).powi(2)).sqrt();
            Rgb([((d / 9.0).sin() * 120.0 + 128.0) as u8, 90, 160])
        })));

        assert!(distance(a, a_small) <= 8, "resized: {}", distance(a, a_small));
        assert!(distance(a, a_dark) <= 8, "darker: {}", distance(a, a_dark));
        assert!(distance(a, rings) > 16, "different: {}", distance(a, rings));
        assert_eq!(from_hex(&to_hex(a)), Some(a));
    }

    #[test]
    fn near_pairs_finds_exactly_the_close_ones() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut hashes: Vec<u64> = (0..3000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            })
            .collect();
        hashes.push(hashes[10] ^ 0b1011); // 3 bits from #10
        hashes.push(hashes[20] ^ 0x1ff); // 9 bits from #20
        let mut pairs = near_pairs(&hashes, 8);
        pairs.sort();
        let mut expected: Vec<(usize, usize)> = Vec::new();
        for i in 0..hashes.len() {
            for j in i + 1..hashes.len() {
                if distance(hashes[i], hashes[j]) <= 8 {
                    expected.push((i, j));
                }
            }
        }
        assert_eq!(pairs, expected);
        assert!(pairs.contains(&(10, 3000)) && !pairs.contains(&(20, 3001)));
    }
}
