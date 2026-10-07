//! End-to-end tests of the steps that look at or rebuild a whole picture: tile,
//! montage, annotate, diff, palette, code reading and hash, through the same
//! entry point the host uses. Expected values come from independent knowledge
//! (the image crate's transforms, counted pixels, known payloads).
use image::{RgbaImage, imageops};
use serde_json::{Value, json};

use crate::testkit::*;

fn step(r: &Value, n: usize) -> &Value {
    &record(r, 0)["steps"][n]
}

/// Every tile part of a tile result placed back on a canvas of the source's size.
fn reassemble(pages: &[Value], w: u32, h: u32) -> (RgbaImage, Vec<(u32, u32, u32, u32)>) {
    let mut canvas = RgbaImage::new(w, h);
    let mut boxes = Vec::new();
    for page in pages {
        let tiles = step(page, 0)["tiles"].as_array().unwrap().clone();
        for t in tiles {
            let part = record(page, 0)["outputs"][t["output"].as_u64().unwrap() as usize]["part"]
                .as_u64()
                .unwrap() as usize;
            let img = part_image(page, part);
            let (x, y) = (
                t["x"].as_u64().unwrap() as u32,
                t["y"].as_u64().unwrap() as u32,
            );
            assert_eq!(
                (img.width(), img.height()),
                (
                    t["width"].as_u64().unwrap() as u32,
                    t["height"].as_u64().unwrap() as u32
                )
            );
            imageops::replace(&mut canvas, &img, i64::from(x), i64::from(y));
            boxes.push((x, y, img.width(), img.height()));
        }
    }
    (canvas, boxes)
}

#[test]
fn tiles_cover_the_picture_with_overlap_and_come_with_an_index() {
    let r = call(&on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 64, "overlap": 8}),
    ))
    .unwrap();
    let s = step(&r, 0);
    assert_eq!(
        (
            s["cols"].clone(),
            s["rows"].clone(),
            s["tiles_total"].clone()
        ),
        (json!(3), json!(2), json!(6))
    );
    assert!(s.get("next_tile").is_none());
    let ix = s["index"].as_u64().unwrap() as usize;
    assert_eq!(record(&r, 0)["outputs"][ix]["role"], "index");
    let index = part_image(
        &r,
        record(&r, 0)["outputs"][ix]["part"].as_u64().unwrap() as usize,
    );
    assert_eq!(
        (index.width(), index.height()),
        (150, 100),
        "a small picture's index is not scaled"
    );
    let (canvas, boxes) = reassemble(std::slice::from_ref(&r), 150, 100);
    assert_eq!(
        canvas,
        fixture_image("tile_src.png"),
        "the tiles put back give the source"
    );
    assert_eq!(boxes.len(), 6);
    let mut covered = vec![0u32; 150 * 100];
    for (x, y, w, h) in &boxes {
        for yy in *y..y + h {
            for xx in *x..x + w {
                covered[(yy * 150 + xx) as usize] += 1;
            }
        }
    }
    assert!(covered.iter().all(|&c| c >= 1));
    assert!(covered.iter().any(|&c| c >= 2), "neighbours overlap");
    assert_eq!(
        s["tiles"][0],
        json!({"n": 1, "row": 0, "col": 0, "x": 0, "y": 0, "width": 64, "height": 64, "output": 1})
    );
    assert_eq!(s["tiles"][5]["n"], 6);
    assert_eq!(
        (s["tiles"][5]["x"].clone(), s["tiles"][5]["y"].clone()),
        (json!(86), json!(36)),
        "the last tile ends at the edge"
    );
    assert_eq!(record(&r, 0)["outputs"][1]["tile"], 1);
}

#[test]
fn tile_pages_with_a_cursor_and_the_pages_add_up_to_the_one_shot_result() {
    let one = call(&on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 64, "overlap": 8}),
    ))
    .unwrap();
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut req = on_fixture(
            "tile_src.png",
            json!({"op": "tile", "size": 64, "overlap": 8, "page_bytes": 20_000}),
        );
        if let Some(c) = &cursor {
            req["cursor"] = json!(c);
        }
        let r = call(&req).unwrap();
        let next = response(&r)["next"]["cursor"].as_str().map(str::to_owned);
        pages.push(r);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
        assert!(pages.len() < 20, "paging must end");
    }
    assert!(
        pages.len() >= 2,
        "a 20000 byte page cannot hold the index and six tiles"
    );
    let (canvas, boxes) = reassemble(&pages, 150, 100);
    assert_eq!(canvas, fixture_image("tile_src.png"));
    let mut ns: Vec<u64> = pages
        .iter()
        .flat_map(|p| {
            step(p, 0)["tiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["n"].as_u64().unwrap())
        })
        .collect();
    assert_eq!(
        ns,
        (1..=6).collect::<Vec<_>>(),
        "every tile once and in order"
    );
    assert_eq!(boxes.len(), 6);
    // The index only comes on the first page, and the tile records equal the one-shot ones.
    assert!(step(&pages[0], 0).get("index").is_some());
    assert!(pages[1..].iter().all(|p| step(p, 0).get("index").is_none()));
    let paged: Vec<Value> = pages
        .iter()
        .flat_map(|p| {
            step(p, 0)["tiles"].as_array().unwrap().iter().map(|t| {
                let mut t = t.clone();
                t.as_object_mut().unwrap().remove("output");
                t
            })
        })
        .collect();
    let whole: Vec<Value> = step(&one, 0)["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let mut t = t.clone();
            t.as_object_mut().unwrap().remove("output");
            t
        })
        .collect();
    assert_eq!(paged, whole);
    ns.dedup();
    assert_eq!(ns.len(), 6);
    assert!(
        pages
            .iter()
            .all(|p| p["parts"].as_array().is_some_and(|a| !a.is_empty()))
    );
}

#[test]
fn a_cursor_from_a_different_request_or_a_changed_file_is_refused() {
    let first = call(&on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 64, "overlap": 8, "page_bytes": 20_000}),
    ))
    .unwrap();
    let cursor = response(&first)["next"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut other = on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 80, "overlap": 8, "page_bytes": 20_000}),
    );
    other["cursor"] = json!(cursor);
    assert!(call(&other).unwrap_err().contains("different request"));
    let mut garbled = on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 64, "overlap": 8, "page_bytes": 20_000}),
    );
    garbled["cursor"] = json!("imgt1.AAAA");
    assert!(call(&garbled).unwrap_err().contains("not valid"));
    // The same request over a file with other content: the cursor names the old bytes.
    let dir = scratch("cursorfile", &["tile_src.png"]);
    let req = |c: Option<&str>| {
        let mut r = json!({"path": dir.to_str().unwrap(), "files": ["tile_src.png"], "op": "tile", "size": 64, "overlap": 8, "page_bytes": 20_000});
        if let Some(c) = c {
            r["cursor"] = json!(c);
        }
        r
    };
    let first = call(&req(None)).unwrap();
    let cursor = response(&first)["next"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(
        dir.join("tile_src.png"),
        crate::fixtures::png(&crate::fixtures::scene(150, 100).clone()),
    )
    .unwrap();
    let changed: image::RgbaImage = crate::fixtures::noise(150, 100, 1);
    std::fs::write(dir.join("tile_src.png"), crate::fixtures::png(&changed)).unwrap();
    let e = call(&req(Some(&cursor))).unwrap_err();
    assert!(e.contains("changed since the cursor"), "{e}");
}

#[test]
fn tile_options_are_validated_and_a_tiny_page_cannot_start_a_cursor_loop() {
    let e = call(&on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 64, "overlap": 64}),
    ))
    .unwrap_err();
    assert!(e.contains("overlap must be smaller"), "{e}");
    let r = call(&on_fixture(
        "tile_src.png",
        json!({"op": "tile", "size": 4096}),
    ))
    .unwrap();
    assert_eq!(step(&r, 0)["tiles_total"], 1);
    let one = step(&r, 0)["tiles"][0].clone();
    assert_eq!(
        (one["width"].clone(), one["height"].clone()),
        (json!(150), json!(100)),
        "a picture smaller than a tile is one tile of its own size"
    );
}

#[test]
fn a_montage_places_every_image_and_marks_the_unreadable_one() {
    let r = call(&json!({"path": format!("{}/montage", fixtures()), "op": "montage", "cell": 32, "gap": 4, "labels": false})).unwrap();
    let rec = record(&r, 0);
    assert_eq!(rec["source"], "montage");
    let f = &rec["steps"][0];
    assert_eq!(
        (f["images"].clone(), f["cols"].clone(), f["rows"].clone()),
        (json!(4), json!(2), json!(2))
    );
    let sheet = part_image(&r, 0);
    assert_eq!((sheet.width(), sheet.height()), (4 + 2 * 36, 4 + 2 * 36));
    let cells = f["cells"].as_array().unwrap();
    let names: Vec<_> = cells
        .iter()
        .map(|c| c["source"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["blue.png", "broken.png", "green.png", "red.png"],
        "name order"
    );
    assert!(
        cells[1]["error"]
            .as_str()
            .unwrap()
            .contains("corrupt or truncated")
            || cells[1]["error"].as_str().unwrap().contains("not a PNG")
    );
    // The blue image is 32 wide and 14 high: fitted to 32 and centred in the 32 px cell.
    let c = &cells[0];
    assert_eq!(
        (
            c["width"].clone(),
            c["height"].clone(),
            c["x"].clone(),
            c["y"].clone()
        ),
        (json!(32), json!(14), json!(4), json!(4 + 9))
    );
    assert_eq!(sheet.get_pixel(4 + 16, 4 + 16).0, [50, 80, 220, 255]);
    let red = &cells[3];
    assert_eq!(
        sheet
            .get_pixel(
                red["x"].as_u64().unwrap() as u32 + 3,
                red["y"].as_u64().unwrap() as u32 + 3
            )
            .0,
        [220, 40, 40, 255]
    );
}

#[test]
fn annotate_draws_the_shapes_where_the_coordinates_say() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "annotate", "shapes": [
            {"type": "box", "x": 2, "y": 2, "width": 10, "height": 8, "color": "#00ff00", "thickness": 1},
            {"type": "line", "from": [0, 23], "to": [31, 23], "color": "black", "thickness": 1},
            {"type": "box", "x": 500, "y": 500, "width": 3, "height": 3}]}),
    ))
    .unwrap();
    let got = part_image(&r, 0);
    let src = fixture_image("scene.png");
    for x in 2..12 {
        assert_eq!(got.get_pixel(x, 2).0, [0, 255, 0, 255]);
        assert_eq!(got.get_pixel(x, 9).0, [0, 255, 0, 255]);
    }
    for y in 2..10 {
        assert_eq!(got.get_pixel(2, y).0, [0, 255, 0, 255]);
        assert_eq!(got.get_pixel(11, y).0, [0, 255, 0, 255]);
    }
    assert_eq!(
        got.get_pixel(5, 5),
        src.get_pixel(5, 5),
        "the inside of an unfilled box is untouched"
    );
    assert_eq!(got.get_pixel(15, 23).0, [0, 0, 0, 255]);
    assert_eq!(got.get_pixel(15, 22), src.get_pixel(15, 22));
    assert_eq!(
        step(&r, 0),
        &json!({"op": "annotate", "shapes": 2, "outside": [2]})
    );
}

#[test]
fn diff_finds_the_two_changed_regions_with_exact_boxes_and_counts() {
    let r = call(&on_fixture(
        "diff_a.png",
        json!({"op": "diff", "against": "diff_b.png", "merge_gap": 0}),
    ))
    .unwrap();
    let s = step(&r, 0);
    // The generator painted a 12x9 black box at (10,8) and a 5x5 white box at (50,40).
    assert_eq!(
        s["regions"][0],
        json!({"x": 10, "y": 8, "width": 12, "height": 9, "changed": 108})
    );
    let second = &s["regions"][1];
    assert_eq!(
        (
            second["x"].clone(),
            second["y"].clone(),
            second["width"].clone(),
            second["height"].clone()
        ),
        (json!(50), json!(40), json!(5), json!(5))
    );
    let a = fixture_image("diff_a.png");
    let b = fixture_image("diff_b.png");
    let truly: usize = a.pixels().zip(b.pixels()).filter(|(p, q)| p != q).count();
    assert_eq!(s["changed"], truly);
    assert_eq!(s["compared"], 64 * 48);
    assert_eq!(s["same_size"], true);
    assert_eq!(s["identical"], false);
    assert!(s["ssim"].as_f64().unwrap() < 1.0);
    let hl = record(&r, 0)["outputs"][s["highlight"].as_u64().unwrap() as usize].clone();
    assert_eq!(hl["role"], "diff");
    let img = part_image(&r, hl["part"].as_u64().unwrap() as usize);
    assert_eq!(
        img.get_pixel(12, 10).0,
        [255, 0, 0, 255],
        "a changed pixel is red"
    );
    assert_eq!(
        img.get_pixel(9, 7).0,
        [255, 0, 255, 255],
        "the region box is drawn one pixel outside"
    );
}

#[test]
fn diff_options_tolerance_ignore_and_size_mismatch() {
    let same = call(&on_fixture(
        "diff_a.png",
        json!({"op": "diff", "against": "diff_a.png"}),
    ))
    .unwrap();
    assert_eq!(
        (
            step(&same, 0)["identical"].clone(),
            step(&same, 0)["changed"].clone(),
            step(&same, 0)["ssim"].clone()
        ),
        (json!(true), json!(0), json!(1.0))
    );
    let ignored = call(&on_fixture("diff_a.png", json!({"op": "diff", "against": "diff_b.png", "ignore": [{"x": 0, "y": 0, "width": 64, "height": 30}], "highlight": false}))).unwrap();
    let s = step(&ignored, 0);
    assert_eq!(s["regions_total"], 1, "the upper change is ignored");
    assert_eq!(s["compared"], 64 * 48 - 64 * 30);
    assert!(
        record(&ignored, 0)["outputs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let tolerant = call(&on_fixture(
        "diff_a.png",
        json!({"op": "diff", "against": "diff_b.png", "tolerance": 255, "highlight": false}),
    ))
    .unwrap();
    assert_eq!(step(&tolerant, 0)["changed"], 0);
    let wide = call(&on_fixture(
        "diff_a.png",
        json!({"op": "diff", "against": "diff_wide.png", "highlight": false}),
    ))
    .unwrap();
    let s = step(&wide, 0);
    assert_eq!(
        (
            s["same_size"].clone(),
            s["width"].clone(),
            s["ssim"].clone()
        ),
        (json!(false), json!(80), json!(null))
    );
    let inline = {
        use base64::Engine as _;
        let bytes = std::fs::read(format!("{}/diff_b.png", fixtures())).unwrap();
        call(&on_fixture("diff_a.png", json!({"op": "diff", "against_base64": base64::engine::general_purpose::STANDARD.encode(bytes), "highlight": false}))).unwrap()
    };
    assert_eq!(step(&inline, 0)["changed"], 108 + 25);
    let missing = call(&on_fixture(
        "diff_a.png",
        json!({"op": "diff", "against": "no_such.png"}),
    ))
    .unwrap();
    assert!(
        record(&missing, 0)["error"]
            .as_str()
            .unwrap()
            .contains("cannot read no_such.png")
    );
}

#[test]
fn palette_reports_the_dominant_colours_of_a_known_picture() {
    let dir = scratch("palette", &[]);
    let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
    for y in 0..10 {
        for x in 6..10 {
            img.put_pixel(x, y, image::Rgba([0, 0, 255, 255]));
        }
    }
    img.save(dir.join("two.png")).unwrap();
    let r = call(
        &json!({"path": dir.to_str().unwrap(), "files": ["two.png"], "op": "palette", "colors": 2}),
    )
    .unwrap();
    assert_eq!(
        step(&r, 0)["colors"],
        json!([
        {"hex": "#ff0000", "rgb": [255, 0, 0], "share": 0.6},
        {"hex": "#0000ff", "rgb": [0, 0, 255], "share": 0.4}])
    );
    let la = call(&on_fixture(
        "la8.png",
        json!({"op": "palette", "colors": 3}),
    ))
    .unwrap();
    assert!(step(&la, 0).get("colors").is_some());
}

#[test]
fn every_code_fixture_decodes_to_its_known_payload() {
    let cases = [
        ("qr_plain.png", "qr_code", "https://example.org/pack?id=42"),
        ("qr_rot90.png", "qr_code", "https://example.org/pack?id=42"),
        ("qr_rot30.png", "qr_code", "https://example.org/pack?id=42"),
        (
            "qr_lowcontrast.png",
            "qr_code",
            "https://example.org/pack?id=42",
        ),
        (
            "qr_inverted.png",
            "qr_code",
            "https://example.org/pack?id=42",
        ),
        (
            "qr_damaged.png",
            "qr_code",
            "DAMAGED-BUT-READABLE-0123456789",
        ),
        ("code128.png", "code_128", "ABC-12345"),
        ("ean13.png", "ean_13", "5901234123457"),
        ("datamatrix.png", "data_matrix", "DM-PAYLOAD-77"),
    ];
    for (file, format, text) in cases {
        let r = call(&json!({"path": format!("{}/codes", fixtures()), "files": [file], "op": "decode_codes"})).unwrap();
        let codes = step(&r, 0)["codes"].as_array().unwrap();
        assert_eq!(codes.len(), 1, "{file}: {codes:?}");
        assert_eq!(
            (codes[0]["format"].clone(), codes[0]["text"].clone()),
            (json!(format), json!(text)),
            "{file}"
        );
        let b = &codes[0]["bbox"];
        assert!(
            b[2].as_i64().unwrap() > 10 && b[3].as_i64().unwrap() >= 0,
            "{file}: {b}"
        );
    }
    let two = call(&json!({"path": format!("{}/codes", fixtures()), "files": ["two_codes.png"], "op": "decode_codes"})).unwrap();
    let texts: Vec<_> = step(&two, 0)["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["LEFT", "RIGHT"], "left to right on the same row");
    let none = call(&json!({"path": format!("{}/codes", fixtures()), "files": ["no_code.png"], "op": "decode_codes"})).unwrap();
    assert_eq!(
        (
            step(&none, 0)["found"].clone(),
            step(&none, 0)["attempt"].clone()
        ),
        (json!(0), json!("none"))
    );
    let only = call(&json!({"path": format!("{}/codes", fixtures()), "files": ["qr_plain.png"], "op": "decode_codes", "formats": ["ean_13"]})).unwrap();
    assert_eq!(step(&only, 0)["found"], 0);
    let bad = call(&json!({"path": format!("{}/codes", fixtures()), "files": ["qr_plain.png"], "op": "decode_codes", "formats": ["qr"]})).unwrap();
    assert!(
        record(&bad, 0)["error"]
            .as_str()
            .unwrap()
            .contains("not a code format")
    );
}

#[test]
fn hash_gives_exact_digests_and_tells_near_duplicates_from_strangers() {
    let r = call(&on_fixture(
        "diff_a.png",
        json!({"op": "hash", "against": "dup_small.png"}),
    ))
    .unwrap();
    let s = step(&r, 0);
    let bytes = std::fs::read(format!("{}/diff_a.png", fixtures())).unwrap();
    assert_eq!(
        s["sha256"],
        crate::model::sha256_hex(&bytes),
        "the exact content hash is the file's SHA-256"
    );
    assert_eq!(s["pixels_sha256"], pixels_sha(&fixture_image("diff_a.png")));
    assert_eq!(s["phash"].as_str().unwrap().len(), 16);
    let a = &s["against"];
    assert_eq!(
        a["similar"], true,
        "a half-size copy is a near duplicate: {a}"
    );
    assert_eq!(a["identical_bytes"], false);
    assert!(
        a["phash_distance"].as_u64().unwrap() <= 8 && a["dhash_distance"].as_u64().unwrap() <= 10,
        "{a}"
    );
    let stranger = call(&on_fixture(
        "diff_a.png",
        json!({"op": "hash", "against": "dup_noise.png"}),
    ))
    .unwrap();
    let a = &step(&stranger, 0)["against"];
    assert_eq!(a["similar"], false, "{a}");
    assert!(a["phash_distance"].as_u64().unwrap() > 10);
    let same = call(&on_fixture(
        "diff_a.png",
        json!({"op": "hash", "against": "diff_a.png"}),
    ))
    .unwrap();
    let a = &step(&same, 0)["against"];
    assert_eq!(
        (
            a["identical_bytes"].clone(),
            a["identical_pixels"].clone(),
            a["phash_distance"].clone()
        ),
        (json!(true), json!(true), json!(0))
    );
}
