//! The review-changes workflow's content: lenses, schemas, prompts, parsers and the result
//! shapes. A port of `workflows/review_changes.rhai` (code_review 2.0); the result objects
//! keep that script's `complete()` shapes verbatim.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub struct Lens {
    pub id: &'static str,
    pub instructions: &'static str,
}

/// The four review lenses; each scanner reviews every changed path through one of them.
pub const LENSES: [Lens; 4] = [
    Lens {
        id: "correctness",
        instructions: "Review functional behavior, state transitions, compatibility, and tests.",
    },
    Lens {
        id: "architecture-reuse",
        instructions: "Review duplication, ownership boundaries, and reuse of existing abstractions.",
    },
    Lens {
        id: "security-concurrency",
        instructions: "Review authorization, identity, filesystem safety, concurrency, and recovery.",
    },
    Lens {
        id: "workflow-invariants",
        instructions: "Review repository-specific workflow, evidence, workspace, and integration invariants.",
    },
];
pub const MAX_CANDIDATES: usize = 3;
pub const MIN_CONFIDENCE: i64 = 80;
pub const MAX_VERIFY: usize = 16;
/// Recon, four scans, up to MAX_VERIFY verifiers and triage.
pub const MIN_BUDGET: u64 = 22;

// Scanners and verifiers read git objects by SHA, so a working-tree edit during the run cannot change what they see.
pub const READ_RULES: &str = "Read the change only with `git diff --find-renames=50% --unified=12 <B> <H> -- <path>` and source only with `git show <H>:<path>` or `git grep -n <pattern> <H>`. Never read working-tree files: they may change while you review.";

pub const RECON_TARGET: &str = "code-review-recon";
pub const SCAN_TARGET: &str = "code-review-scan";
pub const VERIFY_TARGET: &str = "code-review-verify";
pub const TRIAGE_TARGET: &str = "code-review-triage";

fn str_schema() -> Value {
    json!({ "type": "string" })
}

pub fn recon_schema() -> Value {
    let s = str_schema();
    json!({
        "type": "object",
        "required": ["base_sha", "head_sha", "base_tree", "head_tree", "name_only", "stat"],
        "properties": { "base_sha": s, "head_sha": s, "base_tree": s, "head_tree": s, "name_only": s, "stat": s },
    })
}

pub fn scan_schema() -> Value {
    let s = str_schema();
    let candidate = json!({
        "type": "object",
        "required": ["slug", "title", "path", "line", "severity", "excerpt", "impact", "confidence"],
        "properties": {
            "slug": s, "title": s, "path": s, "excerpt": s, "impact": s,
            "line": { "type": "integer" },
            "severity": { "type": "string", "enum": ["Critical", "Major", "Cleanup"] },
            "confidence": { "type": "integer" },
        },
    });
    json!({
        "type": "object",
        "required": ["blocked", "blocker", "candidates"],
        "properties": {
            "blocker": s,
            "blocked": { "type": "boolean" },
            "candidates": { "type": "array", "items": candidate },
        },
    })
}

pub fn verify_schema() -> Value {
    let s = str_schema();
    let verdict = json!({
        "type": "object",
        "required": ["finding_id", "verdict", "confidence", "evidence", "reasoning"],
        "properties": {
            "finding_id": s, "evidence": s, "reasoning": s,
            "verdict": { "type": "string", "enum": ["confirmed", "refuted"] },
            "confidence": { "type": "integer" },
        },
    });
    json!({
        "type": "object",
        "required": ["base_tree", "head_tree", "verdicts"],
        "properties": { "base_tree": s, "head_tree": s, "verdicts": { "type": "array", "items": verdict }, "builds_not_run": { "type": "boolean" } },
    })
}

pub fn triage_schema() -> Value {
    let s = str_schema();
    json!({
        "type": "object",
        "required": ["report", "merge_recommendation"],
        "properties": { "report": s, "merge_recommendation": s },
    })
}

/// The call's arguments with their defaults.
pub struct Args {
    pub base: String,
    pub head: String,
    pub repository: String,
    pub focus: String,
}

impl Args {
    pub fn from(args: Option<&Value>) -> Args {
        let get = |key: &str, fallback: &str| -> String {
            args.and_then(|a| a.get(key))
                .filter(|v| !v.is_null())
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| fallback.to_owned())
        };
        Args {
            base: get("base", "origin/main"),
            head: get("head", "HEAD"),
            repository: get("repository", "."),
            focus: get(
                "focus",
                "Review the diff for material correctness, safety, durability, and maintainability defects.",
            ),
        }
    }
}

/// The pinned change: both commits and both trees.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub base_sha: String,
    pub head_sha: String,
    pub base_tree: String,
    pub head_tree: String,
}

impl Pin {
    pub fn from_recon(o: &Value) -> Pin {
        let s = |k: &str| o[k].as_str().unwrap_or_default().to_owned();
        Pin {
            base_sha: s("base_sha"),
            head_sha: s("head_sha"),
            base_tree: s("base_tree"),
            head_tree: s("head_tree"),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "base_sha": self.base_sha,
            "head_sha": self.head_sha,
            "base_tree": self.base_tree,
            "head_tree": self.head_tree,
        })
    }
}

/// `v` as compact JSON text.
pub fn json_encode(v: &Value) -> String {
    v.to_string()
}

/// The first 16 hex digits of the sha256 of `text`.
pub fn fingerprint(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A git object id: 40 (sha1) or 64 (sha256) lowercase hex digits.
pub fn is_hex_id(v: &Value) -> bool {
    let Some(s) = v.as_str() else {
        return false;
    };
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The file count in the summary line of `git diff --stat`: 0 for an empty stat, -1 when it
/// has no summary line.
pub fn stat_files(stat: &str) -> i64 {
    let last = stat.lines().filter(|l| !l.is_empty()).last().unwrap_or("");
    if last.is_empty() {
        return 0;
    }
    if last.contains('|') || !last.contains(" changed") {
        return -1;
    }
    last.split(' ')
        .find(|w| !w.is_empty())
        .and_then(|w| w.parse::<i64>().ok())
        .unwrap_or(-1)
}

/// Why recon's parsed output cannot be trusted, or "" when it can.
pub fn recon_problem(o: &Value) -> String {
    if !is_hex_id(&o["base_sha"]) || !is_hex_id(&o["head_sha"]) || !is_hex_id(&o["base_tree"]) || !is_hex_id(&o["head_tree"]) {
        return "recon returned a commit or tree id that is not 40 or 64 lowercase hex digits".into();
    }
    let name_only = o["name_only"].as_str().unwrap_or_default();
    let mut named = 0;
    for line in name_only.split('\n') {
        if line.is_empty() {
            continue;
        }
        if line.contains(" | ") || line.starts_with("fatal:") {
            return "recon's name_only listing holds stat or error text".into();
        }
        named += 1;
    }
    if named != stat_files(o["stat"].as_str().unwrap_or_default()) {
        return "recon's name_only and stat listings are cut off, hold error text or disagree on the file count".into();
    }
    String::new()
}

/// Why a scan's parsed output is a limitation instead of a clean lens, or "" when usable.
pub fn scan_problem(o: &Value) -> String {
    if o["blocked"].as_bool().unwrap_or(false) {
        return format!("the scan was blocked: {}", o["blocker"].as_str().unwrap_or_default());
    }
    String::new()
}

/// Why a verify's parsed output proves nothing about its group, or "" when usable.
pub fn verify_problem(o: &Value, pin: &Pin) -> String {
    if o["base_tree"] != pin.base_tree || o["head_tree"] != pin.head_tree {
        return "the verification resolved different trees than the pin".into();
    }
    String::new()
}

/// A git-quoted path ("caf\303\251.txt") as plain text; an unquoted path is returned as is.
pub fn unquote(s: &str) -> String {
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return s.to_owned();
    }
    let body = s[1..s.len() - 1].as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        if c == b'\\' && i + 1 < body.len() {
            let d = body[i + 1];
            if (b'0'..=b'3').contains(&d) && i + 3 < body.len() {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&body[i + 1..i + 4]).unwrap_or("x"), 8) {
                    out.push(v);
                    i += 4;
                    continue;
                }
            }
            out.push(match d {
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                other => other,
            });
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every path named by `git diff --name-only` output, one per line, quoted paths decoded.
pub fn changed_paths(name_only: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in name_only.split('\n') {
        let p = unquote(line);
        if !p.is_empty() && !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

pub fn recon_prompt(base: &str, head: &str) -> String {
    let mut t = String::from("Pin the git change to review. Use only read-only git commands, one at a time, in the repository root. Do not read or edit working-tree files.\n\n");
    t += &format!("Base ref: {}\nHead ref: {}\n\n", json_encode(&json!(base)), json_encode(&json!(head)));
    t += "1. Run `git rev-parse --verify --end-of-options <ref>^{commit}` for the base ref and for the head ref. These are base_sha and head_sha.\n";
    t += "2. Run `git rev-parse <sha>^{tree}` for both. These are base_tree and head_tree.\n";
    t += "3. Run `git diff --name-only --find-renames=50% <base_sha> <head_sha> --`. This is name_only.\n";
    t += "4. Run `git diff --stat --find-renames=50% <base_sha> <head_sha> --`. This is stat.\n\n";
    t += "Return the ids and the exact command outputs, unedited and untruncated; inside a JSON string write each line break as \\n. If a command fails, return its error text in the matching field.";
    t
}

pub fn scan_prompt(lens: &Lens, pin: &Pin, focus: &str, paths: &[String]) -> String {
    let mut t = format!("Review this change through the lens `{}`. {}\n", lens.id, lens.instructions);
    t += &format!("Operator focus: {}\n\n", json_encode(&json!(focus)));
    t += &format!("In the rules below, <B> is {} and <H> is {}.\n", pin.base_sha, pin.head_sha);
    t += READ_RULES;
    t += "\n\n";
    t += &format!("Changed paths (JSON): {}\n\n", json_encode(&json!(paths)));
    t += "Review progressively. Incomplete, truncated, or inconsistent evidence is a blocker, not a completed scan: set blocked to true and describe it in blocker. ";
    t += &format!("Report at most {MAX_CANDIDATES} distinct actionable defects introduced by the change as candidates, or none. ");
    t += &format!("Each candidate needs a short slug and title, an exact changed path and line, a supporting excerpt copied verbatim from `git show <H>:<path>` without diff markers, the concrete impact, and a confidence of at least {MIN_CONFIDENCE}. ");
    t += "Severity: Critical for security or data loss, Major for incorrect behavior or liveness, Cleanup for concrete duplication or unnecessary complexity.";
    t
}

/// Candidates that share one `path:line`.
pub struct Group {
    pub key: String,
    pub candidates: Vec<Value>,
}

pub fn verify_prompt(group: &Group, pin: &Pin) -> String {
    let mut t = format!("Independently verify the review candidates reported at {}. ", json_encode(&json!(group.key)));
    t += "Try to refute each claim by examining its context, callers, tests, and existing abstractions. Confirm only actionable defects introduced by the change, and distinguish uncertainty from evidence.\n\n";
    t += &format!("In the rules below, <B> is {} and <H> is {}.\n", pin.base_sha, pin.head_sha);
    t += READ_RULES;
    t += " First run `git rev-parse <B>^{tree}` and `git rev-parse <H>^{tree}` and return both as base_tree and head_tree.\n\n";
    t += "You cannot build or run tests here. Verify by reading the pinned code only and set builds_not_run to true.\n\n";
    t += &format!("Candidates (JSON): {}\n\n", json_encode(&Value::Array(group.candidates.clone())));
    t += "Return one verdict per candidate, keyed by its finding_id: `confirmed` or `refuted`, a confidence from 0 through 100, the evidence you observed, and your reasoning. ";
    t += &format!("Confirm only with a confidence of at least {MIN_CONFIDENCE} and non-empty evidence.");
    t
}

pub fn counts_text(candidate: usize, confirmed: usize, refuted: usize, unverified: usize, high_priority: usize) -> String {
    format!("{candidate} candidates: {confirmed} confirmed, {refuted} refuted, {unverified} unverified; {high_priority} confirmed Critical or Major.")
}

pub fn triage_prompt(pin: &Pin, counts: &str, confirmed: &[Value], limitations: &[String]) -> String {
    let mut t = format!("Write the operator-facing review report for the change {}..{}.\n", pin.base_sha, pin.head_sha);
    t += &format!("Ledger: {counts} Use these counts as given.\n");
    t += &format!("Confirmed findings (JSON): {}\n", json_encode(&Value::Array(confirmed.to_vec())));
    t += &format!("Review limitations (JSON): {}\n\n", json_encode(&json!(limitations)));
    t += "Summarize the confirmed defects by severity and practical impact, report zero findings when there are none, disclose every limitation, and give a supported merge recommendation. The report does not authorize merging.";
    t
}

pub fn render_report(pin: &Pin, counts: &str, recommendation: &str, report: &str, confirmed: &[Value], limitations: &[String]) -> String {
    let mut t = format!(
        "# Review\n\n{}..{}\n\n{counts}\n\nMerge recommendation: {recommendation}\n\n{report}\n",
        pin.base_sha, pin.head_sha
    );
    if !confirmed.is_empty() {
        t += "\n## Confirmed findings\n";
        for f in confirmed {
            t += &format!(
                "\n- [{}] {}:{} {}: {}\n",
                f["severity"].as_str().unwrap_or_default(),
                f["path"].as_str().unwrap_or_default(),
                f["line"],
                f["title"].as_str().unwrap_or_default(),
                f["impact"].as_str().unwrap_or_default()
            );
        }
    }
    if !limitations.is_empty() {
        t += "\n## Limitations\n";
        for l in limitations {
            t += &format!("\n- {l}\n");
        }
    }
    t
}

pub fn empty_counts() -> Value {
    json!({ "candidate": 0, "confirmed": 0, "refuted": 0, "unverified": 0, "high_priority": 0 })
}

/// The `blocked` result and its report text.
pub fn blocked(reason: &str, path: &str) -> (Value, String) {
    let result = json!({
        "status": "blocked",
        "reason": reason,
        "counts": empty_counts(),
        "findings": [],
        "report": { "report": format!("Blocked: {reason}"), "merge_recommendation": "none" },
        "limitations": [reason],
        "path": path,
    });
    (result, format!("# Review\n\nBlocked: {reason}\n"))
}

/// The scan slot of a lens.
pub fn scan_slot(lens: &Lens) -> String {
    format!("scan-{}", lens.id)
}

/// The verify slot of group `i`.
pub fn verify_slot(i: usize) -> String {
    format!("verify-{i}")
}

/// The candidates every usable scan kept, in lens order, and the lens limitations.
/// `scans` holds each lens's judged output: `Ok(parsed)` or `Err(why)`.
pub fn select_candidates(scans: &[Result<Value, String>], paths: &[String]) -> (Vec<Value>, Vec<String>) {
    let mut candidates = Vec::new();
    let mut limitations = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (lens, scan) in LENSES.iter().zip(scans) {
        let output = match scan {
            Err(why) => {
                limitations.push(format!("{}: {why}", lens.id));
                continue;
            }
            Ok(o) => o,
        };
        let why = scan_problem(output);
        if !why.is_empty() {
            limitations.push(format!("{}: {why}", lens.id));
            continue;
        }
        let mut kept = 0;
        for c in output["candidates"].as_array().into_iter().flatten() {
            let id = format!("{}:{}", lens.id, c["slug"].as_str().unwrap_or_default());
            let path = c["path"].as_str().unwrap_or_default();
            let confidence = c["confidence"].as_i64().unwrap_or(0);
            let drop = !paths.iter().any(|p| p == path)
                || confidence < MIN_CONFIDENCE
                || kept >= MAX_CANDIDATES
                || ids.contains(&id);
            if drop {
                continue;
            }
            ids.push(id.clone());
            kept += 1;
            let mut finding = c.as_object().cloned().unwrap_or_default();
            finding.insert("finding_id".into(), json!(id));
            finding.insert("lens".into(), json!(lens.id));
            candidates.push(Value::Object(finding));
        }
    }
    (candidates, limitations)
}

/// Candidates grouped by `path:line`, in first-seen order.
pub fn group_candidates(candidates: &[Value]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for f in candidates {
        let key = format!("{}:{}", f["path"].as_str().unwrap_or_default(), f["line"]);
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => g.candidates.push(f.clone()),
            None => groups.push(Group { key, candidates: vec![f.clone()] }),
        }
    }
    groups
}

/// What verification concluded over every group.
pub struct Verified {
    pub confirmed: Vec<Value>,
    pub unverified: Vec<Value>,
    pub refuted: usize,
    pub high_priority: usize,
    pub limitations: Vec<String>,
}

/// `verdicts[i]` is group `i`'s judged verify output (`Ok(parsed)` or `Err(why)`); groups past
/// `verdicts.len()` were never sent. Confirmed only by verdict, confidence, evidence and the
/// pinned trees.
pub fn verify_groups(groups: &[Group], verdicts: &[Result<Value, String>], pin: &Pin) -> Verified {
    let mut v = Verified {
        confirmed: Vec::new(),
        unverified: Vec::new(),
        refuted: 0,
        high_priority: 0,
        limitations: Vec::new(),
    };
    for (gi, group) in groups.iter().enumerate() {
        let slot = verdicts.get(gi);
        let why = match slot {
            None => format!("beyond the first {MAX_VERIFY} groups"),
            Some(Err(why)) => why.clone(),
            Some(Ok(o)) => verify_problem(o, pin),
        };
        let output = if why.is_empty() { slot.and_then(|s| s.as_ref().ok()) } else { None };
        if let Some(o) = output {
            if o["builds_not_run"].as_bool().unwrap_or(false) {
                v.limitations.push(format!("verify-{gi}: builds not run"));
            }
        }
        for f in &group.candidates {
            let id = f["finding_id"].as_str().unwrap_or_default();
            let verdict = output.and_then(|o| {
                o["verdicts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| x["finding_id"] == id)
                    .cloned()
            });
            let Some(x) = verdict else {
                v.unverified.push(f.clone());
                continue;
            };
            let evidence = x["evidence"].as_str().unwrap_or_default().trim().to_owned();
            let confidence = x["confidence"].as_i64().unwrap_or(0);
            if x["verdict"] == "confirmed" && confidence >= MIN_CONFIDENCE && !evidence.is_empty() {
                let mut finding: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
                finding.insert("confidence".into(), json!(confidence));
                finding.insert("evidence".into(), json!(evidence));
                finding.insert("reasoning".into(), x["reasoning"].clone());
                finding.insert("verdict".into(), json!("confirmed"));
                finding.insert("verification".into(), x["reasoning"].clone());
                finding.insert("detail".into(), f["impact"].clone());
                finding.insert("area_id".into(), f["lens"].clone());
                if f["severity"] == "Critical" || f["severity"] == "Major" {
                    v.high_priority += 1;
                }
                v.confirmed.push(Value::Object(finding));
            } else if x["verdict"] == "refuted" {
                v.refuted += 1;
            } else {
                v.unverified.push(f.clone());
            }
        }
    }
    if !v.unverified.is_empty() {
        v.limitations.push(format!("unverified candidates: {}", v.unverified.len()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_read_by_sha_and_verifiers_do_not_build() {
        let pin = Pin { base_sha: "b".repeat(40), head_sha: "h".repeat(40), base_tree: "t".repeat(40), head_tree: "u".repeat(40) };
        let paths = vec!["src/lib.rs".to_owned()];
        for lens in &LENSES {
            let p = scan_prompt(lens, &pin, "focus", &paths);
            assert!(p.contains("git show <H>:") && p.contains("Never read working-tree files: they may change while you review."));
            assert!(!p.contains("HEAD") && !p.contains("^{tree}"), "scans resolve no trees and see no bare HEAD");
        }
        let group = Group { key: "src/lib.rs:3".into(), candidates: vec![json!({"finding_id": "correctness:x"})] };
        let v = verify_prompt(&group, &pin);
        assert!(v.contains("cannot build or run tests") && v.contains("builds_not_run") && v.contains("git show <H>:"));
        assert!(!v.contains("git archive") && !v.contains("$TMPDIR"));
        let r = recon_prompt("origin/main", "HEAD");
        assert!(r.contains("git diff --name-only --find-renames=50% ") && r.contains("git diff --stat --find-renames=50% "));
        assert!(!r.contains("--name-status") && !r.contains("--raw"), "recon listings must be tab-free");
    }

    #[test]
    fn hex_ids() {
        assert!(is_hex_id(&json!("a".repeat(40))));
        assert!(is_hex_id(&json!("0123456789abcdef".repeat(4))));
        assert!(!is_hex_id(&json!("A".repeat(40))));
        assert!(!is_hex_id(&json!("abc")));
        assert!(!is_hex_id(&json!(42)));
    }

    #[test]
    fn stat_summary_line() {
        assert_eq!(stat_files(""), 0);
        assert_eq!(stat_files(" src/lib.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n"), 1);
        assert_eq!(stat_files(" src/lib.rs | 2 +-\n"), -1);
        assert_eq!(stat_files("fatal: bad revision"), -1);
    }

    #[test]
    fn unquotes_git_paths() {
        assert_eq!(unquote("plain.txt"), "plain.txt");
        assert_eq!(unquote("\"caf\\303\\251.txt\""), "café.txt");
        assert_eq!(unquote("\"a\\tb\\\"c\""), "a\tb\"c");
        assert_eq!(changed_paths("a\n\"b c\"\na\n"), vec!["a", "b c"]);
    }

    #[test]
    fn recon_problems() {
        let sha = "a".repeat(40);
        let ok = json!({"base_sha": sha, "head_sha": sha, "base_tree": sha, "head_tree": sha,
            "name_only": "src/lib.rs\n", "stat": " src/lib.rs | 1 +\n 1 file changed, 1 insertion(+)\n"});
        assert_eq!(recon_problem(&ok), "");
        let mut bad = ok.clone();
        bad["stat"] = json!(" 2 files changed, 2 insertions(+)\n");
        assert!(recon_problem(&bad).contains("disagree"));
        bad = ok.clone();
        bad["name_only"] = json!("fatal: bad object\n");
        assert!(recon_problem(&bad).contains("error text"));
        bad = ok.clone();
        bad["head_sha"] = json!("HEAD");
        assert!(recon_problem(&bad).contains("hex"));
    }

    #[test]
    fn candidates_are_filtered_and_grouped() {
        let paths = vec!["src/lib.rs".to_owned()];
        let c = |slug: &str, path: &str, line: i64, confidence: i64| {
            json!({"slug": slug, "title": "t", "path": path, "line": line, "severity": "Major",
                   "excerpt": "e", "impact": "i", "confidence": confidence})
        };
        let scan = |cands: Vec<Value>| Ok(json!({"blocked": false, "blocker": "", "candidates": cands}));
        let scans = vec![
            scan(vec![c("a", "src/lib.rs", 3, 90), c("b", "other.rs", 3, 90), c("c", "src/lib.rs", 4, 50)]),
            Err("the scan did not succeed".to_owned()),
            scan(vec![c("a", "src/lib.rs", 3, 85)]),
            scan(vec![]),
        ];
        let (candidates, limitations) = select_candidates(&scans, &paths);
        assert_eq!(candidates.len(), 2);
        assert_eq!(limitations, vec!["architecture-reuse: the scan did not succeed"]);
        let groups = group_candidates(&candidates);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "src/lib.rs:3");
        assert_eq!(groups[0].candidates.len(), 2);
    }

    #[test]
    fn verdicts_confirm_only_with_evidence_and_matching_trees() {
        let pin = Pin { base_sha: "b".into(), head_sha: "h".into(), base_tree: "bt".into(), head_tree: "ht".into() };
        let f = |id: &str| json!({"finding_id": id, "path": "p", "line": 1, "severity": "Critical", "impact": "i", "lens": "correctness"});
        let groups = vec![Group { key: "p:1".into(), candidates: vec![f("correctness:a"), f("correctness:b"), f("correctness:c")] }, Group { key: "p:2".into(), candidates: vec![f("correctness:d")] }];
        let verdicts = vec![Ok(json!({"base_tree": "bt", "head_tree": "ht", "builds_not_run": true, "verdicts": [
            {"finding_id": "correctness:a", "verdict": "confirmed", "confidence": 95, "evidence": " seen ", "reasoning": "r"},
            {"finding_id": "correctness:b", "verdict": "refuted", "confidence": 90, "evidence": "x", "reasoning": "r"},
            {"finding_id": "correctness:c", "verdict": "confirmed", "confidence": 50, "evidence": "x", "reasoning": "r"},
        ]}))];
        let v = verify_groups(&groups, &verdicts, &pin);
        assert_eq!(v.confirmed.len(), 1);
        assert_eq!(v.confirmed[0]["evidence"], "seen");
        assert_eq!(v.high_priority, 1);
        assert_eq!(v.refuted, 1);
        assert_eq!(v.unverified.len(), 2);
        assert_eq!(v.limitations, vec!["verify-0: builds not run", "unverified candidates: 2"]);
    }
}
