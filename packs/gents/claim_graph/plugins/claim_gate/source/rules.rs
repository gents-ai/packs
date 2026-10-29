//! The field, enum, length, uniqueness and reference rules of the research
//! gate (`graph/tools/validate.sh`), with its `ERROR: ...` wording, and the
//! dispute rules. The vocabulary (kinds, confidence classes, edge types,
//! mapping targets and fits, review verdicts, resolvers, the claim id
//! pattern and the word limit) is the graph's profile; without a resolved
//! profile those rules are not run. References and store uniqueness are
//! checked only once `found` (the store's answers) is known; without it a
//! rule that needs the store is not run.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::batch::{Batch, Record};
use crate::profile::Profile;
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
/// The writer's advisory status; not part of a profile.
pub const STATUSES: [&str; 4] = ["live", "superseded", "contested", "retracted"];
/// A disputing claim kind and the edge type that must leave it.
pub const DISPUTES: [(&str, &str); 2] = [("challenge", "challenges"), ("review", "reviews")];

/// Claim keys a Claim row stores; any other key would be lost on export.
const CLAIM_TEXT_KEYS: [&str; 16] = [
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
    "verdict",
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

/// The batch's claims and edges, as the per-claim and per-edge rules see them.
struct Context<'a> {
    claim_ids: BTreeSet<String>,
    entity_ids: BTreeSet<String>,
    perspective_ids: BTreeSet<String>,
    /// Each batch claim's kind.
    kinds: BTreeMap<&'a str, &'a str>,
    /// `(from, type)` of each batch edge, counted.
    edges_from: BTreeMap<(&'a str, &'a str), usize>,
    profile: Option<&'a Profile>,
    found: Option<&'a Found>,
}

pub fn check(batch: &Batch, profile: Option<&Profile>, found: Option<&Found>) -> Vec<String> {
    let mut errors = batch.parse_errors.clone();
    if !batch.claims.is_empty() && batch.section.is_none() {
        errors.push("ERROR: the batch has claims but no section".to_owned());
    }
    let mut edges_from = BTreeMap::new();
    for edge in &batch.edges {
        if let (Some(from), Some(kind)) = (edge.str("from"), edge.str("type")) {
            *edges_from.entry((from, kind)).or_insert(0) += 1;
        }
    }
    let cx = Context {
        claim_ids: ids(&batch.claims),
        entity_ids: ids(&batch.entities),
        perspective_ids: ids(&batch.perspectives),
        kinds: batch
            .claims
            .iter()
            .filter_map(|c| Some((c.str("id")?, c.str("kind")?)))
            .collect(),
        edges_from,
        profile,
        found,
    };

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
        check_claim(claim, &cx, &mut errors);
    }

    for edge in &batch.edges {
        check_edge(edge, &cx, &mut errors);
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

/// An inline profile's resolvers must be perspectives of the graph.
pub fn check_resolvers(batch: &Batch, profile: &Profile, found: &Found) -> Vec<String> {
    let in_batch = ids(&batch.perspectives);
    profile
        .list("resolver_perspectives")
        .filter(|p| !known(p, &in_batch, Some(&found.perspectives)))
        .map(|p| format!("ERROR: profile resolver {p} unknown perspective"))
        .collect()
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

fn check_claim(claim: &Record, cx: &Context, errors: &mut Vec<String>) {
    let id = label(claim);
    let well_formed = claim
        .str("id")
        .is_some_and(|id| cx.profile.is_none_or(|p| p.claim_id_ok(id)));
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
    let one_of = |key: &str, list: &str| {
        claim
            .str(key)
            .is_some_and(|v| cx.profile.is_some_and(|p| p.allows(list, v)))
    };
    if let Some(profile) = cx.profile {
        let words = word_count(claim.str("text").unwrap_or(""));
        let max = profile.text_max_words;
        if words > max {
            errors.push(format!("ERROR: {id} text is {words} words (max {max})"));
        }
        if !one_of("kind", "kinds") {
            errors.push(format!("ERROR: {id} bad kind"));
        }
    }
    if !claim.str("status").is_some_and(|v| STATUSES.contains(&v)) {
        errors.push(format!("ERROR: {id} bad status"));
    }
    if cx.profile.is_some() && !one_of("confidence", "confidence_classes") {
        errors.push(format!("ERROR: {id} bad confidence"));
    }
    if let (Some(by), Some(found)) = (claim.str("by"), cx.found) {
        if !known(by, &cx.perspective_ids, Some(&found.perspectives)) {
            errors.push(format!("ERROR: {id} unknown perspective {by}"));
        }
    }
    if let Some(profile) = cx.profile {
        check_kind_rules(claim, id, profile, cx, errors);
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
            if let Some(found) = cx.found {
                for entity in items.iter().filter_map(Value::as_str) {
                    if !known(entity, &cx.entity_ids, Some(&found.entities)) {
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

/// The rules a claim's kind brings under its profile: a mapping's fit and
/// target, a review's verdict and a disputing claim's edge.
fn check_kind_rules(claim: &Record, id: &str, profile: &Profile, cx: &Context, errors: &mut Vec<String>) {
    let one_of = |key: &str, list: &str| claim.str(key).is_some_and(|v| profile.allows(list, v));
    let kind = claim.str("kind");
    if kind == Some("mapping") {
        if !one_of("fit", "mapping_fits") {
            errors.push(format!("ERROR: {id} mapping without valid fit"));
        }
        if !one_of("against", "mapping_targets") {
            errors.push(format!("ERROR: {id} mapping without valid against"));
        }
    }
    if kind == Some("review") && !one_of("verdict", "review_verdicts") {
        errors.push(format!("ERROR: {id} review without valid verdict"));
    }
    for (dispute, edge_type) in DISPUTES {
        if kind != Some(dispute) || !profile.allows("kinds", dispute) {
            continue;
        }
        let Some(claim_id) = claim.str("id") else {
            continue;
        };
        match cx.edges_from.get(&(claim_id, edge_type)).copied().unwrap_or(0) {
            1 => {}
            0 => errors.push(format!(
                "ERROR: {id} is a {dispute} without its {edge_type} edge"
            )),
            n => errors.push(format!(
                "ERROR: {id} is a {dispute} with {n} {edge_type} edges (one only)"
            )),
        }
    }
}

fn check_edge(edge: &Record, cx: &Context, errors: &mut Vec<String>) {
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
    if let Some(found) = cx.found {
        for (end, side) in [("from", edge.str("from")), ("to", edge.str("to"))] {
            if let Some(claim) = side {
                if !known(claim, &cx.claim_ids, Some(&found.claims)) {
                    errors.push(format!("ERROR: edge {end} unknown claim {claim}"));
                }
            }
        }
    }
    let Some(profile) = cx.profile else {
        if let (Some(by), Some(found)) = (edge.str("by"), cx.found) {
            if !known(by, &cx.perspective_ids, Some(&found.perspectives)) {
                errors.push(format!("ERROR: edge {from}->{to} unknown perspective {by}"));
            }
        }
        return;
    };
    let edge_type = edge.str("type");
    if !edge_type.is_some_and(|t| profile.allows("edge_types", t)) {
        errors.push(format!(
            "ERROR: edge {from}->{to} bad type {}",
            shown(edge.fields.get("type"))
        ));
    }
    if let (Some(by), Some(found)) = (edge.str("by"), cx.found) {
        if !known(by, &cx.perspective_ids, Some(&found.perspectives)) {
            errors.push(format!("ERROR: edge {from}->{to} unknown perspective {by}"));
        }
    }
    let Some(edge_type) = edge_type.filter(|t| profile.allows("edge_types", t)) else {
        return;
    };
    for (dispute, disputing) in DISPUTES {
        if edge_type != disputing {
            continue;
        }
        // A from-claim neither in the batch nor (yet) in the store is an
        // unknown claim, reported above.
        let kind = cx.kinds.get(from).copied().or_else(|| {
            cx.found
                .and_then(|f| f.claim_kinds.get(from))
                .map(String::as_str)
        });
        if kind.is_some_and(|k| k != dispute) {
            errors.push(format!(
                "ERROR: edge {from}->{to} {edge_type} from a claim that is not a {dispute}"
            ));
        }
    }
    if edge_type == "resolves" {
        if let Some(by) = edge.str("by") {
            if !profile.allows("resolver_perspectives", by) {
                errors.push(format!(
                    "ERROR: edge {from}->{to} resolves by {by}, not a resolver"
                ));
            }
        }
    }
}
