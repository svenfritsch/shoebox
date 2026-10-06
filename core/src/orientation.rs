//! The EXIF Orientation tag of a JPEG: where its two bytes are, and what a
//! quarter turn makes of it.
//!
//! Turning a photo means changing these two bytes in place. The picture data
//! is not touched, so nothing is lost and the file keeps its size.

use anyhow::{Result, bail};

const ORIENTATION: u16 = 0x0112;
const TYPE_SHORT: u16 = 3;

/// The Orientation value inside a JPEG file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// Offset of the 2-byte value in the file.
    pub offset: usize,
    pub big_endian: bool,
    /// 1..=8 as the EXIF standard defines them (1 is upright).
    pub value: u8,
}

impl Slot {
    /// The two bytes to write for `value`, in the byte order of the file.
    pub fn bytes(&self, value: u8) -> [u8; 2] {
        if self.big_endian { (value as u16).to_be_bytes() } else { (value as u16).to_le_bytes() }
    }
}

/// Find the Orientation tag in the EXIF block of a whole JPEG file.
/// Fails for a file without one: that needs bytes inserted, which this does
/// not do.
pub fn find(file: &[u8]) -> Result<Slot> {
    if file.get(..2) != Some(&[0xFF, 0xD8]) {
        bail!("not a JPEG file");
    }
    let mut pos = 2;
    loop {
        // Markers: 0xFF, then the marker byte (more 0xFF may pad).
        while file.get(pos) == Some(&0xFF) && file.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let (Some(&0xFF), Some(&marker)) = (file.get(pos), file.get(pos + 1)) else { break };
        pos += 2;
        match marker {
            // Standalone markers without a length.
            0x01 | 0xD0..=0xD7 => continue,
            // The picture data starts, or the file ends: no EXIF came first.
            0xDA | 0xD9 => break,
            _ => {}
        }
        let Some(len) = file.get(pos..pos + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else { break };
        if len < 2 || pos + len > file.len() {
            break;
        }
        let data = &file[pos + 2..pos + len];
        if marker == 0xE1 && data.starts_with(b"Exif\0\0") {
            return in_tiff(file, pos + 2 + 6, &data[6..]);
        }
        pos += len;
    }
    bail!("the photo has no EXIF orientation to change")
}

/// `tiff` is the TIFF structure inside the Exif segment, at `base` in the file.
fn in_tiff(file: &[u8], base: usize, tiff: &[u8]) -> Result<Slot> {
    let big_endian = match tiff.get(..2) {
        Some(b"MM") => true,
        Some(b"II") => false,
        _ => bail!("the EXIF block is damaged"),
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = tiff.get(at..at + 2)?;
        Some(if big_endian { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = tiff.get(at..at + 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    if u16_at(2) != Some(42) {
        bail!("the EXIF block is damaged");
    }
    let Some(ifd0) = u32_at(4).map(|o| o as usize) else { bail!("the EXIF block is damaged") };
    let Some(count) = u16_at(ifd0) else { bail!("the EXIF block is damaged") };
    for k in 0..count as usize {
        let entry = ifd0 + 2 + k * 12;
        if u16_at(entry) != Some(ORIENTATION) {
            continue;
        }
        // One SHORT sits inside the entry itself.
        if u16_at(entry + 2) != Some(TYPE_SHORT) || u32_at(entry + 4) != Some(1) {
            bail!("the EXIF orientation has an unexpected form");
        }
        let Some(value) = u16_at(entry + 8) else { bail!("the EXIF block is damaged") };
        if !(1..=8).contains(&value) {
            bail!("the EXIF orientation is {value}, not 1 to 8");
        }
        let offset = base + entry + 8;
        debug_assert!(offset + 2 <= file.len());
        return Ok(Slot { offset, big_endian, value: value as u8 });
    }
    bail!("the photo has no EXIF orientation to change")
}

/// Each orientation as the 2×2 matrix that takes the stored picture to the
/// displayed one (x right, y down, around the centre).
const MATRIX: [[i8; 4]; 8] = [
    [1, 0, 0, 1],   // 1 upright
    [-1, 0, 0, 1],  // 2 mirrored left-right
    [-1, 0, 0, -1], // 3 turned 180°
    [1, 0, 0, -1],  // 4 mirrored top-bottom
    [0, 1, 1, 0],   // 5 mirrored along the main diagonal
    [0, -1, 1, 0],  // 6 turned 90° clockwise
    [0, -1, -1, 0], // 7 mirrored along the other diagonal
    [0, 1, -1, 0],  // 8 turned 90° counter-clockwise
];

/// The orientation that shows `value` turned further by `quarters` quarter
/// turns clockwise (negative: counter-clockwise).
pub fn turned(value: u8, quarters: i32) -> u8 {
    let mut m = MATRIX[(value as usize - 1) % 8];
    for _ in 0..quarters.rem_euclid(4) {
        // A clockwise quarter turn is [[0, -1], [1, 0]], applied last.
        m = [-m[2], -m[3], m[0], m[1]];
    }
    MATRIX.iter().position(|c| *c == m).expect("the eight matrices are closed under turning") as u8 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_cycle_through_the_orientations() {
        assert_eq!(turned(1, 1), 6);
        assert_eq!(turned(6, 1), 3);
        assert_eq!(turned(3, 1), 8);
        assert_eq!(turned(8, 1), 1);
        assert_eq!(turned(1, -1), 8);
        assert_eq!(turned(1, 2), 3);
        // Mirrored ones turn the other way round.
        assert_eq!(turned(2, 1), 7);
        assert_eq!(turned(7, 1), 4);
        assert_eq!(turned(4, 1), 5);
        assert_eq!(turned(5, 1), 2);
        for v in 1..=8 {
            assert_eq!(turned(v, 4), v);
            assert_eq!(turned(turned(v, 1), -1), v);
            assert_eq!(turned(turned(v, 1), 1), turned(v, 2));
        }
    }

    /// A JPEG-shaped byte string with an Exif segment holding `value`.
    pub fn with_exif(big_endian: bool, value: u16) -> Vec<u8> {
        let w16 = |v: u16| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
        let w32 = |v: u32| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut tiff = Vec::new();
        tiff.extend(if big_endian { b"MM" } else { b"II" });
        tiff.extend(w16(42));
        tiff.extend(w32(8));
        tiff.extend(w16(2));
        // A first entry that is not the orientation (Make, an ASCII string).
        tiff.extend(w16(0x010F));
        tiff.extend(w16(2));
        tiff.extend(w32(1));
        tiff.extend([b'X', 0, 0, 0]);
        tiff.extend(w16(ORIENTATION));
        tiff.extend(w16(TYPE_SHORT));
        tiff.extend(w32(1));
        tiff.extend(w16(value));
        tiff.extend([0, 0]);
        tiff.extend(w32(0));
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, b'J', b'F'];
        out.extend([0xFF, 0xE1]);
        out.extend(((tiff.len() + 8) as u16).to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(tiff);
        out.extend([0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn finds_the_value_in_both_byte_orders() {
        for big in [true, false] {
            let file = with_exif(big, 6);
            let slot = find(&file).unwrap();
            assert_eq!((slot.value, slot.big_endian), (6, big));
            let bytes = slot.bytes(8);
            assert_eq!(slot.bytes(1) == [0, 1], big);
            let mut turned_file = file.clone();
            turned_file[slot.offset..slot.offset + 2].copy_from_slice(&bytes);
            assert_eq!(find(&turned_file).unwrap().value, 8);
        }
    }

    #[test]
    fn refuses_what_it_cannot_change_in_place() {
        assert!(find(b"not a jpeg").is_err());
        // No EXIF at all.
        assert!(find(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9]).is_err());
        // An impossible value.
        assert!(find(&with_exif(true, 9)).is_err());
        // Cut off in the middle of the EXIF block.
        let file = with_exif(false, 6);
        assert!(find(&file[..30]).is_err());
    }
}
