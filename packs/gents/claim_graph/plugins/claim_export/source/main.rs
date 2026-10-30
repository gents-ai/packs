//! Exports one graph back to the research repository's JSONL layout.
//! The plugin reads the graph's `Claim`, `Edge`, `Entity`, `Perspective` and
//! `ClaimGraph` rows through `data_calls`, a few pages of 256 rows per round,
//! then writes `claims/<section>.jsonl`, `edges/edges.jsonl`,
//! `entities/entities.jsonl`, `perspectives/perspectives.jsonl`,
//! `sources/sources.jsonl` (with each source's `text` when
//! `include_source_text` is set) and `claim_graph.json` under the bound
//! `out_dir`, sorted by id (profiles by version). A failure is `{"ok": false, "error": ...}` with a zero exit, never a
//! partial write.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{Map, Value, json};

mod callback;
mod pager;
mod render;
mod source_text;
mod write;

use render::{CLAIM_GRAPH, KINDS, Kind, SOURCES, SPECS};
use write::Output;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SECTIONS: usize = 1000;
/// Top-level fields the host adds to the caller's input.
pub(crate) const HOST_FIELDS: [&str; 3] = ["data_calls", "data_results", "state"];
pub(crate) const FIELDS: [&str; 6] = [
    "graph_id",
    "out_dir",
    "out_dir_original",
    "sections",
    "dry_run",
    "include_source_text",
];

pub(crate) struct Failure(pub(crate) String);

fn invalid(error: impl Into<String>) -> Failure {
    Failure(error.into())
}

struct Request<'a> {
    graph_id: &'a str,
    out_dir: Option<&'a str>,
    sections: Option<Vec<&'a str>>,
    dry_run: bool,
    include_source_text: bool,
}

fn parse(input: &Map<String, Value>) -> Result<Request<'_>, Failure> {
    if let Some(field) = input
        .keys()
        .find(|k| !FIELDS.contains(&k.as_str()) && !HOST_FIELDS.contains(&k.as_str()))
    {
        return Err(invalid(format!(
            "the field {field:?} is not known; the fields are graph_id, out_dir, sections, dry_run and include_source_text"
        )));
    }
    let dry_run = match input.get("dry_run") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(invalid("dry_run must be true or false")),
    };
    let include_source_text = match input.get("include_source_text") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(invalid("include_source_text must be true or false")),
    };
    let graph_id = match input.get("graph_id") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.as_str(),
        None | Some(Value::Null) | Some(Value::String(_)) => {
            return Err(invalid("graph_id is required: the graph to export"));
        }
        Some(_) => return Err(invalid("graph_id must be a string")),
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
        graph_id,
        out_dir,
        sections,
        dry_run,
        include_source_text,
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

/// Every query is scoped to the graph; claim queries also to the sections.
fn filter(request: &Request<'_>, spec: &Kind) -> Value {
    match (&request.sections, spec.key) {
        (Some(sections), "claims") => json!({
            "graph_id": {"_eq": request.graph_id},
            "section": {"_in": sections},
        }),
        _ => json!({"graph_id": {"_eq": request.graph_id}}),
    }
}

fn ask(request: &Request<'_>, state: Value) -> Value {
    pager::ask(&SPECS, |spec| filter(request, spec), state)
}

fn start() -> Value {
    pager::start(&SPECS)
}

fn advance(
    request: &Request<'_>,
    state: &Value,
    results: &Map<String, Value>,
) -> Result<Value, Failure> {
    if source_text::active(state) {
        let dir = text_dir(request)?;
        let lines = source_lines(state)?;
        let (state, done) =
            source_text::advance(dir, state.clone(), results, &lines).map_err(invalid)?;
        if done {
            return finish(request, &state);
        }
        return Ok(source_text::ask(request.graph_id, state));
    }
    let (state, done) = pager::advance(&SPECS, state, results).map_err(invalid)?;
    if !done {
        return Ok(ask(request, state));
    }
    if request.include_source_text && !request.dry_run {
        let state = source_text::start(text_dir(request)?, state).map_err(invalid)?;
        return Ok(source_text::ask(request.graph_id, state));
    }
    finish(request, &state)
}

/// The folder the text pass appends to; only a writing export has one.
fn text_dir<'a>(request: &Request<'a>) -> Result<&'a Path, Failure> {
    request
        .out_dir
        .map(Path::new)
        .ok_or_else(|| invalid("out_dir is required"))
}

/// The metadata line of every source read by the paged pass, by id.
fn source_lines(state: &Value) -> Result<BTreeMap<String, Value>, Failure> {
    let mut lines = BTreeMap::new();
    for row in pager::rows(state, &SOURCES) {
        let row = row
            .as_object()
            .ok_or_else(|| invalid("a Source row is not an object"))?;
        let (id, line) = render::source(row).map_err(invalid)?;
        lines.insert(id, line);
    }
    Ok(lines)
}

/// The records of one type keyed by id, so a row seen twice (the store moved
/// between pages) is written once and the order is the id order.
fn collect(kind: &Kind, state: &Value) -> Result<BTreeMap<String, (Value, Option<String>)>, Failure> {
    let mut records = BTreeMap::new();
    for row in pager::rows(state, kind) {
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
    let mut profiles = BTreeMap::new();
    for row in pager::rows(state, &CLAIM_GRAPH) {
        let row = row
            .as_object()
            .ok_or_else(|| invalid("a ClaimGraph row is not an object"))?;
        let (version, profile) = render::profile(row).map_err(invalid)?;
        profiles.insert(version, profile);
    }
    let sources = source_lines(state)?;
    // With the texts, the file is the part the text pass wrote, put in place
    // after the other files.
    let texts = request.include_source_text && !request.dry_run;
    if !texts {
        outputs.push(Output {
            path: source_text::FILE.to_owned(),
            text: lines(sources.values()),
        });
    }
    counts.insert(CLAIM_GRAPH.key.to_owned(), json!(profiles.len()));
    counts.insert(SOURCES.key.to_owned(), json!(sources.len()));
    outputs.push(Output {
        path: "claim_graph.json".to_owned(),
        text: render::profiles_text(profiles.values()),
    });
    let mut files: Vec<&str> = outputs.iter().map(|o| o.path.as_str()).collect();
    if texts {
        files.insert(files.len() - 1, source_text::FILE);
    }
    let summary = json!({
        "ok": true,
        "graph_id": request.graph_id,
        "dry_run": request.dry_run,
        "include_source_text": request.include_source_text,
        "written": counts,
        "sections": by_section,
        "files": files,
    });
    if request.dry_run {
        return Ok(summary);
    }
    let dir = request.out_dir.ok_or_else(|| invalid("out_dir is required"))?;
    write::write_all(Path::new(dir), &outputs).map_err(invalid)?;
    if texts {
        source_text::commit(Path::new(dir)).map_err(invalid)?;
    }
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
