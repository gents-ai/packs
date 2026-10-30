//! Renders a view of one claim graph as Markdown: the callback of a
//! `ViewRequest` document. The plugin reads the graph's `Claim` and `Edge`
//! rows (and `Perspective` for reputation) through `data_calls`, derives each
//! claim's effective status, renders the requested view, writes it to
//! `<out_dir>/<graph_id>/<view>.md` when an `out_dir` is given and not
//! `dry_run`, and answers with one flat `ViewReceipt` row carrying the
//! Markdown inline. Every failure is a receipt with `ok: false` and a zero
//! exit.
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{Map, Value, json};

mod clock;
mod graph;
mod pager;
mod views;
mod write;

use pager::Spec;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// The most Markdown a receipt carries inline, in characters.
pub(crate) const MAX_INLINE_CHARS: usize = 60_000;

const CLAIMS: Spec = Spec {
    key: "claims",
    collection: "Claim",
    id_field: "claim_id",
    fields: &["claim_id", "text", "kind", "by", "asserted", "confidence", "verdict"],
};
const EDGES: Spec = Spec {
    key: "edges",
    collection: "Edge",
    id_field: "edge_key",
    fields: &["edge_key", "from_claim", "to_claim", "edge_type", "by"],
};
const PERSPECTIVES: Spec = Spec {
    key: "perspectives",
    collection: "Perspective",
    id_field: "perspective_id",
    fields: &["perspective_id", "name"],
};

/// The collections a view reads.
fn specs(view: &str) -> &'static [&'static Spec] {
    match view {
        "reputation" => &[&CLAIMS, &EDGES, &PERSPECTIVES],
        _ => &[&CLAIMS, &EDGES],
    }
}

struct Request<'a> {
    request_id: &'a str,
    graph_id: &'a str,
    view: &'a str,
    out_dir: Option<&'a str>,
    dry_run: bool,
    generated_at: String,
}

/// What the receipt reports when the request cannot be read: the fields that
/// could be.
#[derive(Default)]
struct Known<'a> {
    request_id: &'a str,
    graph_id: &'a str,
    view: &'a str,
    /// `view_now` when it is a valid time, so a failure renders the same
    /// bytes in a test case too.
    now: Option<&'a str>,
}

fn known(input: &Map<String, Value>) -> Known<'_> {
    let field = |name| input.get(name).and_then(Value::as_str).unwrap_or_default();
    let now = input
        .get("view_now")
        .and_then(Value::as_str)
        .filter(|now| clock::days(now).is_some());
    Known {
        request_id: field("request_id"),
        graph_id: field("graph_id"),
        view: field("view"),
        now,
    }
}

/// A graph id becomes a folder name, so it is a safe file stem.
pub(crate) fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn parse(input: &Map<String, Value>) -> Result<Request<'_>, String> {
    let nonempty = |name: &str| match input.get(name) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.as_str())),
        None | Some(Value::Null) | Some(Value::String(_)) => Ok(None),
        Some(_) => Err(format!("{name} must be a string")),
    };
    let request_id = nonempty("request_id")?
        .ok_or("request_id is required: the host correlates the receipt with it")?;
    let graph_id = nonempty("graph_id")?.ok_or("graph_id is required: the graph to view")?;
    if !is_safe_name(graph_id) {
        return Err(format!(
            "graph_id {graph_id:?} is not a name of letters, digits, '-', '_' and '.'"
        ));
    }
    let view = nonempty("view")?
        .filter(|v| views::VIEWS.contains(v))
        .ok_or("view must be one of status, disputes and reputation")?;
    let dry_run = match input.get("dry_run") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("dry_run must be true or false".into()),
    };
    let out_dir = nonempty("out_dir")?;
    let generated_at = match input.get("view_now") {
        None | Some(Value::Null) => clock::now_iso(),
        Some(Value::String(now)) if clock::days(now).is_some() => now.clone(),
        Some(_) => {
            return Err("view_now must be an ISO-8601 date or UTC timestamp".into());
        }
    };
    Ok(Request {
        request_id,
        graph_id,
        view,
        out_dir,
        dry_run,
        generated_at,
    })
}

/// One round: the next page requests, or the receipt.
pub(crate) fn run(input: &Value) -> Value {
    let Some(input) = input.as_object() else {
        return failed(&Known::default(), "the input must be a JSON object");
    };
    let known = known(input);
    match round(input) {
        Ok(output) => output,
        Err(error) => failed(&known, &error),
    }
}

fn round(input: &Map<String, Value>) -> Result<Value, String> {
    let request = parse(input)?;
    let specs = specs(request.view);
    let filter = |_: &Spec| json!({"graph_id": {"_eq": request.graph_id}});
    let state = match input.get("data_results") {
        Some(results) => {
            let state = input
                .get("state")
                .ok_or("the host returned data results without the plugin's state")?;
            let results = results.as_object().ok_or("data_results must be an object")?;
            let (state, done) = pager::advance(specs, state, results)?;
            if !done {
                return Ok(pager::ask(specs, filter, state));
            }
            state
        }
        None if input.get("data_calls") == Some(&Value::Bool(true)) => {
            return Ok(pager::ask(specs, filter, pager::start(specs)));
        }
        None => {
            return Err(
                "the host offers this plugin no data calls; install the pack with its data grant"
                    .into(),
            );
        }
    };
    finish(&request, &state)
}

fn finish(request: &Request<'_>, state: &Value) -> Result<Value, String> {
    let graph = graph::Graph::from_rows(
        pager::rows(state, &CLAIMS),
        pager::rows(state, &EDGES),
        pager::rows(state, &PERSPECTIVES),
    )?;
    let rendered = views::render(request.view, request.graph_id, &graph, &request.generated_at);
    let file = match (request.out_dir, request.dry_run) {
        (Some(dir), false) => {
            let path = format!("{}/{}.md", request.graph_id, request.view);
            write::write_all(
                Path::new(dir),
                &[write::Output {
                    path: path.clone(),
                    text: rendered.markdown.clone(),
                }],
            )?;
            path
        }
        _ => String::new(),
    };
    Ok(json!({
        "receipt_id": format!("view-{}", request.request_id),
        "graph_id": request.graph_id,
        "view": request.view,
        "ok": true,
        "error": "",
        "rows": rendered.rows,
        "markdown": inline(&rendered.markdown, &file),
        "file": file,
        "generated_at": request.generated_at,
    }))
}

/// The Markdown as the receipt carries it: whole when it fits, else cut at a
/// line end with a final line saying so, `MAX_INLINE_CHARS` in all.
pub(crate) fn inline(markdown: &str, file: &str) -> String {
    let total = markdown.chars().count();
    if total <= MAX_INLINE_CHARS {
        return markdown.to_owned();
    }
    let whole = if file.is_empty() {
        "request it with an out_dir for the whole view".to_owned()
    } else {
        format!("the whole view is in {file}")
    };
    let note = format!("\n_Truncated: the view is {total} characters; {whole}._\n");
    let budget = MAX_INLINE_CHARS - note.chars().count();
    let end = markdown
        .char_indices()
        .nth(budget)
        .map_or(markdown.len(), |(i, _)| i);
    let kept = &markdown[..end];
    let kept = kept.rfind('\n').map_or(kept, |i| &kept[..=i]);
    format!("{kept}{note}")
}

fn failed(known: &Known<'_>, error: &str) -> Value {
    json!({
        "receipt_id": format!("view-{}", known.request_id),
        "graph_id": known.graph_id,
        "view": known.view,
        "ok": false,
        "error": error,
        "rows": 0,
        "markdown": "",
        "file": "",
        "generated_at": known.now.map_or_else(clock::now_iso, str::to_owned),
    })
}

fn main() -> std::io::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let nothing = Known::default();
    let output = if bytes.len() > MAX_INPUT_BYTES {
        failed(&nothing, &format!("the input may be at most {MAX_INPUT_BYTES} bytes"))
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(input) => run(&input),
            Err(_) => failed(&nothing, "the input must be JSON"),
        }
    };
    std::io::stdout().write_all(output.to_string().as_bytes())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
