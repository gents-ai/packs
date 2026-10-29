//! The field, enum, length, uniqueness and reference rules of the research
//! gate (`graph/tools/validate.sh`), with its `ERROR: ...` wording. References
//! and store uniqueness are checked only once `found` (the store's answers)
//! is known; without it a rule that needs the store is not run.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::batch::{Batch, Record};
use crate::store::Found;

pub const REQUIRED: [&str; 10] = [
    "text",
    "quote",
    "from",
    "sources",
    "entities",
    "kind",
    "by",
    "asserted",
    "status",
    "confidence",
];
pub const KINDS: [&str; 9] = [
    "fact",
    "number",
    "date",
    "rule",
    "mapping",
    "interpretation",
    "position",
    "quote",
    "question",
];
pub const STATUSES: [&str; 4] = ["live", "superseded", "contested", "retracted"];
pub const CONFIDENCES: [&str; 3] = ["sourced", "memory", "inferred"];
pub const FITS: [&str; 4] = ["native", "partial", "gap", "convention"];
pub const AGAINST: [&str; 2] = ["defradb", "fefra"];
pub const EDGE_TYPES: [&str; 8] = [
    "supports",
    "contradicts",
    "supersedes",
    "retracts",
    "answers",
    "maps-to",
    "said-first",
    "depends-on",
];
pub const MAX_WORDS: usize = 40;

/// Claim keys a Claim row stores; any other key would be lost on export.
const CLAIM_TEXT_KEYS: [&str; 15] = [
    "id",
    "text",
    "quote",
    "from",
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
];
const EDGE_KEYS: [&str; 6] = ["from", "to", "type", "by", "date", "note"];
const ENTITY_TEXT_KEYS: [&str; 4] = ["id", "type", "name", "note"];
const PERSPECTIVE_KEYS: [&str; 5] = ["id", "name", "kind", "note", "did"];

/// How a record names itself in an error: its id, or `?`.
pub fn label(record: &Record) -> &str {
    record.str("id").unwrap_or("?")
}

/// `<from>|<type>|<to>`, the Edge row's unique key.
pub fn edge_key(edge: &Record) -> Option<String> {
    Some(format!(
        "{}|{}|{}",
        edge.str("from")?,
        edge.str("type")?,
        edge.str("to")?
    ))
}

pub fn ids(records: &[Record]) -> BTreeSet<String> {
    records
        .iter()
        .filter_map(|r| r.str("id"))
        .map(str::to_owned)
        .collect()
}

/// Every value seen more than once, sorted (`sort | uniq -d`).
fn repeated<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen = BTreeMap::new();
    for value in values {
        *seen.entry(value).or_insert(0usize) += 1;
    }
    seen.into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(v, _)| v)
        .collect()
}

/// A `code` source reference: `^[A-Za-z0-9_./-]+(:[0-9]+(-[0-9]+)?)?$`.
pub fn code_ref_ok(code: &str) -> bool {
    let (path, lines) = match code.split_once(':') {
        Some((path, lines)) => (path, Some(lines)),
        None => (code, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let path_ok = !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b));
    path_ok
        && match lines {
            None => true,
            Some(lines) => match lines.split_once('-') {
                Some((a, b)) => digits(a) && digits(b),
                None => digits(lines),
            },
        }
}

/// Words as `validate.sh` counts them: `split(" ")`, empty pieces dropped.
pub fn word_count(text: &str) -> usize {
    text.split(' ').filter(|w| !w.is_empty()).count()
}

fn truthy(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null) | Some(Value::Bool(false)))
}

fn shown(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "null".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Whether `id` is known: in the batch, or (when the store answered) in it.
fn known(id: &str, in_batch: &BTreeSet<String>, in_store: Option<&BTreeSet<String>>) -> bool {
    in_batch.contains(id) || in_store.is_none_or(|store| store.contains(id))
}

pub fn check(batch: &Batch, found: Option<&Found>) -> Vec<String> {
    let mut errors = batch.parse_errors.clone();
    if !batch.claims.is_empty() && batch.section.is_none() {
        errors.push("ERROR: the batch has claims but no section".to_owned());
    }
    let claim_ids = ids(&batch.claims);
    let entity_ids = ids(&batch.entities);
    let perspective_ids = ids(&batch.perspectives);

    for (part, records, keys) in [
        ("entities", &batch.entities, &ENTITY_TEXT_KEYS[..]),
        ("perspectives", &batch.perspectives, &PERSPECTIVE_KEYS[..]),
    ] {
        for record in records.iter() {
            check_plain(part, record, keys, &mut errors);
        }
    }
    let duplicates = [
        ("entity", &batch.entities, found.map(|f| &f.entities)),
        ("claim", &batch.claims, found.map(|f| &f.claims)),
        ("perspective", &batch.perspectives, found.map(|f| &f.perspectives)),
    ];
    for (noun, records, store) in duplicates {
        for id in repeated(records.iter().filter_map(|r| r.str("id"))) {
            errors.push(format!("ERROR: duplicate {noun} id {id}"));
        }
        if let Some(store) = store {
            for id in ids(records).iter().filter(|id| store.contains(*id)) {
                errors.push(format!(
                    "ERROR: duplicate {noun} id {id} (already in the store)"
                ));
            }
        }
    }

    for claim in &batch.claims {
        check_claim(
            claim,
            &entity_ids,
            &perspective_ids,
            found,
            &mut errors,
        );
    }

    for edge in &batch.edges {
        check_edge(edge, &claim_ids, &perspective_ids, found, &mut errors);
    }
    let keys: Vec<String> = batch.edges.iter().filter_map(edge_key).collect();
    for key in repeated(keys.iter().map(String::as_str)) {
        errors.push(format!("ERROR: duplicate edge {key}"));
    }
    if let Some(found) = found {
        let keys: BTreeSet<&String> = keys.iter().collect();
        for key in keys.into_iter().filter(|k| found.edges.contains(*k)) {
            errors.push(format!("ERROR: duplicate edge {key} (already in the store)"));
        }
    }
    errors
}

/// An entity or perspective line: an id, string fields, known keys.
fn check_plain(part: &str, record: &Record, keys: &[&str], errors: &mut Vec<String>) {
    let Some(id) = record.str("id").filter(|id| !id.is_empty()) else {
        errors.push(format!("ERROR: {part}:{} missing id", record.line));
        return;
    };
    let noun = if part == "entities" {
        "entity"
    } else {
        "perspective"
    };
    for (key, value) in &record.fields {
        if part == "entities" && key == "aliases" {
            if !value.is_null() && !is_string_array(value) {
                errors.push(format!(
                    "ERROR: {noun} {id} aliases must be an array of strings"
                ));
            }
        } else if !keys.contains(&key.as_str()) {
            errors.push(format!("ERROR: {noun} {id} unknown field {key}"));
        } else if !value.is_null() && !value.is_string() {
            errors.push(format!("ERROR: {noun} {id} {key} must be a string"));
        }
    }
}

fn is_string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

fn check_claim(
    claim: &Record,
    entity_ids: &BTreeSet<String>,
    perspective_ids: &BTreeSet<String>,
    found: Option<&Found>,
    errors: &mut Vec<String>,
) {
    let id = label(claim);
    let well_formed = claim.str("id").is_some_and(|id| claim_id_ok(id));
    if !well_formed {
        errors.push(format!("ERROR: {id} bad id format"));
    }
    for key in claim.fields.keys() {
        if !CLAIM_TEXT_KEYS.contains(&key.as_str()) && key != "sources" && key != "entities" {
            errors.push(format!("ERROR: {id} unknown field {key}"));
        }
    }
    for key in CLAIM_TEXT_KEYS {
        if claim.fields.get(key).is_some_and(|v| !v.is_null() && !v.is_string()) {
            errors.push(format!("ERROR: {id} {key} must be a string"));
        }
    }
    for key in REQUIRED {
        if claim.is_null(key) {
            errors.push(format!("ERROR: {id} missing field {key}"));
        }
    }
    let words = word_count(claim.str("text").unwrap_or(""));
    if words > MAX_WORDS {
        errors.push(format!("ERROR: {id} text is {words} words (max {MAX_WORDS})"));
    }
    let one_of = |key: &str, allowed: &[&str]| claim.str(key).is_some_and(|v| allowed.contains(&v));
    if !one_of("kind", &KINDS) {
        errors.push(format!("ERROR: {id} bad kind"));
    }
    if !one_of("status", &STATUSES) {
        errors.push(format!("ERROR: {id} bad status"));
    }
    if !one_of("confidence", &CONFIDENCES) {
        errors.push(format!("ERROR: {id} bad confidence"));
    }
    if let (Some(by), Some(found)) = (claim.str("by"), found) {
        if !known(by, perspective_ids, Some(&found.perspectives)) {
            errors.push(format!("ERROR: {id} unknown perspective {by}"));
        }
    }
    if claim.str("kind") == Some("mapping") {
        if !one_of("fit", &FITS) {
            errors.push(format!("ERROR: {id} mapping without valid fit"));
        }
        if !one_of("against", &AGAINST) {
            errors.push(format!("ERROR: {id} mapping without valid against"));
        }
    }
    match claim.fields.get("entities") {
        None | Some(Value::Null) => errors.push(format!("ERROR: {id} has no entities")),
        Some(Value::Array(items)) if items.is_empty() => {
            errors.push(format!("ERROR: {id} has no entities"))
        }
        Some(value) if !is_string_array(value) => {
            errors.push(format!("ERROR: {id} entities must be an array of strings"))
        }
        Some(Value::Array(items)) => {
            if let Some(found) = found {
                for entity in items.iter().filter_map(Value::as_str) {
                    if !known(entity, entity_ids, Some(&found.entities)) {
                        errors.push(format!("ERROR: {id} unknown entity {entity}"));
                    }
                }
            }
        }
        Some(_) => {}
    }
    let sources = match claim.fields.get("sources") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.iter().all(Value::is_object) => {
            items.iter().filter_map(Value::as_object).collect()
        }
        Some(_) => {
            errors.push(format!("ERROR: {id} sources must be an array of objects"));
            Vec::new()
        }
    };
    for source in &sources {
        let code = source.get("code");
        if code.is_some_and(|c| !c.is_null()) && !code.and_then(Value::as_str).is_some_and(code_ref_ok) {
            errors.push(format!("ERROR: {id} bad code ref {}", shown(code)));
        }
        if source.get("doc").is_some_and(|d| !d.is_null() && !d.is_string()) {
            errors.push(format!("ERROR: {id} source doc must be a string or null"));
        }
    }
    let cites = sources
        .iter()
        .any(|s| truthy(s.get("doc")) || truthy(s.get("code")));
    if claim.str("confidence") == Some("sourced") && !cites {
        errors.push(format!(
            "ERROR: {id} is sourced but cites no fetched doc or code ref"
        ));
    }
}

/// `^c:[a-z0-9-]+:[0-9]{3}$`.
pub fn claim_id_ok(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("c:") else {
        return false;
    };
    let Some((section, number)) = rest.split_once(':') else {
        return false;
    };
    !section.is_empty()
        && section
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && number.len() == 3
        && number.bytes().all(|b| b.is_ascii_digit())
}

fn check_edge(
    edge: &Record,
    claim_ids: &BTreeSet<String>,
    perspective_ids: &BTreeSet<String>,
    found: Option<&Found>,
    errors: &mut Vec<String>,
) {
    let from = edge.str("from").unwrap_or("?");
    let to = edge.str("to").unwrap_or("?");
    for key in ["from", "to", "type", "by", "date"] {
        if edge.is_null(key) {
            errors.push(format!("ERROR: edge {from}->{to} missing {key}"));
        }
    }
    for (key, value) in &edge.fields {
        if !EDGE_KEYS.contains(&key.as_str()) {
            errors.push(format!("ERROR: edge {from}->{to} unknown field {key}"));
        } else if !value.is_null() && !value.is_string() {
            errors.push(format!("ERROR: edge {from}->{to} {key} must be a string"));
        }
    }
    if let Some(found) = found {
        for (end, side) in [("from", edge.str("from")), ("to", edge.str("to"))] {
            if let Some(claim) = side {
                if !known(claim, claim_ids, Some(&found.claims)) {
                    errors.push(format!("ERROR: edge {end} unknown claim {claim}"));
                }
            }
        }
    }
    if !edge.str("type").is_some_and(|t| EDGE_TYPES.contains(&t)) {
        errors.push(format!(
            "ERROR: edge {from}->{to} bad type {}",
            shown(edge.fields.get("type"))
        ));
    }
    if let (Some(by), Some(found)) = (edge.str("by"), found) {
        if !known(by, perspective_ids, Some(&found.perspectives)) {
            errors.push(format!("ERROR: edge {from}->{to} unknown perspective {by}"));
        }
    }
}
