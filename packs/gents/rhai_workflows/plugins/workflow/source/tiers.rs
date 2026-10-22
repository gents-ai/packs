//! Where a run's `.rhai` comes from (L2 section 4.13; root C4): the pack, project, kept, inline
//! and path tiers. A name resolves pack, then project, then kept; the qualified forms pick one.
use crate::data;
use crate::errors::{WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE, WORKFLOW_RESOLVE_FAILED};
use crate::fail;
use crate::rounds;
use crate::validate;
use crate::wire::{DataCall, DataQueryResponse, Mount};
use serde_json::{Value, json};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The tier a source came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier {
    /// A `workflows` mount of this pack.
    Pack(String),
    /// The project mount (`pack: ""`).
    Project,
    /// A kept name of the caller's own node.
    Kept(String),
    /// A stored script of the caller's own node, named by `sha256:<script_id>`.
    Stored,
    Inline,
    Path,
}

impl Tier {
    /// `WfScript.origin_hint`.
    pub fn hint(&self) -> &'static str {
        match self {
            Tier::Pack(_) => "pack",
            Tier::Project => "project",
            // The script is already stored, so the insert that carries this hint is ignored.
            Tier::Kept(_) | Tier::Stored | Tier::Inline => "inline",
            Tier::Path => "path",
        }
    }

    /// The tier phrase of the `started from` line.
    pub fn label(&self) -> String {
        match self {
            Tier::Pack(pack) => format!("pack {pack}"),
            Tier::Project => "project".into(),
            Tier::Kept(name) => format!("kept {name}"),
            Tier::Stored => "stored script".into(),
            Tier::Inline => "inline script".into(),
            Tier::Path => "path".into(),
        }
    }
}

/// A source and the tier it came from.
pub struct Resolved {
    pub source: String,
    pub tier: Tier,
}

/// A workflow a tier lists.
pub(crate) struct Listed {
    pub pack: String,
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub file: PathBuf,
}

/// An index `file` must stay in its mount: non-empty, with only plain parts, none starting with a
/// dot (L2 section 4.14: asset paths have no dot parts).
pub(crate) fn inside_mount(file: &str) -> bool {
    !file.is_empty()
        && (Path::new(file).components())
            .all(|c| matches!(c, Component::Normal(p) if !p.to_string_lossy().starts_with('.')))
}

/// The pack tier: each `workflows` mount's `index.json`, then each listed file's meta. The file
/// supplies the name, the index the listing fields.
pub(crate) fn pack_tier(mounts: &[Mount]) -> Vec<Listed> {
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
            let description = (item["description"].as_str().map(String::from))
                .or(meta.as_ref().map(|m| m.description.clone()))
                .unwrap_or_default();
            let when_to_use = (item["when_to_use"].as_str().map(String::from))
                .or(meta.and_then(|m| m.when_to_use))
                .unwrap_or_default();
            if let Some(name) = name {
                let pack = mount.pack.clone();
                tier.push(Listed {
                    pack,
                    name,
                    description,
                    when_to_use,
                    file,
                });
            }
        }
    }
    tier.sort_by(|a, b| (&a.pack, &a.name).cmp(&(&b.pack, &b.name)));
    tier
}

/// The project tier: each `*.rhai` file directly in a project mount, named by its `meta.name`.
pub(crate) fn project_tier(mounts: &[Mount]) -> Vec<Listed> {
    let mut tier = Vec::new();
    for mount in mounts
        .iter()
        .filter(|m| m.class == "project" && m.pack.is_empty())
    {
        for entry in fs::read_dir(&mount.path).into_iter().flatten().flatten() {
            let file = entry.path();
            if file.extension().is_none_or(|e| e != "rhai") {
                continue;
            }
            let meta = fs::read_to_string(&file)
                .ok()
                .and_then(|source| validate::validate(&source).ok());
            if let Some(meta) = meta {
                let when_to_use = meta.when_to_use.unwrap_or_default();
                tier.push(Listed {
                    pack: String::new(),
                    name: meta.name,
                    description: meta.description,
                    when_to_use,
                    file,
                });
            }
        }
    }
    tier.sort_by(|a, b| a.name.cmp(&b.name));
    tier
}

fn unreadable(what: &str, next: &str) -> Value {
    fail(WORKFLOW_RESOLVE_FAILED, &format!("{what}. Next: {next}"))
}

fn read(file: &Path) -> Result<String, Value> {
    fs::read_to_string(file).map_err(|e| {
        unreadable(
            &format!("cannot read {} ({e})", file.display()),
            "workflow help run",
        )
    })
}

/// One own-node read for the host to serve; the next input carries its answer.
fn ask(call: DataCall, round: u32) -> Value {
    json!({"data_calls": {"requests": [call], "state": {"rounds": round}}})
}

fn offered(input: &Value) -> bool {
    input["data_calls"] == true
}

/// The stored source a `source` round answered, or the failure when it holds none.
fn stored_source(input: &Value, tier: Tier) -> Result<Option<Resolved>, Value> {
    let Some(answer) = rounds::answer::<DataQueryResponse>(&input["data_results"], "source")?
    else {
        return Ok(None);
    };
    let source = answer.rows.first().and_then(|r| r.get("source"));
    match source.and_then(Value::as_str) {
        Some(source) => Ok(Some(Resolved {
            source: source.into(),
            tier,
        })),
        None => Err(unreadable("the script is not stored here", "workflow list")),
    }
}

/// A kept name, newest version (or `version`): the `name` round finds the WfName row, the
/// `source` round reads its WfScript. Only own-node rows are asked for (L2 section 4.14).
fn kept(input: &Value, name: &str, version: Option<u64>) -> Result<Resolved, Value> {
    let unknown = || {
        let what = format!("no kept workflow '{name}'");
        unreadable(&what, "workflow list")
    };
    let tier = Tier::Kept(name.into());
    if let Some(found) = stored_source(input, tier)? {
        return Ok(found);
    }
    let answer = rounds::answer::<DataQueryResponse>(&input["data_results"], "name")?;
    let Some(answer) = answer else {
        let mut filter = vec![json!({"name": {"_eq": name}}), data::own_node()];
        filter.extend(version.map(|v| json!({"version": {"_eq": v}})));
        let fields = ["script_id", "state"];
        let call = data::query("name", "WfName", json!({"_and": filter}), &fields);
        return Err(ask(
            data::limited(data::ordered(call, json!({"version": "DESC"})), 1),
            1,
        ));
    };
    let row = answer.rows.first().filter(|r| r["state"] == "kept");
    match row.and_then(|r| r["script_id"].as_str()) {
        Some(id) => Err(ask(data::script_source(id), 2)),
        None => Err(unknown()),
    }
}

/// `sha256:<script_id>`: one `source` round over the caller's own WfScript rows.
fn stored(input: &Value, script_id: &str) -> Result<Resolved, Value> {
    match stored_source(input, Tier::Stored)? {
        Some(found) => Ok(found),
        None => Err(ask(data::script_source(script_id), 1)),
    }
}

/// The kept tiers read through `data_calls`; a call the host did not offer them to has no kept
/// tier.
fn kept_offered(input: &Value, form: &str) -> Result<(), Value> {
    if offered(input) {
        return Ok(());
    }
    let what = format!("the host did not offer data_calls, so {form} cannot be read");
    Err(fail(
        WORKFLOW_NOT_AVAILABLE,
        &format!("{what}. Next: workflow help doctor"),
    ))
}

fn read_hit(hit: Listed, tier: Tier) -> Result<Resolved, Value> {
    Ok(Resolved {
        source: read(&hit.file)?,
        tier,
    })
}

/// The source a step-0 `run` names: the inline `script`, else the bound `path`, else the name in
/// `argv[1]`: a qualified form (`pack:`, `project:`, `kept:`, `sha256:`), or an unqualified name
/// through the pack tier, then the project tier, then the kept tier. The mounts are the only
/// place a pack or project script is looked for; the working folder is never probed. A kept
/// name or a stored script needs `data_calls` rounds, so this may return the round's output as
/// its `Err`; the step returns it as it is and calls this again with the answer.
pub fn resolve(input: &Value) -> Result<Resolved, Value> {
    if let Some(source) = input["script"].as_str() {
        return Ok(Resolved {
            source: source.into(),
            tier: Tier::Inline,
        });
    }
    if let Some(path) = input["path"].as_str() {
        return Ok(Resolved {
            source: read(Path::new(path))?,
            tier: Tier::Path,
        });
    }
    let Some(name) = input["argv"][1].as_str().filter(|n| !n.starts_with("--")) else {
        let what = "run needs a script, a path or a name";
        return Err(fail(
            WORKFLOW_INVALID_INPUT,
            &format!("{what}. Next: workflow help run"),
        ));
    };
    let mounts: Vec<Mount> = serde_json::from_value(input["mounts"].clone()).unwrap_or_default();
    let named = |tier: Vec<Listed>, name: &str| tier.into_iter().find(|e| e.name == name);
    let missing = |what: String| Err(unreadable(&what, "workflow list"));
    match name.split_once(':') {
        Some(("pack", rest)) => {
            let (pack, meta) = rest.rsplit_once('/').unwrap_or_default();
            let hit = pack_tier(&mounts)
                .into_iter()
                .find(|e| e.pack == pack && e.name == meta);
            return match hit {
                Some(hit) => read_hit(hit, Tier::Pack(pack.into())),
                None => missing(format!("no workflow '{meta}' in pack '{pack}'")),
            };
        }
        Some(("project", meta)) => {
            return match named(project_tier(&mounts), meta) {
                Some(hit) => read_hit(hit, Tier::Project),
                None => missing(format!("no workflow '{meta}' in the project tier")),
            };
        }
        Some(("kept", rest)) => {
            kept_offered(input, "kept names")?;
            let (name, v) = rest
                .split_once('@')
                .map_or((rest, None), |(n, v)| (n, Some(v)));
            let version = match v.map(str::parse::<u64>) {
                Some(Ok(v)) => Some(v),
                Some(Err(_)) => {
                    return Err(fail(
                        WORKFLOW_INVALID_INPUT,
                        "a kept version is a number. Next: workflow run kept:<name>@<n>",
                    ));
                }
                None => None,
            };
            return kept(input, name, version);
        }
        Some(("sha256", id)) => {
            kept_offered(input, "stored scripts")?;
            return stored(input, id);
        }
        Some(("shared", _)) => {
            return Err(fail(
                WORKFLOW_NOT_AVAILABLE,
                "shared: names are deferred to v1.1. Next: workflow list",
            ));
        }
        _ => {}
    }
    let mut packs = pack_tier(&mounts);
    packs.retain(|e| e.name == name);
    packs.dedup_by(|a, b| a.pack == b.pack);
    if let [first, _, ..] = packs.as_slice() {
        let all: Vec<&str> = packs.iter().map(|e| e.pack.as_str()).collect();
        let what = format!("'{name}' is in {}", all.join(" and "));
        return Err(unreadable(
            &what,
            &format!("workflow run pack:{}/{name}", first.pack),
        ));
    }
    if let Some(hit) = packs.pop() {
        let pack = hit.pack.clone();
        return read_hit(hit, Tier::Pack(pack));
    }
    if let Some(hit) = named(project_tier(&mounts), name) {
        return read_hit(hit, Tier::Project);
    }
    if offered(input) {
        return kept(input, name, None);
    }
    missing(format!(
        "no workflow named '{name}' in the pack or project tier"
    ))
}
