//! The fake host's durable maps (L2 section 4.22): children, job rows and pack data.
use crate::wire::{JobPin, JobState, Usage};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// One child request, keyed `(slot, attempt)`; `text` is what `child_output` serves.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildRecord {
    pub session_id: String,
    pub request_id: String,
    pub title: String,
    pub state: String,
    pub text: String,
    pub usage: Usage,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub row_doc_id: String,
    pub tool_call_id: String,
    pub handle: String,
    pub state: JobState,
    pub pin: Option<JobPin>,
    pub chain_root: String,
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub children: BTreeMap<(String, u64), ChildRecord>,
    pub rows: BTreeMap<String, JobRow>,
    /// Pack data by collection name.
    pub collections: BTreeMap<String, Vec<Value>>,
}

/// A collection's unique index and its stamped fields (L2 section 5; a stamp and the field
/// it fills share a name).
fn schema(collection: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match collection {
        "WfNote" => (
            &["chain_root_doc_id", "note_id"],
            &["chain_root_doc_id", "job_row_doc_id", "node_did"],
        ),
        "WfScript" => (
            &["node_did", "script_id"],
            &["node_did", "session_id", "tool_call_doc_id"],
        ),
        "WfName" => (&["node_did", "name", "version"], &["node_did"]),
        _ => (&[], &[]),
    }
}

impl State {
    /// Applies one output's `writes` atomically. `stamps` holds the call context's values; a
    /// stamped field without one refuses the whole output (root C3).
    pub fn apply_writes(
        &mut self,
        writes: &Value,
        stamps: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let mut next = self.collections.clone();
        for w in writes.as_array().into_iter().flatten() {
            if w["op"] != "insert" {
                return Err(format!(
                    "write op {} is not built in the fake host",
                    w["op"]
                ));
            }
            let coll = w["collection"]
                .as_str()
                .ok_or("a write has no collection")?;
            let (unique, stamped) = schema(coll);
            let mut fields: Map<String, Value> =
                w["fields"].as_object().cloned().unwrap_or_default();
            if fields.contains_key("_docID") {
                return Err(format!(
                    "{coll} write refused: _docID is assigned by the host; drop it from fields"
                ));
            }
            for s in stamped {
                if fields.contains_key(*s) {
                    return Err(format!(
                        "{coll} write refused: {s} is stamped by the host; drop it from fields"
                    ));
                }
                let v = stamps.get(*s).ok_or_else(|| {
                    format!("{coll} write refused: stamp {s} has no value in this call's context")
                })?;
                fields.insert((*s).into(), v.clone().into());
            }
            let rows = next.entry(coll.into()).or_default();
            let same = |r: &Value| unique.iter().all(|k| r.get(k) == fields.get(*k));
            if !unique.is_empty() && rows.iter().any(same) {
                // First wins: read, then insert only when absent.
                if w["on_conflict"] == "ignore" {
                    continue;
                }
                return Err(format!(
                    "{coll} violates unique index ({})",
                    unique.join(", ")
                ));
            }
            // DefraDB assigns every document a `_docID`; mint a deterministic one per collection.
            let doc_id = format!("bae-{coll}-{}", rows.len());
            fields.insert("_docID".into(), doc_id.into());
            rows.push(Value::Object(fields));
        }
        self.collections = next;
        Ok(())
    }
}
