//! End-to-end tests of every step through the same entry point the host uses.
//! Expected values come from independent knowledge (the image crate's own
//! transforms, counted pixels, known payloads), not from the plugin's output.
use image::imageops;
use serde_json::{Value, json};

use crate::testkit::*;

fn step(r: &Value, n: usize) -> &Value {
    &record(r, 0)["steps"][n]
}

fn out(r: &Value, n: usize) -> &Value {
    &record(r, 0)["outputs"][n]
}

#[test]
fn info_describes_every_format_from_its_header() {
    for (file, format, color, alpha) in [
        ("scene.png", "png", "rgba8", true),
        ("scene.jpg", "jpeg", "rgb8", false),
        ("scene.webp", "webp", "rgba8", true),
        ("scene.bmp", "bmp", "rgb8", false),
        ("scene.tif", "tiff", "rgb8", false),
        ("scene.gif", "gif", "rgba8", true),
    ] {
        let r = call(&on_fixture(file, json!({"op": "info"}))).unwrap();
        let s = step(&r, 0);
        assert_eq!(s["format"], format, "{file}");
        assert_eq!(
            (s["width"].clone(), s["height"].clone()),
            (json!(32), json!(24)),
            "{file}"
        );
        assert_eq!(
            (s["color"].clone(), s["has_alpha"].clone()),
            (json!(color), json!(alpha)),
            "{file}"
        );
        assert_eq!(
            (s["bit_depth"].clone(), s["frames"].clone()),
            (json!(8), json!(1)),
            "{file}"
        );
        assert_eq!(s["exif"]["present"], false, "{file}");
        assert_eq!(s["icc"]["present"], false, "{file}");
        assert_eq!(
            s["bytes"],
            std::fs::metadata(format!("{}/{file}", fixtures()))
                .unwrap()
                .len(),
            "{file}"
        );
        assert!(
            record(&r, 0)["outputs"].as_array().unwrap().is_empty(),
            "info writes nothing"
        );
        assert!(r.get("parts").is_none());
    }
}

#[test]
fn info_finds_exif_gps_and_icc_in_each_container_and_hides_the_position() {
    for file in ["photo_meta.jpg", "photo_meta.png", "photo_meta.webp"] {
        let r = call(&on_fixture(file, json!({"op": "info"}))).unwrap();
        let s = step(&r, 0);
        assert_eq!(
            s["exif"],
            json!({"present": true, "orientation": 6, "gps": true}),
            "{file}: GPS is reported as present, never printed"
        );
        assert_eq!(s["icc"], json!({"present": true, "bytes": 160}), "{file}");
        assert_eq!(
            (s["width"].clone(), s["height"].clone()),
            (json!(48), json!(32)),
            "{file}"
        );
        assert_eq!(
            (s["oriented_width"].clone(), s["oriented_height"].clone()),
            (json!(32), json!(48)),
            "{file}"
        );
        assert!(
            !r.to_string().contains("48.8566"),
            "{file}: no coordinates unless asked"
        );
        let g = call(&on_fixture(file, json!({"op": "info", "gps": true}))).unwrap();
        assert_eq!(
            step(&g, 0)["exif"]["position"],
            json!({"lat": 48.8566, "lon": 2.3522}),
            "{file}"
        );
    }
    let r = call(&on_fixture(
        "photo_noexif.jpg",
        json!({"op": "info", "gps": true}),
    ))
    .unwrap();
    assert_eq!(
        step(&r, 0)["exif"],
        json!({"present": false, "orientation": null, "gps": false})
    );
}

#[test]
fn info_counts_animation_frames_and_other_colour_types() {
    for file in ["anim.gif", "anim.webp"] {
        let s = call(&on_fixture(file, json!({"op": "info"}))).unwrap();
        assert_eq!(step(&s, 0)["frames"], 3, "{file}");
    }
    let g16 = call(&on_fixture("gray16.png", json!({"op": "info"}))).unwrap();
    assert_eq!(
        (
            step(&g16, 0)["color"].clone(),
            step(&g16, 0)["bit_depth"].clone()
        ),
        (json!("l16"), json!(16))
    );
    let la = call(&on_fixture("la8.png", json!({"op": "info"}))).unwrap();
    assert_eq!(
        (
            step(&la, 0)["color"].clone(),
            step(&la, 0)["has_alpha"].clone()
        ),
        (json!("la8"), json!(true))
    );
    let cmyk = call(&on_fixture("cmyk.jpg", json!({"op": "info"}))).unwrap();
    assert_eq!(step(&cmyk, 0)["format"], "jpeg");
}

#[test]
fn all_eight_orientations_decode_to_the_same_upright_picture() {
    let upright = pixels_sha(&fixture_image("upright.png"));
    for n in 1..=8 {
        let r = call(&on_fixture(
            &format!("orient_{n}.png"),
            json!({"op": "hash"}),
        ))
        .unwrap();
        assert_eq!(step(&r, 0)["pixels_sha256"], upright, "orientation {n}");
        let want = (n >= 2).then_some(n);
        assert_eq!(
            record(&r, 0)
                .get("orientation_applied")
                .map(|v| v.as_u64().unwrap()),
            want,
            "orientation {n}"
        );
    }
}

#[test]
fn without_orient_the_stored_layout_stays_until_auto_orient_is_asked_for() {
    let upright = pixels_sha(&fixture_image("upright.png"));
    for n in 2..=8 {
        let file = format!("orient_{n}.png");
        let stored = call(&on_fixture(&file, json!({"op": "hash", "orient": false}))).unwrap();
        assert_ne!(
            step(&stored, 0)["pixels_sha256"],
            upright,
            "orientation {n} stays turned"
        );
        assert!(record(&stored, 0).get("orientation_applied").is_none());
        let fixed = call(&on_fixture(
            &file,
            json!({"orient": false, "ops": [{"op": "auto_orient"}, {"op": "hash"}]}),
        ))
        .unwrap();
        assert_eq!(
            step(&fixed, 1)["pixels_sha256"],
            upright,
            "orientation {n} turned by the step"
        );
        assert_eq!(
            step(&fixed, 0),
            &json!({"op": "auto_orient", "orientation": n, "applied": true})
        );
        assert_eq!(record(&fixed, 0)["orientation_applied"], n);
    }
    let one = call(&on_fixture(
        "orient_1.png",
        json!({"orient": false, "ops": [{"op": "auto_orient"}]}),
    ))
    .unwrap();
    assert_eq!(
        step(&one, 0),
        &json!({"op": "auto_orient", "orientation": 1, "applied": false})
    );
}

#[test]
fn view_returns_an_upright_png_fitted_to_the_side_limit() {
    let r = call(&on_fixture(
        "orient_6.png",
        json!({"op": "view", "max_side": 16}),
    ))
    .unwrap();
    assert_eq!(r["parts"].as_array().unwrap().len(), 1);
    assert_eq!(r["parts"][0]["mimeType"], "image/png");
    let img = part_image(&r, 0);
    assert_eq!(
        img,
        fixture_image("upright.png"),
        "6x4 fits in 16, so nothing is scaled and it is upright"
    );
    let o = out(&r["response"], 0);
    assert_eq!(
        (
            o["width"].clone(),
            o["height"].clone(),
            o["part"].clone(),
            o["role"].clone()
        ),
        (json!(6), json!(4), json!(0), json!("view"))
    );
    assert_eq!(o["pixels_sha256"], pixels_sha(&img));
    assert_eq!(o["bytes"], part_bytes(&r, 0).len());
    assert!(record(&r, 0)["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn view_scales_down_and_says_so() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "view", "max_side": 16}),
    ))
    .unwrap();
    let img = part_image(&r, 0);
    assert_eq!((img.width(), img.height()), (16, 12));
    assert_eq!(
        record(&r, 0)["warnings"],
        json!(["scaled from 32x24 to 16x12 to fit 16 pixels on the longest side"])
    );
    let s = step(&r, 0);
    assert_eq!(
        (s["from"].clone(), s["to"].clone(), s["format"].clone()),
        (json!([32, 24]), json!([16, 12]), json!("png"))
    );
}

#[test]
fn view_falls_back_to_jpeg_when_the_png_is_over_the_byte_limit() {
    let dir = scratch("viewjpeg", &[]);
    let noise = image::RgbaImage::from_fn(100, 100, |x, y| {
        let v = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503).wrapping_add(x << 7);
        let v = v >> 3;
        image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
    });
    noise.save(dir.join("n.png")).unwrap();
    let png_len = std::fs::metadata(dir.join("n.png")).unwrap().len();
    assert!(
        png_len > 15_000,
        "a noise PNG is too big for the limit below: {png_len}"
    );
    let r = call(&json!({"path": dir.to_str().unwrap(), "files": ["n.png"], "op": "view", "max_bytes": 10_000})).unwrap();
    assert_eq!(r["parts"][0]["mimeType"], "image/jpeg");
    assert!(part_bytes(&r, 0).len() <= 10_000);
    let w = record(&r, 0)["warnings"][0].as_str().unwrap();
    let size: usize = w
        .strip_prefix("the PNG was ")
        .and_then(|t| t.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(size > 10_000, "{w}");
    assert!(w.contains(", over 10000; sent as JPEG at quality"), "{w}");
    let img = part_image(&r, 0);
    assert_eq!(
        (img.width(), img.height()),
        (100, 100),
        "JPEG sufficed, so the picture was not scaled"
    );
}

#[test]
fn a_large_jpeg_is_viewed_through_a_small_decode() {
    let dir = scratch("bigjpeg", &[]);
    let big = crate::fixtures::scene(2400, 1600);
    std::fs::write(dir.join("big.jpg"), crate::fixtures::jpeg(&big, 85)).unwrap();
    let r = call(&json!({"path": dir.to_str().unwrap(), "files": ["big.jpg"], "op": "view", "max_side": 300})).unwrap();
    let img = part_image(&r, 0);
    assert_eq!((img.width(), img.height()), (300, 200));
    // 2400x1600 shrinks to 300x200, which a 1/8 decode gives exactly, so nothing is resampled afterwards.
    assert_eq!(
        record(&r, 0)["warnings"],
        json!(["the JPEG was decoded at 1/8 of its size, which is all the view needs"])
    );
    assert_eq!(step(&r, 0)["from"], json!([2400, 1600]));
    // A size 1/8 cannot hit exactly is decoded at 1/8 and then fitted; the note names the file's own size.
    std::fs::write(
        dir.join("odd.jpg"),
        crate::fixtures::jpeg(&crate::fixtures::scene(2500, 1700), 85),
    )
    .unwrap();
    let r = call(&json!({"path": dir.to_str().unwrap(), "files": ["odd.jpg"], "op": "view", "max_side": 300})).unwrap();
    let img = part_image(&r, 0);
    assert_eq!((img.width(), img.height()), (300, 204));
    let w = record(&r, 0)["warnings"].to_string();
    assert!(
        w.contains("scaled from 2500x1700 to 300x204 to fit 300 pixels"),
        "{w}"
    );
    assert!(w.contains("decoded at 1/8"), "{w}");
}

fn resized(file: &str, spec: Value) -> Value {
    let mut s = json!({"op": "resize", "output": {"part": true}});
    for (k, v) in spec.as_object().unwrap() {
        s[k] = v.clone();
    }
    call(&on_fixture(file, s)).unwrap()
}

#[test]
fn resize_modes_give_the_known_sizes() {
    let fit = resized("scene.png", json!({"width": 16, "height": 16}));
    assert_eq!(
        (part_image(&fit, 0).width(), part_image(&fit, 0).height()),
        (16, 12)
    );
    assert_eq!(step(&fit, 0)["to"], json!([16, 12]));
    let h_only = resized("scene.png", json!({"height": 12}));
    assert_eq!(step(&h_only, 0)["to"], json!([16, 12]));
    let exact = resized(
        "scene.png",
        json!({"mode": "exact", "width": 10, "height": 30}),
    );
    assert_eq!(step(&exact, 0)["to"], json!([10, 30]));
    let fill = resized(
        "scene.png",
        json!({"mode": "fill", "width": 10, "height": 10}),
    );
    assert_eq!(step(&fill, 0)["to"], json!([10, 10]));
    let same = resized("scene.png", json!({"width": 100, "height": 100}));
    assert_eq!(
        step(&same, 0)["changed"],
        false,
        "a smaller picture is left alone"
    );
    let up = resized("scene.png", json!({"width": 64, "upscale": true}));
    assert_eq!(step(&up, 0)["to"], json!([64, 48]));
}

#[test]
fn resize_with_nearest_doubles_every_pixel() {
    let r = resized(
        "upright.png",
        json!({"mode": "exact", "width": 12, "height": 8, "filter": "nearest"}),
    );
    let got = part_image(&r, 0);
    let src = fixture_image("upright.png");
    for y in 0..8 {
        for x in 0..12 {
            assert_eq!(
                got.get_pixel(x, y),
                src.get_pixel(x / 2, y / 2),
                "({x},{y})"
            );
        }
    }
}

#[test]
fn crop_returns_exactly_the_requested_box() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "crop", "x": 5, "y": 3, "width": 10, "height": 7}),
    ))
    .unwrap();
    let got = part_image(&r, 0);
    let want = imageops::crop_imm(&fixture_image("scene.png"), 5, 3, 10, 7).to_image();
    assert_eq!(got, want);
    assert_eq!(
        step(&r, 0),
        &json!({"op": "crop", "box": [5, 3, 10, 7], "from": [32, 24]})
    );
    let e = call(&on_fixture(
        "scene.png",
        json!({"op": "crop", "x": 30, "y": 0, "width": 5, "height": 5}),
    ))
    .unwrap();
    let msg = record(&e, 0)["error"].as_str().unwrap();
    assert!(msg.contains("outside the 32x24 image"), "{msg}");
    assert_eq!(record(&e, 0)["step"], 1);
}

#[test]
fn rotate_and_flip_match_the_image_crates_own_transforms() {
    let src = fixture_image("scene.png");
    let rot = |d: i64| {
        part_image(
            &call(&on_fixture(
                "scene.png",
                json!({"op": "rotate", "degrees": d}),
            ))
            .unwrap(),
            0,
        )
    };
    assert_eq!(rot(90), imageops::rotate90(&src));
    assert_eq!(rot(180), imageops::rotate180(&src));
    assert_eq!(rot(270), imageops::rotate270(&src));
    assert_eq!(rot(-90), imageops::rotate270(&src));
    assert_eq!(rot(0), src);
    let flip = |a: &str| {
        part_image(
            &call(&on_fixture("scene.png", json!({"op": "flip", "axis": a}))).unwrap(),
            0,
        )
    };
    assert_eq!(flip("horizontal"), imageops::flip_horizontal(&src));
    assert_eq!(flip("vertical"), imageops::flip_vertical(&src));
    let e = call(&on_fixture(
        "scene.png",
        json!({"op": "rotate", "degrees": 45}),
    ))
    .unwrap();
    assert!(
        record(&e, 0)["error"]
            .as_str()
            .unwrap()
            .contains("multiple of 90")
    );
}

#[test]
fn convert_writes_each_format_and_lossless_ones_keep_every_pixel() {
    let src = fixture_image("scene.png");
    for (fmt, mime, lossless) in [
        ("png", "image/png", true),
        ("webp", "image/webp", true),
        ("jpeg", "image/jpeg", false),
        ("gif", "image/gif", false),
    ] {
        let r = call(&on_fixture(
            "scene.png",
            json!({"op": "convert", "format": fmt}),
        ))
        .unwrap();
        assert_eq!(r["parts"][0]["mimeType"], mime, "{fmt}");
        let got = part_image(&r, 0);
        assert_eq!((got.width(), got.height()), (32, 24), "{fmt}");
        if lossless {
            assert_eq!(got, src, "{fmt} keeps every pixel");
        }
        assert_eq!(record(&r, 0)["outputs"][0]["format"], fmt, "{fmt}");
    }
    for fmt in ["bmp", "tiff"] {
        let r = call(&on_fixture(
            "scene.png",
            json!({"op": "convert", "format": fmt}),
        ))
        .unwrap();
        assert!(
            r.get("parts").is_none(),
            "{fmt} cannot be shown to a model, so it is not attached"
        );
        assert_eq!(record(&r, 0)["outputs"][0]["format"], fmt);
        assert!(record(&r, 0)["outputs"][0].get("part").is_none());
    }
    let q = call(&on_fixture(
        "scene.png",
        json!({"ops": [{"op": "convert", "format": "jpeg", "quality": 20}]}),
    ))
    .unwrap();
    let hi = call(&on_fixture(
        "scene.png",
        json!({"ops": [{"op": "convert", "format": "jpeg", "quality": 95}]}),
    ))
    .unwrap();
    assert!(part_bytes(&q, 0).len() < part_bytes(&hi, 0).len());
}

#[test]
fn convert_to_jpeg_flattens_transparency_and_says_so() {
    let r = call(&on_fixture(
        "la8.png",
        json!({"op": "convert", "format": "jpeg"}),
    ))
    .unwrap();
    assert!(
        record(&r, 0)["warnings"]
            .to_string()
            .contains("flattened onto white")
    );
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "convert", "format": "png", "quality": 50}),
    ))
    .unwrap();
    assert!(
        record(&r, 0)["warnings"]
            .to_string()
            .contains("quality is ignored for png")
    );
}

#[test]
fn strip_metadata_leaves_no_exif_gps_or_icc_and_bakes_in_the_orientation() {
    for file in ["photo_meta.jpg", "photo_meta.png", "photo_meta.webp"] {
        let r = call(&on_fixture(file, json!({"op": "strip_metadata"}))).unwrap();
        let s = step(&r, 0);
        assert_eq!(
            s["removed"],
            json!({"exif": true, "gps": true, "icc": true, "xmp": false}),
            "{file}"
        );
        assert_eq!(s["orientation_baked_in"], true, "{file}");
        let bytes = part_bytes(&r, 0);
        let src = crate::src::Source {
            name: "x".into(),
            data: crate::src::Data::Mem(std::sync::Arc::new(bytes)),
        };
        let h = crate::decode::header(&src).unwrap();
        assert!(
            h.exif.is_none() && h.icc.is_none() && !h.xmp,
            "{file}: re-parsing the output finds no metadata"
        );
        assert_eq!(
            (h.width, h.height),
            (32, 48),
            "{file}: the orientation was applied, so the stored size is the upright one"
        );
        assert_eq!(record(&r, 0)["orientation_applied"], 6);
    }
}

#[test]
fn keep_icc_keeps_only_the_colour_profile() {
    let r = call(&on_fixture(
        "photo_meta.png",
        json!({"op": "strip_metadata", "keep_icc": true}),
    ))
    .unwrap();
    assert_eq!(
        step(&r, 0)["removed"],
        json!({"exif": true, "gps": true, "icc": false, "xmp": false})
    );
    let src = crate::src::Source {
        name: "x".into(),
        data: crate::src::Data::Mem(std::sync::Arc::new(part_bytes(&r, 0))),
    };
    let h = crate::decode::header(&src).unwrap();
    assert!(h.exif.is_none());
    assert_eq!(h.icc.map(|p| p.len()), Some(160));
}

#[test]
fn a_chain_runs_in_memory_and_matches_the_same_steps_done_by_hand() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"ops": [
            {"op": "crop", "x": 4, "y": 2, "width": 20, "height": 16},
            {"op": "rotate", "degrees": 90},
            {"op": "flip", "axis": "horizontal"},
            {"op": "resize", "mode": "exact", "width": 8, "height": 10, "filter": "nearest"}]}),
    ))
    .unwrap();
    let mut want = imageops::crop_imm(&fixture_image("scene.png"), 4, 2, 20, 16).to_image();
    want = imageops::rotate90(&want);
    want = imageops::flip_horizontal(&want);
    let got = part_image(&r, 0);
    assert_eq!((got.width(), got.height()), (8, 10));
    let names: Vec<_> = record(&r, 0)["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["op"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["crop", "rotate", "flip", "resize"]);
    assert_eq!(
        record(&r, 0)["outputs"].as_array().unwrap().len(),
        1,
        "one image comes out of a chain"
    );
    // Nearest-neighbour sampling at the centre of each target pixel of the hand-made picture.
    for y in 0..10u32 {
        for x in 0..8u32 {
            let (sx, sy) = (
                (2 * x + 1) * want.width() / 16,
                (2 * y + 1) * want.height() / 20,
            );
            assert_eq!(got.get_pixel(x, y), want.get_pixel(sx, sy), "({x},{y})");
        }
    }
}

#[test]
fn analysis_steps_in_a_chain_report_on_the_picture_as_it_is_then() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"ops": [{"op": "hash"}, {"op": "crop", "x": 0, "y": 0, "width": 8, "height": 8}, {"op": "hash"}, {"op": "palette", "colors": 2}]}),
    ))
    .unwrap();
    let steps = record(&r, 0)["steps"].as_array().unwrap();
    assert_eq!(
        (steps[0]["width"].clone(), steps[2]["width"].clone()),
        (json!(32), json!(8))
    );
    assert_ne!(steps[0]["pixels_sha256"], steps[2]["pixels_sha256"]);
    assert_eq!(
        steps[0]["sha256"], steps[2]["sha256"],
        "the file's own digest does not change"
    );
    assert_eq!(steps[3]["op"], "palette");
    assert_eq!(
        record(&r, 0)["outputs"].as_array().unwrap().len(),
        1,
        "the crop makes the chain write its picture"
    );
}

#[test]
fn a_chain_of_only_analysis_steps_writes_no_image() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"ops": [{"op": "info"}, {"op": "hash"}, {"op": "palette"}, {"op": "decode_codes"}]}),
    ))
    .unwrap();
    assert!(record(&r, 0)["outputs"].as_array().unwrap().is_empty());
    assert!(r.get("parts").is_none());
}
