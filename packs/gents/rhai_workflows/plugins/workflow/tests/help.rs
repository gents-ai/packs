//! `help`, `help <verb>` and `help <topic>` serve embedded text (L2 section 4.17). The tests
//! assert structure, sizes and behaviour, never wording (U15).
use serde_json::{Value, json};
use workflow::errors::CODES;
use workflow::help::TOPICS;
use workflow::invoke;
use workflow::verbs::VERBS;

fn text_of(argv: &[&str], extra: Value) -> String {
    let mut input = json!({"argv": argv});
    if let (Some(into), Some(from)) = (input.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    let out = invoke(input);
    out["result"].as_str().expect(&out.to_string()).to_string()
}

/// `argv` as a step-0 job call, the only call that carries a ceiling.
fn text_of_job_call(argv: &[&str], targets: &[&str]) -> String {
    let out = invoke(json!({
        "protocol": 1, "argv": argv, "mounts": [],
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"session_id": "s", "tool_call_id": "t1", "cwd": "/nowhere"},
        "ceiling": {"targets": targets}
    }));
    out["result"].as_str().expect(&out.to_string()).to_string()
}

#[test]
fn help_verb_and_topic_serve_embedded_text() {
    let index = text_of(&["help"], json!({}));
    for (verb, _) in VERBS {
        assert!(index.contains(verb), "{verb}: {index}");
        let one = text_of(&["help", verb], json!({}));
        assert!(one.contains(verb), "{verb}: {one}");
    }
    for (topic, body) in TOPICS {
        assert!(index.contains(topic), "{topic}: {index}");
        let served = text_of(&["help", topic], json!({}));
        assert!(served.starts_with(body), "{topic}: {served}");
    }
    let unknown = invoke(json!({"argv": ["help", "nonsense"]}));
    assert_eq!(
        unknown["fail"]["code"], "workflow_invalid_input",
        "{unknown}"
    );
}

#[test]
fn topics_and_tool_md_respect_their_size_caps() {
    for (topic, body) in TOPICS {
        assert!(body.len() <= 3 * 1024, "{topic} is {} B", body.len());
    }
    let tool_md = include_str!("../TOOL.md");
    assert!(tool_md.len() <= 700, "TOOL.md is {} B", tool_md.len());
}

#[test]
fn help_agent_lists_exactly_the_ceiling_targets() {
    let targets = ["wf-read-only", "wf-execute", "wf-execute@fast"];
    let served = text_of_job_call(&["help", "agent"], &targets);
    let line = served
        .lines()
        .find_map(|l| l.strip_prefix("Targets here: "))
        .expect(&served);
    assert_eq!(line.split(", ").collect::<Vec<_>>(), targets, "{served}");

    let bare = text_of(&["help", "agent"], json!({}));
    assert!(!bare.contains("Targets here: none"), "{bare}");
    assert!(bare.contains("job ceiling"), "{bare}");
}

const HOST_CODES: [&str; 7] = [
    "plugin_job_stalled",
    "plugin_job_policy_denied",
    "plugin_job_not_available",
    "plugin_job_engine_changed",
    "plugin_job_timed_out",
    "plugin_job_depth_exceeded",
    "plugin_job_step_failed",
];

#[test]
fn help_errors_lists_12_engine_and_7_host_codes_with_next() {
    let served = text_of(&["help", "errors"], json!({}));
    assert_eq!(CODES.len(), 12);
    for code in CODES.iter().chain(&HOST_CODES) {
        let line = served.lines().find(|l| l.contains(code)).expect(code);
        assert!(line.contains("Next:"), "{code}: {line}");
    }
    let line = served
        .lines()
        .find(|l| l.contains("plugin_job_not_available"))
        .unwrap();
    assert!(line.contains("--grant-authority"), "{line}");
}

#[test]
fn help_errors_launch_failed_covers_script_ends_and_names_no_status_call() {
    let served = text_of(&["help", "errors"], json!({}));
    let line = served
        .lines()
        .find(|l| l.contains("workflow_launch_failed"))
        .unwrap();
    assert!(line.contains("complete()"), "{line}");
    assert!(line.contains("uncaught error"), "{line}");
    assert!(!line.contains("workflow status"), "{line}");
}

#[test]
fn help_doctor_reports_engine_facts_and_missing_job_block() {
    let bare = text_of(&["help", "doctor"], json!({}));
    assert!(bare.contains("no job block"), "{bare}");
    assert!(bare.contains("gents-rhai/1"), "{bare}");
    assert!(bare.contains("gents diagnose plugin_jobs"), "{bare}");
    assert!(
        bare.contains("gents pack install <source> --grant-authority"),
        "{bare}"
    );
    assert!(bare.contains("project tier: off"), "{bare}");
    let pin = include_str!("../Cargo.toml");
    let version = bare
        .split("rhai ")
        .nth(1)
        .and_then(|r| r.split([',', ' ']).next());
    let version = version.expect(&bare);
    assert!(
        pin.contains(&format!("rhai = {{ version = \"={version}\"")),
        "{bare}"
    );

    let mounts = json!([{"pack": "", "class": "project", "digest": "", "path": "/nowhere", "trusted": false}]);
    let projected = text_of(&["help", "doctor"], json!({"asset_mounts": mounts}));
    assert!(projected.contains("project tier: on"), "{projected}");
    let job = text_of_job_call(&["help", "doctor"], &["wf-read-only"]);
    assert!(job.contains("job block: present"), "{job}");
}

/// The host side of the draft reads: answers each `data_calls` output with `rows_of(request)`
/// and calls `invoke` again with the base input, that round's results and the echoed state.
fn doctor_with_rows(rows_of: impl Fn(&Value) -> Vec<Value>) -> String {
    let base = json!({"argv": ["help", "doctor"], "data_calls": true});
    let mut input = base.clone();
    for _ in 0..8 {
        let out = invoke(input.clone());
        let Some(calls) = out.get("data_calls") else {
            return out["result"].as_str().expect(&out.to_string()).to_string();
        };
        assert!(out.get("writes").is_none(), "a round writes nothing: {out}");
        let mut results = serde_json::Map::new();
        for request in calls["requests"].as_array().expect(&out.to_string()) {
            let rows = rows_of(request);
            results.insert(
                request["id"].as_str().unwrap().into(),
                json!({"result": {"rows": rows}}),
            );
        }
        input = base.clone();
        input["data_results"] = Value::Object(results);
        input["state"] = calls["state"].clone();
    }
    panic!("the call never ended");
}

fn script_rows(ids: &[String]) -> Vec<Value> {
    ids.iter().map(|id| json!({"script_id": id})).collect()
}

#[test]
fn help_doctor_counts_drafts_and_says_when_it_cannot_see_them() {
    let bare = text_of(&["help", "doctor"], json!({}));
    assert!(bare.contains("drafts: not visible in this call"), "{bare}");

    let ids = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let counted = doctor_with_rows(|request| match request["query"]["collection"].as_str() {
        Some("WfScript") => script_rows(&ids(&["a", "b", "b", "c"])),
        Some("WfName") => script_rows(&ids(&["a"])),
        other => panic!("unexpected read {other:?}"),
    });
    assert!(counted.contains("drafts: 2\n"), "{counted}");
}

#[test]
fn help_doctor_counts_drafts_past_one_full_page() {
    let many: Vec<String> = (0..300).map(|n| format!("s{n}")).collect();
    let counted = doctor_with_rows(|request| {
        let offset = request["query"]["offset"].as_u64().unwrap_or(0) as usize;
        match request["query"]["collection"].as_str() {
            Some("WfScript") => script_rows(&many[offset.min(300)..(offset + 256).min(300)]),
            _ => script_rows(&many[..1]),
        }
    });
    assert!(counted.contains("drafts: 299\n"), "{counted}");
}

#[test]
fn help_doctor_warns_wf_read_only_web_pairs_files_with_web_tools() {
    let served = text_of(&["help", "doctor"], json!({}));
    let line = served
        .lines()
        .find(|l| l.contains("wf-read-only-web"))
        .expect(&served);
    assert!(line.contains("gents/web_fetch"), "{line}");
    assert!(line.contains("gents/rhai_workflows_web"), "{line}");
    assert!(line.contains("operator"), "{line}");
}

/// A `workflows` mount at a scratch dir holding `files` and an `index.json` of `index`.
fn workflows_mount(test: &str, pack: &str, files: &[(&str, &str)], index: Value) -> Value {
    let dir = std::env::temp_dir().join(format!("help-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        std::fs::write(dir.join(name), source).unwrap();
    }
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    json!({"pack": pack, "class": "workflows", "digest": "", "path": dir, "trusted": false})
}

fn workflow_source(name: &str) -> String {
    format!("let meta = #{{ name: {name:?}, description: \"d\" }};\ncomplete(1);\n")
}

#[test]
fn help_doctor_reports_index_meta_mismatches() {
    let line_of = |mounts: Value| {
        let served = text_of(&["help", "doctor"], json!({"asset_mounts": mounts}));
        let line = served
            .lines()
            .find_map(|l| l.strip_prefix("index/meta mismatches: "));
        line.expect(&served).to_string()
    };
    let consistent = workflows_mount(
        "consistent",
        "acme/ok",
        &[("alpha.rhai", &workflow_source("alpha"))],
        json!([{"name": "alpha", "file": "alpha.rhai", "description": "d"}]),
    );
    assert_eq!(line_of(json!([consistent])), "none");

    let broken = workflows_mount(
        "broken",
        "acme/bad",
        &[("review.rhai", &workflow_source("review-changes"))],
        json!([
            {"name": "review", "file": "review.rhai", "description": "d"},
            {"name": "gone", "file": "gone.rhai", "description": "d"},
            {"name": "nofile", "description": "d"},
            {"name": "escape", "file": "../escape.rhai", "description": "d"}
        ]),
    );
    let line = line_of(json!([broken]));
    assert!(
        line.contains("acme/bad/review.rhai: index says review, file says review-changes"),
        "{line}"
    );
    assert!(
        line.contains("acme/bad/gone.rhai: not a valid workflow"),
        "{line}"
    );
    let unusable = line.matches("acme/bad: an index entry has no usable file");
    assert_eq!(unusable.count(), 2, "{line}");

    let unreadable = workflows_mount("unreadable", "acme/raw", &[], json!({}));
    let line = line_of(json!([unreadable]));
    assert!(
        line.contains("acme/raw: index.json does not read"),
        "{line}"
    );
}

#[test]
fn help_grok_lists_the_declared_divergences() {
    let served = text_of(&["help", "grok"], json!({}));
    for divergence in ["budget()", "write_scratch_file", "workflow_depth_exceeded"] {
        assert!(served.contains(divergence), "{divergence}: {served}");
    }
}
