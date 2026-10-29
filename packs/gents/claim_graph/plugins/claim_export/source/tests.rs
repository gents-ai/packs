use super::*;

const FIXTURES: [(&str, &str); 6] = [
    (
        "claims/datapack-flipper.jsonl",
        include_str!("../tests/fixtures/research/claims/datapack-flipper.jsonl"),
    ),
    (
        "claims/mapping-sample.jsonl",
        include_str!("../tests/fixtures/research/claims/mapping-sample.jsonl"),
    ),
    (
        "edges/edges.jsonl",
        include_str!("../tests/fixtures/research/edges/edges.jsonl"),
    ),
    (
        "entities/entities.jsonl",
        include_str!("../tests/fixtures/research/entities/entities.jsonl"),
    ),
    (
        "perspectives/perspectives.jsonl",
        include_str!("../tests/fixtures/research/perspectives/perspectives.jsonl"),
    ),
    (
        "claims/other.jsonl",
        include_str!("../tests/fixtures/research/claims/datapack-flipper.jsonl"),
    ),
];

/// What `jq -c .` does to a research line: compact separators, keys kept in
/// the order written, strings as UTF-8.
fn normalise(line: &str) -> String {
    serde_json::from_str::<Value>(line).unwrap().to_string()
}

/// The store row an importer writes for a research line.
fn store_row(kind: &Kind, section: &str, line: &Value) -> Map<String, Value> {
    let o = line.as_object().unwrap();
    let get = |k: &str| o.get(k).cloned().unwrap_or(Value::Null);
    let mut row = Map::new();
    match kind.key {
        "claims" => {
            row.insert("claim_id".into(), get("id"));
            row.insert("section".into(), json!(section));
            row.insert("text".into(), get("text"));
            row.insert("quote".into(), get("quote"));
            row.insert("from_ref".into(), get("from"));
            row.insert("sources".into(), json!(get("sources").to_string()));
            row.insert("entities".into(), get("entities"));
            for k in ["kind", "by", "asserted", "status", "confidence", "note", "fit", "against", "ref", "valid_from", "valid_to"] {
                row.insert(k.into(), get(k));
            }
        }
        "edges" => {
            let key = format!(
                "{}|{}|{}",
                o["from"].as_str().unwrap(),
                o["type"].as_str().unwrap(),
                o["to"].as_str().unwrap()
            );
            row.insert("edge_key".into(), json!(key));
            row.insert("from_claim".into(), get("from"));
            row.insert("to_claim".into(), get("to"));
            row.insert("edge_type".into(), get("type"));
            for k in ["by", "date", "note"] {
                row.insert(k.into(), get(k));
            }
        }
        "entities" => {
            row.insert("entity_id".into(), get("id"));
            row.insert("entity_type".into(), get("type"));
            row.insert("name".into(), get("name"));
            // An empty list is stored as null.
            let aliases = get("aliases");
            row.insert(
                "aliases".into(),
                if aliases.as_array().is_some_and(Vec::is_empty) { Value::Null } else { aliases },
            );
            row.insert("note".into(), get("note"));
        }
        _ => {
            row.insert("perspective_id".into(), get("id"));
            for k in ["name", "kind", "note", "did"] {
                row.insert(k.into(), get(k));
            }
        }
    }
    row
}

fn kind_of(path: &str) -> &'static Kind {
    match path.split('/').next().unwrap() {
        "claims" => &render::CLAIMS,
        "edges" => &render::EDGES,
        "entities" => &render::ENTITIES,
        _ => &render::PERSPECTIVES,
    }
}

#[test]
fn rendered_lines_equal_the_research_lines_after_jq_normalisation() {
    let mut checked = 0;
    for (path, fixture) in FIXTURES {
        let kind = kind_of(path);
        for line in fixture.lines() {
            let record: Value = serde_json::from_str(line).unwrap();
            let row = store_row(kind, "datapack-flipper", &record);
            let (_, rendered) = render::render(kind, &row).unwrap();
            assert_eq!(rendered.to_string(), normalise(line), "{path}: {line}");
            checked += 1;
        }
    }
    assert_eq!(checked, 11 + 3 + 4 + 6 + 3 + 11);
}

#[test]
fn the_research_lines_are_hand_formatted_so_the_comparison_is_not_trivial() {
    let line = FIXTURES[0].1.lines().next().unwrap();
    assert!(line.contains("\": ") && line.contains("\", \""));
    assert_ne!(line, normalise(line));
    assert!(!normalise(line).contains(": "));
}

#[test]
fn optional_fields_follow_the_contract() {
    let mut row = store_row(&render::CLAIMS, "s", &serde_json::from_str(FIXTURES[1].1.lines().nth(1).unwrap()).unwrap());
    let (_, line) = render::claim(&row).unwrap();
    let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
    assert_eq!(keys[11..], ["note", "fit", "against", "ref"]);
    row.insert("valid_from".into(), json!("2026-01-01"));
    row.insert("valid_to".into(), json!(""));
    row.insert("ref".into(), json!(""));
    let (_, line) = render::claim(&row).unwrap();
    let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
    assert_eq!(keys[11..], ["note", "fit", "against", "ref", "valid_from", "valid_to"]);
    assert_eq!(line["ref"], "");
    assert!(line.to_string().contains(r#""ref":"","valid_from""#));
    row.insert("ref".into(), Value::Null);
    let (_, line) = render::claim(&row).unwrap();
    let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
    assert_eq!(keys[11..], ["note", "fit", "against", "valid_from", "valid_to"]);
    row.insert("note".into(), Value::Null);
    assert_eq!(render::claim(&row).unwrap().1["note"], "");
    let perspective = |did: Value| {
        let mut row = Map::new();
        for (k, v) in [("perspective_id", json!("p:a")), ("name", json!("A")), ("kind", json!("person")), ("did", did)] {
            row.insert(k.into(), v);
        }
        render::perspective(&row).unwrap().1.to_string()
    };
    assert_eq!(perspective(Value::Null), r#"{"id":"p:a","name":"A","kind":"person","note":""}"#);
    assert_eq!(
        perspective(json!("did:key:z")),
        r#"{"id":"p:a","name":"A","kind":"person","note":"","did":"did:key:z"}"#
    );
}

#[test]
fn a_malformed_row_names_itself() {
    let mut row = store_row(&render::CLAIMS, "s", &serde_json::from_str(FIXTURES[0].1.lines().next().unwrap()).unwrap());
    row.insert("sources".into(), json!("{not an array}"));
    let error = render::claim(&row).unwrap_err();
    assert!(error.starts_with("claim c:flipper:001:") && error.contains("sources"), "{error}");
    row.insert("sources".into(), Value::Null);
    row.remove("kind");
    assert_eq!(render::claim(&row).unwrap_err(), "claim c:flipper:001: kind is missing");
}

#[test]
fn only_safe_section_names_become_files() {
    for good in ["datapack-flipper", "mapping_dcl", "v1.2"] {
        assert!(render::section_is_safe(good), "{good}");
    }
    for bad in ["", ".hidden", "../x", "a/b", "a b", "a\\b"] {
        assert!(!render::section_is_safe(bad), "{bad:?}");
    }
}

/// A host serving `data_calls` from rows held in memory, as the real one does:
/// ordered by the id field, filtered on `section`, cut at `limit` and at the
/// output budget (`cap` rows).
struct Host {
    rows: BTreeMap<&'static str, Vec<Map<String, Value>>>,
    cap: usize,
    queries: usize,
}

impl Host {
    fn new(cap: usize) -> Self {
        Host { rows: BTreeMap::new(), cap, queries: 0 }
    }

    fn serve(&mut self, requests: &[Value]) -> Map<String, Value> {
        assert!((1..=16).contains(&requests.len()), "{} requests", requests.len());
        let mut results = Map::new();
        for request in requests {
            let q = &request["query"];
            self.queries += 1;
            let limit = q["limit"].as_u64().unwrap() as usize;
            assert!((1..=256).contains(&limit));
            let id_field = q["order"].as_object().unwrap().keys().next().unwrap().as_str();
            assert_eq!(q["order"][id_field], "ASC");
            let mut rows: Vec<&Map<String, Value>> = self.rows[q["collection"].as_str().unwrap()].iter().collect();
            rows.sort_by(|a, b| a[id_field].as_str().cmp(&b[id_field].as_str()));
            if let Some(allowed) = q["filter"]["section"]["_in"].as_array() {
                rows.retain(|r| allowed.contains(&r["section"]));
            }
            let page: Vec<Value> = rows
                .into_iter()
                .skip(q["offset"].as_u64().unwrap_or(0) as usize)
                .take(limit.min(self.cap))
                .map(|r| {
                    let wanted = q["fields"].as_array().unwrap();
                    Value::Object(r.iter().filter(|(k, _)| wanted.contains(&json!(k))).map(|(k, v)| (k.clone(), v.clone())).collect())
                })
                .collect();
            results.insert(request["id"].as_str().unwrap().into(), json!({"result": {"rows": page}}));
        }
        results
    }

    /// Runs the plugin's rounds to its final answer.
    fn drive(&mut self, mut input: Value) -> (Value, usize) {
        input["data_calls"] = json!(true);
        let mut rounds = 1;
        loop {
            let output = run(&input).unwrap_or_else(|f| json!({"ok": false, "error": f.0}));
            let Some(calls) = output.get("data_calls") else {
                return (output, rounds);
            };
            assert!(rounds < 64, "more than 64 rounds");
            rounds += 1;
            let results = self.serve(calls["requests"].as_array().unwrap());
            input["data_results"] = Value::Object(results);
            input["state"] = calls["state"].clone();
        }
    }
}

fn fixture_host(cap: usize) -> Host {
    let mut host = Host::new(cap);
    for (path, fixture) in &FIXTURES[..5] {
        let kind = kind_of(path);
        let section = path.trim_start_matches("claims/").trim_end_matches(".jsonl");
        let rows = host.rows.entry(kind.collection).or_default();
        for line in fixture.lines() {
            rows.push(store_row(kind, section, &serde_json::from_str(line).unwrap()));
        }
    }
    host
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("claim_export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tree(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                files.insert(rel, std::fs::read_to_string(path).unwrap());
            }
        }
    }
    files
}

#[test]
fn the_first_round_asks_for_every_collection_ordered_by_its_id() {
    let output = run(&json!({"out_dir": "/o", "data_calls": true})).unwrap_or_else(|f| json!(f.0));
    let requests = output["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 16);
    let ids: Vec<&str> = requests.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(&ids[..5], ["claims@0", "claims@256", "claims@512", "claims@768", "edges@0"]);
    let q = &requests[0]["query"];
    assert_eq!((q["collection"].as_str(), q["limit"].as_u64(), q["offset"].as_u64()), (Some("Claim"), Some(256), Some(0)));
    assert_eq!(q["order"], json!({"claim_id": "ASC"}));
    assert_eq!(q["filter"], json!({}));
    assert_eq!(output["data_calls"]["state"]["cursors"]["claims"]["next"], 0);
}

#[test]
fn sections_filter_the_claim_queries_only() {
    let output = run(&json!({"out_dir": "/o", "data_calls": true, "sections": ["a", "b"]})).unwrap_or_else(|f| json!(f.0));
    let requests = output["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests[0]["query"]["filter"], json!({"section": {"_in": ["a", "b"]}}));
    assert_eq!(requests[4]["query"]["filter"], json!({}));
}

#[test]
fn the_exported_files_equal_the_research_files_after_normalisation() {
    let dir = scratch("tree");
    let mut host = fixture_host(usize::MAX);
    let (summary, _) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["ok"], true, "{summary}");
    assert_eq!(summary["dry_run"], false);
    assert_eq!(summary["written"], json!({"claims": 14, "edges": 4, "entities": 6, "perspectives": 3}));
    assert_eq!(
        summary["files"],
        json!([
            "claims/datapack-flipper.jsonl",
            "claims/mapping-sample.jsonl",
            "edges/edges.jsonl",
            "entities/entities.jsonl",
            "perspectives/perspectives.jsonl"
        ])
    );
    let written = tree(&dir);
    assert_eq!(written.len(), 5, "no temporary file is left: {:?}", written.keys());
    for (path, fixture) in &FIXTURES[..5] {
        // The store orders by id, so the expected text is the fixture's lines sorted by id.
        let kind = kind_of(path);
        let mut expected: Vec<(String, String)> = fixture
            .lines()
            .map(|l| {
                let row = store_row(kind, "x", &serde_json::from_str(l).unwrap());
                (render::render(kind, &row).unwrap().0, normalise(l) + "\n")
            })
            .collect();
        expected.sort();
        let expected: String = expected.into_iter().map(|(_, l)| l).collect();
        assert_eq!(written[*path], expected, "{path}");
    }
}

#[test]
fn a_dry_run_counts_and_writes_nothing() {
    let dir = scratch("dry");
    let (summary, _) = fixture_host(usize::MAX).drive(json!({"out_dir": dir.to_str().unwrap(), "dry_run": true}));
    assert_eq!(summary["dry_run"], true);
    assert_eq!(summary["written"]["claims"], 14);
    assert_eq!(summary["sections"], json!({"datapack-flipper": 11, "mapping-sample": 3}));
    assert!(tree(&dir).is_empty());
    let (summary, _) = fixture_host(usize::MAX).drive(json!({"dry_run": true}));
    assert_eq!(summary["ok"], true);
}

#[test]
fn sections_limit_the_exported_claims() {
    let dir = scratch("sections");
    let (summary, _) = fixture_host(usize::MAX).drive(json!({"out_dir": dir.to_str().unwrap(), "sections": ["mapping-sample"]}));
    assert_eq!(summary["written"]["claims"], 3);
    assert!(tree(&dir).contains_key("claims/mapping-sample.jsonl"));
    assert!(!tree(&dir).contains_key("claims/datapack-flipper.jsonl"));
}

fn many_edges(host: &mut Host, n: usize) {
    let rows = host.rows.entry("Edge").or_default();
    rows.clear();
    for i in 0..n {
        let key = format!("c:s:{:03}|supports|c:s:{:03}", i / 1000, i % 1000);
        let mut row = Map::new();
        for (k, v) in [
            ("edge_key", json!(key)),
            ("from_claim", json!(format!("c:s:{:03}", i / 1000))),
            ("to_claim", json!(format!("c:s:{:03}", i % 1000))),
            ("edge_type", json!("supports")),
            ("by", json!("p:a")),
            ("date", json!("2026-10-22")),
            ("note", Value::Null),
        ] {
            row.insert(k.into(), v);
        }
        rows.push(row);
    }
}

#[test]
fn collections_longer_than_a_page_are_paged_without_loss_or_repeats() {
    for n in [255, 256, 257, 1024, 1500] {
        let dir = scratch("paging");
        let mut host = fixture_host(usize::MAX);
        many_edges(&mut host, n);
        let (summary, rounds) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
        assert_eq!(summary["written"]["edges"], n, "{summary}");
        let text = &tree(&dir)["edges/edges.jsonl"];
        let keys: Vec<&str> = text.lines().collect();
        assert_eq!(keys.len(), n);
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "sorted by edge key without repeats");
        assert!(rounds <= 8, "{rounds} rounds for {n} edges");
    }
}

#[test]
fn a_page_the_host_cut_short_is_continued_not_taken_for_the_end() {
    let dir = scratch("cut");
    let mut host = fixture_host(100);
    many_edges(&mut host, 450);
    let (summary, _) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["written"]["edges"], 450, "{summary}");
    assert_eq!(tree(&dir)["edges/edges.jsonl"].lines().count(), 450);
}

#[test]
fn a_query_error_fails_the_call_without_writing() {
    let dir = scratch("error");
    let mut input = json!({"out_dir": dir.to_str().unwrap(), "data_calls": true});
    let calls = run(&input).unwrap_or_else(|f| json!(f.0));
    let mut results = Map::new();
    for request in calls["data_calls"]["requests"].as_array().unwrap() {
        results.insert(request["id"].as_str().unwrap().into(), json!({"error": "collection \"Claim\" is outside the plugin's declared data grant"}));
    }
    input["data_results"] = Value::Object(results);
    input["state"] = calls["data_calls"]["state"].clone();
    let failure = run(&input).err().unwrap().0;
    assert!(failure.starts_with("the Claim query failed:") || failure.starts_with("the Edge query failed:"), "{failure}");
    assert!(tree(&dir).is_empty());
}

#[test]
fn bad_input_is_refused_with_a_sentence() {
    let refused = |input: Value| run(&input).err().map(|f| f.0).unwrap_or_default();
    assert!(refused(json!({"data_calls": true})).starts_with("out_dir is required"));
    assert!(refused(json!({"out_dir": "/o"})).starts_with("the host offers this plugin no data calls"));
    assert!(refused(json!({"out_dir": "/o", "data_calls": true, "bogus": 1})).contains("\"bogus\" is not known"));
    assert!(refused(json!({"out_dir": "/o", "data_calls": true, "sections": ["../x"]})).contains("sections holds"));
    assert!(refused(json!({"out_dir": "/o", "data_calls": true, "sections": []})).contains("1 to 1000"));
    assert!(refused(json!({"out_dir": "/o", "data_calls": true, "dry_run": "yes"})).contains("dry_run"));
    assert!(refused(json!([])).contains("JSON object"));
}

#[test]
fn a_claim_section_that_is_not_a_safe_stem_is_refused() {
    let mut host = fixture_host(usize::MAX);
    host.rows.get_mut("Claim").unwrap()[0].insert("section".into(), json!("../escape"));
    let (summary, _) = host.drive(json!({"dry_run": true}));
    assert_eq!(summary["ok"], false);
    assert!(summary["error"].as_str().unwrap().contains("section"), "{summary}");
}
