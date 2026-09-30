//! Decoded pixels: one gray or RGB buffer with the bounded decode, downscale
//! and encode steps the OCR engine and the figure parts share.
use std::borrow::Cow;
use std::io::Cursor;

use image::{DynamicImage, GenericImageView, ImageReader, Limits};

/// Decoding refuses images above this many pixels (one 8-bit RGB copy is 3 bytes each).
pub const MAX_DECODE_PIXELS: u64 = 50_000_000;

pub struct Pix {
    pub w: u32,
    pub h: u32,
    /// 1 (gray) or 3 (RGB) interleaved 8-bit channels.
    pub ch: u8,
    pub data: Vec<u8>,
}

/// Header-only dimensions and format of an encoded image, `None` when the
/// bytes are not a raster format the plugin decodes.
pub fn probe(bytes: &[u8]) -> Option<(u32, u32, image::ImageFormat)> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some((w, h, format))
}

impl Pix {
    pub fn gray(w: u32, h: u32, data: Vec<u8>) -> Self {
        Self { w, h, ch: 1, data }
    }

    pub fn rgb(w: u32, h: u32, data: Vec<u8>) -> Self {
        Self { w, h, ch: 3, data }
    }

    /// Decodes an encoded image, refusing oversized ones before allocating.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let (w, h, _) = probe(bytes).ok_or(
            "the image format is not readable (PNG, JPEG, GIF, BMP, TIFF and WebP are supported)",
        )?;
        if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS {
            return Err(format!(
                "the image is {w}x{h} pixels, over the {MAX_DECODE_PIXELS} pixel decode limit"
            ));
        }
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = Limits::default();
        limits.max_alloc = Some(768 * 1024 * 1024);
        reader.limits(limits);
        let img = reader
            .decode()
            .map_err(|e| format!("the image is corrupt: {e}"))?;
        Ok(Self::from_dynamic(img))
    }

    /// Converts without alpha: transparent pixels are composited on white.
    pub fn from_dynamic(img: DynamicImage) -> Self {
        let (w, h) = img.dimensions();
        match img {
            DynamicImage::ImageLuma8(g) => Self::gray(w, h, g.into_raw()),
            DynamicImage::ImageLumaA8(ga) => {
                let data = ga
                    .as_raw()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| over_white(p[0], p[1]))
                    .collect();
                Self::gray(w, h, data)
            }
            DynamicImage::ImageRgba8(rgba) => {
                let mut data = Vec::with_capacity((w * h * 3) as usize);
                for p in rgba.as_raw().as_chunks::<4>().0 {
                    data.extend([
                        over_white(p[0], p[3]),
                        over_white(p[1], p[3]),
                        over_white(p[2], p[3]),
                    ]);
                }
                Self::rgb(w, h, data)
            }
            other => {
                if other.color().has_alpha() {
                    Self::from_dynamic(DynamicImage::ImageRgba8(other.to_rgba8()))
                } else if other.color().has_color() {
                    Self::rgb(w, h, other.into_rgb8().into_raw())
                } else {
                    Self::gray(w, h, other.into_luma8().into_raw())
                }
            }
        }
    }

    /// Scales down so the longer side is at most `max_px`; never scales up.
    pub fn fit(self, max_px: u32) -> Result<Self, String> {
        let longest = self.w.max(self.h);
        if longest <= max_px {
            return Ok(self);
        }
        let mismatch = || "the pixel buffer does not match its dimensions".to_string();
        let scale = f64::from(max_px) / f64::from(longest);
        let nw = ((f64::from(self.w) * scale).round() as u32).max(1);
        let nh = ((f64::from(self.h) * scale).round() as u32).max(1);
        let filter = image::imageops::FilterType::Triangle;
        if self.ch == 1 {
            let buf = image::GrayImage::from_raw(self.w, self.h, self.data).ok_or_else(mismatch)?;
            Ok(Self::gray(
                nw,
                nh,
                image::imageops::resize(&buf, nw, nh, filter).into_raw(),
            ))
        } else {
            let buf = image::RgbImage::from_raw(self.w, self.h, self.data).ok_or_else(mismatch)?;
            Ok(Self::rgb(
                nw,
                nh,
                image::imageops::resize(&buf, nw, nh, filter).into_raw(),
            ))
        }
    }

    /// Luma bytes for the OCR engine, without copying when already gray.
    pub fn luma(&self) -> Cow<'_, [u8]> {
        if self.ch == 1 {
            return Cow::Borrowed(&self.data);
        }
        Cow::Owned(
            self.data
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| {
                    ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000)
                        as u8
                })
                .collect(),
        )
    }

    pub fn jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let color = if self.ch == 1 {
            image::ExtendedColorType::L8
        } else {
            image::ExtendedColorType::Rgb8
        };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode(&self.data, self.w, self.h, color)
            .map_err(|e| format!("cannot encode the figure image: {e}"))?;
        Ok(out)
    }
}

fn over_white(value: u8, alpha: u8) -> u8 {
    let a = u32::from(alpha);
    ((u32::from(value) * a + 255 * (255 - a)) / 255) as u8
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;

    use super::*;

    #[test]
    fn fit_downscales_and_never_upscales() {
        let p = Pix::gray(400, 200, vec![128; 400 * 200]).fit(100).unwrap();
        assert_eq!((p.w, p.h, p.data.len()), (100, 50, 5000));
        let q = Pix::rgb(10, 10, vec![0; 300]).fit(100).unwrap();
        assert_eq!((q.w, q.h), (10, 10));
    }

    #[test]
    fn transparent_pixels_become_white() {
        let img =
            DynamicImage::ImageRgba8(image::RgbaImage::from_raw(1, 1, vec![0, 0, 0, 0]).unwrap());
        assert_eq!(Pix::from_dynamic(img).data, vec![255, 255, 255]);
    }

    #[test]
    fn every_supported_raster_format_decodes() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])
        }));
        for (format, name) in [
            (image::ImageFormat::Png, "png"),
            (image::ImageFormat::Jpeg, "jpeg"),
            (image::ImageFormat::Gif, "gif"),
            (image::ImageFormat::Bmp, "bmp"),
            (image::ImageFormat::Tiff, "tiff"),
            (image::ImageFormat::WebP, "webp"),
        ] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            img.write_to(&mut bytes, format)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let pix = Pix::decode(bytes.get_ref()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((pix.w, pix.h), (64, 48), "{name}");
            assert_eq!(
                crate::detect::detect("image.bin", bytes.get_ref())
                    .unwrap()
                    .name(),
                name
            );
        }
    }

    #[test]
    fn oversized_images_are_refused_before_decoding() {
        let side = 7200; // 51.8 megapixels, over MAX_DECODE_PIXELS
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        )
        .write_image(
            &vec![255u8; side * side],
            side as u32,
            side as u32,
            image::ExtendedColorType::L8,
        )
        .unwrap();
        let err = Pix::decode(&png).err().expect("must be refused");
        assert!(err.contains("decode limit"), "{err}");
    }

    #[test]
    fn jpeg_round_trips_through_decode() {
        let jpeg = Pix::gray(32, 32, vec![200; 32 * 32]).jpeg(80).unwrap();
        let back = Pix::decode(&jpeg).unwrap();
        assert_eq!((back.w, back.h), (32, 32));
        assert!(Pix::decode(b"not an image").is_err());
    }
}
