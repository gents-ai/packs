//! `export <name> [--namespace ns] [--pack-name p]` (L2 section 4.15): a kept workflow as the
//! files of an assets pack, returned inline because the guest cannot write outside a bind. The
//! receipt is answer-first and names the install with the pack digest. The digest is the one
//! gents' `declared_paths` and `PackDigester` define (gents `crates/gents/src/pack.rs`): for each
//! declared asset and `manifest.json`, sorted, the path length and the content length as u64
//! big-endian around the path and the content, hashed with sha256.
use crate::data;
use crate::errors::{
    WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE, WORKFLOW_RESOLVE_FAILED,
    WORKFLOW_VALIDATION_FAILED,
};
use crate::fail;
use crate::rounds;
use crate::run::refused;
use crate::validate;
use crate::wire::{DataCall, DataQueryResponse};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The license `import --local-only` stores a bundled script under; it never leaves the node.
const LOCAL_ONLY: &str = "xai-bundled-local-only";

pub fn is_export(input: &Value) -> bool {
    input["argv"][0].as_str() == Some("export")
}

fn invalid(what: &str, next: &str) -> Value {
    fail(WORKFLOW_INVALID_INPUT, &format!("{what}. Next: {next}"))
}

/// The pack digest of `files`: `manifest.json` and every declared asset, by path. The map's
/// order is the sorted order `declared_paths` gives.
pub fn digest(files: &BTreeMap<String, String>) -> String {
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
        hash.update((content.len() as u64).to_be_bytes());
        hash.update(content.as_bytes());
    }
    format!("sha256:{:x}", hash.finalize())
}

/// A pack name or namespace: gents' `is_snake_case_name`.
fn is_snake(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One output asking the host for one read; the next input echoes `carry` under `export`.
fn ask(call: DataCall, carry: Value) -> Value {
    json!({"data_calls": {"requests": [call], "state": {"export": carry}}})
}

/// `<name> [--namespace ns] [--pack-name p]`.
fn parse<'a>(rest: &[&'a str]) -> Option<(&'a str, Option<&'a str>, Option<&'a str>)> {
    let (name, mut flags) = rest.split_first()?;
    let (mut namespace, mut pack) = (None, None);
    while let Some((flag, tail)) = flags.split_first() {
        let (value, tail) = tail.split_first()?;
        flags = tail;
        match *flag {
            "--namespace" => namespace = Some(*value),
            "--pack-name" => pack = Some(*value),
            _ => return None,
        }
    }
    Some((name, namespace, pack))
}

pub fn serve(input: &Value) -> Value {
    if input["data_calls"] != true {
        let what = "the host did not offer data_calls, so kept names cannot be read";
        return fail(
            WORKFLOW_NOT_AVAILABLE,
            &format!("{what}. Next: workflow help doctor"),
        );
    }
    let argv: Vec<&str> = (input["argv"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    export(input, &argv[1..]).unwrap_or_else(|out| out)
}

/// The rounds: the newest own-node WfName row of the name, then the script it names with its
/// license. The name's row is carried across the second round.
fn export(input: &Value, rest: &[&str]) -> Result<Value, Value> {
    let Some((name, namespace, pack)) = parse(rest) else {
        let what = "export takes <name> [--namespace ns] [--pack-name p]";
        return Err(invalid(what, "workflow help export"));
    };
    let results = &input["data_results"];
    if let Some(page) = rounds::answer::<DataQueryResponse>(results, "script")? {
        let row = input["state"]["export"].clone();
        let stored = page.rows.into_iter().next();
        let field = |key: &str| stored.as_ref().and_then(|r| r.get(key)?.as_str());
        if field("license") == Some(LOCAL_ONLY) {
            let what = format!("'{name}' is stored {LOCAL_ONLY} and never leaves this node");
            return Err(invalid(&what, "workflow list"));
        }
        let Some(source) = field("source") else {
            let what = format!("the script of '{name}' is not stored here");
            let next = "workflow list";
            return Err(fail(
                WORKFLOW_RESOLVE_FAILED,
                &format!("{what}. Next: {next}"),
            ));
        };
        return files(source, &row, namespace, pack);
    }
    let fields = [
        "version",
        "state",
        "script_id",
        "description",
        "when_to_use",
    ];
    let filter = json!({"_and": [{"name": {"_eq": name}}, data::own_node()]});
    let call = data::query("name", "WfName", filter, &fields);
    let call = data::limited(data::ordered(call, json!({"version": "DESC"})), 1);
    let Some(page) = rounds::answer::<DataQueryResponse>(results, "name")? else {
        return Err(ask(call, json!({})));
    };
    let row = page.rows.into_iter().next();
    let Some(row) = row.filter(|r| r.get("state") == Some(&json!("kept"))) else {
        let what = format!("no kept workflow '{name}'");
        return Err(fail(
            WORKFLOW_RESOLVE_FAILED,
            &format!("{what}. Next: workflow list"),
        ));
    };
    let id = row
        .get("script_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let filter = json!({"_and": [{"script_id": {"_eq": id}}, data::own_node()]});
    let call = data::query("script", "WfScript", filter, &["source", "license"]);
    Err(ask(data::limited(call, 1), Value::Object(row)))
}

/// The four files and the receipt for the kept `row` over `source`.
fn files(
    source: &str,
    row: &Value,
    namespace: Option<&str>,
    pack: Option<&str>,
) -> Result<Value, Value> {
    let meta =
        validate::validate(source).map_err(|msg| refused(WORKFLOW_VALIDATION_FAILED, msg))?;
    let snake = meta.name.replace('-', "_");
    let namespace = namespace.unwrap_or("local");
    let pack = pack.unwrap_or(&snake);
    for (what, value) in [
        ("namespace", namespace),
        ("pack name", pack),
        ("file name", &snake),
    ] {
        if !is_snake(value) {
            let what =
                format!("the {what} {value:?} must be snake_case: lowercase letters, digits and _");
            return Err(invalid(&what, "workflow help export"));
        }
    }
    let text = |key: &str| row[key].as_str().unwrap_or_default();
    let script = format!("workflows/{snake}.rhai");
    let index = json!([{
        "name": meta.name, "file": format!("{snake}.rhai"),
        "description": text("description"), "when_to_use": text("when_to_use"),
    }]);
    let manifest = json!({
        "assets": ["README.md", "workflows/index.json", script],
        "authors": ["local"],
        "dependencies": [],
        "description": text("description"),
        "kind": "assets",
        "manifest_version": 1,
        "name": pack,
        "namespace": namespace,
        "tags": ["workflow"],
        "version": "0.1.0",
    });
    let pretty = |value: &Value| format!("{:#}\n", value);
    let readme = format!(
        "# {pack}\n\nThe `{name}` workflow, exported as a pack.\n\n## Usage\n\nInstall the pack, \
         add it to `trusted_packs`, then run `workflow run {name}`.\n",
        name = meta.name
    );
    let files = BTreeMap::from([
        ("manifest.json".to_string(), pretty(&manifest)),
        (script.clone(), source.to_string()),
        ("workflows/index.json".to_string(), pretty(&index)),
        ("README.md".to_string(), readme),
    ]);
    let digest = digest(&files);
    let name = &meta.name;
    let order = [
        "manifest.json",
        &script,
        "workflows/index.json",
        "README.md",
    ];
    let mut out = format!(
        "Exported '{name}' as pack {namespace}/{pack} (digest {digest}).\n\
         Next: write these files under ./{pack}/, then run \
         `gents pack install ./{pack} --expect-digest {digest}`.\n\
         files: {}\n\
         The pack runs with installed origin only after an operator installs it and adds it to \
         trusted_packs.\n\n",
        order.join(", ")
    );
    for path in order {
        let content = &files[path];
        out.push_str(&format!(
            "=== {path} ({} bytes) ===\n{content}\n",
            content.len()
        ));
    }
    Ok(json!({"result": out}))
}
