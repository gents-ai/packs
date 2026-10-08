//! Graph mode: the plugin as the `plan` and `extract` nodes of the pack's
//! `ocr` graph. A graph node sends the fields of one document and always
//! includes the run's `run_id`; no other caller does, so its presence selects
//! this mode. An `OcrJob` (no `chunk` field) is planned into `OcrChunk`
//! records; an `OcrChunk` is read into one `OcrDocument` plus its `OcrPage`
//! and `OcrFigure` records. The records are documented in TOOL.md. Every other
//! field is the ordinary input, so the two modes cannot drift apart.
use std::path::Path;
use std::time::Instant;

use serde_json::{Map, Value, json};

/// Content bytes one extract call reads. The Markdown is written twice (the
/// document and its pages) and must fit the 4 MiB output ceiling beside the
/// figure images, which are counted in the same budget.
const EXTRACT_MAX_BYTES: u64 = 1_500_000;
/// The graph runs at most 1024 invocations: the plan and one extract per chunk.
const MAX_CHUNKS: usize = 1000;
/// Fields the graph adds; the ordinary input does not know them.
const GRAPH_ONLY: [&str; 12] = [
    "run_id",
    "chunk",
    "source",
    "format",
    "chunk_id",
    "expected_total",
    "sources_json",
    "book_id",
    "graph_state",
    "read_id",
    "access",
    "license",
];
/// Options a chunk repeats from its job.
const OPTIONS: [&str; 5] = [
    "ocr",
    "remote_ocr",
    "figure_images",
    "max_image_px",
    "min_figure_px",
];
/// Formats whose converters write one marker comment per page, slide, sheet or section.
const PAGED: [&str; 6] = ["pdf", "epub", "pptx", "xlsx", "ods", "odp"];

/// Runs a graph node's request, or returns `None` when `raw` is not one.
pub fn run(raw: &str, started: Instant) -> Option<Result<String, String>> {
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
        None => plan(&fields, started),
        Some(chunk) => extract(fields, chunk, started),
    })
}

/// Splits a job into the chunks that cover every file once. A job names a
/// folder, or one file: for one file the host gives `path` as a link that is
/// gone after the call, so its chunks name the file itself (`path_original`)
/// and the next call binds it again.
fn plan(job: &Map<String, Value>, started: Instant) -> Result<String, String> {
    let path = job.get("path").and_then(Value::as_str).unwrap_or_default();
    let single = Path::new(path).is_file();
    let chunk_path = job
        .get("path_original")
        .and_then(Value::as_str)
        .unwrap_or(path);
    let mut request = job.clone();
    request.insert("mode".into(), json!("plan"));
    request.retain(|k, _| !GRAPH_ONLY.contains(&k.as_str()));
    request.entry("remote_ocr").or_insert(json!("off"));
    let listing = crate::run_at(&Value::Object(request).to_string(), started)?;
    let listing: Value =
        serde_json::from_str(&listing).map_err(|e| format!("reading the plan: {e}"))?;
    if listing["omitted"].as_u64().unwrap_or(0) > 0 {
        return Err(
            "the job holds more files than one plan lists; name the files to read in files".into(),
        );
    }
    let access = job
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or("local_only");
    let license = job
        .get("license")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if !matches!(access, "open" | "local_only" | "restricted")
        || (access == "open" && (license.trim().is_empty() || license == "unknown"))
    {
        return Err(
            "access must be open/local_only/restricted; open requires an explicit license".into(),
        );
    }
    let mut chunks = Vec::new();
    for entry in listing["documents"].as_array().into_iter().flatten() {
        let ranges: Vec<&str> = match entry["chunks"].as_array() {
            Some(c) if !c.is_empty() => c.iter().filter_map(|c| c["pages"].as_str()).collect(),
            _ => vec![""],
        };
        for pages in ranges {
            let mut chunk = Map::new();
            chunk.insert("chunk".into(), json!(chunks.len()));
            chunk.insert("path".into(), json!(chunk_path));
            if !single {
                chunk.insert("files".into(), json!([entry["source"]]));
            }
            chunk.insert("source".into(), entry["source"].clone());
            chunk.insert("format".into(), entry["format"].clone());
            if !pages.is_empty() {
                chunk.insert("pages".into(), json!(pages));
            }
            for key in OPTIONS {
                if let Some(v) = job.get(key) {
                    chunk.insert(key.into(), v.clone());
                }
            }
            chunk.entry("remote_ocr").or_insert(json!("off"));
            chunks.push(Value::Object(chunk));
        }
    }
    if chunks.len() > MAX_CHUNKS {
        return Err(format!(
            "the job needs {} chunks, over the {MAX_CHUNKS} one run reads; name fewer files in files or read a page range per job",
            chunks.len()
        ));
    }
    let sources: Vec<Value> = listing["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| json!({"source":d["source"], "page_count":d["count"].as_u64().unwrap_or(1)}))
        .collect();
    let manifest = serde_json::to_string(&sources).map_err(|e| e.to_string())?;
    let total = chunks.len();
    for chunk in &mut chunks {
        chunk["chunk_id"] = json!(format!(
            "{}:{}",
            job["run_id"].as_str().unwrap_or_default(),
            chunk["chunk"]
        ));
        chunk["expected_total"] = json!(total);
        chunk["sources_json"] = json!(manifest);
        chunk["access"] = json!(access);
        chunk["license"] = json!(license);
        chunk["book_id"] = job.get("book_id").unwrap_or(&job["run_id"]).clone();
    }
    Ok(Value::Array(chunks).to_string())
}

/// Reads one chunk into its document, page and figure records.
fn extract(mut chunk: Map<String, Value>, index: u64, started: Instant) -> Result<String, String> {
    let original = chunk.clone();
    let source = chunk["source"].as_str().unwrap_or_default().to_owned();
    chunk.retain(|k, _| !GRAPH_ONLY.contains(&k.as_str()));
    chunk.insert("max_bytes".into(), json!(EXTRACT_MAX_BYTES));
    let read = crate::run_at(&Value::Object(chunk).to_string(), started);
    // A model call round goes to the host as it is; the host calls again with the answers.
    if let Ok(raw) = &read
        && raw.starts_with("{\"model_calls\"")
    {
        return read;
    }
    let mut document = json!({"chunk": index, "source": source, "page_count":0, "error":""});
    for key in [
        "chunk_id",
        "expected_total",
        "sources_json",
        "book_id",
        "access",
        "license",
    ] {
        if let Some(value) = original.get(key) {
            document[key] = value.clone();
        }
    }
    let (mut pages, mut figures) = (Vec::new(), Vec::new());
    match read.and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string())) {
        Err(why) => {
            document["format"] = json!("unknown");
            document["complete"] = json!(false);
            document["error"] = json!(why);
        }
        Ok(out) => {
            let body = out.get("response").unwrap_or(&out);
            let doc = &body["documents"][0];
            let format = doc["format"].as_str().unwrap_or("unknown");
            let markdown = doc["markdown"].as_str().unwrap_or_default();
            document["format"] = json!(format);
            document["page_count"] = doc["pages"].clone();
            document["markdown"] = json!(markdown);
            document["joint"] = doc["joint"].clone();
            document["complete"] = json!(body["next"].is_null());
            if let Some(cursor) = body["next"]["cursor"].as_str() {
                document["cursor"] = json!(cursor);
            }
            document["warnings"] = doc["warnings"].clone();
            for (page, text) in split_pages(format, markdown) {
                pages.push(
                    json!({"chunk": index, "source": source, "page": page, "markdown": text}),
                );
            }
            for fig in doc["figures"].as_array().into_iter().flatten() {
                let mut record = json!({
                    "chunk": index, "source": source, "figure": fig["id"],
                    "caption": fig["caption"], "text": fig["text"],
                    "width": fig["width"], "height": fig["height"], "ocr": fig["ocr"],
                });
                if !fig["page"].is_null() {
                    record["page"] = fig["page"].clone();
                }
                if let Some(part) = fig["part"]
                    .as_u64()
                    .and_then(|i| out["parts"].get(i as usize))
                {
                    record["image_base64"] = part["data"].clone();
                    record["mime"] = part["mimeType"].clone();
                }
                figures.push(record);
            }
        }
    }
    let failure = |error: String| {
        let mut failed = document.clone();
        failed["complete"] = json!(false);
        failed["extraction_state"] = json!("failed");
        failed["error"] = json!(error);
        failed["page_count"] = json!(0);
        failed["markdown"] = json!("");
        failed.as_object_mut().unwrap().remove("joint");
        failed.as_object_mut().unwrap().remove("cursor");
        json!({"document":failed,"pages":[],"figures":[],"continuation":null}).to_string()
    };
    match continue_read(original, document.clone(), pages, figures) {
        Err(error) => Ok(failure(error)),
        Ok(result) => {
            let out = result.to_string();
            if out.len() > crate::model::OUTPUT_CAP_BYTES + 400_000 {
                Ok(failure(
                    "chunk records exceed the output limit; reduce the chunk or figure images"
                        .into(),
                ))
            } else {
                Ok(out)
            }
        }
    }
}

/// Partial output is held on a durable continuation request. Only the final
/// read publishes pages and the chunk receipt, so repeated page fragments cannot
/// be mistaken for separate completed pages by downstream grouped stages.
fn continue_read(
    mut input: Map<String, Value>,
    mut document: Value,
    pages: Vec<Value>,
    mut figures: Vec<Value>,
) -> Result<Value, String> {
    if document["complete"] == false && document["cursor"].is_null() {
        document["extraction_state"] = json!("failed");
        document["markdown"] = json!("");
        document["page_count"] = json!(0);
        document.as_object_mut().unwrap().remove("joint");
        return Ok(json!({"document":document,"pages":[],"figures":[],"continuation":null}));
    }
    let prior: Value = match input.get("graph_state").and_then(Value::as_str) {
        Some(raw) => serde_json::from_str(raw).map_err(|e| format!("invalid graph state: {e}"))?,
        None => json!({"markdown":"", "figures":[], "warnings":[], "round":0}),
    };
    let joint = match document
        .as_object_mut()
        .unwrap()
        .remove("joint")
        .as_ref()
        .and_then(Value::as_str)
    {
        Some("none") => "",
        Some("line") => "\n",
        _ => "\n\n",
    };
    let previous = prior["markdown"].as_str().unwrap_or_default();
    let markdown = format!(
        "{}{}{}",
        previous,
        if previous.is_empty() { "" } else { joint },
        document["markdown"].as_str().unwrap_or_default()
    );
    let mut all_figures = prior["figures"].as_array().cloned().unwrap_or_default();
    all_figures.append(&mut figures);
    let mut warnings = prior["warnings"].as_array().cloned().unwrap_or_default();
    warnings.extend(
        document["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .cloned(),
    );
    let round = prior["round"].as_u64().unwrap_or(0) + 1;
    if let Some(cursor) = document["cursor"].as_str() {
        if round >= 128 || input.get("cursor").and_then(Value::as_str) == Some(cursor) {
            return Err(
                "OCR continuation made no progress or exceeded 128 reads; reduce the chunk".into(),
            );
        }
        let state =
            json!({"markdown":markdown,"figures":all_figures,"warnings":warnings,"round":round})
                .to_string();
        if state.len() > 1_500_000 {
            return Err("OCR accumulated chunk exceeds 1.5 MB; use smaller page ranges".into());
        }
        input.remove("model_calls");
        input.remove("model_results");
        input.remove("state");
        if let Some(path) = input.remove("path_original") {
            input.insert("path".into(), path);
        }
        input.insert("cursor".into(), json!(cursor));
        input.insert("graph_state".into(), json!(state));
        input.insert(
            "read_id".into(),
            json!(format!(
                "{}:{}:{round}",
                input["run_id"].as_str().unwrap_or_default(),
                input["chunk"]
            )),
        );
        input.remove("run_id");
        return Ok(json!({"continuation":input,"document":null,"pages":[],"figures":[]}));
    }
    let mut merged: Vec<Value> = Vec::new();
    if !markdown.is_empty() {
        for (page, text) in split_pages(document["format"].as_str().unwrap_or_default(), &markdown)
        {
            merged.push(json!({"chunk":document["chunk"],"source":document["source"],"page":page,"markdown":text}));
        }
    } else {
        merged = pages;
    }
    if document["complete"] == true && document["format"] == "pdf" {
        if let Some(range) = input.get("pages").and_then(Value::as_str) {
            let (a, b) = range.split_once('-').unwrap_or((range, range));
            let lo = a.parse::<u64>().map_err(|_| "invalid planned page range")?;
            let hi = b.parse::<u64>().map_err(|_| "invalid planned page range")?;
            let actual: Vec<_> = merged.iter().filter_map(|p| p["page"].as_u64()).collect();
            if actual != (lo..=hi).collect::<Vec<_>>() {
                document["complete"] = json!(false);
                document["error"] = json!("extracted pages do not exactly cover the planned range");
            }
        }
    }
    document["extraction_state"] = json!(if document["complete"] == true {
        "complete"
    } else {
        "failed"
    });
    document["markdown"] = json!(markdown);
    document["warnings"] = json!(warnings);
    Ok(json!({"document":document,"pages":merged,"figures":all_figures,"continuation":null}))
}

/// The text of each page, slide, sheet or section, from its marker comment to
/// the next; any other format is one page.
fn split_pages(format: &str, markdown: &str) -> Vec<(u32, String)> {
    if !PAGED.contains(&format) {
        return vec![(1, markdown.to_owned())];
    }
    let mut out: Vec<(u32, String)> = Vec::new();
    for line in markdown.split('\n') {
        match (marker(line), out.last_mut()) {
            (Some(n), _) => out.push((n, line.to_owned())),
            (None, Some((_, text))) => {
                text.push('\n');
                text.push_str(line);
            }
            (None, None) => {}
        }
    }
    for (_, text) in &mut out {
        text.truncate(text.trim_end().len());
    }
    out
}

/// The unit number of a `<!-- page 3 -->`, `slide`, `sheet 2: name` or `section` marker line.
fn marker(line: &str) -> Option<u32> {
    let inner = line.strip_prefix("<!-- ")?.strip_suffix(" -->")?;
    let (kind, rest) = inner.split_once(' ')?;
    if !matches!(kind, "page" | "slide" | "sheet" | "section") {
        return None;
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> String {
        format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
    }

    fn call(fields: Value) -> Result<Value, String> {
        run(&fields.to_string(), Instant::now())
            .expect("a graph request")
            .map(|out| serde_json::from_str(&out).expect("JSON"))
    }

    #[test]
    fn failed_continuations_publish_a_failure_receipt_without_fake_pages() {
        let mut input = json!({"run_id":"r","chunk":1,"path":fixtures(),"files":["missing.pdf"],
            "source":"missing.pdf","format":"pdf","pages":"21-40",
            "graph_state":json!({"markdown":"<!-- page 21 -->\nPartial text.","figures":[],"warnings":[],"round":1}).to_string()});
        let out = call(input.clone()).unwrap();
        assert_eq!(out["document"]["extraction_state"], "failed");
        assert_eq!(out["pages"], json!([]));
        input["files"] = json!(["text.pdf"]);
        input["source"] = json!("text.pdf");
        input["pages"] = json!("1-2");
        input["graph_state"] = json!("invalid state");
        let out = call(input).unwrap();
        assert_eq!(out["document"]["extraction_state"], "failed");
        assert!(
            out["document"]["error"]
                .as_str()
                .unwrap()
                .contains("graph state")
        );
    }
    #[test]
    fn only_a_request_with_a_run_id_is_a_graph_call() {
        assert!(run(r#"{"path":"/x"}"#, Instant::now()).is_none());
        assert!(run(r#"{"run_id":"r","path":"/x"}"#, Instant::now()).is_some());
    }

    #[test]
    fn plan_lists_every_file_once_and_extract_reads_each_chunk() {
        let job = json!({"run_id": "r", "path": fixtures(), "files": ["many.pdf", "data.csv"],
            "ocr": "never", "pages": null, "figure_images": null});
        let chunks = call(job).unwrap();
        let chunks = chunks.as_array().unwrap();
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        let pdf: Vec<_> = chunks
            .iter()
            .filter(|c| c["source"] == "many.pdf")
            .collect();
        assert_eq!(pdf[0]["pages"], "1-6");
        assert_eq!(chunks.last().unwrap()["source"], "data.csv");
        assert!(chunks.last().unwrap().get("pages").is_none());
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c["chunk"], i);
            assert_eq!(c["files"], json!([c["source"]]));
        }
        let mut chunk = chunks[0].clone();
        chunk["run_id"] = json!("r");
        let out = call(chunk).unwrap();
        assert_eq!(out["document"]["complete"], true);
        assert_eq!(out["document"]["format"], "pdf");
        let pages = out["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 6);
        assert_eq!(pages[0]["page"], 1);
        assert!(
            pages[0]["markdown"]
                .as_str()
                .unwrap()
                .starts_with("<!-- page 1 -->")
        );
        assert_eq!(pages[5]["page"], 6);
    }

    #[test]
    fn extract_records_figures_and_a_whole_file_becomes_one_page() {
        let chunk = |file: &str, pages: Option<&str>| {
            let mut c = json!({"run_id": "r", "chunk": 0, "path": fixtures(), "files": [file],
                "source": file, "format": "x", "ocr": "never", "figure_images": true});
            if let Some(p) = pages {
                c["pages"] = json!(p);
            }
            call(c).unwrap()
        };
        let pdf = chunk("layout.pdf", None);
        let fig = &pdf["figures"][0];
        assert_eq!(fig["caption"], "Figure 1. Sales chart");
        assert_eq!(fig["page"], 1);
        assert!(fig["image_base64"].as_str().is_some_and(|d| !d.is_empty()));
        assert!(
            fig["mime"]
                .as_str()
                .is_some_and(|m| m.starts_with("image/"))
        );
        let docx = chunk("report.docx", None);
        assert_eq!(docx["document"]["format"], "docx");
        assert_eq!(docx["pages"].as_array().unwrap().len(), 1);
        assert_eq!(docx["pages"][0]["page"], 1);
    }

    #[test]
    fn an_unreadable_file_is_a_document_with_an_error_not_a_failed_run() {
        let out = call(
            json!({"run_id": "r", "chunk": 3, "path": fixtures(), "files": ["missing.pdf"],
            "source": "missing.pdf", "format": "pdf"}),
        )
        .unwrap();
        assert_eq!(out["document"]["complete"], false);
        assert!(
            out["document"]["error"]
                .as_str()
                .is_some_and(|e| !e.is_empty())
        );
        assert_eq!(out["document"]["chunk"], 3);
        assert_eq!(out["pages"], json!([]));
    }

    #[test]
    fn a_single_file_job_is_planned_into_chunks_that_name_the_file_itself() {
        let file = format!("{}/text.pdf", fixtures());
        let chunks = call(
            json!({"run_id": "r", "path": file, "path_original": "/real/text.pdf",
            "ocr": "never"}),
        )
        .unwrap();
        let chunk = &chunks.as_array().unwrap()[0];
        assert_eq!(chunk["path"], "/real/text.pdf");
        assert_eq!(chunk["source"], "text.pdf");
        assert!(chunk.get("files").is_none(), "{chunk}");
        let mut read = chunk.clone();
        read["path"] = json!(file);
        read["path_original"] = json!("/real/text.pdf");
        read["run_id"] = json!("r");
        let out = call(read).unwrap();
        assert_eq!(out["document"]["format"], "pdf");
        assert!(!out["pages"].as_array().unwrap().is_empty());
        let tree = format!("{}/tree", fixtures());
        let folder = call(json!({"run_id": "r", "path": tree, "ocr": "never"})).unwrap();
        assert!(folder[0]["files"].is_array());
        assert_eq!(folder[0]["path"], tree);
    }

    #[test]
    fn markers_split_paged_formats_only() {
        let md = "<!-- document: a.xlsx (xlsx) -->\n\n<!-- sheet 1: A -->\n\nx\n\n<!-- sheet 2: B -->\n\ny\n";
        let pages = split_pages("xlsx", md);
        assert_eq!(
            pages,
            [
                (1, "<!-- sheet 1: A -->\n\nx".to_owned()),
                (2, "<!-- sheet 2: B -->\n\ny".to_owned())
            ]
        );
        let text = "<!-- page 3 -->\nnot a marker in plain text";
        assert_eq!(split_pages("text", text), [(1, text.to_owned())]);
        assert_eq!(marker("<!-- document: a (pdf) -->"), None);
    }
}
