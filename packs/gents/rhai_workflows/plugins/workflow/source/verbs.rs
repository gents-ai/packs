//! The verbs of the raw-argument shape (L2 sections 4.3 and 4.14): `help`, `check` and `list`
//! for a tool with no job block. Job verbs answer `workflow_not_available`. A raw output never
//! carries a WfNote write.
use crate::errors::{
    WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE, WORKFLOW_RESOLVE_FAILED,
    WORKFLOW_VALIDATION_FAILED, message,
};
use crate::fail;
use crate::run::{HOST_API_VERSION, refused, sha256_hex};
use crate::smoke::{self, End};
use crate::validate;
use crate::wire::{Mount, RawInput};
use serde_json::{Value, json};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The verbs that need a job block.
const JOB_VERBS: [&str; 7] = [
    "run", "resume", "answer", "stop", "pause", "status", "result",
];

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
        "run (<name> | script | path) [args]: start a workflow in the background",
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

/// A workflow the pack tier lists.
struct Listed {
    pack: String,
    name: String,
    when_to_use: String,
    file: PathBuf,
}

/// Serves one raw-argument call.
pub fn raw(input: &Value) -> Value {
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
        "list" => list(rest, &raw),
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

/// An index `file` must stay in its mount: non-empty, with only plain parts, none starting with a
/// dot (L2 section 4.14: asset paths have no dot parts).
fn inside_mount(file: &str) -> bool {
    !file.is_empty()
        && (Path::new(file).components())
            .all(|c| matches!(c, Component::Normal(p) if !p.to_string_lossy().starts_with('.')))
}

/// The pack tier: each `workflows` mount's `index.json`, then each listed file's meta. The file
/// supplies the name, the index the listing fields.
fn pack_tier(mounts: &[Mount]) -> Vec<Listed> {
    let mut tier = Vec::new();
    for mount in mounts.iter().filter(|m| m.class == "workflows") {
        let dir = Path::new(&mount.path);
        let index = fs::read_to_string(dir.join("index.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        for item in index.iter().flat_map(|i| i.as_array()).flatten() {
            let Some(file) = item["file"].as_str().filter(|f| inside_mount(f)) else {
                continue;
            };
            let file = dir.join(file);
            let meta = fs::read_to_string(&file)
                .ok()
                .and_then(|source| validate::validate(&source).ok());
            let name =
                (meta.as_ref().map(|m| m.name.clone())).or(item["name"].as_str().map(String::from));
            let when_to_use = (item["when_to_use"].as_str().map(String::from))
                .or(meta.and_then(|m| m.when_to_use))
                .unwrap_or_default();
            if let Some(name) = name {
                let pack = mount.pack.clone();
                tier.push(Listed {
                    pack,
                    name,
                    when_to_use,
                    file,
                });
            }
        }
    }
    tier.sort_by(|a, b| (&a.pack, &a.name).cmp(&(&b.pack, &b.name)));
    tier
}

fn list(rest: &[String], raw: &RawInput) -> Value {
    if !rest.is_empty() {
        return fail(
            WORKFLOW_NOT_AVAILABLE,
            "list takes no options here yet. Next: workflow list",
        );
    }
    let tier = pack_tier(&raw.asset_mounts);
    let text = if tier.is_empty() {
        "no workflows are installed in the pack tier".to_string()
    } else {
        (tier.iter())
            .map(|e| format!("{}: {} [{}]", e.name, e.when_to_use, e.pack))
            .collect::<Vec<_>>()
            .join("\n")
    };
    json!({"result": text})
}

/// The source of the pack-tier workflow `name`.
fn pack_source(name: &str, mounts: &[Mount]) -> Result<String, Value> {
    let tier = pack_tier(mounts);
    let mut hits = tier.iter().filter(|e| e.name == name);
    let (Some(hit), second) = (hits.next(), hits.next()) else {
        let what = format!("no workflow named '{name}' in the pack tier");
        return Err(fail(
            WORKFLOW_RESOLVE_FAILED,
            &format!("{what}. Next: workflow list"),
        ));
    };
    if let Some(other) = second {
        let what = format!("'{name}' is in {} and {}", hit.pack, other.pack);
        return Err(fail(
            WORKFLOW_RESOLVE_FAILED,
            &format!("{what}. Next: workflow list"),
        ));
    }
    fs::read_to_string(&hit.file).map_err(|e| {
        let what = format!("cannot read {} ({e})", hit.file.display());
        fail(
            WORKFLOW_RESOLVE_FAILED,
            &format!("{what}. Next: workflow list"),
        )
    })
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
        (Some(name), ..) if !name.starts_with("--") => match pack_source(name, &raw.asset_mounts) {
            Ok(source) => (source, "pack"),
            Err(out) => return out,
        },
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
    let write = json!({"op": "insert", "collection": "WfScript", "on_conflict": "ignore",
        "fields": {"script_id": script_id, "source": source, "origin_hint": origin_hint,
                   "meta_json": json!({"name": meta.name, "description": meta.description}).to_string()}});
    json!({"result": receipt, "writes": [write]})
}
