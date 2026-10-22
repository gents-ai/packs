//! The smoke run behind `check` (L2 section 4.5, step 5). The script runs under a canned
//! oracle that lives in the engine, since `check` runs in wasm and not on the fake host: every
//! `agent` returns success with the minimal instance of its `output_schema`, or `ok`. The run
//! stops at the first terminal or after [`MAX_CALLS`] host calls.
use crate::errors::WORKFLOW_INVALID_INPUT;
use crate::host_api::{self, Host, Shared};
use crate::journal::Journal;
use crate::output_schema;
use crate::rounds::Replay;
use crate::run::DEFAULT_AGENT_BUDGET;
use crate::sandbox::{self, LogBuffer, Suspension, SuspensionCell, infrastructure_error};
use rhai::{Array, Dynamic, EvalAltResult, ImmutableString, Map, Scope};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

/// Host calls (`agent`, or one `parallel` panel) a smoke run makes before it stops.
pub const MAX_CALLS: u64 = 64;

/// How a smoke run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Complete,
    Pause,
    Question,
    CallLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Smoke {
    pub end: End,
    /// Host calls made: each `agent` call, and each `parallel` panel as one.
    pub calls: u64,
}

/// The smallest value a JSON schema accepts: required properties only, empty arrays and
/// strings, zero. ponytail: ignores min/max bounds and combinators; extend when a case needs it.
fn minimal(schema: &Value) -> Value {
    if let Some(v) = schema.get("const").or(schema.pointer("/enum/0")) {
        return v.clone();
    }
    let ty = match &schema["type"] {
        Value::String(t) => t.as_str(),
        Value::Array(ts) => ts.first().and_then(Value::as_str).unwrap_or_default(),
        _ if schema.get("properties").is_some() => "object",
        _ => "",
    };
    match ty {
        "object" => schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|k| (k.to_string(), minimal(&schema["properties"][k])))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        "array" => Value::Array(Vec::new()),
        "string" => "".into(),
        "integer" | "number" => 0.into(),
        "boolean" => false.into(),
        _ => Value::Null,
    }
}

/// What a canned `agent` returns for `opts`: the shape of a real result.
fn reply(opts: &Map) -> Result<Dynamic, Box<EvalAltResult>> {
    let output = match opts.get("output_schema").filter(|v| !v.is_unit()) {
        None => json!("ok"),
        Some(raw) => {
            let schema: Value = rhai::serde::from_dynamic(raw)?;
            output_schema::compile(&schema).map_err(|what| {
                infrastructure_error(WORKFLOW_INVALID_INPUT, &what, "workflow help check")
            })?;
            minimal(&schema)
        }
    };
    rhai::serde::to_dynamic(json!({
        "agent_id": "smoke", "success": true, "output": output,
        "cancelled": false, "tokens_used": 0, "duration_ms": 0
    }))
}

/// Counts one host call that spends `spent` agents, or ends the run at the limit.
fn count(
    calls: &Cell<u64>,
    host: &Shared,
    cell: &SuspensionCell,
    spent: u64,
) -> Result<(), Box<EvalAltResult>> {
    if calls.get() >= MAX_CALLS {
        return Err(sandbox::suspend(cell, Suspension::Pending));
    }
    calls.set(calls.get() + 1);
    host.borrow_mut().spent += spent;
    Ok(())
}

fn panel(items: Array) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut replies = Array::new();
    for item in items {
        let opts = item.try_cast::<Map>().ok_or_else(|| {
            infrastructure_error(
                WORKFLOW_INVALID_INPUT,
                "parallel items must be maps",
                "workflow help check",
            )
        })?;
        replies.push(reply(&opts)?);
    }
    Ok(replies.into())
}

/// Runs `source` with `args` under the canned oracle. The error says why the run failed.
pub fn run(source: &str, args: &Value) -> Result<Smoke, String> {
    let replay = Replay {
        notes: Vec::new(),
        source: None,
        child_text: BTreeMap::new(),
    };
    let mut host = Host::new(Journal::new(replay), BTreeMap::new(), "smoke");
    host.budget = DEFAULT_AGENT_BUDGET;
    // A scratch write needs a run directory; nothing is written.
    host.run_dir = Some("smoke".into());
    let host = Rc::new(RefCell::new(host));
    let (log, cell) = (LogBuffer::default(), SuspensionCell::default());
    let mut engine = sandbox::engine(&log, &cell, false);
    host_api::register(&mut engine, &host, &cell);

    // The same signatures as the real ones, so these replace them.
    let calls = Rc::new(Cell::new(0));
    let (n, h, c) = (Rc::clone(&calls), Rc::clone(&host), Rc::clone(&cell));
    engine.register_fn("agent", move |_: ImmutableString| {
        count(&n, &h, &c, 1)?;
        reply(&Map::new())
    });
    let (n, h, c) = (Rc::clone(&calls), Rc::clone(&host), Rc::clone(&cell));
    engine.register_fn("agent", move |_: ImmutableString, opts: Map| {
        count(&n, &h, &c, 1)?;
        reply(&opts)
    });
    let (n, h, c) = (Rc::clone(&calls), Rc::clone(&host), Rc::clone(&cell));
    engine.register_fn("parallel", move |items: Array| {
        count(&n, &h, &c, items.len() as u64)?;
        panel(items)
    });

    let args = rhai::serde::to_dynamic(args)
        .map_err(|e| format!("args do not convert to a script value ({e})"))?;
    let mut scope = Scope::new();
    scope.push_constant("args", args);
    let ran = engine.run_with_scope(&mut scope, source);
    let end = match (cell.borrow().clone(), ran) {
        (Some(Suspension::Complete(_)), _) => End::Complete,
        (Some(Suspension::Pause { .. }), _) => End::Pause,
        (Some(Suspension::AwaitUser(_)), _) => End::Question,
        (Some(Suspension::Pending), _) => End::CallLimit,
        (None, Ok(())) => return Err("the script ended without calling complete".into()),
        (None, Err(e)) => return Err(format!("the script stopped with an error ({e})")),
    };
    Ok(Smoke {
        end,
        calls: calls.get(),
    })
}
