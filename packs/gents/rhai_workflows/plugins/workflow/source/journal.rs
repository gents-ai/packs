//! The journal view: the chain's WfNote rows plus the notes this replay creates (L2 section 4.6).
use crate::data::WfNote;
use crate::rounds::Replay;
use serde_json::Value;
use std::collections::BTreeMap;

pub fn run_id(row: &str) -> String {
    format!("run:{row}")
}

pub fn reattempt_id(row: &str) -> String {
    format!("reattempt:{row}")
}

pub fn need_id(row: &str) -> String {
    format!("need:{row}")
}

pub fn await_id(c: u64) -> String {
    format!("await:{c}")
}

pub fn out_id(slot: &str) -> String {
    format!("out:{slot}")
}

pub fn corr_id(slot: &str, n: u64) -> String {
    format!("corr:{slot}#{n}")
}

pub fn scratch_id(name: &str) -> String {
    format!("scratch:{name}")
}

pub struct Journal {
    /// Stored notes by id, one per id: among P2P twins the lowest `_docID`.
    notes: BTreeMap<String, Value>,
    pub child_text: BTreeMap<String, String>,
    /// Notes this replay created, held in memory until an output carries a pin.
    pending: Vec<(String, Value)>,
}

impl Journal {
    pub fn new(replay: Replay) -> Self {
        let mut lowest: BTreeMap<&str, (&str, &Value)> = BTreeMap::new();
        for row in &replay.notes {
            let (Some(id), Some(doc)) = (row["note_id"].as_str(), row["_docID"].as_str()) else {
                continue;
            };
            if lowest.get(id).is_none_or(|(d, _)| doc < *d) {
                lowest.insert(id, (doc, row));
            }
        }
        let notes = lowest
            .into_iter()
            .map(|(id, (_, row))| {
                let value = row["value_json"]
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok());
                (id.to_string(), value.unwrap_or(Value::Null))
            })
            .collect();
        Journal {
            notes,
            child_text: replay.child_text,
            pending: Vec::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.notes
            .get(id)
            .or_else(|| self.pending.iter().find(|(i, _)| i == id).map(|(_, v)| v))
    }

    /// Holds a new note; false (nothing held) when the id already has a value, as the host's
    /// `on_conflict: ignore` keeps the first.
    pub fn put(&mut self, id: &str, value: Value) -> bool {
        if self.get(id).is_some() {
            return false;
        }
        self.pending.push((id.to_string(), value));
        true
    }

    /// The held notes as writes, once the output carries a pin; a pinless output writes none.
    pub fn writes(&self, pinned: bool) -> Vec<Value> {
        if !pinned {
            return Vec::new();
        }
        self.pending
            .iter()
            .map(|(id, value)| {
                let note = WfNote {
                    note_id: id.clone(),
                    value_json: value.to_string(),
                    row: None,
                };
                note.write()
            })
            .collect()
    }
}
