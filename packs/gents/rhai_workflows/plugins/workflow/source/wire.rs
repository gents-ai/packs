//! Serde mirrors of the gents job contract (root spec C1 to C6), written by hand so the
//! engine has no gents dependency. Field names are the contract's, verbatim. Fields whose
//! shape the contract leaves open stay `serde_json::Value`.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

type Map = serde_json::Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Installed,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRef {
    pub row_doc_id: String,
    pub chain_root_doc_id: String,
    pub chain: Vec<String>,
    pub handle: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caller {
    pub session_id: String,
    pub request_id: String,
    pub request_doc_id: String,
    pub tool_call_id: String,
    pub node_did: String,
    pub agent_id: String,
    pub request_hop: u64,
    pub correlation: Value,
    pub cwd: String,
    pub started_by_job: Value,
}

/// The pin. The guest supplies the first four fields; the host's own facts ride in `host`
/// so an input pin round-trips whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobPin {
    pub script_id: String,
    pub source_digest: String,
    pub agent_budget: u64,
    pub handle: String,
    #[serde(flatten)]
    pub host: Map,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootCall {
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_budget: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifiedScript {
    pub source: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    pub pack: String,
    pub class: String,
    pub digest: String,
    pub path: String,
    pub trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ceiling {
    pub targets: Value,
    pub max_live: u64,
    pub remaining_spawns: u64,
    pub max_messages_per_slot: u64,
    pub result_bytes: u64,
    pub dynamic: Value,
    pub dynamic_targets: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    pub attempt: u64,
    pub session_id: String,
    pub request_id: String,
    pub title: String,
    pub state: String,
    pub output_bytes: u64,
    pub truncated: bool,
    pub usage: Usage,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Resume,
    Answer,
    EngineCurrent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub kind: ControlKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEntry {
    pub row_doc_id: String,
    pub tool_call_id: String,
    pub handle: String,
    pub state: JobState,
    pub adopted_at: u64,
    pub resume_of: Option<String>,
}

/// C1 `JobStepInput`. `pin`, `origin` and `control` are always present on the wire (null
/// when unset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStepInput {
    pub protocol: u32,
    pub job: JobRef,
    pub caller: Caller,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_call: Option<RootCall>,
    pub pin: Option<JobPin>,
    pub origin: Option<Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_script: Option<VerifiedScript>,
    pub mounts: Vec<Mount>,
    pub ceiling: Ceiling,
    pub children: BTreeMap<String, Child>,
    pub control: Option<Control>,
    pub scratch: BTreeMap<String, String>,
    pub jobs: Vec<JobEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendKind {
    Pause,
    AwaitUser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suspend {
    pub kind: SuspendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<PauseReason>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOp {
    Stop,
    Pause,
}

/// Control request on a non-job `result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub row_doc_id: String,
    pub kind: ControlOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub row_doc_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spawn {
    pub slot: String,
    pub attempt: u64,
    pub target: String,
    pub prompt: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub slot: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interrupt {
    pub slot: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<JobPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Resume>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<Vec<Spawn>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Vec<Message>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<Vec<Interrupt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes: Option<Vec<DataWrite>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<Vec<ScratchFile>>,
}

/// C1 `JobStepOutput`: exactly one of `result`, `fail`, `suspend` or `effects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JobStepOutput {
    Result {
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writes: Option<Vec<DataWrite>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scratch: Option<Vec<ScratchFile>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<JobProgress>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<Vec<ControlRequest>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<JobPin>,
    },
    Fail {
        fail: Failure,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writes: Option<Vec<DataWrite>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<JobProgress>,
    },
    Suspend {
        suspend: Suspend,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writes: Option<Vec<DataWrite>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scratch: Option<Vec<ScratchFile>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<JobProgress>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<JobPin>,
    },
    Effects {
        effects: Effects,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnConflict {
    Ignore,
    Fail,
}

/// C3 `DataWrite`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataWrite {
    pub op: String,
    pub collection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_id: Option<String>,
    pub fields: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_conflict: Option<OnConflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Paused,
    AwaitingUser,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    User,
    BackOff,
    NoProgress,
    Verification,
    Infra,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseState {
    Pending,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phase {
    pub title: String,
    pub state: PhaseState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveChild {
    pub slot: String,
    pub title: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressBudget {
    pub total: u64,
    pub spent: u64,
    pub remaining: u64,
}

/// C6 `JobProgress`; the host owns `revision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProgress {
    pub title: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<PauseReason>,
    pub phases: Vec<Phase>,
    pub active: Vec<ActiveChild>,
    pub spawned: u64,
    pub budget: ProgressBudget,
    pub last_event: String,
    pub last_event_at: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub observed_terminal_children: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    Pack,
    Project,
    Kept,
}

/// C6 `CatalogEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub source: CatalogSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The raw-argument input (no `protocol`): `argv`, the reserved `asset_mounts` (C4) and the
/// `data_calls` offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawInput {
    pub argv: Vec<String>,
    #[serde(default)]
    pub asset_mounts: Vec<Mount>,
    #[serde(default)]
    pub data_calls: bool,
}

/// A JSON object from a `json!` literal, for the `filter` and `order` maps.
pub fn object(v: Value) -> Map {
    match v {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// C3 `DataQuery`: `offset` is omitted when none, `order` when unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataQuery {
    pub collection: String,
    pub filter: Map,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Map>,
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    pub fields: Vec<String>,
}

/// One `data_calls` request; its answer is `data_results[id]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataCall {
    pub id: String,
    pub query: DataQuery,
}

/// The rows a `data_calls` query returns, each holding the requested `fields`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataQueryResponse {
    pub rows: Vec<Map>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildOutputRequest {
    pub slots: Vec<String>,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobResultRequest {
    pub row_doc_id: String,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobResultResponse {
    pub row_doc_id: String,
    pub state: JobState,
    pub result: String,
    pub offset: u64,
    pub total_bytes: u64,
    pub truncated: bool,
}

/// One `job_calls` request; its answer is `job_results[id]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum JobCall {
    ChildOutput {
        id: String,
        child_output: ChildOutputRequest,
    },
    Result {
        id: String,
        result: JobResultRequest,
    },
}

/// The answer to one round request, keyed by the request's `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum RoundResult<T> {
    Result { result: T },
    Error { error: String },
}

/// One slot's text from the requested offset; `truncated` (omitted when false) when the text
/// continues past this slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildOutputSlice {
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildOutputResponse {
    pub slots: BTreeMap<String, ChildOutputSlice>,
}
