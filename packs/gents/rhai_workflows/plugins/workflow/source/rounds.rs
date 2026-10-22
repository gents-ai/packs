//! The round client (L2 section 4.3), after the ocr plugin's `model_calls` client.
//!
//! The host passes `<svc>_calls: true` for each service it offers. The plugin asks for one
//! service per output, `{<svc>_calls: {requests, state}}`; the next input is the base input
//! plus that round's `<svc>_results` (keyed by request id) and the echoed `state`. Only the
//! latest round's results are present, and a call has at most [`MAX_ROUNDS`] rounds.
//!
//! A replay step takes up to three invocations: notes and source (`data_calls`), child text
//! (`job_calls.child_output`), then replay. [`advance`] decides which one the input is at.
use crate::data::{self, PAGE};
use crate::fail;
use crate::wire::{
    ChildOutputRequest, ChildOutputResponse, DataQueryResponse, JobCall, RoundResult,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Rounds the host serves one call (gents `plugin/rounds.rs` `MAX_ROUNDS`).
pub const MAX_ROUNDS: u32 = 64;
/// The output cap a carried state must fit under (4 MiB).
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// What a step carries between rounds. Child text is never part of it.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StepState {
    pub rounds: u32,
    pub notes: Vec<Value>,
    pub notes_done: bool,
    pub source: Option<String>,
}

/// Everything a replay needs: notes and source from the state, child text from this input.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub notes: Vec<Value>,
    pub source: Option<String>,
    pub child_text: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum Step {
    /// An output asking the host for one service.
    Ask(Value),
    Replay(Replay),
}

fn offered(input: &Value, service: &str) -> Result<(), Value> {
    if input.get(format!("{service}_calls")) == Some(&Value::Bool(true)) {
        return Ok(());
    }
    Err(fail(
        "workflow_not_available",
        &format!("the host did not offer {service}_calls to this call. Next: workflow help doctor"),
    ))
}

fn invalid(what: &str) -> Value {
    fail(
        "workflow_invalid_input",
        &format!("{what}. Next: workflow help"),
    )
}

fn ask(service: &str, requests: Vec<Value>, mut state: StepState) -> Result<Step, Value> {
    if state.rounds >= MAX_ROUNDS {
        return Err(fail(
            "workflow_launch_failed",
            &format!(
                "the step needs more than {MAX_ROUNDS} rounds of host requests. Next: lower result_bytes or split the panel"
            ),
        ));
    }
    state.rounds += 1;
    let mut out = Map::new();
    out.insert(
        format!("{service}_calls"),
        json!({"requests": requests, "state": state}),
    );
    let out = Value::Object(out);
    if out.to_string().len() > MAX_OUTPUT_BYTES {
        return Err(fail(
            "workflow_launch_failed",
            "the carried notes and source pass the 4 MiB output cap. Next: lower result_bytes or split the panel",
        ));
    }
    Ok(Step::Ask(out))
}

/// The result of one answered request; a host error is the step's failure and an answer that
/// is not a `RoundResult` of `T` is invalid input.
pub(crate) fn answer<T: DeserializeOwned>(results: &Value, id: &str) -> Result<Option<T>, Value> {
    let Some(one) = results.get(id) else {
        return Ok(None);
    };
    match serde_json::from_value::<RoundResult<T>>(one.clone()) {
        Ok(RoundResult::Result { result }) => Ok(Some(result)),
        Ok(RoundResult::Error { error }) => Err(fail(
            "workflow_launch_failed",
            &format!("a host request failed ({error}). Next: workflow help errors"),
        )),
        Err(e) => Err(invalid(&format!(
            "the {id} answer is not a round result of the asked shape ({e})"
        ))),
    }
}

fn absorb_data(results: &Value, st: &mut StepState) -> Result<(), Value> {
    if let Some(r) = answer::<DataQueryResponse>(results, "notes")? {
        let rows = r.rows;
        if !rows
            .iter()
            .all(|row| row.get("_docID").is_some_and(Value::is_string))
        {
            return Err(invalid("a notes row has no _docID"));
        }
        let readable = |row: &Map<String, Value>| {
            let text = row.get("value_json").and_then(Value::as_str);
            text.is_some_and(|t| serde_json::from_str::<Value>(t).is_ok())
        };
        if !rows.iter().all(readable) {
            return Err(invalid("a notes row has no readable value_json"));
        }
        st.notes_done = rows.len() < PAGE as usize;
        st.notes.extend(rows.into_iter().map(Value::Object));
    }
    if let Some(r) = answer::<DataQueryResponse>(results, "source")? {
        let source = r.rows.first().and_then(|row| row.get("source"));
        st.source = Some(
            source
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    fail(
                        "workflow_resolve_failed",
                        "the script source is not stored here. Next: workflow list",
                    )
                })?
                .to_string(),
        );
    }
    Ok(())
}

/// Decides what the input at hand needs: another data round, the child-text round, or replay.
pub fn advance(input: &Value) -> Result<Step, Value> {
    let mut st: StepState = match input.get("state") {
        None | Some(Value::Null) => StepState::default(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
            invalid(&format!(
                "the echoed state is not one this plugin wrote ({e})"
            ))
        })?,
    };
    if let Some(results) = input.get("data_results") {
        absorb_data(results, &mut st)?;
    }

    let script_id = input.pointer("/pin/script_id").and_then(Value::as_str);
    let host_has_source = input.get("verified_script").is_some_and(|v| !v.is_null());
    let want_source = script_id.is_some() && !host_has_source && st.source.is_none();
    if !st.notes_done || want_source {
        offered(input, "data")?;
        let chain = input
            .pointer("/job/chain_root_doc_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the input has no job.chain_root_doc_id"))?;
        let mut requests = Vec::new();
        if !st.notes_done {
            // `_docID` is a total order, so pages never skip or repeat a P2P twin.
            requests.push(json!(data::notes_page(chain, st.notes.len())));
        }
        if let (true, Some(script_id)) = (want_source, script_id) {
            requests.push(json!(data::script_source(script_id)));
        }
        return ask("data", requests, st);
    }

    // Completed slots whose value no out: note already holds; the host serves text for no other
    // terminal (a failed or interrupted child has none, and its read is empty). A note that
    // recorded a request id holds the value only while that request is the slot's latest: a
    // later request (a `resume_from` reply) is read from the text.
    let noted = |slot: &str, child: &Value| {
        let id = format!("out:{slot}");
        let covers = |n: &Value| {
            let value = n["value_json"].as_str();
            let seen = value.and_then(|v| serde_json::from_str::<Value>(v).ok());
            let seen = seen.as_ref().and_then(|v| v["request_id"].as_str());
            seen.is_none_or(|r| child["request_id"] == r)
        };
        st.notes
            .iter()
            .any(|n| n["note_id"] == id.as_str() && covers(n))
    };
    let slots: Vec<&String> = input["children"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(slot, child)| child["state"] == "completed" && !noted(slot, child))
        .map(|(slot, _)| slot)
        .collect();
    if let Some(results) = input.get("job_results") {
        let r = answer::<ChildOutputResponse>(results, "text")?
            .ok_or_else(|| invalid("the child_output answer is missing"))?;
        let mut child_text = BTreeMap::new();
        for slot in slots {
            let one = r.slots.get(slot).ok_or_else(|| {
                invalid(&format!("the child_output answer has no text for {slot}"))
            })?;
            child_text.insert(slot.clone(), one.text.clone());
        }
        return Ok(Step::Replay(Replay {
            notes: st.notes,
            source: st.source,
            child_text,
        }));
    }
    if slots.is_empty() {
        return Ok(Step::Replay(Replay {
            notes: st.notes,
            source: st.source,
            child_text: BTreeMap::new(),
        }));
    }
    offered(input, "job")?;
    let limit = input
        .pointer("/ceiling/result_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("the input has no ceiling.result_bytes"))?;
    let slots = slots.into_iter().cloned().collect();
    let request = JobCall::ChildOutput {
        id: "text".into(),
        child_output: ChildOutputRequest {
            slots,
            offset: 0,
            limit,
        },
    };
    ask("job", vec![json!(request)], st)
}
