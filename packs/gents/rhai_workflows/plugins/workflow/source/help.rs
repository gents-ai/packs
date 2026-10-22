//! `help`, `help <verb>` and `help <topic>` (L2 section 4.17): the verb index and one line per
//! verb from `verbs::VERBS`, and the topics embedded from `help/*.md`.
use crate::data::{self, PAGE};
use crate::errors::WORKFLOW_INVALID_INPUT;
use crate::fail;
use crate::rounds;
use crate::run::HOST_API_VERSION;
use crate::tiers::inside_mount;
use crate::validate;
use crate::verbs::{JOB_VERBS, VERBS};
use crate::wire::{DataCall, DataQueryResponse, Mount, RawInput};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The rhai version of the `Cargo.toml` pin; a test holds the two together.
const RHAI_VERSION: &str = "1.26.1";

/// Each topic with its embedded text, at most 3 KiB each.
pub const TOPICS: [(&str, &str); 7] = [
    ("language", include_str!("../help/language.md")),
    ("host", include_str!("../help/host.md")),
    ("agent", include_str!("../help/agent.md")),
    ("lifecycle", include_str!("../help/lifecycle.md")),
    ("errors", include_str!("../help/errors.md")),
    ("grok", include_str!("../help/grok.md")),
    ("doctor", include_str!("../help/doctor.md")),
];

/// The index: every verb's line, then the topics.
fn index() -> String {
    let lines = VERBS.iter().map(|(_, line)| *line).collect::<Vec<_>>();
    let topics = TOPICS.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    format!(
        "{}\ntopics (help <topic>): {}",
        lines.join("\n"),
        topics.join(", ")
    )
}

/// `agent` ends with the targets this call may use, from `ceiling.targets`; a call without a
/// ceiling cannot see them.
fn agent(body: &str, input: &Value) -> String {
    let here = match input.pointer("/ceiling/targets").and_then(Value::as_array) {
        Some(targets) => targets
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        None => "not visible in this call; a run uses its job ceiling".into(),
    };
    format!("{body}\nTargets here: {here}")
}

/// Each `workflows` mount's index entries whose file does not read as a workflow, or whose
/// `meta.name` is not the name the index gives it.
fn mismatches(mounts: &[Mount]) -> Vec<String> {
    let mut found = Vec::new();
    for mount in mounts.iter().filter(|m| m.class == "workflows") {
        let dir = Path::new(&mount.path);
        let index = fs::read_to_string(dir.join("index.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let Some(items) = index.as_ref().and_then(Value::as_array) else {
            found.push(format!("{}: index.json does not read", mount.pack));
            continue;
        };
        for item in items {
            let Some(file) = item["file"].as_str().filter(|f| inside_mount(f)) else {
                found.push(format!("{}: an index entry has no usable file", mount.pack));
                continue;
            };
            let meta = fs::read_to_string(dir.join(file))
                .ok()
                .and_then(|source| validate::validate(&source).ok());
            match (meta, item["name"].as_str()) {
                (None, _) => found.push(format!("{}/{file}: not a valid workflow", mount.pack)),
                (Some(meta), Some(name)) if meta.name != name => {
                    let said = format!("index says {name}, file says {}", meta.name);
                    found.push(format!("{}/{file}: {said}", mount.pack));
                }
                _ => {}
            }
        }
    }
    found
}

/// One page of the caller's own `script_id`s from `WfScript` (`scripts`) or `WfName` (`names`);
/// `_docID` is a total order, so pages never skip or repeat a P2P twin.
fn draft_page(phase: &str, offset: usize) -> DataCall {
    let (id, collection) = match phase {
        "scripts" => ("draft_scripts", "WfScript"),
        _ => ("draft_names", "WfName"),
    };
    let call = data::query(id, collection, data::own_node(), &["script_id"]);
    data::paged(data::ordered(call, json!({"_docID": "ASC"})), offset)
}

fn ask(call: DataCall, state: Value) -> Value {
    json!({"data_calls": {"requests": [call], "state": {"drafts": state}}})
}

/// The drafts: the caller's `WfScript` rows that no `WfName` row names. Two paged reads, scripts
/// then names, each round echoing the ids read so far; a call without `data_calls` cannot see
/// them. `Err` is the next round's output.
fn drafts(input: &Value) -> Result<String, Value> {
    if input["data_calls"] != true {
        return Ok("not visible in this call".into());
    }
    let state = &input["state"]["drafts"];
    let phase = state["phase"].as_str().unwrap_or("scripts");
    let offset = state["offset"].as_u64().unwrap_or(0) as usize;
    let ids = |key: &str| -> Vec<String> {
        let held = state.get(key).cloned().unwrap_or_default();
        serde_json::from_value(held).unwrap_or_default()
    };
    let (mut scripts, mut names) = (ids("scripts"), ids("names"));
    let carry = |phase: &str, offset: usize, scripts: &[String], names: &[String]| json!({"phase": phase, "offset": offset, "scripts": scripts, "names": names});
    let id = if phase == "scripts" {
        "draft_scripts"
    } else {
        "draft_names"
    };
    let Some(page) = rounds::answer::<DataQueryResponse>(&input["data_results"], id)? else {
        let state = carry(phase, offset, &scripts, &names);
        return Err(ask(draft_page(phase, offset), state));
    };
    let rows = page.rows.len();
    let held = if phase == "scripts" {
        &mut scripts
    } else {
        &mut names
    };
    held.extend(
        page.rows
            .iter()
            .filter_map(|r| r["script_id"].as_str().map(String::from)),
    );
    if rows == PAGE as usize {
        let state = carry(phase, offset + rows, &scripts, &names);
        return Err(ask(draft_page(phase, offset + rows), state));
    }
    if phase == "scripts" {
        let state = carry("names", 0, &scripts, &names);
        return Err(ask(draft_page("names", 0), state));
    }
    let named: BTreeSet<&String> = names.iter().collect();
    let count = scripts
        .iter()
        .collect::<BTreeSet<_>>()
        .difference(&named)
        .count();
    Ok(count.to_string())
}

/// `doctor` ends with what this call lets the engine see: the job block or its absence, the
/// mounts, the draft count, the versions, the index/meta mismatches and the `wf-read-only-web`
/// warning.
fn doctor(body: &str, raw: &RawInput, input: &Value) -> Result<String, Value> {
    let job = match input.pointer("/ceiling/targets") {
        Some(targets) => format!("job block: present; targets {targets}"),
        None => format!(
            "job block: no job block in this call, so {} are unavailable",
            JOB_VERBS.join(", ")
        ),
    };
    let mounts = raw.asset_mounts.iter();
    let workflows = mounts.clone().filter(|m| m.class == "workflows").count();
    let project = mounts.clone().any(|m| m.class == "project");
    let state = if project { "on" } else { "off" };
    let drafts = drafts(input)?;
    let found = mismatches(&raw.asset_mounts);
    let index = match found.is_empty() {
        true => "none".into(),
        false => found.join("; "),
    };
    let web = "wf-read-only-web warning: it comes with wf-research from the separate gents/rhai_workflows_web pack when it is installed, and it pairs read-only files and shell with gents/web_search and gents/web_fetch, so what it reads can reach the web; the operator opts in by wiring it as a target";
    Ok(format!(
        "{body}\n{job}\nworkflow mounts: {workflows}; project tier: {state}\ndrafts: {drafts}\nrhai {RHAI_VERSION}, host_api_version {HOST_API_VERSION}\nindex/meta mismatches: {index}\n{web}"
    ))
}

pub fn serve(rest: &[String], raw: &RawInput, input: &Value) -> Value {
    let text = match rest.first().map(String::as_str) {
        None => index(),
        Some(name) => match TOPICS.iter().find(|(topic, _)| *topic == name) {
            Some(("agent", body)) => agent(body, input),
            Some(("doctor", body)) => match doctor(body, raw, input) {
                Ok(text) => text,
                Err(round) => return round,
            },
            Some((_, body)) => (*body).into(),
            None => match VERBS.iter().find(|(verb, _)| *verb == name) {
                Some((_, line)) => (*line).into(),
                None => {
                    let what = format!("no help for {name:?}");
                    return fail(
                        WORKFLOW_INVALID_INPUT,
                        &format!("{what}. Next: workflow help"),
                    );
                }
            },
        },
    };
    json!({"result": text})
}
