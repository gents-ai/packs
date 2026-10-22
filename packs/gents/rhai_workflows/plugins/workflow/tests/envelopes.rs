//! Wire envelopes and the input-shape router of the workflow engine.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use workflow::wire::{
    CatalogEntry, ChildOutputResponse, DataCall, DataQueryResponse, DataWrite, JobCall,
    JobProgress, JobStepInput, JobStepOutput, RawInput, RoundResult,
};
use workflow::{Shape, classify, invoke};

fn round_trip<T: Serialize + DeserializeOwned>(v: Value) {
    let parsed: T = serde_json::from_value(v.clone()).expect("parses");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
}

fn pin() -> Value {
    json!({"script_id": "s1", "source_digest": "d1", "agent_budget": 128, "handle": "h",
           "origin": "installed", "engine_digest": "e1", "cwd": "/w", "plugin_ref": "pr",
           "adopted_at": "2026-10-14T00:00:00Z", "args_digest": "ad", "chain_root_doc_id": "c",
           "resume_of": "r0", "replayed_from": "r1", "ceiling_at_pin": {"max_live": 4}})
}

fn catalog_entry() -> Value {
    json!({"name": "n", "description": "d", "when_to_use": "w", "source": "kept", "path": "/p"})
}

fn raw_input() -> Value {
    json!({"argv": ["list"], "asset_mounts": [], "data_calls": true})
}

fn progress() -> Value {
    json!({"title": "t", "state": "paused", "pause_reason": "budget_exceeded",
           "phases": [{"title": "p", "state": "active"}],
           "active": [{"slot": "1:ab", "title": "a", "session_id": "s"}],
           "spawned": 2, "budget": {"total": 10, "spent": 3, "remaining": 7},
           "last_event": "e", "last_event_at": "2026-10-14T00:00:00Z",
           "question": "q", "observed_terminal_children": 1, "revision": 4})
}

fn write() -> Value {
    json!({"op": "insert", "collection": "WfNote", "doc_id": "d",
           "fields": {"a": 1}, "on_conflict": "ignore"})
}

/// The round request and answer shapes, as the host's `rounds` module reads and writes them.
fn rounds() -> Vec<(&'static str, Value)> {
    vec![
        (
            "data_call",
            json!({"id": "notes", "query": {"collection": "WfNote",
                "filter": {"chain_root_doc_id": {"_eq": "c"}}, "order": {"_docID": "ASC"},
                "limit": 256, "offset": 256, "fields": ["_docID"]}}),
        ),
        (
            "data_call_bare",
            json!({"id": "s", "query": {"collection": "WfScript", "filter": {}, "limit": 1,
                "fields": ["source"]}}),
        ),
        (
            "job_call_child_output",
            json!({"id": "text", "child_output": {"slots": ["1:ab"], "offset": 0, "limit": 9}}),
        ),
        (
            "job_call_result",
            json!({"id": "r", "result": {"row_doc_id": "r", "offset": 0, "limit": 9}}),
        ),
        ("data_rows", json!({"result": {"rows": [{"_docID": "d"}]}})),
        (
            "child_output",
            json!({"result": {"slots": {"1:ab": {"text": "t", "truncated": true}, "2:ab": {"text": "u"}}}}),
        ),
        ("error", json!({"error": "boom"})),
    ]
}

fn full_input() -> Value {
    json!({
        "protocol": 1,
        "job": {"row_doc_id": "r", "chain_root_doc_id": "c", "chain": ["c", "r"], "handle": "h"},
        "caller": {"session_id": "s", "request_id": "q", "request_doc_id": "qd",
                   "tool_call_id": "tc", "node_did": "did:n", "agent_id": "a",
                   "request_hop": 1, "correlation": null, "cwd": "/w", "started_by_job": null},
        "argv": ["run", "x"], "args": {"k": 1}, "path": "p.rhai", "script": "1", "agent_budget": 5,
        "root_call": {"argv": ["run"], "args": {"k": 2}, "path": "p.rhai", "agent_budget": 9},
        "pin": pin(), "origin": "dynamic",
        "verified_script": {"source": "1", "digest": "d"},
        "mounts": [{"pack": "pk", "class": "workflows", "digest": "dg", "path": "/m", "trusted": true}],
        "ceiling": {"targets": ["t"], "max_live": 4, "remaining_spawns": 9,
                    "max_messages_per_slot": 2, "result_bytes": 100,
                    "dynamic": true, "dynamic_targets": ["t"]},
        "children": {"1:ab": {"attempt": 0, "session_id": "s", "request_id": "q", "title": "t",
                              "state": "done", "output_bytes": 3, "truncated": false,
                              "usage": {"input": 1, "output": 2, "total": 3}, "duration_ms": 7}},
        "control": {"kind": "answer", "payload": {"row_doc_id": "r", "answer": "y"}},
        "scratch": {"f": "/w/.gents/workflows/runs/tc/f"},
        "jobs": [{"row_doc_id": "r", "tool_call_id": "tc", "handle": "h", "state": "running", "adopted_at": 7, "resume_of": null}]
    })
}

fn outputs() -> Vec<Value> {
    vec![
        json!({"result": {"x": 1}, "writes": [write()], "scratch": [{"name": "f", "content": "c"}],
               "progress": progress(), "control": [{"row_doc_id": "r", "kind": "stop"}], "pin": pin()}),
        json!({"result": null}),
        json!({"fail": {"code": "workflow_invalid_input", "message": "m"},
               "writes": [write()], "progress": progress()}),
        json!({"suspend": {"kind": "await_user", "reason": "user", "message": "m", "question": "q"},
               "writes": [write()], "scratch": [{"name": "f", "content": "c"}],
               "progress": progress(), "pin": pin()}),
        json!({"effects": {"pin": pin(), "resume": {"row_doc_id": "r", "engine": "current",
               "answer": "a", "budget": 9},
               "spawn": [{"slot": "1:ab", "attempt": 0, "target": "t", "prompt": "p",
                          "title": "t", "cwd": "/w"}],
               "message": [{"slot": "1:ab", "text": "x"}], "interrupt": [{"slot": "1:ab"}],
               "progress": progress(), "writes": [write()],
               "scratch": [{"name": "f", "content": "c"}]}}),
        json!({"effects": {}}),
    ]
}

fn field_names(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => m.iter().for_each(|(k, v)| {
            out.push(k.clone());
            field_names(v, out);
        }),
        Value::Array(a) => a.iter().for_each(|v| field_names(v, out)),
        _ => {}
    }
}

#[test]
fn job_step_input_and_outputs_round_trip() {
    round_trip::<JobStepInput>(full_input());
    for o in outputs() {
        round_trip::<JobStepOutput>(o);
    }
    round_trip::<JobProgress>(progress());
    round_trip::<DataWrite>(write());
    round_trip::<CatalogEntry>(catalog_entry());
    round_trip::<RawInput>(raw_input());
    for (name, v) in rounds() {
        match name {
            n if n.starts_with("data_call") => round_trip::<DataCall>(v),
            n if n.starts_with("job_call") => round_trip::<JobCall>(v),
            "data_rows" => round_trip::<RoundResult<DataQueryResponse>>(v),
            "child_output" => round_trip::<RoundResult<ChildOutputResponse>>(v),
            _ => round_trip::<RoundResult<DataQueryResponse>>(v),
        }
    }
}

#[test]
fn round_shapes_reject_unknown_fields() {
    for (name, mut v) in rounds() {
        v["extra"] = json!(1);
        let bad = match name {
            n if n.starts_with("data_call") => serde_json::from_value::<DataCall>(v).is_err(),
            n if n.starts_with("job_call") => serde_json::from_value::<JobCall>(v).is_err(),
            _ => serde_json::from_value::<RoundResult<Value>>(v).is_err(),
        };
        assert!(bad, "{name} accepted an unknown field");
    }
}

#[test]
fn no_contract_field_name_holds_a_secret_word() {
    let mut names = Vec::new();
    field_names(&full_input(), &mut names);
    outputs().iter().for_each(|o| field_names(o, &mut names));
    for v in [catalog_entry(), raw_input()] {
        field_names(&v, &mut names);
    }
    rounds()
        .iter()
        .for_each(|(_, v)| field_names(v, &mut names));
    // Map keys such as slot ids and scratch names are data, not contract names.
    for n in names {
        let n = n.to_ascii_lowercase();
        for w in ["key", "secret", "token", "password"] {
            assert!(!n.contains(w), "contract field `{n}` holds `{w}`");
        }
    }
}

#[test]
fn invoke_routes_each_input_shape() {
    let inputs = [
        (json!({"protocol": 1, "argv": ["run"]}), Shape::JobStep),
        (
            json!({"protocol": 1, "argv": ["run"], "data_results": [], "state": {}}),
            Shape::Round,
        ),
        (
            json!({"protocol": 1, "job_results": [], "state": {}}),
            Shape::Round,
        ),
        // A job verb on the raw shape: help, check and list are served, so only this refuses.
        (
            json!({"argv": ["run", "x"], "data_calls": true}),
            Shape::Raw,
        ),
        (json!({"source": "1"}), Shape::Callback),
    ];
    let mut messages = Vec::new();
    for (input, shape) in inputs {
        assert_eq!(classify(&input), shape);
        let fail = &invoke(input)["fail"];
        assert_eq!(fail["code"], "workflow_not_available");
        messages.push(fail["message"].as_str().unwrap().to_string());
    }
    assert!(messages.iter().all(|m| m.contains(". Next: ")));
    // The four branches answer in their own words.
    messages.sort();
    messages.dedup();
    assert_eq!(messages.len(), 4);
}
