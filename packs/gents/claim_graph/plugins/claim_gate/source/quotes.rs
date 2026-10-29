//! File existence (`validate.sh`) and verbatim quotes (`quotes.sh`) against
//! the bound source root. The host binds `source_root` read-only and hands
//! the plugin its canonical path, which is also the guest path.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::batch::Record;

/// Claims whose `from` is this file are questions; their quote is the
/// question itself and is not looked for.
pub const QUESTIONS_FILE: &str = "graph/claims/questions.jsonl";

/// The narrative file a claim's `from` names: the part before `#`.
pub fn from_file(claim: &Record) -> Option<&str> {
    claim.str("from").map(|from| from.split('#').next().unwrap_or(""))
}

/// A path under the root, or `None` for an absolute path or one that climbs
/// out with `..`.
fn under(root: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    path.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        .then(|| root.join(path))
}

/// Every claim's `from` file and non-null `sources[].doc`, deduplicated and
/// sorted, that does not exist under the root.
pub fn missing_files(root: &Path, claims: &[Record]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for claim in claims {
        if let Some(file) = from_file(claim) {
            paths.insert(file.to_owned());
        }
        for source in claim
            .fields
            .get("sources")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(doc) = source.get("doc").and_then(Value::as_str) {
                paths.insert(doc.to_owned());
            }
        }
    }
    paths
        .into_iter()
        .filter(|p| under(root, p).is_none_or(|path| std::fs::metadata(path).is_err()))
        .map(|p| format!("ERROR: file not found: {p}"))
        .collect()
}

pub struct QuoteReport {
    pub errors: Vec<String>,
    pub checked: u64,
    pub skipped: u64,
}

/// Whether `quote` is found in `file` as `grep -qF -- "$quote"` finds it:
/// each line of a multi-line quote is its own fixed pattern and any one
/// matching a line is a match; an empty pattern matches any line.
pub fn grep_fixed(file: &[u8], quote: &str) -> bool {
    quote.split('\n').any(|pattern| {
        if pattern.is_empty() {
            !file.is_empty()
        } else {
            contains(file, pattern.as_bytes())
        }
    })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let first = needle[0];
    let last_start = match haystack.len().checked_sub(needle.len()) {
        Some(n) => n,
        None => return false,
    };
    (0..=last_start).any(|i| haystack[i] == first && &haystack[i..i + needle.len()] == needle)
}

/// Checks each claim's quote in its `from` file, except questions and the
/// claims in `historical` (targets of a supersedes or retracts edge).
pub fn check_quotes(root: &Path, claims: &[Record], historical: &BTreeSet<String>) -> QuoteReport {
    let mut report = QuoteReport {
        errors: Vec::new(),
        checked: 0,
        skipped: 0,
    };
    let mut files: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    for claim in claims {
        let (Some(id), Some(file), Some(quote)) =
            (claim.str("id"), from_file(claim), claim.str("quote"))
        else {
            // A claim without these already failed its field rules.
            report.skipped += 1;
            continue;
        };
        if file == QUESTIONS_FILE || historical.contains(id) {
            report.skipped += 1;
            continue;
        }
        report.checked += 1;
        let bytes = files
            .entry(file.to_owned())
            .or_insert_with(|| under(root, file).and_then(|path| std::fs::read(path).ok()));
        if !bytes.as_deref().is_some_and(|b| grep_fixed(b, quote)) {
            report
                .errors
                .push(format!("ERROR: {id} quote not found verbatim in {file}"));
        }
    }
    report
}
