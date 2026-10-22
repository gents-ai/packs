//! The typed pack-data layer (L2 sections 4.6, 4.14 and 4.20; root C3): the three records
//! without their stamped fields, the query builders and the write builders.
use crate::wire::{DataCall, DataQuery, DataWrite, OnConflict, object};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Rows one `data_calls.query` page may ask for (root C3).
pub const PAGE: u32 = 256;

/// `WfScript`: an immutable script row. The stamps `node_did`, `session_id` and
/// `tool_call_doc_id` are the host's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfScript {
    pub script_id: String,
    pub source: String,
    pub meta_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// `WfName`: one version of a kept name. `node_did` is the host's stamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfName {
    pub name: String,
    pub version: u64,
    pub script_id: String,
    pub description: String,
    pub when_to_use: String,
    /// `kept` or `retired`.
    pub state: String,
}

/// `WfNote`: one journal note. `chain_root_doc_id`, `job_row_doc_id` and `node_did` are the
/// host's stamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfNote {
    pub note_id: String,
    pub value_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
}

impl WfScript {
    /// A draft of `source` under `script_id`; `meta_json` holds the script's name and description.
    pub fn draft(
        script_id: &str,
        source: &str,
        origin_hint: &str,
        name: &str,
        about: &str,
    ) -> Self {
        WfScript {
            script_id: script_id.into(),
            source: source.into(),
            meta_json: json!({"name": name, "description": about}).to_string(),
            origin_hint: Some(origin_hint.into()),
            license: None,
            created_at: None,
        }
    }

    /// The insert; the first row wins.
    pub fn write(&self) -> Value {
        insert("WfScript", self, OnConflict::Ignore)
    }
}

impl WfName {
    /// The insert; a row with the same `(node_did, name, version)` fails the call.
    pub fn write(&self) -> Value {
        insert("WfName", self, OnConflict::Fail)
    }
}

impl WfNote {
    /// The insert; the first row wins.
    pub fn write(&self) -> Value {
        insert("WfNote", self, OnConflict::Ignore)
    }
}

fn insert(collection: &str, record: &impl Serialize, on_conflict: OnConflict) -> Value {
    json!(DataWrite {
        op: "insert".into(),
        collection: collection.into(),
        doc_id: None,
        fields: json!(record),
        on_conflict: Some(on_conflict),
    })
}

/// A one-page query of `collection`: [`PAGE`] rows, no order, no offset.
pub fn query(id: &str, collection: &str, filter: Value, fields: &[&str]) -> DataCall {
    DataCall {
        id: id.into(),
        query: DataQuery {
            collection: collection.into(),
            filter: object(filter),
            order: None,
            limit: PAGE,
            offset: None,
            fields: fields.iter().map(|f| f.to_string()).collect(),
        },
    }
}

pub fn ordered(mut call: DataCall, order: Value) -> DataCall {
    call.query.order = Some(object(order));
    call
}

/// At most `limit` rows, never past [`PAGE`].
pub fn limited(mut call: DataCall, limit: u32) -> DataCall {
    call.query.limit = limit.min(PAGE);
    call
}

/// The page starting at row `offset`; the first page omits the key.
pub fn paged(mut call: DataCall, offset: usize) -> DataCall {
    let offset = u32::try_from(offset).unwrap_or(u32::MAX);
    call.query.offset = Some(offset).filter(|n| *n > 0);
    call
}

/// The own-node filter: the host substitutes `$caller.node_did` (root C3).
pub fn own_node() -> Value {
    json!({"node_did": {"_eq": "$caller.node_did"}})
}

/// A chain's notes, one page from row `offset`; `_docID` is a total order, so pages never skip
/// or repeat a P2P twin.
pub fn notes_page(chain_root: &str, offset: usize) -> DataCall {
    let fields = ["_docID", "note_id", "value_json", "row"];
    let call = query(
        "notes",
        "WfNote",
        json!({"chain_root_doc_id": {"_eq": chain_root}}),
        &fields,
    );
    paged(ordered(call, json!({"_docID": "ASC"})), offset)
}

/// The caller's own stored source of `script_id`.
pub fn script_source(script_id: &str) -> DataCall {
    let filter = json!({"_and": [{"script_id": {"_eq": script_id}}, own_node()]});
    limited(query("source", "WfScript", filter, &["source"]), 1)
}

/// The caller's own note `note_id`, lowest `_docID` among P2P twins, with its chain.
pub fn own_note(id: &str, note_id: &str) -> DataCall {
    let filter = json!({"_and": [{"note_id": {"_eq": note_id}}, own_node()]});
    let fields = ["_docID", "note_id", "value_json", "chain_root_doc_id"];
    let call = limited(query(id, "WfNote", filter, &fields), 1);
    ordered(call, json!({"_docID": "ASC"}))
}
