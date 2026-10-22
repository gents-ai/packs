//! `resume <row|handle>` on the fake host: step 0 with the run note, re-attempts of interrupted
//! slots, and the `root_call` check of the steps after it (L2 section 4.10; root C1, C5.3, C8).
use serde_json::{Value, json};
use workflow::fakehost::admit::view;
use workflow::fakehost::adopt::Stepped;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{Answer, canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::wire::JobState;

const PAIR: &str = r##"let meta = #{ name: "pair", description: "two agents in parallel" };
let r = parallel([#{ prompt: "left" }, #{ prompt: "right" }]);
complete(#{ q: args.q, n: r.len() });
"##;

const HOLD: &str = r##"let meta = #{ name: "gate", description: "pauses on every run" };
pause("user", "hold on");
complete(1);
"##;

const PANEL: &str = r##"let meta = #{ name: "panel", description: "one agent, then a panel of three" };
agent("first");
let r = parallel([#{ prompt: "a" }, #{ prompt: "b" }, #{ prompt: "c" }]);
complete(r.len());
"##;

const GATE: &str = r##"let meta = #{ name: "gate", description: "needs a budget of 22" };
if budget().total < 22 { pause("budget_exceeded", "needs a budget of 22"); }
complete("done");
"##;

fn run_call(script: &str, args: Value) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script, "args": args}),
        host: facts("/work"),
    }
}

/// A second tool call resuming: `jobs` and the chain's `children` as the host lists them.
fn resume_call(host: &FakeHost, argv: &[&str]) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!("t2");
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

/// The value of the stored note `id`, parsed.
fn stored(host: &FakeHost, id: &str) -> Option<Value> {
    let rows = host.state.collections.get("WfNote")?;
    let row = rows.iter().find(|r| r["note_id"] == id)?;
    serde_json::from_str(row["value_json"].as_str()?).ok()
}

/// The `value_json` of the note `id` among an output's `writes`.
fn written(out: &Value, id: &str) -> Option<String> {
    let writes = out.pointer("/effects/writes").or(out.get("writes"));
    (writes?.as_array()?.iter())
        .find(|w| w["fields"]["note_id"] == id)
        .and_then(|w| w["fields"]["value_json"].as_str().map(String::from))
}

fn slot_of(out: &Value, prompt: &str) -> String {
    let all = spawns(out);
    let spawn = all.iter().find(|s| s["prompt"] == prompt);
    spawn.expect("a spawn")["slot"].as_str().unwrap().into()
}

fn attempts(out: &Value) -> Vec<u64> {
    spawns(out)
        .iter()
        .map(|s| s["attempt"].as_u64().unwrap())
        .collect()
}

#[test]
fn rows5_and_20_keep_handle_and_chain_root_args() {
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let pin = s0.output["effects"]["pin"].clone();

    // Row 5: a pause kills the live children; resume continues the same run.
    host.pause_row(&call.host, &row).unwrap();
    assert_eq!(host.state.rows[&row].state, JobState::Paused);
    let call = resume_call(&host, &["resume", "pair"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let (fx, resumed) = (&r0.output["effects"], r0.row.clone().unwrap());
    assert!(r0.continues, "{}", r0.output);
    assert_eq!(fx["resume"], json!({"row_doc_id": row}));
    for field in ["script_id", "source_digest", "handle", "agent_budget"] {
        assert_eq!(fx["pin"][field], pin[field], "{field}: {fx}");
    }
    assert_eq!(attempts(&r0.output), [1, 1], "{fx}");

    host.settle(&spawns(&r0.output), canned);
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    assert_eq!(
        s1.output["result"],
        json!({"q": "x", "n": 2}),
        "{}",
        s1.output
    );
    assert_eq!(host.state.rows[&resumed].state, JobState::Completed);

    // Row 20: one handle and one chain root; the new row's run note copies the root's args.
    let chain = host.jobs.chains.values().next().unwrap();
    assert_eq!(chain.rows, [row.clone(), resumed.clone()]);
    assert_eq!(host.state.rows[&resumed].handle, "pair");
    let note = stored(&host, &format!("run:{resumed}")).expect("the resumed row's run note");
    assert_eq!(note["args"], json!({"q": "x"}));
    assert_eq!(note["handle"], "pair");
}

#[test]
fn interrupted_terminal_slots_reattempt_once_live_ones_wait() {
    // A terminal interrupted slot gets attempt 1, the completed one is not re-run.
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let (left, right) = (slot_of(&s0.output, "left"), slot_of(&s0.output, "right"));
    host.interrupt_child(&left, 0).unwrap();
    host.answer_child(&right, 0, Answer::ok("ok")).unwrap();
    host.pause_row(&call.host, &row).unwrap();

    let call = resume_call(&host, &["resume", &row]);
    let r0 = host.step0(&call, invoke).unwrap();
    let resumed = r0.row.clone().unwrap();
    let all = spawns(&r0.output);
    let attempt = |slot: &str| all.iter().find(|s| s["slot"] == slot).unwrap()["attempt"].clone();
    assert_eq!((attempt(&left), attempt(&right)), (json!(1), json!(0)));

    // Interrupted again, it is not re-attempted a second time: the panel ends with a gap.
    host.interrupt_child(&left, 1).unwrap();
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    assert_eq!(
        s1.output["result"],
        json!({"q": "x", "n": 2}),
        "{}",
        s1.output
    );
    assert!(!host.state.children.contains_key(&(left, 3)));

    // A slot whose child is still live is waited for: same attempt, no re-attempt note entry.
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    host.state
        .rows
        .get_mut(s0.row.as_ref().unwrap())
        .unwrap()
        .state = JobState::Paused;
    let r0 = host
        .step0(&resume_call(&host, &["resume", "pair"]), invoke)
        .unwrap();
    assert_eq!(attempts(&r0.output), [0, 0], "{}", r0.output);
    assert_eq!(
        written(&r0.output, "reattempt:row-t2").as_deref(),
        Some("[]")
    );
}

#[test]
fn reattempt_note_keeps_later_steps_consistent() {
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let (left, right) = (slot_of(&s0.output, "left"), slot_of(&s0.output, "right"));
    host.pause_row(&call.host, &row).unwrap();
    let call = resume_call(&host, &["resume", "pair"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let resumed = r0.row.clone().unwrap();
    let listed = json!([[left, 0], [right, 0]]).to_string();
    assert_eq!(written(&r0.output, "reattempt:row-t2"), Some(listed));
    assert_eq!(host.state.children.len(), 4);

    // The same step twice, and again after a crash, asks for attempt 1 and nothing newer.
    let [first, second] = host.duplicate_step(&call, &resumed, invoke).unwrap();
    host.crash();
    let third = host.step(&call, &resumed, invoke).unwrap();
    for step in [&first, &second, &third] {
        assert_eq!(attempts(&step.output), [1, 1], "{}", step.output);
    }
    assert_eq!(host.state.children.len(), 4);
}

const ONE: &str = r##"let meta = #{ name: "one", description: "one agent, then its spent count" };
agent("work");
complete(#{ spent: budget().spent });
"##;

/// Runs `ONE` under `budget`, interrupts its child, pauses the row and resumes it.
fn reattempted(budget: u64) -> (FakeHost, Call, Stepped) {
    let (mut host, mut call) = (FakeHost::default(), run_call(ONE, json!({})));
    call.input["agent_budget"] = json!(budget);
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.interrupt_child(&slot_of(&s0.output, "work"), 0)
        .unwrap();
    host.pause_row(&call.host, &row).unwrap();
    let call = resume_call(&host, &["resume", "one"]);
    let r0 = host.step0(&call, invoke).unwrap();
    (host, call, r0)
}

#[test]
fn a_reattempt_is_charged_to_the_budget() {
    let (mut host, call, r0) = reattempted(5);
    assert_eq!(attempts(&r0.output), [1], "{}", r0.output);
    host.settle(&spawns(&r0.output), canned);
    let s1 = host.step(&call, r0.row.as_ref().unwrap(), invoke).unwrap();
    assert_eq!(s1.output["result"], json!({"spent": 2}), "{}", s1.output);

    // Attempt 2 is a second intent: it does not fit a budget of one.
    let (_, _, r0) = reattempted(1);
    let suspend = &r0.output["suspend"];
    assert_eq!(suspend["reason"], "budget_exceeded", "{}", r0.output);
    assert!(
        suspend["message"]
            .as_str()
            .unwrap()
            .starts_with("0/1. Next:"),
        "{suspend}"
    );
}

#[test]
fn root_call_mismatch_fails_closed() {
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.pause_row(&call.host, &row).unwrap();
    let call = resume_call(&host, &["resume", "pair"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let resumed = r0.row.clone().unwrap();
    host.settle(&spawns(&r0.output), canned);

    let other = json!({"argv": ["run", "inline"], "args": {"q": "other"}});
    host.jobs.rows.get_mut(&resumed).unwrap().root_call = Some(other);
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    let message = s1.output["fail"]["message"].as_str().expect("a fail");
    assert_eq!(
        message,
        "workflow_validation_failed: resume arguments differ from the chain root. \
         Next: workflow run pair"
    );
    assert!(s1.output.get("result").is_none() && spawns(&s1.output).is_empty());
    assert_eq!(host.state.rows[&resumed].state, JobState::Failed);
}

#[test]
fn resumed_row_without_root_call_fails_closed() {
    let (mut host, call) = (FakeHost::default(), run_call(PAIR, json!({"q": "x"})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.pause_row(&call.host, &row).unwrap();
    let call = resume_call(&host, &["resume", "pair"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let resumed = r0.row.clone().unwrap();
    host.settle(&spawns(&r0.output), canned);

    host.jobs.rows.get_mut(&resumed).unwrap().root_call = None;
    let s1 = host.step(&call, &resumed, invoke).unwrap();
    let message = s1.output["fail"]["message"].as_str().expect("a fail");
    assert_eq!(
        message,
        "workflow_validation_failed: resume arguments differ from the chain root. \
         Next: workflow run pair"
    );
    assert!(s1.output.get("result").is_none() && spawns(&s1.output).is_empty());
}

#[test]
fn resume_ending_at_step0_returns_pin_and_resume() {
    // A pause that re-fires: the suspend carries pin and resume, and the new row, already
    // terminal, is the chain head.
    let (mut host, call) = (FakeHost::default(), run_call(HOLD, json!({})));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let r0 = host
        .step0(&resume_call(&host, &["resume", "gate"]), invoke)
        .unwrap();
    let (out, resumed) = (&r0.output, r0.row.clone().unwrap());
    assert_eq!(out["suspend"]["kind"], "pause", "{out}");
    assert_eq!(out["resume"], json!({"row_doc_id": row}));
    assert_eq!(out["pin"]["handle"], "gate");
    assert!(written(out, &format!("run:{resumed}")).is_some(), "{out}");
    assert!(
        written(out, &format!("reattempt:{resumed}")).is_some(),
        "{out}"
    );
    assert!(!r0.continues && r0.code.is_none(), "{out}");
    assert_eq!(host.state.rows[&resumed].state, JobState::Paused);
    assert_eq!(host.jobs.chains.values().next().unwrap().head, resumed);
}

#[test]
fn row21_raised_budget_lifts_the_scripts_gate() {
    let mut call = run_call(GATE, json!({}));
    call.input["agent_budget"] = json!(5);
    let mut host = FakeHost::default();
    let s0 = host.step0(&call, invoke).unwrap();
    assert_eq!(
        s0.output["suspend"]["reason"], "budget_exceeded",
        "{}",
        s0.output
    );

    let call = resume_call(&host, &["resume", "gate", "--budget", "30"]);
    let r0 = host.step0(&call, invoke).unwrap();
    let out = &r0.output;
    assert_eq!(out["result"], "done (run cli:t2, handle gate)", "{out}");
    assert_eq!(out["pin"]["agent_budget"], 30);
    assert_eq!(
        out["resume"],
        json!({"row_doc_id": s0.row.unwrap(), "budget": 30})
    );
    assert_eq!(host.state.rows[&r0.row.unwrap()].state, JobState::Completed);
}

#[test]
fn an_agent_budget_beside_resume_raises_the_budget_like_the_flag() {
    let mut call = run_call(GATE, json!({}));
    call.input["agent_budget"] = json!(5);
    let mut host = FakeHost::default();
    let s0 = host.step0(&call, invoke).unwrap();

    let mut call = resume_call(&host, &["resume", "gate"]);
    call.input["agent_budget"] = json!(30);
    let r0 = host.step0(&call, invoke).unwrap();
    let out = &r0.output;
    assert_eq!(out["pin"]["agent_budget"], 30, "{out}");
    assert_eq!(
        out["resume"],
        json!({"row_doc_id": s0.row.unwrap(), "budget": 30})
    );
}

#[test]
fn a_prior_row_in_no_resumable_state_is_not_applicable() {
    for state in [Some("bogus"), None] {
        let mut job = json!({"row_doc_id": "r1", "tool_call_id": "t1", "handle": "gate"});
        if let Some(state) = state {
            job["state"] = json!(state);
        }
        let out = invoke(json!({"protocol": 1, "argv": ["resume", "gate"], "jobs": [job]}));
        assert_eq!(
            out["fail"]["code"], "workflow_control_not_applicable",
            "{out}"
        );
        let msg = out["fail"]["message"].as_str().unwrap();
        assert!(msg.ends_with("Next: workflow status gate"), "{msg}");
    }
}

#[test]
fn an_unknown_run_names_a_status_call_that_takes_its_operand() {
    let out = invoke(json!({"protocol": 1, "argv": ["resume", "abc"], "jobs": []}));
    assert_eq!(out["fail"]["code"], "workflow_control_unknown_run", "{out}");
    let msg = out["fail"]["message"].as_str().unwrap();
    assert!(
        msg.ends_with("Next: workflow status <key> with the address from the run's receipt"),
        "{msg}"
    );
}

#[test]
fn row11_resume_started_by_job_is_depth_exceeded() {
    let (mut host, call) = (FakeHost::default(), run_call(HOLD, json!({})));
    host.step0(&call, invoke).unwrap();
    let mut call = resume_call(&host, &["resume", "gate"]);
    call.host.caller["started_by_job"] = json!(true);
    let r0 = host.step0(&call, invoke).unwrap();
    assert!(r0.row.is_none() && !r0.continues, "{}", r0.output);
    assert_eq!(r0.output["fail"]["code"], "workflow_depth_exceeded");
    assert_eq!(host.jobs.chains.values().next().unwrap().rows.len(), 1);
}

#[test]
fn a_resume_that_leaves_the_blocked_step_short_names_the_budget_it_needs() {
    let mut call = run_call(PANEL, json!({}));
    call.input["agent_budget"] = json!(2);
    let mut host = FakeHost::default();
    let s0 = host.step0(&call, invoke).unwrap();
    let first = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &first, invoke).unwrap();
    assert_eq!(
        s1.output["suspend"]["reason"], "budget_exceeded",
        "{}",
        s1.output
    );
    assert_eq!(stored(&host, &format!("need:{first}")), Some(json!(4)));

    // Omitted, equal to the old ceiling, or raised too little: refused before the host, naming the call.
    for argv in [
        &["resume", "panel"][..],
        &["resume", "panel", "--budget", "2"],
        &["resume", "panel", "--budget", "3"],
    ] {
        let out = host
            .step0(&resume_call(&host, argv), invoke)
            .unwrap()
            .output;
        assert_eq!(out["fail"]["code"], "workflow_invalid_input", "{out}");
        assert!(
            out["fail"]["message"]
                .as_str()
                .unwrap()
                .ends_with("Next: workflow resume panel --budget 4"),
            "{out}"
        );
    }

    let out = host
        .step0(
            &resume_call(&host, &["resume", "panel", "--budget", "4"]),
            invoke,
        )
        .unwrap()
        .output;
    assert_eq!(out["effects"]["pin"]["agent_budget"], 4, "{out}");
}
