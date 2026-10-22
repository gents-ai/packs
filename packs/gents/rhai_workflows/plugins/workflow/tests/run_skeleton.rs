//! `run` on an inline script, from the step-0 pin to `complete`, on the fake host.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use workflow::fakehost::cli::facts;
use workflow::fakehost::faults::deterministic;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::wire::JobState;

const ONE: &str = r##"let meta = #{ name: "demo", description: "ask once" };
let r = agent("say hi");
complete(#{ said: r.output, ok: r.success });
"##;

const TWO: &str = r##"let meta = #{ name: "pair", description: "two agents in a row" };
let a = agent("first");
let b = agent("second after " + a.output);
complete(#{ a: a.output, b: b.output });
"##;

fn call(script: &str) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script}),
        host: facts("/work"),
    }
}

fn hex(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    parts.iter().for_each(|p| hash.update(p.as_bytes()));
    format!("{:x}", hash.finalize())
}

#[test]
fn row1_one_agent_then_complete() {
    let (mut host, call) = (FakeHost::default(), call(ONE));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert!(s0.continues, "{}", s0.output);
    assert_eq!(host.state.rows[&row].state, JobState::Running);
    let fx = &s0.output["effects"];
    assert_eq!(fx["spawn"].as_array().unwrap().len(), 1, "{fx}");
    assert_eq!(fx["spawn"][0]["target"], "wf-read-only");
    assert_eq!(fx["spawn"][0]["prompt"], "say hi");
    assert!(fx["pin"].is_object() && fx["progress"].is_object());
    assert_eq!(host.state.collections["WfScript"].len(), 1);

    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(s1.output["result"], json!({"said": "ok", "ok": true}));
    assert!(!s1.continues && s1.code.is_none(), "{}", s1.output);
    assert_eq!(host.state.rows[&row].state, JobState::Completed);
}

#[test]
fn script_id_and_handle_follow_the_pin_rules() {
    let input = |jobs: Value, budget: Value| {
        let mut input = json!({
            "protocol": 1, "argv": ["run", "inline"], "script": ONE, "jobs": jobs,
            "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
            "ceiling": {"targets": ["wf-read-only"], "max_live": 4, "remaining_spawns": 9, "dynamic": "run", "dynamic_targets": ["wf-read-only"]}
        });
        if !budget.is_null() {
            input["agent_budget"] = budget;
        }
        invoke(input)
    };

    let out = input(json!([]), Value::Null);
    let pin = out["effects"]["pin"].as_object().unwrap();
    assert_eq!(pin["script_id"], hex(&["gents-rhai/1", ONE]).as_str());
    assert_eq!(
        pin["source_digest"],
        format!("sha256:{}", hex(&[ONE])).as_str()
    );
    assert_eq!(
        (pin["agent_budget"].clone(), pin["handle"].clone()),
        (json!(128), json!("demo"))
    );
    assert_eq!(pin.len(), 4, "the guest supplies four pin fields: {pin:?}");

    let jobs = json!([{"handle": "other-7"}, {"handle": "demo"}, {"handle": "demo-2"}]);
    let out = input(jobs, json!(7));
    assert_eq!(out["effects"]["pin"]["handle"], "demo-3");
    assert_eq!(out["effects"]["pin"]["agent_budget"], 7);

    // The run note records the pin facts under the adopted row.
    let notes: Vec<_> = (out["effects"]["writes"].as_array().unwrap().iter())
        .filter(|w| w["collection"] == "WfNote")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["fields"]["note_id"], "run:r1");
    let run: Value =
        serde_json::from_str(notes[0]["fields"]["value_json"].as_str().unwrap()).unwrap();
    assert_eq!(run["script_id"], hex(&["gents-rhai/1", ONE]).as_str());
    assert_eq!(
        (run["handle"].clone(), run["agent_budget"].clone()),
        (json!("demo-3"), json!(7))
    );
}

#[test]
fn duplicated_step_spawns_nothing_new() {
    let (mut host, call) = (FakeHost::default(), call(TWO));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);

    let [first, second] = host.duplicate_step(&call, &row, invoke).unwrap();
    assert_eq!(spawns(&first.output).len(), 1);
    assert_eq!(spawns(&first.output), spawns(&second.output));
    assert_eq!(host.state.children.len(), 2, "first and second, no third");
    assert!(second.continues);
}

#[test]
fn traces_identical_across_duplicates_and_crashes() {
    let trace = deterministic(|host, crash| {
        let call = call(TWO);
        let s0 = host.step0(&call, invoke).unwrap();
        let row = s0.row.clone().unwrap();
        host.settle(&spawns(&s0.output), canned);
        if crash {
            host.crash();
        }
        let [s1, _] = host.duplicate_step(&call, &row, invoke).unwrap();
        host.settle(&spawns(&s1.output), canned);
        if crash {
            host.crash();
        }
        let s2 = host.step(&call, &row, invoke).unwrap();
        vec![s0.output, s1.output, s2.output]
    })
    .unwrap();
    assert_eq!(trace[2]["result"], json!({"a": "ok", "b": "ok"}));
}

#[test]
fn suspension_set_in_a_closure_suspends_the_step() {
    let script = r##"let meta = #{ name: "closure", description: "agent inside a closure" };
try { [1].map(|x| agent("p")); } catch (e) { }"##;
    let s0 = FakeHost::default().step0(&call(script), invoke).unwrap();
    let fx = &s0.output["effects"];
    assert!(s0.continues, "{}", s0.output);
    assert_eq!(fx["spawn"].as_array().unwrap().len(), 1, "{fx}");
    assert_eq!(fx["spawn"][0]["prompt"], "p");
}

#[test]
fn undecodable_children_refuse_instead_of_replaying_empty() {
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"], "script": ONE,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "ceiling": {"targets": ["wf-read-only"]}, "children": {"s": {"attempt": 1}}
    }));
    assert_eq!(out["fail"]["code"], "workflow_invalid_input", "{out}");
    let message = out["fail"]["message"].as_str().unwrap();
    assert!(message.contains("children do not decode"), "{message}");
}

#[test]
fn an_empty_script_or_path_names_nothing() {
    let input = |script: Option<&str>, path: Option<&str>| {
        let mut input = call(ONE).input;
        input.as_object_mut().unwrap().remove("script");
        for (key, value) in [("script", script), ("path", path)] {
            input[key] = value.map_or(Value::Null, |v| json!(v));
        }
        invoke(input)
    };
    // An empty path leaves the inline script alone; empty script and path are no script at all.
    assert_eq!(input(Some(ONE), Some("")), input(Some(ONE), None));
    assert_eq!(input(Some(""), Some("")), input(None, None));
}

#[test]
fn a_budget_flag_in_argv_sets_the_agent_budget() {
    let run = |argv: Value| {
        let mut input = call(ONE).input;
        input["argv"] = argv;
        input["job"] =
            json!({"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""});
        input["ceiling"] = json!({"targets": ["wf-read-only"], "max_live": 4, "remaining_spawns": 9, "dynamic": "run", "dynamic_targets": ["wf-read-only"]});
        invoke(input)
    };
    let out = run(json!(["run", "inline", "--budget", "1"]));
    assert_eq!(out["effects"]["pin"]["agent_budget"], 1, "{out}");
    let bad = run(json!(["run", "inline", "--budget", "0"]));
    assert_eq!(bad["fail"]["code"], "workflow_invalid_input", "{bad}");
    let none = run(json!(["run", "inline"]));
    assert_eq!(none["effects"]["pin"]["agent_budget"], 128, "{none}");
}
