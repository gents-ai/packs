//! A graph's profile: the vocabulary its batches are checked against. A batch
//! either carries one (`profile`, JSON text of the ClaimGraph fields the
//! writer sets) or is checked against the latest `ClaimGraph` version stored
//! for its `graph_id`. An inline profile is admitted as the next version.
use std::collections::BTreeSet;

use regex_lite::Regex;
use serde_json::{Map, Value, json};

/// The list fields of a profile: each a list of distinct strings. The first
/// `REQUIRED_LISTS` must be non-empty; the rest may be empty or absent (read
/// as empty), which refuses every mapping, review verdict or resolver.
pub const REQUIRED_LISTS: usize = 3;
pub const LISTS: [&str; 7] = [
    "kinds",
    "confidence_classes",
    "edge_types",
    "mapping_targets",
    "mapping_fits",
    "review_verdicts",
    "resolver_perspectives",
];
/// The optional text fields of a profile.
pub const TEXTS: [&str; 5] = [
    "display_name",
    "question_entity",
    "default_asker",
    "questions_from_prefix",
    "note",
];
/// Fields a stored ClaimGraph row has that a writer never sets.
const ASSIGNED: [&str; 4] = ["graph_id", "version", "batch_id", "created_at"];
pub const MAX_WORDS_LIMIT: u64 = 400;

/// Every ClaimGraph field the gate reads back from the store.
pub fn stored_fields() -> Vec<&'static str> {
    let mut fields = vec!["version", "claim_id_pattern", "text_max_words"];
    fields.extend(LISTS);
    fields.extend(TEXTS);
    fields
}

#[derive(Debug)]
pub struct Profile {
    pub fields: Map<String, Value>,
    pattern: Regex,
    pub text_max_words: usize,
}

impl Profile {
    pub fn list(&self, key: &str) -> impl Iterator<Item = &str> {
        self.fields
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
    }

    pub fn allows(&self, key: &str, value: &str) -> bool {
        self.list(key).any(|v| v == value)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    pub fn claim_id_ok(&self, id: &str) -> bool {
        self.pattern.is_match(id)
    }

    /// The `profile` port's ClaimGraph row; the host writes `batch_id`.
    pub fn row(&self, graph_id: &str, version: i64, created_at: &str) -> Value {
        let mut row = Map::new();
        row.insert("graph_id".to_owned(), json!(graph_id));
        row.insert("version".to_owned(), json!(version));
        for (key, value) in &self.fields {
            // An empty list is omitted: a DefraDB mutation never carries `[]`.
            if !value.is_null() && value.as_array().is_none_or(|items| !items.is_empty()) {
                row.insert(key.clone(), value.clone());
            }
        }
        row.insert("created_at".to_owned(), json!(created_at));
        Value::Object(row)
    }
}

/// The batch's `profile` text, checked as a writer's profile.
pub fn parse_inline(text: &str) -> Result<Profile, Vec<String>> {
    let fields = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(fields)) => fields,
        Ok(_) => return Err(vec!["ERROR: profile is not a JSON object".to_owned()]),
        Err(_) => return Err(vec!["ERROR: profile invalid JSON".to_owned()]),
    };
    let mut errors = Vec::new();
    for key in fields.keys() {
        if ASSIGNED.contains(&key.as_str()) {
            errors.push(format!("ERROR: profile sets {key}, which the gate assigns"));
        } else if !LISTS.contains(&key.as_str())
            && !TEXTS.contains(&key.as_str())
            && key != "claim_id_pattern"
            && key != "text_max_words"
        {
            errors.push(format!("ERROR: profile unknown field {key}"));
        }
    }
    let fields: Map<String, Value> = fields
        .into_iter()
        .filter(|(k, _)| !ASSIGNED.contains(&k.as_str()))
        .collect();
    match build(fields, &mut errors) {
        Some(profile) if errors.is_empty() => Ok(profile),
        _ => Err(errors),
    }
}

/// A ClaimGraph row the store returned; it was checked when admitted, so a
/// row that no longer reads is reported, not repaired.
pub fn parse_stored(graph_id: &str, row: &Value) -> Result<(Profile, i64), Vec<String>> {
    let version = row.get("version").and_then(Value::as_i64).unwrap_or(0);
    let fields: Map<String, Value> = row
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| {
            LISTS.contains(&k.as_str())
                || TEXTS.contains(&k.as_str())
                || *k == "claim_id_pattern"
                || *k == "text_max_words"
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut errors = Vec::new();
    match build(fields, &mut errors) {
        Some(profile) if errors.is_empty() => Ok((profile, version)),
        _ => Err(errors
            .into_iter()
            .map(|e| {
                e.replacen(
                    "ERROR: profile",
                    &format!("ERROR: stored profile {graph_id} v{version}"),
                    1,
                )
            })
            .collect()),
    }
}

fn build(fields: Map<String, Value>, errors: &mut Vec<String>) -> Option<Profile> {
    for (index, key) in LISTS.iter().enumerate() {
        let required = index < REQUIRED_LISTS;
        let ok = match fields.get(*key) {
            None | Some(Value::Null) => !required,
            Some(Value::Array(items)) => {
                let strings: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
                strings.is_some_and(|s| {
                    !(required && s.is_empty())
                        && s.iter().collect::<BTreeSet<_>>().len() == s.len()
                })
            }
            Some(_) => false,
        };
        if ok {
            continue;
        }
        errors.push(if required {
            format!("ERROR: profile {key} must be a non-empty list of distinct strings")
        } else {
            format!("ERROR: profile {key} must be a list of distinct strings")
        });
    }
    for key in TEXTS {
        if fields.get(key).is_some_and(|v| !v.is_null() && !v.is_string()) {
            errors.push(format!("ERROR: profile {key} must be a string"));
        }
    }
    let pattern = match fields.get("claim_id_pattern") {
        Some(Value::String(p)) if !p.is_empty() => match Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                let first = e.to_string();
                let first = first.lines().next().unwrap_or("").trim();
                errors.push(format!(
                    "ERROR: profile claim_id_pattern does not compile: {first}"
                ));
                None
            }
        },
        _ => {
            errors.push("ERROR: profile claim_id_pattern must be a regular expression".to_owned());
            None
        }
    };
    let words = fields
        .get("text_max_words")
        .and_then(Value::as_u64)
        .filter(|n| (1..=MAX_WORDS_LIMIT).contains(n));
    if words.is_none() {
        errors.push(format!(
            "ERROR: profile text_max_words must be an integer from 1 to {MAX_WORDS_LIMIT}"
        ));
    }
    Some(Profile {
        fields,
        pattern: pattern?,
        text_max_words: words? as usize,
    })
}
