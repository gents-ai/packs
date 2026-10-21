use super::*;

fn input() -> Value {
    json!({
        "context": {"host": "spark-1", "load1": 0.4, "disk_used_pct": 97},
        "questions": {
            "wake": {"type": "noul", "instructions": "Does a person or agent need to act on this host now?"},
            "area": {"type": "choice", "instructions": "Which area needs attention?",
                     "criteria": {"disk": "Storage", "gpu": null}},
            "urgency": {"type": "score", "instructions": "How urgent is it?",
                        "criteria": ["Routine", "Soon", "Now"]}
        },
        "http_calls": true
    })
}

fn with_results(result: Value) -> Value {
    let mut i = input();
    i["http_results"] = json!({ "decision": result });
    i
}

fn ok_body() -> String {
    json!({"model": "vllm-sr/d3", "answers": {
        "wake": {"type": "noul", "noul": 0.97},
        "area": {"type": "choice", "choice": "disk", "probabilities": {"disk": 0.99, "gpu": 0.01}},
        "urgency": {"type": "score", "score": 1.8, "probabilities": {"0": 0.05, "1": 0.1, "2": 0.85}}
    }, "usage": {"input_tokens": 120, "output_tokens": 0}})
    .to_string()
}

fn failure(i: &Value) -> Failure {
    run(i).expect_err("expected a failure")
}

#[test]
fn asks_spark_1_by_default_and_maps_context_to_state() {
    let out = run(&input()).unwrap();
    let request = &out["http_calls"]["requests"][0];
    assert_eq!(out["http_calls"]["requests"].as_array().unwrap().len(), 1);
    assert_eq!(request["method"], "POST");
    assert_eq!(request["url"], "http://100.74.68.88:8000/v1/systemone");
    assert!(request["headers"].get("authorization").is_none());
    let body: Value = serde_json::from_str(request["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["state"], input()["context"]);
    assert_eq!(body["questions"], input()["questions"]);
}

#[test]
fn spark_2_is_reached_only_when_named() {
    let mut i = input();
    i["server"] = json!("spark-2");
    let out = run(&i).unwrap();
    assert_eq!(
        out["http_calls"]["requests"][0]["url"],
        "http://100.86.62.91:8100/v1/systemone"
    );
}

#[test]
fn server_names_match_the_grant() {
    let manifest: Value = serde_json::from_str(include_str!("../../../manifest.json")).unwrap();
    let plugin = &manifest["plugins"][0];
    let grant: Vec<&str> = plugin["manifold"]["net"]["OutboundHttp"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let names: Vec<&str> = plugin["input_schema"]["properties"]["server"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(grant, SERVERS.map(|(_, origin)| origin));
    assert_eq!(names, SERVERS.map(|(name, _)| name));
}

#[test]
fn refuses_unknown_servers_and_urls() {
    for server in [json!("other"), json!("http://100.74.68.88:8000"), json!(1)] {
        let mut i = input();
        i["server"] = server;
        assert_eq!(failure(&i).error, "server must be spark-1 or spark-2");
    }
}

#[test]
fn validates_questions() {
    let cases = [
        (json!({}), "questions must be an object"),
        (json!({"q": {"type": "noul"}}), "nonempty instructions"),
        (
            json!({"q": {"type": "noul", "instructions": "x", "criteria": ["a", "b"]}}),
            "takes no criteria",
        ),
        (
            json!({"q": {"type": "choice", "instructions": "x", "criteria": {"a": null}}}),
            "choice criteria",
        ),
        (
            json!({"q": {"type": "score", "instructions": "x", "criteria": {"a": null, "b": null}}}),
            "score criteria",
        ),
        (
            json!({"q": {"type": "rank", "instructions": "x"}}),
            "type must be",
        ),
        (
            json!({"q": {"type": "noul", "instruction": "x"}}),
            "\"instruction\" is not known",
        ),
    ];
    for (questions, want) in cases {
        let mut i = input();
        i["questions"] = questions;
        let f = failure(&i);
        assert!(
            f.error.contains(want),
            "{} does not contain {want}",
            f.error
        );
        assert!(!f.retryable);
    }
    let mut i = input();
    i["questions"] = Value::Object(
        (0..17)
            .map(|n| {
                (
                    format!("q{n}"),
                    json!({"type": "noul", "instructions": "x"}),
                )
            })
            .collect(),
    );
    assert!(failure(&i).error.contains("1 to 16"));
}

#[test]
fn refuses_unknown_fields_and_missing_context() {
    let mut i = input();
    i["question"] = json!({});
    assert!(failure(&i).error.contains("\"question\" is not known"));
    let mut i = input();
    i["context"] = Value::Null;
    assert_eq!(failure(&i).error, "context is required");
}

#[test]
fn needs_the_host_http_capability() {
    let mut i = input();
    i.as_object_mut().unwrap().remove("http_calls");
    assert!(failure(&i).error.contains("no HTTP calls"));
}

#[test]
fn caps_the_request_body() {
    let mut i = input();
    i["context"] = json!("x".repeat(MAX_WORK_BYTES));
    let f = failure(&i);
    assert!(!f.retryable);
    assert!(f.error.contains("bytes of work"), "{}", f.error);
}

#[test]
fn bounds_the_work_of_context_times_questions() {
    // Three questions over 3 KiB of context fit; over 4 KiB they do not,
    // though one question over 4 KiB still does.
    let mut i = input();
    i["context"] = json!("x".repeat(3 * 1024));
    assert!(run(&i).unwrap()["http_calls"].is_object());
    i["context"] = json!("x".repeat(4 * 1024));
    let f = failure(&i);
    assert!(!f.retryable);
    assert!(f.error.contains("3 questions"), "{}", f.error);
    assert!(f.error.contains("ask fewer questions"), "{}", f.error);
    let questions = i["questions"].as_object_mut().unwrap();
    questions.retain(|name, _| name == "wake");
    assert!(run(&i).unwrap()["http_calls"].is_object());
}

#[test]
fn success_preserves_probabilities() {
    let out = run(&with_results(json!({"status": 200, "body": ok_body()}))).unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["server"], "spark-1");
    assert_eq!(out["model"], "vllm-sr/d3");
    assert_eq!(out["answers"]["wake"]["noul"], 0.97);
    assert_eq!(out["answers"]["area"]["probabilities"]["disk"], 0.99);
    assert_eq!(out["usage"]["input_tokens"], 120);
}

#[test]
fn a_timeout_is_busy_and_retryable_but_not_retried() {
    let f = failure(&with_results(
        json!({"error": "the http request to 100.74.68.88 timed out"}),
    ));
    assert!(f.retryable);
    assert!(f.error.contains("one request at a time"), "{}", f.error);
    assert!(f.error.contains("Not retried"));
}

#[test]
fn an_unreachable_server_is_retryable_and_not_redirected() {
    let mut i = with_results(json!({"error": "100.86.62.91 could not be reached"}));
    i["server"] = json!("spark-2");
    let f = failure(&i);
    assert!(f.retryable);
    assert!(f.error.starts_with("d3 on spark-2 could not be reached"));
    assert!(f.error.contains("not sent elsewhere"));
}

#[test]
fn a_dropped_connection_is_retryable() {
    for error in [
        "the http request to 100.74.68.88 failed",
        "the http response was cut off",
    ] {
        let f = failure(&with_results(json!({"error": error})));
        assert!(f.retryable, "{error}");
        assert!(f.error.contains("dropped the connection"), "{}", f.error);
        assert!(f.error.contains("Not retried"));
    }
}

#[test]
fn a_grant_refusal_is_not_retryable() {
    let f = failure(&with_results(
        json!({"error": "network access to 100.74.68.88 is not granted"}),
    ));
    assert!(!f.retryable);
    assert!(f.error.contains("not granted"));
}

#[test]
fn http_refusals_say_what_happened() {
    let cases = [
        (401, "", "requires a bearer token", false),
        (
            422,
            r#"{"detail":{"invalid_questions":{"q":"unsupported question type"}}}"#,
            "unsupported question type",
            false,
        ),
        (429, "", "is busy", true),
        (503, "", "is busy", true),
        (500, "boom", "failed (HTTP 500): boom", true),
        (404, "", "answered HTTP 404", false),
    ];
    for (status, body, want, retryable) in cases {
        let f = failure(&with_results(json!({"status": status, "body": body})));
        assert!(
            f.error.contains(want),
            "{} does not contain {want}",
            f.error
        );
        assert_eq!(f.retryable, retryable, "{status}");
    }
}

#[test]
fn requires_every_answer_in_its_asked_shape() {
    let f = failure(&with_results(
        json!({"status": 200, "body": r#"{"answers":{"wake":{"type":"noul","noul":0.5}}}"#}),
    ));
    assert!(f.error.contains("did not answer question"), "{}", f.error);
    let mut body: Value = serde_json::from_str(&ok_body()).unwrap();
    body["answers"]["wake"] = json!({"type": "choice", "choice": "yes"});
    let f = failure(&with_results(
        json!({"status": 200, "body": body.to_string()}),
    ));
    assert!(f.error.contains("other than a noul answer"), "{}", f.error);
    let f = failure(&with_results(json!({"status": 200, "body": "<html>"})));
    assert!(f.error.contains("not JSON"));
}

#[test]
fn long_refusal_bodies_are_cut() {
    assert_eq!(detail("a\n b"), "a b");
    assert!(detail(&"é".repeat(1000)).ends_with("..."));
}
