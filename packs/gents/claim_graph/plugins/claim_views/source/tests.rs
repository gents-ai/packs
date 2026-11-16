use std::collections::BTreeMap;

use super::*;
use graph::{Claim, Edge, Graph, Status};

const FIXTURE: &str = include_str!("../tests/fixtures/graph.json");
const NOW: &str = "2026-10-29T12:00:00Z";

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn rows(key: &str) -> Vec<Value> {
    fixture()[key].as_array().unwrap().clone()
}

fn fixture_graph() -> Graph {
    Graph::from_rows(&rows("claims"), &rows("edges"), &rows("perspectives")).unwrap()
}

/// A graph of bare claims (`id`, `kind`) and edges `(from, type, to)`.
fn tiny(kinds: &[(&str, &str)], edges: &[(&str, &str, &str)]) -> Graph {
    let mut claims = BTreeMap::new();
    let mut add = |id: &str, kind: &str| {
        claims.entry(id.to_owned()).or_insert_with(|| Claim {
            id: id.to_owned(),
            text: String::new(),
            kind: kind.to_owned(),
            by: "p:a".into(),
            asserted: "2026-10-01".into(),
            confidence: "sourced".into(),
            verdict: None,
        });
    };
    for (id, kind) in kinds {
        add(id, kind);
    }
    for (from, _, to) in edges {
        add(from, "fact");
        add(to, "fact");
    }
    let edges = edges
        .iter()
        .map(|(from, t, to)| Edge {
            key: format!("{from}|{t}|{to}"),
            from: (*from).into(),
            to: (*to).into(),
            edge_type: (*t).into(),
            by: "p:a".into(),
        })
        .collect();
    Graph::new(claims, edges, BTreeMap::new())
}

#[test]
fn the_status_rule_over_edge_combinations() {
    use Status::*;
    let ch = [("c", "challenge")];
    let ch2 = [("c", "challenge"), ("d", "challenge")];
    // (name, kinds, edges, status of x, deciding edge)
    let cases: &[(&str, &[(&str, &str)], &[(&str, &str, &str)], Status, &str)] = &[
        ("no edges", &[], &[], Live, ""),
        ("support only", &[], &[("y", "supports", "x")], Live, ""),
        ("retracted", &[], &[("y", "retracts", "x")], Retracted, "y retracts x"),
        ("x retracts another claim", &[], &[("x", "retracts", "y")], Live, ""),
        ("superseded", &[], &[("y", "supersedes", "x")], Superseded, "y supersedes x"),
        ("retracts beats supersedes", &[], &[("y", "supersedes", "x"), ("z", "retracts", "x")], Retracted, "z retracts x"),
        ("contradicts from x", &[], &[("x", "contradicts", "y")], Contested, "x contradicts y"),
        ("contradicts to x", &[], &[("y", "contradicts", "x")], Contested, "y contradicts x"),
        ("x supersedes its contradiction", &[], &[("y", "contradicts", "x"), ("x", "supersedes", "y")], Live, ""),
        ("its contradiction supersedes x", &[], &[("y", "contradicts", "x"), ("y", "supersedes", "x")], Superseded, "y supersedes x"),
        ("a supersedes with a third claim resolves nothing", &[], &[("y", "contradicts", "x"), ("x", "supersedes", "z")], Contested, "y contradicts x"),
        ("one of two contradictions resolved", &[], &[("x", "contradicts", "y"), ("x", "supersedes", "y"), ("z", "contradicts", "x")], Contested, "z contradicts x"),
        ("retracts beats contradicts", &[], &[("y", "contradicts", "x"), ("z", "retracts", "x")], Retracted, "z retracts x"),
        ("open challenge", &ch, &[("c", "challenges", "x")], Challenged, "c challenges x"),
        ("answered challenge", &ch, &[("c", "challenges", "x"), ("a", "answers", "c")], Live, ""),
        ("resolved challenge", &ch, &[("c", "challenges", "x"), ("j", "resolves", "c")], Live, ""),
        ("an answer to x does not close a challenge of x", &ch, &[("c", "challenges", "x"), ("a", "answers", "x")], Challenged, "c challenges x"),
        ("a challenges edge from a claim that is not a challenge", &[("c", "position")], &[("c", "challenges", "x")], Live, ""),
        ("one of two challenges still open", &ch2, &[("c", "challenges", "x"), ("d", "challenges", "x"), ("a", "answers", "c")], Challenged, "d challenges x"),
        ("contradicts beats challenged", &ch, &[("c", "challenges", "x"), ("y", "contradicts", "x")], Contested, "y contradicts x"),
        ("superseded beats challenged", &ch, &[("c", "challenges", "x"), ("y", "supersedes", "x")], Superseded, "y supersedes x"),
        ("reviews never change status", &[("r", "review")], &[("r", "reviews", "x")], Live, ""),
        ("a review beside an open challenge", &[("c", "challenge"), ("r", "review")], &[("c", "challenges", "x"), ("r", "reviews", "x")], Challenged, "c challenges x"),
        ("a resolved review leaves its target alone", &[("r", "review")], &[("r", "reviews", "x"), ("j", "resolves", "r")], Live, ""),
        ("resolves pointing at x itself", &[], &[("j", "resolves", "x")], Live, ""),
        ("an answered question", &[("x", "question")], &[("a", "answers", "x")], Live, ""),
    ];
    for (name, kinds, edges, status, deciding) in cases {
        let graph = tiny(kinds, edges);
        let effective = graph.effective("x");
        assert_eq!(effective.status, *status, "{name}");
        let edge = effective.edge.map(Edge::describe).unwrap_or_default();
        assert_eq!(edge, *deciding, "{name}");
    }
}

#[test]
fn the_fixture_graph_has_every_status() {
    let graph = fixture_graph();
    let status = |id: &str| graph.effective(&format!("c:demo:{id}")).status;
    assert_eq!(status("001"), Status::Challenged);
    assert_eq!(status("002"), Status::Retracted);
    assert_eq!(status("003"), Status::Live, "its challenge was resolved");
    assert_eq!(status("004"), Status::Superseded);
    assert_eq!(status("005"), Status::Live, "its challenge was answered");
    assert_eq!((status("006"), status("007")), (Status::Contested, Status::Contested));
    for live in ["010", "011", "012", "013", "014", "020", "021"] {
        assert_eq!(status(live), Status::Live, "{live}");
    }
}

fn render(view: &str) -> views::Rendered {
    views::render(view, "demo", &fixture_graph(), NOW)
}

const STATUS_MD: &str = "# Status of demo

Generated 2026-10-29T12:00:00Z from 14 claims and 11 edges.

| Status | Claims |
| --- | ---: |
| live | 9 |
| challenged | 1 |
| contested | 2 |
| superseded | 1 |
| retracted | 1 |

## Claims that are not live

| Claim | Status | Deciding edge | By |
| --- | --- | --- | --- |
| c:demo:001 | challenged | c:demo:010 challenges c:demo:001 | p:critic |
| c:demo:002 | retracted | c:demo:003 retracts c:demo:002 | p:alice |
| c:demo:004 | superseded | c:demo:005 supersedes c:demo:004 | p:bob |
| c:demo:006 | contested | c:demo:006 contradicts c:demo:007 | p:critic |
| c:demo:007 | contested | c:demo:006 contradicts c:demo:007 | p:critic |
";

const DISPUTES_MD: &str = "# Open challenges in demo

Generated 2026-10-29T12:00:00Z. 1 open challenge.

| Target | Target text | Challenger | Challenge | Challenge text | Days open | Reviews of the target |
| --- | --- | --- | --- | --- | ---: | --- |
| c:demo:001 | The registry publishes a signed list of approved devices every week, and every device on the list carries a certificate... | p:critic | c:demo:010 | Is the weekly list really signed? The sample \\| list we saw had no signature. | 10 | c:demo:020 sustains (p:bob); c:demo:021 weakens (p:critic) |
";

const REPUTATION_MD: &str = "# Reputation in demo

Generated 2026-10-29T12:00:00Z. 5 perspectives. Counts only, no score: \"Answered\" is the perspective's claims an `answers` edge points at, \"Closed\" its challenges that an `answers` or `resolves` edge closed.

| Perspective | Name | Claims | Sourced | Memory | Inferred | Retracted | Superseded | Answered | Challenges raised | Closed | Challenges received | Reviews given |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| p:alice | Alice | 4 | 3 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 2 | 0 |
| p:bob | Bob | 5 | 1 | 2 | 2 | 0 | 1 | 0 | 1 | 1 | 1 | sustains 1 |
| p:critic | Critic | 4 | 0 | 2 | 2 | 0 | 0 | 1 | 2 | 1 | 0 | weakens 1 |
| p:judge | Judge | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| p:quiet | Quiet | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
";

#[test]
fn the_status_view_of_the_fixture() {
    let r = render("status");
    assert_eq!(r.markdown, STATUS_MD);
    assert_eq!(r.rows, 5);
}

#[test]
fn the_disputes_view_lists_only_the_open_challenge() {
    let r = render("disputes");
    assert_eq!(r.markdown, DISPUTES_MD);
    assert_eq!(r.rows, 1);
}

#[test]
fn the_reputation_view_counts_and_does_not_score() {
    let r = render("reputation");
    assert_eq!(r.markdown, REPUTATION_MD);
    assert_eq!(r.rows, 5);
}

#[test]
fn views_are_byte_stable_whatever_order_the_rows_arrive_in() {
    for view in views::VIEWS {
        let mut claims = rows("claims");
        let mut edges = rows("edges");
        let mut perspectives = rows("perspectives");
        claims.reverse();
        edges.reverse();
        perspectives.reverse();
        // A row seen twice, as when the store moves between pages.
        claims.push(claims[0].clone());
        let shuffled = Graph::from_rows(&claims, &edges, &perspectives).unwrap();
        let a = views::render(view, "demo", &shuffled, NOW).markdown;
        let b = render(view).markdown;
        assert_eq!(a, b, "{view}");
        assert_eq!(a, render(view).markdown, "{view} renders the same twice");
    }
}

#[test]
fn an_empty_graph_renders_every_view_with_no_rows() {
    let empty = Graph::from_rows(&[], &[], &[]).unwrap();
    let status = views::render("status", "g", &empty, NOW);
    assert_eq!(status.rows, 0);
    assert!(status.markdown.contains("| live | 0 |") && status.markdown.ends_with("Every claim is live.\n"));
    let disputes = views::render("disputes", "g", &empty, NOW);
    assert_eq!((disputes.rows, disputes.markdown.ends_with("No challenge is open.\n")), (0, true));
    let reputation = views::render("reputation", "g", &empty, NOW);
    assert_eq!((reputation.rows, reputation.markdown.ends_with("The graph has no perspectives.\n")), (0, true));
}

#[test]
fn a_claim_author_without_a_perspective_row_still_gets_a_row() {
    let graph = Graph::from_rows(&rows("claims"), &rows("edges"), &[]).unwrap();
    let r = views::render("reputation", "demo", &graph, NOW);
    assert_eq!(r.rows, 4, "p:quiet has no claims and no row");
    assert!(r.markdown.contains("| p:alice |  | 4 |"));
}

// ---- rounds ----

fn request(view: &str) -> Value {
    json!({"request_id": "r1", "graph_id": "demo", "view": view, "dry_run": true,
        "view_now": NOW, "out_dir": "", "out_dir_original": "", "note": "", "submitted_at": "x"})
}

/// The state and answers of a final round: every row already read and the
/// page after the last one empty.
fn final_round(view: &str) -> Value {
    let mut input = request(view);
    let mut state = pager::start(specs(view));
    let mut results = Map::new();
    for spec in specs(view) {
        let rows = rows(spec.key);
        state["cursors"][spec.key] = json!({"next": rows.len(), "done": false, "short": true});
        state["rows"][spec.key] = Value::Array(rows.clone());
        results.insert(format!("{}@{}", spec.key, rows.len()), json!({"result": {"rows": []}}));
    }
    input["state"] = state;
    input["data_results"] = Value::Object(results);
    input
}

#[test]
fn the_first_round_asks_for_the_graph_only() {
    let mut input = request("status");
    input["data_calls"] = json!(true);
    let out = run(&input);
    let requests = out["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 16);
    assert_eq!(requests[0]["id"], "claims@0");
    assert_eq!(requests[8]["id"], "edges@0");
    assert!(requests.iter().all(|r| r["query"]["filter"] == json!({"graph_id": {"_eq": "demo"}})));
    input["view"] = json!("reputation");
    let out = run(&input);
    let requests = out["data_calls"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 15);
    assert_eq!(requests[10]["query"]["collection"], "Perspective");
}

#[test]
fn the_final_round_is_a_flat_receipt() {
    for (view, md, rows) in [("status", STATUS_MD, 5), ("disputes", DISPUTES_MD, 1), ("reputation", REPUTATION_MD, 5)] {
        let receipt = run(&final_round(view));
        assert_eq!(receipt, json!({
            "receipt_id": "view-r1", "graph_id": "demo", "view": view, "ok": true, "error": "",
            "rows": rows, "markdown": md, "file": "", "generated_at": NOW,
        }), "{view}");
        assert!(receipt.as_object().unwrap().values().all(|v| !v.is_object() && !v.is_array()));
    }
}

#[test]
fn rounds_continue_until_every_collection_is_read() {
    // A host serving the fixture from memory, one short page at a time.
    let mut input = request("reputation");
    input["data_calls"] = json!(true);
    let fixture = fixture();
    for round in 0.. {
        assert!(round < 64);
        let out = run(&input);
        let Some(calls) = out.get("data_calls") else {
            assert_eq!(out["ok"], true, "{out}");
            assert_eq!(out["markdown"], REPUTATION_MD);
            return;
        };
        let mut results = Map::new();
        for r in calls["requests"].as_array().unwrap() {
            let (key, _) = r["id"].as_str().unwrap().split_once('@').unwrap();
            let offset = r["query"]["offset"].as_u64().unwrap() as usize;
            let page: Vec<Value> = fixture[key].as_array().unwrap().iter().skip(offset).take(3).cloned().collect();
            results.insert(r["id"].as_str().unwrap().into(), json!({"result": {"rows": page}}));
        }
        input["data_results"] = Value::Object(results);
        input["state"] = calls["state"].clone();
    }
}

#[test]
fn the_view_is_written_under_the_graph_folder() {
    let dir = std::env::temp_dir().join(format!("claim_views-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut input = final_round("status");
    input["dry_run"] = json!(false);
    input["out_dir"] = json!(dir.to_str().unwrap());
    let receipt = run(&input);
    assert_eq!((receipt["ok"].as_bool(), receipt["file"].as_str()), (Some(true), Some("demo/status.md")), "{receipt}");
    assert_eq!(std::fs::read_to_string(dir.join("demo/status.md")).unwrap(), STATUS_MD);
    assert_eq!(std::fs::read_dir(dir.join("demo")).unwrap().count(), 1, "no temporary file is left");
    // dry_run and no out_dir write nothing.
    let mut input = final_round("disputes");
    input["out_dir"] = json!(dir.to_str().unwrap());
    assert_eq!(run(&input)["file"], "");
    assert!(!dir.join("demo/disputes.md").exists());
    let mut input = final_round("disputes");
    input["dry_run"] = json!(false);
    assert_eq!(run(&input)["file"], "");
    // A folder that cannot be written is a failed receipt.
    let mut input = final_round("status");
    input["dry_run"] = json!(false);
    input["out_dir"] = json!(dir.join("demo/status.md").to_str().unwrap());
    let receipt = run(&input);
    assert_eq!(receipt["ok"], false);
    assert!(receipt["error"].as_str().unwrap().starts_with("cannot create the folder for demo/status.md"), "{receipt}");
}

#[test]
fn every_error_is_a_failed_receipt() {
    let error = |input: Value| {
        let r = run(&input);
        assert_eq!(r["ok"], false, "{r}");
        assert_eq!((r["rows"].as_u64(), r["markdown"].as_str(), r["file"].as_str()), (Some(0), Some(""), Some("")));
        r["error"].as_str().unwrap().to_owned()
    };
    let with = |key: &str, value: Value| {
        let mut input = request("status");
        input["data_calls"] = json!(true);
        input[key] = value;
        input
    };
    assert_eq!(error(with("request_id", json!(""))), "request_id is required: the host correlates the receipt with it");
    assert_eq!(error(with("graph_id", Value::Null)), "graph_id is required: the graph to view");
    assert_eq!(error(with("graph_id", json!("../x"))), "graph_id \"../x\" is not a name of letters, digits, '-', '_' and '.'");
    assert_eq!(error(with("view", json!("summary"))), "view must be one of status, disputes and reputation");
    assert_eq!(error(with("view", json!(3))), "view must be a string");
    assert_eq!(error(with("dry_run", json!("yes"))), "dry_run must be true or false");
    assert_eq!(error(with("view_now", json!("today"))), "view_now must be an ISO-8601 date or UTC timestamp");
    let mut no_calls = request("status");
    no_calls.as_object_mut().unwrap().remove("data_calls");
    assert!(error(no_calls).starts_with("the host offers this plugin no data calls"));
    let mut lost = final_round("status");
    lost.as_object_mut().unwrap().remove("state");
    assert_eq!(error(lost), "the host returned data results without the plugin's state");
    let mut refused = final_round("status");
    refused["data_results"]["claims@14"] = json!({"error": "query refused"});
    assert_eq!(error(refused), "the Claim query failed: query refused");
    assert_eq!(error(json!([])), "the input must be a JSON object");
    let r = run(&with("view", json!("summary")));
    assert_eq!((r["receipt_id"].as_str(), r["graph_id"].as_str(), r["view"].as_str(), r["generated_at"].as_str()),
        (Some("view-r1"), Some("demo"), Some("summary"), Some(NOW)));
}

#[test]
fn long_markdown_is_cut_at_a_line_with_a_final_note() {
    let line = "| c:x:001 | contested | something long enough to fill a line | p:a |\n";
    let markdown = line.repeat(2000);
    assert!(markdown.chars().count() > MAX_INLINE_CHARS);
    for file in ["", "g/status.md"] {
        let cut = inline(&markdown, file);
        assert!(cut.chars().count() <= MAX_INLINE_CHARS, "{}", cut.chars().count());
        let (body, note) = cut.rsplit_once("\n_Truncated").unwrap();
        assert!(body.ends_with('\n') && body.lines().all(|l| format!("{l}\n") == line));
        assert!(note.starts_with(&format!(": the view is {} characters; ", markdown.chars().count())));
        assert!(note.contains(if file.is_empty() { "request it with an out_dir" } else { "the whole view is in g/status.md" }));
        assert!(cut.ends_with("._\n"));
    }
    let short = "# Status\n";
    assert_eq!(inline(short, ""), short);
    let exact = "é".repeat(MAX_INLINE_CHARS);
    assert_eq!(inline(&exact, ""), exact, "the cap counts characters, not bytes");
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
        assert_eq!(run(&case["input"]), case["expect"], "{}", path.display());
        checked += 1;
    }
    assert_eq!(checked, 11);
}
