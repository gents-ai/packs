//! The workflow engine: `invoke` takes one plugin input and returns one output.
use serde_json::{Value, json};

pub mod address;
pub mod catalog;
pub mod data;
pub mod errors;
pub mod export;
#[cfg(any(test, feature = "fakehost"))]
pub mod fakehost;
pub mod helpers;
pub mod host_api;
pub mod journal;
pub mod keep;
pub mod options;
pub mod origin;
pub mod output_schema;
pub mod progress;
pub mod replay;
pub mod resume;
pub mod rounds;
pub mod run;
pub mod sandbox;
pub mod scratch;
pub mod smoke;
pub mod tiers;
pub mod validate;
pub mod verbs;
pub mod wire;

/// The four input shapes `invoke` tells apart (L2 section 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `protocol: 1`: step 0 or a later job step.
    JobStep,
    /// `protocol: 1` with `data_results` or `job_results`: a round inside one step.
    Round,
    /// `argv` without `protocol`: raw tool arguments and catalog calls.
    Raw,
    /// Neither: a callback projection.
    Callback,
}

pub fn classify(input: &Value) -> Shape {
    if input.get("protocol").and_then(Value::as_u64) == Some(1) {
        if input.get("data_results").is_some() || input.get("job_results").is_some() {
            Shape::Round
        } else {
            Shape::JobStep
        }
    } else if input.get("argv").is_some() {
        Shape::Raw
    } else {
        Shape::Callback
    }
}

/// A `fail` output; `message` already reads `<what>. Next: <one call>`.
pub fn fail(code: &str, message: &str) -> Value {
    json!({"fail": {"code": code, "message": format!("{code}: {message}")}})
}

pub fn invoke(mut input: Value) -> Value {
    // A model fills the optional string inputs it does not use with "": an empty script or path names nothing.
    for key in ["script", "path"] {
        if input[key] == "" {
            input.as_object_mut().map(|o| o.remove(key));
        }
    }
    if let Err(out) = run::budget_flag(&mut input) {
        return out;
    }
    resume::budget_field(&mut input);
    let branch = match classify(&input) {
        Shape::JobStep | Shape::Round if input["pin"].is_null() && resume::is_resume(&input) => {
            return resume::step0(&input);
        }
        Shape::JobStep | Shape::Round if input["pin"].is_null() && verbs::is_control(&input) => {
            return verbs::control(&input);
        }
        Shape::JobStep | Shape::Round if input["pin"].is_null() && keep::is_keep(&input) => {
            return verbs::deferred(&input).unwrap_or_else(|| keep::serve(&input));
        }
        Shape::JobStep | Shape::Round if input["pin"].is_null() && export::is_export(&input) => {
            return export::serve(&input);
        }
        Shape::JobStep | Shape::Round if input["pin"].is_null() => return run::step0(&input),
        Shape::JobStep | Shape::Round => match rounds::advance(&input) {
            Err(out) | Ok(rounds::Step::Ask(out)) => return out,
            Ok(rounds::Step::Replay(replay)) => return run::step(&input, replay),
        },
        Shape::Raw => return verbs::raw(&input),
        Shape::Callback => "callback",
    };
    fail(
        "workflow_not_available",
        &format!("the {branch} branch is not built yet. Next: workflow help doctor"),
    )
}
