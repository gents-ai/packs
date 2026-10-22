//! `help`, `check` and `list` on the raw-argument shape (L2 sections 4.3, 4.5 and 4.14). The
//! tests assert behaviour and codes, never wording (U15).
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use workflow::smoke::{self, End};
use workflow::{invoke, verbs};

const DEMO: &str = r##"let meta = #{ name: "demo", description: "ask once" };
let r = agent("say hi");
complete(#{ said: r.output });
"##;

fn script_id(source: &str) -> String {
    let mut hash = Sha256::new();
    ["gents-rhai/1", source]
        .iter()
        .for_each(|p| hash.update(p.as_bytes()));
    format!("{:x}", hash.finalize())
}

/// A scratch directory for one test, empty.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("raw_verbs-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mount(pack: &str, class: &str, dir: &std::path::Path) -> Value {
    json!({"pack": pack, "class": class, "digest": "", "path": dir, "trusted": false})
}

fn workflow_file(name: &str, when_to_use: Option<&str>) -> String {
    let when = when_to_use.map_or(String::new(), |w| format!(", when_to_use: {w:?}"));
    format!("let meta = #{{ name: {name:?}, description: \"d\"{when} }};\ncomplete(1);\n")
}

fn code_of(out: &Value) -> &str {
    out.pointer("/fail/code")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

#[test]
fn check_receipt_names_script_id_and_one_wfscript_insert() {
    let out = invoke(json!({"argv": ["check", "--script"], "script": DEMO}));
    let id = script_id(DEMO);
    let receipt = out["result"].as_str().expect(&out.to_string());
    assert!(receipt.contains(&id), "{receipt}");
    assert!(receipt.contains("demo"), "{receipt}");
    let writes = out["writes"].as_array().expect("writes");
    assert_eq!(writes.len(), 1, "{writes:?}");
    let write = &writes[0];
    assert_eq!(write["op"], "insert");
    assert_eq!(write["collection"], "WfScript");
    assert_eq!(write["on_conflict"], "ignore");
    assert_eq!(write["fields"]["script_id"], id);
    assert_eq!(write["fields"]["source"], DEMO);
}

#[test]
fn check_refuses_an_invalid_script_and_one_that_never_ends() {
    let out = invoke(json!({"argv": ["check", "--script"], "script": "let x = 1;"}));
    assert_eq!(code_of(&out), "workflow_validation_failed", "{out}");
    let silent = r#"let meta = #{ name: "silent", description: "d" }; let x = 1;"#;
    let out = invoke(json!({"argv": ["check", "--script"], "script": silent}));
    assert_eq!(code_of(&out), "workflow_validation_failed", "{out}");
    assert!(out.get("writes").is_none(), "{out}");
}

#[test]
fn check_reads_a_path_and_a_pack_name() {
    let dir = scratch("check_reads");
    let file = dir.join("alpha.rhai");
    std::fs::write(&file, workflow_file("alpha", None)).unwrap();
    std::fs::write(
        dir.join("index.json"),
        json!([{"name": "alpha", "file": "alpha.rhai", "description": "d"}]).to_string(),
    )
    .unwrap();
    let by_path = invoke(json!({"argv": ["check"], "path": file}));
    assert!(
        by_path["result"].as_str().unwrap().contains("alpha"),
        "{by_path}"
    );
    let mounts = json!([mount("acme/tools", "workflows", &dir)]);
    let by_name = invoke(json!({"argv": ["check", "alpha"], "asset_mounts": mounts}));
    assert_eq!(
        by_name["writes"][0]["fields"]["script_id"],
        by_path["writes"][0]["fields"]["script_id"]
    );
    let missing = invoke(json!({"argv": ["check", "beta"], "asset_mounts": mounts}));
    assert_eq!(code_of(&missing), "workflow_resolve_failed", "{missing}");
}

#[test]
fn smoke_run_stops_at_first_terminal_or_64_calls() {
    let args = json!({});
    // The first terminal ends the run: the second agent call is never reached.
    let first = r#"let meta = #{ name: "one", description: "d" };
agent("a"); complete(1); agent("b");"#;
    let smoke = smoke::run(first, &args).unwrap();
    assert_eq!((smoke.end, smoke.calls), (End::Complete, 1));
    // No terminal: the run stops after 64 host calls.
    let endless = r#"let meta = #{ name: "endless", description: "d" };
for i in 0..100 { agent("call " + i); }
complete(1);"#;
    let smoke = smoke::run(endless, &args).unwrap();
    assert_eq!((smoke.end, smoke.calls), (End::CallLimit, 64));
    let out = invoke(json!({"argv": ["check", "--script"], "script": endless}));
    assert!(out["result"].is_string(), "{out}");
    // Every agent succeeds: with the minimal instance of its schema, or ok.
    let canned = r##"let meta = #{ name: "canned", description: "d" };
let r = agent("q", #{ output_schema: #{ "type": "object", "required": ["n"],
                                        "properties": #{ "n": #{ "type": "integer" } } } });
if !r.success || r.output.n != 0 { throw "schema reply"; }
let s = agent("p");
if !s.success || s.output != "ok" { throw "plain reply"; }
let both = parallel([#{ prompt: "x" }, #{ prompt: "y" }]);
if both.len() != 2 { throw "panel"; }
complete(1);"##;
    let smoke = smoke::run(canned, &args).unwrap();
    assert_eq!((smoke.end, smoke.calls), (End::Complete, 3));
}

#[test]
fn list_reads_index_and_meta_from_asset_mounts() {
    let dir = scratch("list_reads");
    let other = scratch("list_other");
    std::fs::write(
        dir.join("alpha.rhai"),
        workflow_file("alpha", Some("from the file")),
    )
    .unwrap();
    std::fs::write(
        dir.join("beta.rhai"),
        workflow_file("beta", Some("beta from meta")),
    )
    .unwrap();
    let index = json!([
        {"name": "beta", "file": "beta.rhai", "description": "d"},
        {"name": "alpha", "file": "alpha.rhai", "description": "d", "when_to_use": "alpha from index"},
    ]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    // A mount of another class is not the pack tier.
    std::fs::write(other.join("index.json"), index.to_string()).unwrap();
    let mounts = json!([
        mount("acme/tools", "workflows", &dir),
        mount("", "project", &other)
    ]);
    let out = invoke(json!({"argv": ["list"], "asset_mounts": mounts, "data_calls": true}));
    let text = out["result"].as_str().expect(&out.to_string());
    let (alpha, beta) = (
        text.find("alpha").expect(text),
        text.find("beta").expect(text),
    );
    assert!(alpha < beta, "names sort: {text}");
    assert!(text.contains("alpha from index"), "{text}");
    assert!(text.contains("beta from meta"), "{text}");
    assert!(
        !text.contains("from the file"),
        "the index wins for a listing field: {text}"
    );
    assert_eq!(
        text.matches("alpha").count(),
        2,
        "the project mount adds nothing: {text}"
    );
    assert!(out.get("writes").is_none(), "{out}");
}

#[test]
fn list_skips_index_files_outside_their_mount() {
    let root = scratch("list_escape");
    let dir = root.join("mount");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(root.join("out.rhai"), workflow_file("escaped", None)).unwrap();
    std::fs::write(dir.join("in.rhai"), workflow_file("inside", None)).unwrap();
    let absolute = root.join("out.rhai").display().to_string();
    let index = json!([
        {"name": "abs", "file": absolute, "description": "d"},
        {"name": "up", "file": "../out.rhai", "description": "d"},
        {"name": "dot", "file": "./in.rhai", "description": "d"},
        {"name": "inside", "file": "in.rhai", "description": "d"},
    ]);
    std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
    let mounts = json!([mount("acme/tools", "workflows", &dir)]);
    let out = invoke(json!({"argv": ["list"], "asset_mounts": mounts, "data_calls": true}));
    let text = out["result"].as_str().expect(&out.to_string());
    assert!(!text.contains("escaped"), "outside the mount: {text}");
    assert_eq!(text.matches("inside").count(), 1, "dot parts: {text}");
    let out =
        invoke(json!({"argv": ["check", "escaped"], "asset_mounts": mounts, "data_calls": true}));
    assert_eq!(code_of(&out), "workflow_resolve_failed", "{out}");
}

#[test]
fn a_name_in_two_packs_is_refused_naming_both() {
    let root = scratch("ambiguous");
    let mut mounts = Vec::new();
    for pack in ["acme/a", "gents/b"] {
        let dir = root.join(pack.replace('/', "-"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.rhai"), workflow_file("x", None)).unwrap();
        let index = json!([{"name": "x", "file": "x.rhai", "description": "d"}]);
        std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
        mounts.push(mount(pack, "workflows", &dir));
    }
    let out = invoke(json!({"argv": ["check", "x"], "asset_mounts": mounts, "data_calls": true}));
    assert_eq!(
        out["fail"]["message"],
        "workflow_resolve_failed: 'x' is in acme/a and gents/b. Next: workflow list",
        "{out}"
    );
}

#[test]
fn job_verbs_on_raw_shape_are_not_available() {
    let expected = json!({"fail": {"code": "workflow_not_available", "message":
        "workflow_not_available: this tool has no job settings here. Next: workflow help doctor"}});
    for verb in [
        "run", "resume", "answer", "stop", "pause", "status", "result",
    ] {
        assert_eq!(
            verbs::raw(&json!({"argv": [verb, "x"]})),
            expected,
            "{verb}"
        );
        assert_eq!(invoke(json!({"argv": [verb]})), expected, "{verb}");
    }
}

#[test]
fn help_indexes_verbs_and_answers_doctor() {
    let index = invoke(json!({"argv": ["help"]}));
    let text = index["result"].as_str().expect(&index.to_string());
    for verb in ["list", "check", "run", "help"] {
        assert!(text.contains(verb), "{verb}: {text}");
    }
    for topic in ["check", "doctor"] {
        let out = invoke(json!({"argv": ["help", topic]}));
        assert!(out["result"].is_string(), "{topic}: {out}");
    }
    let unknown = invoke(json!({"argv": ["help", "nonsense"]}));
    assert_eq!(code_of(&unknown), "workflow_invalid_input", "{unknown}");
    let unknown = invoke(json!({"argv": ["frobnicate"]}));
    assert_eq!(code_of(&unknown), "workflow_invalid_input", "{unknown}");
}

#[test]
fn help_lines_name_the_operands_the_parser_takes() {
    for (verb, operands) in [
        ("answer", "<key|handle> <text>"),
        ("resume", "[--answer t] [--budget n] [--engine current]"),
    ] {
        let out = invoke(json!({"argv": ["help", verb]}));
        let text = out["result"].as_str().expect(&out.to_string());
        assert!(text.contains(operands), "{verb}: {text}");
    }
}
