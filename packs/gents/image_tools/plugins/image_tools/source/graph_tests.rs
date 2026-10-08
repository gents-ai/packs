//! Tests of graph mode: a job is planned into chunks, a chunk is run into a
//! result and its image records, and the records carry what the next node needs.
use base64::Engine as _;
use serde_json::{Value, json};

use crate::testkit::*;

fn node(fields: Value) -> Result<Value, String> {
    crate::run_text(&fields.to_string()).map(|t| serde_json::from_str(&t).expect("JSON"))
}

#[test]
fn only_a_request_with_a_run_id_is_a_graph_call() {
    assert!(crate::graph::run_node(r#"{"path":"/x","op":"info"}"#).is_none());
    assert!(crate::graph::run_node(r#"{"run_id":"r","path":"/x","ops":"info"}"#).is_some());
    let plain = call(&on_fixture("scene.png", json!({"op": "info"}))).unwrap();
    assert!(
        plain.get("results").is_some(),
        "no run_id: the ordinary result shape"
    );
}

#[test]
fn plan_makes_one_chunk_per_image_carrying_the_job_options() {
    let job = json!({"run_id": "r", "path": format!("{}/batch", fixtures()), "ops": "[{\"op\":\"view\",\"max_side\":64}]", "format": "", "quality": null, "files": []});
    let chunks = node(job).unwrap();
    let chunks = chunks.as_array().unwrap();
    assert_eq!(chunks.len(), 3);
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c["chunk"], i);
        assert_eq!(c["files"], json!([c["source"]]));
        assert_eq!(c["path"], format!("{}/batch", fixtures()));
        assert_eq!(c["ops"], "[{\"op\":\"view\",\"max_side\":64}]");
        assert!(
            c.get("format").is_none() && c.get("quality").is_none(),
            "unset fields are not carried"
        );
    }
    assert_eq!(
        chunks
            .iter()
            .map(|c| c["source"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a.png", "b.jpg", "sub/c.png"]
    );
}

#[test]
fn plan_accepts_ops_as_an_array_an_object_or_a_name_and_refuses_anything_else() {
    for ops in [
        "view",
        "[{\"op\":\"info\"}]",
        "{\"op\":\"hash\"}",
        "  info ",
    ] {
        let c =
            node(json!({"run_id": "r", "path": fixtures(), "files": ["scene.png"], "ops": ops}))
                .unwrap();
        assert_eq!(c.as_array().unwrap().len(), 1, "{ops}");
    }
    for (ops, want) in [
        ("sharpen", "ops must be"),
        ("[1,2]", "step 1"),
        ("[]", "between 1 and 16"),
        ("[{\"op\":\"crop\"}]", "crop"),
        ("{", "ops must be"),
    ] {
        let e =
            node(json!({"run_id": "r", "path": fixtures(), "files": ["scene.png"], "ops": ops}))
                .unwrap_err();
        assert!(e.contains(want), "{ops}: {e}");
    }
    let e = node(json!({"run_id": "r", "path": fixtures()})).unwrap_err();
    assert!(e.contains("needs ops"), "{e}");
}

#[test]
fn a_single_file_job_names_the_original_path_and_a_montage_job_is_one_chunk() {
    let file = format!("{}/scene.png", fixtures());
    let c = node(
        json!({"run_id": "r", "path": file, "path_original": "/real/scene.png", "ops": "info"}),
    )
    .unwrap();
    let chunk = &c[0];
    assert_eq!(chunk["path"], "/real/scene.png");
    assert_eq!(chunk["source"], "scene.png");
    assert!(chunk.get("files").is_none(), "{chunk}");
    let m =
        node(json!({"run_id": "r", "path": format!("{}/montage", fixtures()), "ops": "montage"}))
            .unwrap();
    assert_eq!(m.as_array().unwrap().len(), 1);
    assert_eq!(m[0]["source"], "montage");
    assert_eq!(m[0]["files"].as_array().unwrap().len(), 4);
}

#[test]
fn an_inline_image_from_another_node_becomes_one_chunk_with_the_data() {
    let bytes = std::fs::read(format!("{}/scene.png", fixtures())).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let c = node(
        json!({"run_id": "r", "data_base64": b64, "name": "from_other_node.png", "ops": "view"}),
    )
    .unwrap();
    assert_eq!(c[0]["name"], "from_other_node.png");
    assert_eq!(c[0]["data_base64"], b64);
    assert!(c[0].get("path").is_none() && c[0].get("files").is_none());
    let mut chunk = c[0].clone();
    chunk["run_id"] = json!("r");
    let out = node(chunk).unwrap();
    assert_eq!(out["result"]["ok"], true);
    assert_eq!(out["outputs"][0]["role"], "view");
    assert!(
        out["outputs"][0]["image_base64"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
    let too_big = "A".repeat(3_000_100);
    assert!(
        node(json!({"run_id": "r", "data_base64": too_big, "ops": "view"}))
            .unwrap_err()
            .contains("too large")
    );
}

#[test]
fn running_a_chunk_gives_a_result_and_its_image_records() {
    let chunks = node(json!({"run_id": "r", "path": fixtures(), "files": ["scene.png"], "ops": "[{\"op\":\"view\",\"max_side\":16}]"})).unwrap();
    let mut chunk = chunks[0].clone();
    chunk["run_id"] = json!("r");
    let out = node(chunk).unwrap();
    let res = &out["result"];
    assert_eq!(
        (
            res["chunk"].clone(),
            res["source"].clone(),
            res["ok"].clone(),
            res["complete"].clone()
        ),
        (json!(0), json!("scene.png"), json!(true), json!(true))
    );
    assert_eq!(
        (
            res["input_format"].clone(),
            res["input_width"].clone(),
            res["input_height"].clone(),
            res["outputs"].clone()
        ),
        (json!("png"), json!(32), json!(24), json!(1))
    );
    let steps: Value = serde_json::from_str(res["steps"].as_str().unwrap()).unwrap();
    assert_eq!(steps[0]["op"], "view");
    let o = &out["outputs"][0];
    assert_eq!(
        (
            o["role"].clone(),
            o["format"].clone(),
            o["mime"].clone(),
            o["width"].clone(),
            o["height"].clone()
        ),
        (
            json!("view"),
            json!("png"),
            json!("image/png"),
            json!(16),
            json!(12)
        )
    );
    let img = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(o["image_base64"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
    .to_rgba8();
    assert_eq!(pixels_sha(&img), o["pixels_sha256"]);
    assert!(
        out.get("parts").is_none(),
        "a graph result carries images in records, not as parts"
    );
    assert_eq!(
        res["warnings"],
        json!(["scaled from 32x24 to 16x12 to fit 16 pixels on the longest side"])
    );
}

#[test]
fn an_image_that_cannot_be_read_is_a_result_with_an_error_not_a_failed_run() {
    let out = node(json!({"run_id": "r", "chunk": 3, "path": format!("{}/hostile", fixtures()), "files": ["truncated.png"], "source": "truncated.png", "ops": "view"})).unwrap();
    assert_eq!(out["result"]["ok"], false);
    assert_eq!(out["result"]["chunk"], 3);
    assert!(
        out["result"]["error"]
            .as_str()
            .unwrap()
            .contains("corrupt or truncated")
    );
    assert_eq!(out["outputs"], json!([]));
    let bad = node(json!({"run_id": "r", "chunk": 0, "path": fixtures(), "files": ["scene.png"], "source": "scene.png", "ops": "sharpen"})).unwrap();
    assert_eq!(bad["result"]["ok"], false);
    assert!(
        bad["result"]["error"]
            .as_str()
            .unwrap()
            .contains("ops must be")
    );
}

#[test]
fn a_big_tile_chunk_returns_a_cursor_and_the_follow_up_finishes_it() {
    let d = scratch("graphtiles", &[]);
    crate::fixtures::noise(2048, 2048, 77)
        .save(d.join("big.png"))
        .unwrap();
    let base = json!({"run_id": "r", "chunk": 0, "path": d.to_str().unwrap(), "files": ["big.png"], "source": "big.png",
        "ops": "[{\"op\":\"tile\",\"size\":512,\"overlap\":0,\"max_bytes\":400000}]"});
    let first = node(base.clone()).unwrap();
    assert_eq!(
        first["result"]["complete"], false,
        "sixteen noisy tiles do not fit one run"
    );
    let mut runs = vec![first];
    while runs.last().unwrap()["result"]["complete"] == false {
        let mut follow = base.clone();
        follow["cursor"] = runs.last().unwrap()["result"]["cursor"].clone();
        runs.push(node(follow).unwrap());
        assert!(runs.len() < 10, "paging must end");
    }
    let mut all = Vec::new();
    for o in &runs {
        all.extend(
            o["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["role"] == "tile")
                .map(|r| r["tile"].as_u64().unwrap()),
        );
    }
    assert!(runs.len() >= 2);
    assert_eq!(
        all,
        (1..=16).collect::<Vec<_>>(),
        "every tile once, in order, across the runs"
    );
    assert_eq!(runs[0]["outputs"][0]["role"], "index");
    assert!(
        runs[1..].iter().all(|o| o["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["role"] != "index")),
        "the index comes with the first run only"
    );
}

#[test]
fn a_small_tile_chunk_is_complete_with_its_index() {
    let base = json!({"run_id": "r", "chunk": 0, "path": fixtures(), "files": ["tile_src.png"], "source": "tile_src.png", "ops": "[{\"op\":\"tile\",\"size\":64,\"overlap\":8}]"});
    let one = node(base).unwrap();
    assert_eq!(one["result"]["complete"], true);
    assert!(one["result"].get("cursor").is_none());
    assert_eq!(
        one["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["role"] == "tile")
            .count(),
        6
    );
    assert_eq!(one["outputs"][0]["role"], "index");
}

#[test]
fn unreadable_jobs_fail_the_plan_with_one_sentence() {
    let e = node(json!({"run_id": "r", "path": "/no/such/dir", "ops": "info"})).unwrap_err();
    assert!(e.contains("cannot read"), "{e}");
    let e = node(json!({"run_id": "r", "path": format!("{}/hostile", fixtures()), "files": ["../x.png"], "ops": "info"})).unwrap();
    assert_eq!(
        e[0]["source"], "../x.png",
        "a refused name still plans; its chunk reports the reason"
    );
    let mut chunk = e[0].clone();
    chunk["run_id"] = json!("r");
    let out = node(chunk).unwrap();
    assert_eq!(out["result"]["ok"], false);
    assert!(
        out["result"]["error"]
            .as_str()
            .unwrap()
            .contains("bound folder")
    );
}

#[test]
fn a_graph_output_wider_than_a_model_accepts_keeps_its_bytes() {
    let d = scratch("graphwide", &[]);
    image::RgbaImage::from_pixel(9000, 20, image::Rgba([200, 30, 30, 255]))
        .save(d.join("strip.png"))
        .unwrap();
    let dir = d.to_str().unwrap();
    let chunks = node(json!({"run_id": "r", "path": dir, "files": ["strip.png"], "ops": "[{\"op\":\"flip\",\"axis\":\"vertical\"}]"})).unwrap();
    let mut chunk = chunks[0].clone();
    chunk["run_id"] = json!("r");
    let out = node(chunk).unwrap();
    assert_eq!(out["result"]["ok"], true, "{out}");
    let rec = &out["outputs"][0];
    assert_eq!(
        (rec["width"].clone(), rec["mime"].clone()),
        (json!(9000), json!("image/png"))
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(rec["image_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 9000);
}
