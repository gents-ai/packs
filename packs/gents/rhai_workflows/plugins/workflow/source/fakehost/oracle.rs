//! Child oracles (L2 section 4.22): decide how a spawned child ends. Canned and label oracles
//! are values here; a fault's oracle is any closure from a spawn to an `Answer`. The journal
//! oracle replays a recorded journal for the differential's journal-backed cases.
use super::FakeHost;
use serde_json::Value;
use std::collections::BTreeMap;

/// How a child ends: its terminal `state` and the `text` `child_output` serves.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub state: String,
    pub text: String,
}

impl Answer {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            state: "completed".into(),
            text: text.into(),
        }
    }

    pub fn failed(text: impl Into<String>) -> Self {
        Self {
            state: "failed".into(),
            text: text.into(),
        }
    }

    pub fn interrupted() -> Self {
        Self {
            state: "interrupted".into(),
            text: String::new(),
        }
    }
}

/// The smallest value a JSON schema accepts: required properties only, empty arrays and
/// strings, zero. ponytail: ignores min/max bounds and combinators; extend when a case needs it.
pub fn minimal(schema: &Value) -> Value {
    if let Some(v) = schema.get("const").or(schema.pointer("/enum/0")) {
        return v.clone();
    }
    let ty = match &schema["type"] {
        Value::String(t) => t.as_str(),
        Value::Array(ts) => ts.first().and_then(Value::as_str).unwrap_or_default(),
        _ if schema.get("properties").is_some() => "object",
        _ => "",
    };
    match ty {
        "object" => schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|k| (k.to_string(), minimal(&schema["properties"][k])))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        "array" => Value::Array(Vec::new()),
        "string" => "".into(),
        "integer" | "number" => 0.into(),
        "boolean" => false.into(),
        _ => Value::Null,
    }
}

/// Every agent succeeds: with the minimal instance of the spawn's `output_schema` in a json
/// fence (the fake host's way to learn the schema), or `ok` without one.
pub fn canned(spawn: &Value) -> Answer {
    match spawn.get("output_schema") {
        Some(schema) => Answer::ok(format!("```json\n{}\n```", minimal(schema))),
        None => Answer::ok("ok"),
    }
}

/// Results keyed by spawn title, failing slots included; an unlabeled spawn is answered canned.
#[derive(Debug, Default)]
pub struct Labels(BTreeMap<String, Answer>);

impl Labels {
    /// `{"<title>": "<text>" | {"state": "failed", "text": "..."}}`.
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let labels = v
            .as_object()
            .ok_or("the oracle file is not an object keyed by spawn title")?;
        let mut out = BTreeMap::new();
        for (title, a) in labels {
            let answer = match a {
                Value::String(text) => Answer::ok(text),
                Value::Object(_) => Answer {
                    state: a["state"].as_str().unwrap_or("completed").into(),
                    text: a["text"].as_str().unwrap_or_default().into(),
                },
                _ => return Err(format!("label {title} is not a string or {{state, text}}")),
            };
            out.insert(title.clone(), answer);
        }
        Ok(Self(out))
    }

    pub fn answer(&self, spawn: &Value) -> Answer {
        let labeled = spawn["title"].as_str().and_then(|t| self.0.get(t));
        labeled.cloned().unwrap_or_else(|| canned(spawn))
    }
}

/// Answers spawns from a Grok `journal.jsonl`: its `spawn_agent` entries, in order, one per
/// spawn. A spawn with an output schema gets a non-text result re-rendered as a json fence.
#[derive(Debug)]
pub struct Journal {
    results: Vec<Value>,
    next: usize,
}

impl Journal {
    /// One JSON entry `{seq, kind, req_hash, result, at_ms}` per line; other kinds are skipped.
    pub fn from_jsonl(text: &str) -> Result<Self, String> {
        let mut results = Vec::new();
        for (n, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let entry: Value =
                serde_json::from_str(line).map_err(|e| format!("journal line {}: {e}", n + 1))?;
            if entry["kind"] == "spawn_agent" {
                results.push(entry["result"].clone());
            }
        }
        Ok(Self { results, next: 0 })
    }

    /// The next entry's result; a spent journal fails the spawn.
    pub fn answer(&mut self, spawn: &Value) -> Answer {
        let Some(result) = self.results.get(self.next) else {
            return Answer::failed("the journal has no spawn_agent entry left");
        };
        self.next += 1;
        let out = result.get("output").unwrap_or(result);
        let schema = spawn.get("output_schema").is_some()
            || spawn["prompt"]
                .as_str()
                .is_some_and(|p| p.contains("<output-contract>"));
        let text = match out {
            Value::String(s) => s.clone(),
            v if schema => format!("```json\n{v}\n```"),
            v => v.to_string(),
        };
        if result["success"] == false {
            Answer::failed(text)
        } else {
            Answer::ok(text)
        }
    }
}

/// The `spawn` effects of one output.
pub fn spawns(out: &Value) -> Vec<Value> {
    let spawn = out.pointer("/effects/spawn").and_then(Value::as_array);
    spawn.cloned().unwrap_or_default()
}

impl FakeHost {
    /// Ends the child `(slot, attempt)` as `answer` says.
    pub fn answer_child(&mut self, slot: &str, attempt: u64, answer: Answer) -> Result<(), String> {
        let child = (self.state.children)
            .get_mut(&(slot.into(), attempt))
            .ok_or_else(|| format!("no child ({slot}, {attempt})"))?;
        (child.state, child.text) = (answer.state, answer.text);
        Ok(())
    }

    /// Answers each of `spawns` that is still running through `oracle`.
    pub fn settle(&mut self, spawns: &[Value], mut oracle: impl FnMut(&Value) -> Answer) {
        for s in spawns {
            let key = (
                s["slot"].as_str().unwrap_or_default().to_string(),
                s["attempt"].as_u64().unwrap_or(0),
            );
            if self
                .state
                .children
                .get(&key)
                .is_some_and(|c| c.state == "running")
            {
                let _ = self.answer_child(&key.0, key.1, oracle(s));
            }
        }
    }
}
