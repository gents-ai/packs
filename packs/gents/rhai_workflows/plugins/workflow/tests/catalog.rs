//! `list`, `list --format catalog` and `show` on the raw-argument shape (L2 section 4.14; root
//! C4, C6). The tests assert behaviour and codes, never wording (U15).
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use workflow::wire::{CatalogEntry, RawInput};
use workflow::{catalog, invoke};

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("catalog-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mount(pack: &str, class: &str, dir: &Path) -> Value {
    json!({"pack": pack, "class": class, "digest": "", "path": dir, "trusted": false})
}

fn source(name: &str, when_to_use: &str) -> String {
    format!(
        "let meta = #{{ name: {name:?}, description: \"about {name}\", when_to_use: {when_to_use:?},\n\
         phases: [#{{ title: \"first\" }}, #{{ title: \"second\", detail: \"later\" }}] }};\n\
         complete(1);\n"
    )
}

fn script_id(source: &str) -> String {
    let mut hash = Sha256::new();
    ["gents-rhai/1", source]
        .iter()
        .for_each(|p| hash.update(p.as_bytes()));
    format!("{:x}", hash.finalize())
}

/// A pack mount holding `alpha`, and a project mount holding `gamma`.
fn tiers(test: &str) -> (Value, Value, PathBuf, PathBuf) {
    let pack = scratch(&format!("{test}-pack"));
    let project = scratch(&format!("{test}-project"));
    std::fs::write(pack.join("alpha.rhai"), source("alpha", "to start")).unwrap();
    let index = json!([{"name": "alpha", "file": "alpha.rhai", "description": "from the index"}]);
    std::fs::write(pack.join("index.json"), index.to_string()).unwrap();
    std::fs::write(
        project.join("gamma.rhai"),
        source("gamma", "for the project"),
    )
    .unwrap();
    (
        mount("acme/tools", "workflows", &pack),
        mount("", "project", &project),
        pack,
        project,
    )
}

/// The host side of the data rounds: answers each `data_calls` output with `rows_of(request)`
/// and calls `invoke` again with the base input, that round's results and the echoed state.
fn serve(base: &Value, rows_of: impl Fn(&Value) -> Vec<Value>) -> Value {
    let mut input = base.clone();
    for _ in 0..8 {
        let out = invoke(input.clone());
        let Some(calls) = out.get("data_calls") else {
            return out;
        };
        assert!(out.get("writes").is_none(), "a round writes nothing: {out}");
        let mut results = serde_json::Map::new();
        for request in calls["requests"].as_array().expect(&out.to_string()) {
            let rows = rows_of(request);
            results.insert(
                request["id"].as_str().unwrap().into(),
                json!({"result": {"rows": rows}}),
            );
        }
        input = base.clone();
        input["data_results"] = Value::Object(results);
        input["state"] = calls["state"].clone();
    }
    panic!("the call never ended");
}

fn kept_row(name: &str, version: u64, state: &str) -> Value {
    json!({"name": name, "version": version, "state": state,
           "description": format!("{name} v{version}"), "when_to_use": format!("use {name}")})
}

fn kept_rows(request: &Value) -> Vec<Value> {
    assert_eq!(request["query"]["collection"], "WfName", "{request}");
    vec![
        kept_row("zeta", 1, "kept"),
        kept_row("other", 1, "kept"),
        kept_row("zeta", 2, "kept"),
        kept_row("gone", 1, "kept"),
        kept_row("gone", 2, "retired"),
    ]
}

#[test]
fn catalog_emits_catalog_entry_with_source() {
    let (pack, project, pack_dir, project_dir) = tiers("entry");
    let base = json!({"argv": ["list", "--format", "catalog"],
                      "asset_mounts": [pack, project], "data_calls": true});
    let first = invoke(base.clone());
    let request = &first["data_calls"]["requests"][0];
    assert_eq!(request["query"]["collection"], "WfName", "{first}");
    let filter = request["query"]["filter"].to_string();
    assert!(
        filter.contains("$caller.node_did"),
        "only own-node rows are read: {first}"
    );

    let out = serve(&base, kept_rows);
    let entries = out["result"].as_array().expect(&out.to_string());
    let alpha = pack_dir.join("alpha.rhai").display().to_string();
    let gamma = project_dir.join("gamma.rhai").display().to_string();
    let expected = json!([
        {"name": "alpha", "description": "from the index", "when_to_use": "to start",
         "source": "pack", "path": alpha},
        {"name": "gamma", "description": "about gamma", "when_to_use": "for the project",
         "source": "project", "path": gamma},
        {"name": "other", "description": "other v1", "when_to_use": "use other", "source": "kept"},
        {"name": "zeta", "description": "zeta v2", "when_to_use": "use zeta", "source": "kept"},
    ]);
    assert_eq!(out["result"], expected, "{out}");
    for entry in entries {
        serde_json::from_value::<CatalogEntry>(entry.clone()).expect("a C6 CatalogEntry");
    }
}

#[test]
fn catalog_call_emits_no_write() {
    let (pack, project, ..) = tiers("no_write");
    let base = json!({"argv": ["list", "--format", "catalog"],
                      "asset_mounts": [pack, project], "data_calls": true});
    let raw: RawInput = serde_json::from_value(base.clone()).unwrap();
    let rest = ["--format".to_string(), "catalog".to_string()];
    let ask = catalog::list(&base, &rest, &raw);
    assert!(ask.get("writes").is_none(), "{ask}");
    // Every key of every output is a read: a query round or the result.
    for key in ask.as_object().unwrap().keys() {
        assert_eq!(key, "data_calls", "{ask}");
    }
    let done = serve(&base, kept_rows);
    for key in done.as_object().unwrap().keys() {
        assert_eq!(key, "result", "{done}");
    }
}

#[test]
fn project_entries_only_with_a_project_mount() {
    let (pack, project, ..) = tiers("project");
    let catalog = |mounts: Value| {
        let input = json!({"argv": ["list", "--format", "catalog"], "asset_mounts": mounts});
        invoke(input)["result"].clone()
    };
    let without = catalog(json!([pack]));
    let sources: Vec<&str> = (without.as_array().unwrap().iter())
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["pack"], "{without}");
    let with = catalog(json!([pack, project]));
    let sources: Vec<&str> = (with.as_array().unwrap().iter())
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["pack", "project"], "{with}");
    // The text listing follows the same rule.
    let text = |mounts: Value| {
        let out = invoke(json!({"argv": ["list"], "asset_mounts": mounts}));
        out["result"].as_str().expect(&out.to_string()).to_string()
    };
    assert!(!text(json!([pack])).contains("gamma"));
    assert!(text(json!([pack, project])).contains("gamma"));
}

#[test]
fn kept_listing_pages_past_256_rows() {
    let base = json!({"argv": ["list", "--format", "catalog"], "asset_mounts": [],
                      "data_calls": true});
    let out = serve(&base, |request| {
        let offset = request["query"]["offset"].as_u64().unwrap_or(0);
        let total = 300;
        (offset..total.min(offset + 256))
            .map(|n| kept_row(&format!("n{n:03}"), 1, "kept"))
            .collect()
    });
    assert_eq!(out["result"].as_array().unwrap().len(), 300, "{out}");
}

#[test]
fn show_names_tier_script_id_and_versions() {
    let (pack, project, ..) = tiers("show");
    // A pack workflow: its tier, its script_id, its meta and its phases.
    let base = json!({"argv": ["show", "alpha"], "asset_mounts": [pack, project]});
    let out = invoke(base);
    let text = out["result"].as_str().expect(&out.to_string());
    assert!(text.contains("acme/tools"), "{text}");
    assert!(
        text.contains(&script_id(&source("alpha", "to start"))),
        "{text}"
    );
    for part in ["about alpha", "to start", "first", "second", "later"] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(out.get("writes").is_none(), "{out}");

    // A kept workflow: the newest kept version's script, and every version.
    let kept = source("zeta", "use zeta");
    let base = json!({"argv": ["show", "zeta"], "asset_mounts": [], "data_calls": true});
    let out = serve(&base, |request| {
        match request["query"]["collection"].as_str() {
            Some("WfScript") => vec![json!({"source": kept})],
            _ if request["id"] == "versions" => vec![
                json!({"version": 2, "state": "retired", "script_id": "b"}),
                json!({"version": 1, "state": "kept", "script_id": "a"}),
            ],
            _ => vec![json!({"script_id": "a", "state": "kept"})],
        }
    });
    let text = out["result"].as_str().expect(&out.to_string());
    assert!(text.contains("kept zeta"), "{text}");
    assert!(text.contains(&script_id(&kept)), "{text}");
    for part in ["v1", "v2", "retired"] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(out.get("writes").is_none(), "{out}");
}
