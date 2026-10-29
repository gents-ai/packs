use serde_json::{Value, json};

use crate::clock::iso;
use crate::quotes::grep_fixed;
use crate::rules::{claim_id_ok, code_ref_ok, word_count};
use crate::run;

const NOW: &str = "2026-10-29T12:00:00Z";

fn fixture_root() -> String {
    format!("{}/tests/fixtures/research", env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/flipper/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn call(input: Value) -> Value {
    run(&input.to_string()).unwrap_or_else(|e| panic!("{e}"))
}

fn jsonl(lines: &[Value]) -> String {
    lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn claim() -> Value {
    json!({"id": "c:t:001", "text": "A test claim.", "quote": "A test claim", "from": "datapack.md#x",
        "sources": [{"doc": null, "url": null, "fetched": null}], "entities": ["e:a"], "kind": "fact",
        "by": "p:a", "asserted": "2026-10-22", "status": "live", "confidence": "memory", "note": ""})
}

fn with(mut value: Value, key: &str, field: Value) -> Value {
    if field.is_null() {
        value.as_object_mut().unwrap().remove(key);
    } else {
        value[key] = field;
    }
    value
}

/// A batch whose entities and perspectives cover `claim()`.
fn batch(claims: &[Value]) -> Value {
    json!({
        "batch_id": "b1", "section": "test", "gate_now": NOW, "data_calls": true,
        "claims": jsonl(claims),
        "entities": jsonl(&[json!({"id": "e:a", "type": "concept", "name": "A", "aliases": [], "note": ""})]),
        "perspectives": jsonl(&[json!({"id": "p:a", "name": "A", "kind": "position", "note": ""})]),
    })
}

fn errors(out: &Value) -> Vec<String> {
    serde_json::from_str(out["receipt"]["errors"].as_str().unwrap()).unwrap()
}

/// Runs the rounds against a store holding `held` (collection, field value).
fn drive(mut input: Value, held: &[(&str, &str)]) -> Value {
    for _ in 0..64 {
        let out = call(input.clone());
        let Some(calls) = out.get("data_calls") else {
            return out;
        };
        let mut results = serde_json::Map::new();
        for request in calls["requests"].as_array().unwrap() {
            let query = &request["query"];
            let field = query["fields"][0].as_str().unwrap();
            let wanted: Vec<&str> = query["filter"][field]["_in"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let rows: Vec<Value> = held
                .iter()
                .filter(|(c, v)| *c == query["collection"] && wanted.contains(v))
                .map(|(_, v)| json!({field: v}))
                .collect();
            results.insert(
                request["id"].as_str().unwrap().to_owned(),
                json!({"result": {"rows": rows}}),
            );
        }
        input["data_results"] = Value::Object(results);
        input["state"] = calls["state"].clone();
    }
    panic!("more than 64 rounds");
}

#[test]
fn claim_ids_follow_the_research_pattern() {
    for ok in ["c:flipper:001", "c:datapack-eu:120", "c:a1:999"] {
        assert!(claim_id_ok(ok), "{ok}");
    }
    for bad in ["c:flipper:1", "c:Flipper:001", "c::001", "x:a:001", "c:a:0011", "c:a_b:001", "c:a:001:"] {
        assert!(!claim_id_ok(bad), "{bad}");
    }
}

#[test]
fn code_refs_follow_the_research_pattern() {
    for ok in ["crates/db/src/a.rs:33-41", "design/federated-caps.md:464", "a_b.c", "x/y-z"] {
        assert!(code_ref_ok(ok), "{ok}");
    }
    for bad in ["", "a.rs:", "a.rs:1-", "a.rs:x", "a b.rs", "a.rs:1:2", ":3", "a.rs:1-2-3"] {
        assert!(!code_ref_ok(bad), "{bad}");
    }
}

#[test]
fn words_are_counted_on_single_spaces_like_jq_split() {
    assert_eq!(word_count("a  b c "), 3);
    assert_eq!(word_count(""), 0);
    assert_eq!(word_count("a\tb"), 1);
}

#[test]
fn quotes_match_like_grep_fixed_strings() {
    let file = b"first line\nsecond line\n";
    assert!(grep_fixed(file, "st li"));
    assert!(grep_fixed(file, "nothing here\nsecond"), "any line of a multi-line quote");
    assert!(!grep_fixed(file, "nothing\nhere"));
    assert!(grep_fixed(file, ""), "an empty pattern matches a nonempty file");
    assert!(!grep_fixed(b"", ""));
    assert!(!grep_fixed(b"ab", "abc"));
}

#[test]
fn checked_at_is_iso_utc() {
    assert_eq!(iso(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
    let now = crate::clock::now_iso();
    assert!(now.len() == 20 && now.ends_with('Z') && now.as_bytes()[10] == b'T', "{now}");
}

#[test]
fn the_first_round_asks_the_store_about_every_id_it_holds_or_names() {
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:old:001", "type": "supports", "by": "p:b", "date": "2026-10-22", "note": ""})]));
    let out = call(input);
    assert_eq!(
        out["data_calls"]["requests"],
        json!([
            {"id": "q0", "query": {"collection": "Claim", "filter": {"claim_id": {"_in": ["c:old:001", "c:t:001"]}}, "limit": 256, "fields": ["claim_id"]}},
            {"id": "q1", "query": {"collection": "Entity", "filter": {"entity_id": {"_in": ["e:a"]}}, "limit": 256, "fields": ["entity_id"]}},
            {"id": "q2", "query": {"collection": "Perspective", "filter": {"perspective_id": {"_in": ["p:a", "p:b"]}}, "limit": 256, "fields": ["perspective_id"]}},
            {"id": "q3", "query": {"collection": "Edge", "filter": {"edge_key": {"_in": ["c:t:001|supports|c:old:001"]}}, "limit": 256, "fields": ["edge_key"]}},
        ])
    );
    assert_eq!(out["data_calls"]["state"]["cursor"], 4);
    assert_eq!(out["data_calls"]["state"]["asked"], json!(["q0", "q1", "q2", "q3"]));
}

#[test]
fn a_clean_batch_passes_and_returns_every_row() {
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:old:001", "type": "supports", "by": "p:b", "date": "2026-10-22", "note": ""})]));
    let out = drive(input, &[("Claim", "c:old:001"), ("Perspective", "p:b")]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(
        out["receipt"],
        json!({"receipt_id": "receipt-b1", "verdict": "pass", "errors": "[]", "claims_admitted": 1,
            "edges_admitted": 1, "entities_admitted": 1, "perspectives_admitted": 1, "quotes_checked": 0,
            "quotes_skipped": 1, "gate_version": env!("CARGO_PKG_VERSION"), "checked_at": NOW})
    );
    assert_eq!(
        out["claims"],
        json!([{"claim_id": "c:t:001", "section": "test", "text": "A test claim.", "quote": "A test claim",
            "from_ref": "datapack.md#x", "sources": "[{\"doc\":null,\"url\":null,\"fetched\":null}]",
            "entities": ["e:a"], "entity_text": " e:a ", "kind": "fact", "by": "p:a", "asserted": "2026-10-22",
            "status": "live", "confidence": "memory", "note": ""}])
    );
    assert_eq!(
        out["edges"],
        json!([{"edge_key": "c:t:001|supports|c:old:001", "from_claim": "c:t:001", "to_claim": "c:old:001",
            "edge_type": "supports", "by": "p:b", "date": "2026-10-22", "note": ""}])
    );
    assert_eq!(
        out["entities"],
        json!([{"entity_id": "e:a", "entity_type": "concept", "name": "A", "alias_text": " a ", "note": ""}]),
        "an empty alias list is omitted, never written as []"
    );
    assert_eq!(
        out["perspectives"],
        json!([{"perspective_id": "p:a", "name": "A", "kind": "position", "note": ""}])
    );
    assert!(out.get("batch_id").is_none() && out["claims"][0].get("batch_id").is_none());
}

#[test]
fn optional_claim_fields_are_kept_and_source_key_order_is_preserved() {
    let c = with(
        with(with(claim(), "kind", json!("mapping")), "fit", json!("gap")),
        "against",
        json!("defradb"),
    );
    let c = with(with(c, "ref", json!("§5.1")), "valid_from", json!("2026-01-01"));
    let c = with(c, "sources", json!([{"code": "a.rs:1-2", "repo": "x@1"}]));
    let out = drive(batch(&[c]), &[]);
    let row = &out["claims"][0];
    assert_eq!(row["fit"], "gap");
    assert_eq!(row["against"], "defradb");
    assert_eq!(row["ref"], "§5.1");
    assert_eq!(row["valid_from"], "2026-01-01");
    assert!(row.get("valid_to").is_none());
    assert_eq!(row["sources"], "[{\"code\":\"a.rs:1-2\",\"repo\":\"x@1\"}]");
    let entity = json!({"id": "e:a", "type": "organisation", "name": "Federal Communications Commission", "aliases": ["FCC"], "note": ""});
    let mut input = batch(&[claim()]);
    input["entities"] = json!(entity.to_string());
    let out = drive(input, &[]);
    assert_eq!(out["entities"][0]["aliases"], json!(["FCC"]));
    assert_eq!(out["entities"][0]["alias_text"], " federal communications commission fcc ");
}

#[test]
fn each_claim_rule_fails_the_batch_in_the_research_wording() {
    let cases: Vec<(Value, &str)> = vec![
        (with(claim(), "id", json!("c:t:1")), "ERROR: c:t:1 bad id format"),
        (with(claim(), "id", Value::Null), "ERROR: ? bad id format"),
        (with(claim(), "quote", Value::Null), "ERROR: c:t:001 missing field quote"),
        (with(claim(), "sources", Value::Null), "ERROR: c:t:001 missing field sources"),
        (with(claim(), "text", json!(["x "; 41].concat())), "ERROR: c:t:001 text is 41 words (max 40)"),
        (with(claim(), "kind", json!("guess")), "ERROR: c:t:001 bad kind"),
        (with(claim(), "kind", Value::Null), "ERROR: c:t:001 bad kind"),
        (with(claim(), "status", json!("dead")), "ERROR: c:t:001 bad status"),
        (with(claim(), "confidence", json!("sure")), "ERROR: c:t:001 bad confidence"),
        (with(claim(), "kind", json!("mapping")), "ERROR: c:t:001 mapping without valid fit"),
        (with(claim(), "kind", json!("mapping")), "ERROR: c:t:001 mapping without valid against"),
        (with(claim(), "entities", json!([])), "ERROR: c:t:001 has no entities"),
        (with(claim(), "sources", json!([{"code": "a b.rs"}])), "ERROR: c:t:001 bad code ref a b.rs"),
        (with(claim(), "confidence", json!("sourced")), "ERROR: c:t:001 is sourced but cites no fetched doc or code ref"),
        (with(claim(), "colour", json!("red")), "ERROR: c:t:001 unknown field colour"),
        (with(claim(), "note", json!(3)), "ERROR: c:t:001 note must be a string"),
        (with(claim(), "entities", json!("e:a")), "ERROR: c:t:001 entities must be an array of strings"),
        (with(claim(), "sources", json!(["x"])), "ERROR: c:t:001 sources must be an array of objects"),
    ];
    for (c, want) in cases {
        let out = call(batch(&[c.clone()]));
        assert_eq!(out["receipt"]["verdict"], "fail", "{c}");
        let errs = errors(&out);
        assert!(errs.iter().any(|e| e == want), "{want} not in {errs:?}");
        assert!(out.get("claims").is_none() && out.get("entities").is_none());
        assert_eq!(out["receipt"]["claims_admitted"], 0);
    }
    let code_backed = with(claim(), "confidence", json!("sourced"));
    let code_backed = with(code_backed, "sources", json!([{"code": "a.rs:3", "repo": "r@1"}]));
    assert!(call(batch(&[code_backed])).get("data_calls").is_some(), "a code ref backs a sourced claim");
}

#[test]
fn lines_that_are_not_objects_are_named_by_field_and_line() {
    let mut input = batch(&[claim()]);
    input["claims"] = json!(format!("{}\n\n{{broken\n[1]\n", claim()));
    let errs = errors(&call(input));
    assert_eq!(
        errs,
        vec!["ERROR: claims:3 invalid JSON", "ERROR: claims:4 is not a JSON object"]
    );
}

#[test]
fn ids_repeated_in_the_batch_are_duplicates() {
    let mut input = batch(&[claim(), claim()]);
    let entity = json!({"id": "e:a", "type": "concept", "name": "A", "aliases": [], "note": ""});
    input["entities"] = json!(jsonl(&[entity.clone(), entity]));
    let edge = json!({"from": "c:t:001", "to": "c:t:001", "type": "supports", "by": "p:a", "date": "d", "note": ""});
    input["edges"] = json!(jsonl(&[edge.clone(), edge]));
    let errs = errors(&call(input));
    for want in [
        "ERROR: duplicate entity id e:a",
        "ERROR: duplicate claim id c:t:001",
        "ERROR: duplicate edge c:t:001|supports|c:t:001",
    ] {
        assert!(errs.iter().any(|e| e == want), "{want} not in {errs:?}");
    }
}

#[test]
fn each_edge_rule_fails_the_batch() {
    let edge = || json!({"from": "c:t:001", "to": "c:x:001", "type": "supports", "by": "p:a", "date": "2026-10-22", "note": ""});
    let cases = [
        (with(edge(), "date", Value::Null), "ERROR: edge c:t:001->c:x:001 missing date"),
        (with(edge(), "type", json!("likes")), "ERROR: edge c:t:001->c:x:001 bad type likes"),
        (with(edge(), "type", Value::Null), "ERROR: edge c:t:001->c:x:001 bad type null"),
        (with(edge(), "weight", json!(1)), "ERROR: edge c:t:001->c:x:001 unknown field weight"),
    ];
    for (e, want) in cases {
        let mut input = batch(&[claim()]);
        input["edges"] = json!(e.to_string());
        let errs = errors(&call(input));
        assert!(errs.iter().any(|x| x == want), "{want} not in {errs:?}");
    }
}

#[test]
fn references_are_resolved_against_the_batch_and_then_the_store() {
    let mut c = with(claim(), "entities", json!(["e:a", "e:held", "e:nowhere"]));
    c = with(c, "by", json!("p:ghost"));
    let mut input = batch(&[c]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:missing:001", "to": "c:t:001", "type": "supports", "by": "p:nobody", "date": "d", "note": ""})]));
    let out = drive(input, &[("Entity", "e:held")]);
    assert_eq!(
        errors(&out),
        vec![
            "ERROR: c:t:001 unknown perspective p:ghost",
            "ERROR: c:t:001 unknown entity e:nowhere",
            "ERROR: edge from unknown claim c:missing:001",
            "ERROR: edge c:missing:001->c:t:001 unknown perspective p:nobody",
        ]
    );
    assert_eq!(out["receipt"]["verdict"], "fail");
    assert!(out.get("claims").is_none());
}

#[test]
fn ids_already_in_the_store_are_duplicates() {
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:t:001", "type": "supports", "by": "p:a", "date": "d", "note": ""})]));
    let out = drive(
        input,
        &[("Claim", "c:t:001"), ("Entity", "e:a"), ("Perspective", "p:a"), ("Edge", "c:t:001|supports|c:t:001")],
    );
    assert_eq!(
        errors(&out),
        vec![
            "ERROR: duplicate entity id e:a (already in the store)",
            "ERROR: duplicate claim id c:t:001 (already in the store)",
            "ERROR: duplicate perspective id p:a (already in the store)",
            "ERROR: duplicate edge c:t:001|supports|c:t:001 (already in the store)",
        ]
    );
}

#[test]
fn entity_and_perspective_lines_need_an_id_and_known_string_fields() {
    let mut input = batch(&[claim()]);
    input["entities"] = json!(jsonl(&[json!({"id": "e:a", "aliases": "A"}), json!({"name": "no id"})]));
    input["perspectives"] = json!(jsonl(&[json!({"id": "p:a", "kind": 1, "colour": "red"})]));
    let errs = errors(&call(input));
    for want in [
        "ERROR: entity e:a aliases must be an array of strings",
        "ERROR: entities:2 missing id",
        "ERROR: perspective p:a kind must be a string",
        "ERROR: perspective p:a unknown field colour",
    ] {
        assert!(errs.iter().any(|e| e == want), "{want} not in {errs:?}");
    }
}

#[test]
fn a_batch_needs_lines_and_claims_need_a_section() {
    let errs = errors(&call(json!({"batch_id": "b", "gate_now": NOW, "data_calls": true})));
    assert_eq!(errs, vec!["ERROR: the batch carries no claims, edges, entities or perspectives"]);
    let mut input = batch(&[claim()]);
    input["section"] = json!("");
    assert!(errors(&call(input)).contains(&"ERROR: the batch has claims but no section".to_owned()));
}

#[test]
fn errors_are_capped_at_two_hundred() {
    let claims: Vec<Value> = (0..150)
        .map(|i| with(with(claim(), "id", json!(format!("c:t:{i:03}"))), "kind", json!("x")))
        .map(|c| with(c, "status", json!("x")))
        .collect();
    let errs = errors(&call(batch(&claims)));
    assert_eq!(errs.len(), 200);
    assert_eq!(errs[199], "ERROR: 101 more errors not shown");
}

#[test]
fn a_store_refusal_or_failure_ends_in_a_fail_receipt() {
    let first = call(batch(&[claim()]));
    let mut input = batch(&[claim()]);
    input["state"] = first["data_calls"]["state"].clone();
    input["data_results"] = json!({
        "q0": {"error": "collection \"Claim\" is outside the plugin's declared data grant"},
        "q1": {"result": {"rows": []}},
        "q2": {"result": {"rows": []}},
    });
    let out = call(input);
    assert_eq!(
        errors(&out),
        vec!["ERROR: store query on Claim failed: collection \"Claim\" is outside the plugin's declared data grant"]
    );
}

#[test]
fn a_full_answer_is_paged_with_offset() {
    let claims: Vec<Value> = (0..300)
        .map(|i| with(with(claim(), "quote", json!("FCC ID 2A2V6-FZ")), "id", json!(format!("c:t:{i:03}"))))
        .collect();
    let mut input = batch(&claims);
    input["source_root"] = json!(fixture_root());
    let first = call(input.clone());
    let requests = first["data_calls"]["requests"].as_array().unwrap();
    // Claim in two chunks, Entity, Perspective, Historical in two chunks.
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[4]["query"]["filter"]["edge_type"], json!({"_in": ["supersedes", "retracts"]}));
    let mut results = serde_json::Map::new();
    for r in requests {
        let id = r["id"].as_str().unwrap().to_owned();
        let rows: Vec<Value> = if id == "q4" {
            (0..256).map(|_| json!({"to_claim": "c:t:000"})).collect()
        } else {
            vec![]
        };
        results.insert(id, json!({"result": {"rows": rows}}));
    }
    input["data_results"] = Value::Object(results);
    input["state"] = first["data_calls"]["state"].clone();
    let second = call(input);
    assert_eq!(
        second["data_calls"]["requests"],
        json!([{"id": "q4-256", "query": {"collection": "Edge", "filter": {"to_claim": {"_in": claims[..256].iter().map(|c| c["id"].clone()).collect::<Vec<_>>()}, "edge_type": {"_in": ["supersedes", "retracts"]}}, "limit": 256, "offset": 256, "fields": ["to_claim"]}}])
    );
    assert_eq!(second["data_calls"]["state"]["found"]["historical"], json!(["c:t:000"]));
}

#[test]
fn rounds_carry_on_past_sixteen_queries() {
    let claims: Vec<Value> = (0..=999)
        .map(|i| with(claim(), "id", json!(format!("c:s{}:{:03}", i / 1000, i % 1000))))
        .collect();
    let mut input = batch(&claims);
    let ids: Vec<String> = (0..5000).map(|i| format!("e:{i}")).collect();
    input["entities"] = json!(jsonl(
        &ids.iter().map(|id| json!({"id": id})).chain([json!({"id": "e:a"})]).collect::<Vec<_>>()
    ));
    let first = call(input.clone());
    assert_eq!(first["data_calls"]["requests"].as_array().unwrap().len(), 16);
    let out = drive(input, &[]);
    assert_eq!(out["receipt"]["verdict"], "pass", "{}", out["receipt"]);
    assert_eq!(out["receipt"]["entities_admitted"], 5001);
}

#[test]
fn the_flipper_pilot_passes_with_its_quotes_checked() {
    let input = json!({
        "batch_id": "datapack-flipper-20261029-1", "section": "datapack-flipper", "gate_now": NOW,
        "data_calls": true, "source_root": fixture_root(),
        "claims": fixture("claims"), "entities": fixture("entities"), "perspectives": fixture("perspectives"),
        "extractor": "test", "sample_size": 10, "failure_count": 0, "note": "", "submitted_at": NOW,
        "a_field_the_host_added": 1,
    });
    let out = drive(input, &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["receipt"]["quotes_checked"], 10);
    assert_eq!(out["receipt"]["quotes_skipped"], 0);
    assert_eq!(out["claims"].as_array().unwrap().len(), 10);
    assert_eq!(out["entities"].as_array().unwrap().len(), 11);
}

#[test]
fn quotes_and_files_are_checked_under_the_source_root() {
    let base = |c: Value| {
        let mut input = batch(&[c]);
        input["source_root"] = json!(fixture_root());
        input
    };
    let errs = errors(&drive(base(with(claim(), "quote", json!("not in the narrative"))), &[]));
    assert_eq!(errs, vec!["ERROR: c:t:001 quote not found verbatim in datapack.md"]);
    let errs = errors(&call(base(with(claim(), "from", json!("missing.md#x")))));
    assert_eq!(
        errs,
        vec!["ERROR: file not found: missing.md", "ERROR: c:t:001 quote not found verbatim in missing.md"]
    );
    let escaping = with(claim(), "sources", json!([{"doc": "../outside.pdf"}]));
    assert_eq!(errors(&call(base(escaping))), vec!["ERROR: file not found: ../outside.pdf", "ERROR: c:t:001 quote not found verbatim in datapack.md"]);
    let real = "FCC ID 2A2V6-FZ, granted 14 Jan 2022 to Flipper Devices Inc.";
    let out = call(base(with(claim(), "quote", json!(real))));
    assert!(out.get("data_calls").is_some(), "{out}");
}

#[test]
fn questions_and_superseded_claims_are_not_quote_checked() {
    let live = with(claim(), "quote", json!("FCC ID 2A2V6-FZ"));
    let question = with(
        with(with(claim(), "id", json!("c:q:001")), "from", json!("graph/claims/questions.jsonl")),
        "quote",
        json!("Why?"),
    );
    let old = with(with(claim(), "id", json!("c:t:002")), "quote", json!("since corrected"));
    let mut input = batch(&[live, question, old]);
    input["source_root"] = json!(fixture_root());
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:t:002", "type": "supersedes", "by": "p:a", "date": "d", "note": ""})]));
    let out = drive(input.clone(), &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["receipt"]["quotes_checked"], 1);
    assert_eq!(out["receipt"]["quotes_skipped"], 2);
    let mut no_edge = input.clone();
    no_edge["edges"] = Value::Null;
    assert_eq!(
        errors(&drive(no_edge.clone(), &[])),
        vec!["ERROR: c:t:002 quote not found verbatim in datapack.md"]
    );
    let first = call(no_edge.clone());
    assert!(first.get("data_calls").is_some(), "a quote alone waits for the store");
    let out = drive(no_edge, &[("Edge", "c:t:002")]);
    assert_eq!(errors(&out), Vec::<String>::new(), "a supersedes edge in the store exempts it");
    assert_eq!(out["receipt"]["quotes_skipped"], 2);
}

#[test]
fn without_a_source_root_no_file_is_read_and_quotes_are_skipped() {
    let out = drive(batch(&[with(claim(), "from", json!("missing.md#x"))]), &[]);
    assert_eq!(out["receipt"]["verdict"], "pass");
    assert_eq!(out["receipt"]["quotes_checked"], 0);
    assert_eq!(out["receipt"]["quotes_skipped"], 1);
}

#[test]
fn malformed_host_input_is_an_error_not_a_receipt() {
    for (raw, want) in [
        ("not json", "invalid input"),
        ("[1]", "the input must be a JSON object"),
        ("{}", "batch_id is required"),
        ("{\"batch_id\": 3}", "batch_id must be a string"),
        ("{\"batch_id\": \"b\", \"claims\": 5}", "claims must be a string"),
        ("{\"batch_id\": \"b\", \"gate_now\": 5}", "gate_now must be a string"),
    ] {
        let e = run(raw).unwrap_err();
        assert!(e.contains(want), "{raw}: {e}");
    }
    let mut no_host = batch(&[claim()]);
    no_host["data_calls"] = Value::Null;
    assert!(run(&no_host.to_string()).unwrap_err().contains("no data_calls"));
    let mut bad_state = batch(&[claim()]);
    bad_state["data_results"] = json!({});
    bad_state["state"] = json!({"cursor": "x"});
    assert!(run(&bad_state.to_string()).unwrap_err().contains("state is not"));
    let first = call(batch(&[claim()]));
    let mut unanswered = batch(&[claim()]);
    unanswered["state"] = first["data_calls"]["state"].clone();
    unanswered["data_results"] = json!({});
    assert!(run(&unanswered.to_string()).unwrap_err().contains("no answer for data request q0"));
}
