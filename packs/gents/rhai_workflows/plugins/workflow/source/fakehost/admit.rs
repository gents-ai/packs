//! Admission (root C5, loosely; not a conformance oracle) and the kill path.
use super::adopt::Chain;
use super::state::{ChildRecord, State};
use crate::wire::Usage;
use serde_json::Value;
use std::collections::BTreeMap;

/// A host refusal: the job ends with `code`.
#[derive(Debug)]
pub struct Denied {
    pub code: &'static str,
    pub why: String,
}

fn denied(why: String) -> Denied {
    Denied {
        code: "plugin_job_policy_denied",
        why,
    }
}

/// The new child requests (by `(slot, attempt)`) and interrupted slots an output admits.
#[derive(Debug, Default)]
pub struct Plan {
    pub spawns: Vec<Value>,
    pub interrupts: Vec<String>,
}

fn strings<'a>(v: &'a Value) -> Vec<&'a str> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn list<'a>(effects: &'a Value, key: &str) -> &'a [Value] {
    effects[key].as_array().map_or(&[], Vec::as_slice)
}

fn key(v: &Value) -> (String, u64) {
    (
        v["slot"].as_str().unwrap_or_default().into(),
        v["attempt"].as_u64().unwrap_or(0),
    )
}

/// The chain's children that are still running.
pub fn live(state: &State, chain: &Chain) -> Vec<(String, u64)> {
    let running = |k: &&(String, u64)| state.children.get(*k).is_some_and(|c| c.state == "running");
    chain.keys.iter().filter(running).cloned().collect()
}

/// Ends every live child of `chain`.
pub fn kill(state: &mut State, chain: &Chain) {
    for k in live(state, chain) {
        state.children.get_mut(&k).expect("live child").state = "interrupted".into();
    }
}

/// Checks one admitted-or-refused output against the ceiling. On `Ok` it charges `chain`
/// (new keys, messages); `state` is only read, `apply` commits the plan.
pub fn check(
    state: &State,
    chain: &mut Chain,
    ceiling: &Value,
    origin: &Value,
    out: &Value,
) -> Result<Plan, Denied> {
    let live_now = live(state, chain);
    if ["result", "fail", "suspend"]
        .iter()
        .any(|k| out.get(k).is_some())
    {
        return match live_now.len() {
            0 => Ok(Plan::default()),
            n => Err(denied(format!("a terminal while {n} children are live"))),
        };
    }
    let fx = &out["effects"];
    let (targets, dynamic) = (
        strings(&ceiling["targets"]),
        strings(&ceiling["dynamic_targets"]),
    );
    let mut spawns = Vec::new();
    for s in list(fx, "spawn") {
        if chain.keys.contains(&key(s)) {
            continue; // a re-served key is a no-op
        }
        let slot = s["slot"].as_str().unwrap_or_default();
        let latest = chain.keys.iter().filter(|k| k.0 == slot).map(|k| k.1).max();
        let want = latest.map_or(0, |a| a + 1);
        if s["attempt"].as_u64() != Some(want) {
            return Err(denied(format!(
                "slot {slot} attempt {} is not {want}: a fresh slot starts at 0, a re-attempt at the latest + 1",
                s["attempt"]
            )));
        }
        let t = s["target"].as_str().unwrap_or_default();
        if !targets.contains(&t) || (origin == "dynamic" && !dynamic.contains(&t)) {
            return Err(denied(format!("target {t} is outside the ceiling")));
        }
        spawns.push(s.clone());
    }
    let left = ceiling["remaining_spawns"]
        .as_u64()
        .unwrap_or(0)
        .saturating_sub(chain.charged);
    if spawns.len() as u64 > left {
        return Err(denied(format!(
            "{} new spawns pass the {left} left",
            spawns.len()
        )));
    }
    let interrupts: Vec<String> = list(fx, "interrupt")
        .iter()
        .filter_map(|i| i["slot"].as_str().map(Into::into))
        .collect();
    let stopped = live_now
        .iter()
        .filter(|(s, _)| interrupts.contains(s))
        .count();
    let live_after = live_now.len() - stopped + spawns.len();
    if live_after as u64 > ceiling["max_live"].as_u64().unwrap_or(0) {
        return Err(denied(format!("{live_after} live children pass max_live")));
    }
    let mut messages = chain.messages.clone();
    for m in list(fx, "message") {
        let n = messages
            .entry(m["slot"].as_str().unwrap_or_default().into())
            .or_insert(0u64);
        *n += 1;
        if *n > ceiling["max_messages_per_slot"].as_u64().unwrap_or(0) {
            return Err(denied(format!("slot {} passes the message cap", m["slot"])));
        }
    }
    let unobserved = chain.keys.iter().any(|k| {
        !chain.observed.contains(k) && state.children.get(k).is_some_and(|c| c.state != "running")
    });
    if live_after == 0
        && !unobserved
        && spawns.is_empty()
        && list(fx, "message").is_empty()
        && interrupts.is_empty()
    {
        return Err(Denied {
            code: "plugin_job_stalled",
            why: "a non-terminal output left nothing live, nothing to observe and nothing new"
                .into(),
        });
    }
    chain.keys.extend(spawns.iter().map(key));
    chain.charged += spawns.len() as u64;
    chain.messages = messages;
    Ok(Plan { spawns, interrupts })
}

/// Commits an admitted plan: interrupts first, then the new child requests, running.
pub fn apply(state: &mut State, root: &str, chain: &Chain, plan: &Plan) {
    for k in live(state, chain)
        .iter()
        .filter(|k| plan.interrupts.contains(&k.0))
    {
        state.children.get_mut(k).expect("live child").state = "interrupted".into();
    }
    for s in &plan.spawns {
        let (slot, attempt) = key(s);
        let rec = ChildRecord {
            session_id: format!("job-session:{root}:{slot}:{attempt}"),
            request_id: format!("job-request:{root}:{slot}:{attempt}"),
            title: s["title"].as_str().unwrap_or_default().into(),
            state: "running".into(),
            text: String::new(),
            usage: Usage {
                input: 0,
                output: 0,
                total: 0,
            },
            duration_ms: 0,
        };
        state.children.insert((slot, attempt), rec);
    }
}

/// The `children` input field: each slot's latest attempt in the C1 shape.
pub fn view(state: &State, chain: &Chain) -> Value {
    let mut out = BTreeMap::new();
    for k in &chain.keys {
        if let Some(c) = state.children.get(k) {
            out.insert(
                k.0.clone(),
                serde_json::json!({
                    "attempt": k.1, "session_id": c.session_id, "request_id": c.request_id,
                    "title": c.title, "state": c.state, "output_bytes": c.text.len(),
                    "truncated": false, "usage": c.usage, "duration_ms": c.duration_ms
                }),
            );
        }
    }
    serde_json::to_value(out).expect("children serialize")
}
