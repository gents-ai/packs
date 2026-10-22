//! `export` (L2 section 4.15; row 15): a kept workflow returned inline as a pack with the pack
//! digest `gents pack build` computes. The tests assert codes, files and the digest vector,
//! never Grok text (U15).
use serde_json::{Value, json};
use std::collections::BTreeMap;
use workflow::data::{WfName, WfScript};
use workflow::fakehost::admit::view;
use workflow::fakehost::cli::facts;
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::{export, invoke};

const REVIEW: &str = "let meta = #{ name: \"review-changes\", description: \"Review a diff\", \
                      when_to_use: \"before a merge\" };\nlet r = agent(\"review\");\n\
                      complete(r.output);\n";

fn run_as(source: &str, tool_call_id: &str) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!(tool_call_id);
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": source, "args": {}}),
        host: facts,
    }
}

/// A later non-job call `tool_call_id` with `argv`.
fn tool(host: &FakeHost, tool_call_id: &str, argv: &[&str]) -> Call {
    let mut facts = facts("/work");
    facts.caller["tool_call_id"] = json!(tool_call_id);
    let jobs: Vec<Value> = (host.state.rows.values())
        .map(|r| {
            json!({"row_doc_id": r.row_doc_id, "tool_call_id": r.tool_call_id,
                   "handle": r.handle, "state": r.state})
        })
        .collect();
    let chain = host.jobs.chains.values().next();
    let children = chain.map_or(json!({}), |c| json!(view(&host.state, c)));
    Call {
        kind: Kind::ToolCall,
        input: json!({"protocol": 1, "argv": argv, "jobs": jobs, "children": children}),
        host: facts,
    }
}

fn call(host: &mut FakeHost, tool_call_id: &str, argv: &[&str]) -> Value {
    let call = tool(host, tool_call_id, argv);
    host.tool_call(&call, invoke).expect("the host accepts it")
}

/// A host that ran `source` and kept it under its own name.
fn kept(source: &str) -> FakeHost {
    let mut host = FakeHost::default();
    host.step0(&run_as(source, "t1"), invoke).unwrap();
    let name = call(&mut host, "k1", &["keep", "review-changes"]);
    assert!(name.get("fail").is_none(), "{name}");
    host
}

fn exported(host: &mut FakeHost, argv: &[&str]) -> String {
    let out = call(host, "e1", argv);
    assert!(export::is_export(&tool(host, "e2", argv).input));
    out["result"].as_str().expect(&out.to_string()).to_string()
}

/// The files of a receipt: each is `=== <path> (<n> bytes) ===`, its `n` bytes, a newline.
fn files_of(text: &str) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut rest = &text[text.find("=== ").expect(text)..];
    while let Some(r) = rest.strip_prefix("=== ") {
        let (head, body) = r.split_once(" ===\n").unwrap();
        let (path, n) = head.rsplit_once(" (").unwrap();
        let n: usize = n.strip_suffix(" bytes)").unwrap().parse().unwrap();
        files.insert(path.to_string(), body[..n].to_string());
        rest = body[n..].strip_prefix('\n').unwrap();
    }
    assert!(rest.is_empty(), "{rest:?}");
    files
}

fn canonical(path: &str) -> bool {
    let snake = |s: &str| {
        s.split('.').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
        })
    };
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        let ok = match parts.peek() {
            Some(_) => snake(part) && !part.contains('.'),
            None => part == "README.md" || snake(part),
        };
        if !ok {
            return false;
        }
    }
    true
}

#[test]
fn export_returns_four_files_with_canonical_spelling() {
    let mut host = kept(REVIEW);
    let text = exported(&mut host, &["export", "review-changes"]);
    let files = files_of(&text);
    let paths: Vec<&str> = files.keys().map(String::as_str).collect();
    let want = [
        "README.md",
        "manifest.json",
        "workflows/index.json",
        "workflows/review_changes.rhai",
    ];
    assert_eq!(paths, want);
    let line = text
        .lines()
        .find(|l| l.starts_with("files: "))
        .expect(&text);
    for path in want {
        assert!(line.contains(path), "{line}");
        assert!(canonical(path), "{path}");
    }
    assert_eq!(files["workflows/review_changes.rhai"], REVIEW);
    assert!(
        files["README.md"].contains("\n## "),
        "the pack check wants a heading"
    );

    let manifest: Value = serde_json::from_str(&files["manifest.json"]).unwrap();
    assert_eq!(manifest["kind"], "assets");
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["namespace"], "local");
    assert_eq!(manifest["name"], "review_changes");
    assert_eq!(manifest["version"], "0.1.0");
    let assets = [
        "README.md",
        "workflows/index.json",
        "workflows/review_changes.rhai",
    ];
    assert_eq!(manifest["assets"], json!(assets));
    let index: Value = serde_json::from_str(&files["workflows/index.json"]).unwrap();
    let entry = json!([{"name": "review-changes", "file": "review_changes.rhai",
                        "description": "Review a diff", "when_to_use": "before a merge"}]);
    assert_eq!(index, entry);

    // The flags choose the namespace and the pack's name; the script keeps its own.
    let text = exported(
        &mut host,
        &[
            "export",
            "review-changes",
            "--namespace",
            "acme",
            "--pack-name",
            "tools",
        ],
    );
    let files = files_of(&text);
    let manifest: Value = serde_json::from_str(&files["manifest.json"]).unwrap();
    assert_eq!(
        (&manifest["namespace"], &manifest["name"]),
        (&json!("acme"), &json!("tools"))
    );
    assert!(files.contains_key("workflows/review_changes.rhai"));
    assert!(text.starts_with("Exported 'review-changes' as pack acme/tools (digest sha256:"));

    // A name that is not kept, and a pack name that is not snake_case, are refused.
    let out = call(&mut host, "e3", &["export", "nope"]);
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");
    let out = call(
        &mut host,
        "e4",
        &["export", "review-changes", "--pack-name", "Bad-Name"],
    );
    assert_eq!(out["fail"]["code"], "workflow_invalid_input", "{out}");
}

#[test]
fn ported_digest_equals_the_gents_vector() {
    let vector: Value = serde_json::from_str(include_str!("fixtures/export_digest.json")).unwrap();
    let files: BTreeMap<String, String> = serde_json::from_value(vector["files"].clone()).unwrap();
    let digest = vector["digest"].as_str().unwrap();
    assert!(digest.starts_with("sha256:"), "{digest}");
    assert_eq!(export::digest(&files), digest);

    // The files an export writes for the vector's script are the vector's files.
    let source = files["workflows/review_changes.rhai"].clone();
    let mut host = kept(&source);
    let text = exported(&mut host, &["export", "review-changes"]);
    assert_eq!(files_of(&text), files);
    assert!(text.contains(digest), "{text}");
}

#[test]
fn export_receipt_is_answer_first_with_expect_digest() {
    let mut host = kept(REVIEW);
    let text = exported(&mut host, &["export", "review-changes"]);
    let digest = export::digest(&files_of(&text));
    let first = text.lines().next().unwrap();
    assert_eq!(
        first,
        format!("Exported 'review-changes' as pack local/review_changes (digest {digest}).")
    );
    let next = text.lines().nth(1).unwrap();
    let want = format!(
        "Next: write these files under ./review_changes/, then run \
         `gents pack install ./review_changes --expect-digest {digest}`."
    );
    assert_eq!(next, want);
    assert!(text.contains("trusted_packs"), "{text}");
}

#[test]
fn xai_bundled_local_only_scripts_are_refused() {
    let mut host = FakeHost::default();
    let id = "a".repeat(64);
    let mut script = WfScript::draft(&id, REVIEW, "path", "review-changes", "Review a diff");
    script.license = Some("xai-bundled-local-only".into());
    let name = WfName {
        name: "review-changes".into(),
        version: 1,
        script_id: id,
        description: "Review a diff".into(),
        when_to_use: "before a merge".into(),
        state: "kept".into(),
    };
    let stamps = ["session_id", "tool_call_doc_id"].map(|k| (k.to_string(), k.to_string()));
    let mut stamps = BTreeMap::from(stamps);
    stamps.insert("node_did".into(), "did:cli".into());
    let writes = json!([script.write(), name.write()]);
    host.state.apply_writes(&writes, &stamps).unwrap();

    let out = call(&mut host, "e1", &["export", "review-changes"]);
    assert_eq!(out["fail"]["code"], "workflow_invalid_input", "{out}");
    let message = out["fail"]["message"].as_str().unwrap();
    assert!(message.contains("xai-bundled-local-only"), "{message}");
    assert!(message.contains("Next: "), "{message}");
    assert!(out.get("result").is_none(), "{out}");
}
