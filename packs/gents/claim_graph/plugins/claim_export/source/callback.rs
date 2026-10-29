//! Callback mode: the same export, driven by an `ExportRequest` document,
//! answered with one flat `ExportReceipt` row (the whole output object, as
//! the callback has one port), because an operator's `plugin run` binds folders
//! read-only. A failure is a receipt with `ok: false`, never a non-zero exit.
//! Fields the document binding adds beyond the request's own are ignored.
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::{FIELDS, HOST_FIELDS, run_tool};

/// The tool-mode input a request document stands for: only the fields the
/// export knows, with document-shaped empties (blank folder, blank or JSON
/// text `sections`) turned into the tool's own forms.
fn tool_input(input: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in input.as_object().into_iter().flatten() {
        if FIELDS.contains(&key.as_str()) || HOST_FIELDS.contains(&key.as_str()) {
            out.insert(key.clone(), value.clone());
        }
    }
    if out.get("out_dir").and_then(Value::as_str).is_some_and(|s| s.trim().is_empty()) {
        out.remove("out_dir");
    }
    if let Some(Value::String(text)) = out.get("sections") {
        let parsed = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(text).unwrap_or_else(|_| json!([text]))
        };
        out.insert("sections".into(), parsed);
    }
    if out.get("sections").and_then(Value::as_array).is_some_and(Vec::is_empty) {
        out.remove("sections");
    }
    out
}

pub fn run(input: &Value, request_id: &Value) -> Value {
    let id = request_id.as_str().unwrap_or_default();
    let tool = Value::Object(tool_input(input));
    let dry_run = tool.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    let graph_id = tool.get("graph_id").and_then(Value::as_str).unwrap_or_default();
    match run_tool(&tool) {
        Ok(output) if output.get("data_calls").is_some() => output,
        Ok(summary) => receipt(id, graph_id, &summary, now()),
        Err(failure) => failed(id, graph_id, &failure.0, dry_run, now()),
    }
}

fn receipt(request_id: &str, graph_id: &str, summary: &Value, exported_at: u64) -> Value {
    let count = |key: &str| summary["written"][key].as_u64().unwrap_or(0);
    json!({
        "receipt_id": format!("export-{request_id}"),
        "graph_id": graph_id,
        "ok": true,
        "error": "",
        "claims_written": count("claims"),
        "edges_written": count("edges"),
        "entities_written": count("entities"),
        "perspectives_written": count("perspectives"),
        "sections_json": summary["sections"].to_string(),
        "files_json": summary["files"].to_string(),
        "dry_run": summary["dry_run"].as_bool().unwrap_or(false),
        "exported_at": iso_utc(exported_at),
    })
}

fn failed(request_id: &str, graph_id: &str, error: &str, dry_run: bool, exported_at: u64) -> Value {
    json!({
        "receipt_id": format!("export-{request_id}"),
        "graph_id": graph_id,
        "ok": false,
        "error": error,
        "claims_written": 0,
        "edges_written": 0,
        "entities_written": 0,
        "perspectives_written": 0,
        "sections_json": "{}",
        "files_json": "[]",
        "dry_run": dry_run,
        "exported_at": iso_utc(exported_at),
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `2026-10-29T17:39:08Z` for Unix seconds (civil-from-days, proleptic Gregorian).
pub fn iso_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_iso_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_782_754_748), "2026-06-29T17:39:08Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn a_receipt_is_flat_scalars() {
        let summary = json!({"ok": true, "dry_run": true,
            "written": {"claims": 2, "edges": 1, "entities": 3, "perspectives": 4},
            "sections": {"a": 2}, "files": ["claims/a.jsonl"]});
        let r = receipt("r1", "g", &summary, 0);
        assert_eq!(r, json!({
            "receipt_id": "export-r1", "graph_id": "g", "ok": true, "error": "",
            "claims_written": 2, "edges_written": 1, "entities_written": 3, "perspectives_written": 4,
            "sections_json": "{\"a\":2}", "files_json": "[\"claims/a.jsonl\"]",
            "dry_run": true, "exported_at": "1970-01-01T00:00:00Z"}));
        assert!(r.as_object().unwrap().values().all(|v| !v.is_object() && !v.is_array()));
        assert!(r.get("request_id").is_none());
    }

    #[test]
    fn document_shaped_inputs_become_tool_inputs() {
        let t = tool_input(&json!({"request_id": "r", "graph_id": "g", "out_dir": " ", "sections": "[\"a\",\"b\"]",
            "dry_run": true, "submitted_at": "x", "out_dir_original": "/o"}));
        assert_eq!(Value::Object(t), json!({"graph_id": "g", "sections": ["a", "b"], "dry_run": true, "out_dir_original": "/o"}));
        assert!(tool_input(&json!({"sections": ""}))["sections"].is_null());
        assert!(tool_input(&json!({"sections": "[]"})).get("sections").is_none());
        assert!(tool_input(&json!({"sections": null})).get("sections").unwrap().is_null());
        assert_eq!(tool_input(&json!({"sections": "plain"}))["sections"], json!(["plain"]));
    }

    #[test]
    fn failures_are_receipts() {
        let out = run(&json!({"request_id": "r2", "graph_id": "g", "data_calls": true, "out_dir": ""}), &json!("r2"));
        let r = &out;
        assert_eq!((r["ok"].as_bool(), r["receipt_id"].as_str()), (Some(false), Some("export-r2")));
        assert!(r["error"].as_str().unwrap().starts_with("out_dir is required"));
        assert_eq!(r["graph_id"], "g");
        let out = run(&json!({"request_id": "r3", "graph_id": "g", "dry_run": true}), &json!("r3"));
        assert!(out["error"].as_str().unwrap().contains("no data calls"));
        let out = run(&json!({"request_id": "r4", "graph_id": "g", "dry_run": true, "data_calls": true, "sections": "[\"../x\"]"}), &json!("r4"));
        assert!(out["error"].as_str().unwrap().contains("sections holds"));
        let out = run(&json!({"request_id": "r5", "dry_run": true, "data_calls": true}), &json!("r5"));
        assert_eq!(out["error"], "graph_id is required: the graph to export");
        assert_eq!(out["graph_id"], "");
    }

    #[test]
    fn the_first_round_asks_for_the_same_pages_as_tool_mode() {
        let out = run(&json!({"request_id": "r", "graph_id": "g", "dry_run": true, "data_calls": true, "sections": "", "out_dir": ""}), &json!("r"));
        assert_eq!(out["data_calls"]["requests"].as_array().unwrap().len(), 15);
        assert!(out.get("receipt_id").is_none());
    }
}
