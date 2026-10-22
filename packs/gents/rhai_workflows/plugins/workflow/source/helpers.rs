//! The pure host helpers of L2 section 4.7: `fingerprint`, `json_encode`, `render_template`
//! and the `git_diff_since` refusal. None takes a call index.
use crate::errors::{WORKFLOW_INVALID_INPUT, WORKFLOW_NOT_AVAILABLE};
use crate::replay::canonical;
use crate::sandbox::infrastructure_error;
use rhai::{Dynamic, EvalAltResult, Map};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Bundled templates, name to text. None ship yet.
const TEMPLATES: &[(&str, &str)] = &[];

/// `hex(sha256(utf8))[..16]`.
pub fn fingerprint(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].into()
}

/// Compact JSON with sorted keys.
pub fn json_encode(v: &Dynamic) -> Result<String, Box<EvalAltResult>> {
    let value: Value = rhai::serde::from_dynamic(v)?;
    Ok(canonical(&value))
}

/// Plain `{{key}}` replacement over `registry`'s text for `name`.
pub fn render(
    registry: &[(&str, &str)],
    name: &str,
    map: &Map,
) -> Result<String, Box<EvalAltResult>> {
    let Some((_, text)) = registry.iter().find(|(n, _)| *n == name) else {
        let known: Vec<&str> = registry.iter().map(|(n, _)| *n).collect();
        let known = if known.is_empty() {
            "none".into()
        } else {
            known.join(", ")
        };
        let what = format!("unknown template {name}; known: {known}");
        return Err(infrastructure_error(
            WORKFLOW_INVALID_INPUT,
            &what,
            "workflow help host",
        ));
    };
    let fill = |text: String, (key, value): (&_, &Dynamic)| {
        text.replace(&format!("{{{{{key}}}}}"), &value.to_string())
    };
    Ok(map.iter().fold((*text).into(), fill))
}

pub fn render_template(name: &str, map: &Map) -> Result<String, Box<EvalAltResult>> {
    render(TEMPLATES, name, map)
}

/// Always refuses: git is outside the sandbox.
pub fn git_diff_since(_rev: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    Err(infrastructure_error(
        WORKFLOW_NOT_AVAILABLE,
        "git_diff_since is not supported",
        r#"agent(#{prompt: "run git diff <rev> and return it", capability_mode: "read-only"})"#,
    ))
}
