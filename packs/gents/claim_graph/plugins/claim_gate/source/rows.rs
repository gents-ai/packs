//! The documents an admitted batch writes, one per JSONL line, and the
//! receipt. A row never sets `batch_id` (the host writes it) and omits an
//! absent field rather than writing null; an empty list is omitted too,
//! because a DefraDB mutation never carries `[]`.
use serde_json::{Map, Value, json};

use crate::batch::{Batch, Record};
use crate::rules::edge_key;

fn put(row: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    match value {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) if items.is_empty() => {}
        Some(value) => {
            row.insert(key.to_owned(), value.clone());
        }
    }
}

/// `" a b c "`: space-joined and space-padded, so `_like: "% a %"` matches
/// any member including the first and last.
fn padded<'a>(words: impl Iterator<Item = &'a str>) -> Option<String> {
    let joined: Vec<&str> = words.collect();
    (!joined.is_empty()).then(|| format!(" {} ", joined.join(" ")))
}

fn strings(value: Option<&Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

pub fn claim(section: &str, claim: &Record) -> Value {
    let f = &claim.fields;
    let mut row = Map::new();
    put(&mut row, "claim_id", f.get("id"));
    row.insert("section".to_owned(), json!(section));
    for key in ["text", "quote"] {
        put(&mut row, key, f.get(key));
    }
    put(&mut row, "from_ref", f.get("from"));
    if let Some(sources) = f.get("sources").filter(|s| !s.is_null()) {
        row.insert("sources".to_owned(), json!(sources.to_string()));
    }
    put(&mut row, "entities", f.get("entities"));
    if let Some(text) = padded(strings(f.get("entities"))) {
        row.insert("entity_text".to_owned(), json!(text));
    }
    for key in [
        "kind",
        "by",
        "asserted",
        "status",
        "confidence",
        "note",
        "fit",
        "against",
        "ref",
        "valid_from",
        "valid_to",
    ] {
        put(&mut row, key, f.get(key));
    }
    Value::Object(row)
}

pub fn edge(edge: &Record) -> Value {
    let f = &edge.fields;
    let mut row = Map::new();
    if let Some(key) = edge_key(edge) {
        row.insert("edge_key".to_owned(), json!(key));
    }
    put(&mut row, "from_claim", f.get("from"));
    put(&mut row, "to_claim", f.get("to"));
    put(&mut row, "edge_type", f.get("type"));
    for key in ["by", "date", "note"] {
        put(&mut row, key, f.get(key));
    }
    Value::Object(row)
}

pub fn entity(entity: &Record) -> Value {
    let f = &entity.fields;
    let mut row = Map::new();
    put(&mut row, "entity_id", f.get("id"));
    put(&mut row, "entity_type", f.get("type"));
    put(&mut row, "name", f.get("name"));
    put(&mut row, "aliases", f.get("aliases"));
    let names = entity.str("name").into_iter().chain(strings(f.get("aliases")));
    if let Some(text) = padded(names) {
        row.insert("alias_text".to_owned(), json!(text.to_lowercase()));
    }
    put(&mut row, "note", f.get("note"));
    Value::Object(row)
}

pub fn perspective(perspective: &Record) -> Value {
    let f = &perspective.fields;
    let mut row = Map::new();
    put(&mut row, "perspective_id", f.get("id"));
    for key in ["name", "kind", "note", "did"] {
        put(&mut row, key, f.get(key));
    }
    Value::Object(row)
}

/// Errors a receipt carries, at most.
pub const MAX_ERRORS: usize = 200;

pub struct Verdict {
    pub errors: Vec<String>,
    pub quotes_checked: u64,
    pub quotes_skipped: u64,
}

/// The ports object the host turns into documents: on a pass the receipt
/// and every row, on a fail the receipt alone.
pub fn result(batch_id: &str, batch: &Batch, verdict: Verdict, checked_at: &str) -> Value {
    let pass = verdict.errors.is_empty();
    let mut errors = verdict.errors;
    if errors.len() > MAX_ERRORS {
        let more = errors.len() - (MAX_ERRORS - 1);
        errors.truncate(MAX_ERRORS - 1);
        errors.push(format!("ERROR: {more} more errors not shown"));
    }
    let admitted = |n: usize| if pass { n } else { 0 };
    let receipt = json!({
        "receipt_id": format!("receipt-{batch_id}"),
        "verdict": if pass { "pass" } else { "fail" },
        "errors": Value::from(errors).to_string(),
        "claims_admitted": admitted(batch.claims.len()),
        "edges_admitted": admitted(batch.edges.len()),
        "entities_admitted": admitted(batch.entities.len()),
        "perspectives_admitted": admitted(batch.perspectives.len()),
        "quotes_checked": verdict.quotes_checked,
        "quotes_skipped": verdict.quotes_skipped,
        "gate_version": env!("CARGO_PKG_VERSION"),
        "checked_at": checked_at,
    });
    let mut out = Map::new();
    out.insert("receipt".to_owned(), receipt);
    if pass {
        let section = batch.section.as_deref().unwrap_or("");
        let ports: [(&str, Vec<Value>); 4] = [
            ("claims", batch.claims.iter().map(|c| claim(section, c)).collect()),
            ("edges", batch.edges.iter().map(edge).collect()),
            ("entities", batch.entities.iter().map(entity).collect()),
            ("perspectives", batch.perspectives.iter().map(perspective).collect()),
        ];
        for (name, rows) in ports {
            if !rows.is_empty() {
                out.insert(name.to_owned(), Value::Array(rows));
            }
        }
    }
    Value::Object(out)
}
