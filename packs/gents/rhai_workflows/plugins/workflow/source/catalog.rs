//! `list`, `list --format catalog` and `show` (L2 section 4.14; root C4, C6). Pack and project
//! entries come from the mounts, kept entries from the caller's own WfName rows through
//! `data_calls`. Neither verb writes.
use crate::data::{self, PAGE};
use crate::errors::{WORKFLOW_INVALID_INPUT, WORKFLOW_VALIDATION_FAILED};
use crate::fail;
use crate::rounds;
use crate::run::{HOST_API_VERSION, refused, sha256_hex};
use crate::tiers::{self, Tier, pack_tier, project_tier};
use crate::validate;
use crate::wire::{CatalogEntry, CatalogSource, DataCall, DataQueryResponse, RawInput};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type Map = serde_json::Map<String, Value>;

fn usage(what: &str, next: &str) -> Value {
    fail(WORKFLOW_INVALID_INPUT, &format!("{what}. Next: {next}"))
}

/// One output asking the host for one read; the next input echoes `state`.
fn ask(call: DataCall, state: Value) -> Value {
    json!({"data_calls": {"requests": [call], "state": state}})
}

/// The page of own-node WfName rows from `offset`, in `_docID` order: a total order, so pages
/// never skip or repeat a P2P twin.
fn kept_page(offset: usize) -> DataCall {
    let fields = ["name", "version", "description", "when_to_use", "state"];
    let call = data::query("kept", "WfName", data::own_node(), &fields);
    data::paged(data::ordered(call, json!({"_docID": "ASC"})), offset)
}

/// The kept tier: the newest version of each name, unless that version is retired. The newest
/// version per name is found client-side; a full page asks for the next one, and the state
/// carries the newest row of each name so far. Without `data_calls` there is no kept tier.
fn kept(input: &Value) -> Result<Vec<CatalogEntry>, Value> {
    if input["data_calls"] != true {
        return Ok(Vec::new());
    }
    let state = &input["state"];
    let mut newest: BTreeMap<String, Map> = state
        .get("newest")
        .and_then(|n| serde_json::from_value(n.clone()).ok())
        .unwrap_or_default();
    let offset = state["offset"].as_u64().unwrap_or(0) as usize;
    let page = rounds::answer::<DataQueryResponse>(&input["data_results"], "kept")?;
    let Some(page) = page else {
        let state = json!({"offset": offset, "newest": newest});
        return Err(ask(kept_page(offset), state));
    };
    let rows = page.rows.len();
    for row in page.rows {
        let version = row.get("version").and_then(Value::as_u64);
        let name = row.get("name").and_then(Value::as_str).map(String::from);
        let (Some(name), Some(version)) = (name, version) else {
            continue;
        };
        let held = newest.get(&name).and_then(|r| r["version"].as_u64());
        if held.is_none_or(|v| version > v) {
            newest.insert(name, row);
        }
    }
    if rows == PAGE as usize {
        let next = offset + rows;
        let state = json!({"offset": next, "newest": newest});
        return Err(ask(kept_page(next), state));
    }
    let text = |row: &Map, key: &str| row[key].as_str().unwrap_or_default().to_string();
    Ok((newest.iter())
        .filter(|(_, row)| row["state"] == "kept")
        .map(|(name, row)| CatalogEntry {
            name: name.clone(),
            description: text(row, "description"),
            when_to_use: text(row, "when_to_use"),
            source: CatalogSource::Kept,
            path: None,
        })
        .collect())
}

/// Every tier's entries: pack, then project (when a project mount is present), then kept.
fn entries(input: &Value, raw: &RawInput) -> Result<Vec<(CatalogEntry, String)>, Value> {
    let path = |file: &std::path::Path| Some(file.display().to_string());
    let mut all = Vec::new();
    for e in pack_tier(&raw.asset_mounts) {
        let entry = CatalogEntry {
            name: e.name,
            description: e.description,
            when_to_use: e.when_to_use,
            source: CatalogSource::Pack,
            path: path(&e.file),
        };
        all.push((entry, e.pack));
    }
    for e in project_tier(&raw.asset_mounts) {
        let entry = CatalogEntry {
            name: e.name,
            description: e.description,
            when_to_use: e.when_to_use,
            source: CatalogSource::Project,
            path: path(&e.file),
        };
        all.push((entry, "project".into()));
    }
    all.extend(kept(input)?.into_iter().map(|e| (e, "kept".into())));
    Ok(all)
}

/// `list` and `list --format catalog`.
pub fn list(input: &Value, rest: &[String], raw: &RawInput) -> Value {
    let catalog = match rest {
        [] => false,
        [flag, format] if flag == "--format" && format == "catalog" => true,
        _ => return usage("list takes --format catalog", "workflow help list"),
    };
    let all = match entries(input, raw) {
        Ok(all) => all,
        Err(out) => return out,
    };
    if catalog {
        let entries: Vec<&CatalogEntry> = all.iter().map(|(e, _)| e).collect();
        return json!({"result": entries});
    }
    if all.is_empty() {
        return json!({"result": "no workflows are listed here"});
    }
    let lines: Vec<String> = (all.iter())
        .map(|(e, tier)| format!("{}: {} [{tier}]", e.name, e.when_to_use))
        .collect();
    json!({"result": lines.join("\n")})
}

/// `show <name>`: the meta, the phases, the tier, the script_id and, for a kept name, every
/// version. The name resolves as `run` does; a kept name then needs one more round for its
/// versions, which carries the source in the state.
pub fn show(input: &Value, rest: &[String], raw: &RawInput) -> Value {
    let [name] = rest else {
        return usage("show takes <name>", "workflow help show");
    };
    if let Some(carried) = input["state"].get("show") {
        let source = carried["source"].as_str().unwrap_or_default();
        let kept = carried["kept"].as_str().unwrap_or_default();
        return match rounds::answer::<DataQueryResponse>(&input["data_results"], "versions") {
            Ok(Some(page)) => render(source, Tier::Kept(kept.into()), Some(&page.rows)),
            Ok(None) => usage("the versions answer is missing", "workflow show"),
            Err(out) => out,
        };
    }
    // The tiers read `mounts`; the raw shape names them `asset_mounts`.
    let resolving = json!({"argv": ["show", name], "mounts": raw.asset_mounts,
                           "data_calls": raw.data_calls, "data_results": input["data_results"]});
    let found = match tiers::resolve(&resolving) {
        Ok(found) => found,
        Err(out) => return out,
    };
    let Tier::Kept(kept) = &found.tier else {
        return render(&found.source, found.tier, None);
    };
    let filter = json!({"_and": [{"name": {"_eq": kept}}, data::own_node()]});
    let call = data::query("versions", "WfName", filter, &["version", "state"]);
    let call = data::ordered(call, json!({"version": "DESC"}));
    let state = json!({"show": {"source": found.source, "kept": kept}});
    ask(call, state)
}

fn render(source: &str, tier: Tier, versions: Option<&[Map]>) -> Value {
    let meta = match validate::validate(source) {
        Ok(meta) => meta,
        Err(msg) => return refused(WORKFLOW_VALIDATION_FAILED, msg),
    };
    let mut lines = vec![
        format!("{}: {}", meta.name, meta.description),
        format!("tier: {}", tier.label()),
        format!("script_id: {}", sha256_hex(&[HOST_API_VERSION, source])),
    ];
    lines.extend(meta.when_to_use.map(|w| format!("when to use: {w}")));
    for (n, phase) in meta.phases.iter().enumerate() {
        let detail = (phase.detail.as_ref()).map_or(String::new(), |d| format!(": {d}"));
        lines.push(format!("phase {}: {}{detail}", n + 1, phase.title));
    }
    if let Some(rows) = versions {
        let each: Vec<String> = (rows.iter())
            .map(|r| format!("v{} ({})", r["version"], r["state"].as_str().unwrap_or("?")))
            .collect();
        lines.push(format!("versions: {}", each.join(", ")));
    }
    json!({"result": lines.join("\n")})
}
