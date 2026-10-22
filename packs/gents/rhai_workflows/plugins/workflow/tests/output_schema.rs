//! `output_schema` (L2 section 4.9) and `resume_from` (section 4.7), on the fake host. The
//! tests assert behaviour and codes, never wording (U15).
use serde_json::{Value, json};
use workflow::fakehost::adopt::Stepped;
use workflow::fakehost::cli::facts;
use workflow::fakehost::oracle::{Answer, spawns};
use workflow::fakehost::{Call, FakeHost, Kind};
use workflow::invoke;

fn call(script: &str) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script,
                      "agent_budget": 8}),
        host: facts("/work"),
    }
}

const SCHEMA_RUN: &str = r##"let meta = #{ name: "schema", description: "an agent with an output schema" };
let r = agent("report", #{ output_schema: #{ type: "object", required: ["n"],
                                             properties: #{ n: #{ type: "integer" } } } });
complete(#{ output: r.output, success: r.success, spent: budget().spent });
"##;

/// The `message` effects of one output.
fn messages(out: &Value) -> Vec<Value> {
    let m = out.pointer("/effects/message").and_then(Value::as_array);
    m.cloned().unwrap_or_default()
}

/// The note ids among an output's `writes`.
fn notes(out: &Value) -> Vec<String> {
    let fx = out.get("effects").unwrap_or(out);
    (fx["writes"].as_array().into_iter().flatten())
        .filter(|w| w["collection"] == "WfNote")
        .map(|w| w["fields"]["note_id"].as_str().unwrap().to_string())
        .collect()
}

/// What the host does with a `message`: the slot's child runs again, then ends with `reply`
/// (a new request, as the children view shows only the latest).
fn deliver(host: &mut FakeHost, message: &Value, reply: &str) {
    let slot = message["slot"].as_str().unwrap().to_string();
    let child = host.state.children.get_mut(&(slot.clone(), 0)).unwrap();
    child.request_id = format!("{}+", child.request_id);
    host.answer_child(&slot, 0, Answer::ok(reply)).unwrap();
}

/// Steps the run once and checks the host kept the row going or ended it cleanly.
fn step(host: &mut FakeHost, call: &Call, row: &str) -> Stepped {
    let s = host.step(call, row, invoke).unwrap();
    assert!(s.code.is_none(), "{}", s.output);
    s
}

#[test]
fn row4_prose_then_valid_fence_is_one_uncharged_message() {
    let (mut host, call) = (FakeHost::default(), call(SCHEMA_RUN));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let asked = spawns(&s0.output);
    assert_eq!(asked.len(), 1, "{}", s0.output);
    let prompt = asked[0]["prompt"].as_str().unwrap();
    assert!(
        prompt.starts_with("report") && prompt.contains("<output-contract>"),
        "{prompt}"
    );
    assert!(
        prompt.contains("\"required\""),
        "the schema is embedded: {prompt}"
    );

    host.settle(&asked, |_| {
        Answer::ok("I looked around and found nothing to list.")
    });
    let s1 = step(&mut host, &call, &row);
    let sent = messages(&s1.output);
    assert_eq!(sent.len(), 1, "{}", s1.output);
    assert_eq!(sent[0]["slot"], asked[0]["slot"]);
    assert!(!sent[0]["text"].as_str().unwrap().is_empty());
    let corr = format!("corr:{}#1", asked[0]["slot"].as_str().unwrap());
    assert!(notes(&s1.output).contains(&corr), "{}", s1.output);
    assert!(s1.continues, "{}", s1.output);

    deliver(
        &mut host,
        &sent[0],
        "Here it is.\n```json\n{\"n\": 3}\n```\n",
    );
    let s2 = step(&mut host, &call, &row);
    assert!(messages(&s2.output).is_empty(), "{}", s2.output);
    assert_eq!(
        s2.output["result"],
        json!({"output": {"n": 3}, "success": true, "spent": 1})
    );
    assert_eq!(host.state.children.len(), 1, "a correction spawns nothing");
}

#[test]
fn three_invalid_replies_end_unsuccessful_after_two_corrections() {
    let (mut host, call) = (FakeHost::default(), call(SCHEMA_RUN));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), |_| Answer::ok("no fence at all"));

    let s1 = step(&mut host, &call, &row);
    let first = messages(&s1.output);
    assert_eq!(first.len(), 1, "{}", s1.output);
    deliver(&mut host, &first[0], "```json\n{\"n\": \"three\"}\n```");

    let s2 = step(&mut host, &call, &row);
    let second = messages(&s2.output);
    assert_eq!(second.len(), 1, "{}", s2.output);
    assert!(
        notes(&s2.output).iter().any(|n| n.ends_with("#2")),
        "{}",
        s2.output
    );
    deliver(&mut host, &second[0], "```json\n[1, 2]\n```");

    let s3 = step(&mut host, &call, &row);
    assert!(messages(&s3.output).is_empty(), "{}", s3.output);
    let result = &s3.output["result"];
    assert_eq!(result["success"], false, "{}", s3.output);
    assert!(
        result["output"].as_str().is_some_and(|e| !e.is_empty()),
        "the output names the failure: {}",
        s3.output
    );
    assert_eq!(result["spent"], 1);
}

#[test]
fn resume_from_writes_out_note_before_its_message() {
    let script = r##"let meta = #{ name: "again", description: "asks the same agent twice" };
let first = agent("one");
let second = agent("two", #{ resume_from: first.agent_id });
complete(#{ first: first.output, second: second.output, spent: budget().spent });
"##;
    let (mut host, call) = (FakeHost::default(), call(script));
    let s0 = host.step0(&call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    let asked = spawns(&s0.output);
    let slot = asked[0]["slot"].as_str().unwrap().to_string();
    host.settle(&asked, |_| Answer::ok("alpha"));

    // The out: note rides in the same output as the message, so the host stores it first.
    let s1 = step(&mut host, &call, &row);
    let sent = messages(&s1.output);
    assert_eq!(
        sent,
        vec![json!({"slot": slot, "text": "two"})],
        "{}",
        s1.output
    );
    assert!(
        notes(&s1.output).contains(&format!("out:{slot}")),
        "{}",
        s1.output
    );
    let stored = &host.state.collections["WfNote"];
    let out = stored
        .iter()
        .find(|n| n["note_id"] == format!("out:{slot}").as_str());
    let value: Value = serde_json::from_str(
        out.expect("the out: note is stored")["value_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (&value["value"], &value["success"]),
        (&json!("alpha"), &json!(true))
    );

    // While the session works on the message, the run waits and sends nothing again.
    let child = host.state.children.get_mut(&(slot.clone(), 0)).unwrap();
    child.state = "running".into();
    let waiting = step(&mut host, &call, &row);
    assert!(
        waiting.continues && messages(&waiting.output).is_empty(),
        "{}",
        waiting.output
    );

    // The original call replays from the note; the new call reads the latest terminal.
    deliver(&mut host, &sent[0], "beta");
    let done = step(&mut host, &call, &row);
    assert_eq!(
        done.output["result"],
        json!({"first": "alpha", "second": "beta", "spent": 1}),
        "{}",
        done.output
    );
}

/// Steps a run of `call` past the settled first reply "alpha".
fn after_first_reply(call: &Call) -> (FakeHost, String, Stepped) {
    let mut host = FakeHost::default();
    let s0 = host.step0(call, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), |_| Answer::ok("alpha"));
    let s1 = step(&mut host, call, &row);
    (host, row, s1)
}

#[test]
fn a_second_resume_of_one_agent_is_refused() {
    let script = r##"let meta = #{ name: "twice", description: "resumes one agent twice" };
let a = agent("one");
let b = agent("two", #{ resume_from: a.agent_id });
try { agent("three", #{ resume_from: a.agent_id }); } catch (e) { complete(e); }
"##;
    let call = call(script);
    let (mut host, row, s1) = after_first_reply(&call);
    let sent = messages(&s1.output);
    assert_eq!(sent.len(), 1, "{}", s1.output);
    deliver(&mut host, &sent[0], "beta");
    let done = step(&mut host, &call, &row);
    assert!(messages(&done.output).is_empty(), "{}", done.output);
    assert!(
        done.output["result"]
            .to_string()
            .contains("workflow_invalid_input"),
        "{}",
        done.output
    );
}

#[test]
fn resume_from_past_the_message_cap_is_refused_with_no_message() {
    let script = r##"let meta = #{ name: "capped", description: "resumes past the cap" };
let a = agent("one");
try { agent("two", #{ resume_from: a.agent_id }); } catch (e) { complete(e); }
"##;
    let mut call = call(script);
    call.host.ceiling["max_messages_per_slot"] = json!(0);
    let (_, _, s1) = after_first_reply(&call);
    assert!(messages(&s1.output).is_empty(), "{}", s1.output);
    assert!(
        s1.output["result"]
            .to_string()
            .contains("workflow_invalid_input"),
        "{}",
        s1.output
    );
}

fn checked(reply: &str) -> Result<Value, String> {
    let schema =
        json!({"type": "object", "required": ["s"], "properties": {"s": {"type": "string"}}});
    workflow::output_schema::check(&workflow::output_schema::compile(&schema).unwrap(), reply)
}

#[test]
fn a_fenced_reply_parses() {
    let v = checked("done\n```json\n{\"s\": \"a\"}\n```\n").unwrap();
    assert_eq!(v["s"], "a");
}

#[test]
fn bare_json_without_a_fence_parses() {
    let v = checked("Here is the result: {\"s\": \"a\"} and that is all.").unwrap();
    assert_eq!(v["s"], "a");
}

#[test]
fn a_raw_tab_in_a_string_parses() {
    let v = checked("```json\n{\"s\": \"M\tsrc/lib.rs\"}\n```").unwrap();
    assert_eq!(v["s"], "M\tsrc/lib.rs");
}

#[test]
fn a_raw_newline_in_a_string_parses() {
    let v = checked("{\"s\": \"one\ntwo\"}").unwrap();
    assert_eq!(v["s"], "one\ntwo");
}

#[test]
fn a_reply_with_no_object_is_refused() {
    assert!(checked("no json here").is_err());
}
