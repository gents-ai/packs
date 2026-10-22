//! The data_calls and job_calls round client, driven by a fake host that answers the
//! requests the way the plugin round protocol does.
use serde_json::{Value, json};
use workflow::invoke;
use workflow::rounds::{MAX_ROUNDS, Step, advance};
use workflow::wire::{DataCall, JobCall, object};

const CHILD_TEXT: &str = "CHILD-TEXT-MARKER";

fn base_input() -> Value {
    json!({
        "protocol": 1,
        "data_calls": true,
        "job_calls": true,
        "job": {"row_doc_id": "r", "chain_root_doc_id": "c", "chain": ["c", "r"], "handle": "h"},
        "argv": ["run"],
        "pin": {"script_id": "s1", "source_digest": "d1", "agent_budget": 128, "handle": "h"},
        "ceiling": {"result_bytes": 4000},
        "children": {
            "1:aa": {"state": "completed"},
            "2:bb": {"state": "completed"},
            "3:cc": {"state": "live"}
        }
    })
}

fn calls_keys(out: &Value) -> Vec<&String> {
    out.as_object()
        .unwrap()
        .keys()
        .filter(|k| k.ends_with("_calls"))
        .collect()
}

/// Answers one output of the plugin: the next input is the base input plus that round's
/// results and the echoed state. `total` is how many note rows the chain holds.
fn serve(base: &Value, out: &Value, total: usize) -> Value {
    let mut next = base.clone();
    next["state"] = out[calls_keys(out)[0].as_str()]["state"].clone();
    if let Some(data) = out.get("data_calls") {
        let mut results = serde_json::Map::new();
        for r in data["requests"].as_array().unwrap() {
            let q = &r["query"];
            let rows: Vec<Value> = if q["collection"] == "WfNote" {
                let offset = q["offset"].as_u64().unwrap_or(0) as usize;
                let n = (q["limit"].as_u64().unwrap() as usize).min(total.saturating_sub(offset));
                (0..n)
                    .map(|i| json!({"_docID": format!("d{}", offset + i), "note_id": format!("n{}", offset + i), "value_json": "1", "row": "r"}))
                    .collect()
            } else {
                vec![json!({"source": "SOURCE"})]
            };
            results.insert(
                r["id"].as_str().unwrap().into(),
                json!({"result": {"rows": rows}}),
            );
        }
        next["data_results"] = Value::Object(results);
    } else {
        let job = &out["job_calls"];
        let mut results = serde_json::Map::new();
        for r in job["requests"].as_array().unwrap() {
            let slots: serde_json::Map<String, Value> = r["child_output"]["slots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s.as_str().unwrap().into(),
                        json!({"text": CHILD_TEXT, "truncated": false}),
                    )
                })
                .collect();
            results.insert(
                r["id"].as_str().unwrap().into(),
                json!({"result": {"slots": slots}}),
            );
        }
        next["job_results"] = Value::Object(results);
    }
    next
}

#[test]
fn no_output_requests_two_services() {
    let base = base_input();
    let mut input = base.clone();
    let mut asks = 0;
    // 300 notes forces a second data round before the job round.
    loop {
        match advance(&input).expect("advances") {
            Step::Ask(out) => {
                asks += 1;
                assert_eq!(calls_keys(&out).len(), 1, "one service per output: {out}");
                input = serve(&base, &out, 300);
            }
            Step::Replay(_) => break,
        }
    }
    assert_eq!(asks, 3);
    // The same holds when a round continuation goes through invoke.
    let Step::Ask(first) = advance(&base).unwrap() else {
        panic!("asks")
    };
    let out = invoke(serve(&base, &first, 300));
    assert_eq!(calls_keys(&out).len(), 1, "{out}");
}

#[test]
fn three_round_step_carries_notes_and_source_not_child_text() {
    let base = base_input();
    // Round 1: notes and the script source, together in one data_calls output.
    let Step::Ask(out1) = advance(&base).unwrap() else {
        panic!("round 1 asks")
    };
    let reqs = out1["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0]["query"]["collection"], "WfNote");
    assert_eq!(reqs[0]["query"]["limit"], 256);
    assert_eq!(reqs[1]["query"]["collection"], "WfScript");

    // Round 2: child text for the terminal slots with no out: note, limit = result_bytes.
    let mut input = serve(&base, &out1, 1);
    input["data_results"]["notes"]["result"]["rows"] =
        json!([{"_docID": "d1", "note_id": "out:2:bb", "value_json": "1", "row": "r"}]);
    let Step::Ask(out2) = advance(&input).unwrap() else {
        panic!("round 2 asks")
    };
    let child = &out2["job_calls"]["requests"][0]["child_output"];
    assert_eq!(child["slots"], json!(["1:aa"]));
    assert_eq!(child["limit"], 4000);
    assert_eq!(child["offset"], 0);
    let state = &out2["job_calls"]["state"];
    assert_eq!(state["source"], "SOURCE");
    assert_eq!(state["notes"][0]["note_id"], "out:2:bb");

    // Round 3: replay holds the notes and source from state and the text from the input.
    let input3 = serve(&base, &out2, 0);
    assert!(input3["job_results"].to_string().contains(CHILD_TEXT));
    let Step::Replay(replay) = advance(&input3).unwrap() else {
        panic!("round 3 replays")
    };
    assert_eq!(replay.source.as_deref(), Some("SOURCE"));
    assert_eq!(replay.notes.len(), 1);
    assert_eq!(replay.child_text["1:aa"], CHILD_TEXT);
    // Once the text is served, the carried state (echoed, then the notes and source the
    // replay holds) still does not carry it; only child_text does.
    assert!(!input3["state"].to_string().contains(CHILD_TEXT));
    assert!(!json!(replay.notes).to_string().contains(CHILD_TEXT));
    assert!(!replay.source.unwrap().contains(CHILD_TEXT));
}

#[test]
fn a_bad_child_output_answer_is_refused() {
    let base = base_input();
    let mut input = base.clone();
    input["state"] = json!({"notes_done": true, "source": "S"});
    let ask = |input: &Value| match advance(input).unwrap() {
        Step::Ask(out) => out,
        Step::Replay(_) => panic!("asks for child text"),
    };
    let out = ask(&input);
    let mut served = serve(&base, &out, 0);
    for bad in [
        json!({}),
        json!({"text": {}}),
        json!({"text": {"result": {"slots": {"1:aa": {"text": 7}}}}}),
        json!({"text": {"result": {}}}),
        json!({"text": {"result": null}}),
        // 1:aa and 2:bb were asked for; 2:bb is missing.
        json!({"text": {"result": {"slots": {"1:aa": {"text": "x"}}}}}),
    ] {
        served["job_results"] = bad.clone();
        let fail = advance(&served)
            .err()
            .unwrap_or_else(|| panic!("{bad} was accepted"));
        assert_eq!(fail["fail"]["code"], "workflow_invalid_input", "{bad}");
    }
}

#[test]
fn more_than_64_rounds_is_refused() {
    let base = base_input();
    let mut input = base.clone();
    let mut asks = 0;
    // The chain holds more notes than 64 pages carry.
    let fail = loop {
        match advance(&input) {
            Ok(Step::Ask(out)) => {
                asks += 1;
                input = serve(&base, &out, usize::MAX);
            }
            Ok(Step::Replay(_)) => panic!("an endless chain never replays"),
            Err(fail) => break fail,
        }
    };
    assert_eq!(asks, MAX_ROUNDS);
    assert_eq!(fail["fail"]["code"], "workflow_launch_failed");
    assert!(
        fail["fail"]["message"]
            .as_str()
            .unwrap()
            .contains(". Next: ")
    );
}

#[test]
fn unoffered_service_is_workflow_not_available() {
    let mut no_data = base_input();
    no_data.as_object_mut().unwrap().remove("data_calls");
    let fail = advance(&no_data).unwrap_err();
    assert_eq!(fail["fail"]["code"], "workflow_not_available");
    assert!(
        fail["fail"]["message"]
            .as_str()
            .unwrap()
            .contains(". Next: ")
    );

    // Notes arrive, but the host offers no job_calls for the child text.
    let base = base_input();
    let Step::Ask(out) = advance(&base).unwrap() else {
        panic!("asks")
    };
    let mut no_job = serve(&base, &out, 0);
    no_job.as_object_mut().unwrap().remove("job_calls");
    let fail = advance(&no_job).unwrap_err();
    assert_eq!(fail["fail"]["code"], "workflow_not_available");
    no_data["data_results"] = json!({});
    assert_eq!(invoke(no_data)["fail"]["code"], "workflow_not_available");
}

#[test]
fn requests_and_answers_follow_the_contract_shapes() {
    let base = base_input();
    // The first notes page omits `offset`; the next one carries it. `order` is a map.
    let Step::Ask(out1) = advance(&base).unwrap() else {
        panic!("round 1 asks")
    };
    let calls: Vec<DataCall> = serde_json::from_value(out1["data_calls"]["requests"].clone())
        .expect("data_calls requests are DataCalls");
    assert_eq!(calls[0].id, "notes");
    assert_eq!(calls[0].query.offset, None);
    assert!(
        out1["data_calls"]["requests"][0]["query"]
            .get("offset")
            .is_none()
    );
    assert_eq!(calls[0].query.order, Some(object(json!({"_docID": "ASC"}))));
    assert_eq!(calls[1].query.limit, 1);
    let Step::Ask(out2) = advance(&serve(&base, &out1, 256)).unwrap() else {
        panic!("a full page asks for the next one")
    };
    assert_eq!(out2["data_calls"]["requests"][0]["query"]["offset"], 256);

    // The child text request is a JobCall::ChildOutput; the answer may omit `truncated`.
    let mut input = base.clone();
    input["state"] = json!({"notes_done": true, "source": "S"});
    let Step::Ask(ask) = advance(&input).unwrap() else {
        panic!("asks for child text")
    };
    let call: JobCall = serde_json::from_value(ask["job_calls"]["requests"][0].clone()).unwrap();
    assert!(matches!(call, JobCall::ChildOutput { ref id, .. } if id == "text"));
    let mut served = serve(&base, &ask, 0);
    served["job_results"] = json!({"text": {"result": {"slots": {
        "1:aa": {"text": "1:aa"}, "2:bb": {"text": "2:bb"}}}}});
    let Step::Replay(replay) = advance(&served).unwrap() else {
        panic!("replays")
    };
    assert_eq!(replay.child_text["1:aa"], "1:aa");

    // A truncated slot's text passes through unchanged.
    served["job_results"]["text"]["result"]["slots"]["2:bb"]["truncated"] = json!(true);
    let Step::Replay(replay) = advance(&served).unwrap() else {
        panic!("replays")
    };
    assert_eq!(replay.child_text["2:bb"], "2:bb");

    // An unknown field, or both `result` and `error`, is not a round result.
    for bad in [
        json!({"text": {"result": {"slots": {}}, "extra": 1}}),
        json!({"text": {"result": {"slots": {}}, "error": "x"}}),
        json!({"text": {"result": {"slots": {"1:aa": {"text": "x", "extra": 1}}}}}),
    ] {
        served["job_results"] = bad.clone();
        let fail = advance(&served)
            .err()
            .unwrap_or_else(|| panic!("{bad} was accepted"));
        assert_eq!(fail["fail"]["code"], "workflow_invalid_input", "{bad}");
    }

    // A host error is the step's failure, with the error text.
    served["job_results"] = json!({"text": {"error": "boom"}});
    let fail = advance(&served).unwrap_err();
    assert_eq!(fail["fail"]["code"], "workflow_launch_failed");
    assert!(fail["fail"]["message"].as_str().unwrap().contains("(boom)"));
}
