//! Store rows to the research repository's JSONL lines.
//!
//! The key order is the contract's: claims `id, text, quote, from, sources,
//! entities, kind, by, asserted, status, confidence, note` then the optional
//! `fit, against, ref` and `valid_from, valid_to`; edges `from, to, type, by,
//! date, note`; entities `id, type, name, aliases, note`; perspectives `id,
//! name, kind, note` and `did` only when present. A line is compact JSON, the
//! form `jq -c` prints.
use serde_json::{Map, Value};

type Row = Map<String, Value>;

/// One exported record type: its collection, the row field that orders it and
/// the fields a query for it asks for.
pub type Kind = crate::pager::Spec;

pub const CLAIMS: Kind = Kind {
    key: "claims",
    collection: "Claim",
    id_field: "claim_id",
    fields: &[
        "claim_id",
        "section",
        "text",
        "quote",
        "from_ref",
        "sources",
        "entities",
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
    ],
};
pub const EDGES: Kind = Kind {
    key: "edges",
    collection: "Edge",
    id_field: "edge_key",
    fields: &["edge_key", "from_claim", "to_claim", "edge_type", "by", "date", "note"],
};
pub const ENTITIES: Kind = Kind {
    key: "entities",
    collection: "Entity",
    id_field: "entity_id",
    fields: &["entity_id", "entity_type", "name", "aliases", "note"],
};
pub const PERSPECTIVES: Kind = Kind {
    key: "perspectives",
    collection: "Perspective",
    id_field: "perspective_id",
    fields: &["perspective_id", "name", "kind", "note", "did"],
};
pub const KINDS: [&Kind; 4] = [&CLAIMS, &EDGES, &ENTITIES, &PERSPECTIVES];

/// The profile versions of the graph, exported as `claim_graph.json`.
pub const CLAIM_GRAPH: Kind = Kind {
    key: "claim_graph",
    collection: "ClaimGraph",
    id_field: "version",
    fields: &[
        "graph_id",
        "version",
        "display_name",
        "kinds",
        "confidence_classes",
        "edge_types",
        "mapping_targets",
        "mapping_fits",
        "review_verdicts",
        "resolver_perspectives",
        "question_entity",
        "default_asker",
        "questions_from_prefix",
        "claim_id_pattern",
        "text_max_words",
        "note",
        "created_at",
        "batch_id",
    ],
};

/// Everything one export reads: the four JSONL record types and the profiles.
pub const SPECS: [&Kind; 5] = [&CLAIMS, &EDGES, &ENTITIES, &PERSPECTIVES, &CLAIM_GRAPH];

/// One profile version as written to `claim_graph.json`: every ClaimGraph
/// field in schema order, a stored null kept as null, keyed by its version.
pub fn profile(row: &Row) -> Result<(i64, Value), String> {
    let version = row
        .get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| "a ClaimGraph row has no integer version".to_owned())?;
    let mut out = Row::new();
    for field in CLAIM_GRAPH.fields {
        put(&mut out, field, row.get(*field).cloned().unwrap_or(Value::Null));
    }
    Ok((version, Value::Object(out)))
}

/// `claim_graph.json`: a JSON array, one compact profile per line, by version.
pub fn profiles_text<'a>(profiles: impl Iterator<Item = &'a Value>) -> String {
    let items: Vec<String> = profiles.map(Value::to_string).collect();
    if items.is_empty() {
        return "[]\n".to_owned();
    }
    format!("[\n{}\n]\n", items.join(",\n"))
}

/// A string field; null and absent are `None`, a value of another type is a
/// refusal that names the row.
fn text<'a>(row: &'a Row, field: &str, who: &str) -> Result<Option<&'a str>, String> {
    match row.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{who}: {field} is not a string")),
    }
}

fn required<'a>(row: &'a Row, field: &str, who: &str) -> Result<&'a str, String> {
    text(row, field, who)?.ok_or_else(|| format!("{who}: {field} is missing"))
}

fn list(row: &Row, field: &str, who: &str) -> Result<Value, String> {
    match row.get(field) {
        None | Some(Value::Null) => Ok(Value::Array(Vec::new())),
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            Ok(Value::Array(items.clone()))
        }
        Some(_) => Err(format!("{who}: {field} is not a list of strings")),
    }
}

/// The store keeps `sources` as the JSON text of the array.
fn sources(row: &Row, who: &str) -> Result<Value, String> {
    match text(row, "sources", who)? {
        None => Ok(Value::Array(Vec::new())),
        Some(json) => serde_json::from_str::<Value>(json)
            .ok()
            .filter(Value::is_array)
            .ok_or_else(|| format!("{who}: sources is not the JSON text of an array")),
    }
}

fn put(out: &mut Row, key: &str, value: Value) {
    out.insert(key.to_owned(), value);
}

fn put_text(out: &mut Row, key: &str, value: &str) {
    put(out, key, Value::String(value.to_owned()));
}

/// An optional field is written whenever the row holds a string, the empty string included: the research writes `"ref": ""`; null and absent are left out.
fn put_optional(out: &mut Row, row: &Row, field: &str, key: &str, who: &str) -> Result<(), String> {
    if let Some(value) = text(row, field, who)? {
        put_text(out, key, value);
    }
    Ok(())
}

pub fn claim(row: &Row) -> Result<(String, Value), String> {
    let id = required(row, "claim_id", "a claim")?;
    let who = &format!("claim {id}");
    let mut out = Row::new();
    put_text(&mut out, "id", id);
    for (field, key) in [("text", "text"), ("quote", "quote"), ("from_ref", "from")] {
        put_text(&mut out, key, required(row, field, who)?);
    }
    put(&mut out, "sources", sources(row, who)?);
    put(&mut out, "entities", list(row, "entities", who)?);
    for field in ["kind", "by", "asserted", "status", "confidence"] {
        put_text(&mut out, field, required(row, field, who)?);
    }
    put_text(&mut out, "note", text(row, "note", who)?.unwrap_or(""));
    for field in ["fit", "against", "ref", "valid_from", "valid_to"] {
        put_optional(&mut out, row, field, field, who)?;
    }
    Ok((id.to_owned(), Value::Object(out)))
}

pub fn edge(row: &Row) -> Result<(String, Value), String> {
    let key = required(row, "edge_key", "an edge")?;
    let who = &format!("edge {key}");
    let mut out = Row::new();
    for (field, name) in [
        ("from_claim", "from"),
        ("to_claim", "to"),
        ("edge_type", "type"),
        ("by", "by"),
        ("date", "date"),
    ] {
        put_text(&mut out, name, required(row, field, who)?);
    }
    put_text(&mut out, "note", text(row, "note", who)?.unwrap_or(""));
    Ok((key.to_owned(), Value::Object(out)))
}

pub fn entity(row: &Row) -> Result<(String, Value), String> {
    let id = required(row, "entity_id", "an entity")?;
    let who = &format!("entity {id}");
    let mut out = Row::new();
    put_text(&mut out, "id", id);
    put_text(&mut out, "type", required(row, "entity_type", who)?);
    put_text(&mut out, "name", required(row, "name", who)?);
    put(&mut out, "aliases", list(row, "aliases", who)?);
    put_text(&mut out, "note", text(row, "note", who)?.unwrap_or(""));
    Ok((id.to_owned(), Value::Object(out)))
}

pub fn perspective(row: &Row) -> Result<(String, Value), String> {
    let id = required(row, "perspective_id", "a perspective")?;
    let who = &format!("perspective {id}");
    let mut out = Row::new();
    put_text(&mut out, "id", id);
    put_text(&mut out, "name", required(row, "name", who)?);
    put_text(&mut out, "kind", required(row, "kind", who)?);
    put_text(&mut out, "note", text(row, "note", who)?.unwrap_or(""));
    put_optional(&mut out, row, "did", "did", who)?;
    Ok((id.to_owned(), Value::Object(out)))
}

pub fn render(kind: &Kind, row: &Row) -> Result<(String, Value), String> {
    match kind.key {
        "claims" => claim(row),
        "edges" => edge(row),
        "entities" => entity(row),
        _ => perspective(row),
    }
}

/// The section a claim row is filed under, as a safe file stem.
pub fn section(row: &Row) -> Result<String, String> {
    let id = row.get("claim_id").and_then(Value::as_str).unwrap_or("?");
    let section = text(row, "section", &format!("claim {id}"))?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("claim {id}: section is missing"))?;
    if !section_is_safe(section) {
        return Err(format!(
            "claim {id}: section {section:?} is not a file stem of letters, digits, '-', '_' and '.'"
        ));
    }
    Ok(section.to_owned())
}

pub fn section_is_safe(section: &str) -> bool {
    !section.is_empty()
        && !section.starts_with('.')
        && section
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
