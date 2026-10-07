//! Whole-stream check for a JPEG. The lenient decoder fills the rows it cannot
//! read with flat grey and reports success, so a file cut inside its scan data
//! would pass as a picture. Walking the marker structure to the closing EOI
//! marker finds the cut without decoding: entropy data never holds a bare
//! marker (a 0xFF there is stuffed with 0x00), so an EOI inside an Exif
//! thumbnail cannot be mistaken for the end of the main image.
use std::io::{BufRead, Read};

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;

/// Reads one byte, `None` at the end of the data or on a read error.
fn byte<R: Read>(r: &mut R) -> Option<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).ok()?;
    Some(b[0])
}

/// Skips `n` bytes, `None` when the data ends first.
fn skip<R: Read>(r: &mut R, n: u64) -> Option<()> {
    (std::io::copy(&mut r.by_ref().take(n), &mut std::io::sink()).ok()? == n).then_some(())
}

/// Skips entropy-coded data and returns the marker that ends it, `None` at the end of the data.
fn after_scan<R: BufRead>(r: &mut R) -> Option<u8> {
    loop {
        let buf = r.fill_buf().ok()?;
        if buf.is_empty() {
            return None;
        }
        match buf.iter().position(|&b| b == 0xFF) {
            Some(i) => r.consume(i + 1),
            None => {
                let n = buf.len();
                r.consume(n);
                continue;
            }
        }
        // Fill bytes (more 0xFF), a stuffed 0x00 and restart markers stay inside the scan.
        loop {
            match byte(r)? {
                0xFF => {}
                0x00 | 0xD0..=0xD7 => break,
                m => return Some(m),
            }
        }
    }
}

/// Whether `r` holds a JPEG stream that reaches its EOI marker with every
/// segment whole. Trailing bytes after EOI are ignored.
pub fn is_complete<R: BufRead>(r: &mut R) -> bool {
    let mut marker = match (byte(r), byte(r)) {
        (Some(0xFF), Some(SOI)) => SOI,
        _ => return false,
    };
    loop {
        match marker {
            EOI => return true,
            SOI | 0x01 | 0xD0..=0xD7 => {}
            m => {
                let (Some(hi), Some(lo)) = (byte(r), byte(r)) else {
                    return false;
                };
                let len = u64::from(u16::from_be_bytes([hi, lo]));
                if len < 2 || skip(r, len - 2).is_none() {
                    return false;
                }
                if m == SOS {
                    let Some(next) = after_scan(r) else {
                        return false;
                    };
                    marker = next;
                    continue;
                }
            }
        }
        // The next marker: 0xFF, any fill 0xFF, then its code.
        if byte(r) != Some(0xFF) {
            return false;
        }
        marker = loop {
            match byte(r) {
                Some(0xFF) => {}
                Some(m) => break m,
                None => return false,
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::is_complete;
    use crate::fixtures as fx;
    use std::io::Cursor;

    fn complete(b: &[u8]) -> bool {
        is_complete(&mut Cursor::new(b))
    }

    #[test]
    fn a_whole_jpeg_is_complete_and_every_cut_before_its_end_is_not() {
        let jpg = fx::jpeg(&fx::scene(64, 48), 85);
        assert!(complete(&jpg));
        for cut in (0..jpg.len())
            .step_by(7)
            .chain([jpg.len() - 2, jpg.len() - 1])
        {
            assert!(!complete(&jpg[..cut]), "a cut at {cut} of {}", jpg.len());
        }
    }

    #[test]
    fn bytes_after_the_end_marker_are_ignored() {
        let mut jpg = fx::jpeg(&fx::scene(32, 32), 85);
        jpg.extend(b"trailing bytes \xFF\xD8");
        assert!(complete(&jpg));
    }

    #[test]
    fn an_end_marker_inside_a_segment_is_not_the_end() {
        // An APP1 holding FF D9 in its payload, then a cut: the payload bytes are skipped by length.
        let mut s = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x06, 0xFF, 0xD9, 0x01, 0x02];
        assert!(!complete(&s), "the stream ends after the segment");
        s.extend([0xFF, 0xD9]);
        assert!(complete(&s));
    }

    #[test]
    fn stuffed_bytes_restart_markers_and_fill_bytes_stay_inside_the_scan() {
        let s = [
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x03, 0x00, // SOS header of length 3
            0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xFF, 0x00, 0x78, // entropy data
            0xFF, 0xFF, 0xD9, // fill byte, then EOI
        ];
        assert!(complete(&s));
        assert!(!complete(&s[..s.len() - 1]));
        assert!(!complete(&s[..9]));
    }

    #[test]
    fn a_progressive_style_second_scan_is_followed_to_its_end() {
        let s = [
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xDA, 0x00, 0x02, 0x33, 0xFF,
            0xD9,
        ];
        assert!(complete(&s));
        assert!(!complete(&s[..13]));
    }

    #[test]
    fn garbage_and_bad_lengths_are_not_complete() {
        assert!(!complete(b""));
        assert!(!complete(b"\xFF"));
        assert!(!complete(b"\xFF\xD8"));
        assert!(!complete(b"GIF89a not a jpeg"));
        assert!(!complete(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01, 0xFF, 0xD9]));
        assert!(
            !complete(&[0xFF, 0xD8, 0x00, 0xFF, 0xD9]),
            "no marker after SOI"
        );
        assert!(complete(&[0xFF, 0xD8, 0xFF, 0xD9]));
        assert!(
            !complete(&[0xFF, 0xE0, 0xFF, 0xD9]),
            "a stream must start with the SOI marker"
        );
        assert!(!complete(&[0xFF, 0xD9]), "an EOI alone is not a picture");
    }
}
