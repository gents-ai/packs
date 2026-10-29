//! Exports the claim graph back to the research repository's JSONL layout.
//! The plugin reads `Claim`, `Edge`, `Entity` and `Perspective` through
//! `data_calls`, a few pages of 256 rows per round, then writes
//! `claims/<section>.jsonl`, `edges/edges.jsonl`, `entities/entities.jsonl`
//! and `perspectives/perspectives.jsonl` under the bound `out_dir`, sorted by
//! id. A failure is `{"ok": false, "error": ...}` with a zero exit, never a
//! partial write.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{Map, Value, json};

mod callback;
mod render;
mod write;

use render::{KINDS, Kind};
use write::Output;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// The host's limits on one `data_calls` round and one query.
const MAX_QUERIES_PER_ROUND: usize = 16;
const PAGE: usize = 256;
/// The host refuses an offset past this.
const MAX_OFFSET: usize = 100_000;
const MAX_SECTIONS: usize = 1000;
/// Top-level fields the host adds to the caller's input.
pub(crate) const HOST_FIELDS: [&str; 3] = ["data_calls", "data_results", "state"];
pub(crate) const FIELDS: [&str; 4] = ["out_dir", "out_dir_original", "sections", "dry_run"];

pub(crate) struct Failure(pub(crate) String);

fn invalid(error: impl Into<String>) -> Failure {
    Failure(error.into())
}

struct Request<'a> {
    out_dir: Option<&'a str>,
    sections: Option<Vec<&'a str>>,
    dry_run: bool,
}

fn parse(input: &Map<String, Value>) -> Result<Request<'_>, Failure> {
    if let Some(field) = input
        .keys()
        .find(|k| !FIELDS.contains(&k.as_str()) && !HOST_FIELDS.contains(&k.as_str()))
    {
        return Err(invalid(format!(
            "the field {field:?} is not known; the fields are out_dir, sections and dry_run"
        )));
    }
    let dry_run = match input.get("dry_run") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(invalid("dry_run must be true or false")),
    };
    let out_dir = match input.get("out_dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
        Some(_) => return Err(invalid("out_dir must be a nonempty folder path")),
    };
    if out_dir.is_none() && !dry_run {
        return Err(invalid(
            "out_dir is required: the allowed folder to write the JSONL tree into (or set dry_run to count only)",
        ));
    }
    let sections = match input.get("sections") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) if (1..=MAX_SECTIONS).contains(&items.len()) => {
            let mut names = Vec::new();
            for item in items {
                match item.as_str().filter(|s| render::section_is_safe(s)) {
                    Some(s) => names.push(s),
                    None => {
                        return Err(invalid(format!(
                            "sections holds {item}; a section is a file stem of letters, digits, '-', '_' and '.'"
                        )));
                    }
                }
            }
            Some(names)
        }
        Some(_) => {
            return Err(invalid(format!(
                "sections must be a list of 1 to {MAX_SECTIONS} section names"
            )));
        }
    };
    Ok(Request {
        out_dir,
        sections,
        dry_run,
    })
}

/// Tool mode, or callback mode when the input carries `request_id`.
fn run(input: &Value) -> Result<Value, Failure> {
    match input.get("request_id") {
        Some(id) => Ok(callback::run(input, id)),
        None => run_tool(input),
    }
}

pub(crate) fn run_tool(input: &Value) -> Result<Value, Failure> {
    let input = input
        .as_object()
        .ok_or_else(|| invalid("input must be a JSON object"))?;
    let request = parse(input)?;
    match input.get("data_results") {
        Some(results) => {
            let state = input
                .get("state")
                .ok_or_else(|| invalid("the host returned data results without the plugin's state"))?;
            let results = results
                .as_object()
                .ok_or_else(|| invalid("data_results must be an object"))?;
            advance(&request, state, results)
        }
        None if input.get("data_calls") == Some(&Value::Bool(true)) => {
            let state = start();
            Ok(ask(&request, state))
        }
        None => Err(invalid(
            "the host offers this plugin no data calls; install the pack with its data grant",
        )),
    }
}

/// The state before the first round: every record type at offset 0.
fn start() -> Value {
    let mut cursors = Map::new();
    let mut rows = Map::new();
    for kind in KINDS {
        cursors.insert(
            kind.key.to_owned(),
            json!({"next": 0, "done": false, "short": false}),
        );
        rows.insert(kind.key.to_owned(), json!([]));
    }
    json!({"cursors": cursors, "rows": rows})
}

fn query(request: &Request<'_>, kind: &Kind, offset: usize) -> Value {
    let filter = match (&request.sections, kind.key) {
        (Some(sections), "claims") => json!({"section": {"_in": sections}}),
        _ => json!({}),
    };
    json!({
        "id": format!("{}@{offset}", kind.key),
        "query": {
            "collection": kind.collection,
            "filter": filter,
            "order": {kind.id_field: "ASC"},
            "limit": PAGE,
            "offset": offset,
            "fields": kind.fields,
        },
    })
}

/// The next round: consecutive pages for every record type still being read,
/// sharing the round's 16 queries. A type whose last page came back short is
/// asked for one page only (the page that confirms the end).
fn ask(request: &Request<'_>, state: Value) -> Value {
    let active: Vec<&Kind> = KINDS
        .iter()
        .copied()
        .filter(|k| !state["cursors"][k.key]["done"].as_bool().unwrap_or(false))
        .collect();
    let share = (MAX_QUERIES_PER_ROUND / active.len().max(1)).max(1);
    let mut requests = Vec::new();
    for kind in active {
        let cursor = &state["cursors"][kind.key];
        let next = cursor["next"].as_u64().unwrap_or(0) as usize;
        let pages = if cursor["short"].as_bool().unwrap_or(false) {
            1
        } else {
            share
        };
        for page in 0..pages {
            requests.push(query(request, kind, next + page * PAGE));
        }
    }
    json!({"data_calls": {"requests": requests, "state": state}})
}

fn advance(
    request: &Request<'_>,
    state: &Value,
    results: &Map<String, Value>,
) -> Result<Value, Failure> {
    let mut state = state.clone();
    for kind in KINDS {
        if state["cursors"][kind.key]["done"].as_bool() != Some(false) {
            continue;
        }
        let mut pages: Vec<(usize, &Value)> = results
            .iter()
            .filter_map(|(id, result)| {
                let (key, offset) = id.split_once('@')?;
                (key == kind.key).then_some(())?;
                Some((offset.parse().ok()?, result))
            })
            .collect();
        pages.sort_by_key(|(offset, _)| *offset);
        if pages.is_empty() {
            return Err(invalid(format!(
                "the host returned no answer for the {} query",
                kind.collection
            )));
        }
        let mut next = state["cursors"][kind.key]["next"].as_u64().unwrap_or(0) as usize;
        let mut done = false;
        let mut short = false;
        let mut taken = Vec::new();
        for (offset, result) in pages {
            if offset != next {
                break;
            }
            let rows = page_rows(kind, result)?;
            if rows.is_empty() {
                done = true;
                break;
            }
            next += rows.len();
            // A page shorter than the limit may be the end or a page the host
            // cut at its output budget; the following page tells which.
            short = rows.len() < PAGE;
            taken.extend(rows.iter().cloned());
            if short {
                break;
            }
        }
        if !done && next > MAX_OFFSET {
            return Err(invalid(format!(
                "{} holds more than {MAX_OFFSET} rows, more than one export can page",
                kind.collection
            )));
        }
        if let Some(stored) = state["rows"][kind.key].as_array_mut() {
            stored.extend(taken);
        }
        state["cursors"][kind.key] = json!({"next": next, "done": done, "short": short});
    }
    if KINDS
        .iter()
        .all(|k| state["cursors"][k.key]["done"].as_bool() == Some(true))
    {
        return finish(request, &state);
    }
    Ok(ask(request, state))
}

fn page_rows<'a>(kind: &Kind, result: &'a Value) -> Result<&'a Vec<Value>, Failure> {
    if let Some(error) = result.get("error") {
        return Err(invalid(format!(
            "the {} query failed: {}",
            kind.collection,
            error.as_str().unwrap_or("unknown error")
        )));
    }
    result
        .pointer("/result/rows")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            invalid(format!(
                "the {} query returned no rows array",
                kind.collection
            ))
        })
}

/// The records of one type keyed by id, so a row seen twice (the store moved
/// between pages) is written once and the order is the id order.
fn collect(kind: &Kind, state: &Value) -> Result<BTreeMap<String, (Value, Option<String>)>, Failure> {
    let mut records = BTreeMap::new();
    for row in state["rows"][kind.key].as_array().into_iter().flatten() {
        let row = row
            .as_object()
            .ok_or_else(|| invalid(format!("a {} row is not an object", kind.collection)))?;
        let (id, line) = render::render(kind, row).map_err(invalid)?;
        let section = if kind.key == "claims" {
            Some(render::section(row).map_err(invalid)?)
        } else {
            None
        };
        records.insert(id, (line, section));
    }
    Ok(records)
}

fn lines<'a>(values: impl Iterator<Item = &'a Value>) -> String {
    values.fold(String::new(), |mut text, value| {
        text.push_str(&value.to_string());
        text.push('\n');
        text
    })
}

fn finish(request: &Request<'_>, state: &Value) -> Result<Value, Failure> {
    let mut outputs = Vec::new();
    let mut counts = Map::new();
    let mut by_section = Map::new();
    for kind in KINDS {
        let records = collect(kind, state)?;
        counts.insert(kind.key.to_owned(), json!(records.len()));
        if kind.key == "claims" {
            let mut sections: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
            for (line, section) in records.values() {
                let section = section.as_deref().unwrap_or_default();
                sections.entry(section).or_default().push(line);
            }
            for (section, claims) in sections {
                by_section.insert(section.to_owned(), json!(claims.len()));
                outputs.push(Output {
                    path: format!("claims/{section}.jsonl"),
                    text: lines(claims.into_iter()),
                });
            }
        } else {
            outputs.push(Output {
                path: format!("{}/{}.jsonl", kind.key, kind.key),
                text: lines(records.values().map(|(line, _)| line)),
            });
        }
    }
    let files: Vec<&str> = outputs.iter().map(|o| o.path.as_str()).collect();
    let summary = json!({
        "ok": true,
        "dry_run": request.dry_run,
        "written": counts,
        "sections": by_section,
        "files": files,
    });
    if request.dry_run {
        return Ok(summary);
    }
    let dir = request.out_dir.ok_or_else(|| invalid("out_dir is required"))?;
    write::write_all(Path::new(dir), &outputs).map_err(invalid)?;
    Ok(summary)
}

fn main() -> std::io::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let result = if bytes.len() > MAX_INPUT_BYTES {
        Err(invalid(format!(
            "the input may be at most {MAX_INPUT_BYTES} bytes"
        )))
    } else {
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| invalid("the input must be JSON"))
            .and_then(|input| run(&input))
    };
    let output = result.unwrap_or_else(|f| json!({"ok": false, "error": f.0, "retryable": false}));
    std::io::stdout().write_all(output.to_string().as_bytes())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
