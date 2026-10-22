//! The verbs of the raw-argument shape (L2 sections 4.3 and 4.14): `help`, `check`, `list` and
//! `show` for a tool with no job block. Job verbs answer `workflow_not_available`. A raw output
//! never carries a WfNote write. The control and read verbs `stop`, `pause`, `status` and `result`
//! of a tool with a job block are non-job outputs (section 4.10): the calling row is never
//! adopted.
use crate::address;
use crate::catalog;
use crate::data::WfScript;
use crate::errors::{
    WORKFLOW_CONTROL_NOT_APPLICABLE, WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE,
    WORKFLOW_RESOLVE_FAILED, WORKFLOW_VALIDATION_FAILED, message,
};
use crate::export;
use crate::fail;
use crate::keep;
use crate::rounds;
use crate::run::{HOST_API_VERSION, refused, sha256_hex};
use crate::smoke::{self, End};
use crate::tiers;
use crate::validate;
use crate::wire::{JobCall, JobResultRequest, JobResultResponse, RawInput};
use serde_json::{Value, json};
use std::fs;

/// The verbs that need a job block.
const JOB_VERBS: [&str; 7] = [
    "run", "resume", "answer", "stop", "pause", "status", "result",
];

/// One page of `result`: 64 KiB.
const RESULT_PAGE: u64 = 64 * 1024;

/// Each verb with its one-line help. Full topics come with the help assets.
const VERBS: [(&str, &str); 14] = [
    ("help", "help [verb]: this index, or one verb"),
    (
        "list",
        "list: the installed workflows, names first, with when to use each",
    ),
    (
        "show",
        "show <name>: a workflow's description, phases and versions",
    ),
    (
        "check",
        "check (--script | path | <name>): validate a script, smoke-run it and store the draft",
    ),
    (
        "run",
        "run (<name> | script | path) [args] [--budget n]: start a workflow in the background",
    ),
    ("status", "status <key|handle>: where a run is"),
    ("result", "result <key|handle>: a finished run's result"),
    ("stop", "stop <key|handle>: end a run"),
    ("pause", "pause <key|handle>: hold a run"),
    (
        "resume",
        "resume <key|handle> [--answer t] [--budget n] [--engine current]: continue a paused or stalled run",
    ),
    (
        "answer",
        "answer <key|handle> <text>: reply to a run that asked a question",
    ),
    (
        "keep",
        "keep <key|handle|script_id>: keep a checked script under a name",
    ),
    (
        "export",
        "export <name>: write a kept workflow out as a pack",
    ),
    ("retire", "retire <name>: stop offering a kept workflow"),
];

/// `import` and `keep --from` answer `workflow_not_available` in every shape (U3).
pub fn deferred(input: &Value) -> Option<Value> {
    let argv: Vec<&str> = (input["argv"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let what = match argv.as_slice() {
        ["import", ..] => "import",
        ["keep", rest @ ..] if rest.contains(&"--from") => "keep --from",
        _ => return None,
    };
    Some(fail(
        WORKFLOW_NOT_AVAILABLE,
        &format!("{what} is deferred to v1.1. Next: workflow help"),
    ))
}

/// A verb other than `run` on a step-0 job call: only `run` starts a job, so it never adopts the
/// calling row.
pub fn not_run(verb: &str) -> Value {
    match VERBS.iter().any(|(name, _)| *name == verb) {
        true => fail(
            WORKFLOW_NOT_AVAILABLE,
            &format!("the {verb} verb does not start a job. Next: workflow help {verb}"),
        ),
        false => fail(
            WORKFLOW_INVALID_INPUT,
            &format!("unknown verb {verb:?}. Next: workflow help"),
        ),
    }
}

/// `help`, `check`, `list` and `show` on a step-0 job call end inline, as on the raw shape. The
/// job shape names the mounts `mounts`; the raw handlers read `asset_mounts`. The rest of the
/// input, the ceiling included, rides along.
pub fn inline(input: &Value) -> Value {
    let mut shaped = input.clone();
    if input["mounts"].is_array() {
        shaped["asset_mounts"] = input["mounts"].clone();
    }
    raw(&shaped)
}

/// Serves one raw-argument call.
pub fn raw(input: &Value) -> Value {
    if let Some(out) = deferred(input) {
        return out;
    }
    let raw: RawInput = match serde_json::from_value(input.clone()) {
        Ok(raw) => raw,
        Err(e) => {
            let what = format!("the raw input does not decode ({e})");
            return fail(
                WORKFLOW_INVALID_INPUT,
                &format!("{what}. Next: workflow help"),
            );
        }
    };
    let (verb, rest) = raw
        .argv
        .split_first()
        .map_or(("help", &[][..]), |(v, r)| (v.as_str(), r));
    match verb {
        "help" => help(rest, &raw),
        "check" => check(input, rest, &raw),
        "list" => catalog::list(input, rest, &raw),
        "show" => catalog::show(input, rest, &raw),
        "keep" | "retire" => keep::serve(input),
        "export" => export::serve(input),
        v if JOB_VERBS.contains(&v) => fail(
            WORKFLOW_NOT_AVAILABLE,
            "this tool has no job settings here. Next: workflow help doctor",
        ),
        v if VERBS.iter().any(|(name, _)| *name == v) => fail(
            WORKFLOW_NOT_AVAILABLE,
            &format!("the {v} verb is not built yet. Next: workflow help"),
        ),
        v => fail(
            WORKFLOW_INVALID_INPUT,
            &format!("unknown verb {v:?}. Next: workflow help"),
        ),
    }
}

fn help(rest: &[String], raw: &RawInput) -> Value {
    let text = match rest.first().map(String::as_str) {
        None => (VERBS.iter())
            .map(|(_, line)| *line)
            .collect::<Vec<_>>()
            .join("\n"),
        Some("doctor") => {
            let mounts = raw.asset_mounts.iter().filter(|m| m.class == "workflows");
            format!(
                "this call has no job settings, so {} are unavailable here; {} workflow mount(s) are visible",
                JOB_VERBS.join(", "),
                mounts.count()
            )
        }
        Some(verb) => match VERBS.iter().find(|(name, _)| *name == verb) {
            Some((_, line)) => (*line).into(),
            None => {
                let what = format!("no help for {verb:?}");
                return fail(
                    WORKFLOW_INVALID_INPUT,
                    &format!("{what}. Next: workflow help"),
                );
            }
        },
    };
    json!({"result": text})
}

/// `check (--script | path | <name>)`: static validation, the smoke run, then the draft.
fn check(input: &Value, rest: &[String], raw: &RawInput) -> Value {
    let (source, origin_hint) = match (
        rest.first().map(String::as_str),
        input["script"].as_str(),
        input["path"].as_str(),
    ) {
        (None | Some("--script"), Some(script), _) => (script.to_string(), "inline"),
        (None, None, Some(path)) => match fs::read_to_string(path) {
            Ok(source) => (source, "path"),
            Err(e) => {
                let what = format!("cannot read {path} ({e})");
                let next = "Next: workflow check --script";
                return fail(WORKFLOW_RESOLVE_FAILED, &format!("{what}. {next}"));
            }
        },
        (Some(name), ..) if !name.starts_with("--") => {
            // The tiers read `mounts`; the raw shape names them `asset_mounts`.
            let resolving = json!({"argv": ["check", name], "mounts": raw.asset_mounts,
                                   "data_calls": raw.data_calls,
                                   "data_results": input["data_results"]});
            match tiers::resolve(&resolving) {
                Ok(found) => (found.source, found.tier.hint()),
                Err(out) => return out,
            }
        }
        _ => {
            let what = "check needs a script, a path or a name";
            return fail(
                WORKFLOW_INVALID_INPUT,
                &format!("{what}. Next: workflow help check"),
            );
        }
    };
    let meta = match validate::validate(&source) {
        Ok(meta) => meta,
        Err(msg) => return refused(WORKFLOW_VALIDATION_FAILED, msg),
    };
    let args = if input["args"].is_object() {
        input["args"].clone()
    } else {
        json!({})
    };
    let smoke = match smoke::run(&source, &args) {
        Ok(smoke) => smoke,
        Err(why) => {
            let what = format!("the smoke run of {} failed: {why}", meta.name);
            let next = "fix the script, then run workflow check";
            let msg = message(WORKFLOW_VALIDATION_FAILED, &what, next);
            return refused(WORKFLOW_VALIDATION_FAILED, msg);
        }
    };
    let script_id = sha256_hex(&[HOST_API_VERSION, &source]);
    let stopped = match smoke.end {
        End::Complete => "reached complete()".to_string(),
        End::Pause => "reached a pause".to_string(),
        End::Question => "stopped at a question for the user".to_string(),
        End::CallLimit => format!("stopped at the {}-call limit", smoke::MAX_CALLS),
    };
    let receipt = format!(
        "The smoke check passed for {name}: the script ran against canned agent replies and \
         {stopped} after {calls} host calls. Draft stored as script_id {script_id}. Not covered: \
         real agent output, branching on real replies, the budget, targets and policy, and \
         anything after the run stopped.",
        name = meta.name,
        calls = smoke.calls,
    );
    let script = WfScript::draft(
        &script_id,
        &source,
        origin_hint,
        &meta.name,
        &meta.description,
    );
    json!({"result": receipt, "writes": [script.write()]})
}

pub fn is_control(input: &Value) -> bool {
    matches!(
        input["argv"][0].as_str(),
        Some("stop" | "pause" | "status" | "result")
    )
}

/// `stop`, `pause`, `status` and `result <key|handle> [--offset n]`.
pub fn control(input: &Value) -> Value {
    let argv: Vec<&str> = (input["argv"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let verb = argv[0];
    let (target, offset) = match (verb, &argv[1..]) {
        ("result", [target, "--offset", n]) => match n.parse::<u64>() {
            Ok(n) => (*target, n),
            Err(_) => return control_usage(verb),
        },
        (_, [target]) => (*target, 0),
        _ => return control_usage(verb),
    };
    let job = match address::resolve(input, target) {
        Ok(job) => job,
        Err(out) => return out,
    };
    let row = job["row_doc_id"].as_str().unwrap_or_default();
    let handle = job["handle"].as_str().unwrap_or(target);
    let state = job["state"].as_str().unwrap_or("unknown");
    let key = address::address(input, job);
    let not_applicable = |what: String, next: String| {
        fail(
            WORKFLOW_CONTROL_NOT_APPLICABLE,
            &format!("{what}. Next: {next}"),
        )
    };
    match verb {
        "status" => {
            let next = next_call(state, &key);
            json!({"result": format!("{handle} ({key}) is {state}. Next: {next}")})
        }
        "stop" | "pause" if state != "running" => not_applicable(
            format!("{handle} is {state}, not running"),
            format!("workflow status {key}"),
        ),
        "stop" => json!({
            "result": format!("Stopping {handle} ({key}). The final message arrives as usual."),
            "control": [{"row_doc_id": row, "kind": "stop"}]
        }),
        "pause" => json!({
            "result": format!("Pausing {handle} ({key}). Resume it with: workflow resume {handle}"),
            "control": [{"row_doc_id": row, "kind": "pause"}]
        }),
        _ => match rounds::answer::<JobResultResponse>(&input["job_results"], "result") {
            Err(out) => out,
            Ok(Some(page)) => {
                let end = page.offset + page.result.len() as u64;
                let mut text = page.result;
                if page.truncated {
                    text.push_str(&format!("\nNext: workflow result {key} --offset {end}"));
                }
                json!({"result": text})
            }
            Ok(None) if matches!(state, "running" | "paused" | "awaiting_user") => {
                let what = match state {
                    "running" => format!("{handle} is still running"),
                    _ => format!("{handle} is {state}, not finished"),
                };
                not_applicable(what, next_call(state, &key))
            }
            Ok(None) => {
                let request = JobCall::Result {
                    id: "result".into(),
                    result: JobResultRequest {
                        row_doc_id: row.into(),
                        offset,
                        limit: RESULT_PAGE,
                    },
                };
                json!({"job_calls": {"requests": [request], "state": {}}})
            }
        },
    }
}

/// The call that moves a run in `state` forward.
fn next_call(state: &str, key: &str) -> String {
    match state {
        "running" => "end your turn; the result arrives as a message".into(),
        "paused" => format!("workflow resume {key}"),
        "awaiting_user" => format!("workflow answer {key} <text>"),
        _ => format!("workflow result {key}"),
    }
}

fn control_usage(verb: &str) -> Value {
    let what = match verb {
        "result" => "result takes <key|handle> [--offset n]".to_string(),
        _ => format!("{verb} takes <key|handle>"),
    };
    fail(
        WORKFLOW_INVALID_INPUT,
        &format!("{what}. Next: workflow help {verb}"),
    )
}
