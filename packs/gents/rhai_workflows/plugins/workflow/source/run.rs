//! `run` on an inline script (L2 sections 4.6 and 4.10): step 0 validates, pins and replays to
//! the first pending call; later steps replay with the stored notes and source.
use crate::data::WfScript;
use crate::errors::{
    WORKFLOW_DEPTH_EXCEEDED, WORKFLOW_INVALID_INPUT, WORKFLOW_LAUNCH_FAILED,
    WORKFLOW_POLICY_DENIED, WORKFLOW_RESOLVE_FAILED, WORKFLOW_VALIDATION_FAILED, message,
};
use crate::fail;
use crate::host_api::{self, Host};
use crate::journal::{Journal, run_id};
use crate::origin;
use crate::progress::{self, Kind, pause_reason};
use crate::resume;
use crate::rounds::Replay;
use crate::sandbox::{self, LogBuffer, Suspension, SuspensionCell};
use crate::tiers::{self, Resolved};
use crate::validate::{self, Meta};
use crate::verbs;
use crate::wire::{Child, JobPin, Origin};
use rhai::Scope;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

/// The host API version a `script_id` is hashed with.
pub(crate) const HOST_API_VERSION: &str = "gents-rhai/1";
pub(crate) const DEFAULT_AGENT_BUDGET: u64 = 128;
/// The bounds of `agent_budget` (root C9).
pub(crate) const AGENT_BUDGET: std::ops::RangeInclusive<u64> = 1..=1024;

/// The `agent_budget` of a `run`: the default when absent, else an integer within the bounds.
fn agent_budget(input: &Value) -> Result<u64, Value> {
    match &input["agent_budget"] {
        Value::Null => Ok(DEFAULT_AGENT_BUDGET),
        value => value
            .as_u64()
            .filter(|n| AGENT_BUDGET.contains(n))
            .ok_or_else(|| {
                let what = format!("agent_budget {value} is not an integer within 1..1024");
                fail(
                    WORKFLOW_INVALID_INPUT,
                    &format!("{what}. Next: workflow help run"),
                )
            }),
    }
}

pub(crate) fn sha256_hex(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    parts.iter().for_each(|p| hash.update(p.as_bytes()));
    format!("{:x}", hash.finalize())
}

/// A script digest as the host writes it: `sha256:<hex>`.
pub(crate) fn sha256_digest(parts: &[&str]) -> String {
    format!("sha256:{}", sha256_hex(parts))
}

/// `run <name> --budget <n>`: the flag becomes `agent_budget`, for a caller that cannot send the
/// field next to `args`. The flag leaves `argv`; a value outside the bounds is refused.
pub(crate) fn budget_flag(input: &mut Value) -> Result<(), Value> {
    let argv = input["argv"].as_array().cloned().unwrap_or_default();
    let at = argv.iter().position(|a| a == "--budget");
    let (Some(at), Some("run")) = (at, argv.first().and_then(Value::as_str)) else {
        return Ok(());
    };
    let value = argv.get(at + 1).and_then(Value::as_str);
    let Some(budget) = value
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| (1..=1024).contains(n))
    else {
        let what = "--budget takes an integer within 1..1024";
        return Err(crate::fail(
            WORKFLOW_INVALID_INPUT,
            &format!("{what}. Next: workflow help run"),
        ));
    };
    input["argv"] = json!(
        argv.iter()
            .enumerate()
            .filter(|(i, _)| *i != at && *i != at + 1)
            .map(|(_, a)| a)
            .collect::<Vec<_>>()
    );
    if input["agent_budget"].is_null() {
        input["agent_budget"] = json!(budget);
    }
    Ok(())
}

/// A `fail` whose `message` is already `<code>: <what>. Next: <one call>`.
pub(crate) fn refused(code: &str, message: String) -> Value {
    json!({"fail": {"code": code, "message": message}})
}

/// `name`, or `name-<n>` with n one above the highest suffix among `jobs[].handle`.
fn handle(name: &str, jobs: &Value) -> String {
    let highest = (jobs.as_array().into_iter().flatten())
        .filter_map(|job| job["handle"].as_str())
        .filter_map(|h| match h == name {
            true => Some(1),
            false => h.strip_prefix(name)?.strip_prefix('-')?.parse::<u64>().ok(),
        })
        .max();
    highest.map_or(name.into(), |n| format!("{name}-{}", n + 1))
}

/// What a step-0 replay adopts: the WfScript write of an inline run (a resume has none), the
/// `last_event` line and, on a resume, its `resume` block, which rides beside the pin.
pub(crate) struct Adopting {
    pub script: Option<Value>,
    pub last_event: String,
    pub resume: Option<Value>,
}

/// Step 0 (no pin yet): resolves the source (L2 section 4.13), then the pin, the first spawn
/// window and the notes.
pub fn step0(input: &Value) -> Value {
    if let Some(out) = verbs::deferred(input) {
        return out;
    }
    if let Some(verb) = input["argv"][0].as_str().filter(|v| *v != "run") {
        return match verb {
            "help" | "check" | "list" | "show" => verbs::inline(input),
            _ => verbs::not_run(verb),
        };
    }
    let Some(row) = input.pointer("/job/row_doc_id").and_then(Value::as_str) else {
        return fail(
            WORKFLOW_INVALID_INPUT,
            "the input has no job.row_doc_id. Next: workflow help",
        );
    };
    if let Some(msg) = origin::nested(input) {
        return refused(WORKFLOW_DEPTH_EXCEEDED, msg);
    }
    let agent_budget = match agent_budget(input) {
        Ok(budget) => budget,
        Err(out) => return out,
    };
    let Resolved { source, tier } = match tiers::resolve(input) {
        Ok(resolved) => resolved,
        Err(out) => return out,
    };
    let source = source.as_str();
    let meta = match validate::validate(source) {
        Ok(meta) => meta,
        Err(msg) => return refused(WORKFLOW_VALIDATION_FAILED, msg),
    };
    let pin = JobPin {
        script_id: sha256_hex(&[HOST_API_VERSION, source]),
        source_digest: sha256_digest(&[source]),
        agent_budget,
        handle: handle(&meta.name, &input["jobs"]),
        host: Map::new(),
    };
    let mut journal = Journal::new(Replay {
        notes: Vec::new(),
        source: None,
        child_text: BTreeMap::new(),
    });
    let mut run = json!({
        "script_id": pin.script_id, "source_digest": pin.source_digest, "args": input["args"],
        "handle": pin.handle, "agent_budget": pin.agent_budget
    });
    if let Some(path) = input.get("path") {
        run["path"] = path.clone();
    }
    journal.put(&run_id(row), run);
    let script = WfScript::draft(
        &pin.script_id,
        source,
        tier.hint(),
        &meta.name,
        &meta.description,
    );
    let predicted = match origin::of(input, source) {
        Origin::Installed => "installed",
        Origin::Dynamic => "dynamic",
    };
    let last_event = format!(
        "started from {} (predicted origin {predicted}, {})",
        tier.label(),
        pin.source_digest
    );
    let adopting = Adopting {
        script: Some(script.write()),
        last_event,
        resume: None,
    };
    replay(input, source, &meta, journal, &pin, Some(adopting))
}

/// A later step: replays the script over the notes, source and child text the rounds gathered.
pub fn step(input: &Value, replayed: Replay) -> Value {
    let Some(pin) =
        (input.get("pin")).and_then(|p| serde_json::from_value::<JobPin>(p.clone()).ok())
    else {
        return fail(
            WORKFLOW_INVALID_INPUT,
            "the input has no readable pin. Next: workflow help",
        );
    };
    let stored = replayed.source.clone();
    let journal = Journal::new(replayed);
    // The host's verified bytes when it passes a `verified_script` (the rounds then skip the
    // WfScript fetch), else the stored script; never the echoed `script`.
    let verified = input.get("verified_script").filter(|v| !v.is_null());
    let source = match verified {
        Some(v) => v["source"].as_str().map(String::from),
        None => stored,
    };
    let Some(source) = source else {
        return match verified {
            Some(_) => {
                let what = "the verified script carries no source";
                let msg = message(WORKFLOW_VALIDATION_FAILED, what, "workflow list");
                refused(WORKFLOW_VALIDATION_FAILED, msg)
            }
            None => fail(
                WORKFLOW_RESOLVE_FAILED,
                "the script source is not stored here. Next: workflow list",
            ),
        };
    };
    let which = if verified.is_some() {
        "installed"
    } else {
        "stored"
    };
    if sha256_digest(&[&source]) != pin.source_digest {
        let what = format!("the {which} script's bytes differ from the pinned digest");
        let msg = message(WORKFLOW_VALIDATION_FAILED, &what, "workflow list");
        return refused(WORKFLOW_VALIDATION_FAILED, msg);
    }
    match validate::validate(&source) {
        Ok(meta) => match resume::rooted(input, &journal, &meta.name) {
            Ok(input) => replay(&input, &source, &meta, journal, &pin, None),
            Err(out) => out,
        },
        Err(msg) => refused(WORKFLOW_VALIDATION_FAILED, msg),
    }
}

/// Adds the scratch records this replay created to an output (or to its `effects`).
fn carry_scratch(out: &mut Value, host: &Host) {
    if !host.scratch.is_empty() {
        out["scratch"] = json!(host.scratch);
    }
}

/// Runs the script to its first suspension. `adopting` is the WfScript write and the
/// `last_event` line of step 0, whose output carries the pin.
pub(crate) fn replay(
    input: &Value,
    source: &str,
    meta: &Meta,
    journal: Journal,
    pin: &JobPin,
    adopting: Option<Adopting>,
) -> Value {
    let mut children: BTreeMap<String, Child> = match &input["children"] {
        Value::Null => BTreeMap::new(),
        block => match serde_json::from_value(block.clone()) {
            Ok(children) => children,
            Err(e) => {
                let what = format!("children do not decode ({e})");
                return fail(
                    WORKFLOW_INVALID_INPUT,
                    &format!("{what}. Next: workflow help"),
                );
            }
        },
    };
    resume::retry(&mut children, &journal, input);
    let mut host = Host::new(journal, children, &meta.name);
    host.targets = (input.pointer("/ceiling/targets").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    let policy = match origin::policy(input, source, &meta.name) {
        Ok(policy) => policy,
        Err(msg) => return refused(WORKFLOW_POLICY_DENIED, msg),
    };
    if let Some(allowed) = policy.targets {
        host.targets.retain(|t| allowed.contains(t));
    }
    host.spawn_denied = (policy.spawn_refusal).map(|msg| refused(WORKFLOW_POLICY_DENIED, msg));
    host.remaining_spawns = (input.pointer("/ceiling/remaining_spawns"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    host.max_live = input
        .pointer("/ceiling/max_live")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    host.max_messages = input
        .pointer("/ceiling/max_messages_per_slot")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    host.host_scratch = (input["scratch"].as_object().into_iter().flatten())
        .filter_map(|(name, path)| Some((name.clone(), path.as_str()?.to_string())))
        .collect();
    host.run_dir = match (
        input.pointer("/caller/cwd").and_then(Value::as_str),
        input
            .pointer("/caller/tool_call_id")
            .and_then(Value::as_str),
    ) {
        (Some(cwd), Some(call)) => {
            let dir = Path::new(cwd).join(".gents/workflows/runs").join(call);
            Some(dir.to_string_lossy().into_owned())
        }
        _ => None,
    };
    host.budget = pin.agent_budget;
    host.answer = resume::answer_of(input);
    host.row = (input.pointer("/job/row_doc_id").and_then(Value::as_str)).map(String::from);
    host.address = match (
        input.pointer("/caller/session_id").and_then(Value::as_str),
        input
            .pointer("/caller/tool_call_id")
            .and_then(Value::as_str),
    ) {
        (Some(session), Some(call)) => format!("{session}:{call}"),
        _ => pin.handle.clone(),
    };
    if adopting.is_some() {
        host.tag = format!(" (run {}, handle {})", host.address, pin.handle);
    }
    let host = Rc::new(RefCell::new(host));
    let (log, cell) = (LogBuffer::default(), SuspensionCell::default());
    let mut engine = sandbox::engine(&log, &cell, false);
    host_api::register(&mut engine, &host, &cell);
    let args = match rhai::serde::to_dynamic(&input["args"]) {
        Ok(args) => args,
        Err(e) => {
            let what = format!("args do not convert to a script value ({e})");
            return fail(
                WORKFLOW_INVALID_INPUT,
                &format!("{what}. Next: workflow help"),
            );
        }
    };
    let mut scope = Scope::new();
    scope.push_constant("args", args);
    let ran = engine.run_with_scope(&mut scope, source);

    let host = host.borrow();
    let (later, last_event) = (
        adopting.is_none(),
        adopting.as_ref().map(|a| a.last_event.as_str()),
    );
    let resume = adopting.as_ref().and_then(|a| a.resume.clone());
    let pin_json = || serde_json::to_value(pin).expect("pin serializes");
    let failed = |mut out: Value| {
        let held = host.journal.writes(later);
        if !held.is_empty() {
            out["writes"] = json!(held);
        }
        out["progress"] = progress::build(meta, pin, &host, Kind::Failed, last_event);
        out
    };
    if let Some(refusal) = &host.refusal {
        return failed(refusal.clone());
    }
    let token = cell.borrow().clone();
    let launch_failed = |what: String| {
        let next = "end the script with complete(value)";
        failed(refused(
            WORKFLOW_LAUNCH_FAILED,
            message(WORKFLOW_LAUNCH_FAILED, &what, next),
        ))
    };
    // A suspend is always adopted: at step 0 it carries the pin, and its notes with it.
    let suspended = |suspend: Value, kind: Kind, mut writes: Vec<Value>| {
        writes.extend(host.journal.writes(true));
        let progress = progress::build(meta, pin, &host, kind, last_event);
        let mut out = json!({"suspend": suspend, "writes": writes, "progress": progress});
        if !later {
            out["pin"] = pin_json();
            if let Some(resume) = &resume {
                out["resume"] = resume.clone();
            }
        }
        carry_scratch(&mut out, &host);
        out
    };
    let mut writes: Vec<Value> = adopting.iter().filter_map(|a| a.script.clone()).collect();
    match (token, ran) {
        (Some(Suspension::Pending), _) => {
            writes.extend(host.journal.writes(true));
            let progress = progress::build(meta, pin, &host, Kind::Running, last_event);
            let mut effects = json!({"spawn": host.spawns, "writes": writes, "progress": progress});
            if !host.messages.is_empty() {
                effects["message"] = json!(host.messages);
            }
            if !later {
                effects["pin"] = pin_json();
                if let Some(resume) = &resume {
                    effects["resume"] = resume.clone();
                }
            }
            carry_scratch(&mut effects, &host);
            json!({"effects": effects})
        }
        (Some(Suspension::Complete(result)), _) => {
            // A step-0 result that carries scratch, or that ends a resume, is adopted: it carries
            // the pin (and the resume block) and the notes.
            let pinned = !later && (!host.scratch.is_empty() || resume.is_some());
            writes.extend(host.journal.writes(later || pinned));
            let progress = progress::build(meta, pin, &host, Kind::Completed, last_event);
            // The caller must see the run's address and handle to keep or fetch it.
            // serde_json sorts keys, so "answer" leads the wrapper.
            let result = match result {
                Value::String(s) if pinned => json!(format!("{s}{}", host.tag)),
                value if pinned => {
                    json!({"answer": value, "run": host.address, "handle": pin.handle})
                }
                value => value,
            };
            let mut out = json!({"result": result, "writes": writes, "progress": progress});
            if pinned {
                out["pin"] = pin_json();
                if let Some(resume) = &resume {
                    out["resume"] = resume.clone();
                }
            }
            carry_scratch(&mut out, &host);
            out
        }
        (Some(Suspension::Pause { kind, message }), _) => {
            let reason = pause_reason(&kind).map(|r| json!(r));
            let suspend = json!({"kind": "pause", "reason": reason, "message": message});
            suspended(suspend, Kind::Paused(kind), writes)
        }
        (Some(Suspension::AwaitUser(ask)), _) => {
            let kind = ask["kind"].as_str().unwrap_or_default();
            let mut suspend = json!({"kind": "await_user", "message": ask["message"],
                                     "question": ask["question"]});
            // Only a kind in the closed enum is a `reason`; any other kind has none.
            if let Some(reason) = pause_reason(kind) {
                suspend["reason"] = json!(reason);
            }
            let question = ask["question"].as_str().unwrap_or_default();
            suspended(suspend, Kind::Awaiting(question.into()), writes)
        }
        (None, Ok(())) => launch_failed("the script ended without calling complete".into()),
        (None, Err(e)) => launch_failed(format!("the script stopped with an error ({e})")),
    }
}
