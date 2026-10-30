//! Where a claim's quote comes from, file existence (`validate.sh`) and
//! verbatim quotes (`quotes.sh`): against the bound source root for a file
//! `from`, against the stored `Source.text` for a `source:` one. The host
//! binds `source_root` read-only and hands the plugin its canonical path,
//! which is also the guest path.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::batch::Record;
use crate::store::Found;

/// Kinds whose quote may be their own text: a question, challenge or review
/// quotes nobody; such a claim is not looked for when `quote == text`.
pub const SELF_QUOTING: [&str; 3] = ["question", "challenge", "review"];
/// A `from` citing a stored source: `source:<source_id>#<anchor>`.
pub const SOURCE_PREFIX: &str = "source:";
/// A `from` of an unsourced claim: `memory:<topic>`.
pub const MEMORY_PREFIX: &str = "memory:";

/// What a claim's `from` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin<'a> {
    /// A narrative file under the source root: the part before `#`.
    File(&'a str),
    /// A stored `Source`: its id, between `source:` and `#` (may be empty,
    /// which the field rules refuse).
    Source(&'a str),
    /// Nothing outside the writer: no quote to look for.
    Memory,
}

fn before_anchor(from: &str) -> &str {
    from.split('#').next().unwrap_or("")
}

pub fn origin(claim: &Record) -> Option<Origin<'_>> {
    let from = claim.str("from")?;
    Some(if let Some(rest) = from.strip_prefix(SOURCE_PREFIX) {
        Origin::Source(before_anchor(rest))
    } else if from.starts_with(MEMORY_PREFIX) {
        Origin::Memory
    } else {
        Origin::File(before_anchor(from))
    })
}

/// Whether the claim is a self-quoting kind whose quote is its text.
pub fn self_quoted(claim: &Record) -> bool {
    claim.str("kind").is_some_and(|k| SELF_QUOTING.contains(&k))
        && claim.str("quote").is_some()
        && claim.str("text") == claim.str("quote")
}

/// The non-empty `sources[].source` ids a claim lists.
pub fn listed_sources(claim: &Record) -> impl Iterator<Item = &str> {
    claim
        .fields
        .get("sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("source").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

/// Every stored source a claim cites, its `from` first, deduplicated.
pub fn cited_sources(claim: &Record) -> Vec<&str> {
    let mut cited = Vec::new();
    if let Some(Origin::Source(id)) = origin(claim) {
        if !id.is_empty() {
            cited.push(id);
        }
    }
    for id in listed_sources(claim) {
        if !cited.contains(&id) {
            cited.push(id);
        }
    }
    cited
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
/// sorted, that does not exist under the root. A `source:` or `memory:`
/// `from` names no file.
pub fn missing_files(root: &Path, claims: &[Record]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for claim in claims {
        if let Some(Origin::File(file)) = origin(claim) {
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

/// Whether the claim's quote is to be looked for at all: not a `memory:`
/// claim, not a self-quoting kind whose quote is its text, not under the
/// profile's `questions_from_prefix`, not in `historical` (targets of a
/// supersedes or retracts edge).
pub fn wants_check(
    claim: &Record,
    historical: &BTreeSet<String>,
    questions_prefix: Option<&str>,
) -> bool {
    let question_file = questions_prefix
        .is_some_and(|prefix| claim.str("from").is_some_and(|from| from.starts_with(prefix)));
    origin(claim).is_some_and(|o| o != Origin::Memory)
        && !self_quoted(claim)
        && !question_file
        && !claim.str("id").is_some_and(|id| historical.contains(id))
}

/// Where the quotes are looked for: the bound root (file `from`s), and the
/// store's answers once it has answered (`source:` `from`s).
pub struct Places<'a> {
    pub root: Option<&'a Path>,
    pub store: Option<&'a Found>,
}

/// Checks each claim's quote where its `from` points, unless `wants_check`
/// exempts it. A file quote without a root, a source quote before the store
/// answered, and a quote whose source is missing or failed (reported by the
/// source reference rule) are skipped too.
pub fn check_quotes(
    places: &Places,
    claims: &[Record],
    historical: &BTreeSet<String>,
    questions_prefix: Option<&str>,
) -> QuoteReport {
    let mut report = QuoteReport {
        errors: Vec::new(),
        checked: 0,
        skipped: 0,
    };
    let mut files: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    for claim in claims {
        let (Some(id), Some(origin), Some(quote)) =
            (claim.str("id"), origin(claim), claim.str("quote"))
        else {
            // A claim without these already failed its field rules.
            report.skipped += 1;
            continue;
        };
        if !wants_check(claim, historical, questions_prefix) {
            report.skipped += 1;
            continue;
        }
        match origin {
            Origin::File(file) => {
                let Some(root) = places.root else {
                    report.skipped += 1;
                    continue;
                };
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
            Origin::Source(source) => {
                let Some(found) = places.store.filter(|f| f.source_ok(source)) else {
                    report.skipped += 1;
                    continue;
                };
                report.checked += 1;
                if !found.source_quotes.contains(id) {
                    report.errors.push(format!(
                        "ERROR: {id} quote not found verbatim in source {source}"
                    ));
                }
            }
            Origin::Memory => report.skipped += 1,
        }
    }
    report
}
