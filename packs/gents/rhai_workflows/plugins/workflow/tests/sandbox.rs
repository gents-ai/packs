//! The sandboxed engine and the error vocabulary (L2 sections 4.4 and 4.17).
//! Tests assert codes and behaviour, never Grok's text (U15).
use std::cell::Cell;
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult};
use workflow::errors::{self, CODES};
use workflow::sandbox::{self, LogBuffer, Suspension, SuspensionCell};

fn engine() -> (Engine, LogBuffer, SuspensionCell) {
    let log = LogBuffer::default();
    let cell = SuspensionCell::default();
    (sandbox::engine(&log, &cell, false), log, cell)
}

fn runtime_message(err: &EvalAltResult) -> String {
    match err {
        EvalAltResult::ErrorRuntime(value, _) => value.to_string(),
        other => panic!("expected ErrorRuntime, got {other:?}"),
    }
}

#[test]
fn each_stub_throws_its_code() {
    let (engine, ..) = engine();
    for call in [
        "timestamp()",
        "sleep()",
        "sleep(5)",
        "sleep(0.5)",
        "sleep(\"x\")",
        "exit()",
        "exit(1)",
    ] {
        let err = engine.eval::<Dynamic>(call).unwrap_err();
        let message = runtime_message(&err);
        assert!(
            message.starts_with(&format!("{}: ", errors::WORKFLOW_CAPABILITY_UNAVAILABLE)),
            "{call}: {message}"
        );
        assert!(err.is_catchable(), "{call} must be catchable");
    }
    // `exit` is a stub, not a reserved word: it can be called and caught.
    let caught: String = engine
        .eval(r#"let r = (); try { exit() } catch (e) { r = e; } r"#)
        .unwrap();
    assert!(caught.starts_with(errors::WORKFLOW_CAPABILITY_UNAVAILABLE));
}

#[test]
fn eval_import_and_print_are_contained() {
    let (engine, log, _) = engine();
    assert!(matches!(
        *engine.eval::<Dynamic>(r#"eval("1 + 1")"#).unwrap_err(),
        EvalAltResult::ErrorParsing(..)
    ));
    assert!(matches!(
        *engine.eval::<Dynamic>(r#"import "x" as m; 1"#).unwrap_err(),
        EvalAltResult::ErrorTooManyModules(..)
    ));
    engine.run(r#"print("to the log"); debug(42);"#).unwrap();
    let lines = log.borrow().clone();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("to the log"));
    assert!(lines[1].contains("42"));
}

#[test]
fn suspension_escapes_try_catch_infrastructure_error_does_not() {
    let (mut engine, _, cell) = engine();
    let raise = Rc::clone(&cell);
    engine.register_fn("suspend_now", move || -> Result<(), Box<EvalAltResult>> {
        Err(sandbox::suspend(&raise, Suspension::Pending))
    });
    engine.register_fn("refuse_now", || -> Result<(), Box<EvalAltResult>> {
        Err(sandbox::infrastructure_error(
            errors::WORKFLOW_POLICY_DENIED,
            "the target is outside the ceiling",
            "workflow help agent",
        ))
    });

    let err = engine
        .eval::<Dynamic>("try { suspend_now(); 1 } catch (e) { 99 }")
        .unwrap_err();
    assert!(!err.is_catchable());
    assert_eq!(*cell.borrow(), Some(Suspension::Pending));

    cell.take(); // the next step
    let caught: String = engine
        .eval("let r = (); try { refuse_now(); } catch (e) { r = e; } r")
        .unwrap();
    assert!(caught.starts_with(errors::WORKFLOW_POLICY_DENIED));

    for token in [
        Suspension::Pending,
        Suspension::Complete("done".into()),
        Suspension::Pause {
            kind: "budget".into(),
            message: "m".into(),
        },
        Suspension::AwaitUser("question".into()),
    ] {
        let cell = SuspensionCell::default();
        let err = sandbox::suspend(&cell, token.clone());
        assert!(!err.is_catchable());
        assert_eq!(*cell.borrow(), Some(token));
    }
}

/// A closure passed to an array method has its error wrapped in the catchable
/// `ErrorInFunctionCall`, so the suspension must survive through the step's cell.
#[test]
fn suspension_raised_in_a_closure_escapes_try_catch() {
    let touched = Rc::new(Cell::new(0));
    for script in [
        "try { [1].map(|x| suspend_now()); } catch (e) { 1 } touch();",
        "try { [1].map(|x| suspend_now()); } catch (e) { } touch();",
    ] {
        let (mut engine, _, cell) = engine();
        let raise = Rc::clone(&cell);
        engine.register_fn("suspend_now", move || -> Result<(), Box<EvalAltResult>> {
            Err(sandbox::suspend(&raise, Suspension::Pending))
        });
        let count = Rc::clone(&touched);
        engine.register_fn("touch", move || count.set(count.get() + 1));

        let err = engine.eval::<Dynamic>(script).unwrap_err();
        assert!(!err.is_catchable(), "{script}: {err:?}");
        assert_eq!(*cell.borrow(), Some(Suspension::Pending), "{script}");
        assert_eq!(touched.get(), 0, "{script} ran past the suspension");
    }
}

#[test]
fn engine_errors_have_code_what_next_shape() {
    assert_eq!(CODES.len(), 12);
    for code in CODES {
        let message = errors::message(code, "something went wrong", "workflow help doctor");
        assert_eq!(
            message,
            format!("{code}: something went wrong. Next: workflow help doctor")
        );
    }
    let (engine, ..) = engine();
    for call in ["timestamp()", "sleep()", "exit()"] {
        let message = runtime_message(&engine.eval::<Dynamic>(call).unwrap_err());
        let (code, rest) = message.split_once(": ").expect("code prefix");
        assert!(CODES.contains(&code), "{message}");
        let (what, next) = rest.split_once(". Next: ").expect("what and next");
        assert!(!what.is_empty() && !next.is_empty(), "{message}");
    }
}
