//! The corpus differential (L2 section 7.1 D-test). Every case runs under rhai's `Simple` and
//! `None` optimizer levels with the canned oracle (the journal oracle when `<case>.jsonl` sits
//! beside it), and the two transcripts must agree. The cases are the gents-written suite in
//! `tests/dialect/`, plus, only when `$GROK_CORPUS_DIR` is set, scripts read from there and
//! never committed (U15). With `$WF_DIFF_OUT` set the transcripts are written to a file named
//! by the rhai version, so CI can diff one pin against the other.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use workflow::fakehost::cli::{facts, render};
use workflow::fakehost::faults::deterministic;
use workflow::fakehost::oracle::{Answer, Journal, canned, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::{errors, invoke, sandbox};

const MAX_STEPS: usize = 64;
static RUNS: AtomicUsize = AtomicUsize::new(0);
struct Case {
    name: String,
    script: String,
    /// `<case>.args.json`, when it sits beside the case.
    args: Option<Value>,
    /// `<case>.jsonl`, when it sits beside the case.
    journal: Option<String>,
}

/// The cases in `dir`, by name: a `.rhai` file with its optional `.args.json` and `.jsonl`.
fn cases_in(dir: &Path, prefix: &str) -> Vec<Case> {
    let mut paths: Vec<PathBuf> = (fs::read_dir(dir).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
        .collect();
    paths.sort();
    let read = |p: PathBuf| fs::read_to_string(p).ok();
    paths
        .into_iter()
        .map(|p| Case {
            name: format!("{prefix}{}", p.file_stem().unwrap().to_string_lossy()),
            script: fs::read_to_string(&p).unwrap(),
            args: read(p.with_extension("args.json")).map(|t| serde_json::from_str(&t).unwrap()),
            journal: read(p.with_extension("jsonl")),
        })
        .collect()
}

fn dialect() -> Vec<Case> {
    cases_in(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dialect"),
        "",
    )
}

/// The `*.rhai` scripts found in `dir` at run time, none when `dir` is unset or missing.
fn corpus(dir: Option<&str>) -> Vec<Case> {
    match dir.map(Path::new).filter(|dir| dir.is_dir()) {
        Some(dir) => cases_in(dir, "corpus/"),
        None => Vec::new(),
    }
}

/// The effects, progress frames, notes and result of one run on `host`, crashing it before each
/// step when `crash` is set. A refusal is part of the transcript: a corpus script may refuse
/// (some are Grok-only). A row the host ended (stalled, denied) fails the case.
fn run_case(case: &Case, host: &mut FakeHost, crash: bool) -> String {
    let tag = case.name.replace('/', "-");
    // Tests run in parallel, so each run gets a directory of its own.
    let n = RUNS.fetch_add(1, Ordering::Relaxed);
    let cwd = std::env::temp_dir().join(format!("wf-diff-{}-{n}-{tag}", std::process::id()));
    let cwd = cwd.to_string_lossy().to_string();
    let _ = fs::remove_dir_all(&cwd);
    fs::create_dir_all(&cwd).unwrap();
    let mut input = json!({"protocol": 1, "argv": ["run", "inline"], "script": case.script});
    if let Some(args) = &case.args {
        input["args"] = args.clone();
    }
    let call = Call {
        kind: Kind::JobStep,
        input,
        host: facts(&cwd),
    };
    let mut journal = (case.journal.as_deref()).map(|j| Journal::from_jsonl(j).unwrap());
    let mut oracle = |s: &Value| match journal.as_mut() {
        Some(j) => j.answer(s),
        None => canned(s),
    };

    let mut step = host.step0(&call, invoke).unwrap();
    let (mut outs, mut asked) = (vec![step.output.clone()], spawns(&step.output));
    while let Some(row) = step.row.clone().filter(|_| step.continues) {
        assert!(outs.len() < MAX_STEPS, "{}: the run did not end", case.name);
        host.settle(&asked, &mut oracle);
        for message in messages(&step.output) {
            deliver(host, &message, &mut oracle);
        }
        if crash {
            host.crash();
        }
        step = host.step(&call, &row, invoke).unwrap();
        asked.extend(spawns(&step.output));
        outs.push(step.output.clone());
    }
    assert!(
        step.code.is_none(),
        "{}: the host ended the row: {:?}",
        case.name,
        step.code
    );
    let _ = fs::remove_dir_all(&cwd);
    // A refusal is recorded by its code: its message carries rhai's own wording, which a
    // version bump may change.
    for out in &mut outs {
        if let Some(code) = out.pointer("/fail/code").cloned() {
            out["fail"] = json!({ "code": code });
        }
    }
    render(&outs, true).replace(&cwd, "<cwd>")
}

fn transcript(case: &Case, unoptimized: bool) -> String {
    sandbox::set_unoptimized(unoptimized);
    let text = run_case(case, &mut FakeHost::default(), false);
    sandbox::set_unoptimized(false);
    text
}

/// The `message` effects of one output: corrections and `resume_from` texts.
fn messages(out: &Value) -> Vec<Value> {
    let sent = out.pointer("/effects/message").and_then(Value::as_array);
    sent.cloned().unwrap_or_default()
}

/// What the host does with a `message`: the slot's child runs again (a new request) and ends
/// as the oracle says.
fn deliver(host: &mut FakeHost, message: &Value, oracle: &mut impl FnMut(&Value) -> Answer) {
    let slot = message["slot"].as_str().unwrap().to_string();
    let child = host.state.children.get_mut(&(slot.clone(), 0)).unwrap();
    child.request_id = format!("{}+", child.request_id);
    host.answer_child(&slot, 0, oracle(message)).unwrap();
}

/// Runs every case at both levels, asserts the levels agree, and returns the transcripts.
fn run_all(cases: &[Case]) -> Vec<(String, String)> {
    let mut all = Vec::new();
    for case in cases {
        let (simple, none) = (transcript(case, false), transcript(case, true));
        assert_eq!(simple, none, "{}: Simple and None differ", case.name);
        all.push((case.name.clone(), simple));
    }
    all
}

/// The rhai version this build resolved, from the lockfile.
fn rhai_version() -> String {
    let lock = include_str!("../Cargo.lock");
    let entry = lock
        .split("[[package]]")
        .find(|p| p.contains("name = \"rhai\"\n"));
    let line = (entry.unwrap().lines()).find(|l| l.starts_with("version = "));
    line.unwrap()
        .trim_start_matches("version = ")
        .trim_matches('"')
        .into()
}

/// Writes `rhai-<version>.txt` under `dir`, one section per case; returns its path.
fn write_transcripts(dir: &Path, all: &[(String, String)]) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("rhai-{}.txt", rhai_version()));
    let body: Vec<String> = (all.iter())
        .map(|(name, text)| format!("== {name} ==\n{text}\n"))
        .collect();
    fs::write(&path, body.concat()).unwrap();
    path
}

#[test]
fn dialect_cases_agree_at_simple_and_none() {
    let mut cases = dialect();
    assert_eq!(cases.len(), 40, "the dialect suite holds 40 cases");
    cases.extend(corpus(std::env::var("GROK_CORPUS_DIR").ok().as_deref()));
    let all = run_all(&cases);
    assert!(all.iter().all(|(_, text)| !text.is_empty()));
    if let Ok(out) = std::env::var("WF_DIFF_OUT") {
        write_transcripts(Path::new(&out), &all);
    }
}

/// L2 section 7.1: each case runs twice, and three times with crashes between steps, to one trace.
#[test]
fn dialect_cases_replay_identically_across_crashes() {
    for case in dialect() {
        let trace = deterministic(|host, crash| vec![json!(run_case(&case, host, crash))]);
        trace.unwrap_or_else(|e| panic!("{}: {e}", case.name));
    }
}

#[test]
fn transcripts_written_per_rhai_version() {
    let dir = std::env::temp_dir().join(format!("wf-diff-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let cases = dialect();
    let all = run_all(&cases[..2]);
    let path = write_transcripts(&dir, &all);
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(name, format!("rhai-{}.txt", rhai_version()));
    assert!(rhai_version().starts_with("1."), "{name}");
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        all.iter()
            .all(|(n, t)| text.contains(&format!("== {n} ==\n{t}")))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn corpus_runs_only_with_grok_corpus_dir() {
    assert!(corpus(None).is_empty());
    // A directory holding one script (gents-written here) yields exactly that case.
    let dir = std::env::temp_dir().join(format!("wf-diff-corpus-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let script = "let meta = #{ name: \"stand-in\", description: \"a corpus stand-in\" };\ncomplete(1 + 1);\n";
    fs::write(dir.join("stand-in.rhai"), script).unwrap();
    let found = corpus(Some(&dir.to_string_lossy()));
    let names: Vec<_> = found.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["corpus/stand-in"]);
    let all = run_all(&found);
    assert!(all[0].1.contains("result"), "{}", all[0].1);
    let _ = fs::remove_dir_all(&dir);
}

/// The refusal cases and the code each records: the script is refused before it runs.
const REFUSALS: [(&str, &str); 2] = [
    ("deep_nesting", errors::WORKFLOW_VALIDATION_FAILED),
    ("shadowing_refusal", errors::WORKFLOW_VALIDATION_FAILED),
];

#[test]
fn refusal_cases_record_codes_not_text() {
    let all = run_all(&dialect());
    let transcript_of = |name: &str| (all.iter().find(|(n, _)| n == name)).map(|(_, t)| t.as_str());
    for (name, expected) in REFUSALS {
        let text = transcript_of(name).unwrap_or_else(|| panic!("no dialect case {name}"));
        let fails: Vec<_> = text.lines().filter(|l| l.starts_with("fail ")).collect();
        assert_eq!(fails.len(), 1, "{name}: {text}");
        let code = fails[0].strip_prefix("fail ").unwrap();
        let record: Value = serde_json::from_str(code).unwrap();
        assert_eq!(record["code"], expected, "{name}: {text}");
        assert_eq!(record.as_object().unwrap().len(), 1, "{name}: {text}");
    }
    // An error a script catches is recorded by its code too: the script keeps the part before ':'.
    let stubs = transcript_of("stubs_throw_codes").expect("the stubs case exists");
    let codes = [errors::WORKFLOW_CAPABILITY_UNAVAILABLE; 3];
    let line = format!("result {}", json!({ "codes": codes }));
    assert!(stubs.lines().any(|l| l == line), "{stubs}");
}
