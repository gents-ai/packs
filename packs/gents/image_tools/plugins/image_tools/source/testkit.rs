//! Helpers shared by the end-to-end tests: call the plugin the way the host
//! does (JSON text in, JSON text out), decode the parts it returns, and make
//! scratch folders.
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::{Value, json};

/// The committed fixtures folder.
pub fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

/// Calls the plugin with `request`; the result as JSON, or the error sentence.
pub fn call(request: &Value) -> Result<Value, String> {
    crate::run_text(&request.to_string())
        .map(|t| serde_json::from_str(&t).expect("the plugin prints JSON"))
}

/// The response object of a result, with or without parts around it.
pub fn response(v: &Value) -> &Value {
    v.get("response").unwrap_or(v)
}

/// The `n`th record of a result.
pub fn record(v: &Value, n: usize) -> &Value {
    &response(v)["results"][n]
}

/// The decoded bytes of part `i`.
pub fn part_bytes(v: &Value, i: usize) -> Vec<u8> {
    let data = v["parts"][i]["data"].as_str().expect("a part with data");
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64")
}

/// Part `i` decoded to pixels.
pub fn part_image(v: &Value, i: usize) -> image::RgbaImage {
    image::load_from_memory(&part_bytes(v, i))
        .expect("a decodable part")
        .to_rgba8()
}

/// A fixture decoded to pixels.
pub fn fixture_image(name: &str) -> image::RgbaImage {
    image::open(format!("{}/{name}", fixtures()))
        .expect("a fixture")
        .to_rgba8()
}

/// SHA-256 hex of the RGBA bytes of an image, as `pixels_sha256` reports.
pub fn pixels_sha(img: &image::RgbaImage) -> String {
    crate::model::sha256_hex(img.as_raw())
}

/// A fresh scratch folder holding a copy of the files under `names` of the fixtures.
pub fn scratch(tag: &str, names: &[&str]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("image_tools_e2e_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch folder");
    for n in names {
        let to = d.join(n);
        std::fs::create_dir_all(to.parent().expect("parent")).expect("scratch subfolder");
        std::fs::copy(Path::new(&fixtures()).join(n), to).expect("copying a fixture");
    }
    d
}

/// A request over the fixtures folder for one file and a step.
pub fn on_fixture(file: &str, step: Value) -> Value {
    let mut req = json!({"path": fixtures(), "files": [file]});
    for (k, v) in step.as_object().expect("a step object") {
        req[k] = v.clone();
    }
    req
}
