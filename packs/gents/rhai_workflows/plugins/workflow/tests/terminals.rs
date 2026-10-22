//! `budget()`, BudgetExceeded, `pause`, `await_user` and the terminals that carry their
//! effects (L2 sections 4.6 and 4.7), on the fake host.
use serde_json::{Value, json};
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::wire::JobState;

fn call(script: &str, budget: u64) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script,
                      "agent_budget": budget}),
        host: facts("/work"),
    }
}

/// The note ids among an output's `writes`, wherever the output keeps them.
fn notes(out: &Value) -> Vec<String> {
    let fx = out.get("effects").unwrap_or(out);
    (fx["writes"].as_array().into_iter().flatten())
        .filter(|w| w["collection"] == "WfNote")
        .map(|w| w["fields"]["note_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn row3_crossing_panel_admits_none_and_pauses() {
    let script = r##"let meta = #{ name: "row3", description: "a panel past the budget" };
let first = agent("one");
let panel = parallel([#{ prompt: "a" }, #{ prompt: "b" }, #{ prompt: "c" }]);
complete(#{ n: panel.len() });
"##;
    let (mut host, call) = (FakeHost::default(), call(script, 2));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);

    let s1 = host.step(&call, &row, invoke).unwrap();
    let out = &s1.output;
    assert!(spawns(out).is_empty() && !s1.continues, "{out}");
    assert!(s1.code.is_none(), "{out}");
    assert_eq!(host.state.children.len(), 1, "the panel spawned nothing");
    assert_eq!(
        out["suspend"],
        json!({"kind": "pause", "reason": "budget_exceeded",
               "message": "1/2. Next: workflow resume cli:t1 --budget <n>"})
    );
    assert_eq!(out["progress"]["state"], "paused");
    assert_eq!(out["progress"]["pause_reason"], "budget_exceeded");
    assert_eq!(host.state.rows[&row].state, JobState::Paused);
}

#[test]
fn row21_step0_budget_pause_is_pinned_and_inline() {
    let script = r##"let meta = #{ name: "gate", description: "pauses on a low budget" };
if budget().total < 22 { pause("budget_exceeded", "needs a budget of 22"); }
let r = agent("work");
complete(r.output);
"##;
    let (mut host, call) = (FakeHost::default(), call(script, 5));
    let s0 = host.step0(&call, invoke).unwrap();
    let out = &s0.output;
    let row = s0.row.clone().expect("a pinned suspend adopts a row");
    assert!(!s0.continues && s0.code.is_none(), "{out}");
    assert!(spawns(out).is_empty(), "{out}");
    assert_eq!(out["pin"]["handle"], "gate", "{out}");
    assert!(notes(out).iter().any(|n| n.starts_with("run:")), "{out}");
    assert_eq!(out["suspend"]["kind"], "pause");
    assert_eq!(out["suspend"]["reason"], "budget_exceeded");
    let message = out["suspend"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("needs a budget of 22") && message.contains("cli:t1"),
        "answer first, then the run's address: {message}"
    );
    assert!(
        message.contains("gate"),
        "and the proposed handle: {message}"
    );
    assert_eq!(host.state.rows[&row].state, JobState::Paused);
    assert!(host.state.rows[&row].pin.is_some());
}

#[test]
fn step0_budget_message_keeps_the_pinned_text_and_tags_after_it() {
    let script = r##"let meta = #{ name: "same", description: "an agent past the budget" };
parallel([#{ prompt: "x" }, #{ prompt: "y" }]);
"##;
    let s0 = FakeHost::default().step0(&call(script, 1), invoke).unwrap();
    assert_eq!(
        s0.output["suspend"]["message"],
        "0/1. Next: workflow resume cli:t1 --budget <n> (run cli:t1, handle same)"
    );
}

#[test]
fn budget_exceeded_and_the_script_kind_give_the_same_suspend() {
    let past = r##"let meta = #{ name: "same", description: "an agent past the budget" };
parallel([#{ prompt: "x" }, #{ prompt: "y" }]);
"##;
    let said = r##"let meta = #{ name: "same", description: "an agent past the budget" };
pause("budget_exceeded", "0/1");
"##;
    let step0 = |script: &str| FakeHost::default().step0(&call(script, 1), invoke).unwrap();
    let (a, b) = (step0(past).output, step0(said).output);
    assert_eq!(a["suspend"]["kind"], b["suspend"]["kind"]);
    assert_eq!(a["suspend"]["reason"], b["suspend"]["reason"]);
    assert_eq!(a["progress"]["pause_reason"], "budget_exceeded");
    assert_eq!(a["progress"]["pause_reason"], b["progress"]["pause_reason"]);
    assert!(a["pin"].is_object() && b["pin"].is_object());
}

#[test]
fn budget_reads_the_current_pin_and_writes_no_note() {
    let script = r##"let meta = #{ name: "peek", description: "reads the budget twice" };
let before = budget();
agent("spend one");
let after = budget();
complete(#{ before: before, after: after });
"##;
    let (mut host, call) = (FakeHost::default(), call(script, 7));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(
        s1.output["result"],
        json!({"before": {"total": 7, "spent": 0, "reserved": 0, "remaining": 7},
               "after": {"total": 7, "spent": 1, "reserved": 0, "remaining": 6}}),
        "{}",
        s1.output
    );
    let all: Vec<String> = [&s0.output, &s1.output]
        .iter()
        .flat_map(|o| notes(o))
        .collect();
    assert_eq!(all, ["run:row-t1"], "budget() journals nothing: {all:?}");
}

#[test]
fn pinless_result_or_fail_carries_no_wfnote_write() {
    let done = r##"let meta = #{ name: "quick", description: "ends at once" };
complete(1);
"##;
    let ends = r##"let meta = #{ name: "lost", description: "forgets to complete" };
let b = budget();
"##;
    for script in [done, ends] {
        let mut host = FakeHost::default();
        let s0 = host.step0(&call(script, 4), invoke).unwrap();
        assert!(s0.row.is_none() && !s0.continues, "{}", s0.output);
        assert!(s0.output.get("pin").is_none(), "{}", s0.output);
        assert!(notes(&s0.output).is_empty(), "{}", s0.output);
        assert!(!host.state.collections.contains_key("WfNote"));
    }
}

#[test]
fn await_user_suspends_with_the_question() {
    let script = r##"let meta = #{ name: "ask", description: "asks the user" };
let a = await_user("user", "which branch?");
complete(a);
"##;
    let mut host = FakeHost::default();
    let s0 = host.step0(&call(script, 4), invoke).unwrap();
    let out = &s0.output;
    let row = s0.row.clone().expect("a pinned suspend adopts a row");
    assert_eq!(out["suspend"]["kind"], "await_user", "{out}");
    assert_eq!(out["suspend"]["reason"], "user");
    assert_eq!(out["suspend"]["question"], "which branch?");
    assert!(
        out["suspend"]["message"]
            .as_str()
            .unwrap()
            .starts_with("which branch?")
    );
    assert_eq!(out["progress"]["state"], "awaiting_user");
    assert_eq!(out["progress"]["question"], "which branch?");
    assert_eq!(host.state.rows[&row].state, JobState::AwaitingUser);
}

#[test]
fn pause_refuses_an_unknown_kind_catchably() {
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"],
        "script": r##"let meta = #{ name: "bad", description: "pauses with a made-up kind" };
try { pause("sleepy", "zzz"); } catch (e) { complete(e); }
"##,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""}
    }));
    let thrown = out["result"].as_str().unwrap_or_else(|| panic!("{out}"));
    assert!(thrown.starts_with("workflow_invalid_input: "), "{thrown}");
}
