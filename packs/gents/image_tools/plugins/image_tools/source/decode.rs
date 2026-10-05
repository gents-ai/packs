//! Reading an image: the header facts, then the pixels. The header is read
//! first and checked against the size caps, so a decompression bomb or a
//! 100000x100000 header is refused before any pixel memory is allocated.
//! Orientation is reported, not applied: the chain applies it.
use std::io::{Cursor, Read, Seek, SeekFrom};

use image::{
    AnimationDecoder, ExtendedColorType, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits,
};

use crate::exif::{self, Facts};
use crate::geom;
use crate::model::{Format, Img, MAX_ALLOC, MAX_SIDE, check_size};
use crate::src::Source;

/// What the file says about itself, without decoding its pixels.
#[derive(Debug, Clone)]
pub struct Header {
    /// The format found from the file's content.
    pub format: Format,
    /// Stored width in pixels.
    pub width: u32,
    /// Stored height in pixels.
    pub height: u32,
    /// The colour type of the file, such as `rgb8` or `l16`.
    pub color: String,
    /// Bits per channel.
    pub bit_depth: u32,
    /// Whether the file has an alpha channel.
    pub has_alpha: bool,
    /// Frames in the file; 1 for a still image.
    pub frames: u32,
    /// What the Exif block says, when there is one.
    pub exif: Option<Facts>,
    /// The embedded ICC profile.
    pub icc: Option<Vec<u8>>,
    /// Whether the file carries an XMP packet.
    pub xmp: bool,
    /// Size of the file in bytes.
    pub bytes: u64,
    /// Things that could not be read, one sentence each.
    pub warnings: Vec<String>,
}

impl Header {
    /// The orientation to apply, 1 when the file says nothing.
    pub fn orientation(&self) -> u8 {
        self.exif.and_then(|e| e.orientation).unwrap_or(1)
    }
}

/// How to decode.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOpts {
    /// The frame of an animated image, 0-based.
    pub frame: u32,
    /// A size, in stored orientation, the caller will shrink the result to;
    /// a JPEG may then be decoded at a fraction of its size.
    pub downscale: Option<(u32, u32)>,
}

/// Decoded pixels and what was done to get them.
pub struct Decoded {
    /// The pixels.
    pub img: Img,
    /// The decode ran at 1/n of the stored size, when set.
    pub scale_denominator: Option<u32>,
}

fn image_format(f: Format) -> ImageFormat {
    match f {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Gif => ImageFormat::Gif,
        Format::Bmp => ImageFormat::Bmp,
        Format::Tiff => ImageFormat::Tiff,
        Format::Webp => ImageFormat::WebP,
    }
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_SIDE);
    l.max_image_height = Some(MAX_SIDE);
    l.max_alloc = Some(MAX_ALLOC);
    l
}

/// One plain sentence for a decoder error.
fn describe(format: Format, e: &ImageError) -> String {
    match e {
        ImageError::Limits(_) => {
            format!("the {format} image is over the size limits; use a smaller image")
        }
        ImageError::Unsupported(_) => {
            format!(
                "this {format} image uses a feature that is not supported; convert it to PNG first"
            )
        }
        _ => corrupt(format),
    }
}

fn corrupt(format: Format) -> String {
    format!("the {format} image is corrupt or truncated; use an intact file")
}

const NOT_AN_IMAGE: &str = "the file is not a PNG, JPEG, GIF, BMP, TIFF or WebP image";

/// Reads the header facts of `src`.
pub fn header(src: &Source) -> Result<Header, String> {
    let format = src.sniff()?.ok_or_else(|| NOT_AN_IMAGE.to_string())?;
    let bytes = src.len()?;
    let mut reader = ImageReader::with_format(src.open_buf()?, image_format(format));
    reader.limits(limits());
    let mut dec = reader.into_decoder().map_err(|e| describe(format, &e))?;
    let (width, height) = dec.dimensions();
    check_size(width, height)?;
    if format == Format::Jpeg && !crate::jpeg::is_complete(&mut src.open_buf()?) {
        return Err(corrupt(format));
    }
    let ext = dec.original_color_type();
    let mut warnings = Vec::new();
    let mut exif = match dec.exif_metadata() {
        Ok(Some(blob)) => exif::read_facts(&mut Cursor::new(blob), 0),
        Ok(None) => None,
        Err(_) => {
            warnings.push("the Exif block could not be read".into());
            None
        }
    };
    if format == Format::Tiff {
        // A TIFF's first directory is not an Exif block unless it says something Exif-like.
        exif = exif::read_facts(&mut src.open()?, 0)
            .filter(|f| f.orientation.is_some() || f.has_exif_ifd || f.has_gps);
    }
    let icc = dec.icc_profile().unwrap_or_else(|_| {
        warnings.push("the ICC profile could not be read".into());
        None
    });
    let xmp = matches!(dec.xmp_metadata(), Ok(Some(_)));
    drop(dec);
    let frames = match format {
        Format::Gif => gif_frames(&mut src.open()?),
        Format::Webp => webp_frames(&mut src.open()?),
        _ => 1,
    }
    .max(1);
    Ok(Header {
        format,
        width,
        height,
        color: color_name(ext),
        bit_depth: bit_depth(ext),
        has_alpha: has_alpha(ext),
        frames,
        exif,
        icc,
        xmp,
        bytes,
        warnings,
    })
}

fn color_name(c: ExtendedColorType) -> String {
    format!("{c:?}").to_ascii_lowercase()
}

fn has_alpha(c: ExtendedColorType) -> bool {
    use ExtendedColorType as C;
    matches!(
        c,
        C::A8
            | C::La1
            | C::La2
            | C::La4
            | C::La8
            | C::La16
            | C::Rgba1
            | C::Rgba2
            | C::Rgba4
            | C::Rgba8
            | C::Rgba16
            | C::Bgra8
            | C::Rgba32F
    )
}

fn bit_depth(c: ExtendedColorType) -> u32 {
    let ch = u32::from(c.channel_count()).max(1);
    u32::from(c.bits_per_pixel()) / ch
}

/// Counts the image frames of a GIF by walking its blocks, without decoding them.
fn gif_frames<R: Read + Seek>(r: &mut R) -> u32 {
    fn skip_sub_blocks<R: Read + Seek>(r: &mut R) -> Option<()> {
        loop {
            let mut n = [0u8; 1];
            r.read_exact(&mut n).ok()?;
            if n[0] == 0 {
                return Some(());
            }
            r.seek(SeekFrom::Current(i64::from(n[0]))).ok()?;
        }
    }
    let walk = |r: &mut R| -> Option<u32> {
        let mut head = [0u8; 13];
        r.read_exact(&mut head).ok()?;
        if head[10] & 0x80 != 0 {
            r.seek(SeekFrom::Current(3 << ((head[10] & 7) + 1))).ok()?;
        }
        let mut frames = 0u32;
        loop {
            let mut b = [0u8; 1];
            if r.read_exact(&mut b).is_err() {
                return Some(frames);
            }
            match b[0] {
                0x2C => {
                    let mut d = [0u8; 9];
                    r.read_exact(&mut d).ok()?;
                    if d[8] & 0x80 != 0 {
                        r.seek(SeekFrom::Current(3 << ((d[8] & 7) + 1))).ok()?;
                    }
                    r.seek(SeekFrom::Current(1)).ok()?;
                    skip_sub_blocks(r)?;
                    frames = frames.saturating_add(1);
                }
                0x21 => {
                    r.seek(SeekFrom::Current(1)).ok()?;
                    skip_sub_blocks(r)?;
                }
                _ => return Some(frames),
            }
        }
    };
    r.seek(SeekFrom::Start(0)).ok();
    walk(r).unwrap_or(1)
}

/// Counts the frames of an animated WebP from its `ANMF` chunks.
fn webp_frames<R: Read + Seek>(r: &mut R) -> u32 {
    let walk = |r: &mut R| -> Option<u32> {
        r.seek(SeekFrom::Start(12)).ok()?;
        let mut frames = 0u32;
        loop {
            let mut h = [0u8; 8];
            if r.read_exact(&mut h).is_err() {
                return Some(frames);
            }
            let len = i64::from(u32::from_le_bytes([h[4], h[5], h[6], h[7]]));
            if &h[..4] == b"ANMF" {
                frames = frames.saturating_add(1);
            }
            r.seek(SeekFrom::Current(len + (len & 1))).ok()?;
        }
    };
    walk(r).unwrap_or(1)
}

/// Decodes the pixels of `src` to 8-bit RGBA.
pub fn decode(src: &Source, h: &Header, o: &LoadOpts) -> Result<Decoded, String> {
    if o.frame >= h.frames {
        return Err(if h.frames == 1 {
            "this image has one frame; leave frame out or set it to 0".into()
        } else {
            format!(
                "this image has {} frames, numbered 0 to {}; choose a frame in that range",
                h.frames,
                h.frames - 1
            )
        });
    }
    if o.frame > 0 {
        return decode_frame(src, h, o.frame);
    }
    if let (Format::Jpeg, Some(target)) = (h.format, o.downscale)
        && let Some(d) = jpeg_scaled(src, h, target)
    {
        return Ok(d);
    }
    let mut reader = ImageReader::with_format(src.open_buf()?, image_format(h.format));
    reader.limits(limits());
    let img = reader.decode().map_err(|e| describe(h.format, &e))?;
    from_dynamic(img.into_rgba8())
}

fn from_dynamic(buf: image::RgbaImage) -> Result<Decoded, String> {
    let (w, h) = buf.dimensions();
    Ok(Decoded {
        img: Img::from_raw(w, h, buf.into_raw())?,
        scale_denominator: None,
    })
}

fn decode_frame(src: &Source, h: &Header, frame: u32) -> Result<Decoded, String> {
    let bad = |e: ImageError| describe(h.format, &e);
    let mut frames = match h.format {
        Format::Gif => {
            let mut d = image::codecs::gif::GifDecoder::new(src.open_buf()?).map_err(bad)?;
            d.set_limits(limits()).map_err(bad)?;
            d.into_frames()
        }
        Format::Webp => {
            let mut d = image::codecs::webp::WebPDecoder::new(src.open_buf()?).map_err(bad)?;
            d.set_limits(limits()).map_err(bad)?;
            d.into_frames()
        }
        _ => return Err("only GIF and WebP images have more than one frame".into()),
    };
    match frames.nth(frame as usize) {
        Some(Ok(f)) => from_dynamic(f.into_buffer()),
        Some(Err(e)) => Err(describe(h.format, &e)),
        None => Err(format!(
            "frame {frame} could not be read from this {} image",
            h.format
        )),
    }
}

/// The side length of a JPEG decoded at `k`/8 of its size.
fn scaled_side(len: u32, k: u32) -> u32 {
    (len * k - 1) / 8 + 1
}

/// Decodes a JPEG at 1/2, 1/4 or 1/8 of its size when the caller will shrink
/// it to `target` anyway, which skips most of the inverse-transform work and
/// memory. `None` means decode it the normal way (full size needed, or a
/// pixel layout the scaled decoder does not give as RGB or gray).
fn jpeg_scaled(src: &Source, h: &Header, target: (u32, u32)) -> Option<Decoded> {
    let k = [1u32, 2, 4]
        .into_iter()
        .find(|&k| scaled_side(h.width, k) >= target.0 && scaled_side(h.height, k) >= target.1)?;
    let (sw, sh) = (scaled_side(h.width, k), scaled_side(h.height, k));
    let mut dec = jpeg_decoder::Decoder::new(src.open_buf().ok()?);
    dec.scale(u16::try_from(sw).ok()?, u16::try_from(sh).ok()?)
        .ok()?;
    let px = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, hh) = (u32::from(info.width), u32::from(info.height));
    let rgba: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 if px.len() == (w * hh * 3) as usize => px
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        jpeg_decoder::PixelFormat::L8 if px.len() == (w * hh) as usize => {
            px.iter().flat_map(|&g| [g, g, g, 255]).collect()
        }
        _ => return None,
    };
    Some(Decoded {
        img: Img::from_raw(w, hh, rgba).ok()?,
        scale_denominator: Some(8 / k),
    })
}

/// The size a view of `max_side` shrinks an image of `w` x `h` to (an image
/// already inside the box keeps its size). The box is square, so the answer is
/// the same for the stored and the turned image.
pub fn view_target(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    if w <= max_side && h <= max_side {
        return (w, h);
    }
    geom::fit_dims(w, h, Some(max_side), Some(max_side))
}
