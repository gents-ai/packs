let meta = #{
    name: "review-changes",
    description: "Review a git diff through four lenses, adversarially verify every candidate, and triage",
    when_to_use: "Before merging a branch or commit range. args: {base, head, repository, focus}",
    phases: [#{ title: "Recon" }, #{ title: "Scan" }, #{ title: "Verify" }, #{ title: "Triage" }],
};

// The four review lenses; each scanner reviews every changed path through one of them.
const LENSES = [
    #{ id: "correctness", instructions: "Review functional behavior, state transitions, compatibility, and tests." },
    #{ id: "architecture-reuse", instructions: "Review duplication, ownership boundaries, and reuse of existing abstractions." },
    #{ id: "security-concurrency", instructions: "Review authorization, identity, filesystem safety, concurrency, and recovery." },
    #{ id: "workflow-invariants", instructions: "Review repository-specific workflow, evidence, workspace, and integration invariants." },
];
const MAX_CANDIDATES = 3;
const MIN_CONFIDENCE = 80;
const MAX_VERIFY = 16;

// Scanners and verifiers read git objects by SHA, so a working-tree edit during the run cannot change what they see.
const READ_RULES = "Read the change only with `git diff --find-renames=50% --unified=12 <B> <H> -- <path>` and source only with `git show <H>:<path>` or `git grep -n <pattern> <H>`. Never read working-tree files: they may change while you review.";

const STR = #{ type: "string" };
const RECON = #{
    type: "object",
    required: ["base_sha", "head_sha", "base_tree", "head_tree", "name_only", "stat"],
    properties: #{ base_sha: STR, head_sha: STR, base_tree: STR, head_tree: STR, name_only: STR, stat: STR },
};

const CANDIDATE = #{
    type: "object",
    required: ["slug", "title", "path", "line", "severity", "excerpt", "impact", "confidence"],
    properties: #{
        slug: STR, title: STR, path: STR, excerpt: STR, impact: STR,
        line: #{ type: "integer" },
        severity: #{ type: "string", enum: ["Critical", "Major", "Cleanup"] },
        confidence: #{ type: "integer" },
    },
};
const SCAN = #{
    type: "object",
    required: ["blocked", "blocker", "candidates"],
    properties: #{
        blocker: STR,
        blocked: #{ type: "boolean" },
        candidates: #{ type: "array", items: CANDIDATE },
    },
};

const VERDICT = #{
    type: "object",
    required: ["finding_id", "verdict", "confidence", "evidence", "reasoning"],
    properties: #{
        finding_id: STR, evidence: STR, reasoning: STR,
        verdict: #{ type: "string", enum: ["confirmed", "refuted"] },
        confidence: #{ type: "integer" },
    },
};
const VERIFY = #{
    type: "object",
    required: ["base_tree", "head_tree", "verdicts"],
    properties: #{ base_tree: STR, head_tree: STR, verdicts: #{ type: "array", items: VERDICT }, builds_not_run: #{ type: "boolean" } },
};
const TRIAGE = #{
    type: "object",
    required: ["report", "merge_recommendation"],
    properties: #{ report: STR, merge_recommendation: STR },
};

// An args field, or its default when args is missing, not a map, or lacks the field.
fn arg_or(a, key, fallback) {
    if type_of(a) == "map" && key in a && a[key] != () { a[key] } else { fallback }
}

// A git object id: 40 (sha1) or 64 (sha256) lowercase hex digits.
fn is_hex_id(s) {
    if type_of(s) != "string" || (s.len() != 40 && s.len() != 64) { return false; }
    for i in 0..s.len() {
        if !"0123456789abcdef".contains(s.sub_string(i, 1)) { return false; }
    }
    true
}

// The file count in the summary line of `git diff --stat`: 0 for an empty stat, -1 when it has no summary line.
fn stat_files(stat) {
    let last = "";
    for line in stat.split("\n") { if line != "" { last = line; } }
    if last == "" { return 0; }
    if last.contains("|") || !last.contains(" changed") { return -1; }
    let words = [];
    for w in last.split(" ") { if w != "" { words.push(w); } }
    try { parse_int(words[0]) } catch { -1 }
}

// Why recon cannot be trusted, or "" when it can.
fn recon_problem(r) {
    if r == () || !r.success { return "recon did not succeed"; }
    let o = r.output;
    if !is_hex_id(o.base_sha) || !is_hex_id(o.head_sha) || !is_hex_id(o.base_tree) || !is_hex_id(o.head_tree) {
        return "recon returned a commit or tree id that is not 40 or 64 lowercase hex digits";
    }
    let named = 0;
    for line in o.name_only.split("\n") {
        if line != "" { if line.contains(" | ") || line.starts_with("fatal:") { return "recon's name_only listing holds stat or error text"; } named += 1; }
    }
    if named != stat_files(o.stat) {
        return "recon's name_only and stat listings are cut off, hold error text or disagree on the file count";
    }
    ""
}

// Why a scan slot is a limitation instead of a clean lens, or "" when it is usable.
fn scan_problem(s) {
    if s == () || !s.success { return "the scan did not succeed"; }
    let o = s.output;
    if o.blocked { return "the scan was blocked: " + o.blocker; }
    ""
}

fn escaped(d) { switch d { "t" => "\t", "n" => "\n", "r" => "\r", _ => d } }

fn octal(body, i) { parse_int(body.sub_string(i + 1, 3), 8) }

// A git-quoted path ("caf\303\251.txt") as plain text; an unquoted path is returned as is.
fn unquote(s) {
    if s.len() < 2 || !s.starts_with("\"") || !s.ends_with("\"") { return s; }
    let body = s.sub_string(1, s.len() - 2);
    let out = "";
    let run = blob();
    let i = 0;
    while i < body.len() {
        let c = body.sub_string(i, 1);
        let d = body.sub_string(i + 1, 1);
        if c == "\\" && d != "" && "0123".contains(d) {
            run.push(octal(body, i));
            i += 4;
            continue;
        }
        out += run.as_string();
        run = blob();
        out += if c == "\\" { escaped(d) } else { c };
        i += if c == "\\" { 2 } else { 1 };
    }
    out + run.as_string()
}

// Every path named by `git diff --name-only` output, one per line, quoted paths decoded.
fn changed_paths(name_only) {
    let paths = [];
    for line in name_only.split("\n") {
        let p = unquote(line);
        if p != "" && !paths.contains(p) { paths.push(p); }
    }
    paths
}

fn recon_prompt(base, head) {
    let t = "Pin the git change to review. Use only read-only git commands, one at a time, in the repository root. Do not read or edit working-tree files.\n\n";
    t += "Base ref: " + json_encode(base) + "\nHead ref: " + json_encode(head) + "\n\n";
    t += "1. Run `git rev-parse --verify --end-of-options <ref>^{commit}` for the base ref and for the head ref. These are base_sha and head_sha.\n";
    t += "2. Run `git rev-parse <sha>^{tree}` for both. These are base_tree and head_tree.\n";
    t += "3. Run `git diff --name-only --find-renames=50% <base_sha> <head_sha> --`. This is name_only.\n";
    t += "4. Run `git diff --stat --find-renames=50% <base_sha> <head_sha> --`. This is stat.\n\n";
    t += "Return the ids and the exact command outputs, unedited and untruncated; inside a JSON string write each line break as \\n. If a command fails, return its error text in the matching field.";
    t
}

fn scan_prompt(lens, pin, focus, paths, rules, max_candidates, min_confidence) {
    let t = "Review this change through the lens `" + lens.id + "`. " + lens.instructions + "\n";
    t += "Operator focus: " + json_encode(focus) + "\n\n";
    t += "In the rules below, <B> is " + pin.base_sha + " and <H> is " + pin.head_sha + ".\n";
    t += rules + "\n\n";
    t += "Changed paths (JSON): " + json_encode(paths) + "\n\n";
    t += "Review progressively. Incomplete, truncated, or inconsistent evidence is a blocker, not a completed scan: set blocked to true and describe it in blocker. ";
    t += "Report at most " + max_candidates + " distinct actionable defects introduced by the change as candidates, or none. ";
    t += "Each candidate needs a short slug and title, an exact changed path and line, a supporting excerpt copied verbatim from `git show <H>:<path>` without diff markers, the concrete impact, and a confidence of at least " + min_confidence + ". ";
    t += "Severity: Critical for security or data loss, Major for incorrect behavior or liveness, Cleanup for concrete duplication or unnecessary complexity.";
    t
}

// Why a verify slot proves nothing about its group, or "" when it is usable.
fn verify_problem(v, pin) {
    if v == () || !v.success { return "the verification did not succeed"; }
    if v.output.base_tree != pin.base_tree || v.output.head_tree != pin.head_tree {
        return "the verification resolved different trees than the pin";
    }
    ""
}

// The verdict a usable verify slot returned for one finding, or ().
fn verdict_for(slot, id) {
    for x in slot.output.verdicts {
        if x.finding_id == id { return x; }
    }
    ()
}

fn verify_prompt(group, pin, rules, min_confidence) {
    let t = "Independently verify the review candidates reported at " + json_encode(group.key) + ". ";
    t += "Try to refute each claim by examining its context, callers, tests, and existing abstractions. Confirm only actionable defects introduced by the change, and distinguish uncertainty from evidence.\n\n";
    t += "In the rules below, <B> is " + pin.base_sha + " and <H> is " + pin.head_sha + ".\n";
    t += rules + " First run `git rev-parse <B>^{tree}` and `git rev-parse <H>^{tree}` and return both as base_tree and head_tree.\n\n";
    t += "You cannot build or run tests here. Verify by reading the pinned code only and set builds_not_run to true.\n\n";
    t += "Candidates (JSON): " + json_encode(group.candidates) + "\n\n";
    t += "Return one verdict per candidate, keyed by its finding_id: `confirmed` or `refuted`, a confidence from 0 through 100, the evidence you observed, and your reasoning. ";
    t += "Confirm only with a confidence of at least " + min_confidence + " and non-empty evidence.";
    t
}

fn counts_text(candidate, confirmed, refuted, unverified, high_priority) {
    "" + candidate + " candidates: " + confirmed + " confirmed, " + refuted + " refuted, " + unverified + " unverified; " + high_priority + " confirmed Critical or Major."
}

fn triage_prompt(pin, counts, confirmed, limitations) {
    let t = "Write the operator-facing review report for the change " + pin.base_sha + ".." + pin.head_sha + ".\n";
    t += "Ledger: " + counts + " Use these counts as given.\n";
    t += "Confirmed findings (JSON): " + json_encode(confirmed) + "\n";
    t += "Review limitations (JSON): " + json_encode(limitations) + "\n\n";
    t += "Summarize the confirmed defects by severity and practical impact, report zero findings when there are none, disclose every limitation, and give a supported merge recommendation. The report does not authorize merging.";
    t
}

fn render_report(pin, counts, recommendation, report, confirmed, limitations) {
    let t = "# Review\n\n" + pin.base_sha + ".." + pin.head_sha + "\n\n" + counts + "\n\nMerge recommendation: " + recommendation + "\n\n" + report + "\n";
    if confirmed.len() > 0 {
        t += "\n## Confirmed findings\n";
        for f in confirmed {
            t += "\n- [" + f.severity + "] " + f.path + ":" + f.line + " " + f.title + ": " + f.impact + "\n";
        }
    }
    if limitations.len() > 0 {
        t += "\n## Limitations\n";
        for l in limitations { t += "\n- " + l + "\n"; }
    }
    t
}

fn empty_counts() { #{ candidate: 0, confirmed: 0, refuted: 0, unverified: 0, high_priority: 0 } }

// Step 0: a resumed run with --budget 22 or more passes this gate.
if budget().total < 22 { pause("budget_exceeded", "review-changes needs agent_budget >= 22"); }

let base = arg_or(args, "base", "origin/main");
let head = arg_or(args, "head", "HEAD");
let repo = arg_or(args, "repository", ".");
let focus = arg_or(args, "focus", "Review the diff for material correctness, safety, durability, and maintainability defects.");

phase("Recon");
let recon = agent(recon_prompt(base, head), #{
    agent_type: "code-review-recon",
    cwd: repo,
    label: "recon",
    output_schema: RECON,
});
let problem = recon_problem(recon);
if problem != "" {
    let blocked_report = write_scratch_file("review.md", "# Review\n\nBlocked: " + problem + "\n");
    complete(#{ status: "blocked", reason: problem, counts: empty_counts(), findings: [], report: #{ report: "Blocked: " + problem, merge_recommendation: "none" }, limitations: [problem], path: blocked_report });
}

let PIN = #{
    base_sha: recon.output.base_sha,
    head_sha: recon.output.head_sha,
    base_tree: recon.output.base_tree,
    head_tree: recon.output.head_tree,
};
let diff_id = fingerprint(json_encode([PIN, recon.output.name_only]));
log("pinned " + PIN.base_sha + ".." + PIN.head_sha + " diff " + diff_id);

let paths = changed_paths(recon.output.name_only);
if (paths.len() == 0) != (PIN.base_tree == PIN.head_tree) {
    let why = "recon's listings disagree with the pinned trees";
    let mismatch_report = write_scratch_file("review.md", "# Review\n\nBlocked: " + why + "\n");
    complete(#{ status: "blocked", reason: why, counts: empty_counts(), findings: [], report: #{ report: "Blocked: " + why, merge_recommendation: "none" }, limitations: [why], path: mismatch_report });
}
if paths.len() == 0 {
    let empty_report = write_scratch_file("review.md", "# Review\n\nNo changed paths between " + PIN.base_sha + " and " + PIN.head_sha + ".\n");
    complete(#{
        status: "complete",
        base_sha: PIN.base_sha,
        head_sha: PIN.head_sha,
        diff_id: diff_id,
        counts: empty_counts(),
        findings: [],
        report: #{ report: "No changed paths.", merge_recommendation: "none" },
        limitations: [],
        path: empty_report,
    });
}

phase("Scan");
let jobs = [];
for lens in LENSES {
    jobs.push(#{
        prompt: scan_prompt(lens, PIN, focus, paths, READ_RULES, MAX_CANDIDATES, MIN_CONFIDENCE),
        agent_type: "code-review-scan",
        cwd: repo,
        label: "scan-" + lens.id,
        output_schema: SCAN,
    });
}
let scans = parallel(jobs);

let candidates = [];
let limitations = [];
let ids = [];
for slot in 0..LENSES.len() {
    let lens = LENSES[slot];
    let scan = scans[slot];
    let why = scan_problem(scan);
    if why != "" {
        limitations.push(lens.id + ": " + why);
        log("lens " + lens.id + " is a limitation: " + why);
        continue;
    }
    let kept = 0;
    for c in scan.output.candidates {
        let id = lens.id + ":" + c.slug;
        let drop = "";
        if !paths.contains(c.path) {
            drop = "its path is not in the diff";
        } else if !(c.confidence >= MIN_CONFIDENCE) {
            drop = "its confidence is below " + MIN_CONFIDENCE;
        } else if kept >= MAX_CANDIDATES {
            drop = "the lens already has " + MAX_CANDIDATES + " candidates";
        } else if ids.contains(id) {
            drop = "its finding id repeats";
        }
        if drop != "" {
            log("dropped " + id + ": " + drop);
            continue;
        }
        ids.push(id); kept += 1;
        let finding = c;
        finding.finding_id = id;
        finding.lens = lens.id;
        candidates.push(finding);
    }
}

let all_blocked = limitations.len() == LENSES.len();

phase("Verify");
let groups = [];
let group_at = #{};
for f in candidates {
    let key = f.path + ":" + f.line;
    if key in group_at {
        let g = groups[group_at[key]];
        g.candidates.push(f);
        groups[group_at[key]] = g;
    } else {
        group_at[key] = groups.len();
        groups.push(#{ key: key, candidates: [f] });
    }
}
let sent = if groups.len() > MAX_VERIFY { MAX_VERIFY } else { groups.len() };
for gi in sent..groups.len() {
    log("not verified: " + groups[gi].key + " is beyond the first " + MAX_VERIFY + " groups");
}
let verify_jobs = [];
for gi in 0..sent {
    verify_jobs.push(#{
        prompt: verify_prompt(groups[gi], PIN, READ_RULES, MIN_CONFIDENCE),
        agent_type: "code-review-verify",
        cwd: repo,
        label: "verify-" + gi,
        output_schema: VERIFY,
    });
}
let verdicts = if verify_jobs.len() > 0 { parallel(verify_jobs) } else { [] };

// Confirmed only by verdict, confidence, evidence and the pinned trees; a () slot is unverified.
let confirmed = [];
let unverified = [];
let n_refuted = 0;
let n_high = 0;
for gi in 0..groups.len() {
    let slot = if gi < verdicts.len() { verdicts[gi] } else { () };
    let why = if gi < verdicts.len() { verify_problem(slot, PIN) } else { "beyond the first " + MAX_VERIFY + " groups" };
    if why == "" && "builds_not_run" in slot.output && slot.output.builds_not_run { limitations.push("verify-" + gi + ": builds not run"); }
    for f in groups[gi].candidates {
        let v = if why == "" { verdict_for(slot, f.finding_id) } else { () };
        if v == () {
            unverified.push(f);
            log("unverified " + f.finding_id + ": " + (if why == "" { "no verdict returned" } else { why }));
            continue;
        }
        let evidence = v.evidence;
        evidence.trim();
        if v.verdict == "confirmed" && v.confidence >= MIN_CONFIDENCE && evidence != "" {
            let finding = f;
            finding.confidence = v.confidence;
            finding.evidence = evidence;
            finding.reasoning = v.reasoning;
            finding.verdict = "confirmed";
            finding.verification = v.reasoning;
            finding.detail = f.impact;
            finding.area_id = f.lens;
            confirmed.push(finding);
            if f.severity == "Critical" || f.severity == "Major" { n_high += 1; }
        } else if v.verdict == "refuted" {
            n_refuted += 1;
        } else {
            unverified.push(f);
            log("unverified " + f.finding_id + ": the confirmation is below the bar");
        }
    }
}
if unverified.len() > 0 { limitations.push("unverified candidates: " + unverified.len()); }
let summary = counts_text(candidates.len(), confirmed.len(), n_refuted, unverified.len(), n_high);

phase("Triage");
let triage = agent(triage_prompt(PIN, summary, confirmed, limitations), #{
    agent_type: "code-review-triage",
    cwd: repo,
    label: "triage",
    output_schema: TRIAGE,
});
let report = "Triage did not succeed; the confirmed findings below stand on their own.";
let recommendation = "none";
if triage != () && triage.success {
    report = triage.output.report;
    recommendation = triage.output.merge_recommendation;
} else {
    limitations.push("triage did not succeed");
}

let p = write_scratch_file("review.md", render_report(PIN, summary, recommendation, report, confirmed, limitations));
complete(#{
    status: if all_blocked { "blocked" } else { "complete" },
    base_sha: PIN.base_sha,
    head_sha: PIN.head_sha,
    diff_id: diff_id,
    counts: #{ candidate: candidates.len(), confirmed: confirmed.len(), refuted: n_refuted, unverified: unverified.len(), high_priority: n_high },
    findings: confirmed,
    report: #{ report: report, merge_recommendation: recommendation },
    limitations: limitations,
    path: p,
});
