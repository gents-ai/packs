//! The `output_schema` contract (L2 section 4.9): a text contract any backend can follow.
//! The prompt carries the schema, the child ends its final message with one json fence, and
//! the engine parses and validates the last fence. Wording is gents' own (U15); tests assert
//! behaviour, never text.
use crate::clip;
use jsonschema::Validator;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A schema past this many serialized bytes is refused.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Corrections sent to one slot before it gives up as unsuccessful.
pub const MAX_CORRECTIONS: u64 = 2;
/// The validator's first error keeps to this many bytes.
const FIRST_ERROR_MAX: usize = 512;

/// A `$ref` that leaves the document; with no retrieval features it could never resolve.
fn foreign_ref(v: &Value) -> Option<&str> {
    match v {
        Value::Object(o) => o.iter().find_map(|(k, v)| match (k.as_str(), v) {
            ("$ref", Value::String(r)) if !r.starts_with('#') => Some(r.as_str()),
            _ => foreign_ref(v),
        }),
        Value::Array(a) => a.iter().find_map(foreign_ref),
        _ => None,
    }
}

/// Compiles `schema`, or says (as the `<what>` of a `workflow_invalid_input`) why it is refused:
/// too large, not self-contained, or not a JSON Schema.
pub fn compile(schema: &Value) -> Result<Validator, String> {
    let bytes = schema.to_string().len();
    if bytes > MAX_SCHEMA_BYTES {
        return Err(format!(
            "output_schema is {bytes} bytes; the limit is {MAX_SCHEMA_BYTES}"
        ));
    }
    if let Some(r) = foreign_ref(schema) {
        return Err(format!(
            "output_schema must be self-contained, but it refers to `{r}`"
        ));
    }
    jsonschema::validator_for(schema)
        .map_err(|e| format!("output_schema is not a valid JSON Schema ({e})"))
}

/// `prompt` followed by the contract block that embeds `schema`.
pub fn with_contract(prompt: &str, schema: &Value) -> String {
    format!(
        "{prompt}\n\n<output-contract>\n\
         Do the work above with your tools first. When you are done, end your final message \
         with exactly one fenced block tagged json. The block holds one JSON value that \
         conforms to the schema below, and nothing else: no prose inside the fence.\n\
         Schema:\n{schema}\n\
         </output-contract>"
    )
}

/// The body of the last ```json fence in `text` that is closed.
fn last_fence(text: &str) -> Option<String> {
    let (mut open, mut last): (Option<Vec<&str>>, Option<String>) = (None, None);
    for line in text.lines() {
        let tag = line.trim();
        if let Some(body) = open.as_mut() {
            if tag == "```" {
                last = Some(body.join("\n"));
                open = None;
            } else {
                body.push(line);
            }
        } else if tag.eq_ignore_ascii_case("```json") {
            open = Some(Vec::new());
        }
    }
    last
}

/// The last closed json fence in `text` or, when it has none, the reply's outermost `{...}`:
/// local models often end with a bare object.
fn candidate_block(text: &str) -> Option<String> {
    last_fence(text).or_else(|| {
        let (start, end) = (text.find('{')?, text.rfind('}')?);
        (end > start).then(|| text[start..=end].to_owned())
    })
}

/// `block` with raw control characters inside string literals written as JSON escapes.
fn escape_control_chars(block: &str) -> String {
    let mut out = String::with_capacity(block.len() + 16);
    let (mut in_string, mut escaped) = (false, false);
    for c in block.chars() {
        match (in_string, escaped, c) {
            (true, false, '\\') => {
                escaped = true;
                out.push(c);
            }
            (true, true, _) => {
                escaped = false;
                out.push(c);
            }
            (true, false, '"') => {
                in_string = false;
                out.push(c);
            }
            (true, false, '\t') => out.push_str("\\t"),
            (true, false, '\n') => out.push_str("\\n"),
            (true, false, '\r') => out.push_str("\\r"),
            (true, false, c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            (false, _, '"') => {
                in_string = true;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// The last json fence of a child's reply (or its bare outermost object) as a value that
/// conforms to `validator`, or the first thing wrong with it (within 512 bytes).
pub fn check(validator: &Validator, text: &str) -> Result<Value, String> {
    let block = candidate_block(text).ok_or("the reply has no json object or closed json block")?;
    let value: Value = serde_json::from_str(&escape_control_chars(&block)).map_err(|e| {
        clip(
            &format!("the json block does not parse ({e})"),
            FIRST_ERROR_MAX,
        )
    })?;
    if let Err(e) = validator.validate(&value) {
        let at = e.instance_path.to_string();
        let first = if at.is_empty() {
            e.to_string()
        } else {
            format!("at {at}: {e}")
        };
        return Err(clip(&first, FIRST_ERROR_MAX));
    }
    Ok(value)
}

/// The message sent to a child whose reply failed: the first error, then what to do.
pub fn correction(first_error: &str) -> String {
    format!(
        "Your last reply did not meet the output contract: {first_error}\n\
         Reply again with only one json fenced block that conforms to the schema in your \
         <output-contract>. Write any tab or line break inside a string as \\t or \\n, and \
         end the message with a closed fence."
    )
}

/// What `output` holds when the corrections ran out.
pub fn gave_up(first_error: &str) -> String {
    format!(
        "the reply never met the output_schema after {MAX_CORRECTIONS} corrections: {first_error}"
    )
}

/// The first 16 hex digits of the sha256 of `text`: a correction note's value.
pub fn digest(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
