//! `<key|handle>` addressing and the `stop`, `pause`, `status` and `result` verbs (L2 section
//! 4.10; root C1, C2.2, C6, C8): an address splits at the last colon, a foreign session is
//! refused, a stop leaves the chain resumable, and a result pages 64 KiB at a time.
use serde_json::{Value, json};
use workflow::address;
use workflow::fakehost::admit::view;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{Answer, canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::wire::JobState;

const PAIR: &str = r##"let meta = #{ name: "pair", description: "two agents in parallel" };
let r = parallel([#{ prompt: "left" }, #{ prompt: "right" }]);
complete(#{ n: r.len() });
"##;

const BIG: &str = r##"let meta = #{ name: "big", description: "returns about 70 KB" };
agent("go");
let s = "";
for i in 0..7000 { s += "0123456789"; }
complete(s);
"##;

fn run_call(script: &str) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script, "args": {}}),
        host: facts("/work"),
    }
}

/// A second tool call of the same session (`kind` says which sort), with `jobs` as the host
/// lists them.
fn call_of(host: &FakeHost, kind: Kind, argv: &[&str]) -> Call {
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
        kind,
        input,
        host: facts,
    }
}

/// A status call by `key` over crafted `jobs`, in a session called `session`.
fn status(session: &str, key: &str, jobs: &Value) -> Value {
    invoke(json!({"protocol": 1, "argv": ["status", key], "jobs": jobs,
                  "caller": {"session_id": session, "tool_call_id": "t9"}}))
}

#[test]
fn addresses_split_at_last_colon_and_foreign_sessions_are_refused() {
    let jobs = json!([
        {"row_doc_id": "r1", "tool_call_id": "t1", "handle": "scan", "state": "running"},
        {"row_doc_id": "r2", "tool_call_id": "t2", "handle": "scan-2", "state": "completed"}
    ]);
    // A child session id holds colons; only the last one splits.
    let session = "job-session:abc:def";
    let out = status(session, "job-session:abc:def:t1", &jobs);
    let text = out["result"].as_str().expect(&out.to_string());
    assert!(text.contains("scan") && text.contains("running"), "{out}");
    let out = status(session, "job-session:abc:def:t2", &jobs);
    assert!(
        out["result"].as_str().unwrap().contains("completed"),
        "{out}"
    );
    assert_eq!(
        address::split("job-session:abc:def:t1"),
        Some(("job-session:abc:def", "t1"))
    );
    assert_eq!(address::split("scan"), None);

    // A plain session id works the same, and a handle names its newest row.
    assert!(status("s1", "s1:t1", &jobs)["result"].is_string());
    let out = status("s1", "scan", &jobs);
    assert!(out["result"].as_str().unwrap().contains("running"), "{out}");

    // Another session's address, and a tool call this session never made, are refused.
    for key in ["s2:t1", "job-session:abc:def:t1", "s1:t7"] {
        let out = status("s1", key, &jobs);
        let expected = format!(
            "workflow_control_unknown_run: {key} is not a run of this session. Next: workflow status <key> with the address from the run's receipt"
        );
        assert_eq!(out["fail"]["message"], expected, "{out}");
        assert_eq!(out["fail"]["code"], "workflow_control_unknown_run", "{out}");
    }
}

#[test]
fn row7_stop_emits_control_and_chain_stays_resumable() {
    let (mut host, call) = (FakeHost::default(), run_call(PAIR));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert_eq!(
        spawns(&s0.output).len(),
        2,
        "children are live: {}",
        s0.output
    );

    let stop = call_of(&host, Kind::ToolCall, &["stop", "pair"]);
    let out = host.tool_call(&stop, invoke).unwrap();
    assert_eq!(
        out["result"], "Stopping pair (cli:t1). The final message arrives as usual.",
        "{out}"
    );
    assert_eq!(out["control"], json!([{"row_doc_id": row, "kind": "stop"}]));
    assert!(
        out.get("pin").is_none() && out.get("effects").is_none(),
        "{out}"
    );
    assert_eq!(host.state.rows[&row].state, JobState::Cancelled);
    assert!(
        !host.state.rows.contains_key("row-t2"),
        "the calling row is never adopted"
    );

    // The chain is still resumable: a cancelled row continues, one attempt further.
    let resume = call_of(&host, Kind::JobStep, &["resume", "pair"]);
    let r0 = host.step0(&resume, invoke).unwrap();
    assert_eq!(
        r0.output["effects"]["resume"],
        json!({"row_doc_id": row}),
        "{}",
        r0.output
    );
    host.settle(&spawns(&r0.output), canned);
    let s1 = host
        .step(&resume, r0.row.as_ref().unwrap(), invoke)
        .unwrap();
    assert_eq!(s1.output["result"], json!({"n": 2}), "{}", s1.output);

    // pause carries kind pause and says how to resume; a row that is not running is refused.
    let (mut host, call) = (FakeHost::default(), run_call(PAIR));
    let row = host.step0(&call, invoke).unwrap().row.unwrap();
    let pause = call_of(&host, Kind::ToolCall, &["pause", &row]);
    let out = host.tool_call(&pause, invoke).unwrap();
    assert_eq!(
        out["control"],
        json!([{"row_doc_id": row, "kind": "pause"}]),
        "{out}"
    );
    assert!(
        out["result"]
            .as_str()
            .unwrap()
            .contains("workflow resume pair"),
        "{out}"
    );
    assert_eq!(host.state.rows[&row].state, JobState::Paused);
    let again = call_of(&host, Kind::ToolCall, &["stop", "pair"]);
    let out = host.tool_call(&again, invoke).unwrap();
    assert_eq!(
        out["fail"]["code"], "workflow_control_not_applicable",
        "{out}"
    );
    assert!(out.get("control").is_none(), "{out}");
}

#[test]
fn row19_result_pages_a_64k_value() {
    let (mut host, call) = (FakeHost::default(), run_call(BIG));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    // A row still running answers not applicable, without asking the host.
    let running = call_of(&host, Kind::ToolCall, &["result", "big"]);
    let out = host.tool_call(&running, invoke).unwrap();
    assert_eq!(
        out["fail"]["message"],
        "workflow_control_not_applicable: big is still running. Next: end your turn; the result arrives as a message",
        "{out}"
    );

    host.answer_child(&slot_of(&s0.output), 0, Answer::ok("ok"))
        .unwrap();
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert!(
        s1.output["result"]
            .as_str()
            .is_some_and(|s| s.len() == 70_000),
        "done"
    );

    // The stored result is the JSON text of the 70 000 byte string: two bytes of quotes more.
    let total = 70_002;
    let first = host.tool_call(&call_of(&host, Kind::ToolCall, &["result", "big"]), invoke);
    let first = first.unwrap();
    let text = first["result"].as_str().expect(&first.to_string());
    let (body, next) = text.rsplit_once("\nNext: ").expect("a next call");
    assert_eq!(body.len(), 65_536, "one page");
    assert_eq!(next, "workflow result cli:t1 --offset 65536", "{first}");

    let argv = ["result", "cli:t1", "--offset", "65536"];
    let second = host
        .tool_call(&call_of(&host, Kind::ToolCall, &argv), invoke)
        .unwrap();
    let rest = second["result"].as_str().expect(&second.to_string());
    assert_eq!(
        rest.len(),
        total - 65_536,
        "the last page ends the value, no next call"
    );
    assert!(!rest.contains("Next:"), "{rest}");
    assert!(
        rest.ends_with("\""),
        "the value's closing quote ends the answer"
    );
}

fn slot_of(out: &Value) -> String {
    spawns(out)[0]["slot"].as_str().unwrap().into()
}

#[test]
fn status_shows_paused_and_awaiting_user_distinctly() {
    let job = |n: u8, state: &str| {
        json!({"row_doc_id": format!("r{n}"), "tool_call_id": format!("t{n}"),
               "handle": format!("w{n}"), "state": state})
    };
    let states = [
        "running",
        "paused",
        "awaiting_user",
        "completed",
        "failed",
        "cancelled",
        "timed_out",
    ];
    let jobs: Vec<Value> = (states.iter().enumerate())
        .map(|(n, s)| job(n as u8, s))
        .collect();
    let jobs = json!(jobs);
    let mut seen = Vec::new();
    for (n, state) in states.iter().enumerate() {
        let out = status("s1", &format!("s1:t{n}"), &jobs);
        let text = out["result"].as_str().expect(&out.to_string()).to_string();
        assert!(text.contains(state), "{state}: {text}");
        let next = match *state {
            "running" => "end your turn; the result arrives as a message".to_string(),
            "paused" => format!("workflow resume s1:t{n}"),
            "awaiting_user" => format!("workflow answer s1:t{n} <text>"),
            _ => format!("workflow result s1:t{n}"),
        };
        assert!(text.ends_with(&format!("Next: {next}")), "{text}");
        seen.push(text);
    }
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        states.len(),
        "every state reads differently: {seen:?}"
    );
}

#[test]
fn result_on_any_non_terminal_row_is_not_applicable_without_a_host_call() {
    let cases = [
        (
            "running",
            "w is still running. Next: end your turn; the result arrives as a message",
        ),
        (
            "paused",
            "w is paused, not finished. Next: workflow resume s1:t1",
        ),
        (
            "awaiting_user",
            "w is awaiting_user, not finished. Next: workflow answer s1:t1 <text>",
        ),
    ];
    for (state, message) in cases {
        let jobs =
            json!([{"row_doc_id": "r1", "tool_call_id": "t1", "handle": "w", "state": state}]);
        let out = invoke(json!({"protocol": 1, "argv": ["result", "w"], "jobs": jobs,
                                "caller": {"session_id": "s1", "tool_call_id": "t9"}}));
        assert_eq!(
            out["fail"]["message"],
            format!("workflow_control_not_applicable: {message}"),
            "{state}: {out}"
        );
        assert!(out.get("job_calls").is_none(), "{state}: {out}");
    }
}

#[test]
fn a_host_suffixed_handle_reaches_step_1() {
    let mut host = FakeHost::default();
    let first = run_call(PAIR);
    host.step0(&first, invoke).unwrap();
    let mut second = run_call(PAIR);
    second.host.caller["tool_call_id"] = json!("t2");
    let s0 = host.step0(&second, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert_eq!(
        host.state.rows[&row].handle, "pair-2",
        "the duplicate is suffixed"
    );

    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&second, &row, invoke).unwrap();
    assert_eq!(s1.output["result"], json!({"n": 2}), "{}", s1.output);

    // The suffixed handle names its own row, the bare one the first.
    let call = call_of(&host, Kind::ToolCall, &["status", "pair-2"]);
    let out = host.tool_call(&call, invoke).unwrap();
    assert!(
        out["result"]
            .as_str()
            .is_some_and(|t| t.contains("pair-2") && t.contains("completed")),
        "{out}"
    );
}

#[test]
fn catalog_inputs_offer_no_result_service() {
    // `result` on the raw shape a catalog call uses never asks the host for a stored result.
    let out = invoke(json!({"argv": ["result", "s1:t1"]}));
    assert_eq!(out["fail"]["code"], "workflow_not_available", "{out}");
    assert!(out.get("job_calls").is_none(), "{out}");
}

#[test]
fn a_handle_names_the_newest_row_listed() {
    let job = |row: &str, resume_of: Value| json!({"row_doc_id": row, "handle": "h", "state": "paused", "tool_call_id": row, "adopted_at": 1, "resume_of": resume_of});
    // The host lists oldest first, so the chain's newest row is the last one listed.
    let listed = json!({"jobs": [job("b", Value::Null), job("a", json!("b"))]});
    assert_eq!(
        workflow::address::resolve(&listed, "h").unwrap()["row_doc_id"],
        "a"
    );
    assert_eq!(
        workflow::address::resolve(&listed, "b").unwrap()["row_doc_id"],
        "b"
    );
}

#[test]
fn a_bare_tool_call_id_names_its_run() {
    let jobs = json!({"jobs": [
        {"row_doc_id": "r1", "handle": "h", "state": "awaiting_user", "tool_call_id": "call1", "adopted_at": 1, "resume_of": null},
        {"row_doc_id": "r2", "handle": "h-2", "state": "awaiting_user", "tool_call_id": "call2", "adopted_at": 2, "resume_of": null},
    ]});
    assert_eq!(
        workflow::address::resolve(&jobs, "call1").unwrap()["row_doc_id"],
        "r1"
    );
    assert!(workflow::address::resolve(&jobs, "call3").is_err());
}
