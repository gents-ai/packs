//! Agent options to AgentTarget ids (L2 section 4.8), through step 0 of an inline script.
use serde_json::{Value, json};
use workflow::invoke;
use workflow::options::map;

const ALL: [&str; 7] = [
    "wf-read-only",
    "wf-read-write",
    "wf-execute",
    "wf-all",
    "wf-read-only@grok-4",
    "wf-read-only@grok-4-high",
    "wf-read-write@high",
];

/// Step 0 of a script that makes one `agent` call with `opts` (a rhai map literal); a throw
/// is caught and becomes the result.
fn step0(opts: &str, targets: &[&str]) -> Value {
    let script = format!(
        r##"let meta = #{{ name: "opts", description: "option mapping" }};
try {{ agent("work", {opts}); }} catch (e) {{ complete(e); }}
"##
    );
    invoke(json!({
        "protocol": 1, "argv": ["run", "inline"], "script": script,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"cwd": "/work"}, "ceiling": {"targets": targets, "max_live": 4, "remaining_spawns": 9, "dynamic": "run", "dynamic_targets": targets}
    }))
}

fn spawned(opts: &str, targets: &[&str]) -> Value {
    let out = step0(opts, targets);
    let spawn = &out["effects"]["spawn"];
    assert_eq!(spawn.as_array().map(Vec::len), Some(1), "{out}");
    spawn[0].clone()
}

fn thrown(opts: &str, targets: &[&str]) -> String {
    let out = step0(opts, targets);
    out["result"]
        .as_str()
        .unwrap_or_else(|| panic!("{out}"))
        .into()
}

#[test]
fn each_grok_option_maps_or_refuses() {
    let target = |opts: &str| spawned(opts, &ALL)["target"].clone();
    assert_eq!(target("#{}"), "wf-read-only");
    for mode in ["read-only", "read-write", "execute", "all"] {
        assert_eq!(
            target(&format!(r#"#{{ capability_mode: "{mode}" }}"#)),
            format!("wf-{mode}").as_str()
        );
    }
    assert_eq!(
        target(r#"#{ capability_mode: "read-only", agent_type: "wf-all" }"#),
        "wf-all"
    );
    assert_eq!(target(r#"#{ model: "grok-4" }"#), "wf-read-only@grok-4");
    assert_eq!(
        target(r#"#{ model: "grok-4", effort: "high" }"#),
        "wf-read-only@grok-4-high"
    );
    assert_eq!(
        target(r#"#{ capability_mode: "read-write", effort: "high" }"#),
        "wf-read-write@high"
    );
    assert_eq!(target("#{ max_output_tokens: 10 }"), "wf-read-only");

    let msg = thrown(r#"#{ model: "gpt-9" }"#, &ALL);
    assert!(
        msg.starts_with("workflow_invalid_input: no target wf-read-only@gpt-9 here; allowed: ")
    );
    assert!(msg.ends_with("Next: workflow help agent"), "{msg}");
    assert!(thrown(r#"#{ capability_mode: "root" }"#, &ALL).starts_with("workflow_invalid_input"));

    let msg = thrown("#{ isolation_worktree: true }", &ALL);
    assert!(msg.starts_with("workflow_not_available: "), "{msg}");
    assert!(
        msg.contains("Next: use capability_mode read-write"),
        "{msg}"
    );
    let msg = thrown("#{ fork_context: true }", &ALL);
    assert!(
        msg.starts_with("workflow_invalid_input: fork_context"),
        "{msg}"
    );
    assert_eq!(target("#{ isolation_worktree: false }"), "wf-read-only");

    let opts = rhai::Map::new();
    let mapped = map(&opts, &["wf-read-only".into()]).unwrap();
    assert_eq!(
        (mapped.target.as_str(), mapped.note),
        ("wf-read-only", None)
    );
    let mut opts = rhai::Map::new();
    opts.insert("agent_type".into(), "wf-all".into());
    opts.insert("phase".into(), "scan".into());
    let mapped = map(&opts, &["wf-all".into()]).unwrap();
    assert_eq!(mapped.phase.as_deref(), Some("scan"));
    assert!(mapped.note.is_some_and(|n| n.contains("agent_type wf-all")));
}

#[test]
fn target_outside_ceiling_throws_catchable_capability_unavailable() {
    for opts in [
        r#"#{ agent_type: "wf-execute" }"#,
        r#"#{ capability_mode: "all" }"#,
    ] {
        let msg = thrown(opts, &["wf-read-only", "wf-read-write"]);
        assert!(
            msg.starts_with("workflow_capability_unavailable: "),
            "{msg}"
        );
        assert!(
            msg.contains("allowed: wf-read-only, wf-read-write"),
            "{msg}"
        );
        assert!(msg.ends_with("Next: workflow help agent"), "{msg}");
    }
}

#[test]
fn input_without_ceiling_targets_allows_no_target() {
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"],
        "script": r##"let meta = #{ name: "bare", description: "no ceiling" };
try { agent("work"); } catch (e) { complete(e); }
"##,
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""},
        "caller": {"cwd": "/work"}, "ceiling": {"dynamic": "run"}
    }));
    let msg = out["result"].as_str().unwrap_or_else(|| panic!("{out}"));
    assert!(
        msg.starts_with("workflow_capability_unavailable: "),
        "{msg}"
    );
}

#[test]
fn cwd_escapes_are_refused() {
    assert_eq!(spawned(r#"#{ cwd: "pkg/core" }"#, &ALL)["cwd"], "pkg/core");
    assert!(spawned("#{}", &ALL).get("cwd").is_none());
    for bad in ["../up", "a/../../b", "/etc", ""] {
        let msg = thrown(&format!(r#"#{{ cwd: "{bad}" }}"#), &ALL);
        assert!(msg.starts_with("workflow_invalid_input: "), "{bad}: {msg}");
    }
}

#[test]
fn labels_over_256_bytes_are_truncated_and_named() {
    let title = |label: &str| {
        spawned(&format!(r#"#{{ label: "{label}" }}"#), &ALL)["title"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(title("short"), "short");
    let exact = "x".repeat(256);
    assert_eq!(title(&exact), exact);
    for long in ["y".repeat(300), "é".repeat(200)] {
        let t = title(&long);
        assert!(t.len() <= 256 && t.contains("truncated"), "{t}");
        assert!(t.contains(&long.len().to_string()), "{t}");
    }
}
