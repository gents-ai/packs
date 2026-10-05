//! Perceptual hashes for near-duplicate detection. Both are 64 bits and both
//! look at the picture shrunk to a handful of gray cells, so a resize, a
//! re-encode or a small colour shift leaves them nearly unchanged.
//!
//! * `dhash`: compares each cell with its right neighbour on a 9x8 grid.
//! * `phash`: the 8x8 lowest frequencies of a 32x32 discrete cosine transform,
//!   each bit set when its coefficient is above the median.
//!
//! The shrink is an exact area average done in integers and the transform uses
//! a fixed cosine table, so the same pixels hash the same on every platform.
use crate::model::Img;

/// cos(k * pi / 64) for k = 0..=32; the rest follow by symmetry.
const COS: [f64; 33] = [
    1.0, 0.9987954562051724, 0.9951847266721969, 0.989176509964781, 0.9807852804032304,
    0.970031253194544, 0.9569403357322088, 0.9415440651830208, 0.9238795325112867,
    0.9039892931234433, 0.881921264348355, 0.8577286100002721, 0.8314696123025452,
    0.8032075314806449, 0.773010453362737, 0.7409511253549591, 0.7071067811865476,
    0.6715589548470183, 0.6343932841636455, 0.5956993044924335, 0.5555702330196023,
    0.5141027441932217, 0.4713967368259978, 0.4275550934302822, 0.38268343236508984,
    0.33688985339222005, 0.29028467725446233, 0.24298017990326398, 0.19509032201612833,
    0.14673047445536175, 0.09801714032956077, 0.049067674327418126, 0.0,
];

/// cos(m * pi / 64) for any non-negative `m`.
fn cos64(m: usize) -> f64 {
    let m = m % 128;
    let m = if m > 64 { 128 - m } else { m };
    if m > 32 { -COS[64 - m] } else { COS[m] }
}

/// Luma of one RGBA pixel over a white background, 0 to 255000 (three extra digits).
pub fn luma_milli(p: &[u8]) -> u64 {
    let a = u64::from(p[3]);
    let on_white = |c: u8| (u64::from(c) * a + 255 * (255 - a) + 127) / 255;
    299 * on_white(p[0]) + 587 * on_white(p[1]) + 114 * on_white(p[2])
}

/// Overlap, in shared units, of source cell `i` and target cell `j` when `sn`
/// source cells are laid over `tn` target cells.
fn overlap(i: u64, j: u64, sn: u64, tn: u64) -> u64 {
    let lo = (i * tn).max(j * sn);
    let hi = ((i + 1) * tn).min((j + 1) * sn);
    hi.saturating_sub(lo)
}

/// The image shrunk to `tw` x `th` gray cells by exact area averaging, row-major.
pub fn gray_cells(img: &Img, tw: usize, th: usize) -> Vec<f64> {
    let (sw, sh) = (img.w as usize, img.h as usize);
    // Horizontal pass: for every source row, the weighted sums of each target column.
    let mut cols = vec![0u64; sh * tw];
    for (y, row) in img.px.chunks_exact(sw * 4).enumerate() {
        for (x, p) in row.chunks_exact(4).enumerate() {
            let l = luma_milli(p);
            let first = (x * tw) / sw;
            for j in first..tw.min(first + 2) {
                cols[y * tw + j] += l * overlap(x as u64, j as u64, sw as u64, tw as u64);
            }
        }
    }
    let mut out = vec![0f64; tw * th];
    let area = (sw as u128) * (sh as u128);
    for j in 0..th {
        for i in 0..tw {
            let mut sum = 0u128;
            for y in 0..sh {
                let o = overlap(y as u64, j as u64, sh as u64, th as u64);
                if o > 0 {
                    sum += u128::from(cols[y * tw + i]) * u128::from(o);
                }
            }
            out[j * tw + i] = (sum as f64) / (area as f64) / 1000.0;
        }
    }
    out
}

/// The difference hash of the image.
pub fn dhash(img: &Img) -> u64 {
    let g = gray_cells(img, 9, 8);
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            bits = bits << 1 | u64::from(g[y * 9 + x] > g[y * 9 + x + 1]);
        }
    }
    bits
}

/// The DCT hash of the image.
pub fn phash(img: &Img) -> u64 {
    const N: usize = 32;
    let g = gray_cells(img, N, N);
    // Rows then columns, keeping only the 8 lowest frequencies on each axis.
    let mut rows = [[0f64; 8]; N];
    for (y, out) in rows.iter_mut().enumerate() {
        for (u, o) in out.iter_mut().enumerate() {
            let mut s = 0.0;
            for x in 0..N {
                s += g[y * N + x] * cos64((2 * x + 1) * u);
            }
            *o = s;
        }
    }
    let mut coef = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for (y, row) in rows.iter().enumerate() {
                s += row[u] * cos64((2 * y + 1) * v);
            }
            coef[v * 8 + u] = s;
        }
    }
    let mut ac: Vec<f64> = coef[1..].to_vec();
    ac.sort_by(f64::total_cmp);
    let median = ac[ac.len() / 2];
    coef.iter().fold(0u64, |bits, c| bits << 1 | u64::from(*c > median))
}

/// The number of differing bits between two hashes.
pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A hash as 16 lower-case hex digits.
pub fn hex16(h: u64) -> String {
    format!("{h:016x}")
}

/// Parses 16 hex digits back into a hash.
pub fn parse16(s: &str) -> Option<u64> {
    (s.len() == 16).then(|| u64::from_str_radix(s, 16).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures as fx;
    use crate::resize::{Filter, resample};

    fn img(i: &image::RgbaImage) -> Img {
        Img::from_raw(i.width(), i.height(), i.as_raw().clone()).unwrap()
    }

    fn ramp(w: u32, h: u32, rising: bool) -> Img {
        let mut m = Img::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let v = if rising { x * 255 / (w - 1) } else { 255 - x * 255 / (w - 1) } as u8;
                let i = m.at(x, y);
                m.px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        m
    }

    #[test]
    fn the_cosine_table_follows_the_symmetries() {
        assert_eq!(cos64(0), 1.0);
        assert_eq!(cos64(32), 0.0);
        assert_eq!(cos64(64), -1.0);
        assert_eq!(cos64(128), 1.0);
        assert!((cos64(16) - 0.5f64.sqrt()).abs() < 1e-15);
        assert!((cos64(48) + 0.5f64.sqrt()).abs() < 1e-15);
        assert!((cos64(96) - cos64(32)).abs() < 1e-15);
        for m in 0..256 {
            let exact = (m as f64 * std::f64::consts::PI / 64.0).cos();
            assert!((cos64(m) - exact).abs() < 1e-12, "m = {m}");
        }
    }

    #[test]
    fn gray_cells_are_exact_area_means() {
        // 4x2 image, left half 0 and right half 200 over one row, a second row of 100: shrink to 2x1.
        let mut m = Img::filled(4, 2, [0, 0, 0, 255]).unwrap();
        for x in 2..4 {
            let i = m.at(x, 0);
            m.px[i..i + 3].copy_from_slice(&[200, 200, 200]);
        }
        for x in 0..4 {
            let i = m.at(x, 1);
            m.px[i..i + 3].copy_from_slice(&[100, 100, 100]);
        }
        let g = gray_cells(&m, 2, 1);
        assert_eq!(g, vec![50.0, 150.0]);
        // A non-integer ratio weights the straddling cell by its overlap: 3 cells onto 2.
        let mut t = Img::filled(3, 1, [0, 0, 0, 255]).unwrap();
        t.px[8..11].copy_from_slice(&[90, 90, 90]);
        let g = gray_cells(&t, 2, 1);
        // Target cell 0 covers source cell 0 and half of cell 1 (both 0); cell 1 covers half of 1 and all of 2.
        assert_eq!(g, vec![0.0, 60.0]);
    }

    #[test]
    fn transparent_pixels_count_as_white() {
        let m = Img::filled(2, 2, [0, 0, 0, 0]).unwrap();
        assert_eq!(gray_cells(&m, 1, 1), vec![255.0]);
    }

    #[test]
    fn dhash_of_a_rising_ramp_is_zero_and_of_a_falling_ramp_all_ones() {
        assert_eq!(hex16(dhash(&ramp(90, 40, true))), "0000000000000000");
        assert_eq!(hex16(dhash(&ramp(90, 40, false))), "ffffffffffffffff");
    }

    #[test]
    fn dhash_sets_exactly_the_bits_where_a_cell_is_brighter_than_its_right_neighbour() {
        // 9x8 cells, one pixel each: only the first cell of the first row is bright.
        let mut m = Img::filled(9, 8, [10, 10, 10, 255]).unwrap();
        m.px[0..3].copy_from_slice(&[200, 200, 200]);
        assert_eq!(hex16(dhash(&m)), "8000000000000000");
        let mut m = Img::filled(9, 8, [10, 10, 10, 255]).unwrap();
        let i = m.at(7, 7);
        m.px[i..i + 3].copy_from_slice(&[200, 200, 200]);
        assert_eq!(hex16(dhash(&m)), "0000000000000001");
    }

    #[test]
    fn hashes_survive_a_resize_and_a_reencode() {
        let src = img(&fx::scene(256, 192));
        let small = resample(src.clone(), 96, 72, Filter::Lanczos3).unwrap();
        assert!(distance(phash(&src), phash(&small)) <= 6, "{}", distance(phash(&src), phash(&small)));
        assert!(distance(dhash(&src), dhash(&small)) <= 6);
        assert_eq!(phash(&src), phash(&src.clone()));
    }

    #[test]
    fn unrelated_pictures_have_distant_hashes() {
        let a = img(&fx::noise(64, 64, 1));
        let b = img(&fx::noise(64, 64, 2));
        let scene = img(&fx::scene(64, 64));
        assert!(distance(phash(&a), phash(&scene)) >= 12, "{}", distance(phash(&a), phash(&scene)));
        assert!(distance(dhash(&a), dhash(&b)) >= 12, "{}", distance(dhash(&a), dhash(&b)));
    }

    #[test]
    fn distance_and_hex_helpers() {
        assert_eq!(distance(0, u64::MAX), 64);
        assert_eq!(distance(0b1010, 0b0110), 2);
        assert_eq!(hex16(0xAB), "00000000000000ab");
        assert_eq!(parse16("00000000000000ab"), Some(0xAB));
        assert_eq!(parse16("zz"), None);
        assert_eq!(parse16("ab"), None);
    }
}
