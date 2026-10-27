//! Serde mirrors of the gents job contract (`gents_protocol::job`), written by hand so the
//! plugin has no gents dependency and stays lenient: the host adds round keys
//! (`job_calls`, `job_results`, `state`) beside the step input, which the real
//! `deny_unknown_fields` type would refuse. Field names are the contract's, verbatim.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

type Map = serde_json::Map<String, Value>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caller {
    pub tool_call_id: String,
    pub cwd: String,
}

/// The pin. The guest supplies the first four fields; the host's own facts (`cwd`, `origin`,
/// `adopted_at`, ...) ride in `host` so an input pin round-trips whole.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobPin {
    pub script_id: String,
    pub source_digest: String,
    pub agent_budget: u64,
    pub handle: String,
    #[serde(flatten)]
    pub host: Map,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ceiling {
    pub max_live: u64,
    pub remaining_spawns: u64,
    pub result_bytes: u64,
}

/// One slot's latest request as the host sees it. `state` is `live` or a terminal word
/// (`completed`, `failed`, `interrupted`); only a completed child has text to read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Child {
    pub session_id: String,
    pub title: String,
    pub state: String,
    pub truncated: bool,
}

impl Child {
    pub fn live(&self) -> bool {
        self.state == "live"
    }
}

/// The step input (gents `JobStepInput`), plus the round keys the host adds to it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobStepInput {
    pub caller: Caller,
    pub args: Option<Value>,
    pub agent_budget: Option<u64>,
    pub pin: Option<JobPin>,
    pub ceiling: Ceiling,
    pub children: BTreeMap<String, Child>,
    /// The answers to the previous round's `job_calls` requests, keyed by request id.
    pub job_results: Option<Value>,
    /// This plugin's own state from its last step that carried one (gents `JobStepInput.state`).
    pub state: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendKind {
    Pause,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suspend {
    pub kind: SuspendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<PauseReason>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spawn {
    pub slot: String,
    pub attempt: u64,
    pub target: String,
    pub prompt: String,
    pub title: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub slot: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<JobPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<Vec<Spawn>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Vec<Message>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    /// This plugin's state for its next step; absent keeps the stored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

/// `JobStepOutput`: exactly one of `result`, `fail`, `suspend` or `effects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JobStepOutput {
    Result {
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scratch: Option<Vec<ScratchFile>>,
        progress: JobProgress,
    },
    Suspend {
        suspend: Suspend,
        progress: JobProgress,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<JobPin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<Value>,
    },
    Effects {
        effects: Effects,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Paused,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseState {
    Pending,
    Active,
    Done,
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

/// `JobProgress`; the host owns `revision` and stamps `last_event_at`.
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
    pub observed_terminal_children: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildOutputRequest {
    pub slots: Vec<String>,
    pub offset: u64,
    pub limit: u64,
}

/// One `job_calls` request; its answer is `job_results[id]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobCall {
    pub id: String,
    pub child_output: ChildOutputRequest,
}

/// The answer to one round request, keyed by the request's `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoundResult<T> {
    Result { result: T },
    Error { error: String },
}

/// One slot's text from the requested offset; `truncated` when the text continues past it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildOutputSlice {
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildOutputResponse {
    pub slots: BTreeMap<String, ChildOutputSlice>,
}
