//! secscan plugin: the security_scan pack's regex pre-scan, ported from
//! gents `crates/gents-cli/src/commands/pack/secscan/{mod,matchers,payload}.rs`
//! so the pre-scan runs sandboxed inside the pack instead of the gents
//! binary. One canonical JSON value on stdin, one on stdout; stderr carries
//! diagnostics (see TOOL.md).
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

mod matchers;
mod payload;
mod scan;

pub(crate) use matchers::NoiseTier;
pub(crate) use scan::FileCandidates;
// Only the registry/payload discovery tests reach these directly; the real
// entry point goes through `scan::scan_root`/`scan::scan_files`.
#[cfg(test)]
pub(crate) use scan::{compile_patterns, match_content, CandidateMatch};

const DEFAULT_MAX_PAYLOAD_CHARS: usize = 49152;

#[derive(Debug, Serialize)]
struct SlugCount {
    slug: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct PluginOutput {
    payload: String,
    candidate_total: usize,
    candidate_files: usize,
    slug_counts: Vec<SlugCount>,
    overflow_count: usize,
}

impl From<payload::ScanOutput> for PluginOutput {
    fn from(out: payload::ScanOutput) -> Self {
        PluginOutput {
            payload: out.payload,
            candidate_total: out.candidate_total,
            candidate_files: out.candidate_files,
            slug_counts: out
                .slug_counts
                .into_iter()
                .map(|(slug, count)| SlugCount { slug, count })
                .collect(),
            overflow_count: out.overflow_count,
        }
    }
}

fn main() {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("secscan: reading stdin: {err}");
        std::process::exit(1);
    }

    match run(&input) {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("secscan: serializing the output: {err}");
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!("secscan: {err}");
            std::process::exit(1);
        }
    }
}

fn run(input: &str) -> Result<PluginOutput, String> {
    let value: Value =
        serde_json::from_str(input).map_err(|err| format!("invalid JSON input: {err}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "input must be a JSON object".to_string())?;

    let max_payload_chars = match obj.get("max_payload_chars") {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "\"max_payload_chars\" must be a non-negative integer".to_string())?
            as usize,
        None => DEFAULT_MAX_PAYLOAD_CHARS,
    };

    let has_root = obj.contains_key("root");
    let has_files = obj.contains_key("files");
    if has_root == has_files {
        return Err("input must have exactly one of \"root\" or \"files\"".to_string());
    }

    let patterns = scan::compile_patterns()?;
    let files: Vec<FileCandidates> = if has_root {
        let root = obj["root"]
            .as_str()
            .ok_or_else(|| "\"root\" must be a string".to_string())?;
        let root_path = Path::new(root);
        if !root_path.is_absolute() {
            return Err(format!("\"root\" must be an absolute path, got {root:?}"));
        }
        if !root_path.is_dir() {
            return Err(format!("\"root\" is not a readable directory: {root}"));
        }
        scan::scan_root(&patterns, root_path)?
    } else {
        let entries = obj["files"]
            .as_array()
            .ok_or_else(|| "\"files\" must be an array".to_string())?;
        let mut inputs = Vec::with_capacity(entries.len());
        for (i, entry) in entries.iter().enumerate() {
            let entry_obj = entry
                .as_object()
                .ok_or_else(|| format!("files[{i}] must be an object"))?;
            let path = entry_obj
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("files[{i}].path must be a string"))?;
            let content = entry_obj
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("files[{i}].content must be a string"))?;
            inputs.push((path.to_string(), content.to_string()));
        }
        scan::scan_files(&patterns, inputs)
    };

    let output = payload::format_payload(&files, max_payload_chars);
    Ok(PluginOutput::from(output))
}

#[cfg(test)]
mod tests {
    //! Bad-input coverage. The `gents pack test` fixture format
    //! (`{"input": ..., "expect": ...}`) can only assert a call succeeds; it
    //! has no way to express "this input must fail", so every malformed-input
    //! case lives here instead, run with `cargo test`.
    use super::*;

    fn err(input: &str) -> String {
        run(input).expect_err("expected this input to be rejected")
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(err("not json").contains("invalid JSON input"));
    }

    #[test]
    fn rejects_non_object_input() {
        assert!(err("[1, 2, 3]").contains("must be a JSON object"));
    }

    #[test]
    fn rejects_neither_root_nor_files() {
        assert!(err(r#"{"max_payload_chars": 100}"#).contains("exactly one of"));
    }

    #[test]
    fn rejects_both_root_and_files() {
        assert!(err(r#"{"root": "/tmp", "files": []}"#).contains("exactly one of"));
    }

    #[test]
    fn rejects_non_string_root() {
        assert!(err(r#"{"root": 42}"#).contains("\"root\" must be a string"));
    }

    #[test]
    fn rejects_relative_root() {
        assert!(err(r#"{"root": "relative/path"}"#).contains("absolute path"));
    }

    #[test]
    fn rejects_unreadable_root() {
        assert!(err(r#"{"root": "/does/not/exist/at/all"}"#).contains("readable directory"));
    }

    #[test]
    fn rejects_non_array_files() {
        assert!(err(r#"{"files": "nope"}"#).contains("\"files\" must be an array"));
    }

    #[test]
    fn rejects_file_entry_missing_content() {
        assert!(err(r#"{"files": [{"path": "a.rs"}]}"#).contains("content must be a string"));
    }

    #[test]
    fn rejects_file_entry_missing_path() {
        assert!(err(r#"{"files": [{"content": "x"}]}"#).contains("path must be a string"));
    }

    #[test]
    fn rejects_negative_max_payload_chars() {
        assert!(err(r#"{"files": [], "max_payload_chars": -1}"#).contains("non-negative integer"));
    }

    #[test]
    fn rejects_non_integer_max_payload_chars() {
        assert!(
            err(r#"{"files": [], "max_payload_chars": "big"}"#).contains("non-negative integer")
        );
    }

    #[test]
    fn accepts_empty_files_list() {
        let out = run(r#"{"files": []}"#).expect("empty file list is valid input");
        assert_eq!(out.candidate_total, 0);
    }
}
