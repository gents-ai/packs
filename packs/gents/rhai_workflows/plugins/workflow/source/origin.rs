//! Origin prediction and the policy pre-checks (L2 section 4.10; root C1, C5.5, C5.6). The host
//! still decides; these refusals only give the model a better message first.
use crate::errors::{WORKFLOW_DEPTH_EXCEEDED, WORKFLOW_POLICY_DENIED, message};
use crate::wire::Origin;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// How many directories below a mount the file search goes.
const DEPTH: usize = 8;

/// Whether a file under `dir` holds exactly `source`. Symlinks are not followed.
fn holds(dir: &Path, source: &[u8], depth: usize) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        match (kind.is_dir(), kind.is_file()) {
            (true, _) => depth > 0 && holds(&entry.path(), source, depth - 1),
            (_, true) => {
                let sized = entry
                    .metadata()
                    .is_ok_and(|m| m.len() == source.len() as u64);
                sized && fs::read(entry.path()).is_ok_and(|bytes| bytes == source)
            }
            _ => false,
        }
    })
}

/// `installed` iff `source` is a file under a `mounts[]` entry with `trusted: true`; anything
/// else (project, kept, inline, path, an untrusted pack) is `dynamic`.
pub fn predict(source: &str, mounts: &Value) -> Origin {
    let mut trusted = (mounts.as_array().into_iter().flatten())
        .filter(|m| m["trusted"] == true)
        .filter_map(|m| m["path"].as_str());
    match trusted.any(|path| holds(Path::new(path), source.as_bytes(), DEPTH)) {
        true => Origin::Installed,
        false => Origin::Dynamic,
    }
}

/// The origin a step runs under: the host's echo from step 1 on, else the prediction.
pub fn of(input: &Value, source: &str) -> Origin {
    match input["origin"].as_str() {
        Some("installed") => Origin::Installed,
        Some("dynamic") => Origin::Dynamic,
        _ => predict(source, &input["mounts"]),
    }
}

/// What `ceiling.dynamic` leaves a replay.
#[derive(Debug, Default)]
pub struct Policy {
    /// `Some(ids)` narrows `ceiling.targets` to these: `dynamic: run`.
    pub targets: Option<Vec<String>>,
    /// The `workflow_policy_denied` message the first `agent()` or `parallel()` gets.
    pub spawn_refusal: Option<String>,
}

/// The policy for a script of `name`. `Err` is the message of a run refused outright.
/// A ceiling without a readable `dynamic` field reads as `check`, the job block's default.
pub fn policy(input: &Value, source: &str, name: &str) -> Result<Policy, String> {
    if of(input, source) == Origin::Installed {
        return Ok(Policy::default());
    }
    let ceiling = &input["ceiling"];
    let would = format!("{name} would run as a dynamic script and job.policy.dynamic is");
    let denied = |what: String, next: &str| message(WORKFLOW_POLICY_DENIED, &what, next);
    match ceiling["dynamic"].as_str() {
        Some("off") => Err(denied(format!("{would} off"), "workflow help lifecycle")),
        Some("run") => Ok(Policy {
            targets: Some(
                (ceiling["dynamic_targets"].as_array().into_iter().flatten())
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
            ),
            spawn_refusal: None,
        }),
        _ => Ok(Policy {
            targets: None,
            spawn_refusal: Some(denied(
                format!("{would} check"),
                r#"ask the operator to trust the pack or set dynamic "run""#,
            )),
        }),
    }
}

/// The `workflow_depth_exceeded` message when a job's agent started this call (root C5.6).
pub fn nested(input: &Value) -> Option<String> {
    (input.pointer("/caller/started_by_job") == Some(&Value::Bool(true))).then(|| {
        let what = "a workflow starts only from a top-level session, not from a job's agent";
        let next = "end your turn and ask the user to run the workflow from their own session";
        message(WORKFLOW_DEPTH_EXCEEDED, what, next)
    })
}
