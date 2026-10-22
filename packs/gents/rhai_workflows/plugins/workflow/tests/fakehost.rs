//! The fake host's durable state and round services, driven by hand-written guests.
use serde_json::{Value, json};
use workflow::fakehost::adopt::RowInfo;
use workflow::fakehost::faults::{deterministic, engine_changed, narrowed, resume_with_budget};
use workflow::fakehost::oracle::{Answer, Journal, Labels, canned, spawns};
use workflow::fakehost::state::{ChildRecord, JobRow};
use workflow::fakehost::{Call, FakeHost, HostFacts, Kind};
use workflow::wire::{JobState, Usage};

fn facts() -> HostFacts {
    HostFacts {
        job: Some(json!({
            "row_doc_id": "r1", "chain_root_doc_id": "c1", "chain": ["c1", "r1"], "handle": "h"
        })),
        caller: json!({
            "session_id": "s1", "request_id": "q1", "request_doc_id": "qd1",
            "tool_call_id": "t1", "node_did": "did:me"
        }),
        ceiling: json!({"max_live": 4}),
        mounts: json!([]),
        scratch: json!({}),
        ..HostFacts::default()
    }
}

fn call(kind: Kind, input: Value, host: HostFacts) -> Call {
    Call { kind, input, host }
}

/// A guest that asks `request` of `service` once and returns the answers as its result.
fn ask_once(
    host: &mut FakeHost,
    kind: Kind,
    service: &str,
    request: Value,
) -> Result<Value, String> {
    let key = format!("{service}_results");
    let out = host.run(&call(kind, json!({"argv": ["run"]}), facts()), |input| {
        match input.get(&key) {
            Some(answers) => json!({"result": {"answers": answers, "state": input["state"]}}),
            None => json!({format!("{service}_calls"): {"requests": [request.clone()], "state": {"echo": 7}}}),
        }
    })?;
    let result = &out["result"];
    assert_eq!(result["state"], json!({"echo": 7}), "the state is echoed");
    Ok(result["answers"].clone())
}

fn seed(host: &mut FakeHost, collection: &str, rows: Vec<Value>) {
    host.state.collections.insert(collection.into(), rows);
}

fn query(collection: &str, extra: Value) -> Value {
    let mut q = json!({"collection": collection, "filter": {}, "limit": 256,
                       "fields": ["note_id", "chain_root_doc_id"]});
    q.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"id": "q", "query": q})
}

fn note_write(note_id: &str, value: &str, on_conflict: &str) -> Value {
    json!({"op": "insert", "collection": "WfNote", "on_conflict": on_conflict,
           "fields": {"note_id": note_id, "value_json": value}})
}

fn write_step(host: &mut FakeHost, hf: HostFacts, writes: Value) -> Result<Value, String> {
    host.run(
        &call(Kind::JobStep, json!({"argv": ["run"]}), hf),
        |_| json!({"result": 1, "writes": writes}),
    )
}

#[test]
fn data_calls_filters_order_and_pages_at_256() {
    let mut host = FakeHost::default();
    let notes = (0..300)
        .map(|i| json!({"note_id": format!("n{i:03}"), "chain_root_doc_id": if i < 290 { "c1" } else { "c2" }}))
        .collect();
    seed(&mut host, "WfNote", notes);
    seed(
        &mut host,
        "WfScript",
        vec![
            json!({"node_did": "did:me", "script_id": "mine"}),
            json!({"node_did": "did:other", "script_id": "theirs"}),
        ],
    );
    let ids = |ans: &Value| -> Vec<String> {
        ans["result"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["note_id"].as_str().unwrap().into())
            .collect()
    };

    let eq_c1 = json!({"chain_root_doc_id": {"_eq": "c1"}});
    let mut page = |extra: Value| {
        let a = ask_once(&mut host, Kind::JobStep, "data", query("WfNote", extra)).unwrap();
        a["q"].clone()
    };
    let p1 = page(json!({"filter": eq_c1, "order": {"note_id": "ASC"}, "limit": 256}));
    assert_eq!(ids(&p1).len(), 256);
    assert_eq!(ids(&p1)[255], "n255");
    let p2 =
        page(json!({"filter": eq_c1, "order": {"note_id": "ASC"}, "offset": 256, "limit": 256}));
    assert_eq!(ids(&p2).len(), 34, "290 rows of c1, 256 already read");
    let desc = page(json!({
        "filter": {"_and": [eq_c1, {"note_id": {"_in": ["n001", "n002", "n295"]}}]},
        "order": {"note_id": "DESC"}, "limit": 10
    }));
    assert_eq!(
        ids(&desc),
        ["n002", "n001"],
        "_and, _in and desc order; n295 is in c2's range"
    );
    let or = page(json!({"filter": {"_or": [eq_c1]}}));
    assert!(or["error"].as_str().unwrap().contains("_or"), "{or}");
    let bare = page(json!({"filter": {"note_id": "n001"}}));
    assert!(bare["error"].is_string(), "{bare}");
    let over = page(json!({"limit": 257}));
    assert!(over["error"].as_str().unwrap().contains("256"), "{over}");
    for filter in [json!(null), json!("c1"), json!([eq_c1])] {
        let odd = page(json!({"filter": filter}));
        assert!(odd["error"].is_string(), "{odd}");
    }
    let nested = page(json!({"filter": {"_and": ["c1"]}}));
    assert!(nested["error"].is_string(), "{nested}");

    let own = ask_once(
        &mut host,
        Kind::JobStep,
        "data",
        json!({"id": "q", "query": {
            "collection": "WfScript",
            "filter": {"node_did": {"_eq": "$caller.node_did"}},
            "fields": ["script_id"], "limit": 10
        }}),
    )
    .unwrap();
    assert_eq!(own["q"]["result"]["rows"], json!([{"script_id": "mine"}]));
}

#[test]
fn round_requests_off_the_contract_shape_are_refused() {
    let mut host = FakeHost::default();
    let good = json!({"id": "q", "query": {"collection": "WfNote", "filter": {}, "limit": 1,
                                           "fields": ["note_id"]}});
    assert!(
        ask_once(&mut host, Kind::JobStep, "data", good.clone()).unwrap()["q"]["result"]
            .is_object()
    );
    for (what, patch) in [
        ("an array order", json!({"order": [{"note_id": "asc"}]})),
        (
            "a lowercase direction",
            json!({"order": {"note_id": "asc"}}),
        ),
        ("an unknown field", json!({"extra": 1})),
        ("no limit", json!({"limit": null})),
        ("a string offset", json!({"offset": "1"})),
    ] {
        let mut bad = good.clone();
        bad["query"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let ans = ask_once(&mut host, Kind::JobStep, "data", bad).unwrap();
        assert!(ans["q"]["error"].is_string(), "{what}: {ans}");
    }
    let mut extra = good;
    extra["junk"] = json!(1);
    let ans = ask_once(&mut host, Kind::JobStep, "data", extra).unwrap();
    assert!(
        ans["q"]["error"].is_string(),
        "an unknown request field: {ans}"
    );

    // child_output answers omit `truncated` when false.
    host.state.children.insert(
        ("1:aa".into(), 0),
        ChildRecord {
            session_id: "cs".into(),
            request_id: "cq".into(),
            title: "t".into(),
            state: "completed".into(),
            text: "abc".into(),
            usage: Usage {
                input: 1,
                output: 1,
                total: 2,
            },
            duration_ms: 5,
        },
    );
    let req = json!({"id": "t", "child_output": {"slots": ["1:aa"], "offset": 0, "limit": 100}});
    let a = ask_once(&mut host, Kind::JobStep, "job", req).unwrap();
    assert_eq!(a["t"]["result"]["slots"]["1:aa"], json!({"text": "abc"}));

    // The host serves text for a completed child only.
    host.fail_child("1:aa", 0, "boom").unwrap();
    let req = json!({"id": "t", "child_output": {"slots": ["1:aa"], "offset": 0, "limit": 100}});
    let a = ask_once(&mut host, Kind::JobStep, "job", req).unwrap();
    assert!(
        a["t"]["error"].as_str().unwrap().contains("no output yet"),
        "{a}"
    );
}

#[test]
fn first_wins_notes_keep_the_first_value() {
    let mut host = FakeHost::default();
    write_step(&mut host, facts(), json!([note_write("a", "1", "ignore")])).unwrap();
    write_step(&mut host, facts(), json!([note_write("a", "2", "ignore")])).unwrap();
    let rows = &host.state.collections["WfNote"];
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["value_json"], "1");

    let err = write_step(&mut host, facts(), json!([note_write("a", "3", "fail")])).unwrap_err();
    assert!(err.contains("unique index"), "{err}");

    let name = json!({"op": "insert", "collection": "WfName", "on_conflict": "fail",
                      "fields": {"name": "x", "version": 1}});
    write_step(&mut host, facts(), json!([name])).unwrap();
    let err = write_step(&mut host, facts(), json!([name])).unwrap_err();
    assert!(err.contains("unique index"), "{err}");
}

#[test]
fn written_notes_read_back_with_a_doc_id() {
    let mut host = FakeHost::default();
    let notes = json!([note_write("a", "1", "fail"), note_write("b", "2", "fail")]);
    write_step(&mut host, facts(), notes).unwrap();
    let ans = ask_once(
        &mut host,
        Kind::JobStep,
        "data",
        json!({"id": "q", "query": {
            "collection": "WfNote",
            "filter": {"chain_root_doc_id": {"_eq": "c1"}},
            "order": {"_docID": "ASC"},
            "fields": ["_docID", "note_id", "value_json", "row"], "limit": 256
        }}),
    )
    .unwrap();
    let rows = ans["q"]["result"]["rows"].as_array().unwrap().clone();
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["_docID"].as_str()).collect();
    assert_eq!(ids.len(), 2, "every written row has a _docID: {rows:?}");
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn write_without_stamp_value_is_refused() {
    let mut host = FakeHost::default();
    let pinless = HostFacts {
        job: None,
        ..facts()
    };
    let err = write_step(&mut host, pinless, json!([note_write("a", "1", "ignore")])).unwrap_err();
    assert!(
        err.contains("stamp") && err.contains("chain_root_doc_id"),
        "{err}"
    );
    assert!(
        host.state.collections.is_empty(),
        "a refused output writes nothing"
    );
}

#[test]
fn write_with_a_guest_stamp_value_is_refused() {
    let mut host = FakeHost::default();
    let mut w = note_write("a", "1", "ignore");
    w["fields"]["node_did"] = json!("did:guest");
    let err = write_step(&mut host, facts(), json!([w])).unwrap_err();
    assert!(err.contains("node_did") && err.contains("stamped"), "{err}");
    assert!(host.state.collections.is_empty(), "nothing is written");
}

#[test]
fn write_with_a_guest_doc_id_is_refused() {
    let mut host = FakeHost::default();
    let mut w = note_write("a", "1", "ignore");
    w["fields"]["_docID"] = json!("bae-guest");
    let err = write_step(&mut host, facts(), json!([w])).unwrap_err();
    assert!(err.contains("_docID") && err.contains("host"), "{err}");
    assert!(host.state.collections.is_empty(), "nothing is written");
}

#[test]
fn every_dagger_field_is_overwritten_each_step() {
    let stale = json!({
        "protocol": 1, "argv": ["run", "x"], "args": {"k": 1}, "path": "p.rhai",
        "script": "let meta = #{};", "agent_budget": 9,
        "job": "stale", "caller": "stale", "ceiling": "stale", "mounts": "stale",
        "control": "stale", "root_call": "stale", "scratch": "stale",
        "verified_script": "stale", "origin": "stale"
    });
    let mut host = FakeHost::default();
    let echo = |host: &mut FakeHost, hf: HostFacts| {
        host.run(
            &call(Kind::JobStep, stale.clone(), hf),
            |input| json!({"result": input}),
        )
        .unwrap()["result"]
            .clone()
    };

    let full = HostFacts {
        control: json!({"kind": "resume"}),
        root_call: Some(json!({"argv": ["run"]})),
        verified_script: Some(json!({"source": "s", "digest": "d"})),
        origin: json!("installed"),
        mounts: json!([{"pack": "p"}]),
        scratch: json!({"a": "/x/a"}),
        ..facts()
    };
    let got = echo(&mut host, full.clone());
    assert_eq!(got["job"], full.job.unwrap());
    assert_eq!(got["caller"], full.caller);
    assert_eq!(got["ceiling"], full.ceiling);
    assert_eq!(got["mounts"], full.mounts);
    assert_eq!(got["control"], full.control);
    assert_eq!(got["root_call"], full.root_call.unwrap());
    assert_eq!(got["scratch"], full.scratch);
    assert_eq!(got["verified_script"], full.verified_script.unwrap());
    assert_eq!(got["origin"], full.origin);
    for (k, v) in [
        ("argv", json!(["run", "x"])),
        ("args", json!({"k": 1})),
        ("path", json!("p.rhai")),
        ("script", json!("let meta = #{};")),
        ("agent_budget", json!(9)),
    ] {
        assert_eq!(got[k], v, "C9 argument {k} is echoed");
    }

    let bare = echo(&mut host, facts());
    for k in ["root_call", "verified_script"] {
        assert!(
            bare.get(k).is_none(),
            "an unset host field drops the guest's {k}"
        );
    }
    assert_eq!(bare["control"], Value::Null);
    assert_eq!(bare["origin"], Value::Null);
}

#[test]
fn child_output_and_result_page_their_text() {
    let mut host = FakeHost::default();
    let child = |text: &str| ChildRecord {
        session_id: "cs".into(),
        request_id: "cq".into(),
        title: "t".into(),
        state: "completed".into(),
        text: text.into(),
        usage: Usage {
            input: 1,
            output: 1,
            total: 2,
        },
        duration_ms: 5,
    };
    host.state.children.insert(("1:aa".into(), 0), child("old"));
    host.state
        .children
        .insert(("1:aa".into(), 1), child("abcdefghij"));
    host.state.rows.insert(
        "r9".into(),
        JobRow {
            row_doc_id: "r9".into(),
            tool_call_id: "t9".into(),
            handle: "h9".into(),
            state: JobState::Completed,
            pin: None,
            chain_root: "r9".into(),
            result: Some(json!("0123456789")),
        },
    );

    let req = json!({"id": "t", "child_output": {"slots": ["1:aa"], "offset": 2, "limit": 4}});
    let a = ask_once(&mut host, Kind::JobStep, "job", req).unwrap();
    assert_eq!(
        a["t"]["result"]["slots"]["1:aa"],
        json!({"text": "cdef", "truncated": true})
    );
    let req = json!({"id": "t", "result": {"row_doc_id": "r9", "offset": 0, "limit": 100}});
    // A row of another session is outside the caller's scope.
    let info = |session: &str| RowInfo {
        session_id: session.into(),
        origin: Value::Null,
        control: Value::Null,
        root_call: None,
        adopted_at: 1,
    };
    host.jobs.rows.insert("r9".into(), info("s2"));
    let a = ask_once(&mut host, Kind::ToolCall, "job", req.clone()).unwrap();
    assert!(a["t"]["error"].as_str().unwrap().contains("scope"), "{a}");
    host.jobs.rows.insert("r9".into(), info("s1"));
    let a = ask_once(&mut host, Kind::ToolCall, "job", req.clone()).unwrap();
    assert_eq!(
        a["t"]["result"],
        json!({"row_doc_id": "r9", "state": "completed", "result": "\"0123456789\"",
               "offset": 0, "total_bytes": 12, "truncated": false})
    );
    let paged = json!({"id": "t", "result": {"row_doc_id": "r9", "offset": 1, "limit": 4}});
    let a = ask_once(&mut host, Kind::ToolCall, "job", paged).unwrap();
    assert_eq!(a["t"]["result"]["result"], "0123");
    assert_eq!(a["t"]["result"]["truncated"], true);
    // A running row has no result yet.
    host.state.rows.get_mut("r9").unwrap().state = JobState::Running;
    let a = ask_once(&mut host, Kind::ToolCall, "job", req).unwrap();
    assert!(
        a["t"]["error"]
            .as_str()
            .unwrap()
            .starts_with("not_terminal"),
        "{a}"
    );
    // A paused or awaiting row is not terminal either.
    for state in [JobState::Paused, JobState::AwaitingUser] {
        host.state.rows.get_mut("r9").unwrap().state = state;
        let a = ask_once(
            &mut host,
            Kind::ToolCall,
            "job",
            json!({"id": "t", "result": {"row_doc_id": "r9", "offset": 0, "limit": 100}}),
        )
        .unwrap();
        assert!(
            a["t"]["error"]
                .as_str()
                .unwrap()
                .starts_with("not_terminal"),
            "{a}"
        );
    }

    let seen = host
        .run(
            &call(Kind::Callback, json!({}), facts()),
            |i| json!({"result": i}),
        )
        .unwrap();
    assert!(
        seen["result"].get("job_calls").is_none(),
        "callbacks are not offered job_calls"
    );
    assert_eq!(seen["result"]["data_calls"], true);
}

// ---- adoption, admission and controls ----

use std::fs;
use workflow::fakehost::adopt::Stepped;
use workflow::fakehost::{PackAssets, mounts};

fn temp_cwd(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("wf-fakehost-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into()
}

/// Host facts for a job call of `session`/`tool_call`; `job` is the host's to fill.
fn job_host(session: &str, tool_call: &str, cwd: &str) -> HostFacts {
    HostFacts {
        caller: json!({
            "session_id": session, "request_id": "q", "request_doc_id": "qd",
            "tool_call_id": tool_call, "node_did": "did:me", "cwd": cwd
        }),
        ceiling: json!({
            "targets": ["wf-read-only"], "max_live": 2, "remaining_spawns": 5,
            "max_messages_per_slot": 1, "dynamic": "check", "dynamic_targets": []
        }),
        mounts: json!([]),
        scratch: json!({}),
        origin: json!("installed"),
        ..HostFacts::default()
    }
}

fn pin(handle: &str) -> Value {
    json!({"script_id": "sid", "source_digest": format!("sha256:{}", "ab".repeat(32)), "agent_budget": 8, "handle": handle})
}

fn spawn(slot: &str, target: &str) -> Value {
    json!({"slot": slot, "attempt": 0, "target": target, "prompt": "p", "title": "t"})
}

fn root_input() -> Value {
    json!({"argv": ["run", "x"], "args": {"k": 1}})
}

fn start(host: &mut FakeHost, hf: HostFacts, input: Value, out: Value) -> Stepped {
    host.step0(&call(Kind::JobStep, input, hf), |_| out.clone())
        .unwrap()
}

fn later(host: &mut FakeHost, hf: HostFacts, row: &str, out: Value) -> Stepped {
    host.step(
        &call(Kind::JobStep, json!({"argv": ["run", "x"]}), hf),
        row,
        |_| out.clone(),
    )
    .unwrap()
}

fn running_with_child(host: &mut FakeHost, hf: HostFacts, slot: &str) -> String {
    let out = json!({"effects": {"pin": pin("h"), "spawn": [spawn(slot, "wf-read-only")]}});
    start(host, hf, root_input(), out).row.unwrap()
}

#[test]
fn each_adoption_trigger_and_resume_beside_pin() {
    let cwd = temp_cwd("adopt");
    let mut host = FakeHost::default();

    // Trigger 1: pin on step-0 effects.
    let out = json!({"effects": {"pin": pin("wf"), "spawn": [spawn("1:aa", "wf-read-only")]}});
    let s = start(&mut host, job_host("s1", "t1", &cwd), root_input(), out);
    assert_eq!(s.row.as_deref(), Some("row-t1"));
    assert!(s.continues, "an adopted running row has a Continuation");
    let row = &host.state.rows["row-t1"];
    assert_eq!(row.state, JobState::Running);
    assert_eq!(
        row.pin.as_ref().unwrap().host["chain_root_doc_id"],
        "row-t1"
    );
    assert!(host.state.children.contains_key(&("1:aa".into(), 0)));

    // Trigger 2: pin on a step-0 suspend; it returns inline.
    let out =
        json!({"suspend": {"kind": "pause", "reason": "user", "message": "m"}, "pin": pin("wf2")});
    let s = start(&mut host, job_host("s1", "t2", &cwd), root_input(), out);
    assert!(
        !s.continues,
        "a pinned terminal or suspend has no Continuation"
    );
    assert_eq!(host.state.rows["row-t2"].state, JobState::Paused);

    // Trigger 3: pin on a step-0 result that carries scratch.
    let out = json!({"result": "done", "pin": pin("wf3"),
                     "scratch": [{"name": "a.txt", "content": "hi"}]});
    let s = start(&mut host, job_host("s1", "t3", &cwd), root_input(), out);
    assert!(!s.continues);
    assert_eq!(host.state.rows["row-t3"].state, JobState::Completed);
    let path = format!("{cwd}/.gents/workflows/runs/t3/a.txt");
    assert_eq!(fs::read_to_string(path).unwrap(), "hi");

    // Without a pin nothing is adopted, and a WfNote write is refused for want of a stamp.
    let s = start(
        &mut host,
        job_host("s1", "t4", &cwd),
        root_input(),
        json!({"result": 1}),
    );
    assert_eq!(s.row, None);
    let err = host
        .step0(
            &call(Kind::JobStep, root_input(), job_host("s1", "t4", &cwd)),
            |_| json!({"result": 1, "writes": [note_write("a", "1", "ignore")]}),
        )
        .unwrap_err();
    assert!(err.contains("stamp"), "{err}");
    assert!(!host.state.rows.contains_key("row-t4"));

    // Resume beside pin on a step-0 result: a new, already-terminal row of the prior chain.
    let out = json!({"result": "fin", "pin": pin("ignored"),
                     "resume": {"row_doc_id": "row-t2", "answer": "yes"}});
    let s = start(
        &mut host,
        job_host("s1", "t5", &cwd),
        json!({"argv": ["resume", "row-t2"]}),
        out,
    );
    assert!(!s.continues);
    let (prior, row) = (&host.state.rows["row-t2"], &host.state.rows["row-t5"]);
    assert_eq!(row.state, JobState::Completed);
    assert_eq!(
        (row.handle.as_str(), row.chain_root.as_str()),
        ("wf2", "row-t2")
    );
    assert_eq!(
        row.pin.as_ref().unwrap().host["args_digest"],
        prior.pin.as_ref().unwrap().host["args_digest"],
        "the digest is copied from the chain root, not computed from the resume verb"
    );
    let chain = &host.jobs.chains["row-t2"];
    assert_eq!((chain.head.as_str(), chain.rows.len()), ("row-t5", 2));
    assert_eq!(
        chain.journal,
        [json!({"row_doc_id": "row-t5", "answer": "yes"})]
    );

    // Resume beside pin on a step-0 suspend.
    let out = json!({"suspend": {"kind": "await_user", "message": "q", "question": "q?"},
                     "pin": pin("x"), "resume": {"row_doc_id": "row-t5"}});
    start(
        &mut host,
        job_host("s1", "t6", &cwd),
        json!({"argv": ["resume"]}),
        out,
    );
    assert_eq!(host.state.rows["row-t6"].state, JobState::AwaitingUser);
    assert_eq!(host.jobs.chains["row-t2"].head, "row-t6");

    // A running row cannot be resumed.
    let err = host
        .step0(
            &call(
                Kind::JobStep,
                json!({"argv": ["resume"]}),
                job_host("s1", "t7", &cwd),
            ),
            |_| json!({"result": 1, "pin": pin("y"), "resume": {"row_doc_id": "row-t1"}}),
        )
        .unwrap_err();
    assert!(err.contains("running"), "{err}");
}

#[test]
fn refused_scratch_leaves_no_write_or_file_behind() {
    let mut host = FakeHost::default();
    let cwd = temp_cwd("scratch-refused");
    let out = json!({"result": "done", "pin": pin("w"),
                     "writes": [note_write("a", "1", "ignore")],
                     "scratch": [{"name": "ok.txt", "content": "hi"},
                                 {"name": "../x", "content": "no"}]});
    let err = host
        .step0(
            &call(Kind::JobStep, root_input(), job_host("s1", "t1", &cwd)),
            |_| out.clone(),
        )
        .unwrap_err();
    assert!(err.contains("one path component"), "{err}");
    assert!(host.state.collections.is_empty(), "no write was committed");
    assert!(!host.state.rows.contains_key("row-t1"));
    assert!(!std::path::Path::new(&format!("{cwd}/.gents")).exists());
}

#[test]
fn duplicate_handle_is_suffixed_by_the_host() {
    let cwd = temp_cwd("handles");
    let mut host = FakeHost::default();
    let suspend = json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("h")});
    for (session, tc) in [("s1", "t1"), ("s1", "t2"), ("s2", "t3")] {
        start(
            &mut host,
            job_host(session, tc, &cwd),
            root_input(),
            suspend.clone(),
        );
    }
    let handle = |r: &str| host.state.rows[r].handle.clone();
    assert_eq!(
        [handle("row-t1"), handle("row-t2"), handle("row-t3")],
        ["h", "h-2", "h"],
        "a duplicate is suffixed within its caller session only"
    );
    assert_eq!(
        host.state.rows["row-t2"].pin.as_ref().unwrap().handle,
        "h-2"
    );
}

#[test]
fn terminal_with_live_child_is_policy_denied_and_kills() {
    let cwd = temp_cwd("policy");
    let mut host = FakeHost::default();
    let row = running_with_child(&mut host, job_host("s1", "t1", &cwd), "1:aa");
    let s = later(
        &mut host,
        job_host("s1", "t1", &cwd),
        &row,
        json!({"result": 1}),
    );
    assert_eq!(s.code.as_deref(), Some("plugin_job_policy_denied"));
    assert!(!s.continues);
    assert_eq!(host.state.rows[&row].state, JobState::Failed);
    assert_eq!(
        host.state.children[&("1:aa".into(), 0)].state,
        "interrupted"
    );

    // A target outside the ceiling is refused, and nothing of the output is admitted.
    let out = json!({"effects": {"pin": pin("h"), "spawn": [spawn("1:bb", "elsewhere")]}});
    let s = start(&mut host, job_host("s1", "t2", &cwd), root_input(), out);
    assert_eq!(s.code.as_deref(), Some("plugin_job_policy_denied"));
    assert!(!host.state.children.contains_key(&("1:bb".into(), 0)));

    // A re-served key is a no-op: not charged, not a new spawn.
    let row = running_with_child(&mut host, job_host("s1", "t3", &cwd), "1:cc");
    let again = json!({"effects": {"spawn": [spawn("1:cc", "wf-read-only")]}});
    let s = later(&mut host, job_host("s1", "t3", &cwd), &row, again);
    assert!(s.continues && s.code.is_none());
    assert_eq!(host.jobs.chains[&row].charged, 1);
}

#[test]
fn the_pin_digest_and_first_attempt_follow_the_host_contract() {
    let cwd = temp_cwd("contract");
    let mut host = FakeHost::default();

    // The host reads `source_digest` as `sha256:<hex>`; bare hex is refused.
    let mut bare = pin("h");
    bare["source_digest"] = json!("ab".repeat(32));
    let out = json!({"effects": {"pin": bare, "spawn": [spawn("1:aa", "wf-read-only")]}});
    let refused = host.step0(
        &call(Kind::JobStep, root_input(), job_host("s1", "t1", &cwd)),
        |_| out.clone(),
    );
    assert!(refused.unwrap_err().contains("sha256:"));

    // A fresh slot starts at attempt 0; 1 is refused and nothing is admitted.
    let mut first = spawn("1:bb", "wf-read-only");
    first["attempt"] = json!(1);
    let out = json!({"effects": {"pin": pin("h"), "spawn": [first]}});
    let s = start(&mut host, job_host("s1", "t2", &cwd), root_input(), out);
    assert_eq!(s.code.as_deref(), Some("plugin_job_policy_denied"));
    assert!(!host.state.children.contains_key(&("1:bb".into(), 1)));
}

#[test]
fn stall_and_three_failed_steps_end_the_job() {
    let cwd = temp_cwd("stall");
    let mut host = FakeHost::default();
    let row = running_with_child(&mut host, job_host("s1", "t1", &cwd), "1:aa");
    host.state
        .children
        .get_mut(&("1:aa".into(), 0))
        .unwrap()
        .state = "completed".into();
    let idle = json!({"effects": {"progress": {"title": "t"}}});
    let s = later(&mut host, job_host("s1", "t1", &cwd), &row, idle);
    assert_eq!(s.code.as_deref(), Some("plugin_job_stalled"));
    assert_eq!(host.state.rows[&row].state, JobState::Failed);

    let row = running_with_child(&mut host, job_host("s1", "t2", &cwd), "1:bb");
    for n in 1..=3 {
        let s = later(
            &mut host,
            job_host("s1", "t2", &cwd),
            &row,
            json!("garbage"),
        );
        assert_eq!(s.continues, n < 3, "failure {n}");
        assert_eq!(
            s.code.as_deref(),
            (n == 3).then_some("plugin_job_step_failed")
        );
    }
    assert_eq!(host.state.rows[&row].state, JobState::Failed);
    assert_eq!(
        host.state.children[&("1:bb".into(), 0)].state,
        "interrupted"
    );
}

#[test]
fn control_echo_follows_answer_engine_current_resume() {
    let cwd = temp_cwd("control");
    let mut host = FakeHost::default();
    let suspend = json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("w")});

    // Only a chain's newest row resumes, so each kind resumes a row paused of its own.
    let kinds = [
        (
            json!({"answer": "a", "engine": "current", "budget": 9}),
            "answer",
        ),
        (json!({"engine": "current", "budget": 9}), "engine_current"),
        (json!({"budget": 9}), "resume"),
    ];
    for (i, (extra, kind)) in kinds.into_iter().enumerate() {
        let prior = format!("row-p{i}");
        start(
            &mut host,
            job_host("s1", &format!("p{i}"), &cwd),
            root_input(),
            suspend.clone(),
        );
        let mut block = extra;
        block["row_doc_id"] = json!(prior);
        let tc = format!("r{i}");
        let slot = format!("1:r{i}");
        let hf = || {
            let mut hf = job_host("s1", &tc, &cwd);
            hf.ceiling["max_live"] = json!(5);
            hf
        };
        let out = json!({"effects": {"pin": pin("w"), "resume": block.clone(), "spawn": [spawn(&slot, "wf-read-only")]}});
        let s = start(&mut host, hf(), json!({"argv": ["resume", &prior]}), out);
        let row = s.row.unwrap();

        let mut seen = Value::Null;
        host.step(
            &call(Kind::JobStep, json!({"argv": ["resume", &prior]}), hf()),
            &row,
            |input| {
                seen = input;
                json!({"effects": {"progress": {"title": "t"}}})
            },
        )
        .unwrap();
        assert_eq!(seen["control"], json!({"kind": kind, "payload": block}));
        assert_eq!(
            seen["root_call"],
            root_input(),
            "the chain root's arguments"
        );
        let jobs = seen["jobs"].as_array().unwrap();
        assert!(
            jobs.iter()
                .any(|j| j["row_doc_id"] == prior.as_str() && j["state"] == "paused"),
            "{jobs:?}"
        );
    }
}

#[test]
fn jobs_list_oldest_first_with_the_row_each_resumed() {
    let cwd = temp_cwd("jobs-order");
    let mut host = FakeHost::default();
    let suspend = json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("w")});
    // The ids sort the other way round from the adoption order.
    start(
        &mut host,
        job_host("s1", "z", &cwd),
        root_input(),
        suspend.clone(),
    );
    let resume = json!({"effects": {"pin": pin("w"), "resume": {"row_doc_id": "row-z"}}});
    start(
        &mut host,
        job_host("s1", "m", &cwd),
        json!({"argv": ["resume", "row-z"]}),
        resume,
    );
    running_with_child(&mut host, job_host("s1", "a", &cwd), "1:a");

    let mut seen = Value::Null;
    host.step(
        &call(
            Kind::JobStep,
            json!({"argv": ["run", "x"]}),
            job_host("s1", "a", &cwd),
        ),
        "row-a",
        |input| {
            seen = input;
            json!({"effects": {"progress": {"title": "t"}}})
        },
    )
    .unwrap();
    let listed: Vec<_> = seen["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| {
            (
                j["row_doc_id"].as_str().unwrap(),
                j["adopted_at"].as_u64().unwrap(),
                j["resume_of"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("row-z", 1, None),
            ("row-m", 2, Some("row-z")),
            ("row-a", 3, None)
        ]
    );
}

#[test]
fn only_the_newest_row_of_a_chain_resumes() {
    let cwd = temp_cwd("resume-head");
    let mut host = FakeHost::default();
    let suspend = json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("w")});
    start(
        &mut host,
        job_host("s1", "p", &cwd),
        root_input(),
        suspend.clone(),
    );
    let resume = |row: &str| json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("w"), "resume": {"row_doc_id": row}});
    start(
        &mut host,
        job_host("s1", "q", &cwd),
        json!({"argv": ["resume", "row-p"]}),
        resume("row-p"),
    );
    let err = host
        .step0(
            &call(
                Kind::JobStep,
                json!({"argv": ["resume", "row-p"]}),
                job_host("s1", "r", &cwd),
            ),
            |_| resume("row-p"),
        )
        .unwrap_err();
    assert!(err.starts_with("plugin_job_resume_not_head"), "{err}");
    assert!(err.contains("row-q"), "{err}");
    let err = host
        .step0(
            &call(
                Kind::JobStep,
                json!({"argv": ["resume", "gone"]}),
                job_host("s1", "s", &cwd),
            ),
            |_| resume("gone"),
        )
        .unwrap_err();
    assert!(err.starts_with("plugin_job_resume_unknown"), "{err}");
}

#[test]
fn non_job_control_stops_or_pauses_its_row() {
    let cwd = temp_cwd("stop");
    let mut host = FakeHost::default();
    let a = running_with_child(&mut host, job_host("s1", "t1", &cwd), "1:aa");
    let b = running_with_child(&mut host, job_host("s1", "t2", &cwd), "1:bb");
    let control = json!([
        {"row_doc_id": a, "kind": "stop"}, {"row_doc_id": b, "kind": "pause"}
    ]);
    host.tool_call(
        &call(
            Kind::ToolCall,
            json!({"argv": ["stop"]}),
            job_host("s1", "t9", &cwd),
        ),
        |_| json!({"result": "Stopping", "control": control}),
    )
    .unwrap();
    assert_eq!(host.state.rows[&a].state, JobState::Cancelled);
    assert_eq!(host.state.rows[&b].state, JobState::Paused);
    assert_eq!(
        host.state.children[&("1:aa".into(), 0)].state,
        "interrupted"
    );
    assert!(
        !host.state.rows.contains_key("row-t9"),
        "the calling row is never adopted"
    );
}

#[test]
fn resume_and_control_refuse_another_sessions_row() {
    let cwd = temp_cwd("scope");
    let mut host = FakeHost::default();
    let suspend = json!({"suspend": {"kind": "pause", "message": "m"}, "pin": pin("w")});
    start(&mut host, job_host("s1", "p1", &cwd), root_input(), suspend);
    let err = host
        .step0(
            &call(
                Kind::JobStep,
                json!({"argv": ["resume"]}),
                job_host("s2", "t2", &cwd),
            ),
            |_| json!({"result": 1, "pin": pin("y"), "resume": {"row_doc_id": "row-p1"}}),
        )
        .unwrap_err();
    assert!(err.contains("another session"), "{err}");
    assert!(!host.state.rows.contains_key("row-t2"));

    let a = running_with_child(&mut host, job_host("s1", "t3", &cwd), "1:aa");
    let err = host
        .tool_call(
            &call(
                Kind::ToolCall,
                json!({"argv": ["stop"]}),
                job_host("s2", "t4", &cwd),
            ),
            |_| json!({"result": "Stopping", "control": [{"row_doc_id": a, "kind": "stop"}]}),
        )
        .unwrap_err();
    assert!(err.contains("another session"), "{err}");
    assert_eq!(host.state.rows[&a].state, JobState::Running);
}

#[test]
fn a_row_never_adopted_is_refused_not_a_panic() {
    let cwd = temp_cwd("unadopted");
    let mut host = FakeHost::default();
    host.state.rows.insert(
        "row-x".into(),
        JobRow {
            row_doc_id: "row-x".into(),
            tool_call_id: "tx".into(),
            handle: "x".into(),
            state: JobState::Running,
            pin: None,
            chain_root: "row-x".into(),
            result: None,
        },
    );
    let err = host
        .step0(
            &call(
                Kind::JobStep,
                json!({"argv": ["resume"]}),
                job_host("s1", "t2", &cwd),
            ),
            |_| json!({"result": 1, "pin": pin("y"), "resume": {"row_doc_id": "row-x"}}),
        )
        .unwrap_err();
    assert!(err.contains("never adopted"), "{err}");
    let err = host
        .step(
            &call(
                Kind::JobStep,
                json!({"argv": ["run"]}),
                job_host("s1", "t3", &cwd),
            ),
            "row-x",
            |_| json!({}),
        )
        .unwrap_err();
    assert!(err.contains("never adopted"), "{err}");
    let err = host
        .tool_call(
            &call(
                Kind::ToolCall,
                json!({"argv": ["stop"]}),
                job_host("s1", "t4", &cwd),
            ),
            |_| json!({"result": "Stopping", "control": [{"row_doc_id": "row-x", "kind": "stop"}]}),
        )
        .unwrap_err();
    assert!(err.contains("never adopted"), "{err}");
}

#[test]
fn mounts_list_a_temp_dir_per_pack_and_the_project_tier() {
    let packs = [PackAssets {
        pack: "content".into(),
        class: "workflows".into(),
        digest: "d1".into(),
        trusted: true,
    }];
    let m = mounts(&packs, Some("/p/.gents/workflows")).unwrap();
    assert_eq!(m[0]["pack"], "content");
    assert_eq!(m[0]["trusted"], true);
    assert!(std::path::Path::new(m[0]["path"].as_str().unwrap()).is_dir());
    assert_eq!(
        m[1],
        json!({"pack": "", "class": "project", "digest": "", "path": "/p/.gents/workflows", "trusted": false})
    );
    assert_eq!(mounts(&packs, None).unwrap().as_array().unwrap().len(), 1);
}

// ---- oracles, faults and the example ----

use workflow::fakehost::cli;

fn titled(slot: &str, title: &str) -> Value {
    let mut s = spawn(slot, "wf-read-only");
    s["title"] = json!(title);
    s
}

fn child(host: &FakeHost, slot: &str) -> (String, String) {
    let c = &host.state.children[&(slot.to_string(), 0)];
    (c.state.clone(), c.text.clone())
}

#[test]
fn canned_and_label_oracles_answer_spawns() {
    // Canned: the minimal instance of the output_schema in a json fence, or ok.
    let mut with_schema = titled("1:cc", "c");
    with_schema["output_schema"] = json!({
        "type": "object", "required": ["n", "tags", "name"],
        "properties": {"n": {"type": "integer"}, "tags": {"type": "array", "items": {"type": "string"}},
                       "name": {"enum": ["x", "y"]}, "extra": {"type": "string"}}
    });
    assert_eq!(
        canned(&with_schema),
        Answer::ok("```json\n{\"n\":0,\"name\":\"x\",\"tags\":[]}\n```")
    );
    assert_eq!(canned(&titled("1:bb", "b")), Answer::ok("ok"));

    // Label: results keyed by spawn title, a failing slot included; an unlabeled spawn is canned.
    let labels = Labels::from_json(&json!({
        "t-a": "alpha", "t-b": {"state": "failed", "text": "boom"}
    }))
    .unwrap();
    assert!(Labels::from_json(&json!({"t": 3})).is_err());

    let cwd = temp_cwd("oracle");
    let mut host = FakeHost::default();
    let asked = [
        titled("1:aa", "t-a"),
        titled("1:bb", "t-b"),
        with_schema.clone(),
    ];
    let out = json!({"effects": {"pin": pin("h"), "spawn": asked}});
    let mut hf = job_host("s1", "t1", &cwd);
    hf.ceiling["max_live"] = json!(3);
    start(&mut host, hf, root_input(), out.clone());
    host.settle(&spawns(&out), |s| labels.answer(s));
    assert_eq!(child(&host, "1:aa"), ("completed".into(), "alpha".into()));
    assert_eq!(child(&host, "1:bb"), ("failed".into(), "boom".into()));
    assert_eq!(
        child(&host, "1:cc").1,
        "```json\n{\"n\":0,\"name\":\"x\",\"tags\":[]}\n```"
    );
    // A settled child is not answered again.
    host.settle(&spawns(&out), |_| Answer::ok("again"));
    assert_eq!(child(&host, "1:aa").1, "alpha");
}

#[test]
fn journal_oracle_answers_a_synthetic_journal() {
    // Other kinds are skipped; the schema spawn gets its value re-rendered as a json fence.
    let journal = [
        json!({"seq": 1, "kind": "log", "req_hash": "h0", "result": null, "at_ms": 1}),
        json!({"seq": 2, "kind": "spawn_agent", "req_hash": "h1", "result": {"output": "alpha"}, "at_ms": 2}),
        json!({"seq": 3, "kind": "spawn_agent", "req_hash": "h2", "result": {"output": {"n": 7}}, "at_ms": 3}),
        json!({"seq": 4, "kind": "budget", "req_hash": "h3", "result": {"spent": 2}, "at_ms": 4}),
    ]
    .map(|l| l.to_string())
    .join("\n");

    let mut oracle = Journal::from_jsonl(&journal).unwrap();
    let plain = json!({"prompt": "first"});
    let schema = json!({"prompt": "second\n\n<output-contract>\n{}\n</output-contract>"});
    assert_eq!(oracle.answer(&plain).text, "alpha");
    let fenced = oracle.answer(&schema);
    let body = fenced
        .text
        .strip_prefix("```json\n")
        .and_then(|t| t.strip_suffix("\n```"));
    assert_eq!(
        serde_json::from_str::<Value>(body.unwrap()).unwrap(),
        json!({"n": 7})
    );
    assert_eq!(
        oracle.answer(&plain).state,
        "failed",
        "a spent journal fails the spawn"
    );
}

#[test]
fn every_fault_can_be_injected() {
    let cwd = temp_cwd("faults");
    let hf = |tc: &str| {
        let mut hf = job_host("s1", tc, &cwd);
        hf.ceiling["max_live"] = json!(3);
        hf
    };
    let mut host = FakeHost::default();

    // Duplicate step: delivered twice, admitted once.
    let row = running_with_child(&mut host, hf("t1"), "1:aa");
    let step = call(Kind::JobStep, json!({"argv": ["run", "x"]}), hf("t1"));
    let again = json!({"effects": {"spawn": [spawn("1:aa", "wf-read-only")]}});
    let [a, b] = host.duplicate_step(&step, &row, |_| again.clone()).unwrap();
    assert!(a.continues && b.continues && a.code.is_none() && b.code.is_none());
    assert_eq!(host.jobs.chains[&row].charged, 1);

    // Crash: the in-memory counters go, the durable maps stay.
    later(&mut host, hf("t1"), &row, json!("garbage"));
    assert_eq!(host.jobs.chains[&row].failures, 1);
    let (rows, children) = (host.state.rows.clone(), host.state.children.clone());
    host.crash();
    assert_eq!(host.jobs.chains[&row].failures, 0);
    assert_eq!((&host.state.rows, &host.state.children), (&rows, &children));
    later(
        &mut host,
        hf("t1"),
        &row,
        json!({"effects": {"progress": {"title": "t"}}}),
    );

    // Child failure, interrupt and QueueFull as a failed terminal.
    let out = json!({"effects": {"pin": pin("h"), "spawn": [spawn("1:f", "wf-read-only"),
        spawn("1:i", "wf-read-only"), spawn("1:q", "wf-read-only")]}});
    start(&mut host, hf("t2"), root_input(), out);
    host.fail_child("1:f", 0, "bad").unwrap();
    host.interrupt_child("1:i", 0).unwrap();
    host.queue_full("1:q", 0).unwrap();
    assert_eq!(child(&host, "1:f"), ("failed".into(), "bad".into()));
    assert_eq!(child(&host, "1:i").0, "interrupted");
    assert_eq!(child(&host, "1:q"), ("failed".into(), "QueueFull".into()));
    assert!(host.fail_child("9:x", 0, "-").is_err());

    // Stop and pause mid-panel: live children end with the row.
    let a = running_with_child(&mut host, hf("t3"), "1:sa");
    let b = running_with_child(&mut host, hf("t4"), "1:sb");
    host.stop_row(&hf("t9"), &a).unwrap();
    host.pause_row(&hf("t9"), &b).unwrap();
    assert_eq!(host.state.rows[&a].state, JobState::Cancelled);
    assert_eq!(host.state.rows[&b].state, JobState::Paused);
    assert_eq!(child(&host, "1:sa").0, "interrupted");
    assert_eq!(child(&host, "1:sb").0, "interrupted");

    // Resume with a raised budget, and with an engine digest change: the block is echoed. The
    // second resumes the first's row, the chain's newest.
    let mut prior = b.clone();
    for (n, kind) in [(5, "resume"), (6, "engine_current")] {
        let block = if n == 5 {
            resume_with_budget(&prior, 99)
        } else {
            engine_changed(&prior)
        };
        let check = json!({"kind": kind, "payload": block.clone()});
        let out = json!({"result": "fin", "pin": pin("w"), "resume": block});
        let tc = format!("t{n}");
        start(&mut host, hf(&tc), json!({"argv": ["resume"]}), out);
        assert_eq!(host.jobs.rows[&format!("row-{tc}")].control, check);
        prior = format!("row-{tc}");
    }

    // Ceiling narrowed mid-run: the next spawn is refused.
    let row = running_with_child(&mut host, hf("t7"), "1:na");
    let narrow = narrowed(&hf("t7"), json!({"max_live": 1}));
    let more = json!({"effects": {"spawn": [spawn("1:nb", "wf-read-only")]}});
    let s = later(&mut host, narrow, &row, more);
    assert_eq!(s.code.as_deref(), Some("plugin_job_policy_denied"));

    // P2P twin notes: both rows come back from a query, in storage order.
    host.twin_notes("c1", "n1", &[("d9", "1"), ("d2", "1")]);
    let q = json!({"id": "q", "query": {"collection": "WfNote", "fields": ["_docID"],
                                        "filter": {"note_id": {"_eq": "n1"}}, "limit": 256}});
    let answers = ask_once(&mut host, Kind::JobStep, "data", q).unwrap();
    assert_eq!(
        answers["q"]["result"]["rows"],
        json!([{"_docID": "d9"}, {"_docID": "d2"}])
    );
}

#[test]
fn determinism_helper_detects_a_nondeterministic_guest() {
    let cwd = temp_cwd("determinism");
    let steady = |host: &mut FakeHost, crash: bool| {
        let out = json!({"effects": {"pin": pin("h"), "spawn": [spawn("1:aa", "wf-read-only")]}});
        let mut trace =
            vec![start(host, job_host("s1", "t1", &cwd), root_input(), out.clone()).output];
        if crash {
            host.crash();
        }
        host.settle(&spawns(&out), |_| Answer::ok("done"));
        trace.push(
            later(
                host,
                job_host("s1", "t1", &cwd),
                "row-t1",
                json!({"result": "fin"}),
            )
            .output,
        );
        trace
    };
    let trace = deterministic(steady).unwrap();
    assert_eq!(trace.len(), 2);
    assert_eq!(trace[1], json!({"result": "fin"}));

    let runs = std::cell::Cell::new(0);
    let drifting = |_: &mut FakeHost, _| {
        runs.set(runs.get() + 1);
        vec![json!({"effects": {"spawn": [spawn(&format!("1:{}", runs.get()), "wf-read-only")]}})]
    };
    let err = deterministic(drifting).unwrap_err();
    assert!(err.contains("run 2"), "{err}");
}

#[test]
fn cli_parses_arguments_and_prints_the_trace() {
    let args = |v: &[&str]| cli::parse(v.iter().map(|s| s.to_string()));
    let a = args(&[
        "--script", "x.rhai", "--args", "a.json", "--oracle", "l.json", "--trace",
    ])
    .unwrap();
    assert_eq!(
        (
            a.script.to_str(),
            a.args.as_deref().and_then(|p| p.to_str()),
            a.trace
        ),
        (Some("x.rhai"), Some("a.json"), true)
    );
    assert!(!args(&["--script", "x.rhai"]).unwrap().trace);
    assert!(args(&[]).unwrap_err().contains("--script"));
    assert!(args(&["--script"]).unwrap_err().contains("value"));
    assert!(
        args(&["--script", "x", "--nope"])
            .unwrap_err()
            .contains("--nope")
    );

    // A run end to end through a hand-written engine: step 0 spawns, the oracle answers it,
    // the next step reads the child's state and completes.
    let dir = std::path::PathBuf::from(temp_cwd("cli"));
    fs::write(dir.join("x.rhai"), "// script").unwrap();
    fs::write(dir.join("a.json"), r#"{"k": 1}"#).unwrap();
    fs::write(dir.join("l.json"), r#"{"t-a": "alpha"}"#).unwrap();
    let a = cli::Args {
        script: dir.join("x.rhai"),
        args: Some(dir.join("a.json")),
        oracle: Some(dir.join("l.json")),
        trace: true,
    };
    let engine = |input: Value| {
        if input.get("pin").is_some() {
            json!({"result": input["children"]["1:aa"]["state"]})
        } else {
            assert_eq!(input["args"], json!({"k": 1}), "the args file is echoed");
            let notes = [
                json!({"op": "insert", "collection": "WfNote", "fields": {"note_id": "n", "value_json": "1"}}),
            ];
            json!({"effects": {"pin": pin("h"), "spawn": [titled("1:aa", "t-a")],
                               "progress": {"title": "t"}, "writes": notes}})
        }
    };
    let shown = cli::run(&a, engine).unwrap();
    let lines: Vec<&str> = shown.lines().collect();
    assert!(
        lines[0].starts_with("step 0 spawn ") && lines[0].contains("t-a"),
        "{shown}"
    );
    assert!(lines.contains(&"step 0 note \"n\" \"1\""), "{shown}");
    assert!(
        lines.iter().any(|l| l.starts_with("step 0 progress ")),
        "{shown}"
    );
    assert_eq!(lines.last(), Some(&"result \"completed\""), "{shown}");
    let quiet = cli::run(&cli::Args { trace: false, ..a }, engine).unwrap();
    assert!(!quiet.contains(" spawn "), "{quiet}");
}
