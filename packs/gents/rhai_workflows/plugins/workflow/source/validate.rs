//! Static validation of a workflow script (L2 section 4.5, steps 1-4): the token
//! prefix, the meta pass, shadowing and strict variables. The smoke path and the draft
//! store come with the `check` verb.
use rhai::{Dynamic, Scope};

use crate::errors::{WORKFLOW_VALIDATION_FAILED, message};
use crate::sandbox::{self, LogBuffer, SuspensionCell};

/// Names a script `fn` may not take: host functions and stubs.
pub const RESERVED: [&str; 19] = [
    "agent",
    "parallel",
    "phase",
    "log",
    "complete",
    "pause",
    "await_user",
    "budget",
    "write_scratch_file",
    "read_scratch_file",
    "git_diff_since",
    "fingerprint",
    "json_encode",
    "render_template",
    "timestamp",
    "sleep",
    "exit",
    "print",
    "debug",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub phases: Vec<Phase>,
}

fn refuse(what: &str) -> String {
    message(
        WORKFLOW_VALIDATION_FAILED,
        what,
        "fix the script, then run workflow check",
    )
}

/// The source with leading whitespace and comments (`//` and nested `/* */`) removed.
fn skip_trivia(mut source: &str) -> &str {
    loop {
        source = source.trim_start();
        if let Some(rest) = source.strip_prefix("//") {
            source = rest.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(mut rest) = source.strip_prefix("/*") {
            let mut depth = 1;
            while depth > 0 {
                match (rest.find("/*"), rest.find("*/")) {
                    (Some(open), Some(close)) if open < close => {
                        depth += 1;
                        rest = &rest[open + 2..];
                    }
                    (_, Some(close)) => {
                        depth -= 1;
                        rest = &rest[close + 2..];
                    }
                    _ => return "",
                }
            }
            source = rest;
        } else {
            return source;
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn string_field(map: &rhai::Map, key: &str, what: &str) -> Result<Option<String>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value
            .clone()
            .into_string()
            .map(Some)
            .map_err(|_| refuse(&format!("{what} must be a string"))),
    }
}

fn meta_from(value: Dynamic) -> Result<Meta, String> {
    let map = value
        .try_cast::<rhai::Map>()
        .ok_or_else(|| refuse("meta must be an object literal"))?;
    let name =
        string_field(&map, "name", "meta.name")?.ok_or_else(|| refuse("meta.name is required"))?;
    if !valid_name(&name) {
        return Err(refuse(&format!(
            "meta.name {name:?} must be lowercase words joined by single hyphens"
        )));
    }
    let description = string_field(&map, "description", "meta.description")?
        .filter(|text| !text.is_empty())
        .ok_or_else(|| refuse("meta.description must be a non-empty string"))?;
    let when_to_use = string_field(&map, "when_to_use", "meta.when_to_use")?;
    let mut phases: Vec<Phase> = Vec::new();
    if let Some(value) = map.get("phases") {
        let items = value
            .clone()
            .into_array()
            .map_err(|_| refuse("meta.phases must be an array"))?;
        for item in items {
            let entry = item
                .try_cast::<rhai::Map>()
                .ok_or_else(|| refuse("each meta.phases entry must be an object"))?;
            let title = string_field(&entry, "title", "a phase title")?
                .ok_or_else(|| refuse("each phase needs a title"))?;
            if phases.iter().any(|phase| phase.title == title) {
                return Err(refuse(&format!("phase title {title:?} is not unique")));
            }
            let detail = string_field(&entry, "detail", "a phase detail")?;
            phases.push(Phase { title, detail });
        }
    }
    Ok(Meta {
        name,
        description,
        when_to_use,
        phases,
    })
}

/// Validate `source`; the error is the full `workflow_validation_failed: ...` message.
pub fn validate(source: &str) -> Result<Meta, String> {
    if !skip_trivia(source).starts_with("let meta = #{") {
        // One word is a workflow name or file, not source: name the call that runs it.
        let word = source.trim();
        if !word.is_empty() && !word.contains(char::is_whitespace) && word.len() <= 256 {
            let name = (std::path::Path::new(word).file_stem())
                .map_or(word, |n| n.to_str().unwrap_or(word));
            let what = format!("script holds `{word}`, which is a name, not source");
            let next = format!("workflow run {name} (leave out script and path)");
            return Err(message(WORKFLOW_VALIDATION_FAILED, &what, &next));
        }
        return Err(refuse("the script must start with `let meta = #{`"));
    }
    let mut engine = sandbox::engine(&LogBuffer::default(), &SuspensionCell::default(), true);
    let ast = engine
        .compile(source)
        .map_err(|err| refuse(&format!("the script does not compile: {err}")))?;
    let meta = ast
        .iter_literal_variables(true, true)
        .find(|(name, ..)| *name == "meta")
        .map(|(.., value)| value)
        .ok_or_else(|| refuse("meta must be a pure literal"))?;
    let meta = meta_from(meta)?;

    let mut functions: Vec<_> = ast
        .iter_functions()
        .map(|function| (function.name, function.params))
        .collect();
    functions.sort();
    if let Some((name, _)) = functions.iter().find(|(name, _)| RESERVED.contains(name)) {
        return Err(refuse(&format!("fn {name} shadows a host function")));
    }

    engine.set_strict_variables(true);
    let mut scope = Scope::new();
    scope.push_constant("args", Dynamic::UNIT);
    engine
        .compile_with_scope(&scope, source)
        .map_err(|err| refuse(&format!("the script does not compile: {err}")))?;
    Ok(meta)
}
