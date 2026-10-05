//! Shared types: the supported formats, the RGBA working image and the hard
//! limits every call obeys. A decoded image is always 8-bit RGBA, row-major,
//! with no padding, so `px.len() == w * h * 4` holds for every `Img`.
use std::fmt;

/// Most pixels one decoded or created image may hold.
pub const MAX_PIXELS: u64 = 50_000_000;
/// Longest side of any decoded or created image.
pub const MAX_SIDE: u32 = 32_768;
/// Largest compressed image file read.
pub const MAX_FILE_BYTES: u64 = 256 << 20;
/// Largest inline image, as base64 text.
pub const MAX_INLINE_BASE64: usize = 64 * 1024 * 1024;
/// Allocation cap handed to the decoders.
pub const MAX_ALLOC: u64 = 1 << 30;
/// Raw image bytes one call attaches as parts; base64 inflates them by a third and
/// the whole result must stay under the 4 MiB output ceiling.
pub const PART_BUDGET: usize = 2_600_000;
/// Wall-clock seconds after which a call starts no further item and returns a cursor.
pub const WALL_SECS: u64 = 600;

/// An image format the plugin reads and writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
    Webp,
}

impl Format {
    /// The lower-case name used in every result.
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Webp => "webp",
        }
    }

    /// The media type of the encoded image.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Webp => "image/webp",
        }
    }

    /// The file extension written for the format.
    pub fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            other => other.name(),
        }
    }

    /// Whether a model can be shown an image in this format.
    pub fn viewable(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Gif | Self::Webp)
    }

    /// Parses a format name or common alias, case-insensitively.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "bmp" => Some(Self::Bmp),
            "tiff" | "tif" => Some(Self::Tiff),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    /// The format of `head`, the first bytes of a file, from its magic number.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            Some(Self::Png)
        } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if head.starts_with(b"BM") {
            Some(Self::Bmp)
        } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
            Some(Self::Tiff)
        } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An 8-bit RGBA image, row-major and unpadded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Img {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// `w * h * 4` bytes: red, green, blue, alpha.
    pub px: Vec<u8>,
}

impl Img {
    /// A transparent black image, or an error sentence when it would be over the pixel cap.
    pub fn new(w: u32, h: u32) -> Result<Self, String> {
        check_size(w, h)?;
        Ok(Self {
            w,
            h,
            px: vec![0; w as usize * h as usize * 4],
        })
    }

    /// An image of one colour.
    pub fn filled(w: u32, h: u32, rgba: [u8; 4]) -> Result<Self, String> {
        let mut img = Self::new(w, h)?;
        for p in img.px.chunks_exact_mut(4) {
            p.copy_from_slice(&rgba);
        }
        Ok(img)
    }

    /// Wraps existing pixel bytes; an error when their length does not match the size.
    pub fn from_raw(w: u32, h: u32, px: Vec<u8>) -> Result<Self, String> {
        check_size(w, h)?;
        if px.len() != w as usize * h as usize * 4 {
            return Err("the decoded pixel data does not match the image size".into());
        }
        Ok(Self { w, h, px })
    }

    /// The offset of pixel (`x`, `y`) in `px`; the caller keeps it inside the image.
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> usize {
        (y as usize * self.w as usize + x as usize) * 4
    }

    /// Pixel (`x`, `y`) as RGBA.
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = self.at(x, y);
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// Whether every pixel is fully opaque.
    pub fn opaque(&self) -> bool {
        self.px.chunks_exact(4).all(|p| p[3] == 255)
    }

    /// Whether every pixel has equal red, green and blue.
    pub fn gray(&self) -> bool {
        self.px.chunks_exact(4).all(|p| p[0] == p[1] && p[1] == p[2])
    }
}

/// Checks a size against the side and pixel caps.
pub fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("the image has no pixels".into());
    }
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!(
            "an image of {w}x{h} pixels is over the {MAX_PIXELS} pixel limit; use a smaller size"
        ));
    }
    Ok(())
}

/// Lower-case hex of a SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_reads_the_magic_not_the_name() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(Format::sniff(&png), Some(Format::Png));
        assert_eq!(Format::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(Format::sniff(b"GIF89a.."), Some(Format::Gif));
        assert_eq!(Format::sniff(b"GIF87a.."), Some(Format::Gif));
        assert_eq!(Format::sniff(b"BM\0\0"), Some(Format::Bmp));
        assert_eq!(Format::sniff(b"II*\0...."), Some(Format::Tiff));
        assert_eq!(Format::sniff(b"MM\0*...."), Some(Format::Tiff));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WEBPVP8L"), Some(Format::Webp));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(Format::sniff(b"%PDF-1.7"), None);
        assert_eq!(Format::sniff(b""), None);
        assert_eq!(Format::sniff(&[0x89, b'P', b'N', b'G']), None);
    }

    #[test]
    fn parse_accepts_aliases_in_any_case() {
        assert_eq!(Format::parse("JPG"), Some(Format::Jpeg));
        assert_eq!(Format::parse("Tif"), Some(Format::Tiff));
        assert_eq!(Format::parse("webp"), Some(Format::Webp));
        assert_eq!(Format::parse("svg"), None);
    }

    #[test]
    fn size_caps_refuse_zero_huge_and_overflowing_images() {
        assert!(check_size(0, 5).is_err());
        assert!(check_size(5, 0).is_err());
        assert!(check_size(32_769, 1).is_err());
        assert!(check_size(10_000, 10_000).is_err());
        assert!(check_size(100_000, 100_000).is_err());
        assert!(check_size(7000, 7000).is_ok());
        assert!(Img::new(1, 1).is_ok());
        assert!(Img::from_raw(2, 2, vec![0; 15]).is_err());
    }

    #[test]
    fn pixel_helpers_index_row_major() {
        let mut img = Img::new(3, 2).unwrap();
        let i = img.at(2, 1);
        assert_eq!(i, (3 + 2) * 4);
        img.px[i..i + 4].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(img.get(2, 1), [1, 2, 3, 4]);
        assert!(!img.opaque());
        assert!(Img::filled(2, 2, [9, 9, 9, 255]).unwrap().opaque());
        assert!(Img::filled(2, 2, [9, 9, 9, 255]).unwrap().gray());
        assert!(!Img::filled(2, 2, [9, 8, 9, 255]).unwrap().gray());
    }

    #[test]
    fn hex_and_sha256_match_the_known_digest() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
