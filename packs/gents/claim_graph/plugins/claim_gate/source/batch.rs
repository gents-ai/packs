//! The ClaimBatch fields the binding delivers, and its four JSONL texts parsed
//! into records. A line that is not a JSON object is a gate error, not a
//! malformed call: the batch fails with a receipt naming it.
use serde_json::{Map, Value};

/// One JSONL line: its 1-based line number in its field and its object.
pub struct Record {
    pub line: usize,
    pub fields: Map<String, Value>,
}

impl Record {
    /// The string at `key`, when it is one.
    pub fn str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    /// Whether `key` is absent or null, as jq's `.key == null`.
    pub fn is_null(&self, key: &str) -> bool {
        self.fields.get(key).is_none_or(Value::is_null)
    }
}

pub struct Batch {
    pub graph_id: Option<String>,
    /// The `profile` text, unparsed.
    pub profile: Option<String>,
    pub section: Option<String>,
    pub source_root: Option<String>,
    pub claims: Vec<Record>,
    pub edges: Vec<Record>,
    pub entities: Vec<Record>,
    pub perspectives: Vec<Record>,
    /// `ERROR: ...` lines for lines that did not parse.
    pub parse_errors: Vec<String>,
}

/// The four JSONL fields of a batch, in the order errors report them.
pub const PARTS: [&str; 4] = ["claims", "edges", "entities", "perspectives"];

/// A text field of the source document: absent, null and "" are all unset.
pub fn text_field(input: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

pub fn read(input: &Map<String, Value>) -> Result<Batch, String> {
    let mut parse_errors = Vec::new();
    let mut parts: Vec<Vec<Record>> = Vec::new();
    for part in PARTS {
        let text = text_field(input, part)?.unwrap_or_default();
        parts.push(parse_lines(part, &text, &mut parse_errors));
    }
    let perspectives = parts.pop().unwrap_or_default();
    let entities = parts.pop().unwrap_or_default();
    let edges = parts.pop().unwrap_or_default();
    let claims = parts.pop().unwrap_or_default();
    Ok(Batch {
        graph_id: text_field(input, "graph_id")?,
        profile: text_field(input, "profile")?,
        section: text_field(input, "section")?,
        source_root: text_field(input, "source_root")?,
        claims,
        edges,
        entities,
        perspectives,
        parse_errors,
    })
}

/// Blank lines are skipped, as `validate.sh` skips them.
fn parse_lines(part: &str, text: &str, errors: &mut Vec<String>) -> Vec<Record> {
    let mut records = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let line_no = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(fields)) => records.push(Record {
                line: line_no,
                fields,
            }),
            Ok(_) => errors.push(format!("ERROR: {part}:{line_no} is not a JSON object")),
            Err(_) => errors.push(format!("ERROR: {part}:{line_no} invalid JSON")),
        }
    }
    records
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
            && self.edges.is_empty()
            && self.entities.is_empty()
            && self.perspectives.is_empty()
            && self.parse_errors.is_empty()
    }
}
