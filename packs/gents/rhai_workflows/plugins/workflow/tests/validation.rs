//! Static validation (L2 section 4.5, steps 1-4): the token prefix, the meta pass,
//! shadowing and strict variables. Tests assert codes and behaviour, never Grok text (U15).
use workflow::errors::WORKFLOW_VALIDATION_FAILED;
use workflow::validate::{self, Meta, Phase};

const META: &str = r#"let meta = #{ name: "demo", description: "A demo." };"#;

fn refused(source: &str) -> String {
    let message = validate::validate(source).expect_err(source);
    assert!(
        message.starts_with(&format!("{WORKFLOW_VALIDATION_FAILED}: ")),
        "{message}"
    );
    assert!(message.contains(". Next: "), "{message}");
    message
}

#[test]
fn meta_pass_refusals() {
    // The token prefix.
    refused("let m = #{ name: \"demo\", description: \"d\" };");
    refused("// only a comment\nlet other = 1;");
    refused("");
    // A pure-literal meta that does not come first is refused by the prefix rule itself.
    let message = refused(&format!("let x = 1;\n{META}"));
    assert!(
        message.starts_with(&format!(
            "{WORKFLOW_VALIDATION_FAILED}: the script must start with"
        )),
        "{message}"
    );
    // Not a pure literal.
    refused(r#"let meta = #{ name: "de" + "mo", description: "d" };"#);
    refused(r#"let meta = #{ name: "demo", description: "d" + "e" };"#);
    // A bad name.
    for name in ["", "Demo", "a_b", "a--b", "-a", "a-", "a b"] {
        refused(&format!(
            r#"let meta = #{{ name: "{name}", description: "d" }};"#
        ));
    }
    refused(r#"let meta = #{ name: 7, description: "d" };"#);
    refused(r#"let meta = #{ description: "d" };"#);
    // An empty or missing description.
    refused(r#"let meta = #{ name: "demo", description: "" };"#);
    refused(r#"let meta = #{ name: "demo" };"#);
    // when_to_use must be a string when present.
    refused(r#"let meta = #{ name: "demo", description: "d", when_to_use: 3 };"#);
    // Phases: each is {title, detail?} and titles are unique.
    refused(
        r#"let meta = #{ name: "demo", description: "d", phases: [#{ title: "a" }, #{ title: "a" }] };"#,
    );
    refused(r#"let meta = #{ name: "demo", description: "d", phases: [#{ detail: "x" }] };"#);
    refused(r#"let meta = #{ name: "demo", description: "d", phases: "a" };"#);
}

#[test]
fn shadowing_reserved_names_is_refused() {
    for name in validate::RESERVED {
        let message = refused(&format!("{META}\nfn {name}(x) {{ x }}"));
        // rhai reserves `print` and `debug` as keywords: its parser refuses them first.
        if matches!(name, "print" | "debug") {
            continue;
        }
        assert!(
            message.starts_with(&format!(
                "{WORKFLOW_VALIDATION_FAILED}: fn {name} shadows a host function"
            )),
            "{message}"
        );
    }
    // A helper with another name is fine.
    validate::validate(&format!("{META}\nfn helper(x) {{ x }}")).unwrap();
}

#[test]
fn undeclared_variable_is_refused_args_compiles() {
    refused(&format!("{META}\nlet x = missing + 1;"));
    validate::validate(&format!("{META}\nlet x = args.n;")).unwrap();
}

#[test]
fn valid_script_returns_meta() {
    let source = r#"
// a leading comment
/* and a block /* nested */ comment */
let meta = #{
    name: "topic-notes2",
    description: "Research a topic.",
    when_to_use: "When a question needs sources.",
    phases: [#{ title: "plan", detail: "Split the topic." }, #{ title: "write" }],
};
let out = args.topic;
"#;
    assert_eq!(
        validate::validate(source).unwrap(),
        Meta {
            name: "topic-notes2".into(),
            description: "Research a topic.".into(),
            when_to_use: Some("When a question needs sources.".into()),
            phases: vec![
                Phase {
                    title: "plan".into(),
                    detail: Some("Split the topic.".into())
                },
                Phase {
                    title: "write".into(),
                    detail: None
                },
            ],
        }
    );
    let minimal = validate::validate(META).unwrap();
    assert_eq!(minimal.when_to_use, None);
    assert!(minimal.phases.is_empty());
}

#[test]
fn a_workflow_name_in_script_names_the_run_call() {
    for word in [
        "wf-live-args-echo",
        "wf-live-args-echo.rhai",
        "dir/wf-live-args-echo.rhai",
    ] {
        let message = refused(word);
        assert!(message.contains("not source"), "{message}");
        assert!(
            message.ends_with("Next: workflow run wf-live-args-echo (leave out script and path)"),
            "{message}"
        );
    }
}
