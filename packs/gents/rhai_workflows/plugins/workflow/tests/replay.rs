//! The replay core: call sequencer, slot ids, divergence and the journal's notes.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use workflow::fakehost::{FakeHost, services};
use workflow::journal::{Journal, corr_id, out_id, run_id};
use workflow::replay::{HostFn, Sequencer, SlotKey, resolve, slot_id};
use workflow::rounds::{Replay, Step, advance};
use workflow::wire::Child;

fn key(target: &str) -> SlotKey {
    SlotKey {
        target: json!(target),
        prompt: "review the diff".into(),
        title: "review".into(),
        cwd: Some("/work".into()),
        schema: None,
    }
}

fn child(attempt: u64) -> Child {
    serde_json::from_value(json!({
        "attempt": attempt, "session_id": "s", "request_id": "q", "title": "t",
        "state": "live", "output_bytes": 0, "truncated": false,
        "usage": {"input": 0, "output": 0, "total": 0}, "duration_ms": 0
    }))
    .unwrap()
}

fn journal(notes: Vec<Value>) -> Journal {
    Journal::new(Replay {
        notes,
        source: None,
        child_text: BTreeMap::new(),
    })
}

#[test]
fn slot_ids_are_stable_and_cover_the_target() {
    let a = slot_id(3, None, &key("wf-read-only"));
    assert_eq!(a, slot_id(3, None, &key("wf-read-only")));
    let (prefix, hash) = a.split_once(':').unwrap();
    assert_eq!(prefix, "3");
    assert_eq!(hash.len(), 16);
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));

    // Only the target differs: same prefix, different hash.
    let b = slot_id(3, None, &key("wf-all"));
    assert_eq!(b.split_once(':').unwrap().0, "3");
    assert_ne!(a, b);

    // A parallel item is `<c>.<i>`, and no id passes 128 bytes.
    let item = slot_id(u64::MAX, Some(u64::MAX), &key("wf-all"));
    assert!(item.starts_with(&format!("{}.{}:", u64::MAX, u64::MAX)));
    assert!(item.len() <= 128);
}

#[test]
fn divergent_child_fails_at_its_call_without_spawn() {
    let was = slot_id(2, None, &key("wf-read-only"));
    let now = slot_id(2, None, &key("wf-all"));
    let mut children = BTreeMap::from([(was.clone(), child(1))]);

    // The same slot resolves to its latest attempt; an unseen call has no child yet.
    assert_eq!(
        resolve(&children, "2", &was, "wf")
            .unwrap()
            .unwrap()
            .attempt,
        1
    );
    assert!(
        resolve(&children, "3", &slot_id(3, None, &key("wf-all")), "wf")
            .unwrap()
            .is_none()
    );

    let out = resolve(&children, "2", &now, "wf").unwrap_err();
    assert_eq!(out["fail"]["code"], "workflow_validation_failed");
    let msg = out["fail"]["message"].as_str().unwrap();
    assert!(
        msg.contains("replay diverged at call 2. Next: workflow run wf"),
        "{msg}"
    );
    assert!(out.get("effects").is_none(), "{out}");

    // A parallel item diverges at its own prefix, and only there.
    let item = slot_id(5, Some(1), &key("wf-read-only"));
    children.insert(item, child(1));
    let moved = slot_id(5, Some(1), &key("wf-all"));
    assert!(resolve(&children, "5.1", &moved, "wf").is_err());
    let sibling = slot_id(5, Some(2), &key("wf-all"));
    assert!(resolve(&children, "5.2", &sibling, "wf").unwrap().is_none());

    // The same call switching between agent and parallel is a divergence at that call.
    let as_agent = slot_id(5, None, &key("wf-read-only"));
    let out = resolve(&children, "5", &as_agent, "wf").unwrap_err();
    assert!(out["fail"]["message"].as_str().unwrap().contains("call 5."));
    let was_agent = BTreeMap::from([(slot_id(6, None, &key("wf-read-only")), child(1))]);
    let as_item = slot_id(6, Some(0), &key("wf-read-only"));
    assert!(resolve(&was_agent, "6.0", &as_item, "wf").is_err());
    assert!(
        resolve(
            &was_agent,
            "7.0",
            &slot_id(7, Some(0), &key("wf-all")),
            "wf"
        )
        .is_ok()
    );
}

#[test]
fn p2p_twin_notes_resolve_to_lowest_doc_id() {
    let row =
        |doc: &str, value: &str| json!({"_docID": doc, "note_id": "await:1", "value_json": value});
    // The lowest _docID is neither first nor last.
    let j = journal(vec![
        row("bf2", "\"b\""),
        row("af1", "\"a\""),
        row("cf3", "\"c\""),
    ]);
    assert_eq!(j.get("await:1"), Some(&json!("a")));
    // Twins leave nothing to write.
    let mut j = j;
    assert!(!j.put("await:1", json!("other")));
    assert!(j.writes(true).is_empty());
}

#[test]
fn twin_notes_through_the_notes_query_resolve_to_lowest_doc_id() {
    let mut host = FakeHost::default();
    host.twin_notes(
        "c",
        "await:1",
        &[("bf2", "\"b\""), ("af1", "\"a\""), ("cf3", "\"c\"")],
    );
    let input = json!({"data_calls": true, "children": {},
                       "job": {"chain_root_doc_id": "c"}});
    let Ok(Step::Ask(out)) = advance(&input) else {
        panic!("the first round asks for the notes");
    };
    let query = &out["data_calls"]["requests"][0]["query"];
    let query = serde_json::from_value(query.clone()).unwrap();
    let rows = services::query(&host.state, &query, None).unwrap();
    let mut next = input.clone();
    next["state"] = out["data_calls"]["state"].clone();
    next["data_results"] = json!({"notes": {"result": rows}});
    let Ok(Step::Replay(replay)) = advance(&next) else {
        panic!("the notes are all in");
    };
    assert_eq!(Journal::new(replay).get("await:1"), Some(&json!("a")));
}

#[test]
fn a_notes_row_without_doc_id_is_refused() {
    let mut input = json!({"data_calls": true, "children": {}, "job": {"chain_root_doc_id": "c"},
        "data_results": {"notes": {"result": {"rows": [{"note_id": "n", "value_json": "1"}]}}}});
    assert!(advance(&input).is_err());
    input["data_results"]["notes"]["result"]["rows"][0]["_docID"] = json!("d1");
    assert!(advance(&input).is_ok());
}

#[test]
fn a_notes_row_with_a_missing_or_corrupt_value_is_refused() {
    let input = |row: Value| {
        json!({"data_calls": true, "children": {}, "job": {"chain_root_doc_id": "c"},
               "data_results": {"notes": {"result": {"rows": [row]}}}})
    };
    let row = |value: Value| {
        let mut row = json!({"_docID": "d1", "note_id": "out:1:aa"});
        if !value.is_null() {
            row["value_json"] = value;
        }
        row
    };
    for bad in [json!(null), json!("{not json"), json!(7)] {
        let Err(out) = advance(&input(row(bad))) else {
            panic!("a corrupt note must fail the step");
        };
        assert_eq!(out["fail"]["code"], "workflow_invalid_input");
        assert!(
            out["fail"]["message"]
                .as_str()
                .unwrap()
                .contains("value_json")
        );
    }
    assert!(
        advance(&input(row(json!("null")))).is_ok(),
        "a stored null reads"
    );
}

#[test]
fn pinless_output_carries_no_wfnote_write() {
    let mut j = journal(vec![
        json!({"_docID": "d1", "note_id": "run:r", "value_json": "{}"}),
    ]);
    assert!(
        !j.put(&run_id("r"), json!({"again": 1})),
        "a held note is not rewritten"
    );
    assert!(j.put(&out_id("1:aa"), json!({"value": 1, "success": true})));
    assert!(j.put(&corr_id("1:aa", 1), json!("d1")));
    assert!(
        !j.put(&out_id("1:aa"), json!({"value": 2})),
        "the first value is kept"
    );

    assert!(j.writes(false).is_empty(), "no pin, no WfNote");
    let writes = j.writes(true);
    assert_eq!(writes.len(), 2);
    assert_eq!(
        writes[0],
        json!({"op": "insert", "collection": "WfNote", "on_conflict": "ignore",
               "fields": {"note_id": "out:1:aa", "value_json": "{\"success\":true,\"value\":1}"}})
    );
    assert_eq!(writes[1]["fields"]["note_id"], "corr:1:aa#1");
}

#[test]
fn pure_functions_take_no_call_index() {
    let mut seq = Sequencer::default();
    let journaled = [
        HostFn::Agent,
        HostFn::Parallel,
        HostFn::Budget,
        HostFn::AwaitUser,
        HostFn::WriteScratchFile,
        HostFn::ReadScratchFile,
    ];
    let pure = [
        HostFn::Phase,
        HostFn::Log,
        HostFn::Fingerprint,
        HostFn::JsonEncode,
        HostFn::RenderTemplate,
    ];
    for (want, f) in journaled.into_iter().enumerate() {
        for p in pure {
            assert_eq!(seq.call(p), None);
        }
        assert_eq!(seq.call(f), Some(want as u64));
    }
    // A replay starts again at 0.
    assert_eq!(Sequencer::default().call(HostFn::Agent), Some(0));
}
