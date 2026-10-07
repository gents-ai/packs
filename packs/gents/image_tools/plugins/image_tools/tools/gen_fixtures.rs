//! Writes the committed test fixtures into `tests/fixtures`, byte for byte the
//! same on every run (fixed seeds, no clocks). Run it from the plugin folder:
//! `cargo run --example gen_fixtures`. Large inputs (huge JPEGs, bombs that
//! need real data) are built by the tests at run time instead of committed.
#[path = "../source/fixtures.rs"]
mod fixtures;

use std::fs;
use std::path::{Path, PathBuf};

use fixtures as fx;
use image::{DynamicImage, Rgba, RgbaImage, imageops};

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().expect("a parent folder")).expect("creating a fixture folder");
    fs::write(&path, bytes).expect("writing a fixture");
}

/// The upright picture the eight orientation fixtures all decode to: asymmetric in both axes.
fn upright() -> RgbaImage {
    RgbaImage::from_fn(6, 4, |x, y| {
        Rgba([
            (x * 40 + 10) as u8,
            (y * 60 + 20) as u8,
            ((x + y * 6) * 10) as u8,
            255,
        ])
    })
}

/// The stored picture that an Exif orientation `n` turns back into `upright`,
/// derived with the image crate's own transforms.
fn stored_for(n: u8, u: &RgbaImage) -> RgbaImage {
    let d = DynamicImage::ImageRgba8(u.clone());
    let out = match n {
        1 => d,
        2 => d.fliph(),
        3 => d.rotate180(),
        4 => d.flipv(),
        5 => d.rotate90().fliph(),
        6 => d.rotate270(),
        7 => d.rotate270().fliph(),
        8 => d.rotate90(),
        _ => unreachable!("orientations are 1 to 8"),
    };
    out.to_rgba8()
}

/// Rotates by `deg` degrees around the centre onto a white page, nearest neighbour.
fn rotated(img: &RgbaImage, deg: f64) -> RgbaImage {
    let (w, h) = img.dimensions();
    let side = ((f64::from(w * w + h * h)).sqrt().ceil() as u32) + 2;
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    RgbaImage::from_fn(side, side, |x, y| {
        let (dx, dy) = (
            f64::from(x) - f64::from(side) / 2.0,
            f64::from(y) - f64::from(side) / 2.0,
        );
        let (sx, sy) = (dx * c + dy * s + cx, -dx * s + dy * c + cy);
        if sx >= 0.0 && sy >= 0.0 && sx < f64::from(w) && sy < f64::from(h) {
            *img.get_pixel(sx as u32, sy as u32)
        } else {
            Rgba([255, 255, 255, 255])
        }
    })
}

fn paint(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            img.put_pixel(xx, yy, Rgba(c));
        }
    }
}

fn main() {
    let root: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures"]
        .iter()
        .collect();
    let scene = fx::scene(32, 24);

    // One picture in every format.
    put(&root, "scene.png", &fx::png(&scene));
    put(&root, "scene.jpg", &fx::jpeg(&scene, 85));
    put(&root, "scene.webp", &fx::webp(&scene));
    put(
        &root,
        "scene.gif",
        &fx::encoded(&scene, image::ImageFormat::Gif),
    );
    put(
        &root,
        "scene.bmp",
        &fx::encoded(&scene, image::ImageFormat::Bmp),
    );
    put(
        &root,
        "scene.tif",
        &fx::encoded(&scene, image::ImageFormat::Tiff),
    );
    put(&root, "not_a_png.txt", &fx::png(&scene));

    // The eight Exif orientations, lossless, all decoding to one upright picture.
    let up = upright();
    put(&root, "upright.png", &fx::png(&up));
    for n in 1..=8u8 {
        let png = fx::png(&stored_for(n, &up));
        let tagged =
            fx::png_with_chunks(&png, &[fx::png_exif_chunk(&fx::exif_block(Some(n), None))]);
        put(&root, &format!("orient_{n}.png"), &tagged);
    }

    // Metadata: Exif with orientation and GPS, an ICC profile, in each container that carries them.
    let profile: Vec<u8> = (0..160u32).map(|i| (i * 7 + 3) as u8).collect();
    let exif = fx::exif_block(Some(6), Some((48.8566, 2.3522)));
    let photo = RgbaImage::from_fn(48, 32, |x, y| Rgba([(x * 5) as u8, (y * 7) as u8, 90, 255]));
    let jpg = fx::jpeg_with_icc(&fx::jpeg_with_exif(&fx::jpeg(&photo, 88), &exif), &profile);
    put(&root, "photo_meta.jpg", &jpg);
    put(&root, "photo_icc.tif", &fx::tiff_rgb(&photo, &profile));
    put(&root, "transparent.gif", &fx::gif_1x1(true));
    let png = fx::png_with_chunks(
        &fx::png(&photo),
        &[fx::png_icc_chunk(&profile), fx::png_exif_chunk(&exif)],
    );
    put(&root, "photo_meta.png", &png);
    put(
        &root,
        "photo_meta.webp",
        &fx::webp_with_meta(&fx::webp(&photo), 48, 32, Some(&profile), Some(&exif)),
    );
    put(&root, "photo_noexif.jpg", &fx::jpeg(&photo, 88));

    // Animation.
    let frames: Vec<RgbaImage> = (0..3u8)
        .map(|i| {
            RgbaImage::from_fn(16, 12, |x, _| {
                Rgba([i * 100, (x * 10) as u8, 200 - i * 50, 255])
            })
        })
        .collect();
    put(&root, "anim.gif", &fx::gif_animated(&frames, 40));
    put(&root, "anim.webp", &fx::webp_animated(&frames, 40));

    // Other colour types: 16-bit gray, gray with alpha, CMYK JPEG.
    let l16 = image::ImageBuffer::<image::Luma<u16>, _>::from_fn(8, 6, |x, y| {
        image::Luma([(x * 8000 + y * 1000) as u16])
    });
    let mut b = Vec::new();
    l16.write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
        .expect("gray16");
    put(&root, "gray16.png", &b);
    let la8 = image::ImageBuffer::<image::LumaA<u8>, _>::from_fn(8, 6, |x, y| {
        image::LumaA([(x * 30) as u8, (y * 40 + 50) as u8])
    });
    let mut b = Vec::new();
    la8.write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
        .expect("gray alpha");
    put(&root, "la8.png", &b);
    let mut cmyk = Vec::new();
    for y in 0..8 {
        for x in 0..8 {
            cmyk.extend([
                if x < 4 { 255u8 } else { 0 },
                if y < 4 { 255u8 } else { 0 },
                0,
                0,
            ]);
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 95)
        .encode(&cmyk, 8, 8, jpeg_encoder::ColorType::Cmyk)
        .expect("cmyk");
    put(&root, "cmyk.jpg", &out);

    // Diff and hash pairs.
    let base = fx::scene(64, 48);
    let mut changed = base.clone();
    paint(&mut changed, 10, 8, 12, 9, [0, 0, 0, 255]);
    paint(&mut changed, 50, 40, 5, 5, [255, 255, 255, 255]);
    put(&root, "diff_a.png", &fx::png(&base));
    put(&root, "diff_b.png", &fx::png(&changed));
    put(&root, "diff_wide.png", &fx::png(&fx::scene(80, 48)));
    let small = imageops::resize(&base, 32, 24, imageops::FilterType::Lanczos3);
    put(&root, "dup_small.png", &fx::png(&small));
    put(&root, "dup_noise.png", &fx::png(&fx::noise(64, 48, 11)));
    put(&root, "noise100.png", &fx::png(&fx::noise(100, 100, 21)));

    // Codes with known payloads.
    let codes = root.join("codes");
    let qr = fx::qr("https://example.org/pack?id=42", 4);
    put(&codes, "qr_plain.png", &fx::png(&qr));
    put(&codes, "qr_rot90.png", &fx::png(&imageops::rotate90(&qr)));
    put(&codes, "qr_rot30.png", &fx::png(&rotated(&qr, 30.0)));
    let mut faint = qr.clone();
    for p in faint.pixels_mut() {
        let v = 110 + (u32::from(p[0]) * 40 / 255) as u8;
        *p = Rgba([v, v, v, 255]);
    }
    put(&codes, "qr_lowcontrast.png", &fx::png(&faint));
    let mut inverted = qr.clone();
    for p in inverted.pixels_mut() {
        *p = Rgba([255 - p[0], 255 - p[1], 255 - p[2], 255]);
    }
    put(&codes, "qr_inverted.png", &fx::png(&inverted));
    let mut damaged = fx::qr("DAMAGED-BUT-READABLE-0123456789", 5);
    let (dw, dh) = damaged.dimensions();
    paint(
        &mut damaged,
        dw / 2,
        dh / 2,
        dw / 8,
        dh / 12,
        [0, 0, 0, 255],
    );
    put(&codes, "qr_damaged.png", &fx::png(&damaged));
    put(
        &codes,
        "code128.png",
        &fx::png(&fx::barcode(
            rxing::BarcodeFormat::CODE_128,
            "ABC-12345",
            280,
            80,
        )),
    );
    put(
        &codes,
        "ean13.png",
        &fx::png(&fx::barcode(
            rxing::BarcodeFormat::EAN_13,
            "5901234123457",
            240,
            80,
        )),
    );
    put(
        &codes,
        "datamatrix.png",
        &fx::png(&fx::barcode(
            rxing::BarcodeFormat::DATA_MATRIX,
            "DM-PAYLOAD-77",
            100,
            100,
        )),
    );
    let mut page = RgbaImage::from_pixel(260, 120, Rgba([255, 255, 255, 255]));
    imageops::overlay(&mut page, &fx::qr("LEFT", 3), 4, 4);
    imageops::overlay(&mut page, &fx::qr("RIGHT", 3), 140, 4);
    put(&codes, "two_codes.png", &fx::png(&page));
    put(&codes, "no_code.png", &fx::png(&fx::noise(96, 96, 5)));

    // Tiling source and folders for batch and montage.
    put(&root, "tile_src.png", &fx::png(&fx::scene(150, 100)));
    let batch = root.join("batch");
    put(&batch, "a.png", &fx::png(&fx::scene(24, 18)));
    put(&batch, "b.jpg", &fx::jpeg(&fx::scene(30, 20), 80));
    put(&batch, "sub/c.png", &fx::png(&fx::noise(16, 16, 3)));
    put(&batch, "notes.txt", b"not an image\n");
    // Four pictures that do not compress, so a small page holds about one of them.
    for i in 0..4u64 {
        put(
            &root.join("pages"),
            &format!("n{i}.png"),
            &fx::png(&fx::noise(48, 48, 40 + i)),
        );
    }
    // Sources the write cases resize with a suffix; the files those cases write are git-ignored.
    for (i, name) in ["x.png", "y.png"].iter().enumerate() {
        put(
            &root.join("suffix"),
            name,
            &fx::png(&fx::scene(20 + 4 * i as u32, 12)),
        );
    }
    // One image more than a call attaches, for saving with parts across that cap.
    for i in 0..21u32 {
        put(
            &root.join("parts"),
            &format!("p{i:02}.png"),
            &fx::png(&fx::scene(8 + i % 3, 6)),
        );
    }
    put(
        &root,
        ".gitignore",
        b"out/\ntiles/\nsuffix/*_s.png\nparts/*_s.png\n",
    );
    let montage = root.join("montage");
    for (i, c) in [[220u8, 40, 40, 255], [40, 180, 60, 255], [50, 80, 220, 255]]
        .iter()
        .enumerate()
    {
        put(
            &montage,
            &format!("{}.png", ["red", "green", "blue"][i]),
            &fx::png(&RgbaImage::from_pixel(20 + 6 * i as u32, 14, Rgba(*c))),
        );
    }
    put(
        &montage,
        "broken.png",
        b"\x89PNG\r\n\x1a\nthis one is cut short",
    );

    // A folder tree 17 levels deep: the listing reads 16 levels, so `lost.png` is reported, not read.
    let deep = root.join("deep");
    let tiny = fx::png(&fx::scene(8, 6));
    put(&deep, "top.png", &tiny);
    let levels: PathBuf = (0..16).map(|i| format!("d{i}")).collect();
    put(&deep.join(&levels), "ok_level16.png", &tiny);
    let levels = levels.join("d16");
    put(&deep.join(&levels), "lost.png", &tiny);

    // Hostile files: each must give one sentence, never a crash.
    let h = root.join("hostile");
    let full = fx::png(&fx::scene(48, 36));
    put(&h, "truncated.png", &full[..full.len() / 2]);
    let jfull = fx::jpeg(&fx::scene(48, 36), 85);
    put(&h, "truncated.jpg", &jfull[..jfull.len() / 2]);
    // Cut halfway through the scan data of a 128x128 baseline JPEG: the headers are whole.
    let sfull = fx::jpeg(&fx::scene(128, 128), 85);
    put(&h, "truncated_scan.jpg", &sfull[..sfull.len() / 2]);
    put(&h, "empty.png", b"");
    put(
        &h,
        "text.png",
        b"this is plain text pretending to be a PNG\n",
    );
    put(&h, "zero_size.png", &fx::png_claiming(0, 0));
    put(&h, "bomb.png", &fx::png_claiming(100_000, 100_000));
    put(&h, "bomb_wide.png", &fx::png_claiming(40_000, 2));
    put(&h, "bomb.gif", &fx::gif_claiming(65_535, 65_535));
    put(&h, "bomb.bmp", &fx::bmp_claiming(100_000, 100_000));
    put(&h, "bomb.tif", &fx::tiff_claiming(100_000, 100_000));
    put(&h, "bomb.webp", &fx::webp_claiming(100_000, 100_000));
    put(&h, "bomb.jpg", &fx::jpeg_claiming(65_535, 65_535));
    put(&h, "ok.png", &fx::png(&fx::scene(8, 6)));
}
