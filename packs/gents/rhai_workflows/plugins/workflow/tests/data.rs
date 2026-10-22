//! The typed pack-data layer: paging, own-node reads and the write rules, on the fake host.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use workflow::data::{self, PAGE, WfName, WfNote, WfScript};
use workflow::fakehost::cli::facts;
use workflow::fakehost::services::query;
use workflow::fakehost::state::State;
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;
use workflow::rounds::{Step, advance};

/// The fields the host stamps; a guest value for any of them is rejected.
const STAMPED: [&str; 5] = [
    "node_did",
    "session_id",
    "tool_call_doc_id",
    "chain_root_doc_id",
    "job_row_doc_id",
];

fn rows_of(state: &State, call: &workflow::wire::DataCall, did: &str) -> Vec<Value> {
    let answer = query(state, &call.query, Some(did)).expect("the fake host answers");
    answer["rows"].as_array().cloned().unwrap_or_default()
}

fn seeded(collection: &str, rows: Vec<Value>) -> State {
    let mut state = State::default();
    state.collections.insert(collection.into(), rows);
    state
}

#[test]
fn queries_page_past_256_rows() {
    let mut rows: Vec<Value> = (0..600)
        .map(|i| {
            json!({"_docID": format!("d{i:04}"), "note_id": format!("n{i}"),
                        "value_json": "1", "row": "r", "chain_root_doc_id": "c"})
        })
        .collect();
    rows.push(
        json!({"_docID": "d9999", "note_id": "other", "value_json": "1",
                     "row": "r", "chain_root_doc_id": "elsewhere"}),
    );
    let state = seeded("WfNote", rows);

    let first = data::notes_page("c", 0);
    assert_eq!(first.query.limit, PAGE);
    assert_eq!(first.query.offset, None);

    // The production owner pages: `advance` asks again while a page comes back full.
    let base = json!({
        "protocol": 1, "data_calls": true,
        "job": {"row_doc_id": "r", "chain_root_doc_id": "c", "chain": ["c", "r"], "handle": "h"},
        "argv": ["run"],
    });
    let mut input = base.clone();
    let mut offsets = Vec::new();
    let notes = loop {
        match advance(&input).expect("advance accepts the round") {
            Step::Ask(out) => {
                let requests = out["data_calls"]["requests"].as_array().unwrap();
                assert_eq!(requests.len(), 1, "{out}");
                let call: workflow::wire::DataCall =
                    serde_json::from_value(requests[0].clone()).unwrap();
                offsets.push(call.query.offset);
                let page = rows_of(&state, &call, "did:a");
                input = base.clone();
                input["state"] = out["data_calls"]["state"].clone();
                input["data_results"] = json!({"notes": {"result": {"rows": page}}});
            }
            Step::Replay(replay) => break replay.notes,
        }
    };
    assert_eq!(offsets, [None, Some(256), Some(512)]);
    let got: Vec<&str> = notes
        .iter()
        .map(|r| r["_docID"].as_str().unwrap())
        .collect();
    assert_eq!(got.len(), 600, "every row of the chain, none of another");
    assert!(
        got.windows(2).all(|w| w[0] < w[1]),
        "ordered by _docID, no repeats"
    );
    assert_eq!(
        data::limited(data::notes_page("c", 0), 1000).query.limit,
        PAGE
    );
}

#[test]
fn own_node_reads_use_caller_node_did() {
    let row =
        |node: &str, source: &str| json!({"script_id": "s1", "node_did": node, "source": source});
    let state = seeded(
        "WfScript",
        vec![row("did:a", "mine"), row("did:b", "theirs")],
    );

    let call = data::script_source("s1");
    assert!(
        json!(call.query.filter)
            .to_string()
            .contains("$caller.node_did"),
        "{:?}",
        call.query.filter
    );
    assert_eq!(rows_of(&state, &call, "did:a"), [json!({"source": "mine"})]);
    assert_eq!(
        rows_of(&state, &call, "did:b"),
        [json!({"source": "theirs"})]
    );

    let note = data::own_note("run", "run:r1");
    assert!(
        json!(note.query.filter)
            .to_string()
            .contains("$caller.node_did")
    );
    assert_eq!(note.query.limit, 1);
}

#[test]
fn no_guest_write_carries_a_stamped_field() {
    let script = WfScript::draft("s1", "src", "inline", "demo", "about");
    let name = WfName {
        name: "demo".into(),
        version: 1,
        script_id: "s1".into(),
        description: "about".into(),
        when_to_use: "always".into(),
        state: "kept".into(),
    };
    let note = WfNote {
        note_id: "run:r1".into(),
        value_json: "{}".into(),
        row: Some("r1".into()),
    };
    let writes = [script.write(), name.write(), note.write()];
    for w in &writes {
        let fields = w["fields"].as_object().unwrap();
        assert!(STAMPED.iter().all(|s| !fields.contains_key(*s)), "{w}");
        assert_eq!(w["op"], "insert");
    }
    let policy = |w: &Value| w["on_conflict"].clone();
    assert_eq!(policy(&writes[0]), "ignore");
    assert_eq!(policy(&writes[1]), "fail");
    assert_eq!(policy(&writes[2]), "ignore");

    // The real run path writes through the layer, and the host refuses any stamped field.
    let source = "let meta = #{ name: \"demo\", description: \"d\" };\ncomplete(1);\n";
    let call = Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": source}),
        host: facts("/work"),
    };
    let mut host = FakeHost::default();
    host.step0(&call, invoke)
        .expect("the host accepts the run's writes");
    assert_eq!(host.state.collections["WfScript"].len(), 1);
}

#[test]
fn a_wfname_conflict_surfaces_as_an_error() {
    let name = WfName {
        name: "demo".into(),
        version: 1,
        script_id: "s1".into(),
        description: "about".into(),
        when_to_use: "always".into(),
        state: "kept".into(),
    };
    let stamps = BTreeMap::from([("node_did".to_string(), "did:a".to_string())]);
    let mut state = State::default();
    state.apply_writes(&json!([name.write()]), &stamps).unwrap();
    let err = state
        .apply_writes(&json!([name.write()]), &stamps)
        .unwrap_err();
    assert!(err.contains("WfName violates unique index"), "{err}");
}
