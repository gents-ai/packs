//! Scratch files (L2 section 4.12, root C2.5): `write_scratch_file` emits a `{name, content}`
//! record and a `scratch:<name>` note; `read_scratch_file` reads the note back.
use crate::errors::WORKFLOW_INVALID_INPUT;
use crate::host_api::Host;
use crate::journal::scratch_id;
use crate::sandbox::infrastructure_error;
use rhai::EvalAltResult;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_NAME: usize = 64;
/// The content cap of one file.
pub const MAX_BYTES: usize = 1 << 20;
/// The files one row may write.
pub const MAX_FILES: usize = 32;

fn invalid(what: &str, next: &str) -> Box<EvalAltResult> {
    infrastructure_error(WORKFLOW_INVALID_INPUT, what, next)
}

/// One path component of `[a-z0-9._-]`, 1 to 64 bytes, and not `.` or `..`.
fn name_ok(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name != "."
        && name != ".."
        && (name.bytes()).all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// Writes `name` and returns its path: the `scratch:<name>` note's (a replayed write, no new
/// record), else the host's for that name, else `<cwd>/.gents/workflows/runs/<tool_call_id>/<name>`.
pub fn write_scratch_file(
    h: &mut Host,
    name: &str,
    content: &str,
) -> Result<String, Box<EvalAltResult>> {
    if !name_ok(name) {
        let what = format!(
            "scratch name {name:?} must be one path component of a-z, 0-9, '.', '_' or '-', 1 to {MAX_NAME} characters"
        );
        return Err(invalid(&what, "write_scratch_file(\"report.md\", content)"));
    }
    if content.len() > MAX_BYTES {
        let what = format!(
            "scratch content is {} bytes; the cap is {MAX_BYTES} bytes per file",
            content.len()
        );
        return Err(invalid(
            &what,
            "write_scratch_file(name, <shorter content>)",
        ));
    }
    let id = scratch_id(name);
    let note = h.journal.get(&id);
    if note.is_some_and(|n| n["content"].as_str() != Some(content)) {
        let what =
            format!("scratch file {name} was already written in this run with other content");
        return Err(invalid(&what, "write_scratch_file(<a new name>, content)"));
    }
    let noted = note.and_then(|n| n["path"].as_str()).map(String::from);
    // A name counts against the row's cap unless it only replays an earlier row's file.
    let earlier = (noted.as_deref()).is_some_and(|p| {
        h.run_dir
            .as_deref()
            .is_none_or(|dir| !Path::new(p).starts_with(dir))
    });
    if !earlier {
        if h.scratch_names.len() >= MAX_FILES && !h.scratch_names.contains(name) {
            let what = format!("a row writes at most {MAX_FILES} scratch files");
            return Err(invalid(
                &what,
                "write_scratch_file(<one name>, <the content of several files joined>)",
            ));
        }
        h.scratch_names.insert(name.into());
    }
    if let Some(path) = noted {
        return Ok(path);
    }
    let path = match (h.host_scratch.get(name), &h.run_dir) {
        (Some(path), _) => path.clone(),
        (None, Some(dir)) => Path::new(dir).join(name).to_string_lossy().into_owned(),
        (None, None) => {
            let what = "the input has no caller.cwd and caller.tool_call_id".to_string();
            return Err(invalid(&what, "workflow help host"));
        }
    };
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    let note = json!({"sha256": sha256, "bytes": content.len(), "content": content, "path": path});
    h.journal.put(&id, note);
    h.scratch.push(json!({"name": name, "content": content}));
    Ok(path)
}

/// The content of an earlier `write_scratch_file(name, ..)` of this run.
pub fn read_scratch_file(h: &Host, name: &str) -> Result<String, Box<EvalAltResult>> {
    match h
        .journal
        .get(&scratch_id(name))
        .and_then(|n| n["content"].as_str())
    {
        Some(content) => Ok(content.into()),
        None => Err(invalid(
            &format!("no scratch file {name:?} was written in this run"),
            "write_scratch_file(name, content)",
        )),
    }
}
