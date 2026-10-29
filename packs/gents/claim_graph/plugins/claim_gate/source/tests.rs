use serde_json::{Value, json};

use crate::clock::iso;
use crate::profile::parse_inline;
use crate::quotes::grep_fixed;
use crate::rules::{code_ref_ok, word_count};
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

/// The research vocabulary, with `p:a` as its resolver.
fn research() -> Value {
    json!({
        "display_name": "Test graph",
        "kinds": ["fact", "number", "date", "rule", "mapping", "interpretation", "position", "quote", "question", "challenge", "review"],
        "confidence_classes": ["sourced", "memory", "inferred"],
        "edge_types": ["supports", "contradicts", "supersedes", "retracts", "answers", "maps-to", "said-first", "depends-on", "challenges", "reviews", "resolves"],
        "mapping_targets": ["defradb", "fefra"],
        "mapping_fits": ["native", "partial", "gap", "convention"],
        "review_verdicts": ["sustains", "weakens", "unclear"],
        "resolver_perspectives": ["p:a"],
        "question_entity": "e:open-question",
        "default_asker": "p:a",
        "questions_from_prefix": "graph/claims/questions.jsonl",
        "claim_id_pattern": "^c:[a-z0-9-]+:[0-9]{3}$",
        "text_max_words": 40,
        "note": ""
    })
}

/// `research()` as the store keeps it: version `version` of graph `g`.
fn stored(version: i64) -> Value {
    let mut row = research();
    row["version"] = json!(version);
    row
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

/// A batch of graph `g` with an inline profile, whose entities and
/// perspectives cover `claim()`.
fn batch(claims: &[Value]) -> Value {
    json!({
        "batch_id": "b1", "graph_id": "g", "section": "test", "gate_now": NOW, "data_calls": true,
        "profile": research().to_string(),
        "claims": jsonl(claims),
        "entities": jsonl(&[json!({"id": "e:a", "type": "concept", "name": "A", "aliases": [], "note": ""})]),
        "perspectives": jsonl(&[json!({"id": "p:a", "name": "A", "kind": "position", "note": ""})]),
    })
}

fn errors(out: &Value) -> Vec<String> {
    serde_json::from_str(out["receipt"]["errors"].as_str().unwrap()).unwrap()
}

/// Runs the rounds against a store holding `held` (collection, field value)
/// in graph `g`, with no profile stored.
fn drive(input: Value, held: &[(&str, &str)]) -> Value {
    drive_store(input, held, &[], None)
}

/// Runs the rounds against a store holding `held`, the claims `kinds`
/// (id, kind) and the ClaimGraph row `profile`, all in graph `g`.
fn drive_store(mut input: Value, held: &[(&str, &str)], kinds: &[(&str, &str)], profile: Option<Value>) -> Value {
    for _ in 0..64 {
        let out = call(input.clone());
        let Some(calls) = out.get("data_calls") else {
            return out;
        };
        let mut results = serde_json::Map::new();
        for request in calls["requests"].as_array().unwrap() {
            let query = &request["query"];
            let rows: Vec<Value> = if query["collection"] == "ClaimGraph" {
                assert_eq!(query["filter"], json!({"graph_id": {"_eq": "g"}}));
                profile.clone().into_iter().collect()
            } else {
                let clauses = query["filter"]["_and"].as_array().unwrap();
                assert_eq!(clauses[0], json!({"graph_id": {"_eq": "g"}}));
                let field = query["fields"][0].as_str().unwrap();
                let wanted: Vec<&str> = clauses[1][field]["_in"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                let mut rows: Vec<Value> = held
                    .iter()
                    .filter(|(c, v)| *c == query["collection"] && wanted.contains(v))
                    .map(|(_, v)| json!({field: v}))
                    .collect();
                if query["collection"] == "Claim" {
                    rows.extend(
                        kinds
                            .iter()
                            .filter(|(id, _)| wanted.contains(id))
                            .map(|(id, kind)| json!({"claim_id": id, "kind": kind})),
                    );
                }
                rows
            };
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
fn claim_ids_follow_the_profile_pattern() {
    let profile = parse_inline(&research().to_string()).unwrap();
    for ok in ["c:flipper:001", "c:datapack-eu:120", "c:a1:999"] {
        assert!(profile.claim_id_ok(ok), "{ok}");
    }
    for bad in ["c:flipper:1", "c:Flipper:001", "c::001", "x:a:001", "c:a:0011", "c:a_b:001", "c:a:001:"] {
        assert!(!profile.claim_id_ok(bad), "{bad}");
    }
    let other = parse_inline(&with(research(), "claim_id_pattern", json!("^[A-Z]{2}-[0-9]+$")).to_string()).unwrap();
    assert!(other.claim_id_ok("AB-12") && !other.claim_id_ok("c:t:001"));
}

#[test]
fn an_inline_profile_is_checked_for_shape() {
    let errs = |p: Value| parse_inline(&p.to_string()).unwrap_err();
    assert_eq!(
        errs(with(research(), "kinds", json!([]))),
        vec!["ERROR: profile kinds must be a non-empty list of distinct strings"]
    );
    assert_eq!(
        errs(with(research(), "edge_types", json!(["supports", "supports"]))),
        vec!["ERROR: profile edge_types must be a non-empty list of distinct strings"]
    );
    assert_eq!(
        errs(with(research(), "mapping_targets", json!("defradb"))),
        vec!["ERROR: profile mapping_targets must be a list of distinct strings"]
    );
    assert_eq!(
        errs(with(research(), "review_verdicts", json!(["a", "a"]))),
        vec!["ERROR: profile review_verdicts must be a list of distinct strings"]
    );
    assert_eq!(
        errs(with(research(), "confidence_classes", Value::Null)),
        vec!["ERROR: profile confidence_classes must be a non-empty list of distinct strings"]
    );
    assert!(errs(with(research(), "claim_id_pattern", json!("c:(")))[0]
        .starts_with("ERROR: profile claim_id_pattern does not compile: "));
    for words in [json!(0), json!(401), json!("40"), json!(4.5)] {
        assert_eq!(
            errs(with(research(), "text_max_words", words)),
            vec!["ERROR: profile text_max_words must be an integer from 1 to 400"]
        );
    }
    assert_eq!(
        errs(with(with(research(), "version", json!(2)), "colour", json!("red"))),
        vec!["ERROR: profile sets version, which the gate assigns", "ERROR: profile unknown field colour"]
    );
    assert_eq!(errs(with(research(), "note", json!(1))), vec!["ERROR: profile note must be a string"]);
    assert_eq!(parse_inline("[1]").unwrap_err(), vec!["ERROR: profile is not a JSON object"]);
    assert_eq!(parse_inline("{").unwrap_err(), vec!["ERROR: profile invalid JSON"]);
    assert!(parse_inline(&with(research(), "text_max_words", json!(400)).to_string()).is_ok());
}

#[test]
fn a_bad_inline_profile_fails_the_batch_at_once() {
    let mut input = batch(&[claim()]);
    input["profile"] = json!(with(research(), "kinds", json!(["fact", "fact"])).to_string());
    let out = call(input);
    assert_eq!(
        errors(&out),
        vec!["ERROR: profile kinds must be a non-empty list of distinct strings"]
    );
    assert!(out["receipt"].get("profile_version").is_none());
    assert_eq!(out["receipt"]["graph_id"], "g");
}

#[test]
fn a_batch_must_name_its_graph() {
    let mut input = batch(&[claim()]);
    input["graph_id"] = Value::Null;
    let out = call(input);
    assert_eq!(errors(&out), vec!["ERROR: the batch names no graph_id"]);
    assert!(out["receipt"].get("graph_id").is_none());
}

#[test]
fn the_first_batch_admits_its_profile_as_version_one() {
    let out = drive(batch(&[claim()]), &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["receipt"]["profile_version"], 1);
    let mut want = research();
    want["graph_id"] = json!("g");
    want["version"] = json!(1);
    want["created_at"] = json!(NOW);
    assert_eq!(out["profile"], want);
    assert!(out["profile"].get("batch_id").is_none());
}

#[test]
fn an_inline_profile_over_a_stored_one_is_the_next_version() {
    let out = drive_store(batch(&[claim()]), &[], &[], Some(stored(3)));
    assert_eq!(out["receipt"]["profile_version"], 4);
    assert_eq!(out["profile"]["version"], 4);
}

#[test]
fn without_an_inline_profile_the_latest_stored_version_decides() {
    let mut input = batch(&[with(claim(), "kind", json!("guess"))]);
    input["profile"] = Value::Null;
    let first = call(input.clone());
    assert_eq!(first["data_calls"]["requests"][0]["query"]["collection"], "ClaimGraph", "the vocabulary waits for the store");
    let mut v3 = stored(3);
    v3["kinds"] = json!(["fact", "guess"]);
    let out = drive_store(input.clone(), &[], &[], Some(v3));
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["receipt"]["profile_version"], 3);
    assert!(out.get("profile").is_none(), "a stored profile is not admitted again");
    let out = drive_store(input.clone(), &[], &[], Some(stored(3)));
    assert_eq!(errors(&out), vec!["ERROR: c:t:001 bad kind"]);
    assert_eq!(out["receipt"]["profile_version"], 3);
    let out = drive(input, &[]);
    assert_eq!(
        errors(&out),
        vec!["ERROR: unknown graph g: the first batch must carry a profile"]
    );
    assert!(out["receipt"].get("profile_version").is_none());
}

#[test]
fn a_stored_profile_that_no_longer_reads_is_reported() {
    let mut input = batch(&[claim()]);
    input["profile"] = Value::Null;
    let out = drive_store(input, &[], &[], Some(with(stored(2), "text_max_words", Value::Null)));
    assert_eq!(
        errors(&out),
        vec!["ERROR: stored profile g v2 text_max_words must be an integer from 1 to 400"]
    );
}

#[test]
fn a_profile_resolver_must_be_a_known_perspective() {
    let mut input = batch(&[claim()]);
    input["profile"] = json!(with(research(), "resolver_perspectives", json!(["p:a", "p:judge", "p:held"])).to_string());
    let out = drive(input, &[("Perspective", "p:held")]);
    assert_eq!(errors(&out), vec!["ERROR: profile resolver p:judge unknown perspective"]);
}

#[test]
fn the_vocabulary_is_the_profiles() {
    let mut profile = research();
    profile["mapping_targets"] = json!(["postgres"]);
    profile["text_max_words"] = json!(3);
    profile["confidence_classes"] = json!(["sure"]);
    profile["edge_types"] = json!(["cites"]);
    let mut input = batch(&[with(with(with(claim(), "kind", json!("mapping")), "fit", json!("gap")), "against", json!("defradb"))]);
    input["profile"] = json!(profile.to_string());
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:t:001", "type": "supports", "by": "p:a", "date": "d", "note": ""})]));
    assert_eq!(
        errors(&call(input)),
        vec![
            "ERROR: c:t:001 bad confidence",
            "ERROR: c:t:001 mapping without valid against",
            "ERROR: edge c:t:001->c:t:001 bad type supports",
        ]
    );
    let mut input = batch(&[with(claim(), "text", json!("one two three four"))]);
    profile["confidence_classes"] = json!(["memory"]);
    input["profile"] = json!(profile.to_string());
    assert_eq!(errors(&call(input)), vec!["ERROR: c:t:001 text is 4 words (max 3)"]);
}

#[test]
fn a_challenge_needs_exactly_one_challenges_edge_from_it() {
    let challenge = with(with(with(claim(), "id", json!("c:t:002")), "kind", json!("challenge")), "quote", json!("A test claim."));
    let edge = json!({"from": "c:t:002", "to": "c:t:001", "type": "challenges", "by": "p:a", "date": "2026-10-29", "note": ""});
    let mut input = batch(&[claim(), challenge.clone()]);
    input["edges"] = json!(jsonl(&[edge.clone()]));
    let out = drive(input.clone(), &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["claims"][1]["kind"], "challenge");
    input["edges"] = Value::Null;
    assert_eq!(errors(&call(input.clone())), vec!["ERROR: c:t:002 is a challenge without its challenges edge"]);
    let to_store = with(edge.clone(), "to", json!("c:old:001"));
    input["edges"] = json!(jsonl(&[edge, to_store]));
    assert_eq!(errors(&call(input)), vec!["ERROR: c:t:002 is a challenge with 2 challenges edges (one only)"]);
}

#[test]
fn a_challenges_or_reviews_edge_leaves_a_claim_of_that_kind() {
    let edge = json!({"from": "c:t:001", "to": "c:old:001", "type": "reviews", "by": "p:a", "date": "d", "note": ""});
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[edge]));
    assert_eq!(
        errors(&call(input)),
        vec!["ERROR: edge c:t:001->c:old:001 reviews from a claim that is not a review"]
    );
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[
        json!({"from": "c:old:002", "to": "c:t:001", "type": "challenges", "by": "p:a", "date": "d", "note": ""}),
        json!({"from": "c:old:003", "to": "c:t:001", "type": "challenges", "by": "p:a", "date": "d", "note": ""}),
    ]));
    let out = drive_store(
        input,
        &[],
        &[("c:old:002", "challenge"), ("c:old:003", "fact")],
        None,
    );
    assert_eq!(
        errors(&out),
        vec!["ERROR: edge c:old:003->c:t:001 challenges from a claim that is not a challenge"]
    );
}

#[test]
fn a_review_needs_a_verdict_from_the_profile() {
    let review = |verdict: Value| {
        with(with(with(with(claim(), "id", json!("c:t:002")), "kind", json!("review")), "verdict", verdict), "quote", json!("A test claim."))
    };
    let edge = json!({"from": "c:t:002", "to": "c:t:001", "type": "reviews", "by": "p:a", "date": "d", "note": ""});
    let mut input = batch(&[claim(), review(json!("sustains"))]);
    input["edges"] = json!(jsonl(&[edge.clone()]));
    let out = drive(input, &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    assert_eq!(out["claims"][1]["verdict"], "sustains");
    for bad in [json!("great"), Value::Null] {
        let mut input = batch(&[claim(), review(bad)]);
        input["edges"] = json!(jsonl(&[edge.clone()]));
        assert_eq!(errors(&call(input)), vec!["ERROR: c:t:002 review without valid verdict"]);
    }
}

#[test]
fn only_a_resolver_resolves() {
    let mut input = batch(&[claim()]);
    input["perspectives"] = json!(jsonl(&[
        json!({"id": "p:a", "name": "A", "kind": "position", "note": ""}),
        json!({"id": "p:b", "name": "B", "kind": "position", "note": ""}),
    ]));
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:old:001", "type": "resolves", "by": "p:b", "date": "d", "note": ""})]));
    assert_eq!(errors(&call(input.clone())), vec!["ERROR: edge c:t:001->c:old:001 resolves by p:b, not a resolver"]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:old:001", "type": "resolves", "by": "p:a", "date": "d", "note": ""})]));
    assert_eq!(errors(&drive(input, &[("Claim", "c:old:001")])), Vec::<String>::new());
}

#[test]
fn a_profile_without_the_dispute_vocabulary_refuses_disputes() {
    let mut profile = research();
    profile["kinds"] = json!(["fact"]);
    profile["edge_types"] = json!(["supports"]);
    let mut input = batch(&[with(with(claim(), "id", json!("c:t:002")), "kind", json!("challenge"))]);
    input["profile"] = json!(profile.to_string());
    assert_eq!(errors(&call(input)), vec!["ERROR: c:t:002 bad kind"]);
}

#[test]
fn self_quoting_kinds_are_exempt_when_quote_is_text() {
    let mut input = batch(&[
        with(with(claim(), "kind", json!("question")), "quote", json!("A test claim.")),
        with(with(with(claim(), "id", json!("c:t:002")), "kind", json!("question")), "quote", json!("not in the narrative")),
    ]);
    input["source_root"] = json!(fixture_root());
    let out = drive(input, &[]);
    assert_eq!(errors(&out), vec!["ERROR: c:t:002 quote not found verbatim in datapack.md"]);
    assert_eq!(out["receipt"]["quotes_checked"], 1);
    assert_eq!(out["receipt"]["quotes_skipped"], 1);
}

#[test]
fn every_row_and_the_receipt_carry_the_graph() {
    let mut input = batch(&[claim()]);
    input["edges"] = json!(jsonl(&[json!({"from": "c:t:001", "to": "c:t:001", "type": "supports", "by": "p:a", "date": "d", "note": ""})]));
    let out = drive(input, &[]);
    for port in ["claims", "edges", "entities", "perspectives"] {
        assert_eq!(out[port][0]["graph_id"], "g", "{port}");
    }
    assert_eq!(out["receipt"]["graph_id"], "g");
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
            {"id": "q0", "query": {"collection": "ClaimGraph", "filter": {"graph_id": {"_eq": "g"}}, "limit": 1, "order": {"version": "DESC"},
                "fields": ["version", "claim_id_pattern", "text_max_words", "kinds", "confidence_classes", "edge_types", "mapping_targets",
                    "mapping_fits", "review_verdicts", "resolver_perspectives", "display_name", "question_entity", "default_asker",
                    "questions_from_prefix", "note"]}},
            {"id": "q1", "query": {"collection": "Claim", "filter": {"_and": [{"graph_id": {"_eq": "g"}}, {"claim_id": {"_in": ["c:old:001", "c:t:001"]}}]}, "limit": 256, "fields": ["claim_id", "kind"]}},
            {"id": "q2", "query": {"collection": "Entity", "filter": {"_and": [{"graph_id": {"_eq": "g"}}, {"entity_id": {"_in": ["e:a"]}}]}, "limit": 256, "fields": ["entity_id"]}},
            {"id": "q3", "query": {"collection": "Perspective", "filter": {"_and": [{"graph_id": {"_eq": "g"}}, {"perspective_id": {"_in": ["p:a", "p:b"]}}]}, "limit": 256, "fields": ["perspective_id"]}},
            {"id": "q4", "query": {"collection": "Edge", "filter": {"_and": [{"graph_id": {"_eq": "g"}}, {"edge_key": {"_in": ["c:t:001|supports|c:old:001"]}}]}, "limit": 256, "fields": ["edge_key"]}},
        ])
    );
    assert_eq!(out["data_calls"]["state"]["cursor"], 5);
    assert_eq!(out["data_calls"]["state"]["asked"], json!(["q0", "q1", "q2", "q3", "q4"]));
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
            "quotes_skipped": 1, "gate_version": env!("CARGO_PKG_VERSION"), "checked_at": NOW,
            "graph_id": "g", "profile_version": 1})
    );
    assert_eq!(
        out["claims"],
        json!([{"claim_id": "c:t:001", "section": "test", "text": "A test claim.", "quote": "A test claim",
            "from_ref": "datapack.md#x", "sources": "[{\"doc\":null,\"url\":null,\"fetched\":null}]",
            "entities": ["e:a"], "entity_text": " e:a ", "kind": "fact", "by": "p:a", "asserted": "2026-10-22",
            "status": "live", "confidence": "memory", "note": "", "graph_id": "g"}])
    );
    assert_eq!(
        out["edges"],
        json!([{"edge_key": "c:t:001|supports|c:old:001", "from_claim": "c:t:001", "to_claim": "c:old:001",
            "edge_type": "supports", "by": "p:b", "date": "2026-10-22", "note": "", "graph_id": "g"}])
    );
    assert_eq!(
        out["entities"],
        json!([{"entity_id": "e:a", "entity_type": "concept", "name": "A", "alias_text": " a ", "note": "", "graph_id": "g"}]),
        "an empty alias list is omitted, never written as []"
    );
    assert_eq!(
        out["perspectives"],
        json!([{"perspective_id": "p:a", "name": "A", "kind": "position", "note": "", "graph_id": "g"}])
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
    let errs = errors(&call(json!({"batch_id": "b", "graph_id": "g", "gate_now": NOW, "data_calls": true})));
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
        "q0": {"result": {"rows": []}},
        "q1": {"error": "collection \"Claim\" is outside the plugin's declared data grant"},
        "q2": {"result": {"rows": []}},
        "q3": {"result": {"rows": []}},
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
    // ClaimGraph, Claim in two chunks, Entity, Perspective, Historical in two chunks.
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[5]["query"]["filter"]["_and"][2]["edge_type"], json!({"_in": ["supersedes", "retracts"]}));
    let mut results = serde_json::Map::new();
    for r in requests {
        let id = r["id"].as_str().unwrap().to_owned();
        let rows: Vec<Value> = if id == "q5" {
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
        json!([{"id": "q5-256", "query": {"collection": "Edge", "filter": {"_and": [{"graph_id": {"_eq": "g"}}, {"to_claim": {"_in": claims[..256].iter().map(|c| c["id"].clone()).collect::<Vec<_>>()}}, {"edge_type": {"_in": ["supersedes", "retracts"]}}]}, "limit": 256, "offset": 256, "fields": ["to_claim"]}}])
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
        "batch_id": "datapack-flipper-20261029-1", "graph_id": "g", "section": "datapack-flipper", "gate_now": NOW,
        "data_calls": true, "source_root": fixture_root(),
        "profile": with(research(), "resolver_perspectives", json!(["p:datapack"])).to_string(),
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

#[test]
fn optional_lists_may_be_empty_or_absent_and_then_refuse_their_rule() {
    let mut minimal = research();
    for key in ["mapping_targets", "mapping_fits", "review_verdicts"] {
        minimal[key] = json!([]);
    }
    minimal.as_object_mut().unwrap().remove("resolver_perspectives");
    let profile = || json!(minimal.to_string());
    let mut input = batch(&[claim()]);
    input["profile"] = profile();
    let out = drive(input, &[]);
    assert_eq!(errors(&out), Vec::<String>::new());
    for key in ["mapping_targets", "mapping_fits", "review_verdicts", "resolver_perspectives"] {
        assert!(out["profile"].get(key).is_none(), "{key}: an empty list is omitted, never written as []");
    }
    let mapping = with(with(with(claim(), "kind", json!("mapping")), "fit", json!("gap")), "against", json!("defradb"));
    let mut input = batch(&[mapping]);
    input["profile"] = profile();
    assert_eq!(
        errors(&call(input)),
        vec!["ERROR: c:t:001 mapping without valid fit", "ERROR: c:t:001 mapping without valid against"]
    );
    let review = with(with(with(with(claim(), "id", json!("c:t:002")), "kind", json!("review")), "verdict", json!("sustains")), "quote", json!("A test claim."));
    let mut input = batch(&[claim(), review]);
    input["profile"] = profile();
    input["edges"] = json!(jsonl(&[
        json!({"from": "c:t:002", "to": "c:t:001", "type": "reviews", "by": "p:a", "date": "d", "note": ""}),
        json!({"from": "c:t:001", "to": "c:t:002", "type": "resolves", "by": "p:a", "date": "d", "note": ""}),
    ]));
    assert_eq!(
        errors(&call(input)),
        vec!["ERROR: c:t:002 review without valid verdict", "ERROR: edge c:t:001->c:t:002 resolves by p:a, not a resolver"]
    );
}
