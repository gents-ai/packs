//! Resume, guest side (L2 section 4.10; root C1, C5.3 and C8). Step 0 of `resume <row|handle>
//! [--answer t] [--budget n] [--engine current]` and of `answer <row|handle> <text>` reads the prior row's run note and the chain's notes, picks the slots to
//! re-attempt and replays with the chain root's arguments; steps from 1 on replay with the
//! host's `root_call` and fail closed when it differs from this row's run note.
use crate::errors::{
    WORKFLOW_CONTROL_NOT_APPLICABLE, WORKFLOW_CONTROL_UNKNOWN_RUN, WORKFLOW_DEPTH_EXCEEDED,
    WORKFLOW_INVALID_INPUT, WORKFLOW_RESOLVE_FAILED, WORKFLOW_RESUME_UNKNOWN_RUN,
    WORKFLOW_VALIDATION_FAILED,
};
use crate::fail;
use crate::journal::{Journal, need_id, reattempt_id, run_id};
use crate::origin;
use crate::rounds::{self, Replay, Step};
use crate::run::{self, Adopting, refused, sha256_digest};
use crate::validate;
use crate::wire::{Child, DataCall, DataQuery, DataQueryResponse, JobPin, object};
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// How many directories below a mount the source search goes.
const DEPTH: usize = 8;

/// The `run:<row>` note of the prior row: the pin facts and the chain root's arguments.
#[derive(Deserialize)]
struct RunNote {
    script_id: String,
    source_digest: String,
    handle: String,
    agent_budget: u64,
    #[serde(default)]
    args: Value,
    path: Option<String>,
}

pub fn is_resume(input: &Value) -> bool {
    matches!(input["argv"][0].as_str(), Some("resume" | "answer"))
}

/// What the verb asks for: the row or handle and the `resume` block's optional fields.
struct Request {
    target: String,
    budget: Option<u64>,
    engine_current: bool,
    answer: Option<String>,
}

/// `agent_budget` beside a `resume` or `answer` says `--budget`: a model that reads the input
/// schema sends the field as often as it writes the flag.
pub(crate) fn budget_field(input: &mut Value) {
    let argv = input["argv"].as_array().cloned().unwrap_or_default();
    let verb = argv.first().and_then(Value::as_str);
    if let (Some("resume" | "answer"), Some(budget), false) = (
        verb,
        input["agent_budget"].as_u64(),
        argv.iter().any(|a| a == "--budget"),
    ) {
        let mut argv = argv;
        argv.extend([json!("--budget"), json!(budget.to_string())]);
        input["argv"] = json!(argv);
    }
}

/// `resume <row|handle> [--answer t] [--budget n] [--engine current]`, or `answer <row|handle>
/// <text>` with the same flags after the text.
fn parse(input: &Value) -> Result<Request, Value> {
    let argv: Vec<&str> = (input["argv"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let usage = || {
        let what = match argv[0] {
            "answer" => "answer takes <row|handle> <text> [--budget n] [--engine current]",
            _ => "resume takes <row|handle> [--answer text] [--budget n] [--engine current]",
        };
        fail(
            WORKFLOW_INVALID_INPUT,
            &format!("{what}. Next: workflow help"),
        )
    };
    let (target, text, mut flags) = match &argv[1..] {
        [target, text, flags @ ..] if argv[0] == "answer" => (target, Some(text), flags),
        [target, flags @ ..] if argv[0] == "resume" => (target, None, flags),
        _ => return Err(usage()),
    };
    let mut req = Request {
        target: target.to_string(),
        budget: None,
        engine_current: false,
        answer: text.map(|t| t.to_string()),
    };
    while let [flag, value, tail @ ..] = flags {
        match *flag {
            "--budget" => req.budget = Some(value.parse().map_err(|_| usage())?),
            "--engine" if *value == "current" => req.engine_current = true,
            "--answer" if argv[0] == "resume" => req.answer = Some(value.to_string()),
            _ => return Err(usage()),
        }
        flags = tail;
    }
    match flags.is_empty() {
        true => Ok(req),
        false => Err(usage()),
    }
}

/// The `jobs[]` entry `target` names: a row, else the newest row of that handle's chain.
/// ponytail: the newest row is the last `jobs[]` entry with the handle; add an order key if the
/// host stops listing rows oldest first.
fn prior<'a>(input: &'a Value, target: &str) -> Result<&'a Value, Value> {
    let jobs = input["jobs"].as_array().map_or(&[][..], Vec::as_slice);
    (jobs.iter().find(|j| j["row_doc_id"] == target))
        .or_else(|| jobs.iter().rev().find(|j| j["handle"] == target))
        .ok_or_else(|| {
            let what = format!(
                "{target} is not a run of this session. Next: workflow status <key> with the address from the run's receipt"
            );
            fail(WORKFLOW_CONTROL_UNKNOWN_RUN, &what)
        })
}

/// Only a paused, awaiting_user, cancelled, failed or timed_out prior row is resumable.
fn applicable(job: &Value, target: &str) -> Result<(), Value> {
    let handle = job["handle"].as_str().unwrap_or(target);
    let (what, next) = match job["state"].as_str() {
        Some("paused" | "awaiting_user" | "cancelled" | "failed" | "timed_out") => return Ok(()),
        Some("completed") => {
            // A handle is the script name, or the name and `-<n>`.
            let name = (handle.rsplit_once('-'))
                .filter(|(_, n)| n.parse::<u64>().is_ok())
                .map_or(handle, |(name, _)| name);
            (
                format!("{handle} already completed"),
                format!("workflow run {name}"),
            )
        }
        Some("running") => (format!("{handle} is still running"), "end your turn".into()),
        other => (
            format!(
                "{handle} is in state {} and cannot be resumed",
                other.unwrap_or("unknown")
            ),
            format!("workflow status {target}"),
        ),
    };
    Err(fail(
        WORKFLOW_CONTROL_NOT_APPLICABLE,
        &format!("{what}. Next: {next}"),
    ))
}

/// The first round: the prior row's run note and, when it paused for budget, the budget the
/// blocked step needs; each the lowest `_docID` among P2P twins.
fn ask_run(prior_row: &str) -> Value {
    let note = |id: &str, note_id: String| DataCall {
        id: id.into(),
        query: DataQuery {
            collection: "WfNote".into(),
            filter: object(json!({"_and": [
                {"note_id": {"_eq": note_id}},
                {"node_did": {"_eq": "$caller.node_did"}},
            ]})),
            order: Some(object(json!({"_docID": "ASC"}))),
            limit: 1,
            offset: None,
            fields: ["_docID", "note_id", "value_json", "chain_root_doc_id"]
                .map(String::from)
                .into(),
        },
    };
    let requests = [
        note("run", run_id(prior_row)),
        note("need", need_id(prior_row)),
    ];
    json!({"data_calls": {"state": {"rounds": 1}, "requests": requests}})
}

/// What the run-note round answered: `{note, chain, need}`, or the step's failure.
fn context(input: &Value, target: &str) -> Result<Value, Value> {
    let needed = rounds::answer::<DataQueryResponse>(&input["data_results"], "need")?;
    let need = (needed.as_ref().and_then(|a| a.rows.first()))
        .and_then(|r| r.get("value_json")?.as_str()?.parse::<u64>().ok());
    let answered = rounds::answer::<DataQueryResponse>(&input["data_results"], "run")?;
    let row = answered.as_ref().and_then(|a| a.rows.first());
    let field = |name: &str| row.and_then(|r| r.get(name)).and_then(Value::as_str);
    let note = field("value_json").and_then(|s| serde_json::from_str::<Value>(s).ok());
    match (note, field("chain_root_doc_id")) {
        (Some(note), Some(chain)) => Ok(json!({"note": note, "chain": chain, "need": need})),
        _ => {
            let what = format!("{target} has no run note here. Next: workflow status {target}");
            Err(fail(WORKFLOW_RESUME_UNKNOWN_RUN, &what))
        }
    }
}

/// The `.rhai` file under `dir` whose text hashes to `digest`. Symlinks are not followed.
fn find(dir: &Path, digest: &str, depth: usize) -> Option<String> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let (kind, path) = (entry.file_type().ok()?, entry.path());
        if kind.is_dir() && depth > 0 {
            if let Some(text) = find(&path, digest, depth - 1) {
                return Some(text);
            }
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "rhai") {
            if let Ok(text) = fs::read_to_string(&path) {
                if sha256_digest(&[&text]) == digest {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// The source a mount holds under `digest`; the WfScript row is the fallback.
fn mounted(mounts: &Value, digest: &str) -> Option<String> {
    let paths = (mounts.as_array().into_iter().flatten()).filter_map(|m| m["path"].as_str());
    paths
        .into_iter()
        .find_map(|p| find(Path::new(p), digest, DEPTH))
}

/// The prior pin's guest fields; the budget only rises.
fn pin_of(note: &RunNote, budget: Option<u64>) -> JobPin {
    JobPin {
        script_id: note.script_id.clone(),
        source_digest: note.source_digest.clone(),
        agent_budget: note.agent_budget.max(budget.unwrap_or(0)),
        handle: note.handle.clone(),
        host: serde_json::Map::new(),
    }
}

/// Step 0 of a resume. The rounds are the run note (this module), then the chain's notes, the
/// source and the child text (`rounds::advance`, fed the pin and chain root the note gave).
pub fn step0(input: &Value) -> Value {
    if let Some(msg) = origin::nested(input) {
        return refused(WORKFLOW_DEPTH_EXCEEDED, msg);
    }
    let req = match parse(input) {
        Ok(req) => req,
        Err(out) => return out,
    };
    let target = req.target.as_str();
    let job = match prior(input, target).and_then(|j| applicable(j, target).map(|()| j)) {
        Ok(job) => job,
        Err(out) => return out,
    };
    let prior_row = job["row_doc_id"].as_str().unwrap_or_default();
    let mut state = input["state"].clone();
    let echoed = state.as_object_mut().and_then(|s| s.remove("resume"));
    let ctx = match (echoed, input["data_results"].get("run")) {
        (Some(ctx), _) => ctx,
        (None, None) => return ask_run(prior_row),
        (None, Some(_)) => match context(input, target) {
            Ok(ctx) => ctx,
            Err(out) => return out,
        },
    };
    let Ok(note) = serde_json::from_value::<RunNote>(ctx["note"].clone()) else {
        let what =
            format!("the run note of {target} is unreadable. Next: workflow status {target}");
        return fail(WORKFLOW_RESUME_UNKNOWN_RUN, &what);
    };
    // A row that paused for budget resumes only with a budget its blocked step fits in.
    if let Some(need) = ctx["need"].as_u64() {
        let budget = pin_of(&note, req.budget).agent_budget;
        if budget < need {
            let what = format!(
                "budget {budget} is below the {need} the paused step needs. Next: workflow resume {target} --budget {need}"
            );
            return fail(WORKFLOW_INVALID_INPUT, &what);
        }
    }
    let held = mounted(&input["mounts"], &note.source_digest);
    let mut inner = input.clone();
    inner["pin"] = json!(pin_of(&note, req.budget));
    inner["job"]["chain_root_doc_id"] = ctx["chain"].clone();
    inner["state"] = state;
    if let Some(source) = &held {
        inner["verified_script"] = json!({"source": source, "digest": note.source_digest});
    }
    match rounds::advance(&inner) {
        Err(out) => out,
        Ok(Step::Ask(mut out)) => {
            for service in ["data_calls", "job_calls"] {
                if let Some(round) = out.get_mut(service) {
                    round["state"]["resume"] = ctx.clone();
                }
            }
            out
        }
        Ok(Step::Replay(replay)) => replay_prior(input, prior_row, &note, &req, replay, held),
    }
}

/// The replay of a resume's step 0: the pin, the run and `reattempt:` notes, then the script.
fn replay_prior(
    input: &Value,
    prior_row: &str,
    note: &RunNote,
    req: &Request,
    replay: Replay,
    held: Option<String>,
) -> Value {
    let Some(row) = input.pointer("/job/row_doc_id").and_then(Value::as_str) else {
        let what = "the input has no job.row_doc_id. Next: workflow help";
        return fail(WORKFLOW_INVALID_INPUT, what);
    };
    let Some(source) = replay.source.clone().or(held) else {
        let what = "the script source is not stored here. Next: workflow list";
        return fail(WORKFLOW_RESOLVE_FAILED, what);
    };
    let meta = match validate::validate(&source) {
        Ok(meta) => meta,
        Err(msg) => return refused(WORKFLOW_VALIDATION_FAILED, msg),
    };
    let pin = pin_of(note, req.budget);
    // Interrupted children are terminal; one still live is waited for, as on any step.
    let listed: Vec<Value> = (input["children"].as_object().into_iter().flatten())
        .filter(|(_, c)| c["state"] == "interrupted")
        .map(|(slot, c)| json!([slot, c["attempt"]]))
        .collect();
    let mut run = json!({"script_id": pin.script_id, "source_digest": pin.source_digest,
                         "args": note.args, "handle": pin.handle,
                         "agent_budget": pin.agent_budget});
    if let Some(path) = &note.path {
        run["path"] = json!(path);
    }
    let mut journal = Journal::new(replay);
    journal.put(&run_id(row), run);
    journal.put(&reattempt_id(row), json!(listed));
    let mut resume = json!({"row_doc_id": prior_row});
    if let Some(budget) = req.budget {
        resume["budget"] = json!(budget);
    }
    if req.engine_current {
        resume["engine"] = json!("current");
    }
    let mut input = input.clone();
    if let Some(answer) = &req.answer {
        resume["answer"] = json!(answer);
        // The host echoes the block as `control` from step 1 on; step 0 reads it the same way.
        input["control"] = json!({"kind": "answer", "payload": resume});
    }
    let adopting = Adopting {
        script: None,
        last_event: format!("resumed {prior_row} ({})", pin.source_digest),
        resume: Some(resume),
    };
    input["args"] = note.args.clone();
    run::replay(&input, &source, &meta, journal, &pin, Some(adopting))
}

/// The answer this row carries: `control.payload.answer` when `control.kind` is `answer` (the
/// host's precedence puts it first whenever the block has one).
pub fn answer_of(input: &Value) -> Option<String> {
    let control = input.get("control").filter(|c| c["kind"] == "answer")?;
    control
        .pointer("/payload/answer")?
        .as_str()
        .map(String::from)
}

/// C5.3: the slots this row's `reattempt:` note lists, whose latest attempt is still the listed
/// one and interrupted, get one more attempt. They are shown as the live next attempt, so the
/// replay emits its spawn at `attempt + 1` and waits for it; once that attempt exists the slot
/// is read as any other.
pub fn retry(children: &mut BTreeMap<String, Child>, journal: &Journal, input: &Value) {
    let row = input.pointer("/job/row_doc_id").and_then(Value::as_str);
    let listed = row.and_then(|row| journal.get(&reattempt_id(row)));
    let listed = listed.and_then(Value::as_array).into_iter().flatten();
    for (slot, attempt) in listed.filter_map(|e| Some((e[0].as_str()?, e[1].as_u64()?))) {
        let due = children.get_mut(slot);
        if let Some(c) = due.filter(|c| c.attempt == attempt && c.state == "interrupted") {
            c.attempt += 1;
            c.state = "live".into();
        }
    }
}

/// The input a later step of a resumed row replays: `root_call`'s arguments, which must be the
/// ones this row's run note holds. A row that is not a resume has no `root_call`, and its input
/// is the step's own; a resumed row (its step 0 wrote a `reattempt:` note) must carry one.
pub fn rooted<'a>(
    input: &'a Value,
    journal: &Journal,
    name: &str,
) -> Result<Cow<'a, Value>, Value> {
    let row = input.pointer("/job/row_doc_id").and_then(Value::as_str);
    let root = input.get("root_call").filter(|r| !r.is_null());
    let resumed = row.is_some_and(|row| journal.get(&reattempt_id(row)).is_some());
    if root.is_none() && !resumed {
        return Ok(Cow::Borrowed(input));
    }
    let noted = row.and_then(|row| journal.get(&run_id(row)));
    let root = match root.zip(noted) {
        Some((r, n)) if n["args"] == r["args"] && n["path"] == r["path"] => r,
        _ => {
            let what = "resume arguments differ from the chain root";
            return Err(fail(
                WORKFLOW_VALIDATION_FAILED,
                &format!("{what}. Next: workflow run {name}"),
            ));
        }
    };
    let mut rooted = input.clone();
    rooted["args"] = root["args"].clone();
    Ok(Cow::Owned(rooted))
}
