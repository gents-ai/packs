//! Graph mode: the plugin as the `image-plan` and `image-run` nodes of a
//! graph. A graph node sends the fields of one document and always includes
//! the run's `run_id`; no other caller does, so its presence selects this
//! mode. An `ImageJob` (no `chunk` field) is planned into `ImageChunk`
//! records, one per image (or one for all of them when the job is a montage);
//! an `ImageChunk` is run into one `ImageResult` plus its `ImageOutput`
//! records, which carry each produced image as base64. The records are
//! documented in TOOL.md.
use serde_json::{Map, Value, json};

use crate::input::{Input, OP_NAMES};
use crate::run;
use crate::src::resolve;

/// The graph runs at most 1024 invocations: the plan and one run per chunk.
const MAX_CHUNKS: usize = 1000;
/// Largest inline image, as base64 text, a chunk record carries.
const MAX_INLINE: usize = 3_000_000;
/// Result bytes one run returns before it hands back a cursor.
const RUN_PAGE_BYTES: usize = 3_000_000;

/// Runs a graph node's request, or returns `None` when `raw` is not one.
pub fn run_node(raw: &str) -> Option<Result<String, String>> {
    if !raw.contains("\"run_id\"") {
        return None;
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_str(raw) else {
        return None;
    };
    fields.get("run_id")?;
    // A document's unset fields come back as null, an empty string or an empty list.
    fields.retain(|_, v| {
        !(v.is_null() || v.as_str() == Some("") || v.as_array().is_some_and(Vec::is_empty))
    });
    Some(match fields.get("chunk").and_then(Value::as_u64) {
        None => plan(&fields),
        Some(chunk) => run_chunk(fields, chunk),
    })
}

/// The steps a job names: a JSON array of step objects, one step object, or one bare op name.
fn parse_ops(text: &str) -> Result<Vec<Value>, String> {
    let t = text.trim();
    let bad = || {
        format!(
            "ops must be a JSON array of steps, one step object, or one op name such as view; the ops are {OP_NAMES}"
        )
    };
    match t.chars().next() {
        Some('[') => match serde_json::from_str::<Value>(t) {
            Ok(Value::Array(a)) => Ok(a),
            _ => Err(bad()),
        },
        Some('{') => match serde_json::from_str::<Value>(t) {
            Ok(v @ Value::Object(_)) => Ok(vec![v]),
            _ => Err(bad()),
        },
        Some(_) if t == "contact_sheet" || OP_NAMES.split(", ").any(|n| n == t) => {
            Ok(vec![json!({"op": t})])
        }
        _ => Err(bad()),
    }
}

/// The ordinary request a job or chunk stands for.
fn request(f: &Map<String, Value>) -> Result<Value, String> {
    let ops = parse_ops(
        f.get("ops")
            .and_then(Value::as_str)
            .ok_or("the job needs ops: the steps to run, such as [{\"op\":\"view\"}]")?,
    )?;
    let mut req = Map::new();
    for k in [
        "path",
        "path_original",
        "files",
        "data_base64",
        "name",
        "frame",
        "orient",
        "cursor",
    ] {
        if let Some(v) = f.get(k) {
            req.insert(k.into(), v.clone());
        }
    }
    req.insert("ops".into(), Value::Array(ops));
    let mut out = Map::new();
    for (from, to) in [
        ("format", "format"),
        ("quality", "quality"),
        ("suffix", "suffix"),
        ("overwrite", "overwrite"),
        ("attach", "part"),
    ] {
        if let Some(v) = f.get(from) {
            out.insert(to.into(), v.clone());
        }
    }
    if !out.is_empty() {
        req.insert("output".into(), Value::Object(out));
    }
    Ok(Value::Object(req))
}

/// Splits a job into the chunks that cover every image once.
fn plan(job: &Map<String, Value>) -> Result<String, String> {
    let req = request(job)?;
    let input: Input = crate::typed::from_value(req.clone(), "the job", &[])?;
    let steps = input.plan()?;
    let files: Vec<String> = job
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let path = job.get("path").and_then(Value::as_str);
    let inline = job.get("data_base64").and_then(Value::as_str);
    if let Some(b) = inline
        && b.len() > MAX_INLINE
    {
        return Err(
            "data_base64 is too large for a job record; bind the file with path instead".into(),
        );
    }
    let resolved = resolve(
        path,
        &files,
        inline,
        job.get("name").and_then(Value::as_str),
    )?;
    let single = path.is_some_and(|p| std::path::Path::new(p).is_file());
    let chunk_path = job.get("path_original").and_then(Value::as_str).or(path);
    let groups: Vec<Vec<&crate::src::Source>> = if steps.is_montage() {
        vec![resolved.sources.iter().collect()]
    } else {
        resolved.sources.iter().map(|s| vec![s]).collect()
    };
    if groups.len() > MAX_CHUNKS {
        return Err(format!(
            "the job holds {} images, over the {MAX_CHUNKS} one run reads; name fewer in files",
            groups.len()
        ));
    }
    let mut chunks = Vec::new();
    for (i, group) in groups.iter().enumerate() {
        let mut c = Map::new();
        c.insert("chunk".into(), json!(i));
        if let Some(p) = chunk_path {
            c.insert("path".into(), json!(p));
        }
        if inline.is_some() {
            c.insert("data_base64".into(), json!(inline));
            c.insert("name".into(), json!(group[0].name));
        } else if !single {
            c.insert(
                "files".into(),
                json!(group.iter().map(|s| s.name.clone()).collect::<Vec<_>>()),
            );
        }
        c.insert(
            "source".into(),
            json!(if steps.is_montage() {
                "montage".to_owned()
            } else {
                group[0].name.clone()
            }),
        );
        for k in [
            "ops",
            "format",
            "quality",
            "suffix",
            "overwrite",
            "attach",
            "orient",
            "frame",
        ] {
            if let Some(v) = job.get(k) {
                c.insert(k.into(), v.clone());
            }
        }
        chunks.push(Value::Object(c));
    }
    Ok(Value::Array(chunks).to_string())
}

fn failed(chunk: u64, source: &str, error: String) -> String {
    json!({"result": {"chunk": chunk, "source": source, "ok": false, "error": error, "outputs": 0, "complete": false}, "outputs": []}).to_string()
}

/// Runs one chunk into its result and output records.
fn run_chunk(f: Map<String, Value>, chunk: u64) -> Result<String, String> {
    let source = f
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let outcome = request(&f).and_then(|mut req| {
        req["page_bytes"] = json!(RUN_PAGE_BYTES);
        let input: Input = crate::typed::from_value(req, "the chunk", &[])?;
        run::run(&input)
    });
    let done = match outcome {
        Ok(d) => d,
        // A request the plugin refuses is a result with an error, not a failed run.
        Err(e) => return Ok(failed(chunk, &source, e)),
    };
    let item = &done.response["results"][0];
    let mut records = Vec::new();
    for o in item["outputs"].as_array().into_iter().flatten() {
        let mut r = json!({"chunk": chunk, "source": source});
        for k in [
            "role",
            "tile",
            "format",
            "width",
            "height",
            "bytes",
            "sha256",
            "pixels_sha256",
            "file",
        ] {
            if !o[k].is_null() {
                r[k] = o[k].clone();
            }
        }
        if let Some(part) = o["part"].as_u64().and_then(|i| done.parts.get(i as usize)) {
            r["image_base64"] = part["data"].clone();
            r["mime"] = part["mimeType"].clone();
        }
        records.push(r);
    }
    let mut warnings: Vec<Value> = item["warnings"].as_array().cloned().unwrap_or_default();
    warnings.extend(
        done.response["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );
    let mut result = json!({
        "chunk": chunk, "source": source,
        "ok": item["error"].is_null(),
        "steps": item["steps"].to_string(),
        "warnings": warnings,
        "outputs": records.len(),
        "complete": done.response["next"].is_null(),
    });
    if let Some(e) = item["error"].as_str() {
        result["error"] = json!(e);
    }
    for (to, from) in [
        ("input_format", "format"),
        ("input_width", "width"),
        ("input_height", "height"),
    ] {
        if !item["input"][from].is_null() {
            result[to] = item["input"][from].clone();
        }
    }
    if let Some(c) = done.response["next"]["cursor"].as_str() {
        result["cursor"] = json!(c);
    }
    Ok(json!({"result": result, "outputs": records}).to_string())
}
