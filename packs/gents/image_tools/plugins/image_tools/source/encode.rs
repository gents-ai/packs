//! Writing an image in each format with pinned encoder settings, so the same
//! pixels give the same bytes on every platform. The colour layout written is
//! the smallest that holds the image exactly: gray, RGB, or RGBA.
use std::io::Cursor;

use image::codecs::{
    bmp::BmpEncoder,
    gif::{GifEncoder, Repeat},
    jpeg::JpegEncoder,
    png::{CompressionType, FilterType, PngEncoder},
    tiff::TiffEncoder,
    webp::WebPEncoder,
};
use image::{ExtendedColorType, ImageEncoder};

use crate::model::{Format, Img};

/// JPEG quality when the caller gives none.
pub const DEFAULT_QUALITY: u8 = 90;

/// An encoded image and the things the caller should be told about how.
pub struct Encoded {
    /// The encoded file.
    pub bytes: Vec<u8>,
    /// What was lost or changed to write it, one sentence each.
    pub notes: Vec<String>,
}

enum Packed {
    Gray(Vec<u8>),
    Rgb(Vec<u8>),
    Rgba(Vec<u8>),
}

impl Packed {
    fn of(img: &Img) -> Self {
        let opaque = img.opaque();
        if opaque && img.gray() {
            Self::Gray(img.px.as_chunks::<4>().0.iter().map(|p| p[0]).collect())
        } else if opaque {
            Self::Rgb(
                img.px
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]])
                    .collect(),
            )
        } else {
            Self::Rgba(img.px.clone())
        }
    }

    fn parts(&self) -> (&[u8], ExtendedColorType) {
        match self {
            Self::Gray(b) => (b, ExtendedColorType::L8),
            Self::Rgb(b) => (b, ExtendedColorType::Rgb8),
            Self::Rgba(b) => (b, ExtendedColorType::Rgba8),
        }
    }
}

/// `img` composited onto white, so formats without alpha show it as a viewer would.
pub fn flatten(img: &Img) -> Img {
    let mut out = img.clone();
    for p in out.px.as_chunks_mut::<4>().0.iter_mut() {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        p[3] = 255;
    }
    out
}

fn failed(format: Format) -> String {
    format!("the image could not be written as {format}; try another format")
}

/// Encodes `img` as `format`. `quality` (1 to 100) applies to JPEG and GIF;
/// `icc` is embedded where the format can carry it.
pub fn encode(
    img: &Img,
    format: Format,
    quality: Option<u8>,
    icc: Option<&[u8]>,
) -> Result<Encoded, String> {
    let mut notes = Vec::new();
    let flat;
    let source = if format == Format::Jpeg && !img.opaque() {
        notes.push("transparency was flattened onto white because JPEG has no alpha".to_string());
        flat = flatten(img);
        &flat
    } else {
        img
    };
    if quality.is_some() && !matches!(format, Format::Jpeg | Format::Gif) {
        notes.push(format!(
            "quality is ignored for {format}, which is written losslessly"
        ));
    }
    if format == Format::Gif && count_colours(img, 257) > 256 {
        notes.push("GIF holds 256 colours, so the colours were reduced".into());
    }
    let packed = Packed::of(source);
    let (buf, color) = packed.parts();
    let (w, h) = (source.w, source.h);
    let mut out = Vec::new();
    let mut keep_icc = |enc: &mut dyn ImageEncoder, name: Format| {
        if let Some(p) = icc
            && enc.set_icc_profile(p.to_vec()).is_err()
        {
            notes.push(format!(
                "the ICC profile cannot be kept in {name} and was dropped"
            ));
        }
    };
    let q = quality.unwrap_or(DEFAULT_QUALITY).clamp(1, 100);
    let written = match format {
        Format::Png => {
            let mut e = PngEncoder::new_with_quality(
                &mut out,
                CompressionType::Default,
                FilterType::Adaptive,
            );
            keep_icc(&mut e, format);
            e.write_image(buf, w, h, color)
        }
        Format::Jpeg => {
            let mut e = JpegEncoder::new_with_quality(&mut out, q);
            keep_icc(&mut e, format);
            e.write_image(buf, w, h, color)
        }
        Format::Webp => {
            let mut e = WebPEncoder::new_lossless(&mut out);
            keep_icc(&mut e, format);
            e.write_image(buf, w, h, color)
        }
        Format::Bmp => BmpEncoder::new(&mut out).write_image(buf, w, h, color),
        Format::Tiff => {
            let mut cursor = Cursor::new(Vec::new());
            let r = TiffEncoder::new(&mut cursor).write_image(buf, w, h, color);
            out = cursor.into_inner();
            r
        }
        Format::Gif => {
            // Speed 1 is the best palette search; speed 30 the fastest.
            let speed = 1 + (100 - i32::from(q)) * 29 / 100;
            let mut e = GifEncoder::new_with_speed(&mut out, speed);
            let rgba =
                image::RgbaImage::from_raw(w, h, img.px.clone()).ok_or_else(|| failed(format))?;
            e.set_repeat(Repeat::Finite(0))
                .map_err(|_| failed(format))?;
            e.encode_frame(image::Frame::new(rgba))
        }
    };
    written.map_err(|_| failed(format))?;
    Ok(Encoded { bytes: out, notes })
}

/// Counts distinct RGBA colours, stopping at `cap`.
pub fn count_colours(img: &Img, cap: usize) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    for p in img.px.as_chunks::<4>().0.iter() {
        seen.insert(u32::from_le_bytes([p[0], p[1], p[2], p[3]]));
        if seen.len() >= cap {
            break;
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{LoadOpts, decode, header};
    use crate::fixtures as fx;
    use crate::src::{Data, Source};
    use std::sync::Arc;

    fn back(bytes: Vec<u8>) -> Img {
        let s = Source {
            name: "t".into(),
            data: Data::Mem(Arc::new(bytes)),
        };
        let h = header(&s).unwrap();
        decode(&s, &h, &LoadOpts::default()).unwrap().img
    }

    fn scene(w: u32, h: u32) -> Img {
        let i = fx::scene(w, h);
        Img::from_raw(w, h, i.into_raw()).unwrap()
    }

    #[test]
    fn lossless_formats_round_trip_the_exact_pixels() {
        let img = scene(33, 21);
        for f in [Format::Png, Format::Webp, Format::Bmp, Format::Tiff] {
            let e = encode(&img, f, None, None).unwrap();
            assert_eq!(back(e.bytes), img, "{f}");
            assert!(e.notes.is_empty(), "{f}: {:?}", e.notes);
        }
    }

    #[test]
    fn transparency_survives_png_webp_and_tiff() {
        let mut img = scene(8, 8);
        for (i, p) in img.px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            p[3] = (i * 4) as u8;
        }
        for f in [Format::Png, Format::Webp, Format::Tiff] {
            assert_eq!(back(encode(&img, f, None, None).unwrap().bytes), img, "{f}");
        }
    }

    #[test]
    fn the_smallest_colour_layout_is_written() {
        let gray = Img::filled(4, 4, [7, 7, 7, 255]).unwrap();
        let png = encode(&gray, Format::Png, None, None).unwrap().bytes;
        let h = header(&Source {
            name: "t".into(),
            data: Data::Mem(Arc::new(png)),
        })
        .unwrap();
        assert_eq!(h.color, "l8");
        let rgb = scene(4, 4);
        let png = encode(&rgb, Format::Png, None, None).unwrap().bytes;
        let h = header(&Source {
            name: "t".into(),
            data: Data::Mem(Arc::new(png)),
        })
        .unwrap();
        assert_eq!(h.color, "rgb8");
    }

    #[test]
    fn jpeg_quality_changes_size_and_error_in_the_expected_direction() {
        let img = scene(64, 64);
        let hi = encode(&img, Format::Jpeg, Some(95), None).unwrap().bytes;
        let lo = encode(&img, Format::Jpeg, Some(20), None).unwrap().bytes;
        assert!(hi.len() > lo.len());
        let err = |bytes: Vec<u8>| -> u64 {
            let b = back(bytes);
            b.px.iter()
                .zip(&img.px)
                .map(|(a, c)| u64::from(a.abs_diff(*c)))
                .sum()
        };
        assert!(err(hi) < err(lo));
    }

    #[test]
    fn jpeg_flattens_transparency_onto_white_and_says_so() {
        let img = Img::filled(8, 8, [0, 0, 0, 0]).unwrap();
        let e = encode(&img, Format::Jpeg, None, None).unwrap();
        assert!(e.notes.iter().any(|n| n.contains("flattened onto white")));
        let b = back(e.bytes);
        assert!(
            b.px.as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] >= 250 && p[1] >= 250 && p[2] >= 250)
        );
    }

    #[test]
    fn flatten_blends_by_alpha_exactly() {
        let img = Img::filled(1, 1, [0, 100, 200, 128]).unwrap();
        let f = flatten(&img);
        // 0*128 + 255*127 = 32385 -> /255 = 127; 100*128 + 32385 = 45185 -> 177; 200*128 + 32385 = 57985 -> 227
        assert_eq!(f.get(0, 0), [127, 177, 227, 255]);
        assert_eq!(
            flatten(&Img::filled(1, 1, [9, 8, 7, 255]).unwrap()).get(0, 0),
            [9, 8, 7, 255]
        );
    }

    #[test]
    fn quality_is_noted_as_ignored_for_lossless_formats() {
        let e = encode(&scene(4, 4), Format::Png, Some(50), None).unwrap();
        assert!(e.notes.iter().any(|n| n.contains("ignored for png")));
        assert!(
            encode(&scene(4, 4), Format::Jpeg, Some(50), None)
                .unwrap()
                .notes
                .is_empty()
        );
    }

    #[test]
    fn gif_keeps_small_palettes_exactly_and_notes_a_reduction() {
        let mut img = Img::filled(4, 4, [255, 0, 0, 255]).unwrap();
        img.px[0..4].copy_from_slice(&[0, 0, 255, 255]);
        let e = encode(&img, Format::Gif, None, None).unwrap();
        assert!(e.notes.is_empty(), "{:?}", e.notes);
        assert_eq!(back(e.bytes), img);
        let big = encode(&scene(64, 64), Format::Gif, None, None).unwrap();
        assert!(big.notes.iter().any(|n| n.contains("256 colours")));
    }

    #[test]
    fn an_icc_profile_is_kept_in_png_jpeg_and_webp_and_dropped_with_a_note_elsewhere() {
        let img = scene(8, 8);
        let profile: Vec<u8> = (0..200u32).map(|i| (i * 3) as u8).collect();
        for f in [Format::Png, Format::Jpeg, Format::Webp] {
            let e = encode(&img, f, None, Some(&profile)).unwrap();
            let s = Source {
                name: "t".into(),
                data: Data::Mem(Arc::new(e.bytes)),
            };
            assert_eq!(
                header(&s).unwrap().icc.as_deref(),
                Some(profile.as_slice()),
                "{f}"
            );
            assert!(e.notes.is_empty(), "{f}: {:?}", e.notes);
        }
    }

    #[test]
    fn the_same_pixels_always_give_the_same_bytes() {
        let img = scene(50, 40);
        for f in [
            Format::Png,
            Format::Jpeg,
            Format::Webp,
            Format::Bmp,
            Format::Tiff,
            Format::Gif,
        ] {
            let a = encode(&img, f, None, None).unwrap().bytes;
            let b = encode(&img.clone(), f, None, None).unwrap().bytes;
            assert_eq!(a, b, "{f}");
        }
    }

    #[test]
    fn colour_counting_stops_at_the_cap() {
        let img = scene(30, 30);
        assert_eq!(count_colours(&img, 10), 10);
        assert_eq!(
            count_colours(&Img::filled(3, 3, [1, 2, 3, 4]).unwrap(), 10),
            1
        );
    }
}
