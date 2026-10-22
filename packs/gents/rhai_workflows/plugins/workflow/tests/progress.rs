//! JobProgress (L2 section 4.11): the frame every output carries, and the pure `phase` and
//! `log` functions that feed it.
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::host_api::Host;
use workflow::invoke;
use workflow::journal::Journal;
use workflow::progress::{self, Kind as Output};
use workflow::rounds::Replay;
use workflow::validate::{Meta, Phase};
use workflow::wire::{Child, JobPin, Usage};

const SCRIPT: &str = r##"let meta = #{ name: "scan", description: "scan and fix", phases: [
    #{ title: "scan", detail: "read the tree" }, #{ title: "fix" }, #{ title: "report" }] };
phase("fix");
log("fixing now");
let r = agent("work on it");
complete(#{ done: r.success });
"##;

fn call(script: &str) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script}),
        host: facts("/work"),
    }
}

fn states(progress: &Value) -> Vec<(String, String)> {
    let titles = progress["phases"].as_array().unwrap().iter();
    titles
        .map(|p| {
            (
                p["title"].as_str().unwrap().into(),
                p["state"].as_str().unwrap().into(),
            )
        })
        .collect()
}

fn child(title: &str, state: &str) -> Child {
    Child {
        attempt: 0,
        session_id: format!("sess-{title}"),
        request_id: "req".into(),
        title: title.into(),
        state: state.into(),
        output_bytes: 0,
        truncated: false,
        usage: Usage {
            input: 0,
            output: 0,
            total: 0,
        },
        duration_ms: 0,
    }
}

fn frame(output: Output, adopted: Option<&str>) -> Value {
    frame_with(output, adopted, BTreeMap::new())
}

fn frame_with(output: Output, adopted: Option<&str>, children: BTreeMap<String, Child>) -> Value {
    let meta = Meta {
        name: "n".into(),
        description: "desc".into(),
        when_to_use: None,
        phases: vec![Phase {
            title: "one".into(),
            detail: None,
        }],
    };
    let pin = JobPin {
        script_id: "s".into(),
        source_digest: "d".into(),
        agent_budget: 8,
        handle: "n".into(),
        host: Map::new(),
    };
    let journal = Journal::new(Replay {
        notes: vec![],
        source: None,
        child_text: BTreeMap::new(),
    });
    let host = Host::new(journal, children, "n");
    progress::build(&meta, &pin, &host, output, adopted)
}

#[test]
fn phase_and_log_floods_keep_the_last_event_and_a_capped_phase_list() {
    let script = r##"let meta = #{ name: "flood", description: "many phases and logs" };
for i in 0..2000 { phase("p" + i); log("line " + i); }
agent("work");
complete(1);
"##;
    let (mut host, call) = (FakeHost::default(), call(script));
    let s0 = host.step0(&call, invoke).unwrap();
    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, s0.row.as_ref().unwrap(), invoke).unwrap();
    let progress = &s1.output["progress"];
    assert_eq!(progress["last_event"], "line 1999");
    assert_eq!(progress["phases"].as_array().unwrap().len(), 64);
}

#[test]
fn progress_fields_per_output_kind() {
    let (mut host, call) = (FakeHost::default(), call(SCRIPT));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let running = &s0.output["effects"]["progress"];
    assert_eq!(running["title"], "scan and fix");
    assert_eq!(running["state"], "running");
    assert!(running.get("pause_reason").is_none() && running.get("question").is_none());
    assert_eq!(
        states(running),
        [("scan", "done"), ("fix", "active"), ("report", "pending")]
            .map(|(t, s)| (t.into(), s.into()))
    );
    assert_eq!(running["active"], json!([]));
    assert_eq!(running["spawned"], 1);
    assert_eq!(
        running["budget"],
        json!({"total": 128, "spent": 1, "remaining": 127})
    );
    assert_eq!(running["observed_terminal_children"], 0);
    assert!(running["last_event_at"].is_null());
    assert_eq!(running["revision"], 0);

    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    let done = &s1.output["progress"];
    assert_eq!(done["state"], "completed");
    assert_eq!(done["last_event"], "fixing now");
    assert_eq!(done["observed_terminal_children"], 1);
    assert_eq!(done["active"], json!([]));

    assert_eq!(frame(Output::Failed, None)["state"], "failed");
    // A real failing step carries the frame too: the script throws after its agent() settled.
    let boom = Call {
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": r##"let meta = #{ name: "boom", description: "throws" };
let r = agent("work on it");
throw "boom";
"##}),
        ..call.clone()
    };
    let mut host = FakeHost::default();
    let b0 = host.step0(&boom, invoke).unwrap();
    let boom_row = b0.row.clone().unwrap();
    host.settle(&spawns(&b0.output), canned);
    let b1 = host.step(&boom, &boom_row, invoke).unwrap();
    assert_eq!(
        b1.output["fail"]["code"], "workflow_launch_failed",
        "{}",
        b1.output
    );
    assert_eq!(b1.output["progress"]["state"], "failed");
    assert_eq!(b1.output["progress"]["observed_terminal_children"], 1);

    let children = BTreeMap::from([
        ("s-live".to_string(), child("live", "running")),
        ("s-done".to_string(), child("done", "completed")),
    ]);
    let mixed = frame_with(Output::Running, None, children);
    assert_eq!(
        mixed["active"],
        json!([{"slot": "s-live", "title": "live", "session_id": "sess-live"}])
    );
    assert_eq!(mixed["observed_terminal_children"], 1);
    let paused = frame(Output::Paused("user".into()), None);
    assert_eq!(
        (&paused["state"], &paused["pause_reason"]),
        (&json!("paused"), &json!("user"))
    );
    let asking = frame(Output::Awaiting("which branch?".into()), None);
    assert_eq!(asking["state"], "awaiting_user");
    assert_eq!(asking["question"], "which branch?");
    assert!(asking.get("pause_reason").is_none());
    assert_eq!(
        frame(Output::Running, None)["phases"],
        json!([{"title": "one", "state": "pending"}])
    );
}

#[test]
fn pause_aliases_normalize_to_the_closed_enum() {
    let reason = |kind: &str| frame(Output::Paused(kind.into()), None)["pause_reason"].clone();
    for kind in [
        "user",
        "back_off",
        "no_progress",
        "verification",
        "infra",
        "budget_exceeded",
    ] {
        assert_eq!(reason(kind), kind);
    }
    assert_eq!(reason("blocked"), "verification");
    assert_eq!(reason("backoff"), "back_off");
    assert_eq!(reason("sleepy"), Value::Null);
}

#[test]
fn adopting_output_carries_tier_and_origin_line() {
    let (mut host, call) = (FakeHost::default(), call(SCRIPT));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let line = s0.output["effects"]["progress"]["last_event"]
        .as_str()
        .unwrap()
        .to_string();
    // The script called log() before its first agent(); the adopting line still wins.
    assert!(
        line.starts_with("started from inline script (predicted origin dynamic, sha256:"),
        "{line}"
    );
    assert!(line.len() <= 256);

    host.settle(&spawns(&s0.output), canned);
    let s1 = host.step(&call, &row, invoke).unwrap();
    assert_eq!(s1.output["progress"]["last_event"], "fixing now");
}

#[test]
fn progress_and_titles_respect_their_caps() {
    let long = "é".repeat(400);
    let script = format!(
        r##"let meta = #{{ name: "caps", description: "{long}" }};
phase("{long}");
for i in 0..400 {{ phase("phase " + i + " {}"); }}
log("{long}");
let r = agent("work", #{{ label: "x" }});
complete(1);
"##,
        "p".repeat(200)
    );
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"], "script": script,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"cwd": "/work"}, "ceiling": {"targets": ["wf-read-only"], "max_live": 4, "remaining_spawns": 9, "dynamic": "run", "dynamic_targets": ["wf-read-only"]}
    }));
    let p = &out["effects"]["progress"];
    assert!(p.is_object(), "{out}");
    assert!(serde_json::to_vec(p).unwrap().len() <= 16 * 1024);
    let title = p["title"].as_str().unwrap();
    assert!(
        title.len() <= 256 && title.contains("truncated from 800 bytes"),
        "{title}"
    );
    assert!(
        p["phases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ph| ph["title"].as_str().unwrap().len() <= 256)
    );
    assert!(!p["phases"].as_array().unwrap().is_empty());

    // An oversize event line is capped with the truncation named.
    let later = frame(Output::Running, Some(&"x".repeat(1000)));
    let event = later["last_event"].as_str().unwrap();
    assert!(
        event.len() <= 256 && event.contains("event truncated from 1000 bytes"),
        "{event}"
    );
    let asked = frame(Output::Awaiting("q".repeat(1000)), None);
    assert!(asked["question"].as_str().unwrap().len() <= 256);
}
