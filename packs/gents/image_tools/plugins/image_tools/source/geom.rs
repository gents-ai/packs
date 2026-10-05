//! Geometry on whole images: the eight Exif orientations (which also give
//! rotate and flip), crop, and the size arithmetic for fit and fill. All of it
//! is exact integer math, so results are identical on every platform.
use crate::model::Img;

/// Applies the Exif orientation `n` (1 to 8) so the image shows upright.
/// 1 is the identity, 2 mirrors left-right, 3 turns 180 degrees, 4 mirrors
/// top-bottom, 5 transposes, 6 turns 90 degrees clockwise, 7 transposes along
/// the other diagonal and 8 turns 270 degrees clockwise. Any other value is the identity.
pub fn orient(img: &Img, n: u8) -> Img {
    let (w, h) = (img.w as usize, img.h as usize);
    let swap = (5..=8).contains(&n);
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut px = vec![0u8; img.px.len()];
    for yo in 0..oh {
        for xo in 0..ow {
            let (xs, ys) = match n {
                2 => (w - 1 - xo, yo),
                3 => (w - 1 - xo, h - 1 - yo),
                4 => (xo, h - 1 - yo),
                5 => (yo, xo),
                6 => (yo, h - 1 - xo),
                7 => (w - 1 - yo, h - 1 - xo),
                8 => (w - 1 - yo, xo),
                _ => (xo, yo),
            };
            let (s, d) = ((ys * w + xs) * 4, (yo * ow + xo) * 4);
            px[d..d + 4].copy_from_slice(&img.px[s..s + 4]);
        }
    }
    Img {
        w: ow as u32,
        h: oh as u32,
        px,
    }
}

/// Turns the image clockwise by `degrees`, a multiple of 90 (negative turns counter-clockwise).
pub fn rotate(img: &Img, degrees: i64) -> Result<Img, String> {
    if degrees % 90 != 0 {
        return Err("rotate takes a multiple of 90 degrees, such as 90, 180 or -90".into());
    }
    Ok(match degrees.rem_euclid(360) {
        90 => orient(img, 6),
        180 => orient(img, 3),
        270 => orient(img, 8),
        _ => img.clone(),
    })
}

/// Mirrors the image left-right (`horizontal`) or top-bottom.
pub fn flip(img: &Img, horizontal: bool) -> Img {
    orient(img, if horizontal { 2 } else { 4 })
}

/// The box `x`, `y`, `w`, `h` cut out of the image.
pub fn crop(img: &Img, x: u32, y: u32, w: u32, h: u32) -> Result<Img, String> {
    if w == 0 || h == 0 {
        return Err("the crop box must be at least 1 pixel wide and high".into());
    }
    let (right, bottom) = (u64::from(x) + u64::from(w), u64::from(y) + u64::from(h));
    if right > u64::from(img.w) || bottom > u64::from(img.h) {
        return Err(format!(
            "the crop box {x},{y} {w}x{h} is outside the {}x{} image; choose a box inside it",
            img.w, img.h
        ));
    }
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let start = img.at(x, row);
        px.extend_from_slice(&img.px[start..start + w as usize * 4]);
    }
    Ok(Img { w, h, px })
}

/// `a * b / c`, rounded half up, in 128-bit space so it cannot overflow.
fn mul_div(a: u32, b: u32, c: u32) -> u32 {
    ((u128::from(a) * u128::from(b) + u128::from(c) / 2) / u128::from(c)) as u32
}

/// The largest size with the source's aspect ratio that fits in `bw` x `bh`;
/// either bound may be absent. Never exceeds the box and is at least 1x1.
pub fn fit_dims(sw: u32, sh: u32, bw: Option<u32>, bh: Option<u32>) -> (u32, u32) {
    match (bw, bh) {
        (Some(bw), Some(bh)) => {
            // Compare bw/sw with bh/sh without division: the smaller ratio binds.
            if u64::from(bw) * u64::from(sh) <= u64::from(bh) * u64::from(sw) {
                (bw, mul_div(sh, bw, sw).clamp(1, bh))
            } else {
                (mul_div(sw, bh, sh).clamp(1, bw), bh)
            }
        }
        (Some(bw), None) => (bw, mul_div(sh, bw, sw).max(1)),
        (None, Some(bh)) => (mul_div(sw, bh, sh).max(1), bh),
        (None, None) => (sw, sh),
    }
}

/// The size an image is scaled to so it covers `bw` x `bh`, and the offset of
/// the centred `bw` x `bh` window inside it.
pub fn fill_dims(sw: u32, sh: u32, bw: u32, bh: u32) -> ((u32, u32), (u32, u32)) {
    let (cw, ch) = if u64::from(bw) * u64::from(sh) >= u64::from(bh) * u64::from(sw) {
        (bw, mul_div(sh, bw, sw).max(bh))
    } else {
        (mul_div(sw, bh, sh).max(bw), bh)
    };
    ((cw, ch), ((cw - bw) / 2, (ch - bh) / 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3x2 image whose pixels are the letters A..F: `ABC` over `DEF`.
    fn letters() -> Img {
        let mut px = Vec::new();
        for c in b"ABCDEF" {
            px.extend([*c, 0, 0, 255]);
        }
        Img::from_raw(3, 2, px).unwrap()
    }

    fn read(img: &Img) -> Vec<String> {
        (0..img.h)
            .map(|y| (0..img.w).map(|x| img.get(x, y)[0] as char).collect())
            .collect()
    }

    #[test]
    fn each_exif_orientation_gives_the_known_picture() {
        let src = letters();
        let cases: [(u8, &[&str]); 8] = [
            (1, &["ABC", "DEF"]),
            (2, &["CBA", "FED"]),
            (3, &["FED", "CBA"]),
            (4, &["DEF", "ABC"]),
            (5, &["AD", "BE", "CF"]),
            (6, &["DA", "EB", "FC"]),
            (7, &["FC", "EB", "DA"]),
            (8, &["CF", "BE", "AD"]),
        ];
        for (n, want) in cases {
            assert_eq!(read(&orient(&src, n)), want, "orientation {n}");
        }
    }

    #[test]
    fn an_unknown_orientation_changes_nothing() {
        let src = letters();
        for n in [0, 9, 255] {
            assert_eq!(orient(&src, n), src);
        }
    }

    #[test]
    fn rotate_turns_clockwise_and_negative_turns_back() {
        let src = letters();
        assert_eq!(read(&rotate(&src, 90).unwrap()), ["DA", "EB", "FC"]);
        assert_eq!(read(&rotate(&src, 180).unwrap()), ["FED", "CBA"]);
        assert_eq!(read(&rotate(&src, 270).unwrap()), ["CF", "BE", "AD"]);
        assert_eq!(rotate(&src, -90).unwrap(), rotate(&src, 270).unwrap());
        assert_eq!(rotate(&src, -270).unwrap(), rotate(&src, 90).unwrap());
        assert_eq!(rotate(&src, 360).unwrap(), src);
        assert_eq!(rotate(&src, 0).unwrap(), src);
        assert_eq!(rotate(&src, 450).unwrap(), rotate(&src, 90).unwrap());
        assert!(rotate(&src, 45).unwrap_err().contains("multiple of 90"));
    }

    #[test]
    fn flip_mirrors_one_axis() {
        let src = letters();
        assert_eq!(read(&flip(&src, true)), ["CBA", "FED"]);
        assert_eq!(read(&flip(&src, false)), ["DEF", "ABC"]);
    }

    #[test]
    fn rotation_and_flip_compose_into_the_other_orientations() {
        let src = letters();
        // Orientation 5 is a quarter turn clockwise then a mirror; 7 is a quarter turn back then a mirror.
        assert_eq!(orient(&src, 5), flip(&rotate(&src, 90).unwrap(), true));
        assert_eq!(orient(&src, 7), flip(&rotate(&src, 270).unwrap(), true));
        assert_eq!(rotate(&flip(&src, true), 180).unwrap(), flip(&src, false));
    }

    #[test]
    fn crop_takes_the_box_at_the_given_offset() {
        let src = letters();
        assert_eq!(read(&crop(&src, 1, 0, 2, 2).unwrap()), ["BC", "EF"]);
        assert_eq!(read(&crop(&src, 0, 1, 3, 1).unwrap()), ["DEF"]);
        assert_eq!(read(&crop(&src, 2, 1, 1, 1).unwrap()), ["F"]);
        assert_eq!(crop(&src, 0, 0, 3, 2).unwrap(), src);
    }

    #[test]
    fn crop_outside_the_image_or_empty_is_refused_with_the_box() {
        let src = letters();
        for (x, y, w, h) in [
            (1, 0, 3, 2),
            (0, 1, 1, 2),
            (3, 0, 1, 1),
            (0, 0, 0, 1),
            (0, 0, 1, 0),
        ] {
            assert!(crop(&src, x, y, w, h).is_err(), "{x},{y} {w}x{h}");
        }
        assert!(crop(&src, 1, 0, 3, 2).unwrap_err().contains("3x2 image"));
        assert!(
            crop(&src, u32::MAX, 0, 2, 1).is_err(),
            "no overflow past the edge"
        );
    }

    #[test]
    fn fit_keeps_the_aspect_inside_the_box() {
        assert_eq!(fit_dims(4000, 3000, Some(1000), Some(1000)), (1000, 750));
        assert_eq!(fit_dims(3000, 4000, Some(1000), Some(1000)), (750, 1000));
        assert_eq!(fit_dims(100, 100, Some(40), Some(60)), (40, 40));
        assert_eq!(fit_dims(100, 50, Some(30), Some(30)), (30, 15));
        assert_eq!(fit_dims(100, 50, Some(30), None), (30, 15));
        assert_eq!(fit_dims(100, 50, None, Some(10)), (20, 10));
        assert_eq!(fit_dims(100, 50, None, None), (100, 50));
        assert_eq!(
            fit_dims(1000, 1, Some(10), Some(10)),
            (10, 1),
            "a thin image keeps one row"
        );
        assert_eq!(fit_dims(1, 1000, Some(10), Some(10)), (1, 10));
        assert_eq!(
            fit_dims(7, 5, Some(3), Some(3)),
            (3, 2),
            "5*3/7 = 2.14 rounds to 2"
        );
    }

    #[test]
    fn fit_rounds_to_the_nearest_pixel_not_down() {
        // 5 * 5 / 7 = 3.57 -> 4; 7 * 5 / 5 would be exact. A floor would give 3.
        assert_eq!(fit_dims(7, 5, Some(5), Some(5)), (5, 4));
        // 9 * 4 / 11 = 3.27 -> 3 and 9 * 6 / 11 = 4.91 -> 5.
        assert_eq!(fit_dims(11, 9, Some(4), None), (4, 3));
        assert_eq!(fit_dims(11, 9, Some(6), None), (6, 5));
        // Exactly half rounds up: 3 * 1 / 2 = 1.5 -> 2.
        assert_eq!(fit_dims(2, 3, Some(1), None), (1, 2));
        assert_eq!(fit_dims(3, 2, None, Some(1)), (2, 1));
    }

    #[test]
    fn fill_covers_the_box_and_centres_the_window() {
        assert_eq!(fill_dims(400, 200, 100, 100), ((200, 100), (50, 0)));
        assert_eq!(fill_dims(200, 400, 100, 100), ((100, 200), (0, 50)));
        assert_eq!(fill_dims(100, 100, 50, 50), ((50, 50), (0, 0)));
        // An odd remainder rounds the centring offset down.
        assert_eq!(fill_dims(32, 24, 10, 10), ((13, 10), (1, 0)));
        assert_eq!(fill_dims(24, 32, 10, 10), ((10, 13), (0, 1)));
        assert_eq!(fill_dims(40, 30, 20, 10), ((20, 15), (0, 2)));
        let ((cw, ch), (ox, oy)) = fill_dims(333, 111, 50, 70);
        assert!(cw >= 50 && ch >= 70 && ox + 50 <= cw && oy + 70 <= ch);
    }
}
