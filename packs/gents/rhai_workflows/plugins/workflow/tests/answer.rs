//! `answer <row|handle> <text>` and `resume <row|handle> --engine current` on the fake host:
//! `await_user` reading the `await:<c>` notes, the control precedence the host echoes, and the
//! fail-closed replay of an edited script (L2 section 4.10; root C1, C8).
use serde_json::{Value, json};
use workflow::fakehost::admit::view;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::wire::JobState;

/// An agent first, so the answer row's step 0 spawns and the answer is read on step 1.
const SCAN: &str = r##"let meta = #{ name: "scan", description: "two questions after a scan" };
let r = agent("go");
let a = await_user("user", "first?");
let b = await_user("user", "second?");
complete(#{ a: a, b: b });
"##;

const TWO: &str = r##"let meta = #{ name: "two", description: "two questions in a row" };
let a = await_user("user", "first?");
let b = await_user("user", "second?");
complete(#{ a: a, b: b });
"##;

const EDIT: &str = r##"let meta = #{ name: "edit", description: "an agent, then a pause" };
let r = agent("one");
pause("user", "hold");
complete(r);
"##;

const HOLD: &str = r##"let meta = #{ name: "hold", description: "pauses on every run" };
pause("user", "hold on");
complete(1);
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

/// The `value_json` of the note `id` among an output's `writes`, parsed.
fn written(out: &Value, id: &str) -> Option<Value> {
    let writes = out.pointer("/effects/writes").or(out.get("writes"));
    let one = (writes?.as_array()?.iter()).find(|w| w["fields"]["note_id"] == id)?;
    serde_json::from_str(one["fields"]["value_json"].as_str()?).ok()
}

/// How many `await:` notes the host holds.
fn await_notes(host: &FakeHost) -> usize {
    let rows = host.state.collections.get("WfNote").into_iter().flatten();
    rows.filter(|r| {
        r["note_id"]
            .as_str()
            .is_some_and(|i| i.starts_with("await:"))
    })
    .count()
}

#[test]
fn row6_answer_is_consumed_once() {
    let (mut host, call) = (FakeHost::default(), run_call(SCAN));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.pause_row(&call.host, &row).unwrap();

    // The answer row's step 0 only re-attempts the scan; it reads the answer on step 1.
    let call = call_as(&host, "t2", &["answer", "scan", "ann"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let (resumed, out) = (r0.row.clone().unwrap(), &r0.output);
    assert!(r0.continues, "{out}");
    assert_eq!(
        out["effects"]["resume"],
        json!({"row_doc_id": row, "answer": "ann"})
    );
    assert_eq!(await_notes(&host), 0);

    host.settle(&spawns(out), canned);
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    let out = &s1.output;
    assert_eq!(out["suspend"]["kind"], "await_user", "{out}");
    assert_eq!(out["suspend"]["question"], "second?", "{out}");
    assert_eq!(
        written(out, "await:1"),
        Some(json!({"text": "ann", "row": resumed}))
    );
    assert!(written(out, "await:2").is_none(), "{out}");
    assert_eq!(await_notes(&host), 1);
    assert_eq!(host.state.rows[&resumed].state, JobState::AwaitingUser);

    // A second answer goes to the second await alone: the first keeps its note's text.
    let a1 = host
        .step0(&call_as(&host, "t3", &["answer", "scan", "bob"]), invoke)
        .unwrap();
    let out = &a1.output;
    assert_eq!(
        out["result"]["answer"],
        json!({"a": "ann", "b": "bob"}),
        "{out}"
    );
    assert_eq!(out["result"]["handle"], "scan", "{out}");
    assert!(
        out["result"].to_string().starts_with(r#"{"answer":"#),
        "the answer comes first: {out}"
    );
    assert_eq!(await_notes(&host), 2);
}

#[test]
fn two_consecutive_awaits_complete_after_two_answers() {
    let (mut host, call) = (FakeHost::default(), run_call(TWO));
    let s0 = host.step0(&call, invoke).unwrap();
    let first = s0.row.clone().unwrap();
    assert_eq!(host.state.rows[&first].state, JobState::AwaitingUser);

    // The first answer is consumed, and the second await suspends at step 0 with pin and resume.
    let a0 = host
        .step0(&call_as(&host, "t2", &["answer", "two", "one"]), invoke)
        .unwrap();
    let (second, out) = (a0.row.clone().unwrap(), &a0.output);
    assert_eq!(out["suspend"]["question"], "second?", "{out}");
    assert_eq!(out["resume"], json!({"row_doc_id": first, "answer": "one"}));
    assert_eq!(out["pin"]["handle"], "two");
    assert_eq!(
        written(out, "await:0"),
        Some(json!({"text": "one", "row": second}))
    );
    assert!(!a0.continues && a0.code.is_none(), "{out}");
    assert_eq!(host.state.rows[&second].state, JobState::AwaitingUser);
    assert_eq!(host.jobs.chains.values().next().unwrap().head, second);

    // The second answer, against the new head, completes the run.
    let a1 = host
        .step0(&call_as(&host, "t3", &["answer", "two", "two"]), invoke)
        .unwrap();
    let (third, out) = (a1.row.clone().unwrap(), &a1.output);
    assert_eq!(
        out["result"]["answer"],
        json!({"a": "one", "b": "two"}),
        "{out}"
    );
    assert_eq!(out["result"]["handle"], "two", "{out}");
    assert_eq!(
        out["resume"],
        json!({"row_doc_id": second, "answer": "two"})
    );
    assert_eq!(
        written(out, "await:1"),
        Some(json!({"text": "two", "row": third}))
    );
    assert!(written(out, "await:0").is_none(), "{out}");
    assert_eq!(host.state.rows[&third].state, JobState::Completed);
    assert_eq!(await_notes(&host), 2);
}

#[test]
fn row9_engine_change_fails_closed_and_engine_current_replays() {
    let (mut host, call) = (FakeHost::default(), run_call(EDIT));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(s1.output["suspend"]["kind"], "pause", "{}", s1.output);

    // A replay over an edited script diverges and fails closed; nothing is adopted.
    let rows = host.state.rows.len();
    let scripts = host.state.collections.get_mut("WfScript").unwrap();
    let kept = scripts[0]["source"].clone();
    scripts[0]["source"] = json!(EDIT.replace("\"one\"", "\"uno\""));
    let call = call_as(&host, "t2", &["resume", "edit"]);
    let r0 = host.step0(&call, invoke).unwrap();
    assert_eq!(
        r0.output["fail"]["message"],
        "workflow_validation_failed: replay diverged at call 0. Next: workflow run edit"
    );
    assert!(r0.row.is_none() && host.state.rows.len() == rows);

    // --engine current changes the engine, not the script: the edit still diverges.
    let call = call_as(&host, "t3", &["resume", "edit", "--engine", "current"]);
    let r0 = host.step0(&call, invoke).unwrap();
    assert_eq!(
        r0.output["fail"]["message"],
        "workflow_validation_failed: replay diverged at call 0. Next: workflow run edit"
    );
    assert!(r0.row.is_none() && host.state.rows.len() == rows);

    // The unchanged script replays under --engine current.
    host.state.collections.get_mut("WfScript").unwrap()[0]["source"] = kept;
    let call = call_as(&host, "t4", &["resume", "edit", "--engine", "current"]);
    let r1 = host.step0(&call, invoke).unwrap();
    let (resumed, out) = (r1.row.clone().unwrap(), &r1.output);
    assert_eq!(out["suspend"]["kind"], "pause", "{out}");
    assert_eq!(
        out["resume"],
        json!({"row_doc_id": row, "engine": "current"})
    );
    let control = &host.jobs.rows[&resumed].control;
    assert_eq!(control["kind"], "engine_current");
}

#[test]
fn an_answer_echoed_with_engine_current_is_read_on_a_later_step() {
    let script = r##"let meta = #{ name: "ask", description: "an agent, then one question" };
let r = agent("go");
complete(await_user("user", "first?"));
"##;
    let (mut host, call) = (FakeHost::default(), run_call(script));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.pause_row(&call.host, &row).unwrap();

    let call = call_as(
        &host,
        "t2",
        &["answer", "ask", "ann", "--engine", "current"],
    );
    let r0 = host.step0(&call, invoke).unwrap();
    let resumed = r0.row.clone().unwrap();
    assert!(r0.continues, "{}", r0.output);
    assert_eq!(host.jobs.rows[&resumed].control["kind"], "answer");
    host.settle(&spawns(&r0.output), canned);
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    assert_eq!(s1.output["result"], "ann", "{}", s1.output);
}

#[test]
fn control_precedence_is_honoured() {
    let (mut host, call) = (FakeHost::default(), run_call(HOLD));
    host.step0(&call, invoke).unwrap();
    let cases: [(&[&str], &str, Value); 5] = [
        (
            &["resume", "hold", "--budget", "9"],
            "resume",
            json!({"budget": 9}),
        ),
        (
            &["resume", "hold", "--engine", "current", "--budget", "9"],
            "engine_current",
            json!({"engine": "current", "budget": 9}),
        ),
        (
            &["resume", "hold", "--answer", "go"],
            "answer",
            json!({"answer": "go"}),
        ),
        (
            &[
                "answer", "hold", "go", "--engine", "current", "--budget", "9",
            ],
            "answer",
            json!({"answer": "go", "engine": "current", "budget": 9}),
        ),
        (&["answer", "hold", "go"], "answer", json!({"answer": "go"})),
    ];
    for (i, (argv, kind, rest)) in cases.into_iter().enumerate() {
        let prior = host.jobs.chains.values().next().unwrap().head.clone();
        let step = host
            .step0(&call_as(&host, &format!("t{}", i + 2), argv), invoke)
            .unwrap();
        let row = step.row.clone().unwrap();
        let mut payload = rest;
        payload["row_doc_id"] = json!(prior);
        let control = &host.jobs.rows[&row].control;
        assert_eq!(
            control,
            &json!({"kind": kind, "payload": payload}),
            "{argv:?}"
        );
    }
}
