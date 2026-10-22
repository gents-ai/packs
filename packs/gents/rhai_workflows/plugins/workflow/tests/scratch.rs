//! Scratch files and the pure helpers (L2 sections 4.7 and 4.12, root C2.5), by direct calls
//! of `invoke` and on the fake host.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::helpers::render;
use workflow::invoke;
use workflow::scratch::{MAX_BYTES, MAX_FILES};

fn temp_cwd(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("wf-scratch-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into()
}

fn call(script: &str, host: workflow::fakehost::HostFacts) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script,
                      "agent_budget": 8}),
        host,
    }
}

/// The note ids among an output's `writes`, wherever the output keeps them.
fn notes(out: &Value) -> Vec<String> {
    let fx = out.get("effects").unwrap_or(out);
    (fx["writes"].as_array().into_iter().flatten())
        .filter(|w| w["collection"] == "WfNote")
        .map(|w| w["fields"]["note_id"].as_str().unwrap().to_string())
        .collect()
}

/// A step 0 by a direct call: the script's `complete` value, or the whole output.
fn run0(script: &str) -> Value {
    let out = invoke(json!({
        "protocol": 1, "argv": ["run", "inline"], "script": script,
        "caller": {"session_id": "cli", "tool_call_id": "t1", "cwd": "/work"},
        "job": {"row_doc_id": "r1", "chain_root_doc_id": "r1", "chain": ["r1"], "handle": ""}
    }));
    // A result that carries scratch is pinned, so it names its run beside the value.
    match out.get("result") {
        Some(Value::String(s)) => json!(s.split(" (run ").next().unwrap()),
        Some(wrapped) if wrapped.get("run").is_some() => wrapped["answer"].clone(),
        Some(result) => result.clone(),
        None => out,
    }
}

const ROW18: &str = r##"let meta = #{ name: "row18", description: "scratch then complete after one agent" };
let r = agent("work");
let path = write_scratch_file("report.md", "# done\n" + r.output);
complete(#{ path: path, text: read_scratch_file("report.md") });
"##;

#[test]
fn row18_scratch_and_complete_in_one_terminal_output() {
    let cwd = temp_cwd("row18");
    let (mut host, call) = (FakeHost::default(), call(ROW18, facts(&cwd)));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert!(s0.output["effects"]["scratch"].is_null(), "{}", s0.output);
    host.settle(&spawns(&s0.output), canned);

    let s1 = host.step(&call, &row, invoke).unwrap();
    let out = &s1.output;
    let path = format!("{cwd}/.gents/workflows/runs/t1/report.md");
    assert!(!s1.continues && s1.code.is_none(), "{out}");
    assert_eq!(
        out["result"],
        json!({"path": path, "text": "# done\nok"}),
        "{out}"
    );
    assert_eq!(
        out["scratch"],
        json!([{"name": "report.md", "content": "# done\nok"}])
    );
    assert!(
        notes(out).contains(&"scratch:report.md".to_string()),
        "{out}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "# done\nok");
}

#[test]
fn scratch_paths_follow_tool_call_id_and_survive_handle_suffix() {
    let script = r##"let meta = #{ name: "paths", description: "writes one file" };
complete(write_scratch_file("a.txt", "one"));
"##;
    // The formula, and a host handle suffix that changes no path.
    let cwd = temp_cwd("paths");
    let mut host = FakeHost::default();
    let mut result = |tool_call: &str| {
        let mut f = facts(&cwd);
        f.caller["tool_call_id"] = json!(tool_call);
        let s0 = host.step0(&call(script, f), invoke).unwrap();
        let path = s0.output["result"].as_str().unwrap().split(" (run ").next();
        (s0.row.clone().unwrap(), json!(path.unwrap()))
    };
    let ((row1, first), (row2, second)) = (result("t1"), result("t2"));
    assert_eq!(
        first,
        json!(format!("{cwd}/.gents/workflows/runs/t1/a.txt"))
    );
    assert_eq!(
        second,
        json!(format!("{cwd}/.gents/workflows/runs/t2/a.txt"))
    );
    assert_eq!(host.state.rows[&row1].handle, "paths");
    assert_eq!(
        host.state.rows[&row2].handle, "paths-2",
        "the host suffixed it"
    );

    // The host's own path for a name is reused.
    let mut f = facts(&cwd);
    f.scratch = json!({"a.txt": "/host/a.txt"});
    let s0 = FakeHost::default().step0(&call(script, f), invoke).unwrap();
    assert_eq!(
        s0.output["result"],
        "/host/a.txt (run cli:t1, handle paths)"
    );

    // A resumed row: the earlier row's write replays from its note, a new file goes under
    // this row's own tool_call_id.
    let earlier = "/work/.gents/workflows/runs/t1/a.txt";
    let note = json!({"sha256": "x", "bytes": 3, "content": "one", "path": earlier});
    let source = r##"let meta = #{ name: "paths", description: "writes two files" };
let a = write_scratch_file("a.txt", "one");
let b = write_scratch_file("b.txt", "two");
complete(#{ a: a, b: b });
"##;
    let out = invoke(json!({
        "protocol": 1, "argv": ["resume"],
        "caller": {"session_id": "cli", "tool_call_id": "t2", "cwd": "/work"},
        "job": {"row_doc_id": "r2", "chain_root_doc_id": "r1", "chain": ["r1", "r2"], "handle": "paths"},
        "pin": {"script_id": "s", "source_digest": format!("sha256:{:x}", Sha256::digest(source.as_bytes())),
                "agent_budget": 8, "handle": "paths"},
        "verified_script": {"source": source},
        "state": {"rounds": 1, "notes_done": true, "notes": [
            {"_docID": "d1", "note_id": "scratch:a.txt", "row": "r1", "value_json": note.to_string()}]}
    }));
    assert_eq!(
        out["result"],
        json!({"a": earlier, "b": "/work/.gents/workflows/runs/t2/b.txt"}),
        "{out}"
    );
    assert_eq!(
        out["scratch"],
        json!([{"name": "b.txt", "content": "two"}]),
        "{out}"
    );
}

#[test]
fn scratch_before_first_agent_is_pinned_at_step0() {
    let ends = r##"let meta = #{ name: "early", description: "scratch, then complete" };
complete(write_scratch_file("a.txt", "one"));
"##;
    let cwd = temp_cwd("early");
    let mut host = FakeHost::default();
    let s0 = host.step0(&call(ends, facts(&cwd)), invoke).unwrap();
    let out = &s0.output;
    assert_eq!(out["pin"]["handle"], "early", "{out}");
    let text = out["result"].as_str().unwrap_or_default();
    assert!(
        text.starts_with(&cwd) && text.contains(" (run ") && text.ends_with(", handle early)"),
        "the inline result names the run and its handle: {out}"
    );
    assert!(
        s0.row.is_some() && !s0.continues,
        "adopted and delivered inline: {out}"
    );
    assert!(notes(out).iter().any(|n| n == "scratch:a.txt"), "{out}");
    assert!(notes(out).iter().any(|n| n.starts_with("run:")), "{out}");

    let spawns_next = r##"let meta = #{ name: "early", description: "scratch, then an agent" };
write_scratch_file("a.txt", "one");
agent("work");
"##;
    let s0 = FakeHost::default()
        .step0(&call(spawns_next, facts(&cwd)), invoke)
        .unwrap();
    let fx = &s0.output["effects"];
    assert!(fx["pin"].is_object() && s0.continues, "{}", s0.output);
    assert_eq!(fx["scratch"], json!([{"name": "a.txt", "content": "one"}]));
}

#[test]
fn helpers_are_deterministic_and_refusals_are_catchable() {
    let script = r##"let meta = #{ name: "pure", description: "pure helpers" };
let caught = [];
try { git_diff_since("HEAD~1"); } catch (e) { caught.push(e); }
try { render_template("nope", #{}); } catch (e) { caught.push(e); }
complete(#{
  fp: [fingerprint("abc"), fingerprint("abc")],
  json: [json_encode(#{ b: 1, a: [1, 2], c: "x" }), json_encode(#{ c: "x", a: [1, 2], b: 1 })],
  caught: caught
});
"##;
    let (a, b) = (run0(script), run0(script));
    assert_eq!(a, b, "deterministic across runs");
    assert_eq!(a["fp"], json!(["ba7816bf8f01cfea", "ba7816bf8f01cfea"]));
    let compact = r#"{"a":[1,2],"b":1,"c":"x"}"#;
    assert_eq!(a["json"], json!([compact, compact]));
    let caught = (a["caught"].as_array().unwrap().iter()).map(|c| c.as_str().unwrap());
    let caught: Vec<&str> = caught.collect();
    assert!(
        caught[0]
            .starts_with("workflow_not_available: git_diff_since is not supported. Next: agent("),
        "{}",
        caught[0]
    );
    assert_eq!(
        caught[1],
        "workflow_invalid_input: unknown template nope; known: none. Next: workflow help host"
    );

    // A bundled template is plain {{key}} replacement.
    let registry = [("hi", "hello {{who}}, {{who}}!")];
    let map = rhai::Map::from_iter([("who".into(), "you".into())]);
    assert_eq!(render(&registry, "hi", &map).unwrap(), "hello you, you!");
}

#[test]
fn caps_and_names_are_catchable_refusals() {
    let script = format!(
        r##"let meta = #{{ name: "caps", description: "breaches every cap" }};
let caught = [];
let long = "x";
for i in 0..6 {{ long += long; }}
for name in ["", "A", "a/b", "..", long + "x"] {{
  try {{ write_scratch_file(name, "x"); }} catch (e) {{ caught.push(e); }}
}}
let s = "xxxxxxxxxxxxxxxx";
for i in 0..16 {{ s += s; }}
write_scratch_file("exact", s);
try {{ write_scratch_file("big", s + "x"); }} catch (e) {{ caught.push(e); }}
for i in 0..{MAX_FILES} {{
  try {{ write_scratch_file("f" + i, "x"); }} catch (e) {{ caught.push(e); }}
}}
write_scratch_file("f0", "x");
try {{ read_scratch_file("never"); }} catch (e) {{ caught.push(e); }}
complete(caught);
"##
    );
    let out = run0(&script);
    let caught: Vec<&str> = (out.as_array().unwrap_or_else(|| panic!("{out}")).iter())
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(
        caught
            .iter()
            .all(|c| c.starts_with("workflow_invalid_input: ")),
        "{caught:?}"
    );
    // Five bad names, one oversize file, and the 33rd name ("exact" took one of the 32).
    assert_eq!(caught.len(), 5 + 1 + 1 + 1, "{caught:?}");
    assert!(caught[5].contains(&MAX_BYTES.to_string()), "{}", caught[5]);
    assert!(caught[6].contains(&MAX_FILES.to_string()), "{}", caught[6]);
    assert!(caught.iter().all(|c| c.contains(". Next: ")), "{caught:?}");
}

#[test]
fn rewriting_a_name_with_other_content_is_a_catchable_refusal() {
    let out = run0(
        r##"let meta = #{ name: "again", description: "writes one name twice" };
write_scratch_file("report.md", "draft");
write_scratch_file("report.md", "draft");
let caught = "";
try { write_scratch_file("report.md", "final"); } catch (e) { caught = e; }
complete(#{ caught: caught, text: read_scratch_file("report.md") });
"##,
    );
    assert_eq!(
        out,
        json!({"caught": "workflow_invalid_input: scratch file report.md was already written in this run with other content. Next: write_scratch_file(<a new name>, content)",
               "text": "draft"}),
        "{out}"
    );
}

#[test]
fn a_resumed_row_is_not_capped_by_the_earlier_rows_replayed_files() {
    let source = format!(
        r##"let meta = #{{ name: "cap", description: "replays the cap, then adds one" }};
for i in 0..{MAX_FILES} {{ write_scratch_file("f" + i, "x"); }}
complete(write_scratch_file("extra", "x"));
"##
    );
    let notes: Vec<Value> = (0..MAX_FILES)
        .map(|i| {
            let note = json!({"sha256": "x", "bytes": 1, "content": "x",
                              "path": format!("/work/.gents/workflows/runs/t1/f{i}")});
            json!({"_docID": format!("d{i}"), "note_id": format!("scratch:f{i}"), "row": "r1",
                   "value_json": note.to_string()})
        })
        .collect();
    let out = invoke(json!({
        "protocol": 1, "argv": ["resume"],
        "caller": {"session_id": "cli", "tool_call_id": "t2", "cwd": "/work"},
        "job": {"row_doc_id": "r2", "chain_root_doc_id": "r1", "chain": ["r1", "r2"], "handle": "cap"},
        "pin": {"script_id": "s", "source_digest": format!("sha256:{:x}", Sha256::digest(source.as_bytes())),
                "agent_budget": 8, "handle": "cap"},
        "verified_script": {"source": source},
        "state": {"rounds": 1, "notes_done": true, "notes": notes}
    }));
    assert_eq!(
        out["result"], "/work/.gents/workflows/runs/t2/extra",
        "{out}"
    );
}
