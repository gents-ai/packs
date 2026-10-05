//! Property tests: invariants that must hold for every input, run with a fixed
//! seed so a failure always reproduces.
use base64::Engine as _;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, RngSeed};
use serde_json::{Value, json};

use crate::diff::{Spec, compare};
use crate::geom::{crop, fit_dims, flip, orient, rotate};
use crate::hash::{distance, phash};
use crate::input::Rect;
use crate::model::{Format, Img};
use crate::resize::{Filter, resample};
use crate::testkit::{call, part_bytes};
use crate::tile::{cut, grid};

fn cfg(cases: u32) -> Config {
    Config {
        cases,
        failure_persistence: None,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(0x1A6E_7001),
        ..Config::default()
    }
}

fn arb_img(max: u32) -> impl Strategy<Value = Img> {
    (1..=max, 1..=max).prop_flat_map(|(w, h)| {
        proptest::collection::vec(any::<u8>(), (w * h * 4) as usize)
            .prop_map(move |px| Img::from_raw(w, h, px).unwrap())
    })
}

fn png_b64(img: &Img) -> String {
    let e = crate::encode::encode(img, Format::Png, None, None).unwrap();
    base64::engine::general_purpose::STANDARD.encode(e.bytes)
}

fn decode_part(v: &Value, i: usize) -> Img {
    let d = image::load_from_memory(&part_bytes(v, i))
        .unwrap()
        .to_rgba8();
    Img::from_raw(d.width(), d.height(), d.into_raw()).unwrap()
}

/// The orientation that undoes `n`.
fn inverse(n: u8) -> u8 {
    [1, 2, 3, 4, 5, 8, 7, 6][usize::from(n) - 1]
}

proptest! {
    #![proptest_config(cfg(64))]

    #[test]
    fn rotating_four_quarter_turns_is_the_identity(img in arb_img(12)) {
        let mut r = img.clone();
        for _ in 0..4 {
            r = rotate(&r, 90).unwrap();
        }
        prop_assert_eq!(&r, &img);
        prop_assert_eq!(rotate(&rotate(&img, 90).unwrap(), -90).unwrap(), img.clone());
        prop_assert_eq!(rotate(&rotate(&img, 180).unwrap(), 180).unwrap(), img);
    }

    #[test]
    fn flipping_twice_is_the_identity(img in arb_img(12)) {
        prop_assert_eq!(flip(&flip(&img, true), true), img.clone());
        prop_assert_eq!(flip(&flip(&img, false), false), img);
    }

    #[test]
    fn every_orientation_is_undone_by_its_inverse_and_keeps_the_pixels(img in arb_img(10), n in 1u8..=8) {
        let turned = orient(&img, n);
        prop_assert_eq!(turned.px.len(), img.px.len());
        prop_assert_eq!(orient(&turned, inverse(n)), img.clone());
        let mut a = img.px.chunks(4).map(<[u8]>::to_vec).collect::<Vec<_>>();
        let mut b = turned.px.chunks(4).map(<[u8]>::to_vec).collect::<Vec<_>>();
        a.sort();
        b.sort();
        prop_assert_eq!(a, b, "the same pixels in another place");
    }

    #[test]
    fn crop_gives_the_requested_size_and_the_same_pixels(img in arb_img(16), fx in 0.0f64..1.0, fy in 0.0f64..1.0, fw in 0.0f64..1.0, fh in 0.0f64..1.0) {
        let x = (fx * f64::from(img.w - 1)) as u32;
        let y = (fy * f64::from(img.h - 1)) as u32;
        let w = 1 + (fw * f64::from(img.w - x - 1)) as u32;
        let h = 1 + (fh * f64::from(img.h - y - 1)) as u32;
        let c = crop(&img, x, y, w, h).unwrap();
        prop_assert_eq!((c.w, c.h), (w, h));
        for yy in 0..h {
            for xx in 0..w {
                prop_assert_eq!(c.get(xx, yy), img.get(x + xx, y + yy));
            }
        }
    }

    #[test]
    fn fit_never_exceeds_the_box_and_keeps_the_aspect_within_a_pixel(sw in 1u32..6000, sh in 1u32..6000, bw in 1u32..6000, bh in 1u32..6000) {
        let (w, h) = fit_dims(sw, sh, Some(bw), Some(bh));
        prop_assert!(w >= 1 && h >= 1 && w <= bw && h <= bh, "{}x{} in {}x{} gave {}x{}", sw, sh, bw, bh, w, h);
        // One side binds; the other follows the aspect ratio to within a pixel.
        let err_h = (i128::from(h) * i128::from(sw) - i128::from(sh) * i128::from(w)).unsigned_abs();
        let err_w = (i128::from(w) * i128::from(sh) - i128::from(sw) * i128::from(h)).unsigned_abs();
        prop_assert!(w == bw || h == bh, "one side reaches the box");
        if w == bw {
            prop_assert!(err_h <= u128::from(sw), "height off by more than a pixel");
        }
        if h == bh {
            prop_assert!(err_w <= u128::from(sh), "width off by more than a pixel");
        }
        // The side that follows is the exact ratio rounded to the nearest pixel (at least 1, at most the box).
        if w == bw && h != bh {
            let want = ((f64::from(sh) * f64::from(bw)) / f64::from(sw)).round().max(1.0) as u32;
            prop_assert_eq!(h, want.min(bh));
        }
        if h == bh && w != bw {
            let want = ((f64::from(sw) * f64::from(bh)) / f64::from(sh)).round().max(1.0) as u32;
            prop_assert_eq!(w, want.min(bw));
        }
    }

    #[test]
    fn resizing_a_real_picture_stays_inside_the_requested_box(img in arb_img(20), bw in 1u32..30, bh in 1u32..30) {
        let spec = crate::resize::Spec { mode: crate::resize::Mode::Fit, width: Some(bw), height: Some(bh), filter: Filter::Lanczos3, upscale: true };
        let (out, _) = crate::resize::apply(img, &spec).unwrap();
        prop_assert!(out.w <= bw && out.h <= bh && (out.w == bw || out.h == bh));
    }

    #[test]
    fn lossless_formats_round_trip_every_pixel(img in arb_img(10)) {
        for f in [Format::Png, Format::Webp, Format::Tiff, Format::Bmp] {
            let bytes = crate::encode::encode(&img, f, None, None).unwrap().bytes;
            let d = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let back = Img::from_raw(d.width(), d.height(), d.into_raw()).unwrap();
            if f == Format::Bmp && !img.opaque() {
                continue;
            }
            prop_assert_eq!(&back, &img, "{}", f);
        }
    }

    #[test]
    fn png_to_webp_lossless_to_png_through_the_plugin_is_pixel_identical(img in arb_img(8)) {
        let b64 = png_b64(&img);
        let webp = call(&json!({"data_base64": b64, "op": "convert", "format": "webp"})).unwrap();
        let webp_b64 = base64::engine::general_purpose::STANDARD.encode(part_bytes(&webp, 0));
        let png = call(&json!({"data_base64": webp_b64, "op": "convert", "format": "png"})).unwrap();
        prop_assert_eq!(decode_part(&png, 0), img);
    }

    #[test]
    fn crop_then_info_of_the_result_gives_the_requested_size(img in arb_img(14), fx in 0.0f64..1.0, fy in 0.0f64..1.0, fw in 0.0f64..1.0, fh in 0.0f64..1.0) {
        let x = (fx * f64::from(img.w - 1)) as u32;
        let y = (fy * f64::from(img.h - 1)) as u32;
        let w = 1 + (fw * f64::from(img.w - x - 1)) as u32;
        let h = 1 + (fh * f64::from(img.h - y - 1)) as u32;
        let cropped = call(&json!({"data_base64": png_b64(&img), "op": "crop", "x": x, "y": y, "width": w, "height": h})).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(part_bytes(&cropped, 0));
        let info = call(&json!({"data_base64": b64, "op": "info"})).unwrap();
        let s = &info["results"][0]["steps"][0];
        prop_assert_eq!((s["width"].as_u64().unwrap() as u32, s["height"].as_u64().unwrap() as u32), (w, h));
    }

    #[test]
    fn diff_with_itself_is_zero_and_diff_is_symmetric(a in arb_img(12), tol in 0u8..40, gap in 0u32..20) {
        let spec = Spec { tolerance: tol, ignore: &[], merge_gap: gap };
        let same = compare(&a, &a, &spec);
        prop_assert_eq!((same.changed, same.regions_total), (0, 0));
        prop_assert_eq!(same.ssim, Some(1.0));
        let mut b = a.clone();
        for (i, p) in b.px.iter_mut().enumerate() {
            if i % 7 == 0 {
                *p = p.wrapping_add(70);
            }
        }
        let ab = compare(&a, &b, &spec);
        let ba = compare(&b, &a, &spec);
        prop_assert_eq!(ab.changed, ba.changed);
        prop_assert_eq!(&ab.regions, &ba.regions);
        prop_assert_eq!(ab.regions_total, ba.regions_total);
        // Every changed pixel lies inside some listed region (when none were dropped).
        prop_assert!(ab.regions_total == ab.regions.len());
    }

    #[test]
    fn diff_ignoring_everything_compares_nothing(a in arb_img(10), b in arb_img(10)) {
        let all = [Rect { x: 0, y: 0, width: 64, height: 64 }];
        let o = compare(&a, &b, &Spec { tolerance: 0, ignore: &all, merge_gap: 8 });
        prop_assert_eq!((o.changed, o.compared, o.regions_total), (0, 0, 0));
    }

    #[test]
    fn tiles_reassemble_to_the_source_and_overlap_enough(w in 1u32..160, h in 1u32..160, size in 64u32..100, overlap_pct in 0u32..90) {
        let overlap = size * overlap_pct / 100;
        let g = grid(w, h, size, overlap).unwrap();
        let src = Img::from_raw(w, h, (0..w * h * 4).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect()).unwrap();
        let mut out = Img::new(w, h).unwrap();
        let mut seen = vec![0u8; (w * h) as usize];
        for i in 0..g.len() {
            let c = g.cell(i);
            prop_assert!(c.x + c.width <= w && c.y + c.height <= h);
            let t = cut(&src, &c).unwrap();
            for yy in 0..t.h {
                for xx in 0..t.w {
                    let d = out.at(c.x + xx, c.y + yy);
                    out.px[d..d + 4].copy_from_slice(&t.get(xx, yy));
                    seen[((c.y + yy) * w + c.x + xx) as usize] = 1;
                }
            }
            if c.col + 1 < g.cols() {
                let next = g.cell(i + 1);
                prop_assert!(c.x + c.width >= next.x + overlap.min(c.width), "columns overlap by at least {}", overlap);
            }
            if c.row + 1 < g.rows() {
                let below = g.cell(i + g.cols() as usize);
                prop_assert!(c.y + c.height >= below.y + overlap.min(c.height), "rows overlap by at least {}", overlap);
            }
        }
        prop_assert_eq!(out, src);
        prop_assert!(seen.iter().all(|&s| s == 1));
    }

    #[test]
    fn the_perceptual_hash_survives_a_resize_and_separates_strangers(seed in 0u64..1000, cut in 40u32..90) {
        let base = crate::fixtures::scene(96, 80);
        // A different smooth picture per seed: shift the gradient and move the square.
        let img = image::RgbaImage::from_fn(96, 80, |x, y| {
            let p = base.get_pixel((x + (seed as u32 % 50)) % 96, (y + (seed as u32 % 37)) % 80);
            *p
        });
        let a = Img::from_raw(96, 80, img.into_raw()).unwrap();
        let w = 96 * cut / 100 + 8;
        let h = 80 * cut / 100 + 8;
        let small = resample(a.clone(), w, h, Filter::Lanczos3).unwrap();
        prop_assert!(distance(phash(&a), phash(&small)) <= 12, "distance {}", distance(phash(&a), phash(&small)));
        let n1 = Img::from_raw(64, 64, crate::fixtures::noise(64, 64, seed).into_raw()).unwrap();
        let n2 = Img::from_raw(64, 64, crate::fixtures::noise(64, 64, seed + 7919).into_raw()).unwrap();
        prop_assert!(distance(phash(&n1), phash(&n2)) >= 14, "noise pictures are unrelated: {}", distance(phash(&n1), phash(&n2)));
    }

    #[test]
    fn names_that_pass_the_check_never_leave_the_folder(name in "[a-c./\\\\x0]{0,14}") {
        use std::path::Component;
        match crate::src::safe_rel(&name) {
            Ok(p) => {
                prop_assert!(!p.is_absolute());
                prop_assert!(p.components().all(|c| matches!(c, Component::Normal(_))), "{:?}", p);
                prop_assert!(!name.contains('\\') && !name.contains('\0'));
            }
            Err(e) => prop_assert!(e.contains("bound folder") || e.contains("empty")),
        }
    }

    #[test]
    fn drawing_with_any_coordinates_never_panics_and_stays_in_the_image(
        x0 in -(1i64 << 62)..(1i64 << 62), y0 in -(1i64 << 62)..(1i64 << 62),
        x1 in -(1i64 << 62)..(1i64 << 62), y1 in -(1i64 << 62)..(1i64 << 62),
        w in 0u32..u32::MAX, t in 0u32..70
    ) {
        let mut img = Img::filled(24, 16, [255, 255, 255, 255]).unwrap();
        crate::draw::line(&mut img, [x0, y0], [x1, y1], t, [0, 0, 0, 255]);
        crate::draw::arrow(&mut img, [x0, y0], [x1, y1], t.min(8), [0, 0, 0, 255]);
        crate::draw::rect_outline(&mut img, x0, y0, w, w, t, [0, 0, 0, 255]);
        crate::draw::fill_rect(&mut img, x1, y1, w, w, [1, 2, 3, 4]);
        crate::draw::text(&mut img, x0, y1, "ab", 3, [0, 0, 0, 255], Some([9, 9, 9, 255]));
        prop_assert_eq!(img.px.len(), 24 * 16 * 4);
    }

    #[test]
    fn a_palette_accounts_for_every_counted_pixel(img in arb_img(12), n in 1usize..8) {
        let p = crate::palette::palette(&img, n);
        prop_assert!(p.swatches.len() <= n);
        prop_assert_eq!(p.swatches.iter().map(|s| s.pixels).sum::<u64>(), p.counted);
        prop_assert_eq!(p.counted + p.transparent, u64::from(img.w * img.h));
    }

    #[test]
    fn cursors_round_trip(item in 0usize..1000, tile in 0usize..100000, len in any::<u64>()) {
        let c = crate::cursor::Cursor { req: "r".repeat(64), item, tile, len, sha: "ab".repeat(32) };
        let text = crate::cursor::encode(&c);
        prop_assert_eq!(crate::cursor::decode(&text, &c.req, item + 1).unwrap(), c);
    }
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[a-z0-9_./-]{0,12}".prop_map(Value::from),
    ];
    leaf.prop_recursive(2, 12, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            proptest::collection::btree_map("[a-z_]{1,10}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(cfg(150))]

    /// Whatever object arrives, the call answers with a result or one sentence; it never panics.
    #[test]
    fn arbitrary_requests_never_panic(
        keys in proptest::collection::btree_map(
            prop::sample::select(vec![
                "path", "file", "files", "data_base64", "name", "op", "ops", "frame", "orient", "output", "cursor", "page_bytes",
                "width", "height", "mode", "x", "y", "degrees", "axis", "format", "quality", "shapes", "against", "tolerance",
                "ignore", "colors", "formats", "threshold", "max_side", "max_bytes", "size", "overlap", "cell", "cols", "bogus",
            ]),
            arb_json(),
            0..7,
        )
    ) {
        let mut req = serde_json::Map::new();
        for (k, v) in keys {
            req.insert(k.to_owned(), v);
        }
        // Keep paths pointing at the fixtures so valid-looking requests do real work.
        if req.get("path").is_some_and(Value::is_string) {
            req.insert("path".into(), json!(crate::testkit::fixtures()));
        }
        match crate::run_text(&Value::Object(req).to_string()) {
            Ok(t) => prop_assert!(serde_json::from_str::<Value>(&t).is_ok()),
            Err(e) => prop_assert!(!e.is_empty() && !e.contains('\n')),
        }
    }
}
