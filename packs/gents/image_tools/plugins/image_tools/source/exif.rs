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
        if self.0 { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }

    fn u32(self, b: [u8; 4]) -> u32 {
        if self.0 { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
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
        Some(if e.value[0].eq_ignore_ascii_case(&neg) { -1.0 } else { 1.0 })
    };
    let degrees = |r: &mut R, tag: u16| -> Option<f64> {
        let e = find(tag)?;
        if e.kind != 5 || e.count != 3 {
            return None;
        }
        r.seek(SeekFrom::Start(base.checked_add(u64::from(order.u32(e.value)))?))
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
