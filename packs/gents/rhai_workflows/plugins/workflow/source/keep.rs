//! `keep` and `retire` (L2 section 4.14, naming integrity; root C3): they write WfName rows of
//! the caller's own node, read through `data_calls`, and answer as non-job outputs carrying
//! writes. A name's version is the newest own-node version plus one; the unique index of
//! `(node_did, name, version)` rejects a concurrent writer of the same version.
use crate::address;
use crate::data::{self, WfName, WfScript};
use crate::errors::{
    WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE, WORKFLOW_RESOLVE_FAILED,
    WORKFLOW_VALIDATION_FAILED, message,
};
use crate::fail;
use crate::journal::run_id;
use crate::rounds;
use crate::run::{HOST_API_VERSION, refused, sha256_hex};
use crate::tiers::pack_tier;
use crate::validate;
use crate::wire::{DataCall, DataQueryResponse, Mount};
use serde_json::{Map, Value, json};

/// What `keep` was asked for.
struct Args<'a> {
    target: &'a str,
    name: Option<&'a str>,
    description: Option<&'a str>,
    when_to_use: Option<&'a str>,
    replace: bool,
}

/// The script `keep` stores a name for, after any rename.
struct Plan {
    name: String,
    source: String,
    script_id: String,
    /// The meta's description, which `--description` does not change.
    about: String,
    description: String,
    when_to_use: String,
    renamed: bool,
}

pub fn is_keep(input: &Value) -> bool {
    matches!(input["argv"][0].as_str(), Some("keep" | "retire"))
}

fn invalid(what: &str, next: &str) -> Value {
    fail(WORKFLOW_INVALID_INPUT, &format!("{what}. Next: {next}"))
}

/// One output asking the host for one read; the next input echoes `carry` under `keep`.
fn ask(call: DataCall, carry: Value) -> Value {
    json!({"data_calls": {"requests": [call], "state": {"keep": carry}}})
}

/// The newest own-node WfName row of `name`.
fn newest(id: &str, name: &str, fields: &[&str]) -> DataCall {
    let filter = json!({"_and": [{"name": {"_eq": name}}, data::own_node()]});
    let call = data::ordered(
        data::query(id, "WfName", filter, fields),
        json!({"version": "DESC"}),
    );
    data::limited(call, 1)
}

fn first_row(page: DataQueryResponse) -> Option<Map<String, Value>> {
    page.rows.into_iter().next()
}

/// `keep` and `retire`, in the raw-argument or the job-block shape.
pub fn serve(input: &Value) -> Value {
    if input["data_calls"] != true {
        let what = "the host did not offer data_calls, so kept names cannot be read or written";
        return fail(
            WORKFLOW_NOT_AVAILABLE,
            &format!("{what}. Next: workflow help doctor"),
        );
    }
    let argv: Vec<&str> = (input["argv"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let out = match argv.as_slice() {
        ["keep", rest @ ..] => keep(input, rest),
        [_, rest @ ..] => retire(input, rest),
        [] => unreachable!("is_keep saw a verb"),
    };
    out.unwrap_or_else(|out| out)
}

/// `retire <name>`: a `retired` version over the newest kept one; runs already pinned keep their
/// source.
fn retire(input: &Value, rest: &[&str]) -> Result<Value, Value> {
    let [name] = rest else {
        return Err(invalid("retire takes <name>", "workflow help retire"));
    };
    let fields = [
        "version",
        "state",
        "script_id",
        "description",
        "when_to_use",
    ];
    let Some(page) = rounds::answer::<DataQueryResponse>(&input["data_results"], "name")? else {
        return Err(ask(newest("name", name, &fields), json!({})));
    };
    let row = first_row(page).filter(|r| r.get("state") == Some(&json!("kept")));
    let Some(row) = row else {
        let what = format!("no kept workflow '{name}'");
        return Err(fail(
            WORKFLOW_RESOLVE_FAILED,
            &format!("{what}. Next: workflow list"),
        ));
    };
    let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or_default();
    let version = row.get("version").and_then(Value::as_u64).unwrap_or(0) + 1;
    let retired = WfName {
        name: name.to_string(),
        version,
        script_id: text("script_id").into(),
        description: text("description").into(),
        when_to_use: text("when_to_use").into(),
        state: "retired".into(),
    };
    let result = format!("Retired {name} (v{version}). Runs already started keep their script.");
    Ok(json!({"result": result, "writes": [retired.write()]}))
}

fn parse<'a>(rest: &[&'a str]) -> Option<Args<'a>> {
    let (target, mut flags) = rest.split_first()?;
    let mut args = Args {
        target,
        name: None,
        description: None,
        when_to_use: None,
        replace: false,
    };
    while let Some((flag, tail)) = flags.split_first() {
        flags = tail;
        if *flag == "--replace" {
            args.replace = true;
            continue;
        }
        let (value, tail) = flags.split_first()?;
        flags = tail;
        match *flag {
            "--name" => args.name = Some(value),
            "--description" => args.description = Some(value),
            "--when-to-use" => args.when_to_use = Some(value),
            _ => return None,
        }
    }
    Some(args)
}

/// `keep <key|handle|script_id|sha256:..> [--name n] [--description d] [--when-to-use w]
/// [--replace]`. The rounds: the run note (a run's address), the script's source, the name's
/// newest version, and the same read again just before the write, which is where a concurrent
/// keep shows. The carried state holds what the rounds found.
fn keep(input: &Value, rest: &[&str]) -> Result<Value, Value> {
    let Some(args) = parse(rest) else {
        let what = "keep takes <key|handle|script_id> [--name n] [--description d] \
                    [--when-to-use w] [--replace]";
        return Err(invalid(what, "workflow help keep"));
    };
    let results = &input["data_results"];
    let mut carry = input["state"]["keep"].clone();
    if !carry.is_object() {
        carry = json!({});
    }
    if carry["source"].is_null() {
        if carry["script_id"].is_null() {
            carry["script_id"] = json!(script_id_of(input, &args)?);
        }
        let id = carry["script_id"].as_str().unwrap_or_default();
        let Some(page) = rounds::answer::<DataQueryResponse>(results, "source")? else {
            return Err(ask(data::script_source(id), carry));
        };
        let source = first_row(page).and_then(|r| r.get("source")?.as_str().map(String::from));
        let Some(source) = source else {
            let what = format!("the script {id} is not stored here");
            return Err(fail(
                WORKFLOW_RESOLVE_FAILED,
                &format!("{what}. Next: workflow list"),
            ));
        };
        carry["source"] = json!(source);
    }
    let plan = plan(input, &args, carry["source"].as_str().unwrap_or_default())?;
    held_by_pack(input, &args, &plan.name)?;

    let fields = ["version", "state"];
    let version_of = |page: DataQueryResponse| {
        let row = first_row(page);
        let field = |key: &str| row.as_ref().and_then(|r| r.get(key)).cloned();
        (
            field("version").and_then(|v| v.as_u64()).unwrap_or(0),
            field("state"),
        )
    };
    if let Some(page) = rounds::answer::<DataQueryResponse>(results, "confirm")? {
        let (now, _) = version_of(page);
        let seen = carry["seen"].as_u64().unwrap_or(0);
        if now != seen {
            let what = format!("'{}' v{now} was kept concurrently", plan.name);
            return Err(invalid(
                &what,
                &format!("workflow keep {} --replace", args.target),
            ));
        }
        return Ok(write(&plan, now + 1));
    }
    let Some(page) = rounds::answer::<DataQueryResponse>(results, "name")? else {
        return Err(ask(newest("name", &plan.name, &fields), carry));
    };
    let (seen, state) = version_of(page);
    if state == Some(json!("kept")) && !args.replace {
        let what = format!("'{}' is kept (v{seen})", plan.name);
        return Err(invalid(
            &what,
            &format!("workflow keep {} --replace", args.target),
        ));
    }
    carry["seen"] = json!(seen);
    Err(ask(newest("confirm", &plan.name, &fields), carry))
}

/// The id of the script `args.target` names: the script of a run's note, or a stored id.
fn script_id_of(input: &Value, args: &Args) -> Result<String, Value> {
    let target = args.target;
    if let Ok(job) = address::resolve(input, target) {
        let row = job["row_doc_id"].as_str().unwrap_or_default();
        let Some(page) = rounds::answer::<DataQueryResponse>(&input["data_results"], "run")? else {
            return Err(ask(data::own_note("run", &run_id(row)), json!({})));
        };
        let note = first_row(page)
            .and_then(|r| serde_json::from_str::<Value>(r.get("value_json")?.as_str()?).ok());
        return match note.as_ref().and_then(|n| n["script_id"].as_str()) {
            Some(id) => Ok(id.into()),
            None => {
                let what = format!("the run note of {target} is unreadable");
                let next = format!("workflow status {target}");
                Err(fail(
                    WORKFLOW_RESOLVE_FAILED,
                    &format!("{what}. Next: {next}"),
                ))
            }
        };
    }
    let id = target.strip_prefix("sha256:").unwrap_or(target);
    match id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        true => Ok(id.into()),
        false => {
            let what = format!("'{target}' is not a run of this session or a script_id");
            Err(invalid(&what, "workflow status"))
        }
    }
}

/// The name, script and fields `keep` writes: `--name` rewrites `meta.name`, so a rename is a
/// new script. A numbered handle (`x-2`) is not a name.
fn plan(input: &Value, args: &Args, source: &str) -> Result<Plan, Value> {
    let meta =
        validate::validate(source).map_err(|msg| refused(WORKFLOW_VALIDATION_FAILED, msg))?;
    let handle = address::resolve(input, args.target).ok();
    let handle = handle.and_then(|job| job["handle"].as_str());
    if let (None, Some(handle)) = (args.name, handle.filter(|h| *h != meta.name)) {
        let what = format!("'{handle}' is a numbered handle");
        return Err(invalid(
            &what,
            &format!("workflow keep {handle} --name <name>"),
        ));
    }
    let (source, meta, renamed) = match args.name.filter(|n| *n != meta.name) {
        None => (source.to_string(), meta, false),
        Some(new) => {
            let wanted = rename(source, &meta.name, new);
            let source = wanted.ok_or_else(|| {
                let what = "meta.name is not a plain string literal, so it cannot be renamed";
                let next = "edit the script, then run workflow check";
                refused(
                    WORKFLOW_VALIDATION_FAILED,
                    message(WORKFLOW_VALIDATION_FAILED, what, next),
                )
            })?;
            let meta =
                validate::validate(&source).map_err(|m| refused(WORKFLOW_VALIDATION_FAILED, m))?;
            if meta.name != new {
                let what = format!("{new:?} is not a name this script can take");
                let next = "workflow keep <h> --name <lowercase-words>";
                return Err(refused(
                    WORKFLOW_VALIDATION_FAILED,
                    message(WORKFLOW_VALIDATION_FAILED, &what, next),
                ));
            }
            (source, meta, true)
        }
    };
    Ok(Plan {
        name: meta.name.clone(),
        script_id: sha256_hex(&[HOST_API_VERSION, &source]),
        about: meta.description.clone(),
        description: args.description.map_or(meta.description, String::from),
        when_to_use: (args.when_to_use.map(String::from))
            .or(meta.when_to_use)
            .unwrap_or_default(),
        source,
        renamed,
    })
}

/// `source` with the meta's `name` literal `old` replaced by `new`.
fn rename(source: &str, old: &str, new: &str) -> Option<String> {
    let quoted = format!("\"{old}\"");
    let mut from = source.find("let meta = #{")?;
    while let Some(at) = source[from..].find(&quoted).map(|i| from + i) {
        let key = source[..at].trim_end().strip_suffix(':').map(str::trim_end);
        let key = key.and_then(|k| k.strip_suffix("name"));
        if key.is_some_and(|k| !k.ends_with(|c: char| c.is_alphanumeric() || c == '_')) {
            return Some(format!(
                "{}\"{new}\"{}",
                &source[..at],
                &source[at + quoted.len()..]
            ));
        }
        from = at + quoted.len();
    }
    None
}

/// A name the pack tier holds is never kept.
fn held_by_pack(input: &Value, args: &Args, name: &str) -> Result<(), Value> {
    // The job-block shape names the mounts `mounts`; the raw shape `asset_mounts`.
    let mounts = if input["mounts"].is_array() {
        &input["mounts"]
    } else {
        &input["asset_mounts"]
    };
    let mounts: Vec<Mount> = serde_json::from_value(mounts.clone()).unwrap_or_default();
    let Some(hit) = pack_tier(&mounts).into_iter().find(|e| e.name == name) else {
        return Ok(());
    };
    let what = format!("'{name}' is held by the pack tier ({})", hit.pack);
    Err(invalid(
        &what,
        &format!("workflow keep {} --name <other>", args.target),
    ))
}

/// The output that stores `plan` as `version`: a renamed script is a new WfScript row.
fn write(plan: &Plan, version: u64) -> Value {
    let mut writes = Vec::new();
    if plan.renamed {
        let (id, source, name) = (&plan.script_id, &plan.source, &plan.name);
        let draft = WfScript::draft(id, source, "inline", name, &plan.about);
        writes.push(draft.write());
    }
    let kept = WfName {
        name: plan.name.clone(),
        version,
        script_id: plan.script_id.clone(),
        description: plan.description.clone(),
        when_to_use: plan.when_to_use.clone(),
        state: "kept".into(),
    };
    writes.push(kept.write());
    let name = &plan.name;
    let result = format!(
        "Kept {name} (v{version}, script_id {}). Run it with: workflow run {name}",
        plan.script_id
    );
    json!({"result": result, "writes": writes})
}
