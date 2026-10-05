//! Deterministic image fixtures: byte builders for the metadata containers the
//! plugin reads (Exif, ICC, XMP in JPEG, PNG and WebP), animated GIF and WebP,
//! and test pictures with known content. Shared by the unit tests and by
//! `tools/gen_fixtures.rs`, so a committed fixture can always be regenerated.
#![allow(dead_code)]

use image::{Rgba, RgbaImage};

/// A little-endian Exif block holding an orientation and, when given, a GPS
/// position (decimal degrees, latitude then longitude).
pub fn exif_block(orientation: Option<u8>, gps: Option<(f64, f64)>) -> Vec<u8> {
    let mut entries: Vec<[u8; 12]> = Vec::new();
    let mut entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        let mut e = [0u8; 12];
        e[0..2].copy_from_slice(&tag.to_le_bytes());
        e[2..4].copy_from_slice(&kind.to_le_bytes());
        e[4..8].copy_from_slice(&count.to_le_bytes());
        e[8..12].copy_from_slice(&value);
        e
    };
    if let Some(o) = orientation {
        entries.push(entry(0x0112, 3, 1, [o, 0, 0, 0]));
    }
    let ifd0_len = 2 + entries.len() * 12 + 4 + if gps.is_some() { 12 } else { 0 };
    let gps_at = 8 + ifd0_len as u32;
    if gps.is_some() {
        entries.push(entry(0x8825, 4, 1, gps_at.to_le_bytes()));
    }
    let mut out = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    out.extend((entries.len() as u16).to_le_bytes());
    for e in &entries {
        out.extend(e);
    }
    out.extend([0u8; 4]);
    if let Some((lat, lon)) = gps {
        // The GPS directory: four entries, then the two rational triples.
        let dir_len = 2 + 4 * 12 + 4;
        let lat_at = gps_at + dir_len as u32;
        let lon_at = lat_at + 24;
        let gps_entries = [
            entry(1, 2, 2, [if lat < 0.0 { b'S' } else { b'N' }, 0, 0, 0]),
            entry(2, 5, 3, lat_at.to_le_bytes()),
            entry(3, 2, 2, [if lon < 0.0 { b'W' } else { b'E' }, 0, 0, 0]),
            entry(4, 5, 3, lon_at.to_le_bytes()),
        ];
        out.extend(4u16.to_le_bytes());
        for e in &gps_entries {
            out.extend(e);
        }
        out.extend([0u8; 4]);
        for v in [lat.abs(), lon.abs()] {
            let deg = v.floor();
            let min = ((v - deg) * 60.0).floor();
            let sec = (v - deg - min / 60.0) * 3600.0;
            for (num, den) in [(deg as u32, 1u32), (min as u32, 1), ((sec * 1e6).round() as u32, 1_000_000)] {
                out.extend(num.to_le_bytes());
                out.extend(den.to_le_bytes());
            }
        }
    }
    out
}

/// `jpeg` with an Exif APP1 segment right after the start-of-image marker.
pub fn jpeg_with_exif(jpeg: &[u8], exif: &[u8]) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(exif);
    jpeg_with_segment(jpeg, 0xE1, &payload)
}

/// `jpeg` with an ICC profile APP2 segment (one chunk).
pub fn jpeg_with_icc(jpeg: &[u8], profile: &[u8]) -> Vec<u8> {
    let mut payload = b"ICC_PROFILE\0".to_vec();
    payload.extend([1, 1]);
    payload.extend(profile);
    jpeg_with_segment(jpeg, 0xE2, &payload)
}

fn jpeg_with_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, marker]);
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend(&jpeg[2..]);
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// A zlib stream of stored (uncompressed) deflate blocks.
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(65_535).peekable();
    if chunks.peek().is_none() {
        out.extend([1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        out.extend((c.len() as u16).to_le_bytes());
        out.extend((!(c.len() as u16)).to_le_bytes());
        out.extend(c);
    }
    out.extend(adler32(data).to_be_bytes());
    out
}

/// One PNG chunk: length, type, data, CRC.
pub fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(data);
    let mut crc_in = kind.to_vec();
    crc_in.extend(data);
    out.extend(crc32(&crc_in).to_be_bytes());
    out
}

/// `png` with `chunks` inserted right after its IHDR chunk.
pub fn png_with_chunks(png: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let ihdr_end = 8 + 12 + 13;
    let mut out = png[..ihdr_end].to_vec();
    for c in chunks {
        out.extend(c);
    }
    out.extend(&png[ihdr_end..]);
    out
}

/// An `iCCP` chunk holding `profile` under the name `test`.
pub fn png_icc_chunk(profile: &[u8]) -> Vec<u8> {
    let mut data = b"test\0\0".to_vec();
    data.extend(zlib_stored(profile));
    png_chunk(b"iCCP", &data)
}

/// An `eXIf` chunk.
pub fn png_exif_chunk(exif: &[u8]) -> Vec<u8> {
    png_chunk(b"eXIf", exif)
}

fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn riff_file(body: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32 + 4).to_le_bytes());
    out.extend(b"WEBP");
    out.extend(body);
    out
}

/// The `VP8L` chunk data of a lossless WebP file made by the encoder.
fn vp8l_payload(webp: &[u8]) -> Vec<u8> {
    let mut at = 12;
    while at + 8 <= webp.len() {
        let len = u32::from_le_bytes([webp[at + 4], webp[at + 5], webp[at + 6], webp[at + 7]]) as usize;
        if &webp[at..at + 4] == b"VP8L" {
            return webp[at + 8..at + 8 + len].to_vec();
        }
        at += 8 + len + (len & 1);
    }
    Vec::new()
}

fn u24(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// A lossless WebP in the extended format carrying the given ICC and Exif data.
pub fn webp_with_meta(webp: &[u8], w: u32, h: u32, icc: Option<&[u8]>, exif: Option<&[u8]>) -> Vec<u8> {
    let mut flags = 0u8;
    if icc.is_some() {
        flags |= 0x20;
    }
    if exif.is_some() {
        flags |= 0x08;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend(u24(w - 1));
    vp8x.extend(u24(h - 1));
    let mut body = riff_chunk(b"VP8X", &vp8x);
    if let Some(p) = icc {
        body.extend(riff_chunk(b"ICCP", p));
    }
    body.extend(riff_chunk(b"VP8L", &vp8l_payload(webp)));
    if let Some(e) = exif {
        body.extend(riff_chunk(b"EXIF", e));
    }
    riff_file(&body)
}

/// An animated lossless WebP from same-size frames, each shown for `ms` milliseconds.
pub fn webp_animated(frames: &[RgbaImage], ms: u32) -> Vec<u8> {
    let (w, h) = frames[0].dimensions();
    let mut vp8x = vec![0x02, 0, 0, 0];
    vp8x.extend(u24(w - 1));
    vp8x.extend(u24(h - 1));
    let mut body = riff_chunk(b"VP8X", &vp8x);
    body.extend(riff_chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for f in frames {
        let mut still = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut still)
            .encode(f.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .expect("encoding a fixture frame");
        let mut anmf = Vec::new();
        anmf.extend(u24(0));
        anmf.extend(u24(0));
        anmf.extend(u24(w - 1));
        anmf.extend(u24(h - 1));
        anmf.extend(u24(ms));
        anmf.push(0x02);
        anmf.extend(riff_chunk(b"VP8L", &vp8l_payload(&still)));
        body.extend(riff_chunk(b"ANMF", &anmf));
    }
    riff_file(&body)
}

/// An animated GIF from same-size frames, each shown for `ms` milliseconds.
pub fn gif_animated(frames: &[RgbaImage], ms: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 1);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite)
            .expect("gif repeat");
        for f in frames {
            let delay = image::Delay::from_numer_denom_ms(ms, 1);
            enc.encode_frame(image::Frame::from_parts(f.clone(), 0, 0, delay))
                .expect("encoding a fixture frame");
        }
    }
    out
}

/// A picture whose every pixel differs from its neighbours: a smooth diagonal
/// gradient with a coloured square, so crops, flips and resizes are visible.
pub fn scene(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let r = (x * 255 / w.max(1)) as u8;
        let g = (y * 255 / h.max(1)) as u8;
        let b = ((x + y) * 255 / (w + h).max(1)) as u8;
        if x >= w / 4 && x < w / 2 && y >= h / 4 && y < h / 2 {
            Rgba([220, 30, 30, 255])
        } else {
            Rgba([r, g, b, 255])
        }
    })
}

/// Pseudo-random noise from a fixed seed (xorshift64*), opaque.
pub fn noise(w: u32, h: u32, seed: u64) -> RgbaImage {
    let mut s = seed | 1;
    RgbaImage::from_fn(w, h, |_, _| {
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        let v = s.wrapping_mul(0x2545_F491_4F6C_DD1D);
        Rgba([(v >> 24) as u8, (v >> 32) as u8, (v >> 40) as u8, 255])
    })
}

/// A black-on-white QR code of `text`, `module` pixels per module, with a quiet zone.
pub fn qr(text: &str, module: u32) -> RgbaImage {
    let code = qrcode::QrCode::new(text.as_bytes()).expect("a QR code that fits");
    let n = code.width() as u32;
    let quiet = 4;
    let side = (n + 2 * quiet) * module;
    RgbaImage::from_fn(side, side, |x, y| {
        let (mx, my) = (x / module, y / module);
        let dark = mx >= quiet
            && my >= quiet
            && mx < n + quiet
            && my < n + quiet
            && code[((my - quiet) as usize, (mx - quiet) as usize)] == qrcode::Color::Dark;
        if dark { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    })
}

/// A barcode of `text` in `format`, rendered by the encoder at `w` x `h` pixels.
pub fn barcode(format: rxing::BarcodeFormat, text: &str, w: i32, h: i32) -> RgbaImage {
    use rxing::Writer as _;
    let matrix = rxing::MultiFormatWriter
        .encode(text, &format, w, h)
        .expect("encoding a fixture barcode");
    let (mw, mh) = (matrix.getWidth(), matrix.getHeight());
    RgbaImage::from_fn(mw, mh, |x, y| {
        if matrix.get(x, y) { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    })
}

/// A PNG of `img`, encoded by the image crate.
pub fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encoding a fixture");
    out
}

/// A JPEG of `img` at `quality`.
pub fn jpeg(img: &RgbaImage, quality: u8) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .expect("encoding a fixture");
    out
}

/// A lossless WebP of `img`.
pub fn webp(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .expect("encoding a fixture");
    out
}

/// `img` as any format the image crate writes.
pub fn encoded(img: &RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .to_rgb8()
        .write_to(&mut std::io::Cursor::new(&mut out), format)
        .expect("encoding a fixture");
    out
}

/// A PNG file that only claims a size: a valid signature, an IHDR for `w` x
/// `h` pixels at 8-bit RGB, a few bytes of image data and an IEND.
pub fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend(png_chunk(b"IHDR", &ihdr));
    out.extend(png_chunk(b"IDAT", &zlib_stored(&[0, 1, 2])));
    out.extend(png_chunk(b"IEND", &[]));
    out
}

/// A GIF that only claims a size, with no frames.
pub fn gif_claiming(w: u16, h: u16) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    out.extend([0, 0, 0, 0x3B]);
    out
}

/// A BMP whose header claims `w` x `h` 24-bit pixels and holds none.
pub fn bmp_claiming(w: i32, h: i32) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend(54u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0u8; 24]);
    out
}

/// A TIFF whose first directory claims a `w` x `h` 8-bit gray image with one tiny strip.
pub fn tiff_claiming(w: u32, h: u32) -> Vec<u8> {
    let entries: [(u16, u16, u32); 9] = [
        (256, 4, w),
        (257, 4, h),
        (258, 3, 8),
        (259, 3, 1),
        (262, 3, 1),
        (273, 4, 200),
        (277, 3, 1),
        (278, 4, h),
        (279, 4, 10),
    ];
    let mut out = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(1u32.to_le_bytes());
        out.extend(value.to_le_bytes());
    }
    out.extend([0u8; 4]);
    out.resize(210, 0);
    out
}

/// A WebP (extended format) that only claims a canvas of `w` x `h` pixels.
pub fn webp_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut vp8x = vec![0, 0, 0, 0];
    vp8x.extend(u24(w - 1));
    vp8x.extend(u24(h - 1));
    riff_file(&riff_chunk(b"VP8X", &vp8x))
}

/// A JPEG that only claims a size: SOI, a baseline SOF0 for `w` x `h` and EOI.
pub fn jpeg_claiming(w: u16, h: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8];
    out.extend(h.to_be_bytes());
    out.extend(w.to_be_bytes());
    out.extend([1, 1, 0x11, 0, 0xFF, 0xD9]);
    out
}
