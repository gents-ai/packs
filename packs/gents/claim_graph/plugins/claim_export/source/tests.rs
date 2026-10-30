use super::*;

const GRAPH: &str = "government-attestation";

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
    row.insert("graph_id".into(), json!(GRAPH));
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
            let mut rows: Vec<&Map<String, Value>> =
                self.rows.get(q["collection"].as_str().unwrap()).into_iter().flatten().collect();
            rows.sort_by(|a, b| match (a[id_field].as_i64(), b[id_field].as_i64()) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => a[id_field].as_str().cmp(&b[id_field].as_str()),
            });
            let graph = q["filter"]["graph_id"]["_eq"].as_str().expect("every query names its graph");
            rows.retain(|r| r["graph_id"] == graph);
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
        if input.get("graph_id").is_none() {
            input["graph_id"] = json!(GRAPH);
        }
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
    let output = run(&json!({"graph_id": "g", "out_dir": "/o", "data_calls": true})).unwrap_or_else(|f| json!(f.0));
    let requests = output["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 12);
    let ids: Vec<&str> = requests.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(&ids[..4], ["claims@0", "claims@256", "edges@0", "edges@256"]);
    assert_eq!(&ids[8..], ["claim_graph@0", "claim_graph@256", "sources@0", "sources@256"]);
    let q = &requests[0]["query"];
    assert_eq!((q["collection"].as_str(), q["limit"].as_u64(), q["offset"].as_u64()), (Some("Claim"), Some(256), Some(0)));
    assert_eq!(q["order"], json!({"claim_id": "ASC"}));
    assert_eq!(q["filter"], json!({"graph_id": {"_eq": "g"}}));
    assert_eq!(requests[8]["query"]["collection"], "ClaimGraph");
    assert_eq!(requests[8]["query"]["order"], json!({"version": "ASC"}));
    assert!(requests.iter().all(|r| r["query"]["filter"]["graph_id"] == json!({"_eq": "g"})));
    assert_eq!(output["data_calls"]["state"]["cursors"]["claims"]["next"], 0);
}

#[test]
fn sections_filter_the_claim_queries_only() {
    let output = run(&json!({"graph_id": "g", "out_dir": "/o", "data_calls": true, "sections": ["a", "b"]})).unwrap_or_else(|f| json!(f.0));
    let requests = output["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests[0]["query"]["filter"], json!({"graph_id": {"_eq": "g"}, "section": {"_in": ["a", "b"]}}));
    assert_eq!(requests[3]["query"]["filter"], json!({"graph_id": {"_eq": "g"}}));
}

#[test]
fn the_exported_files_equal_the_research_files_after_normalisation() {
    let dir = scratch("tree");
    let mut host = fixture_host(usize::MAX);
    let (summary, _) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["ok"], true, "{summary}");
    assert_eq!(summary["dry_run"], false);
    assert_eq!(summary["written"], json!({"claims": 14, "edges": 4, "entities": 6, "perspectives": 3, "claim_graph": 0, "sources": 0}));
    assert_eq!(
        summary["files"],
        json!([
            "claims/datapack-flipper.jsonl",
            "claims/mapping-sample.jsonl",
            "edges/edges.jsonl",
            "entities/entities.jsonl",
            "perspectives/perspectives.jsonl",
            "sources/sources.jsonl",
            "claim_graph.json"
        ])
    );
    let written = tree(&dir);
    assert_eq!(written.len(), 7, "no temporary file is left: {:?}", written.keys());
    assert_eq!(written["sources/sources.jsonl"], "");
    assert_eq!(written["claim_graph.json"], "[]\n");
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
        row.insert("graph_id".into(), json!(GRAPH));
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
    let mut input = json!({"graph_id": GRAPH, "out_dir": dir.to_str().unwrap(), "data_calls": true});
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
    assert!(refused(json!({"graph_id": "g", "data_calls": true})).starts_with("out_dir is required"));
    assert_eq!(refused(json!({"out_dir": "/o", "data_calls": true})), "graph_id is required: the graph to export");
    assert_eq!(refused(json!({"graph_id": " ", "out_dir": "/o", "data_calls": true})), "graph_id is required: the graph to export");
    assert_eq!(refused(json!({"graph_id": 3, "out_dir": "/o", "data_calls": true})), "graph_id must be a string");
    assert!(refused(json!({"graph_id": "g", "out_dir": "/o"})).starts_with("the host offers this plugin no data calls"));
    assert!(refused(json!({"graph_id": "g", "out_dir": "/o", "data_calls": true, "bogus": 1})).contains("\"bogus\" is not known"));
    assert!(refused(json!({"graph_id": "g", "out_dir": "/o", "data_calls": true, "sections": ["../x"]})).contains("sections holds"));
    assert!(refused(json!({"graph_id": "g", "out_dir": "/o", "data_calls": true, "sections": []})).contains("1 to 1000"));
    assert!(refused(json!({"graph_id": "g", "out_dir": "/o", "data_calls": true, "dry_run": "yes"})).contains("dry_run"));
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

fn profile_row(graph: &str, version: i64) -> Map<String, Value> {
    let mut row = Map::new();
    for field in render::CLAIM_GRAPH.fields {
        row.insert((*field).into(), Value::Null);
    }
    row.insert("graph_id".into(), json!(graph));
    row.insert("version".into(), json!(version));
    row.insert("kinds".into(), json!(["fact", "question"]));
    row.insert("claim_id_pattern".into(), json!("^c:[a-z0-9-]+:[0-9]{3}$"));
    row.insert("text_max_words".into(), json!(40));
    row.insert("batch_id".into(), json!(format!("b-{version}")));
    // Fields the host adds are not exported.
    row.insert("_docID".into(), json!("bae-x"));
    row
}

#[test]
fn the_profile_versions_are_exported_as_a_json_array_in_version_order() {
    let dir = scratch("profiles");
    let mut host = fixture_host(usize::MAX);
    host.rows.insert("ClaimGraph", vec![profile_row(GRAPH, 10), profile_row(GRAPH, 2), profile_row("other", 1)]);
    let (summary, _) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["written"]["claim_graph"], 2, "{summary}");
    let text = &tree(&dir)["claim_graph.json"];
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!((lines[0], lines[3]), ("[", "]"));
    assert!(lines[1].starts_with(r#"{"graph_id":"government-attestation","version":2,"display_name":null,"kinds":["fact","question"]"#), "{}", lines[1]);
    assert!(lines[1].ends_with(r#""created_at":null,"batch_id":"b-2"},"#), "{}", lines[1]);
    assert!(lines[2].contains(r#""version":10,"#) && !lines[2].ends_with(','));
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
    assert!(!text.contains("_docID"));
}

#[test]
fn rows_of_another_graph_are_not_exported() {
    let dir = scratch("graphs");
    let mut host = fixture_host(usize::MAX);
    let mut stray = host.rows["Claim"][0].clone();
    stray.insert("graph_id".into(), json!("other"));
    stray.insert("claim_id".into(), json!("c:flipper:999"));
    host.rows.get_mut("Claim").unwrap().push(stray);
    let (summary, _) = host.drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["written"]["claims"], 14);
    assert_eq!(summary["graph_id"], GRAPH);
    let (summary, _) = fixture_host(usize::MAX).drive(json!({"graph_id": "other", "dry_run": true}));
    assert_eq!(summary["written"], json!({"claims": 0, "edges": 0, "entities": 0, "perspectives": 0, "claim_graph": 0, "sources": 0}));
}

#[test]
fn every_plugin_case_matches_this_build() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let case: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let output = run(&case["input"]).unwrap_or_else(|f| json!({"ok": false, "error": f.0, "retryable": false}));
        match case.get("expect") {
            Some(expect) => assert_eq!(&output, expect, "{}", path.display()),
            // A callback receipt carries the time it was made.
            None => assert!(output["exported_at"].is_string(), "{}: {output}", path.display()),
        }
        checked += 1;
    }
    assert_eq!(checked, 12);
}

fn source_row(graph: &str, n: usize, text: &str) -> Map<String, Value> {
    let mut row = Map::new();
    for field in render::SOURCES.fields {
        row.insert((*field).into(), Value::Null);
    }
    let fields = [
        ("graph_id", json!(graph)),
        ("source_id", json!(format!("s:{n:016x}"))),
        ("url", json!(format!("https://example.org/{n}"))),
        ("final_url", json!(format!("https://example.org/{n}"))),
        ("fetched_at", json!("2026-10-30T12:00:00Z")),
        ("status", json!("ok")),
        ("http_status", json!(200)),
        ("content_type", json!("text/plain")),
        ("text_chars", json!(text.chars().count())),
        ("truncated", json!(false)),
        ("confidential", json!(false)),
        ("request_id", json!(format!("sr-{n}"))),
        ("text", json!(text)),
        ("_docID", json!("bae-y")),
    ];
    for (k, v) in fields {
        row.insert(k.into(), v);
    }
    row
}

fn source_host(cap: usize, n: usize) -> Host {
    let mut host = fixture_host(cap);
    let mut rows: Vec<_> = (0..n).map(|i| source_row(GRAPH, i, &format!("text of source {i}\nline two"))).collect();
    rows.push(source_row("other", 999, "another graph"));
    host.rows.insert("Source", rows);
    host
}

#[test]
fn sources_are_exported_as_metadata_without_text() {
    let dir = scratch("sources-meta");
    let (summary, _) = source_host(usize::MAX, 3).drive(json!({"out_dir": dir.to_str().unwrap()}));
    assert_eq!(summary["written"]["sources"], 3, "{summary}");
    let text = &tree(&dir)["sources/sources.jsonl"];
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        r#"{"id":"s:0000000000000000","url":"https://example.org/0","final_url":"https://example.org/0","title":null,"fetched_at":"2026-10-30T12:00:00Z","status":"ok","error":null,"http_status":200,"content_type":"text/plain","content_hash":null,"text_chars":25,"truncated":false,"confidential":false,"request_id":"sr-0","note":null}"#
    );
    assert!(!text.contains("text of source") && !text.contains("_docID") && !text.contains("another graph"));
}

#[test]
fn source_texts_are_exported_in_a_pass_of_their_own() {
    for (cap, n) in [(usize::MAX, 0), (usize::MAX, 1), (usize::MAX, 32), (usize::MAX, 70), (5, 40)] {
        let dir = scratch("sources-text");
        let mut host = source_host(cap, n);
        let (summary, rounds) = host.drive(json!({"out_dir": dir.to_str().unwrap(), "include_source_text": true}));
        assert_eq!(summary["ok"], true, "{summary}");
        assert_eq!(summary["written"]["sources"], n);
        assert_eq!(summary["include_source_text"], true);
        let files = summary["files"].as_array().unwrap();
        assert_eq!(files[files.len() - 2], "sources/sources.jsonl");
        let written = tree(&dir);
        assert!(!written.keys().any(|k| k.contains(".part")), "{:?}", written.keys());
        let lines: Vec<Value> = written["sources/sources.jsonl"].lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), n, "cap {cap}");
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line["id"], format!("s:{i:016x}"));
            assert_eq!(line["text"], format!("text of source {i}\nline two"));
            assert_eq!(line.as_object().unwrap().keys().last().unwrap(), "text");
        }
        assert!(rounds < 64, "{rounds}");
    }
}

#[test]
fn a_dry_run_with_texts_reads_no_texts() {
    let dir = scratch("sources-dry");
    let (summary, rounds) = source_host(usize::MAX, 5).drive(json!({"out_dir": dir.to_str().unwrap(), "include_source_text": true, "dry_run": true}));
    assert_eq!(summary["written"]["sources"], 5);
    assert_eq!(rounds, 3);
    assert!(tree(&dir).is_empty());
}

#[test]
fn a_text_query_error_fails_the_call() {
    let dir = scratch("sources-error");
    let mut input = json!({"graph_id": GRAPH, "out_dir": dir.to_str().unwrap(), "data_calls": true, "include_source_text": true,
        "state": source_text::start(&dir, pager::start(&SPECS)).unwrap(),
        "data_results": {"source_text@0": {"error": "boom"}, "source_text@16": {"result": {"rows": []}}}});
    assert_eq!(run(&input).err().unwrap().0, "the Source text query failed: boom");
    input["include_source_text"] = json!("yes");
    assert_eq!(run(&input).err().unwrap().0, "include_source_text must be true or false");
}
