//! End-to-end tests of what touches the file system and the limits: writing
//! files, names that try to leave the folder, hostile and oversized inputs,
//! folders, inline images and paging across images.
use std::path::Path;

use base64::Engine as _;
use image::RgbaImage;
use serde_json::{Value, json};

use crate::testkit::*;

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    walk(dir, dir, &mut v);
    v.sort();
    v
}

fn dir_str(d: &Path) -> String {
    d.to_str().unwrap().to_owned()
}

fn err(r: &Value) -> &str {
    record(r, 0)["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error in {r}"))
}

#[test]
fn a_written_file_is_the_result_and_the_source_is_untouched() {
    let d = scratch("write1", &["scene.png"]);
    let before = std::fs::read(d.join("scene.png")).unwrap();
    let r = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 16, "output": {"file": "out/small.png"}})).unwrap();
    assert!(
        r.get("parts").is_none(),
        "writing a file does not attach it"
    );
    let o = &record(&r, 0)["outputs"][0];
    assert_eq!(o["file"], "out/small.png");
    let written = image::open(d.join("out/small.png")).unwrap().to_rgba8();
    assert_eq!((written.width(), written.height()), (16, 12));
    assert_eq!(o["pixels_sha256"], pixels_sha(&written));
    assert_eq!(
        o["sha256"],
        crate::model::sha256_hex(&std::fs::read(d.join("out/small.png")).unwrap())
    );
    assert_eq!(std::fs::read(d.join("scene.png")).unwrap(), before);
    assert_eq!(
        listing(&d),
        ["out/small.png", "scene.png"],
        "no temporary file is left"
    );
}

#[test]
fn an_existing_file_is_never_replaced_unless_overwrite_is_true() {
    let d = scratch("write2", &["scene.png"]);
    let req = |ow: bool| json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 16, "output": {"file": "s.png", "overwrite": ow}});
    call(&req(false)).unwrap();
    let first = std::fs::read(d.join("s.png")).unwrap();
    let again = call(&req(false)).unwrap();
    assert!(err(&again).contains("already exists"), "{}", err(&again));
    assert_eq!(std::fs::read(d.join("s.png")).unwrap(), first);
    let ok = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": "s.png", "overwrite": true}})).unwrap();
    assert!(record(&ok, 0).get("error").is_none());
    assert_eq!(image::open(d.join("s.png")).unwrap().width(), 8);
}

#[test]
fn the_source_is_replaced_only_when_named_with_overwrite() {
    let d = scratch("write3", &["scene.png"]);
    let refuse = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": "scene.png"}})).unwrap();
    assert!(err(&refuse).contains("already exists"));
    assert_eq!(image::open(d.join("scene.png")).unwrap().width(), 32);
    call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": "scene.png", "overwrite": true}})).unwrap();
    assert_eq!(image::open(d.join("scene.png")).unwrap().width(), 8);
}

#[test]
fn a_suffix_writes_one_file_per_image_beside_its_source() {
    let d = scratch("suffix", &["batch/a.png", "batch/b.jpg", "batch/sub/c.png"]);
    let r = call(&json!({"path": dir_str(&d.join("batch")), "op": "resize", "width": 8, "output": {"suffix": "_s"}})).unwrap();
    let files: Vec<_> = response(&r)["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["outputs"][0]["file"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        files,
        ["a_s.png", "b_s.png", "sub/c_s.png"],
        "the output keeps the format (png) and sits beside the source"
    );
    assert!(r.get("parts").is_none());
    for f in &files {
        assert_eq!(
            image::open(d.join("batch").join(f)).unwrap().width(),
            8,
            "{f}"
        );
    }
    let jpeg = call(&json!({"path": dir_str(&d.join("batch")), "files": ["a.png"], "ops": [{"op": "resize", "width": 8}, {"op": "convert", "format": "jpeg"}], "output": {"suffix": "_j"}})).unwrap();
    assert_eq!(record(&jpeg, 0)["outputs"][0]["file"], "a_j.jpg");
}

#[test]
fn a_file_name_and_many_images_do_not_mix() {
    let d = scratch("file_many", &["batch/a.png", "batch/b.jpg"]);
    let e = call(&json!({"path": dir_str(&d.join("batch")), "op": "resize", "width": 8, "output": {"file": "x.png"}})).unwrap_err();
    assert!(e.contains("output.suffix"), "{e}");
}

#[test]
fn output_names_cannot_leave_the_folder_or_pass_a_link() {
    let d = scratch("escape_out", &["scene.png"]);
    let outside = d
        .parent()
        .unwrap()
        .join(format!("image_tools_outside_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let id = std::process::id();
    let (up, up2, abs) = (
        format!("../escape_{id}.png"),
        format!("sub/../../escape2_{id}.png"),
        format!("/tmp/escape_abs_{id}.png"),
    );
    for bad in [up.as_str(), up2.as_str(), abs.as_str(), "a\\b.png"] {
        let r = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": bad}})).unwrap();
        assert!(err(&r).contains("bound folder"), "{bad}: {}", err(&r));
    }
    let parent = d.parent().unwrap();
    assert!(!parent.join(format!("escape_{id}.png")).exists());
    assert!(!parent.join(format!("escape2_{id}.png")).exists());
    assert!(!Path::new(&abs).exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
        let r = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": "link/x.png"}})).unwrap();
        assert!(err(&r).contains("symbolic link"), "{}", err(&r));
        assert!(
            listing(&outside).is_empty(),
            "nothing was written through the link"
        );
    }
}

#[test]
fn a_bound_single_file_cannot_have_files_written_beside_it() {
    let d = scratch("single_out", &["scene.png"]);
    let r = call(&json!({"path": dir_str(&d.join("scene.png")), "op": "resize", "width": 8, "output": {"file": "x.png"}})).unwrap();
    assert!(err(&r).contains("bound folder"), "{}", err(&r));
}

#[cfg(unix)]
#[test]
fn a_read_only_folder_is_one_sentence_not_a_crash() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("ro", &["scene.png"]);
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = call(&json!({"path": dir_str(&d), "files": ["scene.png"], "op": "resize", "width": 8, "output": {"file": "x.png"}})).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    if record(&r, 0).get("error").is_some() {
        assert!(err(&r).contains("read-write access"), "{}", err(&r));
    } else {
        assert!(
            d.join("x.png").exists(),
            "running as an account that ignores permissions"
        );
    }
}

#[test]
fn tiles_can_be_written_as_files_and_reassemble() {
    let d = scratch("tilefiles", &["tile_src.png"]);
    let r = call(&json!({"path": dir_str(&d), "files": ["tile_src.png"], "op": "tile", "size": 64, "overlap": 8, "output": {"file": "tiles/t.png"}})).unwrap();
    assert!(r.get("parts").is_none());
    let outs = record(&r, 0)["outputs"].as_array().unwrap().clone();
    assert_eq!(outs.len(), 7, "the index and six tiles");
    assert_eq!(outs[0]["file"], "tiles/t_index.png");
    assert_eq!(outs[1]["file"], "tiles/t_r1c1.png");
    assert_eq!(outs[6]["file"], "tiles/t_r2c3.png");
    let mut canvas = RgbaImage::new(150, 100);
    for t in record(&r, 0)["steps"][0]["tiles"].as_array().unwrap() {
        let f = outs[t["output"].as_u64().unwrap() as usize]["file"]
            .as_str()
            .unwrap();
        let img = image::open(d.join(f)).unwrap().to_rgba8();
        image::imageops::replace(
            &mut canvas,
            &img,
            t["x"].as_i64().unwrap(),
            t["y"].as_i64().unwrap(),
        );
    }
    assert_eq!(canvas, fixture_image("tile_src.png"));
}

#[test]
fn every_hostile_file_gives_one_sentence_and_the_good_file_still_works() {
    let started = std::time::Instant::now();
    let names = [
        "truncated.png",
        "truncated.jpg",
        "truncated_scan.jpg",
        "empty.png",
        "text.png",
        "zero_size.png",
        "bomb.png",
        "bomb_wide.png",
        "bomb.gif",
        "bomb.bmp",
        "bomb.tif",
        "bomb.webp",
        "bomb.jpg",
        "ok.png",
    ];
    for op in [
        json!({"op": "info"}),
        json!({"op": "view"}),
        json!({"op": "hash"}),
        json!({"op": "decode_codes"}),
        json!({"op": "palette"}),
        json!({"ops": [{"op": "crop", "x": 0, "y": 0, "width": 4, "height": 4}, {"op": "view"}]}),
    ] {
        let mut req = json!({"path": format!("{}/hostile", fixtures()), "files": names});
        for (k, v) in op.as_object().unwrap() {
            req[k] = v.clone();
        }
        let r = call(&req).unwrap();
        let results = response(&r)["results"].as_array().unwrap();
        assert_eq!(results.len(), names.len());
        for (rec, name) in results.iter().zip(names) {
            assert_eq!(rec["source"], name);
            if name == "ok.png" {
                assert!(rec.get("error").is_none(), "{op}: {rec}");
                continue;
            }
            // info reads only the header, so a PNG cut short after its header is a clean info;
            // a JPEG is checked to its end marker, so a cut one fails for info too.
            let e = match rec["error"].as_str() {
                Some(e) => e,
                None if op["op"] == "info" && name == "truncated.png" => continue,
                None => panic!("{name} {op}: {rec}"),
            };
            assert!(
                !e.is_empty() && !e.contains('\n') && e.len() < 220,
                "{name}: {e}"
            );
            assert!(
                !e.to_lowercase().contains("panic") && !e.contains("src/"),
                "{name}: {e}"
            );
        }
    }
    assert!(
        started.elapsed().as_secs() < 20,
        "hostile inputs never hang"
    );
}

#[test]
fn bombs_and_zero_sized_images_are_refused_by_name_of_the_reason() {
    let h = |f: &str| {
        call(&json!({"path": format!("{}/hostile", fixtures()), "files": [f], "op": "info"}))
            .unwrap()
    };
    for f in [
        "bomb.png",
        "bomb_wide.png",
        "bomb.gif",
        "bomb.bmp",
        "bomb.tif",
        "bomb.webp",
        "bomb.jpg",
    ] {
        let r = h(f);
        let e = err(&r);
        assert!(
            e.contains("limit") || e.contains("corrupt") || e.contains("not supported"),
            "{f}: {e}"
        );
    }
    assert!(
        err(&h("bomb.png")).contains("over the 50000000 pixel limit")
            || err(&h("bomb.png")).contains("size limits")
    );
    assert!(err(&h("text.png")).contains("not a PNG, JPEG"));
    assert!(err(&h("empty.png")).contains("not a PNG, JPEG"));
}

#[test]
fn a_wrong_extension_is_fine_and_the_content_decides() {
    let r = call(&on_fixture("not_a_png.txt", json!({"op": "info"}))).unwrap();
    assert_eq!(record(&r, 0)["steps"][0]["format"], "png");
}

#[test]
fn names_that_leave_the_folder_fail_only_their_own_record() {
    let names = [
        "../scene.png",
        "/etc/passwd",
        "a/../../scene.png",
        "scene.png",
        "sub/../../../x.png",
    ];
    let r = call(&json!({"path": fixtures(), "files": names, "op": "info"})).unwrap();
    let results = response(&r)["results"].as_array().unwrap();
    for (i, rec) in results.iter().enumerate() {
        if names[i] == "scene.png" {
            assert!(rec.get("error").is_none());
        } else {
            assert!(
                rec["error"].as_str().unwrap().contains("bound folder"),
                "{}: {rec}",
                names[i]
            );
            assert_eq!(rec["source"], names[i]);
        }
    }
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_in_a_folder_is_skipped_in_listings_and_refused_by_name() {
    let d = scratch("links", &["scene.png"]);
    let outside = d
        .parent()
        .unwrap()
        .join(format!("image_tools_secret_{}.png", std::process::id()));
    std::fs::copy(format!("{}/scene.png", fixtures()), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, d.join("evil.png")).unwrap();
    let listed = call(&json!({"path": dir_str(&d), "op": "info"})).unwrap();
    let names: Vec<_> = response(&listed)["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["source"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["scene.png"]);
    assert!(
        response(&listed)["warnings"][0]
            .as_str()
            .unwrap()
            .contains("not images were left out")
    );
    let named = call(&json!({"path": dir_str(&d), "files": ["evil.png"], "op": "info"})).unwrap();
    assert!(err(&named).contains("symbolic link"), "{}", err(&named));
}

#[test]
fn a_folder_lists_images_in_name_order_and_counts_what_it_skipped() {
    let r = call(&json!({"path": format!("{}/batch", fixtures()), "op": "info"})).unwrap();
    let names: Vec<_> = response(&r)["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["source"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a.png", "b.jpg", "sub/c.png"]);
    assert_eq!(
        response(&r)["warnings"],
        json!(["1 entries in the folder that are hidden, links or not images were left out"])
    );
    let one =
        call(&json!({"path": format!("{}/batch", fixtures()), "file": "b.jpg", "op": "info"}))
            .unwrap();
    assert_eq!(response(&one)["results"].as_array().unwrap().len(), 1);
    let both = call(&json!({"path": format!("{}/batch", fixtures()), "file": "a.png", "files": ["b.jpg"], "op": "info"})).unwrap_err();
    assert!(both.contains("not both"));
}

#[test]
fn an_inline_image_works_like_a_file() {
    let bytes = std::fs::read(format!("{}/scene.jpg", fixtures())).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let r =
        call(&json!({"data_base64": b64, "name": "whatever.bin", "op": "view", "max_side": 16}))
            .unwrap();
    assert_eq!(record(&r, 0)["source"], "whatever.bin");
    assert_eq!(record(&r, 0)["input"]["format"], "jpeg");
    assert_eq!(part_image(&r, 0).width(), 16);
    let nameless = call(&json!({"data_base64": b64, "op": "info"})).unwrap();
    assert_eq!(record(&nameless, 0)["source"], "inline");
    assert!(
        call(&json!({"data_base64": "!!!", "op": "info"}))
            .unwrap_err()
            .contains("not valid base64")
    );
    assert!(
        call(&json!({"data_base64": b64, "path": fixtures(), "op": "info"}))
            .unwrap_err()
            .contains("not both")
    );
    let text = base64::engine::general_purpose::STANDARD.encode(b"hello");
    assert!(
        err(&call(&json!({"data_base64": text, "op": "info"})).unwrap())
            .contains("not a PNG, JPEG")
    );
}

#[test]
fn bad_requests_fail_the_call_with_one_sentence() {
    for (req, want) in [
        (json!({"op": "info"}), "path"),
        (
            json!({"path": "/no/such/place", "op": "info"}),
            "cannot read",
        ),
        (json!({"path": fixtures()}), "give an op"),
        (
            json!({"path": fixtures(), "op": "info", "bogus": 1}),
            "info",
        ),
        (
            json!({"path": fixtures(), "op": "view", "max_side": "big"}),
            "view",
        ),
        (
            json!({"path": fixtures(), "files": ["scene.png"], "op": "info", "cursor": "x"}),
            "cursor",
        ),
        (
            json!({"path": fixtures(), "files": ["scene.png"], "op": "montage", "cursor": "x"}),
            "montage",
        ),
    ] {
        let e = call(&req).unwrap_err();
        assert!(
            e.contains(want) && !e.contains('\n') && e.len() < 400,
            "{req}: {e}"
        );
    }
    assert!(
        crate::run_text("not json")
            .unwrap_err()
            .contains("not valid")
    );
    assert!(crate::run_text("[1,2]").unwrap_err().contains("not valid"));
    assert!(crate::run_text("").unwrap_err().contains("not valid"));
    let empty = scratch("empty_dir", &[]);
    assert!(
        call(&json!({"path": dir_str(&empty), "op": "info"}))
            .unwrap_err()
            .contains("holds no")
    );
}

#[test]
fn a_step_error_names_the_step_and_keeps_what_ran_before() {
    let r = call(&on_fixture("scene.png", json!({"ops": [{"op": "info"}, {"op": "crop", "x": 99, "y": 0, "width": 5, "height": 5}, {"op": "view"}]}))).unwrap();
    let rec = record(&r, 0);
    assert_eq!(rec["step"], 2);
    assert_eq!(rec["steps"].as_array().unwrap().len(), 1);
    assert_eq!(rec["steps"][0]["op"], "info");
    assert!(rec["outputs"].as_array().unwrap().is_empty());
    assert!(r.get("parts").is_none());
    assert_eq!(rec["input"]["width"], 32);
}

#[test]
fn oversize_outputs_are_refused_with_the_limit_in_one_sentence() {
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "resize", "mode": "exact", "width": 40_000, "height": 40_000}),
    ))
    .unwrap();
    assert!(err(&r).contains("pixel limit"), "{}", err(&r));
    let r = call(&on_fixture(
        "scene.png",
        json!({"op": "resize", "mode": "fit", "width": 20_000, "height": 20_000, "upscale": true}),
    ))
    .unwrap();
    assert!(err(&r).contains("pixel limit"), "{}", err(&r));
}

#[test]
fn an_image_too_big_to_attach_says_so_and_a_file_still_works() {
    let d = scratch("bigout", &[]);
    // Noise does not compress: 1400x1000 is far over what one call can attach.
    let noise: RgbaImage = crate::fixtures::noise(1400, 1000, 4);
    noise.save(d.join("n.png")).unwrap();
    let r =
        call(&json!({"path": dir_str(&d), "files": ["n.png"], "op": "convert", "format": "png"}))
            .unwrap();
    let e = err(&r);
    assert!(
        e.contains("over the") && e.contains("bytes one call can attach") && e.contains("view"),
        "{e}"
    );
    assert!(r.get("parts").is_none());
    let with_file = call(&json!({"path": dir_str(&d), "files": ["n.png"], "op": "convert", "format": "bmp", "output": {"file": "n.bmp", "part": true}})).unwrap();
    assert!(
        record(&with_file, 0).get("error").is_some(),
        "a BMP cannot be a part"
    );
    let png_out = call(&json!({"path": dir_str(&d), "files": ["n.png"], "op": "convert", "format": "png", "output": {"file": "n2.png", "part": true}})).unwrap();
    assert!(record(&png_out, 0).get("error").is_none(), "{png_out}");
    assert!(
        record(&png_out, 0)["warnings"]
            .to_string()
            .contains("written to the file")
    );
    assert!(d.join("n2.png").exists());
    assert!(png_out.get("parts").is_none());
    let view = call(&json!({"path": dir_str(&d), "files": ["n.png"], "op": "view"})).unwrap();
    assert!(part_bytes(&view, 0).len() <= 1_000_000);
    assert!(part_image(&view, 0).width() <= 1400);
    assert!(
        record(&view, 0)["warnings"]
            .to_string()
            .contains("to fit 1000000 bytes"),
        "a view that had to shrink says what it did"
    );
}

#[test]
fn a_big_photo_is_viewed_within_the_limits() {
    let d = scratch("photo", &[]);
    let big = crate::fixtures::scene(6000, 4000);
    std::fs::write(d.join("big.jpg"), crate::fixtures::jpeg(&big, 80)).unwrap();
    let started = std::time::Instant::now();
    let r = call(&json!({"path": dir_str(&d), "files": ["big.jpg"], "op": "view"})).unwrap();
    let img = part_image(&r, 0);
    assert_eq!((img.width(), img.height()), (1568, 1045));
    assert!(started.elapsed().as_secs() < 30);
    assert_eq!(record(&r, 0)["input"]["width"], 6000);
}

#[test]
fn a_36_megapixel_png_is_read_and_viewed() {
    let d = scratch("bigpng", &[]);
    // A flat colour compresses to a few KB even at this size.
    let flat = RgbaImage::from_pixel(6000, 6000, image::Rgba([10, 120, 200, 255]));
    flat.save(d.join("flat.png")).unwrap();
    drop(flat);
    let r =
        call(&json!({"path": dir_str(&d), "files": ["flat.png"], "op": "view", "max_side": 64}))
            .unwrap();
    let img = part_image(&r, 0);
    assert_eq!((img.width(), img.height()), (64, 64));
    assert_eq!(img.get_pixel(30, 30).0, [10, 120, 200, 255]);
}

#[test]
fn images_page_across_a_folder_with_nothing_repeated_or_skipped() {
    let d = scratch("paging", &[]);
    for i in 0..9u64 {
        crate::fixtures::noise(100, 100, 100 + i)
            .save(d.join(format!("n{i}.png")))
            .unwrap();
    }
    let req = |page: usize, cursor: Option<&str>| {
        let mut r = json!({"path": dir_str(&d), "op": "view", "page_bytes": page});
        if let Some(c) = cursor {
            r["cursor"] = json!(c);
        }
        r
    };
    let whole = call(&req(3_800_000, None)).unwrap();
    assert!(response(&whole).get("next").is_none());
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let r = call(&req(120_000, cursor.as_deref())).unwrap();
        cursor = response(&r)["next"]["cursor"].as_str().map(str::to_owned);
        pages.push(r);
        if cursor.is_none() {
            break;
        }
        assert!(pages.len() < 20);
    }
    assert!(pages.len() >= 3, "{} pages", pages.len());
    let sources = |rs: &[Value]| -> Vec<String> {
        rs.iter()
            .flat_map(|p| {
                response(p)["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x["source"].as_str().unwrap().to_owned())
            })
            .collect()
    };
    assert_eq!(sources(&pages), sources(std::slice::from_ref(&whole)));
    assert_eq!(
        sources(&pages),
        (0..9).map(|i| format!("n{i}.png")).collect::<Vec<_>>()
    );
    // The images themselves are the same whichever way they were paged.
    let all_parts: Vec<Vec<u8>> = pages
        .iter()
        .flat_map(|p| (0..p["parts"].as_array().unwrap().len()).map(|i| part_bytes(p, i)))
        .collect();
    let whole_parts: Vec<Vec<u8>> = (0..whole["parts"].as_array().unwrap().len())
        .map(|i| part_bytes(&whole, i))
        .collect();
    assert_eq!(all_parts, whole_parts);
    for p in &pages {
        let used = p.to_string().len();
        assert!(
            used < 4 * 1024 * 1024,
            "every page stays under the output ceiling"
        );
    }
}

#[test]
fn a_wide_folder_of_info_records_pages_by_response_size() {
    let d = scratch("manyinfo", &[]);
    let one = std::fs::read(format!("{}/scene.png", fixtures())).unwrap();
    for i in 0..120 {
        std::fs::write(d.join(format!("p{i:03}.png")), &one).unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut calls = 0;
    loop {
        let mut r = json!({"path": dir_str(&d), "op": "info", "page_bytes": 20_000});
        if let Some(c) = &cursor {
            r["cursor"] = json!(c);
        }
        let out = call(&r).unwrap();
        seen.extend(
            response(&out)["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["source"].as_str().unwrap().to_owned()),
        );
        cursor = response(&out)["next"]["cursor"].as_str().map(str::to_owned);
        calls += 1;
        if cursor.is_none() {
            break;
        }
        assert!(calls < 100);
    }
    assert!(calls > 1);
    assert_eq!(
        seen,
        (0..120).map(|i| format!("p{i:03}.png")).collect::<Vec<_>>()
    );
}

#[test]
fn an_image_that_fails_part_way_leaves_no_orphan_parts_in_the_result() {
    let d = scratch("orphans", &["tile_src.png"]);
    // The second tile's file is already there, so tile 2 fails after the index and tile 1 were attached.
    std::fs::write(d.join("t_r1c2.png"), b"in the way").unwrap();
    let r = call(&json!({"path": dir_str(&d), "files": ["tile_src.png"], "op": "tile", "size": 64, "overlap": 8,
        "output": {"file": "t.png", "part": true}}))
    .unwrap();
    assert!(err(&r).contains("t_r1c2.png already exists"), "{}", err(&r));
    assert!(
        r.get("parts").is_none(),
        "parts that no record refers to are dropped: {r}"
    );
    assert!(
        d.join("t_index.png").exists() && d.join("t_r1c1.png").exists(),
        "files already written stay"
    );
    assert_eq!(
        std::fs::read(d.join("t_r1c2.png")).unwrap(),
        b"in the way",
        "the file in the way is untouched"
    );
}
