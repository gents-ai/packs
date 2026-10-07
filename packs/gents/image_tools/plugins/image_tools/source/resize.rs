//! Resizing: fit, fill and exact, with the filters of the convolution
//! resampler. The resampler is pinned to its portable code path so the pixels
//! are identical on every CPU, and it weights colour by alpha so transparent
//! pixels do not bleed into their neighbours.
use fast_image_resize::{
    CpuExtensions, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image,
};

use crate::geom;
use crate::model::{Img, check_size};

/// How the target size is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The largest size inside the box that keeps the aspect ratio.
    Fit,
    /// Scale to cover the box, then cut the centred box out of it.
    Fill,
    /// Exactly the box, stretching the picture if needed.
    Exact,
}

/// The resampling filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    /// Nearest pixel: hard edges, for pixel art.
    Nearest,
    /// Box average: exact means when shrinking by whole factors.
    Box,
    /// Linear interpolation.
    Bilinear,
    /// Sharper cubic filter.
    CatmullRom,
    /// Highest quality for photos; the default.
    Lanczos3,
}

impl Filter {
    /// Parses a filter name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "nearest" => Some(Self::Nearest),
            "box" => Some(Self::Box),
            "bilinear" => Some(Self::Bilinear),
            "catmull_rom" => Some(Self::CatmullRom),
            "lanczos3" => Some(Self::Lanczos3),
            _ => None,
        }
    }

    /// The name used in results.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Box => "box",
            Self::Bilinear => "bilinear",
            Self::CatmullRom => "catmull_rom",
            Self::Lanczos3 => "lanczos3",
        }
    }

    fn alg(self) -> ResizeAlg {
        match self {
            Self::Nearest => ResizeAlg::Nearest,
            Self::Box => ResizeAlg::Convolution(FilterType::Box),
            Self::Bilinear => ResizeAlg::Convolution(FilterType::Bilinear),
            Self::CatmullRom => ResizeAlg::Convolution(FilterType::CatmullRom),
            Self::Lanczos3 => ResizeAlg::Convolution(FilterType::Lanczos3),
        }
    }
}

/// A resize request.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// How the size is chosen.
    pub mode: Mode,
    /// Box width in pixels.
    pub width: Option<u32>,
    /// Box height in pixels.
    pub height: Option<u32>,
    /// The filter.
    pub filter: Filter,
    /// Whether `Fit` may enlarge an image that is already inside the box.
    pub upscale: bool,
}

/// The size to resample to and, for fill, the window (x, y, width, height) to keep.
pub type Target = ((u32, u32), Option<(u32, u32, u32, u32)>);

/// The size to resample to and the window to keep, or an error sentence.
/// `None` means the image is already inside the box and stays as it is.
pub fn plan(sw: u32, sh: u32, s: &Spec) -> Result<Option<Target>, String> {
    if s.width == Some(0) || s.height == Some(0) {
        return Err("width and height must be at least 1 pixel".into());
    }
    match s.mode {
        Mode::Fit => {
            if s.width.is_none() && s.height.is_none() {
                return Err("fit needs a width, a height or both".into());
            }
            let inside = s.width.is_none_or(|w| sw <= w) && s.height.is_none_or(|h| sh <= h);
            if inside && !s.upscale {
                return Ok(None);
            }
            let (w, h) = geom::fit_dims(sw, sh, s.width, s.height);
            check_size(w, h)?;
            Ok(Some(((w, h), None)))
        }
        Mode::Fill | Mode::Exact => {
            let (Some(bw), Some(bh)) = (s.width, s.height) else {
                return Err("fill and exact need both a width and a height".into());
            };
            check_size(bw, bh)?;
            if s.mode == Mode::Exact {
                return Ok(Some(((bw, bh), None)));
            }
            let ((cw, ch), (ox, oy)) = geom::fill_dims(sw, sh, bw, bh);
            check_size(cw, ch)?;
            Ok(Some(((cw, ch), Some((ox, oy, bw, bh)))))
        }
    }
}

/// Resamples to exactly `w` x `h`.
pub fn resample(img: Img, w: u32, h: u32, filter: Filter) -> Result<Img, String> {
    check_size(w, h)?;
    if (w, h) == (img.w, img.h) {
        return Ok(img);
    }
    fn broken<E>(_: E) -> String {
        "the image could not be resized; try a smaller size".to_string()
    }
    let src = Image::from_vec_u8(img.w, img.h, img.px, PixelType::U8x4).map_err(broken)?;
    let mut dst = Image::new(w, h, PixelType::U8x4);
    let mut resizer = Resizer::new();
    // SAFETY: `None` is the portable code path every CPU supports.
    unsafe { resizer.set_cpu_extensions(CpuExtensions::None) };
    resizer
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(filter.alg()),
        )
        .map_err(broken)?;
    Img::from_raw(w, h, dst.into_vec())
}

/// Applies a resize request. The second value says whether the size changed.
pub fn apply(img: Img, s: &Spec) -> Result<(Img, bool), String> {
    match plan(img.w, img.h, s)? {
        None => Ok((img, false)),
        Some(((w, h), window)) => {
            let scaled = resample(img, w, h, s.filter)?;
            Ok(match window {
                Some((x, y, bw, bh)) => (geom::crop(&scaled, x, y, bw, bh)?, true),
                None => (scaled, true),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Img {
        Img::filled(w, h, c).unwrap()
    }

    fn spec(mode: Mode, w: Option<u32>, h: Option<u32>) -> Spec {
        Spec {
            mode,
            width: w,
            height: h,
            filter: Filter::Lanczos3,
            upscale: false,
        }
    }

    #[test]
    fn fit_shrinks_inside_the_box_and_keeps_the_aspect() {
        let (img, changed) = apply(
            solid(400, 200, [9, 9, 9, 255]),
            &spec(Mode::Fit, Some(100), Some(100)),
        )
        .unwrap();
        assert!(changed);
        assert_eq!((img.w, img.h), (100, 50));
        let (img, _) = apply(
            solid(400, 200, [9, 9, 9, 255]),
            &spec(Mode::Fit, None, Some(40)),
        )
        .unwrap();
        assert_eq!((img.w, img.h), (80, 40));
    }

    #[test]
    fn fit_leaves_a_smaller_image_alone_unless_upscale_is_set() {
        let (img, changed) = apply(
            solid(40, 20, [1, 2, 3, 255]),
            &spec(Mode::Fit, Some(100), Some(100)),
        )
        .unwrap();
        assert!(!changed);
        assert_eq!((img.w, img.h), (40, 20));
        let mut s = spec(Mode::Fit, Some(100), Some(100));
        s.upscale = true;
        let (img, changed) = apply(solid(40, 20, [1, 2, 3, 255]), &s).unwrap();
        assert!(changed);
        assert_eq!((img.w, img.h), (100, 50));
    }

    #[test]
    fn fit_to_exactly_the_current_size_is_unchanged_and_one_pixel_less_shrinks() {
        let run = |w, h| apply(solid(40, 20, [1, 2, 3, 255]), &spec(Mode::Fit, w, h)).unwrap();
        for (w, h) in [(Some(40), None), (None, Some(20)), (Some(40), Some(20))] {
            let (img, changed) = run(w, h);
            assert!(!changed, "{w:?} x {h:?}");
            assert_eq!((img.w, img.h), (40, 20));
        }
        assert_eq!(
            plan(40, 20, &spec(Mode::Fit, Some(40), None)).unwrap(),
            None
        );
        assert_eq!(
            plan(40, 20, &spec(Mode::Fit, None, Some(20))).unwrap(),
            None
        );
        let (img, changed) = run(Some(39), None);
        assert!(changed);
        assert_eq!((img.w, img.h), (39, 20), "19.5 rounds up");
        let (img, changed) = run(None, Some(19));
        assert!(changed);
        assert_eq!((img.w, img.h), (38, 19));
    }

    #[test]
    fn exact_stretches_and_fill_covers_then_crops_the_centre() {
        let (img, _) = apply(
            solid(40, 20, [5, 5, 5, 255]),
            &spec(Mode::Exact, Some(10), Some(10)),
        )
        .unwrap();
        assert_eq!((img.w, img.h), (10, 10));
        // Left half red, right half blue: filling a square keeps the centre, which is half and half.
        let mut src = solid(40, 20, [255, 0, 0, 255]);
        for y in 0..20 {
            for x in 20..40 {
                let i = src.at(x, y);
                src.px[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let mut s = spec(Mode::Fill, Some(10), Some(10));
        s.filter = Filter::Nearest;
        let (img, _) = apply(src, &s).unwrap();
        assert_eq!((img.w, img.h), (10, 10));
        assert_eq!(img.get(0, 5), [255, 0, 0, 255]);
        assert_eq!(img.get(4, 5), [255, 0, 0, 255]);
        assert_eq!(img.get(5, 5), [0, 0, 255, 255]);
        assert_eq!(img.get(9, 5), [0, 0, 255, 255]);
    }

    #[test]
    fn missing_or_zero_sizes_and_oversize_targets_are_refused() {
        let i = || solid(10, 10, [0, 0, 0, 255]);
        assert!(
            apply(i(), &spec(Mode::Fit, None, None))
                .unwrap_err()
                .contains("fit needs")
        );
        assert!(
            apply(i(), &spec(Mode::Fill, Some(5), None))
                .unwrap_err()
                .contains("both")
        );
        assert!(
            apply(i(), &spec(Mode::Exact, None, Some(5)))
                .unwrap_err()
                .contains("both")
        );
        assert!(
            apply(i(), &spec(Mode::Fit, Some(0), Some(5)))
                .unwrap_err()
                .contains("at least 1")
        );
        assert!(
            apply(i(), &spec(Mode::Exact, Some(100_000), Some(100_000)))
                .unwrap_err()
                .contains("limit")
        );
        let mut up = spec(Mode::Fit, Some(30_000), Some(30_000));
        up.upscale = true;
        assert!(apply(i(), &up).unwrap_err().contains("limit"));
    }

    #[test]
    fn box_filter_gives_the_mean_when_halving() {
        let mut src = solid(2, 2, [0, 0, 0, 255]);
        let vals = [10u8, 20, 30, 40];
        for (i, v) in vals.iter().enumerate() {
            src.px[i * 4..i * 4 + 3].copy_from_slice(&[*v, *v, *v]);
        }
        let out = resample(src, 1, 1, Filter::Box).unwrap();
        assert_eq!(out.get(0, 0), [25, 25, 25, 255]);
    }

    #[test]
    fn nearest_repeats_pixels_when_doubling() {
        let mut src = solid(2, 1, [0, 0, 0, 255]);
        src.px[0..4].copy_from_slice(&[1, 2, 3, 255]);
        src.px[4..8].copy_from_slice(&[9, 8, 7, 255]);
        let out = resample(src, 4, 2, Filter::Nearest).unwrap();
        assert_eq!(out.get(0, 0), [1, 2, 3, 255]);
        assert_eq!(out.get(1, 1), [1, 2, 3, 255]);
        assert_eq!(out.get(2, 0), [9, 8, 7, 255]);
        assert_eq!(out.get(3, 1), [9, 8, 7, 255]);
    }

    #[test]
    fn a_flat_colour_stays_flat_under_every_filter() {
        for f in [
            Filter::Nearest,
            Filter::Box,
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Lanczos3,
        ] {
            let out = resample(solid(37, 29, [10, 120, 230, 255]), 11, 7, f).unwrap();
            assert!(
                out.px
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == [10, 120, 230, 255]),
                "{}",
                f.name()
            );
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_colour_into_their_neighbours() {
        // Left column opaque red, right column fully transparent green.
        let mut src = solid(2, 1, [255, 0, 0, 255]);
        src.px[4..8].copy_from_slice(&[0, 255, 0, 0]);
        let out = resample(src, 1, 1, Filter::Box).unwrap();
        let p = out.get(0, 0);
        assert!(p[0] >= 250 && p[1] <= 5, "alpha weighted: {p:?}");
        assert!((i32::from(p[3]) - 128).abs() <= 1, "{p:?}");
    }

    #[test]
    fn filter_names_round_trip() {
        for n in ["nearest", "box", "bilinear", "catmull_rom", "lanczos3"] {
            assert_eq!(Filter::parse(n).unwrap().name(), n);
        }
        assert!(Filter::parse("sinc").is_none());
    }
}
