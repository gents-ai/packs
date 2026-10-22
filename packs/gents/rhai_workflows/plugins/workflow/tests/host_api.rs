//! `parallel()` (L2 sections 4.6 and 4.7): one call index, admission windows, results in input
//! order, and effects that only grow.
use serde_json::{Value, json};
use std::collections::BTreeSet;
use workflow::fakehost::adopt::Stepped;
use workflow::fakehost::cli::facts;
use workflow::fakehost::faults::{deterministic, narrowed};
use workflow::fakehost::oracle::{Labels, canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;

const PANEL: &str = r##"let meta = #{ name: "panel", description: "a panel of five" };
let items = [];
for i in 0..5 { items.push(#{ prompt: "p" + i, label: "l" + i }); }
let rs = parallel(items);
let out = [];
for r in rs { out.push(if type_of(r) == "()" { "-" } else { r.output }); }
complete(#{ out: out });
"##;

fn call(script: &str, max_live: u64) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script}),
        host: narrowed(&facts("/work"), json!({"max_live": max_live})),
    }
}

fn keys(out: &Value) -> BTreeSet<(String, u64)> {
    let key = |s: &Value| {
        (
            s["slot"].as_str().unwrap().to_string(),
            s["attempt"].as_u64().unwrap(),
        )
    };
    spawns(out).iter().map(key).collect()
}

fn live(host: &FakeHost) -> usize {
    (host.state.children.values())
        .filter(|c| c.state == "running")
        .count()
}

/// Runs the panel to its end, settling every spawn canned between steps; each step comes back
/// with the live and total child counts the host held after it.
fn run(host: &mut FakeHost, call: &Call, crash: bool) -> Vec<(Stepped, usize, usize)> {
    let first = host.step0(call, invoke).unwrap();
    let row = first.row.clone().unwrap();
    let mut steps = vec![(first, live(host), host.state.children.len())];
    while steps.last().unwrap().0.continues {
        host.settle(&spawns(&steps.last().unwrap().0.output), canned);
        if crash {
            host.crash();
        }
        let next = host.step(call, &row, invoke).unwrap();
        steps.push((next, live(host), host.state.children.len()));
    }
    steps
}

#[test]
fn row2_failing_slot_yields_unit_in_input_order() {
    let script = r##"let meta = #{ name: "row2", description: "one slot fails" };
let rs = parallel([#{ prompt: "a", label: "a" }, #{ prompt: "b", label: "b" }, #{ prompt: "c", label: "c" }]);
let out = [];
for r in rs { out.push(if type_of(r) == "()" { "-" } else { r.output }); }
let tail = agent("tail");
complete(#{ out: out, tail: tail.output });
"##;
    let (mut host, call) = (FakeHost::default(), call(script, 64));
    let labels = Labels::from_json(&json!({"b": {"state": "failed", "text": "no"}})).unwrap();
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let slots: Vec<String> = (spawns(&s0.output).iter())
        .map(|s| s["slot"].as_str().unwrap()[..3].to_string())
        .collect();
    assert_eq!(slots, ["0.0", "0.1", "0.2"], "one call index, items c.i");

    host.settle(&spawns(&s0.output), |s| labels.answer(s));
    let s1 = host.step(&call, &row, invoke).unwrap();
    let tail = spawns(&s1.output);
    assert!(
        tail.len() == 1 && tail[0]["slot"].as_str().unwrap().starts_with("1:"),
        "{}",
        s1.output
    );

    host.settle(&tail, canned);
    let s2 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(
        s2.output["result"],
        json!({"out": ["ok", "-", "ok"], "tail": "ok"})
    );
}

#[test]
fn a_panel_with_no_room_and_nothing_live_is_refused_not_stalled() {
    let mut absent = call(PANEL, 0);
    absent
        .host
        .ceiling
        .as_object_mut()
        .unwrap()
        .remove("max_live");
    for call in [call(PANEL, 0), absent] {
        let s0 = FakeHost::default().step0(&call, invoke).unwrap();
        assert!(
            spawns(&s0.output).is_empty() && !s0.continues,
            "{}",
            s0.output
        );
        assert_eq!(s0.output["fail"]["code"], "workflow_launch_failed");
        let msg = s0.output["fail"]["message"].as_str().unwrap();
        assert!(
            msg.contains("max_live") && msg.ends_with("Next: workflow status cli:t1"),
            "{msg}"
        );
    }
}

#[test]
fn an_agent_with_no_room_is_refused_not_spawned() {
    let one =
        "let meta = #{ name: \"one\", description: \"one agent\" };\nagent(\"a\");\ncomplete(1);\n";
    let s0 = FakeHost::default().step0(&call(one, 0), invoke).unwrap();
    assert!(spawns(&s0.output).is_empty(), "{}", s0.output);
    assert_eq!(s0.output["fail"]["code"], "workflow_launch_failed");
    let msg = s0.output["fail"]["message"].as_str().unwrap();
    assert!(
        msg.contains("max_live") && msg.ends_with("Next: workflow status cli:t1"),
        "{msg}"
    );
}

#[test]
fn a_part_settled_panel_with_no_room_and_nothing_live_is_refused() {
    let mut host = FakeHost::default();
    let s0 = host.step0(&call(PANEL, 2), invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call(PANEL, 0), &row, invoke).unwrap();
    assert!(
        spawns(&s1.output).is_empty() && !s1.continues,
        "{}",
        s1.output
    );
    assert_eq!(s1.output["fail"]["code"], "workflow_launch_failed");
}

#[test]
fn spawns_respect_max_live_and_other_live_children() {
    let (mut host, call) = (FakeHost::default(), call(PANEL, 2));
    let steps = run(&mut host, &call, false);
    let counts: Vec<(usize, usize)> = steps.iter().map(|(_, live, all)| (*live, *all)).collect();
    assert_eq!(counts, [(2, 2), (2, 4), (1, 5), (0, 5)]);
    assert!(steps.iter().all(|(s, ..)| s.code.is_none()));
    let done = &steps.last().unwrap().0.output;
    assert_eq!(
        done["result"],
        json!({"out": ["ok", "ok", "ok", "ok", "ok"]})
    );

    // Another live child of the chain takes a share of max_live.
    let other = json!({"attempt": 0, "session_id": "s", "request_id": "q", "title": "other",
        "state": "running", "output_bytes": 0, "truncated": false,
        "usage": {"input": 0, "output": 0, "total": 0}, "duration_ms": 0});
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"], "script": PANEL,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "ceiling": {"targets": ["wf-read-only"], "max_live": 2, "remaining_spawns": 64,
                    "dynamic": "run", "dynamic_targets": ["wf-read-only"]},
        "children": {"9:aa": other}
    }));
    assert_eq!(spawns(&out).len(), 1, "{out}");
}

#[test]
fn re_emitted_windows_are_supersets() {
    let script = r##"let meta = #{ name: "wide", description: "three in a panel" };
let rs = parallel([#{ prompt: "a" }, #{ prompt: "b" }, #{ prompt: "c" }]);
complete(#{ n: rs.len() });
"##;
    let (mut host, call) = (FakeHost::default(), call(script, 2));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert_eq!(keys(&s0.output).len(), 2);

    // Only item 0 ends: the window grows by one and keeps the live item.
    let first = spawns(&s0.output)[0].clone();
    host.settle(&[first], canned);
    let [s1, s1_again] = host.duplicate_step(&call, &row, invoke).unwrap();
    assert!(
        keys(&s0.output).is_subset(&keys(&s1.output)),
        "{}",
        s1.output
    );
    assert_eq!(keys(&s1.output).len(), 3);
    assert_eq!(keys(&s1.output), keys(&s1_again.output));
    assert_eq!(
        host.state.children.len(),
        3,
        "the duplicate spawned nothing new"
    );
}

#[test]
fn no_output_breaks_the_stall_rule() {
    let trace = deterministic(|host, crash| {
        let steps = run(host, &call(PANEL, 2), crash);
        // The fake host ends a row with plugin_job_stalled on any stalled output.
        assert!(steps.iter().all(|(s, ..)| s.code.is_none()));
        steps.into_iter().map(|(s, ..)| s.output).collect()
    })
    .unwrap();
    assert_eq!(trace.len(), 4);
}

#[test]
fn non_map_items_are_invalid_input() {
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"],
        "script": r##"let meta = #{ name: "bad", description: "items must be maps" };
try { parallel([1]); } catch (e) { complete(e); }
"##,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "ceiling": {"targets": ["wf-read-only"], "max_live": 2, "dynamic": "run", "dynamic_targets": ["wf-read-only"]}
    }));
    let thrown = out["result"].as_str().unwrap_or_else(|| panic!("{out}"));
    assert!(thrown.starts_with("workflow_invalid_input: "), "{thrown}");
}
