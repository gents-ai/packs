//! Regex matcher engine core: walk a tree (or scan an explicit set of file
//! contents), apply the matcher registry to each admitted file, and collect
//! per-file candidate matches.
//!
//! Ported from gents `crates/gents-cli/src/commands/pack/secscan/mod.rs`,
//! with the per-file matching reworked for throughput: every pattern is
//! compiled exactly once (never duplicated per extension), the extension
//! gate is checked before a pattern ever runs against the content, and line
//! numbers are resolved with a single forward sweep over the sorted match
//! offsets rather than by recounting newlines from the start of the file for
//! every hit (the original `line_at` was O(offset) per match).
//!
//! A `RegexSet` prefilter (one combined pass deciding which of the ~14
//! patterns can possibly match, before running `find_iter` only for those)
//! was tried first and measured slower, not faster: on `/home/vcq/projects/gents`
//! it ran 2-2.7x slower than the original CLI's own naive per-pattern loop,
//! because the `RegexSet` meta-engine's combined automaton for this
//! particular pattern mix (several bounded repetitions, case-insensitive
//! alternations) loses the cheap literal prefilter each pattern gets when
//! compiled on its own. Compiling each pattern once and running `find_iter`
//! directly (this file's design) measured ~2.5x *faster* than the original
//! CLI on the same repo; see the pack's benchmark for the numbers.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::matchers::{registry, Matcher, NoiseTier};

#[derive(Debug, Clone)]
pub(crate) struct CandidateMatch {
    pub slug: &'static str,
    pub tier: NoiseTier,
    pub line: usize,     // 1-based
    pub excerpt: String, // matched line, trimmed, max 160 chars
}

#[derive(Debug, Clone)]
pub(crate) struct FileCandidates {
    pub path: String, // relative to scan root, forward slashes
    pub matches: Vec<CandidateMatch>,
}

/// Every registry pattern compiled exactly once, in registry/pattern
/// declaration order, so a `find_iter` hit traces back to its registry
/// entry by index and the resulting candidate order matches the ported
/// algorithm exactly.
///
/// Built on `regex::bytes` and `.unicode(false)` rather than the default
/// (Unicode-aware) string API: the string API's compiler always inserts an
/// extra automaton stage that guarantees every match boundary lands on a
/// UTF-8 char boundary, and building that stage for these ~14 patterns costs
/// tens of millions of instructions, which alone exhausts a wasm plugin
/// call's fuel budget before it reads its first byte of input. None of these
/// patterns rely on Unicode-specific semantics (no `\p{..}`, no non-ASCII
/// literals) and every `content` slice they run against is `&str`, so the
/// byte offsets `find_iter` returns are never used to slice `content`
/// directly (only newline-derived offsets are, and a `\n` byte is always a
/// valid char boundary) - matching stays correct for the ASCII-only tokens
/// these patterns target. vertexia: on genuinely non-ASCII content near a
/// bounded quantifier (`{0,200}` etc.) a handful of multi-byte characters
/// could count differently against that bound than the Unicode-aware string
/// API would; verified byte-identical against the string-API port on a real
/// repo (see the pack's benchmark). Upgrade path if that ever matters: split
/// hot ASCII-only patterns (byte mode) from the rare ones that must see full
/// Unicode (string mode, only for those), rather than paying the Utf8Compiler
/// cost for the whole registry.
pub(crate) type Patterns = Vec<(usize, regex::bytes::Regex)>;

/// Every registry pattern compiled once per call, tagged with the registry
/// index it came from so a hit can be traced back to its `Matcher`.
pub(crate) fn compile_patterns() -> Result<Patterns, String> {
    let mut compiled = Vec::new();
    for (idx, matcher) in registry().iter().enumerate() {
        for pattern in matcher.patterns {
            let regex = regex::bytes::RegexBuilder::new(pattern)
                .unicode(false)
                .build()
                .map_err(|err| format!("invalid matcher pattern {pattern:?}: {err}"))?;
            compiled.push((idx, regex));
        }
    }
    Ok(compiled)
}

fn admits_extension(matcher: &Matcher, extension: Option<&str>) -> bool {
    matcher.extensions.is_empty()
        || matches!(extension, Some(ext) if matcher.extensions.contains(&ext))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

pub(crate) fn match_content(patterns: &Patterns, content: &str, path: &str) -> Vec<CandidateMatch> {
    let extension = Path::new(path).extension().and_then(|e| e.to_str());
    let reg = registry();
    let bytes = content.as_bytes();

    // Pass 1: the extension gate is checked before a pattern ever runs
    // against the content; `find_iter` only runs for the admitted patterns,
    // in registry/pattern declaration order so the resulting candidate
    // order matches the ported algorithm exactly.
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (registry idx, byte offset)
    for (registry_idx, regex) in patterns {
        if !admits_extension(&reg[*registry_idx], extension) {
            continue;
        }
        for found in regex.find_iter(bytes) {
            hits.push((*registry_idx, found.start()));
        }
    }
    if hits.is_empty() {
        return Vec::new();
    }

    // Pass 2: resolve every distinct offset's line number and excerpt with
    // one forward sweep over offsets sorted ascending; no offset is
    // recounted from the start of the file.
    let mut sorted_offsets: Vec<usize> = hits.iter().map(|&(_, start)| start).collect();
    sorted_offsets.sort_unstable();
    sorted_offsets.dedup();

    let mut line_no = 1usize;
    let mut line_start = 0usize;
    let mut line_end = memchr::memchr(b'\n', bytes).unwrap_or(bytes.len());
    let mut resolved: HashMap<usize, (usize, String)> =
        HashMap::with_capacity(sorted_offsets.len());

    for start in sorted_offsets {
        while start > line_end && line_end < bytes.len() {
            let next_start = line_end + 1;
            line_no += 1;
            line_end = memchr::memchr(b'\n', &bytes[next_start..])
                .map(|i| next_start + i)
                .unwrap_or(bytes.len());
            line_start = next_start;
        }
        let excerpt = truncate_chars(content[line_start..line_end].trim(), 160);
        resolved.insert(start, (line_no, excerpt));
    }

    // Pass 3: rebuild in original declaration order, deduping per
    // (slug, line) exactly like the ported algorithm.
    let mut seen = HashSet::new();
    let mut matches = Vec::with_capacity(hits.len());
    for (registry_idx, start) in hits {
        let matcher = &reg[registry_idx];
        let (line, excerpt) = &resolved[&start];
        if !seen.insert((matcher.slug, *line)) {
            continue;
        }
        matches.push(CandidateMatch {
            slug: matcher.slug,
            tier: matcher.tier,
            line: *line,
            excerpt: excerpt.clone(),
        });
    }

    matches
}

const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Scan explicit file contents supplied by the caller, never touching the
/// filesystem. Same core (`match_content`), same candidate semantics
/// (size cap, sort by path) as [`scan_root`].
pub(crate) fn scan_files(patterns: &Patterns, files: Vec<(String, String)>) -> Vec<FileCandidates> {
    let mut out = Vec::new();
    for (path, content) in files {
        if content.len() as u64 > MAX_FILE_BYTES {
            continue;
        }
        let matches = match_content(patterns, &content, &path);
        if matches.is_empty() {
            continue;
        }
        out.push(FileCandidates { path, matches });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub(crate) fn scan_root(patterns: &Patterns, root: &Path) -> Result<Vec<FileCandidates>, String> {
    let mut files = Vec::new();

    // `require_git` defaults to true, which silently disables `.gitignore`
    // parsing outside an actual git repository (e.g. a bare tempdir in
    // tests, or a pack root that isn't its own git checkout).
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .build()
    {
        let entry = entry.map_err(|err| format!("walking {}: {err}", root.display()))?;
        let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }

        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }

        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(content) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");

        let matches = match_content(patterns, content, &relative);
        if matches.is_empty() {
            continue;
        }

        files.push(FileCandidates {
            path: relative,
            matches,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_content_maps_offsets_to_lines() {
        let content = "fn ok() {}\nlet api_key = \"sk_live_ABCDEF1234567890\";\n";
        let matches = match_content(&compile_patterns().unwrap(), content, "src/config.rs");
        assert!(
            matches
                .iter()
                .any(|m| m.slug == "secrets-exposure" && m.line == 2),
            "expected secrets-exposure on line 2, got {matches:?}"
        );
    }

    #[test]
    fn extension_gate_excludes_non_matching_files() {
        // graphql-injection is gated to .rs; the same text in a .md must not fire it.
        let content =
            "format!(\"mutation {{ create_Job(input: {{ run_id: \\\"{run_id}\\\" }}) }}\")";
        let rs = match_content(&compile_patterns().unwrap(), content, "src/a.rs");
        let md = match_content(&compile_patterns().unwrap(), content, "docs/a.md");
        assert!(rs.iter().any(|m| m.slug == "graphql-injection"));
        assert!(!md.iter().any(|m| m.slug == "graphql-injection"));
    }

    #[test]
    fn scan_root_respects_gitignore_and_returns_relative_paths() {
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(
            dir.join("src/leak.rs"),
            "let api_key = \"sk_live_ABCDEF1234567890\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("target/leak.rs"),
            "let api_key = \"sk_live_ABCDEF1234567890\";\n",
        )
        .unwrap();
        let files = scan_root(&compile_patterns().unwrap(), &dir).expect("scan");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["src/leak.rs"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A bare tempdir helper: this crate has no dependency on `tempfile`
    /// (not on the plugin's approved dependency list), so it makes its own
    /// unique directory under the OS temp dir directly.
    fn tempfile_dir() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "secscan-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        dir.push(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
