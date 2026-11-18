//! Splits a `git diff` patch into one entry per changed file.
//!
//! The patch is untrusted text: nothing here interprets it beyond the
//! header lines git writes, and the diff of each file is kept verbatim (cut
//! at a line boundary when it exceeds the per-file bound).

/// One changed file as it appears in the patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub old_path: String,
    /// `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied.
    pub change: &'static str,
    pub binary: bool,
    pub additions: u64,
    pub deletions: u64,
    /// The whole block from its `diff --git` line, verbatim.
    pub diff: String,
}

/// Splits `patch` into file blocks in patch order. Text before the first
/// `diff --git` line (if any) is ignored.
pub fn split(patch: &str) -> Vec<FileDiff> {
    let mut blocks: Vec<&str> = Vec::new();
    let mut start: Option<usize> = None;
    let mut offset = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(begin) = start {
                blocks.push(&patch[begin..offset]);
            }
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(begin) = start {
        blocks.push(&patch[begin..]);
    }
    blocks.into_iter().map(parse_block).collect()
}

fn parse_block(block: &str) -> FileDiff {
    let mut lines = block.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    let (header_old, header_new) = header_paths(trim_eol(first));
    let mut minus: Option<String> = None;
    let mut plus: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut copy = false;
    let mut added = false;
    let mut deleted = false;
    let mut binary = false;
    let mut in_hunks = false;
    let mut additions = 0;
    let mut deletions = 0;
    for raw in lines {
        let line = trim_eol(raw);
        if in_hunks {
            if line.starts_with('+') {
                additions += 1;
            } else if line.starts_with('-') {
                deletions += 1;
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunks = true;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            minus = strip_side(rest, "a/");
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            plus = strip_side(rest, "b/");
        } else if let Some(rest) = line.strip_prefix("rename from ") {
            rename_from = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            rename_to = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("copy from ") {
            copy = true;
            rename_from = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("copy to ") {
            copy = true;
            rename_to = Some(unquote(rest));
        } else if line.starts_with("new file mode") {
            added = true;
        } else if line.starts_with("deleted file mode") {
            deleted = true;
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            binary = true;
        }
    }
    let old_path = rename_from
        .or(minus)
        .or(header_old)
        .unwrap_or_default();
    let path = rename_to
        .or(plus)
        .or(header_new)
        .unwrap_or_else(|| old_path.clone());
    let path = if path.is_empty() { old_path.clone() } else { path };
    let change = if added {
        "A"
    } else if deleted {
        "D"
    } else if copy {
        "C"
    } else if path != old_path && !old_path.is_empty() {
        "R"
    } else {
        "M"
    };
    FileDiff {
        old_path: if old_path.is_empty() { path.clone() } else { old_path },
        path,
        change,
        binary,
        additions,
        deletions,
        diff: block.to_string(),
    }
}

fn trim_eol(line: &str) -> &str {
    line.strip_suffix('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .unwrap_or(line)
}

/// `--- a/x` / `+++ b/x`; `/dev/null` is no path.
fn strip_side(rest: &str, prefix: &str) -> Option<String> {
    let rest = rest.split('\t').next().unwrap_or(rest);
    if rest == "/dev/null" {
        return None;
    }
    let path = unquote(rest);
    Some(path.strip_prefix(prefix).map(str::to_string).unwrap_or(path))
}

/// The two paths of `diff --git a/X b/Y`. Unquoted paths may hold spaces, so
/// the split prefers the one where both sides name the same path (the
/// common, non-rename case); the `---`/`+++`/`rename` lines override it.
fn header_paths(line: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = line.strip_prefix("diff --git ") else {
        return (None, None);
    };
    if rest.starts_with('"') {
        let (old, tail) = take_quoted(rest);
        let tail = tail.trim_start();
        let new = if tail.starts_with('"') {
            take_quoted(tail).0
        } else {
            tail.to_string()
        };
        return (
            Some(old.strip_prefix("a/").unwrap_or(&old).to_string()),
            Some(new.strip_prefix("b/").unwrap_or(&new).to_string()),
        );
    }
    let Some(body) = rest.strip_prefix("a/") else {
        return (None, None);
    };
    let splits: Vec<usize> = body.match_indices(" b/").map(|(i, _)| i).collect();
    let pick = splits
        .iter()
        .copied()
        .find(|&i| body[..i] == body[i + 3..])
        .or_else(|| splits.first().copied());
    match pick {
        Some(i) => (Some(body[..i].to_string()), Some(body[i + 3..].to_string())),
        None => (Some(body.to_string()), None),
    }
}

/// A C-quoted git path (`"a/caf\303\251"`) to its text; unquoted as is.
fn unquote(text: &str) -> String {
    if text.starts_with('"') {
        take_quoted(text).0
    } else {
        text.to_string()
    }
}

/// Reads one C-quoted string at the start of `text`; returns it and the rest.
fn take_quoted(text: &str) -> (String, &str) {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                return (String::from_utf8_lossy(&out).into_owned(), &text[i + 1..]);
            }
            b'\\' if i + 1 < bytes.len() => {
                let next = bytes[i + 1];
                if next.is_ascii_digit()
                    && i + 3 < bytes.len()
                    && bytes[i + 2].is_ascii_digit()
                    && bytes[i + 3].is_ascii_digit()
                {
                    let octal = &text[i + 1..i + 4];
                    out.push(u8::from_str_radix(octal, 8).unwrap_or(b'?'));
                    i += 4;
                    continue;
                }
                out.push(match next {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    b'v' => 0x0b,
                    other => other,
                });
                i += 2;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    (String::from_utf8_lossy(&out).into_owned(), "")
}

/// `text` cut to at most `max` bytes at the last line boundary (or char
/// boundary, if one line alone is longer); `None` when it already fits.
pub fn bound(text: &str, max: usize) -> Option<&str> {
    if text.len() <= max {
        return None;
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &text[..cut];
    Some(match head.rfind('\n') {
        Some(newline) => &head[..=newline],
        None => head,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFY: &str = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn b() -> u8 { 1 }\n fn c() {}\n";
    const ADD: &str = "diff --git a/docs/new file.md b/docs/new file.md\nnew file mode 100644\nindex 0000000..3333333\n--- /dev/null\n+++ b/docs/new file.md\n@@ -0,0 +1,2 @@\n+# Title\n+--- not a header\n";
    const DELETE: &str = "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\nindex 4444444..0000000\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
    const RENAME: &str = "diff --git a/a/x.rs b/b/y.rs\nsimilarity index 90%\nrename from a/x.rs\nrename to b/y.rs\nindex 5555555..6666666 100644\n--- a/a/x.rs\n+++ b/b/y.rs\n@@ -1 +1 @@\n-x\n+y\n";
    const BINARY: &str = "diff --git a/logo.png b/logo.png\nindex 7777777..8888888 100644\nBinary files a/logo.png and b/logo.png differ\n";

    #[test]
    fn splits_one_entry_per_file_in_patch_order() {
        let patch = format!("{MODIFY}{ADD}{DELETE}{RENAME}{BINARY}");
        let files = split(&patch);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["src/lib.rs", "docs/new file.md", "old.txt", "b/y.rs", "logo.png"]
        );
        let joined: String = files.iter().map(|f| f.diff.as_str()).collect();
        assert_eq!(joined, patch, "blocks cover the patch exactly");
    }

    #[test]
    fn classifies_changes_and_counts_hunk_lines_only() {
        let files = split(&format!("{MODIFY}{ADD}{DELETE}{RENAME}{BINARY}"));
        let summary: Vec<(&str, &str, bool, u64, u64)> = files
            .iter()
            .map(|f| (f.old_path.as_str(), f.change, f.binary, f.additions, f.deletions))
            .collect();
        assert_eq!(
            summary,
            [
                ("src/lib.rs", "M", false, 1, 1),
                ("docs/new file.md", "A", false, 2, 0),
                ("old.txt", "D", false, 0, 1),
                ("a/x.rs", "R", false, 1, 1),
                ("logo.png", "M", true, 0, 0),
            ]
        );
    }

    #[test]
    fn a_pure_rename_without_hunks_takes_its_paths_from_the_rename_lines() {
        let patch = "diff --git a/one.rs b/two.rs\nsimilarity index 100%\nrename from one.rs\nrename to two.rs\n";
        let file = &split(patch)[0];
        assert_eq!((file.old_path.as_str(), file.path.as_str(), file.change), ("one.rs", "two.rs", "R"));
    }

    #[test]
    fn quoted_paths_are_unescaped() {
        let patch = "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\nindex 1..2 100644\n--- \"a/caf\\303\\251.txt\"\n+++ \"b/caf\\303\\251.txt\"\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(split(patch)[0].path, "café.txt");
    }

    #[test]
    fn text_before_the_first_file_and_an_empty_patch_yield_nothing_extra() {
        assert!(split("").is_empty());
        assert_eq!(split(&format!("warning: noise\n{MODIFY}")).len(), 1);
    }

    #[test]
    fn bound_cuts_at_a_line_boundary_and_never_inside_a_char() {
        assert_eq!(bound("ab\ncd\n", 10), None);
        assert_eq!(bound("ab\ncd\nef\n", 7), Some("ab\ncd\n"));
        assert_eq!(bound("ééé", 3), Some("é"));
    }
}
