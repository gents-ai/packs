let meta = #{
    name: "composite-review",
    description: "a review that mixes phases, panels, scratch files, closures and error handling",
    when_to_use: "to exercise most of the dialect in one script",
    phases: [
        #{ title: "survey", detail: "list the areas to review" },
        #{ title: "review", detail: "one agent per area" },
        #{ title: "verify", detail: "check the findings" },
        #{ title: "report" },
    ],
};

// Helpers: plain fns, no host calls inside the pure ones.
fn pad(n, width) {
    let s = `${n}`;
    while s.len() < width { s = "0" + s; }
    s
}

fn label_of(area, i) { "review-" + pad(i, 2) + "-" + area }

fn tally(items) {
    let counts = #{};
    for item in items {
        let key = item.kind;
        counts[key] = if key in counts { counts[key] + 1 } else { 1 };
    }
    counts
}

fn describe(count) {
    switch count {
        0 => "nothing",
        1 | 2 => "a few",
        3..=5 => "several",
        _ => "many",
    }
}

fn clamp(x, lo, hi) { if x < lo { lo } else if x > hi { hi } else { x } }

// Survey: the areas come from args when given, else a default list.
phase("survey");
log("survey started");
let areas = if args == () { ["parser", "runtime", "docs", "tests", "build"] } else { args.areas };
let survey = agent("list one risk per area: " + areas.len() + " areas", #{ label: "survey", phase: "survey" });
let risks = if survey.success { [survey.output] } else { [] };
log(`survey found ${risks.len()} risks`);
write_scratch_file("survey.json", json_encode(#{ areas: areas, risks: risks }));

// Review: one panel, one agent per area.
phase("review");
let items = [];
let i = 0;
for area in areas {
    items.push(#{
        prompt: "review the " + area + " area for defects",
        label: label_of(area, i),
        phase: "review",
    });
    i += 1;
}
let panel = parallel(items);
let findings = [];
let failed = 0;
i = 0;
for p in panel {
    if p == () || !p.success {
        failed += 1;
    } else {
        findings.push(#{ area: areas[i], kind: if p.output.len() > 3 { "long" } else { "short" }, text: p.output });
    }
    i += 1;
}
log(`review done, ${failed} failed`);

// A second, smaller panel that reads the first one.
let second = [];
for f in findings {
    second.push(#{ prompt: "double-check: " + f.text + " (" + f.area + ")", label: "check-" + f.area });
}
let checks = if second.len() > 0 { parallel(second) } else { [] };
let confirmed = checks.filter(|c| c != () && c.success).len();

// Verify: a guarded call, a caught refusal, and a budget read.
phase("verify");
let note = "none";
try {
    agent("probe with a bad option", #{ label: 7 });
} catch (e) {
    note = e.split(":")[0];
}
let before = budget();
let verdict = agent("summarize the confirmed findings in one line", #{ label: "verdict" });
let after = budget();

// Pure work: sorting, a map walk, strings and a closure.
let kinds = tally(findings);
let names = kinds.keys();
names.sort();
let weights = names.map(|k| kinds[k] * 10);
let total = 0;
for w in weights { total += w; }
let width = clamp(total, 0, 100);
let ruler = "";
for n in 0..width / 10 { ruler += "#"; }
let digest = fingerprint(json_encode(kinds));

// Report: one scratch file with the table, one without.
phase("report");
let table = "";
for f in findings {
    table += "| " + f.area + " | " + f.kind + " | " + f.text + " |\n";
}
let table_path = write_scratch_file("table.md", "| area | kind | text |\n" + table);
let back = read_scratch_file("table.md");
let rows = back.split("\n").len() - 1;
let by_kind = #{};
for k in names { by_kind[k] = kinds[k]; }

let outcome = if failed > 0 {
    "partial"
} else if confirmed == findings.len() {
    "confirmed"
} else {
    "unconfirmed"
};
log("report written");

complete(#{
    outcome: outcome,
    areas: areas.len(),
    risks: risks,
    findings: findings.len(),
    summary: describe(findings.len()),
    failed: failed,
    confirmed: confirmed,
    note: note,
    verdict: verdict.output,
    spent: after.spent - before.spent,
    remaining: after.remaining,
    by_kind: by_kind,
    weights: weights,
    ruler: ruler,
    digest: digest,
    rows: rows,
    table: table_path,
});
