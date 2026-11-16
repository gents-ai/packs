//! The `text` of the graph's sources, read after the paged pass when the
//! export includes it. A source's text is up to 200000 characters, so the
//! texts are never kept in the round `state` (which every round sends back
//! and the host caps at the plugin's output budget): each round's pages are
//! appended to a hidden part file under `out_dir`, renamed over
//! `sources/sources.jsonl` when the export finishes.
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::render::SOURCES;

/// Rows per text query and queries per round: at most 32 texts, a few MiB,
/// in one round's results.
pub const TEXT_PAGE: usize = 16;
pub const TEXT_QUERIES: usize = 2;
pub const FILE: &str = "sources/sources.jsonl";
const PART: &str = "sources/.claim_export-sources.part";
const KEY: &str = "source_text";

pub fn active(state: &Value) -> bool {
    state.get(KEY).is_some()
}

/// Starts the pass: an empty part file and the cursor at offset 0.
pub fn start(dir: &Path, mut state: Value) -> Result<Value, String> {
    let part = dir.join(PART);
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create the folder for {FILE}: {e}"))?;
    }
    std::fs::write(&part, b"").map_err(|e| format!("cannot write {FILE}: {e}"))?;
    state[KEY] = json!({"next": 0, "last": null, "written": 0});
    Ok(state)
}

pub fn ask(graph_id: &str, state: Value) -> Value {
    let next = state[KEY]["next"].as_u64().unwrap_or(0) as usize;
    let requests: Vec<Value> = (0..TEXT_QUERIES)
        .map(|page| {
            let offset = next + page * TEXT_PAGE;
            json!({
                "id": format!("{KEY}@{offset}"),
                "query": {
                    "collection": SOURCES.collection,
                    "filter": {"graph_id": {"_eq": graph_id}},
                    "order": {SOURCES.id_field: "ASC"},
                    "limit": TEXT_PAGE,
                    "offset": offset,
                    "fields": [SOURCES.id_field, "text"],
                },
            })
        })
        .collect();
    json!({"data_calls": {"requests": requests, "state": state}})
}

/// Appends one round's texts. `lines` holds the metadata line of every
/// source of the paged pass by id; a source created since has none and is
/// left out, as the paged pass left it out. Returns the state and whether
/// the pass reached the end.
pub fn advance(
    dir: &Path,
    mut state: Value,
    results: &Map<String, Value>,
    lines: &BTreeMap<String, Value>,
) -> Result<(Value, bool), String> {
    let mut next = state[KEY]["next"].as_u64().unwrap_or(0) as usize;
    let mut last = state[KEY]["last"].as_str().map(str::to_owned);
    let mut written = state[KEY]["written"].as_u64().unwrap_or(0);
    let mut pages: Vec<(usize, &Value)> = results
        .iter()
        .filter_map(|(id, result)| {
            let offset = id.strip_prefix(KEY)?.strip_prefix('@')?.parse().ok()?;
            Some((offset, result))
        })
        .collect();
    pages.sort_by_key(|(offset, _)| *offset);
    if pages.is_empty() {
        return Err("the host returned no answer for the Source text query".into());
    }
    let mut text = String::new();
    let mut finished = false;
    for (offset, result) in pages {
        if offset != next {
            break;
        }
        if let Some(error) = result.get("error") {
            return Err(format!(
                "the Source text query failed: {}",
                error.as_str().unwrap_or("unknown error")
            ));
        }
        let rows = result
            .pointer("/result/rows")
            .and_then(Value::as_array)
            .ok_or("the Source text query returned no rows array")?;
        if rows.is_empty() {
            finished = true;
            break;
        }
        next += rows.len();
        for row in rows {
            let Some(id) = row[SOURCES.id_field].as_str() else {
                continue;
            };
            // Pages ordered by id: an id at or before the last one written is
            // a row seen again because the store grew between pages.
            if last.as_deref().is_some_and(|l| id <= l) {
                continue;
            }
            let Some(Value::Object(meta)) = lines.get(id) else {
                continue;
            };
            let mut line = meta.clone();
            line.insert("text".into(), row.get("text").cloned().unwrap_or(Value::Null));
            text.push_str(&Value::Object(line).to_string());
            text.push('\n');
            last = Some(id.to_owned());
            written += 1;
        }
        // A short page may be the host's output budget; the next page tells.
        if rows.len() < TEXT_PAGE {
            break;
        }
    }
    if !text.is_empty() {
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join(PART))
            .map_err(|e| format!("cannot write {FILE}: {e}"))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("cannot write {FILE}: {e}"))?;
    }
    state[KEY] = json!({"next": next, "last": last, "written": written});
    Ok((state, finished))
}

/// Puts the finished part file in place.
pub fn commit(dir: &Path) -> Result<(), String> {
    std::fs::rename(dir.join(PART), dir.join(FILE))
        .map_err(|e| format!("cannot replace {FILE}: {e}"))
}
