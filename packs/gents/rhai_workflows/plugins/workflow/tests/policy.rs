//! Origin prediction and the policy pre-checks (L2 sections 4.6 and 4.10; root C1, C5.5, C5.6),
//! on the fake host and through direct calls of `invoke`.
use serde_json::{Value, json};
use workflow::fakehost::cli::facts;
use workflow::fakehost::faults::narrowed;
use workflow::fakehost::oracle::{canned, spawns};
use workflow::fakehost::{Call, FakeHost, HostFacts, Kind, PackAssets, mounts};
use workflow::invoke;
use workflow::origin::predict;
use workflow::wire::Origin;

const SPAWNS: &str = r##"let meta = #{ name: "gate", description: "spawns one agent" };
let r = agent("work");
complete(r.output);
"##;

fn call(script: &str, host: HostFacts) -> Call {
    Call {
        kind: Kind::JobStep,
        input: json!({"protocol": 1, "argv": ["run", "inline"], "script": script}),
        host,
    }
}

/// The demo facts with the ceiling fields in `patch` replaced.
fn ceiling(patch: Value) -> HostFacts {
    narrowed(&facts("/work"), patch)
}

/// A mounts value with one workflows pack holding `script`, trusted or not.
fn mounted(pack: &str, trusted: bool, script: &str) -> Value {
    let assets = [PackAssets {
        pack: pack.into(),
        class: "workflows".into(),
        digest: "sha256:pack".into(),
        trusted,
    }];
    let value = mounts(&assets, None).unwrap();
    let dir = value[0]["path"].as_str().unwrap().to_string();
    std::fs::create_dir_all(format!("{dir}/nested")).unwrap();
    std::fs::write(format!("{dir}/nested/gate.rhai"), script).unwrap();
    value
}

fn message(out: &Value) -> &str {
    out["fail"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{out}"))
}

#[test]
fn trusted_mount_predicts_installed_else_dynamic() {
    let trusted = mounted("pol-trusted", true, SPAWNS);
    let untrusted = mounted("pol-untrusted", false, SPAWNS);
    let other = mounted("pol-other", true, "complete(1);");
    assert_eq!(predict(SPAWNS, &trusted), Origin::Installed);
    assert_eq!(predict(SPAWNS, &untrusted), Origin::Dynamic, "untrusted");
    assert_eq!(predict(SPAWNS, &other), Origin::Dynamic, "other bytes");
    assert_eq!(predict(SPAWNS, &json!([])), Origin::Dynamic, "no mount");
    assert_eq!(predict(SPAWNS, &Value::Null), Origin::Dynamic, "no mounts");
    let edited = format!("{SPAWNS} ");
    assert_eq!(predict(&edited, &trusted), Origin::Dynamic, "one byte off");
    let project = mounts(&[], Some("/nowhere")).unwrap();
    assert_eq!(predict(SPAWNS, &project), Origin::Dynamic, "project tier");
}

#[test]
fn row12_dynamic_off_check_and_run_policies() {
    // off: the run is refused at step 0, before any pin.
    let mut host = FakeHost::default();
    let off = ceiling(json!({"dynamic": "off"}));
    let s0 = host.step0(&call(SPAWNS, off), invoke).unwrap();
    assert!(s0.row.is_none() && !s0.continues, "{}", s0.output);
    assert_eq!(s0.output["fail"]["code"], "workflow_policy_denied");
    assert_eq!(
        message(&s0.output),
        "workflow_policy_denied: gate would run as a dynamic script and job.policy.dynamic is off. \
         Next: workflow help lifecycle"
    );

    // check: the run starts, the first spawn is refused; a script without one completes.
    let check = ceiling(json!({"dynamic": "check"}));
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, check.clone()), invoke)
        .unwrap();
    assert_eq!(s0.output["fail"]["code"], "workflow_policy_denied");
    assert!(
        message(&s0.output)
            .ends_with(r#"Next: ask the operator to trust the pack or set dynamic "run""#)
    );
    assert!(s0.row.is_none() && spawns(&s0.output).is_empty());
    let quiet = "let meta = #{ name: \"quiet\", description: \"no agents\" };\ncomplete(1);\n";
    let s0 = FakeHost::default()
        .step0(&call(quiet, check.clone()), invoke)
        .unwrap();
    assert_eq!(s0.output["result"], 1, "{}", s0.output);
    // check: a panel is refused like an agent.
    let panel = "let meta = #{ name: \"panel\", description: \"a panel\" };\n\
                 parallel([#{ prompt: \"a\" }]);\ncomplete(1);\n";
    let s0 = FakeHost::default()
        .step0(&call(panel, check.clone()), invoke)
        .unwrap();
    assert_eq!(s0.output["fail"]["code"], "workflow_policy_denied");
    // check, but the bytes sit in a trusted mount: predicted installed, so no policy applies.
    let mut trusted = check.clone();
    trusted.mounts = mounted("pol-check", true, SPAWNS);
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, trusted), invoke)
        .unwrap();
    assert_eq!(spawns(&s0.output).len(), 1, "{}", s0.output);

    // run: targets are ceiling.targets within dynamic_targets; a wider one is catchable.
    let run = ceiling(json!({
        "dynamic": "run", "targets": ["wf-read-only", "wf-read-write"],
        "dynamic_targets": ["wf-read-only"]
    }));
    let wide = r##"let meta = #{ name: "wide", description: "asks for more" };
try { agent("edit", #{ capability_mode: "read-write" }); } catch (e) { complete(e); }
"##;
    let s0 = FakeHost::default()
        .step0(&call(wide, run.clone()), invoke)
        .unwrap();
    let msg = s0.output["result"]
        .as_str()
        .unwrap_or_else(|| panic!("{}", s0.output));
    assert!(
        msg.starts_with("workflow_capability_unavailable: "),
        "{msg}"
    );
    assert!(msg.contains("allowed: wf-read-only. Next:"), "{msg}");
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, run), invoke)
        .unwrap();
    assert_eq!(spawns(&s0.output).len(), 1, "{}", s0.output);
}

#[test]
fn verified_script_digest_mismatch_fails_closed() {
    let mut host = FakeHost::default();
    let at0 = call(SPAWNS, facts("/work"));
    let s0 = host.step0(&at0, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);

    let mut tampered = at0.clone();
    let forged = format!("{SPAWNS}// tampered\n");
    tampered.host.verified_script = Some(json!({"source": forged, "digest": "sha256:x"}));
    let s1 = host.step(&tampered, &row, invoke).unwrap();
    assert_eq!(s1.output["fail"]["code"], "workflow_validation_failed");
    assert!(
        message(&s1.output).contains("differ from the pinned digest. Next: workflow list"),
        "{}",
        s1.output
    );
    assert!(spawns(&s1.output).is_empty() && !s1.continues);

    // The pinned bytes run as the verified script, with no WfScript fetch.
    let mut honest = FakeHost::default();
    let s0 = honest.step0(&at0, invoke).unwrap();
    let row = s0.row.clone().unwrap();
    honest.settle(&spawns(&s0.output), canned);
    let mut same = at0.clone();
    same.host.verified_script = Some(json!({"source": SPAWNS, "digest": "sha256:ok"}));
    let s1 = honest.step(&same, &row, invoke).unwrap();
    assert!(s1.output.get("result").is_some(), "{}", s1.output);
}

#[test]
fn a_ceiling_missing_dynamic_or_remaining_spawns_fails_closed() {
    // No `dynamic`: the job block's default, check, refuses the first spawn.
    let mut bare = ceiling(json!({}));
    bare.ceiling.as_object_mut().unwrap().remove("dynamic");
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, bare), invoke)
        .unwrap();
    assert_eq!(
        s0.output["fail"]["code"], "workflow_policy_denied",
        "{}",
        s0.output
    );
    assert!(message(&s0.output).contains("job.policy.dynamic is check"));

    // No `remaining_spawns`: a panel is refused, as when the budget is spent.
    let panel = "let meta = #{ name: \"panel\", description: \"a panel\" };\n\
                 parallel([#{ prompt: \"a\" }]);\ncomplete(1);\n";
    let mut open = ceiling(json!({}));
    open.ceiling
        .as_object_mut()
        .unwrap()
        .remove("remaining_spawns");
    let s0 = FakeHost::default()
        .step0(&call(panel, open), invoke)
        .unwrap();
    assert_eq!(
        s0.output["fail"]["code"], "workflow_policy_denied",
        "{}",
        s0.output
    );
    assert!(message(&s0.output).contains("remaining_spawns"));
}

#[test]
fn an_agent_past_remaining_spawns_is_refused_as_a_panel_is() {
    let s0 = FakeHost::default()
        .step0(
            &call(SPAWNS, ceiling(json!({"remaining_spawns": 0}))),
            invoke,
        )
        .unwrap();
    assert_eq!(
        s0.output["fail"]["code"], "workflow_policy_denied",
        "{}",
        s0.output
    );
    assert!(
        message(&s0.output).contains("remaining_spawns is 0")
            && message(&s0.output).ends_with("Next: workflow status cli:t1"),
        "{}",
        s0.output
    );
    assert!(spawns(&s0.output).is_empty() && s0.code.is_none());
}

#[test]
fn a_later_step_never_runs_a_script_that_does_not_match_the_pin() {
    let at0 = call(SPAWNS, facts("/work"));
    let started = || {
        let mut host = FakeHost::default();
        let s0 = host.step0(&at0, invoke).unwrap();
        let row = s0.row.clone().unwrap();
        host.settle(&spawns(&s0.output), canned);
        (host, row)
    };

    // A verified_script block without source is refused, not skipped.
    let (mut host, row) = started();
    let mut bare = at0.clone();
    bare.host.verified_script = Some(json!({"digest": "sha256:x"}));
    let s1 = host.step(&bare, &row, invoke).unwrap();
    assert_eq!(s1.output["fail"]["code"], "workflow_validation_failed");
    assert!(
        message(&s1.output).contains("carries no source"),
        "{}",
        s1.output
    );

    // The stored script is held to the pinned digest too.
    let (mut host, row) = started();
    let stored = host.state.collections.get_mut("WfScript").unwrap();
    stored[0]["source"] = json!(format!("{SPAWNS}// tampered\n"));
    let s1 = host.step(&at0, &row, invoke).unwrap();
    assert_eq!(s1.output["fail"]["code"], "workflow_validation_failed");
    assert!(
        message(&s1.output).contains("stored script's bytes differ from the pinned digest"),
        "{}",
        s1.output
    );

    // The echoed `script` is not a source on a later step: with no stored one, it fails.
    let input = json!({"protocol": 1, "argv": ["run", "inline"], "script": SPAWNS,
        "pin": {"script_id": "s", "source_digest": "d", "agent_budget": 8, "handle": "h"}});
    let replay = workflow::rounds::Replay {
        notes: Vec::new(),
        source: None,
        child_text: Default::default(),
    };
    let out = workflow::run::step(&input, replay);
    assert_eq!(out["fail"]["code"], "workflow_resolve_failed", "{out}");
}

#[test]
fn row11_started_by_job_is_depth_exceeded() {
    let mut nested = facts("/work");
    nested.caller["started_by_job"] = json!(true);
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, nested), invoke)
        .unwrap();
    assert!(s0.row.is_none() && !s0.continues, "{}", s0.output);
    assert_eq!(s0.output["fail"]["code"], "workflow_depth_exceeded");
    let msg = message(&s0.output);
    assert!(
        msg.contains("top-level session") && msg.contains("Next: "),
        "{msg}"
    );

    let mut top = facts("/work");
    top.caller["started_by_job"] = json!(false);
    let s0 = FakeHost::default()
        .step0(&call(SPAWNS, top), invoke)
        .unwrap();
    assert_eq!(spawns(&s0.output).len(), 1, "{}", s0.output);
}

#[test]
fn row10_narrowed_ceiling_refuses_and_live_children_end_policy_denied() {
    // A panel start past ceiling.remaining_spawns is refused with a status call.
    let script = r##"let meta = #{ name: "narrow", description: "an agent, then a panel" };
agent("one");
parallel([#{ prompt: "a" }, #{ prompt: "b" }, #{ prompt: "c" }]);
complete(1);
"##;
    let mut host = FakeHost::default();
    let wide = ceiling(json!({"remaining_spawns": 4}));
    let s0 = host.step0(&call(script, wide), invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let tight = call(script, ceiling(json!({"remaining_spawns": 3})));
    let s1 = host.step(&tight, &row, invoke).unwrap();
    assert_eq!(s1.output["fail"]["code"], "workflow_policy_denied");
    assert!(
        message(&s1.output).ends_with("Next: workflow status cli:t1"),
        "{}",
        s1.output
    );
    assert!(spawns(&s1.output).is_empty() && s1.code.is_none());

    // A removed target is catchable, as at any agent call.
    let removed = r##"let meta = #{ name: "gone", description: "a target goes away" };
agent("one");
try { parallel([#{ prompt: "a", capability_mode: "read-write" }]); } catch (e) { complete(e); }
"##;
    let mut host = FakeHost::default();
    let both = ceiling(json!({"targets": ["wf-read-only", "wf-read-write"]}));
    let s0 = host.step0(&call(removed, both), invoke).unwrap();
    let row = s0.row.clone().unwrap();
    host.settle(&spawns(&s0.output), canned);
    let one = call(removed, ceiling(json!({"targets": ["wf-read-only"]})));
    let s1 = host.step(&one, &row, invoke).unwrap();
    let msg = s1.output["result"]
        .as_str()
        .unwrap_or_else(|| panic!("{}", s1.output));
    assert!(
        msg.starts_with("workflow_capability_unavailable: "),
        "{msg}"
    );

    // A refusal while the window's children are live is returned as `fail`; the host ends the
    // row with plugin_job_policy_denied and kills them.
    let panel = r##"let meta = #{ name: "live", description: "a panel of two" };
parallel([#{ prompt: "a" }, #{ prompt: "b" }]);
complete(1);
"##;
    let mut dynamic = facts("/work");
    dynamic.origin = json!("dynamic");
    let mut host = FakeHost::default();
    let s0 = host.step0(&call(panel, dynamic.clone()), invoke).unwrap();
    let row = s0.row.clone().unwrap();
    assert_eq!(spawns(&s0.output).len(), 2, "{}", s0.output);
    let mut narrowed_off = dynamic;
    narrowed_off.ceiling["dynamic"] = json!("off");
    let s1 = host.step(&call(panel, narrowed_off), &row, invoke).unwrap();
    assert_eq!(
        s1.output["fail"]["code"], "workflow_policy_denied",
        "{}",
        s1.output
    );
    assert_eq!(s1.code.as_deref(), Some("plugin_job_policy_denied"));
    assert!(!s1.continues);
    assert!(
        host.state
            .children
            .values()
            .all(|c| c.state == "interrupted"),
        "the host kills the window"
    );
}
