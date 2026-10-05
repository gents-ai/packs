//! Tests for reading images: headers, pixels, metadata, frames, and refusals.
use super::decode::*;
use super::fixtures as fx;
use super::model::{Format, Img};
use super::src::{Data, Source};
use std::sync::Arc;

fn mem(bytes: Vec<u8>) -> Source {
    Source {
        name: "t".into(),
        data: Data::Mem(Arc::new(bytes)),
    }
}

fn pixels(bytes: Vec<u8>) -> Img {
    let s = mem(bytes);
    let h = header(&s).unwrap();
    decode(&s, &h, &LoadOpts::default()).unwrap().img
}

fn rgba(img: &image::RgbaImage) -> Img {
    Img::from_raw(img.width(), img.height(), img.as_raw().clone()).unwrap()
}

#[test]
fn lossless_formats_decode_to_the_exact_pixels() {
    let img = fx::scene(23, 17);
    let want = rgba(&img);
    assert_eq!(pixels(fx::png(&img)), want);
    assert_eq!(pixels(fx::webp(&img)), want);
    for f in [image::ImageFormat::Bmp, image::ImageFormat::Tiff] {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgba8();
        assert_eq!(pixels(fx::encoded(&rgb, f)), want, "{f:?}");
    }
}

#[test]
fn headers_report_format_size_and_colour_without_decoding() {
    let img = fx::scene(40, 30);
    for (bytes, format) in [
        (fx::png(&img), Format::Png),
        (fx::jpeg(&img, 80), Format::Jpeg),
        (fx::webp(&img), Format::Webp),
        (fx::encoded(&img, image::ImageFormat::Bmp), Format::Bmp),
        (fx::encoded(&img, image::ImageFormat::Tiff), Format::Tiff),
        (fx::encoded(&img, image::ImageFormat::Gif), Format::Gif),
    ] {
        let len = bytes.len() as u64;
        let h = header(&mem(bytes)).unwrap();
        assert_eq!(h.format, format);
        assert_eq!((h.width, h.height), (40, 30), "{format}");
        assert_eq!(h.bytes, len);
        assert_eq!(h.frames, 1, "{format}");
        assert!(h.exif.is_none() && h.icc.is_none() && !h.xmp, "{format}");
    }
    let h = header(&mem(fx::png(&img))).unwrap();
    assert_eq!(
        (h.color.as_str(), h.bit_depth, h.has_alpha),
        ("rgba8", 8, true)
    );
}

#[test]
fn gray_sixteen_bit_and_alpha_colour_types_are_reported_and_decoded() {
    let l16 = image::ImageBuffer::<image::Luma<u16>, _>::from_fn(4, 3, |x, y| {
        image::Luma([(x * 20000 + y * 5000) as u16])
    });
    let mut png = Vec::new();
    l16.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let s = mem(png);
    let h = header(&s).unwrap();
    assert_eq!(
        (h.color.as_str(), h.bit_depth, h.has_alpha),
        ("l16", 16, false)
    );
    let d = decode(&s, &h, &LoadOpts::default()).unwrap().img;
    assert_eq!((d.w, d.h), (4, 3));
    // 16-bit 20000 maps to 8-bit 78 (20000 / 257 rounded); the pixel is gray and opaque.
    assert_eq!(d.get(1, 0), [78, 78, 78, 255]);
    assert_eq!(d.get(0, 0), [0, 0, 0, 255]);

    let la8 = image::ImageBuffer::<image::LumaA<u8>, _>::from_fn(2, 2, |x, _| {
        image::LumaA([100, if x == 0 { 255 } else { 128 }])
    });
    let mut png = Vec::new();
    la8.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let s = mem(png);
    let h = header(&s).unwrap();
    assert_eq!((h.color.as_str(), h.has_alpha), ("la8", true));
    let d = decode(&s, &h, &LoadOpts::default()).unwrap().img;
    assert_eq!(d.get(1, 0), [100, 100, 100, 128]);
}

#[test]
fn orientation_and_gps_come_from_the_exif_block_of_each_container() {
    let img = fx::scene(16, 12);
    let exif = fx::exif_block(Some(6), Some((48.8566, -2.3522)));
    let files = [
        ("jpeg", fx::jpeg_with_exif(&fx::jpeg(&img, 80), &exif)),
        (
            "png",
            fx::png_with_chunks(&fx::png(&img), &[fx::png_exif_chunk(&exif)]),
        ),
        (
            "webp",
            fx::webp_with_meta(&fx::webp(&img), 16, 12, None, Some(&exif)),
        ),
    ];
    for (name, bytes) in files {
        let h = header(&mem(bytes)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let e = h.exif.unwrap_or_else(|| panic!("{name}: no exif"));
        assert_eq!(e.orientation, Some(6), "{name}");
        assert!(e.has_gps, "{name}");
        let p = e.position.unwrap();
        assert_eq!((p.lat, p.lon), (48.8566, -2.3522), "{name}");
        assert_eq!(h.orientation(), 6);
    }
}

#[test]
fn an_exif_block_without_gps_or_orientation_says_so() {
    let img = fx::scene(16, 12);
    let h = header(&mem(fx::jpeg_with_exif(
        &fx::jpeg(&img, 80),
        &fx::exif_block(None, None),
    )))
    .unwrap();
    let e = h.exif.unwrap();
    assert_eq!((e.orientation, e.has_gps, e.position), (None, false, None));
    assert_eq!(h.orientation(), 1);
    let h = header(&mem(fx::jpeg_with_exif(
        &fx::jpeg(&img, 80),
        &fx::exif_block(Some(3), None),
    )))
    .unwrap();
    assert_eq!(h.orientation(), 3);
    assert!(!h.exif.unwrap().has_gps);
}

#[test]
fn every_orientation_value_round_trips_and_out_of_range_is_ignored() {
    let img = fx::scene(16, 12);
    for n in 1..=8u8 {
        let h = header(&mem(fx::jpeg_with_exif(
            &fx::jpeg(&img, 80),
            &fx::exif_block(Some(n), None),
        )))
        .unwrap();
        assert_eq!(h.orientation(), n);
    }
    for n in [0u8, 9, 200] {
        let h = header(&mem(fx::jpeg_with_exif(
            &fx::jpeg(&img, 80),
            &fx::exif_block(Some(n), None),
        )))
        .unwrap();
        assert_eq!(
            h.orientation(),
            1,
            "orientation {n} is not a valid Exif value"
        );
    }
}

#[test]
fn a_tiff_file_reports_its_own_first_directory() {
    let img = fx::scene(10, 8);
    let h = header(&mem(fx::encoded(&img, image::ImageFormat::Tiff))).unwrap();
    // The encoder writes no orientation tag.
    assert_eq!(h.orientation(), 1);
}

#[test]
fn icc_profiles_are_found_with_their_exact_size() {
    let img = fx::scene(16, 12);
    let profile: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
    let files = [
        ("jpeg", fx::jpeg_with_icc(&fx::jpeg(&img, 80), &profile)),
        (
            "png",
            fx::png_with_chunks(&fx::png(&img), &[fx::png_icc_chunk(&profile)]),
        ),
        (
            "webp",
            fx::webp_with_meta(&fx::webp(&img), 16, 12, Some(&profile), None),
        ),
    ];
    for (name, bytes) in files {
        let h = header(&mem(bytes)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(h.icc.as_deref(), Some(profile.as_slice()), "{name}");
    }
}

#[test]
fn animated_gif_and_webp_count_frames_and_decode_the_chosen_one() {
    let frames: Vec<_> = (0..3)
        .map(|i| image::RgbaImage::from_pixel(8, 6, image::Rgba([i * 100, 50, 200 - i * 50, 255])))
        .collect();
    for (name, bytes) in [
        ("gif", fx::gif_animated(&frames, 40)),
        ("webp", fx::webp_animated(&frames, 40)),
    ] {
        let s = mem(bytes);
        let h = header(&s).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(h.frames, 3, "{name}");
        for (i, f) in frames.iter().enumerate() {
            let d = decode(
                &s,
                &h,
                &LoadOpts {
                    frame: i as u32,
                    downscale: None,
                },
            )
            .unwrap()
            .img;
            let want = f.get_pixel(0, 0).0;
            let got = d.get(3, 3);
            let close = |a: u8, b: u8| a.abs_diff(b) <= 8;
            assert!(
                close(got[0], want[0]) && close(got[1], want[1]) && close(got[2], want[2]),
                "{name} frame {i}: {got:?} vs {want:?}"
            );
        }
        let e = decode(
            &s,
            &h,
            &LoadOpts {
                frame: 3,
                downscale: None,
            },
        )
        .err()
        .unwrap();
        assert!(
            e.contains("3 frames") && e.contains("0 to 2"),
            "{name}: {e}"
        );
    }
}

#[test]
fn a_still_image_refuses_a_frame_other_than_zero() {
    let s = mem(fx::png(&fx::scene(4, 4)));
    let h = header(&s).unwrap();
    let e = decode(
        &s,
        &h,
        &LoadOpts {
            frame: 1,
            downscale: None,
        },
    )
    .err()
    .unwrap();
    assert!(e.contains("one frame"), "{e}");
}

#[test]
fn the_format_comes_from_the_content_not_the_name() {
    let s = Source {
        name: "photo.txt".into(),
        data: Data::Mem(Arc::new(fx::png(&fx::scene(5, 5)))),
    };
    assert_eq!(header(&s).unwrap().format, Format::Png);
    for junk in [
        &b""[..],
        b"hello world, not an image",
        b"%PDF-1.7 ....",
        &[0u8; 64],
    ] {
        let e = header(&mem(junk.to_vec())).err().unwrap();
        assert!(e.contains("not a PNG, JPEG"), "{e}");
    }
}

#[test]
fn a_header_claiming_a_huge_size_is_refused_before_any_allocation() {
    let started = std::time::Instant::now();
    let cases = [
        ("png", fx::png_claiming(100_000, 100_000)),
        ("png-wide", fx::png_claiming(40_000, 2)),
        ("gif", fx::gif_claiming(65_535, 65_535)),
        ("bmp", fx::bmp_claiming(100_000, 100_000)),
        ("tiff", fx::tiff_claiming(100_000, 100_000)),
        ("webp", fx::webp_claiming(100_000, 100_000)),
        ("jpeg", fx::jpeg_claiming(65_535, 65_535)),
        (
            "png-under-side-cap-over-pixel-cap",
            fx::png_claiming(30_000, 30_000),
        ),
    ];
    for (name, bytes) in cases {
        let s = mem(bytes);
        let outcome = header(&s).and_then(|h| decode(&s, &h, &LoadOpts::default()).map(|_| ()));
        let e = outcome
            .err()
            .unwrap_or_else(|| panic!("{name} was not refused"));
        assert!(
            !e.is_empty() && !e.contains('\n') && !e.to_lowercase().contains("panick"),
            "{name}: {e}"
        );
    }
    assert!(
        started.elapsed().as_secs() < 5,
        "the bombs are refused from the header alone"
    );
}

#[test]
fn the_size_limit_sentence_names_the_limit() {
    let e = header(&mem(fx::png_claiming(30_000, 30_000)))
        .err()
        .unwrap();
    assert!(e.contains("limit"), "{e}");
}

#[test]
fn truncated_files_fail_with_one_sentence_per_format() {
    let img = fx::scene(64, 48);
    let files = [
        ("png", fx::png(&img)),
        ("jpeg", fx::jpeg(&img, 85)),
        ("webp", fx::webp(&img)),
        ("bmp", fx::encoded(&img, image::ImageFormat::Bmp)),
        ("tiff", fx::encoded(&img, image::ImageFormat::Tiff)),
        ("gif", fx::encoded(&img, image::ImageFormat::Gif)),
    ];
    for (name, bytes) in files {
        let mut failures = 0;
        for cut in [bytes.len() / 2, bytes.len() * 9 / 10, bytes.len() - 1] {
            let s = mem(bytes[..cut].to_vec());
            let r = header(&s).and_then(|h| decode(&s, &h, &LoadOpts::default()).map(|_| ()));
            if let Err(e) = r {
                failures += 1;
                assert!(!e.is_empty() && !e.contains('\n'), "{name}: {e}");
            }
        }
        assert!(failures >= 1, "{name}: a half file must fail to decode");
    }
}

#[test]
fn view_target_fits_a_square_box_and_never_enlarges() {
    assert_eq!(view_target(4000, 3000, 1000), (1000, 750));
    assert_eq!(view_target(3000, 4000, 1000), (750, 1000));
    assert_eq!(view_target(100, 80, 1000), (100, 80));
}

#[test]
fn a_jpeg_decoded_at_a_fraction_matches_the_shrunk_full_decode() {
    let img = fx::scene(800, 480);
    let s = mem(fx::jpeg(&img, 90));
    let h = header(&s).unwrap();
    let small = decode(
        &s,
        &h,
        &LoadOpts {
            frame: 0,
            downscale: Some((100, 60)),
        },
    )
    .unwrap();
    assert_eq!(small.scale_denominator, Some(8));
    assert_eq!((small.img.w, small.img.h), (100, 60));
    let full = decode(&s, &h, &LoadOpts::default()).unwrap();
    assert_eq!(full.scale_denominator, None);
    assert_eq!((full.img.w, full.img.h), (800, 480));
    // Compare with a box average of the full decode: both are 8x8 block means.
    for (x, y) in [(5u32, 5u32), (50, 30), (90, 55)] {
        let mut sum = [0u32; 3];
        for dy in 0..8 {
            for dx in 0..8 {
                let p = full.img.get(x * 8 + dx, y * 8 + dy);
                for c in 0..3 {
                    sum[c] += u32::from(p[c]);
                }
            }
        }
        let got = small.img.get(x, y);
        for c in 0..3 {
            let mean = (sum[c] / 64) as i32;
            assert!(
                (i32::from(got[c]) - mean).abs() <= 6,
                "({x},{y}) channel {c}: {} vs {mean}",
                got[c]
            );
        }
    }
}

#[test]
fn a_jpeg_that_needs_full_size_is_decoded_normally() {
    let s = mem(fx::jpeg(&fx::scene(200, 100), 90));
    let h = header(&s).unwrap();
    let d = decode(
        &s,
        &h,
        &LoadOpts {
            frame: 0,
            downscale: Some((200, 100)),
        },
    )
    .unwrap();
    assert_eq!(d.scale_denominator, None);
    assert_eq!((d.img.w, d.img.h), (200, 100));
    let d = decode(
        &s,
        &h,
        &LoadOpts {
            frame: 0,
            downscale: Some((150, 90)),
        },
    )
    .unwrap();
    assert_eq!(
        d.scale_denominator, None,
        "1/2 would give 100x50, too small"
    );
}

#[test]
fn a_cmyk_jpeg_is_converted_to_rgb() {
    // Pure cyan ink on every pixel: red absorbed, green and blue reflected.
    let mut data = Vec::new();
    for _ in 0..64 {
        data.extend([255u8, 0, 0, 0]);
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 95)
        .encode(&data, 8, 8, jpeg_encoder::ColorType::Cmyk)
        .unwrap();
    let img = pixels(out);
    let p = img.get(4, 4);
    assert!(
        p[0] < 40 && p[1] > 215 && p[2] > 215 && p[3] == 255,
        "{p:?}"
    );
}

#[test]
fn a_later_gif_frame_is_placed_on_the_full_canvas_by_the_files_own_rules() {
    use image::{Delay, Frame, Rgba, RgbaImage};
    // Frame 0 is all red and frame 1 covers only the top-left 4x4 in blue. The encoder marks frame 0 to be
    // cleared before the next one, so frame 1 is blue in that corner and transparent elsewhere: the decoder
    // follows the file's own disposal rules and always returns the whole canvas.
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        let f1 = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let f2 = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 255, 255]));
        enc.encode_frame(Frame::from_parts(
            f1,
            0,
            0,
            Delay::from_numer_denom_ms(40, 1),
        ))
        .unwrap();
        enc.encode_frame(Frame::from_parts(
            f2,
            0,
            0,
            Delay::from_numer_denom_ms(40, 1),
        ))
        .unwrap();
    }
    let s = mem(out);
    let h = header(&s).unwrap();
    assert_eq!(h.frames, 2);
    let d = decode(
        &s,
        &h,
        &LoadOpts {
            frame: 1,
            downscale: None,
        },
    )
    .unwrap()
    .img;
    assert_eq!((d.w, d.h), (8, 8));
    assert_eq!(d.get(1, 1), [0, 0, 255, 255]);
    assert_eq!(
        d.get(5, 5),
        [0, 0, 0, 0],
        "outside the small frame the cleared canvas shows"
    );
}
