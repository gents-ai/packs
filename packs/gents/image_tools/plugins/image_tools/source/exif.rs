//! A bounded reader for the first directory of a TIFF structure, which is both
//! an Exif block (inside JPEG, PNG and WebP) and a whole TIFF file. It reads
//! only the entries it needs through `Read + Seek`, so a large TIFF is never
//! loaded, and every offset is checked: a corrupt block yields `None` fields,
//! never a panic.
use std::io::{Read, Seek, SeekFrom};

const TAG_ORIENTATION: u16 = 0x0112;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_GPS_IFD: u16 = 0x8825;
const MAX_ENTRIES: u16 = 4096;

/// A GPS position in decimal degrees, rounded to six decimals.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Position {
    /// Degrees north of the equator, negative to the south.
    pub lat: f64,
    /// Degrees east of Greenwich, negative to the west.
    pub lon: f64,
}

/// What the first directory of a TIFF structure says.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Facts {
    /// The Exif orientation, 1 to 8.
    pub orientation: Option<u8>,
    /// Whether the structure points at an Exif sub-directory.
    pub has_exif_ifd: bool,
    /// Whether the structure points at a GPS directory.
    pub has_gps: bool,
    /// The GPS position, when both coordinates could be read.
    pub position: Option<Position>,
}

#[derive(Clone, Copy)]
struct Order(bool);

impl Order {
    fn u16(self, b: [u8; 2]) -> u16 {
        if self.0 {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    }

    fn u32(self, b: [u8; 4]) -> u32 {
        if self.0 {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }
}

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

fn read<const N: usize, R: Read>(r: &mut R) -> Option<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b).ok()?;
    Some(b)
}

/// Reads the facts from the TIFF structure that starts `base` bytes into `r`.
/// `None` when the bytes are not a TIFF structure.
pub fn read_facts<R: Read + Seek>(r: &mut R, base: u64) -> Option<Facts> {
    r.seek(SeekFrom::Start(base)).ok()?;
    let order = match read::<4, _>(r)? {
        [b'I', b'I', 42, 0] => Order(true),
        [b'M', b'M', 0, 42] => Order(false),
        _ => return None,
    };
    let ifd0 = u64::from(order.u32(read(r)?));
    let entries = directory(r, base, ifd0, order)?;
    let mut facts = Facts::default();
    let mut gps_at = None;
    for e in &entries {
        match e.tag {
            TAG_ORIENTATION if e.kind == 3 && e.count == 1 => {
                let v = order.u16([e.value[0], e.value[1]]);
                facts.orientation = u8::try_from(v).ok().filter(|v| (1..=8).contains(v));
            }
            TAG_EXIF_IFD => facts.has_exif_ifd = true,
            TAG_GPS_IFD => {
                facts.has_gps = true;
                gps_at = Some(u64::from(order.u32(e.value)));
            }
            _ => {}
        }
    }
    if let Some(at) = gps_at {
        facts.position = gps_position(r, base, at, order);
    }
    Some(facts)
}

fn directory<R: Read + Seek>(r: &mut R, base: u64, at: u64, order: Order) -> Option<Vec<Entry>> {
    r.seek(SeekFrom::Start(base.checked_add(at)?)).ok()?;
    let n = order.u16(read(r)?).min(MAX_ENTRIES);
    let mut out = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        let tag = order.u16(read(r)?);
        let kind = order.u16(read(r)?);
        let count = order.u32(read(r)?);
        out.push(Entry {
            tag,
            kind,
            count,
            value: read(r)?,
        });
    }
    Some(out)
}

fn gps_position<R: Read + Seek>(r: &mut R, base: u64, at: u64, order: Order) -> Option<Position> {
    let entries = directory(r, base, at, order)?;
    let find = |tag: u16| entries.iter().find(|e| e.tag == tag);
    let sign = |tag: u16, neg: u8| -> Option<f64> {
        let e = find(tag)?;
        (e.kind == 2 && e.count >= 1).then_some(())?;
        Some(if e.value[0].eq_ignore_ascii_case(&neg) {
            -1.0
        } else {
            1.0
        })
    };
    let degrees = |r: &mut R, tag: u16| -> Option<f64> {
        let e = find(tag)?;
        if e.kind != 5 || e.count != 3 {
            return None;
        }
        r.seek(SeekFrom::Start(
            base.checked_add(u64::from(order.u32(e.value)))?,
        ))
        .ok()?;
        let mut v = [0f64; 3];
        for part in &mut v {
            let num = f64::from(order.u32(read(r)?));
            let den = f64::from(order.u32(read(r)?));
            if den == 0.0 {
                return None;
            }
            *part = num / den;
        }
        Some(v[0] + v[1] / 60.0 + v[2] / 3600.0)
    };
    let round6 = |x: f64| (x * 1e6).round() / 1e6;
    let lat = degrees(r, 2)? * sign(1, b'S')?;
    let lon = degrees(r, 4)? * sign(3, b'W')?;
    if !(lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return None;
    }
    Some(Position {
        lat: round6(lat),
        lon: round6(lon),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::exif_block;
    use std::io::Cursor;

    fn facts(bytes: &[u8]) -> Option<Facts> {
        read_facts(&mut Cursor::new(bytes.to_vec()), 0)
    }

    /// A big-endian block with one entry per `(tag, kind, count, value)`; values sit in the entry itself.
    fn big_endian(entries: &[(u16, u16, u32, [u8; 4])]) -> Vec<u8> {
        let mut out = vec![b'M', b'M', 0, 42, 0, 0, 0, 8];
        out.extend((entries.len() as u16).to_be_bytes());
        for (tag, kind, count, value) in entries {
            out.extend(tag.to_be_bytes());
            out.extend(kind.to_be_bytes());
            out.extend(count.to_be_bytes());
            out.extend(value);
        }
        out.extend([0u8; 4]);
        out
    }

    #[test]
    fn a_little_endian_block_gives_the_orientation() {
        for n in 1..=8u8 {
            assert_eq!(
                facts(&exif_block(Some(n), None)).unwrap().orientation,
                Some(n)
            );
        }
        let f = facts(&exif_block(None, None)).unwrap();
        assert_eq!(
            (f.orientation, f.has_gps, f.has_exif_ifd, f.position),
            (None, false, false, None)
        );
    }

    #[test]
    fn a_big_endian_block_is_read_the_same_way() {
        let f = facts(&big_endian(&[(0x0112, 3, 1, [0, 8, 0, 0])])).unwrap();
        assert_eq!(f.orientation, Some(8));
        let f = facts(&big_endian(&[(0x8769, 4, 1, [0, 0, 0, 99])])).unwrap();
        assert!(f.has_exif_ifd && f.orientation.is_none());
    }

    #[test]
    fn orientation_values_outside_one_to_eight_or_of_the_wrong_type_are_ignored() {
        for v in [0u8, 9, 255] {
            assert_eq!(
                facts(&exif_block(Some(v), None)).unwrap().orientation,
                None,
                "{v}"
            );
        }
        // Type LONG (4) where SHORT is required.
        assert_eq!(
            facts(&big_endian(&[(0x0112, 4, 1, [0, 0, 0, 6])]))
                .unwrap()
                .orientation,
            None
        );
        // Two values instead of one.
        assert_eq!(
            facts(&big_endian(&[(0x0112, 3, 2, [0, 6, 0, 0])]))
                .unwrap()
                .orientation,
            None
        );
    }

    #[test]
    fn gps_positions_have_signs_from_the_hemisphere_and_six_decimals() {
        for (lat, lon) in [
            (48.8566, 2.3522),
            (-33.8688, 151.2093),
            (40.7128, -74.006),
            (-0.5, -0.25),
            (90.0, 180.0),
        ] {
            let f = facts(&exif_block(None, Some((lat, lon)))).unwrap();
            assert!(f.has_gps);
            let p = f.position.unwrap();
            assert!(
                (p.lat - lat).abs() < 1e-6 && (p.lon - lon).abs() < 1e-6,
                "{lat},{lon} -> {p:?}"
            );
            assert_eq!(
                p.lat,
                (p.lat * 1e6).round() / 1e6,
                "rounded to six decimals"
            );
        }
    }

    #[test]
    fn a_gps_pointer_without_readable_coordinates_is_present_but_has_no_position() {
        // The pointer says there is a GPS directory, at an offset outside the block.
        let f = facts(&big_endian(&[(0x8825, 4, 1, [0, 0, 255, 255])])).unwrap();
        assert!(f.has_gps);
        assert_eq!(f.position, None);
        // A zero denominator is not a number.
        let mut b = exif_block(None, Some((10.5, 20.5)));
        let at = b.len() - 4 * 3 * 2 * 2 + 4;
        b[at..at + 4].copy_from_slice(&[0, 0, 0, 0]);
        let f = facts(&b).unwrap();
        assert!(f.has_gps);
        assert_eq!(f.position, None);
    }

    #[test]
    fn every_truncation_of_a_valid_block_is_handled_without_a_panic() {
        let block = exif_block(Some(6), Some((48.8566, 2.3522)));
        let mut full = 0;
        for cut in 0..=block.len() {
            if let Some(f) = facts(&block[..cut]) {
                // Whatever survives is consistent: an orientation is never invented.
                assert!(f.orientation.is_none() || f.orientation == Some(6));
                full += usize::from(f.position.is_some());
            }
        }
        assert_eq!(full, 1, "only the complete block yields the position");
    }

    #[test]
    fn something_that_is_not_a_tiff_structure_is_none() {
        for bytes in [
            &b""[..],
            b"II",
            b"II*",
            b"XX*\0\x08\0\0\0",
            b"II\x2b\0\x08\0\0\0",
            b"not exif at all",
        ] {
            assert!(facts(bytes).is_none(), "{bytes:?}");
        }
    }

    #[test]
    fn a_wild_entry_count_or_directory_offset_does_not_run_away() {
        // 0xFFFF entries claimed in a block that holds none of them.
        let mut b = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0xFF, 0xFF];
        assert!(facts(&b).is_none());
        b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(facts(&b).is_none(), "an offset past the end");
    }

    #[test]
    fn a_directory_claiming_65535_entries_is_read_up_to_4096_and_no_further() {
        // `n` filler entries with the orientation entry (6) at index `at`; the count field says 65535.
        let block = |n: usize, at: usize| {
            let mut e = vec![(0x0100u16, 4u16, 1u32, [0u8; 4]); n];
            e[at] = (0x0112, 3, 1, [0, 6, 0, 0]);
            let mut b = big_endian(&e);
            b[8..10].copy_from_slice(&u16::MAX.to_be_bytes());
            b
        };
        let last_read = facts(&block(4097, 4095)).expect("the first 4096 entries are read");
        assert_eq!(
            last_read.orientation,
            Some(6),
            "entry 4096 is inside the cap"
        );
        let first_cut = facts(&block(4097, 4096)).expect("the cap stops the read, not fails it");
        assert_eq!(first_cut.orientation, None, "entry 4097 is beyond the cap");
    }

    #[test]
    fn the_block_may_start_inside_a_larger_buffer() {
        let mut file = vec![0xAA; 37];
        file.extend(exif_block(Some(3), None));
        let f = read_facts(&mut Cursor::new(file), 37).unwrap();
        assert_eq!(f.orientation, Some(3));
    }
}
