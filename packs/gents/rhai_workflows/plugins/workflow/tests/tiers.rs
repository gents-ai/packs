//! Where a run's `.rhai` comes from: the pack, project, inline and path tiers on step 0.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, HostFacts, Kind, PackAssets, mounts};
use workflow::invoke;
use workflow::wire::JobState;

fn script(name: &str) -> String {
    format!(
        "let meta = #{{ name: \"{name}\", description: \"d\" }};\n\
         let r = agent(\"go\");\ncomplete(r.output);\n"
    )
}

/// An empty directory of its own.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wf-tiers-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mount(pack: &str, class: &str, path: &std::path::Path, trusted: bool) -> Value {
    json!({"pack": pack, "class": class, "digest": "", "path": path, "trusted": trusted})
}

/// A step-0 `run` input under `dynamic`, with `extra`'s keys laid over it.
fn step0(dynamic: &str, extra: Value) -> Value {
    let mut input = json!({
        "protocol": 1, "argv": ["run", "gate"],
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"session_id": "s", "tool_call_id": "t1", "cwd": "/nowhere"},
        "ceiling": {"targets": ["wf-read-only"], "max_live": 4, "remaining_spawns": 9,
                    "dynamic": dynamic, "dynamic_targets": ["wf-read-only"]}
    });
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    invoke(input)
}

fn stored(out: &Value) -> Value {
    let writes = out["effects"]["writes"].as_array().expect(&out.to_string());
    let rows = (writes.iter()).filter(|w| w["collection"] == "WfScript");
    rows.map(|w| w["fields"].clone())
        .next()
        .expect("a WfScript write")
}

fn line(out: &Value) -> &str {
    out["effects"]["progress"]["last_event"]
        .as_str()
        .expect(&out.to_string())
}

fn digest(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

#[test]
fn pack_tier_resolves_through_index_and_meta() {
    let dir = scratch("pack_index");
    let source = script("gate");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/file.rhai"), &source).unwrap();
    // The index names the entry differently; the file's meta.name is the identity.
    let index = json!([{"name": "listed", "file": "sub/file.rhai", "description": "d"}]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    let mounts = json!([mount("acme/tools", "workflows", &dir, false)]);

    let out = step0("run", json!({"mounts": mounts}));
    assert_eq!(stored(&out)["source"], source.as_str(), "{out}");
    assert_eq!(stored(&out)["origin_hint"], "pack");
    assert!(
        line(&out).starts_with("started from pack acme/tools ("),
        "{}",
        line(&out)
    );

    let by_index = step0("run", json!({"argv": ["run", "listed"], "mounts": mounts}));
    assert_eq!(
        by_index["fail"]["code"], "workflow_resolve_failed",
        "{by_index}"
    );
}

#[test]
fn project_tier_only_with_a_project_mount() {
    let cwd = scratch("project_cwd");
    let dir = cwd.join(".gents/workflows");
    std::fs::create_dir_all(&dir).unwrap();
    let source = script("gate");
    std::fs::write(dir.join("gate.rhai"), &source).unwrap();
    let caller = json!({"session_id": "s", "tool_call_id": "t1", "cwd": cwd});

    // The directory is on disk but no mount names it: the tier is off.
    let off = step0("run", json!({"caller": caller, "mounts": []}));
    assert_eq!(off["fail"]["code"], "workflow_resolve_failed", "{off}");

    let mounts = json!([mount("", "project", &dir, false)]);
    let on = step0("run", json!({"caller": caller, "mounts": mounts}));
    assert_eq!(stored(&on)["source"], source.as_str(), "{on}");
    assert_eq!(stored(&on)["origin_hint"], "project");
    let want = format!(
        "started from project (predicted origin dynamic, sha256:{})",
        digest(&source)
    );
    assert_eq!(line(&on), want);

    // An entry with no pack is never a pack, even if it claims to be trusted.
    let claimed = json!([mount("", "project", &dir, true)]);
    let out = step0("off", json!({"caller": caller, "mounts": claimed}));
    assert_eq!(out["fail"]["code"], "workflow_policy_denied", "{out}");
}

#[test]
fn path_source_is_stored_as_wfscript_at_step0() {
    let dir = scratch("path");
    let file = dir.join("flow.rhai");
    let source = script("flow");
    std::fs::write(&file, &source).unwrap();
    let call = Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run"], "path": file}),
        host: facts("/work"),
    };
    let mut host = FakeHost::default();
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().expect(&s0.output.to_string());
    assert_eq!(stored(&s0.output)["source"], source.as_str());
    assert_eq!(stored(&s0.output)["origin_hint"], "path");
    assert_eq!(host.state.collections["WfScript"].len(), 1);

    // Later steps run headless: the file may be gone.
    std::fs::remove_file(&file).unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(s1.output["result"], "ok", "{}", s1.output);
    assert_eq!(host.state.rows[&row].state, JobState::Completed);

    let missing = step0(
        "run",
        json!({"argv": ["run"], "path": dir.join("gone.rhai")}),
    );
    assert_eq!(
        missing["fail"]["code"], "workflow_resolve_failed",
        "{missing}"
    );
}

#[test]
fn trusted_pack_mount_predicts_installed_and_names_the_pack() {
    let source = script("gate");
    let packs = |pack: &str, trusted: bool| {
        let assets = [PackAssets {
            pack: pack.into(),
            class: "workflows".into(),
            digest: "sha256:pack".into(),
            trusted,
        }];
        let value = mounts(&assets, None).unwrap();
        let dir = value[0]["path"].as_str().unwrap().to_string();
        std::fs::write(format!("{dir}/gate.rhai"), &source).unwrap();
        let index = json!([{"name": "gate", "file": "gate.rhai", "description": "d"}]);
        std::fs::write(format!("{dir}/index.json"), index.to_string()).unwrap();
        value
    };

    // `dynamic: off` refuses a dynamic run at step 0, so only an installed one passes.
    let trusted = step0("off", json!({"mounts": packs("tiers-trusted", true)}));
    let want = format!(
        "started from pack tiers-trusted (predicted origin installed, sha256:{})",
        digest(&source)
    );
    assert_eq!(line(&trusted), want, "{trusted}");

    let untrusted = step0("off", json!({"mounts": packs("tiers-untrusted", false)}));
    assert_eq!(
        untrusted["fail"]["code"], "workflow_policy_denied",
        "{untrusted}"
    );
}

/// `script(name)` with a marker line, so two sources of one name differ.
fn marked(name: &str, tag: &str) -> String {
    format!("{}// {tag}\n", script(name))
}

/// A directory holding `file` (named `file`) and an `index.json` that lists it.
fn index_of(tag: &str, name: &str, source: &str) -> PathBuf {
    let dir = scratch(tag);
    std::fs::write(dir.join("f.rhai"), source).unwrap();
    let index = json!([{"name": name, "file": "f.rhai", "description": "d"}]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    dir
}

fn host_row(host: &mut FakeHost, collection: &str, row: Value) {
    (host.state.collections.entry(collection.into()))
        .or_default()
        .push(row);
}

/// A fake host that holds `source` as a stored script of `node`, under `script_id`.
fn store(host: &mut FakeHost, node: &str, script_id: &str, source: &str) {
    let row = json!({"node_did": node, "script_id": script_id, "source": source});
    host_row(host, "WfScript", row);
}

/// A WfName row of `node`.
fn keep(host: &mut FakeHost, node: &str, name: &str, version: u64, id: &str, state: &str) {
    let row = json!({"node_did": node, "name": name, "version": version, "script_id": id,
                     "state": state, "description": "d", "when_to_use": "w"});
    host_row(host, "WfName", row);
}

/// Step 0 of `run <name>` on `host` (caller `did:cli`) with `mounts`; the step's output.
fn go(host: &mut FakeHost, name: &str, mounts: Value) -> Value {
    let call = Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", name]}),
        host: HostFacts {
            mounts,
            ..facts("/work")
        },
    };
    host.step0(&call, invoke).unwrap().output
}

fn source_of(out: &Value) -> String {
    stored(out)["source"].as_str().unwrap().into()
}

#[test]
fn kept_resolution_reads_only_own_node_rows() {
    let mut host = FakeHost::default();
    let (old, new, theirs) = (
        marked("mine", "old"),
        marked("mine", "new"),
        marked("theirs", "t"),
    );
    store(&mut host, "did:cli", "s-old", &old);
    store(&mut host, "did:cli", "s-new", &new);
    store(&mut host, "did:other", "s-theirs", &theirs);
    store(&mut host, "did:cli", "s-gone", &marked("gone", "g"));
    keep(&mut host, "did:cli", "mine", 1, "s-old", "kept");
    keep(&mut host, "did:cli", "mine", 2, "s-new", "kept");
    keep(&mut host, "did:other", "theirs", 1, "s-theirs", "kept");
    keep(&mut host, "did:cli", "gone", 1, "s-gone", "kept");
    keep(&mut host, "did:cli", "gone", 2, "s-gone", "retired");

    // The newest version of an own name is the one that runs.
    let own = go(&mut host, "mine", json!([]));
    assert_eq!(source_of(&own), new, "{own}");
    assert!(
        line(&own).starts_with("started from kept mine ("),
        "{}",
        line(&own)
    );

    // Another node's row is never used, and a name whose newest row is retired is not kept.
    for name in ["theirs", "kept:theirs", "gone"] {
        let out = go(&mut host, name, json!([]));
        assert_eq!(
            out["fail"]["code"], "workflow_resolve_failed",
            "{name}: {out}"
        );
        assert!(out.to_string().contains("Next: workflow list"), "{out}");
    }
    // A script id of another node is not read either.
    let out = go(&mut host, "sha256:s-theirs", json!([]));
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");
}

#[test]
fn unqualified_order_is_pack_project_kept() {
    let (p, j, k) = (
        marked("gate", "pack"),
        marked("gate", "project"),
        marked("gate", "kept"),
    );
    let pack = mount(
        "acme/tools",
        "workflows",
        &index_of("order_pack", "gate", &p),
        false,
    );
    let project = mount("", "project", &index_of("order_project", "gate", &j), false);
    let mut host = FakeHost::default();
    store(&mut host, "did:cli", "s-k", &k);
    keep(&mut host, "did:cli", "gate", 1, "s-k", "kept");

    let all = json!([pack, project]);
    assert_eq!(source_of(&go(&mut host, "gate", all)), p);
    assert_eq!(source_of(&go(&mut host, "gate", json!([project]))), j);
    assert_eq!(source_of(&go(&mut host, "gate", json!([]))), k);
}

#[test]
fn cross_pack_ambiguity_is_refused_with_qualified_next() {
    let (a, b) = (marked("gate", "a"), marked("gate", "b"));
    let one = mount(
        "gents/a",
        "workflows",
        &index_of("amb_a", "gate", &a),
        false,
    );
    let two = mount("acme/b", "workflows", &index_of("amb_b", "gate", &b), false);
    let mounts = json!([one, two]);
    let out = step0("run", json!({"mounts": mounts}));
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");
    let message = out["fail"]["message"].as_str().unwrap();
    assert!(
        message.contains("'gate' is in acme/b and gents/a"),
        "{message}"
    );
    assert!(
        message.ends_with("Next: workflow run pack:acme/b/gate"),
        "{message}"
    );

    let picked = step0(
        "run",
        json!({"argv": ["run", "pack:gents/a/gate"], "mounts": mounts}),
    );
    assert_eq!(stored(&picked)["source"], a.as_str(), "{picked}");
}

#[test]
fn qualified_forms_resolve_and_shared_is_not_available() {
    let (p, j, k) = (
        marked("gate", "pack"),
        marked("gate", "project"),
        marked("gate", "kept"),
    );
    let pack = mount(
        "acme/tools",
        "workflows",
        &index_of("qual_pack", "gate", &p),
        false,
    );
    let dir = scratch("qual_project");
    std::fs::write(dir.join("gate.rhai"), &j).unwrap();
    let project = mount("", "project", &dir, false);
    let mounts = json!([pack, project]);
    let mut host = FakeHost::default();
    let (k1, k2) = (marked("gate", "k1"), k);
    store(&mut host, "did:cli", "s-1", &k1);
    store(&mut host, "did:cli", "s-2", &k2);
    keep(&mut host, "did:cli", "gate", 1, "s-1", "kept");
    keep(&mut host, "did:cli", "gate", 2, "s-2", "kept");

    let cases = [
        ("pack:acme/tools/gate", &p),
        ("project:gate", &j),
        ("kept:gate", &k2),
        ("kept:gate@1", &k1),
        ("sha256:s-1", &k1),
    ];
    for (name, want) in cases {
        let out = go(&mut host, name, mounts.clone());
        assert_eq!(source_of(&out), want.as_str(), "{name}: {out}");
    }
    // A qualified form never falls through to another tier.
    let out = go(&mut host, "pack:acme/tools/other", mounts.clone());
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");

    let shared = go(&mut host, "shared:did:peer/gate", mounts);
    assert_eq!(shared["fail"]["code"], "workflow_not_available", "{shared}");
}
