//! The job verbs over every tier (L2 sections 4.10 and 4.14; root C9): `run` takes a name, a
//! script or a path and bounds `agent_budget`, `resume` and `answer` take an address or a
//! handle, and `import` and `keep --from` are deferred.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use workflow::fakehost::admit::view;
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
    let dir = std::env::temp_dir().join(format!("wf-lifecycle-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pack mount of `dir` holding `source` as `gate.rhai`, listed in its index.
fn pack_mount(dir: &Path, source: &str) -> Value {
    std::fs::write(dir.join("gate.rhai"), source).unwrap();
    let index = json!([{"name": "gate", "file": "gate.rhai", "description": "d"}]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    json!([{"pack": "acme/tools", "class": "workflows", "digest": "", "path": dir,
            "trusted": false}])
}

/// A step-0 `run` input with `extra`'s keys laid over it.
fn step0(extra: Value) -> Value {
    let mut input = json!({
        "protocol": 1, "argv": ["run", "gate"],
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"session_id": "s", "tool_call_id": "t1", "cwd": "/nowhere"},
        "ceiling": {"targets": ["wf-read-only"], "max_live": 4, "remaining_spawns": 9,
                    "dynamic": "run", "dynamic_targets": ["wf-read-only"]}
    });
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    invoke(input)
}

#[test]
fn run_accepts_name_script_or_path_and_bounds_agent_budget() {
    let (dir, source) = (scratch("run"), script("gate"));
    let mounts = pack_mount(&dir, &source);
    let file = dir.join("flow.rhai");
    std::fs::write(&file, script("flow")).unwrap();

    let by_name = step0(json!({"mounts": mounts}));
    assert_eq!(by_name["effects"]["pin"]["handle"], "gate", "{by_name}");
    let by_script = step0(json!({"argv": ["run", "inline"], "script": script("inline")}));
    assert_eq!(
        by_script["effects"]["pin"]["handle"], "inline",
        "{by_script}"
    );
    let by_path = step0(json!({"argv": ["run"], "path": file}));
    assert_eq!(by_path["effects"]["pin"]["handle"], "flow", "{by_path}");

    let pinned = |budget: Value| step0(json!({"mounts": mounts, "agent_budget": budget}));
    assert_eq!(
        step0(json!({"mounts": mounts}))["effects"]["pin"]["agent_budget"],
        128
    );
    for ok in [1, 1024] {
        assert_eq!(pinned(json!(ok))["effects"]["pin"]["agent_budget"], ok);
    }
    for bad in [json!(0), json!(1025), json!(-1), json!(1.5), json!("7")] {
        let out = pinned(bad.clone());
        assert_eq!(
            out["fail"]["code"], "workflow_invalid_input",
            "{bad}: {out}"
        );
        let message = out["fail"]["message"].as_str().unwrap();
        assert!(message.contains("1..1024"), "{message}");
        assert!(message.ends_with("Next: workflow help run"), "{message}");
    }
}

#[test]
fn trusted_pack_script_runs_installed_end_to_end() {
    let source = script("gate");
    let assets = [PackAssets {
        pack: "lifecycle-trusted".into(),
        class: "workflows".into(),
        digest: "sha256:pack".into(),
        trusted: true,
    }];
    let mounts = mounts(&assets, None).unwrap();
    pack_mount(Path::new(mounts[0]["path"].as_str().unwrap()), &source);

    // `dynamic: off` refuses anything but an installed origin.
    let host_facts = HostFacts {
        mounts,
        ceiling: json!({"targets": ["wf-read-only"], "max_live": 64, "remaining_spawns": 1024,
                        "max_messages_per_slot": 8, "result_bytes": 65536,
                        "dynamic": "off", "dynamic_targets": ["wf-read-only"]}),
        ..facts("/work")
    };
    let call = Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "gate"]}),
        host: host_facts,
    };
    let mut host = FakeHost::default();
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().expect(&s0.output.to_string());
    let digest = format!("{:x}", Sha256::digest(source.as_bytes()));
    let line = s0.output["effects"]["progress"]["last_event"]
        .as_str()
        .unwrap();
    let want = format!(
        "started from pack lifecycle-trusted (predicted origin installed, sha256:{digest})"
    );
    assert_eq!(line, want, "{}", s0.output);

    // Step 1 runs the host's verified bytes under the installed origin.
    host.settle(&spawns(&s0.output), canned);
    let mut call = call;
    call.host.verified_script =
        Some(json!({"source": source, "digest": format!("sha256:{digest}")}));
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(s1.output["result"], "ok", "{}", s1.output);
    assert_eq!(host.state.rows[&row].state, JobState::Completed);
}

const HOLD: &str = r##"let meta = #{ name: "hold", description: "pauses on every run" };
pause("user", "hold on");
complete(1);
"##;

const TWO: &str = r##"let meta = #{ name: "two", description: "two questions in a row" };
let a = await_user("user", "first?");
let b = await_user("user", "second?");
complete(#{ a: a, b: b });
"##;

fn run_call(script: &str) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script, "args": {}}),
        host: facts("/work"),
    }
}

/// A later tool call `tool_call_id` with `argv`: `jobs` and the chain's `children` as listed.
fn call_as(host: &FakeHost, tool_call_id: &str, argv: &[&str]) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!(tool_call_id);
    let jobs: Vec<Value> = (host.state.rows.values())
        .map(|r| {
            json!({"row_doc_id": r.row_doc_id, "tool_call_id": r.tool_call_id,
                   "handle": r.handle, "state": r.state})
        })
        .collect();
    let chain = host.jobs.chains.values().next().expect("one chain");
    let input = json!({"protocol": 1, "argv": argv, "jobs": jobs,
                       "children": view(&host.state, chain)});
    Call {
        kind: Kind::JobStep,
        input,
        host: facts,
    }
}

#[test]
fn resume_and_answer_accept_address_or_handle() {
    // resume: by the job address, then by the handle (the newest row of the chain).
    let (mut host, call) = (FakeHost::default(), run_call(HOLD));
    let first = host.step0(&call, invoke).unwrap().row.unwrap();
    let by_address = host
        .step0(&call_as(&host, "t2", &["resume", "cli:t1"]), invoke)
        .unwrap();
    let second = by_address.row.clone().unwrap();
    let out = &by_address.output;
    assert_eq!(out["resume"], json!({"row_doc_id": first}), "{out}");
    let by_handle = host
        .step0(&call_as(&host, "t3", &["resume", "hold"]), invoke)
        .unwrap();
    let out = &by_handle.output;
    assert_eq!(out["resume"], json!({"row_doc_id": second}), "{out}");

    // A budget outside 1..1024 is a bad operand.
    for budget in ["0", "1025", "x"] {
        let out = invoke(json!({"protocol": 1, "argv": ["resume", "hold", "--budget", budget]}));
        assert_eq!(
            out["fail"]["code"], "workflow_invalid_input",
            "{budget}: {out}"
        );
        let message = out["fail"]["message"].as_str().unwrap();
        assert!(message.ends_with("Next: workflow help resume"), "{message}");
    }

    // answer: by the address of the awaiting row, then by the handle.
    let (mut host, call) = (FakeHost::default(), run_call(TWO));
    let first = host.step0(&call, invoke).unwrap().row.unwrap();
    let a0 = host
        .step0(&call_as(&host, "t2", &["answer", "cli:t1", "one"]), invoke)
        .unwrap();
    let out = &a0.output;
    assert_eq!(
        out["resume"],
        json!({"row_doc_id": first, "answer": "one"}),
        "{out}"
    );
    let a1 = host
        .step0(&call_as(&host, "t3", &["answer", "two", "two"]), invoke)
        .unwrap();
    assert_eq!(
        a1.output["result"]["answer"],
        json!({"a": "one", "b": "two"}),
        "{}",
        a1.output
    );
}

#[test]
fn import_and_keep_from_are_not_available() {
    let calls = [
        json!({"argv": ["import", "./pack"]}),
        json!({"argv": ["keep", "h", "--from", "shared:did:peer/x"]}),
        json!({"protocol": 1, "argv": ["import", "./pack"]}),
        json!({"protocol": 1, "argv": ["keep", "h", "--from", "shared:did:peer/x"]}),
    ];
    for input in calls {
        let out = invoke(input.clone());
        assert_eq!(
            out["fail"]["code"], "workflow_not_available",
            "{input}: {out}"
        );
        let message = out["fail"]["message"].as_str().unwrap();
        assert!(message.contains("deferred to v1.1"), "{message}");
        assert!(message.contains("Next: workflow help"), "{message}");
    }
}

#[test]
fn only_run_starts_a_job_on_a_step0_call() {
    let (dir, source) = (scratch("verbs"), script("gate"));
    let mounts = pack_mount(&dir, &source);
    let out = step0(json!({"argv": ["bogus", "gate"], "mounts": mounts}));
    assert_eq!(out["fail"]["code"], "workflow_invalid_input", "{out}");
    assert!(out["effects"]["pin"].is_null(), "{out}");
    let message = out["fail"]["message"].as_str().unwrap();
    assert!(message.contains("Next: workflow help"), "{message}");
}

#[test]
fn help_check_list_and_show_end_inline_on_a_step0_call() {
    let (dir, source) = (scratch("inline"), script("gate"));
    let mounts = pack_mount(&dir, &source);
    for (argv, text) in [
        (json!(["help", "show"]), "show <name>"),
        (json!(["list"]), "gate: "),
        (json!(["show", "gate"]), "tier: pack"),
        (json!(["check", "gate"]), "The smoke check passed for gate"),
    ] {
        let out = step0(json!({"argv": argv, "mounts": mounts}));
        let result = out["result"].as_str().unwrap_or_default();
        assert!(result.contains(text), "{argv}: {out}");
        assert!(out["effects"]["pin"].is_null(), "{argv}: {out}");
    }
}
