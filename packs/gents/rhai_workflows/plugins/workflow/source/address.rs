//! `<key|handle>` resolution (L2 section 4.10; root C2.2): the one lookup `resume`, `answer`,
//! `stop`, `pause`, `status` and `result` share.
use crate::errors::WORKFLOW_CONTROL_UNKNOWN_RUN;
use crate::fail;
use serde_json::Value;

/// An address `session_id:tool_call_id` splits at the last colon: a child session id holds
/// colons, a tool-call id never does.
pub fn split(key: &str) -> Option<(&str, &str)> {
    key.rsplit_once(':')
}

/// The run's address, as every receipt prints it.
pub fn address(input: &Value, job: &Value) -> String {
    let session = input.pointer("/caller/session_id").and_then(Value::as_str);
    let call = job["tool_call_id"].as_str().unwrap_or_default();
    format!("{}:{call}", session.unwrap_or_default())
}

/// The `jobs[]` entry `key` names: an address of this session, else a row, else a tool-call id of
/// this session, else the newest row of that handle's chain.
pub fn resolve<'a>(input: &'a Value, key: &str) -> Result<&'a Value, Value> {
    let jobs = input["jobs"].as_array().map_or(&[][..], Vec::as_slice);
    let found = match split(key) {
        Some((session, call)) => {
            let own = input.pointer("/caller/session_id").and_then(Value::as_str);
            let call = |j: &&Value| j["tool_call_id"] == call;
            own.filter(|own| *own == session)
                .and_then(|_| jobs.iter().find(call))
        }
        // The host lists rows oldest first, so the newest row of a handle is the last one listed.
        // A bare tool-call id is what a model keeps when it cuts the address down.
        None => (jobs.iter().find(|j| j["row_doc_id"] == key))
            .or_else(|| jobs.iter().find(|j| j["tool_call_id"] == key))
            .or_else(|| jobs.iter().rev().find(|j| j["handle"] == key)),
    };
    found.ok_or_else(|| {
        let what = format!(
            "{key} is not a run of this session. Next: workflow status <key> with the address from the run's receipt"
        );
        fail(WORKFLOW_CONTROL_UNKNOWN_RUN, &what)
    })
}
