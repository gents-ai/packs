//! Making an image fit what a model can be sent: a longest side and a byte
//! budget. PNG is tried first (lossless); when it is too big the same pixels
//! go out as JPEG at falling quality, and only then is the picture scaled
//! down. Every step taken is reported, so a downscale is never silent.
use crate::encode::encode;
use crate::geom::fit_dims;
use crate::model::{Format, Img};
use crate::resize::{Filter, resample};

/// JPEG qualities tried, best first.
const LADDER: [u8; 6] = [90, 80, 70, 60, 50, 40];
/// Scale-down rounds before giving up.
const MAX_ROUNDS: usize = 12;
/// A picture is never shrunk below this longest side to meet a byte budget.
const FLOOR_SIDE: u32 = 16;

/// Which formats may be used.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefer {
    /// PNG when it fits, else JPEG.
    Auto,
    /// PNG only.
    Png,
    /// JPEG only.
    Jpeg,
}

/// The encoded result.
pub struct Fitted {
    /// The file.
    pub bytes: Vec<u8>,
    /// Its format.
    pub format: Format,
    /// The pixels that were encoded, at their final size.
    pub img: Img,
    /// Quality used when it is a JPEG.
    pub quality: Option<u8>,
    /// What was changed to make it fit, one sentence each.
    pub notes: Vec<String>,
}

/// Fits `img` within `max_side` and `max_bytes`. `origin` is the size to name
/// in notes when `img` is already a reduced decode of a larger picture. `icc`
/// is kept when the format can carry it.
pub fn fit(
    img: Img,
    origin: Option<(u32, u32)>,
    max_side: u32,
    max_bytes: usize,
    prefer: Prefer,
    icc: Option<&[u8]>,
) -> Result<Fitted, String> {
    let mut notes = Vec::new();
    let (ow, oh) = origin.unwrap_or((img.w, img.h));
    let mut cur = img;
    if cur.w.max(cur.h) > max_side {
        let (w, h) = fit_dims(cur.w, cur.h, Some(max_side), Some(max_side));
        cur = resample(cur, w, h, Filter::Lanczos3)?;
        notes.push(format!(
            "scaled from {ow}x{oh} to {}x{} to fit {max_side} pixels on the longest side",
            cur.w, cur.h
        ));
    }
    let mut size_note: Option<String> = None;
    let mut png_size: Option<usize> = None;
    for round in 0..=MAX_ROUNDS {
        if prefer != Prefer::Jpeg {
            let e = encode(&cur, Format::Png, None, icc)?;
            if e.bytes.len() <= max_bytes {
                notes.extend(size_note);
                return Ok(Fitted {
                    bytes: e.bytes,
                    format: Format::Png,
                    img: cur,
                    quality: None,
                    notes,
                });
            }
            png_size = png_size.or(Some(e.bytes.len()));
        }
        if prefer != Prefer::Png {
            let mut last = 0;
            for q in LADDER {
                let e = encode(&cur, Format::Jpeg, Some(q), icc)?;
                last = e.bytes.len();
                if last <= max_bytes {
                    if prefer == Prefer::Auto {
                        let png = png_size.map_or_else(String::new, |p| {
                            format!("the PNG was {p} bytes, over {max_bytes}; ")
                        });
                        notes.push(format!("{png}sent as JPEG at quality {q}"));
                    }
                    notes.extend(size_note);
                    notes.extend(e.notes);
                    return Ok(Fitted {
                        bytes: e.bytes,
                        format: Format::Jpeg,
                        img: cur,
                        quality: Some(q),
                        notes,
                    });
                }
            }
            png_size = png_size.or(Some(last));
        }
        if round == MAX_ROUNDS || cur.w.max(cur.h) <= FLOOR_SIDE {
            break;
        }
        // Bytes grow with area: aim a little under the budget so one round usually suffices.
        let have = png_size.unwrap_or(max_bytes * 2).max(1) as f64;
        let f = ((max_bytes as f64 / have) * 0.8).sqrt().clamp(0.3, 0.9);
        let (w, h) = (
            ((f64::from(cur.w) * f) as u32).max(1),
            ((f64::from(cur.h) * f) as u32).max(1),
        );
        cur = resample(cur, w, h, Filter::Lanczos3)?;
        png_size = None;
        size_note = Some(format!(
            "scaled from {ow}x{oh} to {}x{} to fit {max_bytes} bytes",
            cur.w, cur.h
        ));
    }
    Err(format!(
        "the image cannot be made to fit {max_bytes} bytes; raise max_bytes or send a simpler image"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures as fx;

    fn photo(w: u32, h: u32) -> Img {
        Img::from_raw(w, h, fx::noise(w, h, 7).into_raw()).unwrap()
    }

    #[test]
    fn a_small_flat_image_goes_out_as_png_untouched() {
        let img = Img::filled(32, 32, [10, 20, 30, 255]).unwrap();
        let f = fit(img.clone(), None, 1000, 100_000, Prefer::Auto, None).unwrap();
        assert_eq!(f.format, Format::Png);
        assert_eq!(f.img, img);
        assert!(f.notes.is_empty(), "{:?}", f.notes);
        assert!(f.quality.is_none());
    }

    #[test]
    fn the_longest_side_is_enforced_and_reported() {
        let img = Img::filled(400, 200, [1, 2, 3, 255]).unwrap();
        let f = fit(img, None, 100, 100_000, Prefer::Auto, None).unwrap();
        assert_eq!((f.img.w, f.img.h), (100, 50));
        assert_eq!(
            f.notes,
            vec!["scaled from 400x200 to 100x50 to fit 100 pixels on the longest side"]
        );
    }

    #[test]
    fn a_png_over_the_budget_falls_back_to_jpeg_and_says_so() {
        let img = photo(120, 120);
        let png = encode(&img, Format::Png, None, None).unwrap().bytes.len();
        let f = fit(img.clone(), None, 1000, png - 1, Prefer::Auto, None).unwrap();
        assert!(f.bytes.len() < png);
        assert_eq!(f.format, Format::Jpeg);
        assert!(f.quality.is_some());
        assert!(
            f.notes[0].contains(&format!("the PNG was {png} bytes")),
            "{:?}",
            f.notes
        );
        assert!(
            f.notes[0].contains("sent as JPEG at quality"),
            "{:?}",
            f.notes
        );
        assert_eq!(
            (f.img.w, f.img.h),
            (120, 120),
            "pixels were not scaled when JPEG sufficed"
        );
    }

    #[test]
    fn a_tight_budget_scales_the_picture_down_and_reports_it() {
        let img = photo(200, 200);
        let f = fit(img, None, 1000, 3000, Prefer::Auto, None).unwrap();
        assert!(f.bytes.len() <= 3000);
        assert!(f.img.w < 200 && f.img.h < 200);
        assert!(
            f.notes
                .iter()
                .any(|n| n.contains("bytes") && n.contains("scaled from 200x200")),
            "{:?}",
            f.notes
        );
    }

    #[test]
    fn png_only_and_jpeg_only_are_honoured() {
        let img = photo(80, 80);
        assert_eq!(
            fit(img.clone(), None, 1000, 1_000_000, Prefer::Jpeg, None)
                .unwrap()
                .format,
            Format::Jpeg
        );
        assert_eq!(
            fit(img.clone(), None, 1000, 1_000_000, Prefer::Png, None)
                .unwrap()
                .format,
            Format::Png
        );
        let tight = fit(img, None, 1000, 2500, Prefer::Png, None).unwrap();
        assert_eq!(tight.format, Format::Png);
        assert!(
            tight.img.w < 80,
            "PNG-only shrinks the picture instead of changing format"
        );
    }

    #[test]
    fn transparent_images_are_flattened_for_jpeg_with_a_note() {
        let mut img = photo(100, 100);
        for p in img.px.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 128;
        }
        let f = fit(img, None, 1000, 6000, Prefer::Jpeg, None).unwrap();
        assert!(
            f.notes.iter().any(|n| n.contains("flattened onto white")),
            "{:?}",
            f.notes
        );
    }

    #[test]
    fn an_impossible_budget_is_one_sentence() {
        let e = fit(photo(64, 64), None, 1000, 100, Prefer::Auto, None)
            .err()
            .unwrap();
        assert!(e.contains("cannot be made to fit 100 bytes"), "{e}");
    }

    #[test]
    fn fitting_is_deterministic() {
        let a = fit(photo(150, 90), None, 80, 5000, Prefer::Auto, None).unwrap();
        let b = fit(photo(150, 90), None, 80, 5000, Prefer::Auto, None).unwrap();
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(a.notes, b.notes);
    }
}
