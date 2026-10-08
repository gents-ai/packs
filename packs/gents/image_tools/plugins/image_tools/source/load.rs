//! Loading the pixels of a source the way every step wants them: decoded, and
//! turned upright by its Exif orientation unless the caller asked to keep the
//! stored layout.
use crate::decode::{Decoded, Header, LoadOpts, decode, header, view_target};
use crate::geom::orient;
use crate::model::Img;
use crate::src::Source;

/// Decodes `src`'s pixels. `max_side`, when set, says the caller will shrink
/// the result to that longest side, so a JPEG may be decoded at a fraction of its size.
pub fn pixels(
    src: &Source,
    h: &Header,
    frame: u32,
    turn: bool,
    max_side: Option<u32>,
) -> Result<(Img, Option<u32>, u8), String> {
    let downscale = max_side.map(|m| view_target(h.width, h.height, m));
    let Decoded {
        img,
        scale_denominator,
    } = decode(src, h, &LoadOpts { frame, downscale })?;
    let o = h.orientation();
    if turn && o != 1 {
        return Ok((orient(&img, o), scale_denominator, o));
    }
    Ok((img, scale_denominator, 1))
}

/// Reads the header and the pixels of `src` in one go; the picture is upright unless `turn` is false.
pub fn load(src: &Source, frame: u32, turn: bool, max_side: Option<u32>) -> Result<Img, String> {
    let header = header(src)?;
    pixels(src, &header, frame, turn, max_side).map(|(img, _, _)| img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures as fx;
    use crate::src::Data;
    use std::sync::Arc;

    fn mem(bytes: Vec<u8>) -> Source {
        Source {
            name: "t".into(),
            data: Data::Mem(Arc::new(bytes)),
        }
    }

    /// A 4x2 PNG whose pixels are distinct, with orientation `o` in its Exif block.
    fn tagged(o: u8) -> Source {
        let img = image::RgbaImage::from_fn(4, 2, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 100) as u8, 7, 255])
        });
        mem(fx::png_with_chunks(
            &fx::png(&img),
            &[fx::png_exif_chunk(&fx::exif_block(Some(o), None))],
        ))
    }

    fn read(src: &Source, turn: bool, max_side: Option<u32>) -> (Header, Img, Option<u32>, u8) {
        let h = header(src).unwrap();
        let (img, scale, turned) = pixels(src, &h, 0, turn, max_side).unwrap();
        (h, img, scale, turned)
    }

    #[test]
    fn the_pixels_are_turned_upright_by_default_and_left_stored_on_request() {
        let (h, upright, _, turned) = read(&tagged(6), true, None);
        assert_eq!((upright.w, upright.h, turned), (2, 4, 6));
        assert_eq!(
            (h.width, h.height),
            (4, 2),
            "the header keeps the stored size"
        );
        let (_, stored, _, turned) = read(&tagged(6), false, None);
        assert_eq!((stored.w, stored.h, turned), (4, 2, 1));
        // Orientation 6 turns clockwise: the stored top-left pixel lands at the top-right.
        assert_eq!(upright.get(1, 0), stored.get(0, 0));
        assert_eq!(upright.get(1, 3), stored.get(3, 0));
        assert_eq!(upright.get(0, 0), stored.get(0, 1));
    }

    #[test]
    fn an_image_without_orientation_is_untouched() {
        let (_, img, _, turned) = read(&tagged(1), true, None);
        assert_eq!((img.w, img.h, turned), (4, 2, 1));
    }

    #[test]
    fn a_jpeg_view_decodes_small_and_then_turns() {
        let jpeg = fx::jpeg_with_exif(
            &fx::jpeg(&fx::scene(800, 400), 90),
            &fx::exif_block(Some(6), None),
        );
        let (_, img, scale, turned) = read(&mem(jpeg), true, Some(100));
        // 800x400 shrinks to 100x50 for a 100 px view, which 1/8 decoding gives exactly; then it turns upright.
        assert_eq!(scale, Some(8));
        assert_eq!((img.w, img.h, turned), (50, 100, 6));
    }

    #[test]
    fn load_gives_the_upright_pixels_in_one_call() {
        let img = load(&tagged(6), 0, true, None).unwrap();
        assert_eq!((img.w, img.h), (2, 4));
        assert!(load(&mem(b"nope".to_vec()), 0, true, None).is_err());
    }
}
