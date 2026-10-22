//! `keep` and `retire` (L2 section 4.14; root C3): versions, renames, the naming refusals and the
//! lost race, all through the fake host's own-node data and unique index. The tests assert
//! codes, versions and rows, never Grok text (U15).
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use workflow::fakehost::admit::view;
use workflow::fakehost::cli::facts;
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::{invoke, keep};

const FLOW: &str = "let meta = #{ name: \"flow\", description: \"d\", when_to_use: \"to flow\" };\n\
                    let r = agent(\"go\");\ncomplete(r.output);\n";

const HOLD: &str = r##"let meta = #{ name: "hold", description: "pauses on every run" };
pause("user", "hold on");
complete(1);
"##;

fn script_id(source: &str) -> String {
    let mut hash = Sha256::new();
    ["gents-rhai/1", source]
        .iter()
        .for_each(|p| hash.update(p.as_bytes()));
    format!("{:x}", hash.finalize())
}

/// A run of `source` as tool call `tool_call_id`.
fn run_as(source: &str, tool_call_id: &str) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!(tool_call_id);
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": source, "args": {}}),
        host: facts,
    }
}

/// A later non-job call `tool_call_id` with `argv`, `jobs` as the host lists them.
fn tool(host: &FakeHost, tool_call_id: &str, argv: &[&str]) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!(tool_call_id);
    let jobs: Vec<Value> = (host.state.rows.values())
        .map(|r| {
            json!({"row_doc_id": r.row_doc_id, "tool_call_id": r.tool_call_id,
                   "handle": r.handle, "state": r.state})
        })
        .collect();
    let chain = host.jobs.chains.values().next();
    let children = chain.map_or(json!({}), |c| json!(view(&host.state, c)));
    Call {
        kind: Kind::ToolCall,
        input: json!({"protocol": 1, "argv": argv, "jobs": jobs, "children": children}),
        host: facts,
    }
}

/// One non-job `keep` or `retire` call, run to its final output with the writes applied.
fn call(host: &mut FakeHost, tool_call_id: &str, argv: &[&str]) -> Value {
    let call = tool(host, tool_call_id, argv);
    assert!(keep::is_keep(&call.input), "{argv:?} is a keep verb");
    host.tool_call(&call, invoke).expect("the host accepts it")
}

/// `(name, version, state)` of every WfName row of `name`, in write order.
fn versions(host: &FakeHost, name: &str) -> Vec<(u64, String)> {
    let rows = host.state.collections.get("WfName");
    (rows.into_iter().flatten())
        .filter(|r| r["name"] == name)
        .map(|r| (r["version"].as_u64().unwrap(), r["state"].to_string()))
        .collect()
}

fn kept(version: u64) -> (u64, String) {
    (version, "\"kept\"".into())
}

fn refusal(out: &Value) -> (&str, &str) {
    let fail = &out["fail"];
    (
        fail["code"].as_str().expect(&out.to_string()),
        fail["message"].as_str().unwrap(),
    )
}

#[test]
fn row14_keep_rename_replace_bump_versions() {
    let mut host = FakeHost::default();
    host.step0(&run_as(FLOW, "t1"), invoke).unwrap();
    let id = script_id(FLOW);

    // A handle keeps the script under its own name, with the meta's fields.
    let out = call(&mut host, "k1", &["keep", "flow"]);
    assert!(out["result"].as_str().is_some(), "{out}");
    assert_eq!(versions(&host, "flow"), [kept(1)]);
    let row = &host.state.collections["WfName"][0];
    assert_eq!(row["script_id"], id.as_str(), "{row}");
    assert_eq!(row["description"], "d");
    assert_eq!(row["when_to_use"], "to flow");

    // --replace bumps the version and takes the given description and when_to_use.
    let args = ["keep", "flow", "--replace", "--description", "new d"];
    let out = call(
        &mut host,
        "k2",
        &[&args[..], &["--when-to-use", "new w"]].concat(),
    );
    assert!(out.get("fail").is_none(), "{out}");
    assert_eq!(versions(&host, "flow"), [kept(1), kept(2)]);
    let row = &host.state.collections["WfName"][1];
    assert_eq!(
        (&row["description"], &row["when_to_use"]),
        (&json!("new d"), &json!("new w"))
    );

    // --name renames: the meta is rewritten, so the script has a new id and a new WfScript row.
    call(&mut host, "k3", &["keep", "flow", "--name", "renamed"]);
    assert_eq!(versions(&host, "renamed"), [kept(1)]);
    let source = FLOW.replace("name: \"flow\"", "name: \"renamed\"");
    let renamed_id = script_id(&source);
    let scripts = &host.state.collections["WfScript"];
    assert_eq!(scripts.len(), 2, "{scripts:?}");
    assert_eq!(scripts[1]["script_id"], renamed_id.as_str());
    assert_eq!(scripts[1]["source"], source.as_str());
    assert_eq!(
        scripts[1]["meta_json"],
        json!({"name": "renamed", "description": "d"}).to_string()
    );
    assert_eq!(
        host.state.collections["WfName"][2]["script_id"],
        renamed_id.as_str()
    );

    // A script_id and a sha256: id name a stored script the same way.
    call(&mut host, "k4", &["keep", &renamed_id, "--replace"]);
    assert_eq!(versions(&host, "renamed"), [kept(1), kept(2)]);
    call(
        &mut host,
        "k5",
        &["keep", &format!("sha256:{id}"), "--replace"],
    );
    assert_eq!(versions(&host, "flow"), [kept(1), kept(2), kept(3)]);
    assert_eq!(
        host.state.collections["WfScript"].len(),
        2,
        "no script is stored twice"
    );
}

#[test]
fn existing_numbered_and_pack_names_are_refused() {
    let mut host = FakeHost::default();
    host.step0(&run_as(FLOW, "t1"), invoke).unwrap();
    call(&mut host, "k1", &["keep", "flow"]);

    // An existing kept name needs --replace.
    let out = call(&mut host, "k2", &["keep", "flow"]);
    let (code, message) = refusal(&out);
    assert_eq!(code, "workflow_invalid_input");
    let want = "workflow_invalid_input: 'flow' is kept (v1). Next: workflow keep flow --replace";
    assert_eq!(message, want);
    assert!(out.get("writes").is_none(), "{out}");
    assert_eq!(versions(&host, "flow"), [kept(1)]);

    // A numbered handle needs --name, and the name it is given is the one kept.
    host.step0(&run_as(FLOW, "t2"), invoke).unwrap();
    let out = call(&mut host, "k3", &["keep", "flow-2"]);
    let (code, message) = refusal(&out);
    assert_eq!(code, "workflow_invalid_input");
    let want = "workflow_invalid_input: 'flow-2' is a numbered handle. Next: workflow keep flow-2 --name <name>";
    assert_eq!(message, want);
    call(&mut host, "k4", &["keep", "flow-2", "--name", "flow-two"]);
    assert_eq!(versions(&host, "flow-two"), [kept(1)]);

    // A name a pack of the pack tier holds is always refused, with or without --replace.
    let dir: PathBuf = std::env::temp_dir().join(format!("wf-keep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("flow.rhai"), FLOW).unwrap();
    let index = json!([{"name": "flow", "file": "flow.rhai", "description": "d"}]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    let mounts = json!([{"pack": "acme/tools", "class": "workflows", "digest": "", "path": dir,
                         "trusted": false}]);
    let mut host = FakeHost::default();
    host.step0(&run_as(FLOW, "t1"), invoke).unwrap();
    for argv in [&["keep", "flow"][..], &["keep", "flow", "--replace"]] {
        let mut held = tool(&host, "k1", argv);
        held.host.mounts = mounts.clone();
        let out = host.tool_call(&held, invoke).unwrap();
        let (code, message) = refusal(&out);
        assert_eq!(code, "workflow_invalid_input");
        let want = "workflow_invalid_input: 'flow' is held by the pack tier (acme/tools). Next: workflow keep flow --name <other>";
        assert_eq!(message, want, "{argv:?}");
    }
    assert!(!host.state.collections.contains_key("WfName"));
    let mut held = tool(&host, "k2", &["keep", "flow", "--name", "mine"]);
    held.host.mounts = mounts;
    host.tool_call(&held, invoke).unwrap();
    assert_eq!(versions(&host, "mine"), [kept(1)]);
}

#[test]
fn concurrent_keep_reports_the_race() {
    let mut host = FakeHost::default();
    host.step0(&run_as(FLOW, "t1"), invoke).unwrap();

    // Another keep of the name lands between this keep's two reads of its newest version.
    let racing = tool(&host, "k1", &["keep", "flow"]);
    let out = host
        .tool_call(&racing, |mut input| {
            if input["data_results"].get("confirm").is_some() {
                let rows = json!([{"version": 1, "state": "kept"}]);
                input["data_results"]["confirm"] = json!({"result": {"rows": rows}});
            }
            invoke(input)
        })
        .unwrap();
    let (code, message) = refusal(&out);
    assert_eq!(code, "workflow_invalid_input");
    let want = "workflow_invalid_input: 'flow' v1 was kept concurrently. Next: workflow keep flow --replace";
    assert_eq!(message, want);
    assert!(out.get("writes").is_none(), "{out}");
    assert!(!host.state.collections.contains_key("WfName"));

    // The unique index is the backstop: a keep whose reads are stale writes the version another
    // keep already holds. The index rejects that write, the host fails the whole call, and no
    // row is added.
    call(&mut host, "k2", &["keep", "flow"]);
    let stale = tool(&host, "k3", &["keep", "flow"]);
    let err = host
        .tool_call(&stale, |mut input| {
            for round in ["name", "confirm"] {
                if input["data_results"].get(round).is_some() {
                    input["data_results"][round] = json!({"result": {"rows": []}});
                }
            }
            invoke(input)
        })
        .unwrap_err();
    assert!(err.contains("WfName violates unique index"), "{err}");
    assert_eq!(versions(&host, "flow"), [kept(1)]);
}

#[test]
fn retire_writes_retired_version_and_keeps_running_pins() {
    let mut host = FakeHost::default();
    let first = host
        .step0(&run_as(HOLD, "t1"), invoke)
        .unwrap()
        .row
        .unwrap();
    call(&mut host, "k1", &["keep", "hold"]);

    let out = call(&mut host, "k2", &["retire", "hold"]);
    assert!(out.get("fail").is_none(), "{out}");
    let writes = out["writes"].as_array().expect(&out.to_string());
    assert_eq!(writes.len(), 1, "{out}");
    assert_eq!(writes[0]["collection"], "WfName");
    assert_eq!(writes[0]["on_conflict"], "fail");
    assert!(out.get("control").is_none(), "no run is touched: {out}");
    assert_eq!(
        versions(&host, "hold"),
        [kept(1), (2, "\"retired\"".into())]
    );
    let rows = &host.state.collections["WfName"];
    for field in ["script_id", "description", "when_to_use"] {
        assert_eq!(rows[0][field], rows[1][field], "{field}");
    }

    // The retired name no longer runs, and an unknown one cannot be retired.
    let call_run = Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "hold"]}),
        host: facts("/work"),
    };
    let out = host.step0(&call_run, invoke).unwrap().output;
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");
    let out = call(&mut host, "k3", &["retire", "nope"]);
    let (code, message) = refusal(&out);
    assert_eq!(code, "workflow_resolve_failed");
    assert!(message.ends_with("Next: workflow list"), "{message}");

    // A run that was already pinned resumes from its own stored source.
    let mut resume = tool(&host, "t2", &["resume", "hold"]);
    resume.kind = Kind::JobStep;
    let resumed = host.step0(&resume, invoke).unwrap();
    assert_eq!(
        resumed.output["resume"],
        json!({"row_doc_id": first}),
        "{}",
        resumed.output
    );
}
