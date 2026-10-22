//! The sandboxed rhai engine (L2 section 4.4): no modules, no eval, bounded work, and
//! output that never reaches stdout, which is the JSON channel.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rhai::packages::{Package, StandardPackage};
use rhai::{Dynamic, Engine, EvalAltResult, FLOAT, INT, OptimizationLevel, Position};
use serde_json::Value;

use crate::errors::{self, WORKFLOW_CAPABILITY_UNAVAILABLE};

/// The step's log buffer: what `print` and `debug` write.
pub type LogBuffer = Rc<RefCell<Vec<String>>>;

/// The S0 operation limit.
const MAX_OPERATIONS: u64 = 50_000_000;
const MAX_STRING_SIZE: usize = 4 * 1024 * 1024;
const MAX_COLLECTION_SIZE: usize = 100_000;

/// The step's pending suspension. A host function records its token here as it raises it.
pub type SuspensionCell = Rc<RefCell<Option<Suspension>>>;

/// A signal a host function raises to end the step. It travels as `ErrorTerminated`, which
/// `try/catch` in a script cannot catch, but rhai wraps an error from a closure passed to an
/// array method in a catchable `ErrorInFunctionCall`. So the token also lives in the step's
/// [`SuspensionCell`], `engine` stops the run while the cell is set, and the step is classified
/// from the cell, never from the error shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Suspension {
    Pending,
    Complete(Value),
    Pause { kind: String, message: String },
    AwaitUser(Value),
}

/// Record the first suspension of the step and return the error that ends the run.
pub fn suspend(cell: &SuspensionCell, token: Suspension) -> Box<EvalAltResult> {
    let token = cell.borrow_mut().get_or_insert(token).clone();
    EvalAltResult::ErrorTerminated(Dynamic::from(token), Position::NONE).into()
}

/// A catchable refusal: `ErrorRuntime` carrying `<code>: <what>. Next: <one call>`.
pub fn infrastructure_error(code: &str, what: &str, next: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(errors::message(code, what, next).into(), Position::NONE).into()
}

fn stub_error(what: &str, next: &str) -> Box<EvalAltResult> {
    infrastructure_error(WORKFLOW_CAPABILITY_UNAVAILABLE, what, next)
}

thread_local! {
    static UNOPTIMIZED: Cell<bool> = const { Cell::new(false) };
}

/// Make every engine this thread builds run at `None`, as `invoke` builds them without a flag
/// to pass. The differential's second leg; the default is off.
pub fn set_unoptimized(on: bool) {
    UNOPTIMIZED.set(on);
}

/// Build the engine at `Simple`, or at `None` when `unoptimized` (the differential's second leg).
pub fn engine(log: &LogBuffer, suspension: &SuspensionCell, unoptimized: bool) -> Engine {
    let mut engine = Engine::new_raw();
    engine.register_global_module(StandardPackage::new().as_shared_module());
    engine
        .set_max_modules(0)
        .set_max_operations(MAX_OPERATIONS)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_array_size(MAX_COLLECTION_SIZE)
        .set_max_map_size(MAX_COLLECTION_SIZE)
        .set_optimization_level(if unoptimized || UNOPTIMIZED.get() {
            OptimizationLevel::None
        } else {
            OptimizationLevel::Simple
        })
        .disable_symbol("eval");

    let progress_cell = Rc::clone(suspension);
    engine.on_progress(move |_| progress_cell.borrow().clone().map(Dynamic::from));

    let print_log = Rc::clone(log);
    engine.on_print(move |text| print_log.borrow_mut().push(text.to_string()));
    let debug_log = Rc::clone(log);
    engine.on_debug(move |text, _source, _position| debug_log.borrow_mut().push(text.to_string()));

    engine.register_fn("timestamp", || -> Result<Dynamic, Box<EvalAltResult>> {
        Err(stub_error(
            "the script must be deterministic, so wall-clock time is unavailable",
            "pass a timestamp in args",
        ))
    });
    // StandardPackage has typed `sleep(INT)` and `sleep(FLOAT)`; they win over a `Dynamic` stub.
    let sleep = || {
        stub_error(
            "sleep is unavailable because host calls already wait for their work",
            "workflow help host",
        )
    };
    engine.register_fn("sleep", move || -> Result<Dynamic, Box<EvalAltResult>> {
        Err(sleep())
    });
    engine.register_fn(
        "sleep",
        move |_: INT| -> Result<Dynamic, Box<EvalAltResult>> { Err(sleep()) },
    );
    engine.register_fn(
        "sleep",
        move |_: FLOAT| -> Result<Dynamic, Box<EvalAltResult>> { Err(sleep()) },
    );
    engine.register_fn(
        "sleep",
        move |_: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> { Err(sleep()) },
    );
    let exit = || {
        stub_error(
            "exit is unavailable",
            "end with complete(value) or pause(kind, message)",
        )
    };
    engine.register_fn("exit", move || -> Result<Dynamic, Box<EvalAltResult>> {
        Err(exit())
    });
    engine.register_fn(
        "exit",
        move |_: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> { Err(exit()) },
    );
    engine
}
