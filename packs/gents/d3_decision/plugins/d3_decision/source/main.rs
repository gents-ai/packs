//! Asks the D3 decision model (`POST /v1/systemone`) typed questions about a
//! text or JSON context and returns its probabilities. The plugin never opens
//! a socket: it hands the host one HTTP request (`http_calls`) and reads the
//! host's answer (`http_results`) on the second round. One request per call,
//! to the one server the caller named; nothing is retried or redirected.
use std::io::{Read, Write};

use serde_json::{Map, Value, json};

/// The servers a caller may name, and the origin each is reached at. These
/// are the manifest's `OutboundHttp` grant: change both together.
const SERVERS: [(&str, &str); 2] = [
    ("spark-1", "http://100.74.68.88:8000"),
    ("spark-2", "http://100.86.62.91:8000"),
];
const DEFAULT_SERVER: &str = "spark-1";
const ROUTE: &str = "/v1/systemone";
const REQUEST_ID: &str = "decision";
const MAX_INPUT_BYTES: usize = 256 * 1024;
/// D3 reads the whole context once per question, at about 1 ms a token on
/// spark-1, and finishes a request the host has given up on. The sum over
/// questions of context bytes plus question bytes stays under this, which
/// D3 answers in about 7 seconds alone; anything larger would time out and
/// keep D3 busy for every other caller after the host stopped waiting.
const MAX_WORK_BYTES: usize = 12 * 1024;
const MAX_QUESTIONS: usize = 16;
const MAX_CRITERIA: usize = 32;
/// Characters of a refusal's body quoted back in the error.
const MAX_DETAIL_CHARS: usize = 400;
/// Top-level fields the host adds to the caller's input.
const HOST_FIELDS: [&str; 3] = ["http_calls", "http_results", "state"];

#[derive(Debug)]
struct Failure {
    error: String,
    /// The same call may succeed later unchanged (busy, timeout, unreachable).
    retryable: bool,
}

fn invalid(error: impl Into<String>) -> Failure {
    Failure {
        error: error.into(),
        retryable: false,
    }
}

fn transient(error: impl Into<String>) -> Failure {
    Failure {
        error: error.into(),
        retryable: true,
    }
}

fn run(input: &Value) -> Result<Value, Failure> {
    let input = input
        .as_object()
        .ok_or_else(|| invalid("input must be a JSON object"))?;
    if let Some(field) = input.keys().find(|k| {
        !["context", "questions", "server"].contains(&k.as_str())
            && !HOST_FIELDS.contains(&k.as_str())
    }) {
        return Err(invalid(format!(
            "the field {field:?} is not known; the fields are context, questions and server"
        )));
    }
    let context = input
        .get("context")
        .filter(|v| !v.is_null())
        .ok_or_else(|| invalid("context is required"))?;
    let questions = questions(input.get("questions"))?;
    let (server, origin) = server(input.get("server"))?;
    if let Some(results) = input.get("http_results") {
        return answer(server, questions, results);
    }
    if input.get("http_calls") != Some(&Value::Bool(true)) {
        return Err(invalid(
            "the host offers this plugin no HTTP calls; install the pack with its network grant approved",
        ));
    }
    let context_bytes = context.to_string().len();
    let work: usize = questions
        .values()
        .map(|q| context_bytes + q.to_string().len())
        .sum();
    if work > MAX_WORK_BYTES {
        let asked = match questions.len() {
            1 => "1 question".to_owned(),
            n => format!("{n} questions"),
        };
        return Err(invalid(format!(
            "d3 reads the context once per question: {asked} over {context_bytes} bytes of context is {work} bytes of work, more than the {MAX_WORK_BYTES} it can answer before the timeout; shorten the context or ask fewer questions"
        )));
    }
    let body = json!({"state": context, "questions": questions}).to_string();
    Ok(json!({"http_calls": {"requests": [{
        "id": REQUEST_ID,
        "method": "POST",
        "url": format!("{origin}{ROUTE}"),
        "headers": {"content-type": "application/json"},
        "body": body,
    }]}}))
}

fn server(value: Option<&Value>) -> Result<(&'static str, &'static str), Failure> {
    let name = match value {
        None | Some(Value::Null) => DEFAULT_SERVER,
        Some(Value::String(name)) => name.as_str(),
        Some(_) => return Err(invalid("server must be spark-1 or spark-2")),
    };
    SERVERS
        .iter()
        .find(|(known, _)| *known == name)
        .copied()
        .ok_or_else(|| invalid("server must be spark-1 or spark-2"))
}

fn questions(value: Option<&Value>) -> Result<&Map<String, Value>, Failure> {
    let questions = value
        .and_then(Value::as_object)
        .filter(|q| (1..=MAX_QUESTIONS).contains(&q.len()))
        .ok_or_else(|| invalid("questions must be an object of 1 to 16 named questions"))?;
    for (name, question) in questions {
        check_question(question).map_err(|why| invalid(format!("question {name:?}: {why}")))?;
    }
    Ok(questions)
}

fn check_question(question: &Value) -> Result<(), String> {
    let question = question.as_object().ok_or("must be an object")?;
    if let Some(field) = question
        .keys()
        .find(|k| !["type", "instructions", "criteria"].contains(&k.as_str()))
    {
        return Err(format!(
            "the field {field:?} is not known; a question has type, instructions and criteria"
        ));
    }
    if question
        .get("instructions")
        .and_then(Value::as_str)
        .is_none_or(|s| s.trim().is_empty())
    {
        return Err("needs nonempty instructions".into());
    }
    let criteria = question.get("criteria");
    let count_ok = |n: usize| (2..=MAX_CRITERIA).contains(&n);
    match question.get("type").and_then(Value::as_str) {
        Some("noul") if criteria.is_none() => Ok(()),
        Some("noul") => Err("a noul question takes no criteria".into()),
        Some("choice") => match criteria.and_then(Value::as_object) {
            Some(c) if count_ok(c.len()) && c.values().all(|v| v.is_null() || v.is_string()) => {
                Ok(())
            }
            _ => Err("choice criteria must be an object of 2 to 32 option names, each mapped to a description or null".into()),
        },
        Some("score") => match criteria.and_then(Value::as_array) {
            Some(c) if count_ok(c.len()) && c.iter().all(Value::is_string) => Ok(()),
            _ => Err("score criteria must be an array of 2 to 32 labels, lowest first".into()),
        },
        _ => Err("type must be choice, noul or score".into()),
    }
}

/// Reads the host's answer to the one request.
fn answer(server: &str, questions: &Map<String, Value>, results: &Value) -> Result<Value, Failure> {
    let response = results
        .get(REQUEST_ID)
        .ok_or_else(|| transient("the host returned no answer for the d3 request"))?;
    if let Some(error) = response.get("error") {
        return Err(host_error(
            server,
            error.as_str().unwrap_or("unknown error"),
        ));
    }
    let status = response.get("status").and_then(Value::as_u64).unwrap_or(0);
    let text = response.get("body").and_then(Value::as_str);
    if status != 200 {
        return Err(refusal(server, status, text.unwrap_or("")));
    }
    let body: Value = text
        .and_then(|t| serde_json::from_str(t).ok())
        .ok_or_else(|| invalid(format!("d3 on {server} returned a body that is not JSON")))?;
    let answers = body
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("d3 on {server} returned no answers object")))?;
    for (name, question) in questions {
        let asked = question["type"].as_str().unwrap_or_default();
        let answered = answers
            .get(name)
            .ok_or_else(|| invalid(format!("d3 on {server} did not answer question {name:?}")))?;
        if !answer_fits(asked, answered) {
            return Err(invalid(format!(
                "d3 on {server} answered question {name:?} with something other than a {asked} answer"
            )));
        }
    }
    Ok(json!({
        "ok": true,
        "server": server,
        "model": body.get("model"),
        "answers": answers,
        "usage": body.get("usage"),
    }))
}

fn answer_fits(asked: &str, answer: &Value) -> bool {
    let number = |key: &str| {
        answer
            .get(key)
            .and_then(Value::as_f64)
            .is_some_and(f64::is_finite)
    };
    answer.get("type").and_then(Value::as_str) == Some(asked)
        && match asked {
            "noul" => number("noul"),
            "choice" => answer.get("choice").is_some_and(Value::is_string),
            "score" => number("score"),
            _ => false,
        }
}

/// A request the host could not complete. D3 serves one request at a time and
/// queues the rest, so a timeout means it is busy or stalled; a server that is
/// off the network times out too, and the host's message does not tell these
/// apart.
fn host_error(server: &str, error: &str) -> Failure {
    if error.contains("timed out") {
        transient(format!(
            "d3 on {server} did not answer in time ({error}); it serves one request at a time, so it may be busy with other callers, stalled or down. Not retried and not sent elsewhere"
        ))
    } else if error.contains("could not be reached") {
        transient(format!(
            "d3 on {server} could not be reached; the server is down or off the network. Not retried and not sent elsewhere"
        ))
    } else if (error.starts_with("the http request to ") && error.ends_with(" failed"))
        || error.contains("response was cut off")
    {
        transient(format!(
            "d3 on {server} dropped the connection before answering ({error}); it may be restarting. Not retried and not sent elsewhere"
        ))
    } else {
        invalid(format!(
            "the host refused the d3 request to {server}: {error}"
        ))
    }
}

fn refusal(server: &str, status: u64, body: &str) -> Failure {
    let detail = detail(body);
    match status {
        401 | 403 => invalid(format!(
            "d3 on {server} requires a bearer token (HTTP {status}); this plugin sends none"
        )),
        400 | 422 => invalid(format!(
            "d3 on {server} rejected the questions (HTTP {status}): {detail}"
        )),
        429 | 503 => transient(format!(
            "d3 on {server} is busy (HTTP {status}); it serves one request at a time. Not retried"
        )),
        500..=599 => transient(format!("d3 on {server} failed (HTTP {status}): {detail}")),
        _ => invalid(format!("d3 on {server} answered HTTP {status}: {detail}")),
    }
}

/// The start of a refusal's body, on one line.
fn detail(body: &str) -> String {
    let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

fn main() -> std::io::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let result = if bytes.len() > MAX_INPUT_BYTES {
        Err(invalid(format!(
            "the input may be at most {MAX_INPUT_BYTES} bytes"
        )))
    } else {
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| invalid("the input must be JSON"))
            .and_then(|input| run(&input))
    };
    let output =
        result.unwrap_or_else(|f| json!({"ok": false, "error": f.error, "retryable": f.retryable}));
    std::io::stdout().write_all(output.to_string().as_bytes())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
