//! The host functions a script calls (L2 section 4.7): `agent`, `parallel`, `budget`, `pause`, `await_user` and `complete`. Each
//! journaled call takes the next call index of the replay and ends the step when it cannot
//! answer yet.
use crate::errors::{WORKFLOW_INVALID_INPUT, WORKFLOW_LAUNCH_FAILED, WORKFLOW_POLICY_DENIED};
use crate::fail;
use crate::helpers;
use crate::journal::{Journal, await_id, corr_id, need_id, out_id};
use crate::options;
use crate::output_schema::{self, MAX_CORRECTIONS};
use crate::progress::{capped, pause_reason};
use crate::replay::{HostFn, Sequencer, SlotKey, resolve, slot_id, window};
use crate::sandbox::{self, Suspension, SuspensionCell, infrastructure_error};
use crate::scratch;
use crate::wire::{Child, Spawn};
use jsonschema::Validator;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Phase titles a replay keeps; the progress frame is capped at 16 KiB anyway.
const MAX_PHASES: usize = 64;

/// What one replay step holds while the script runs.
pub struct Host {
    pub journal: Journal,
    pub children: BTreeMap<String, Child>,
    /// `meta.name`, for the divergence message's next call.
    pub name: String,
    /// `agent()` calls the replay reached: the budget's `spent`.
    pub spent: u64,
    /// The current pin's `agent_budget`: the budget's `total`.
    pub budget: u64,
    /// The run's address `session_id:tool_call_id`, or its handle when the input has no caller.
    pub address: String,
    /// What a step-0 suspend message names beside its text: the run's address and handle. Empty
    /// on later steps, where the host has already adopted the run.
    pub tag: String,
    /// Scratch records created in this replay, `{name, content}`; terminals carry them.
    pub scratch: Vec<Value>,
    /// The answer this row carries (`control.payload.answer`), until an `await_user` consumes it.
    pub answer: Option<String>,
    /// `job.row_doc_id`: the row an `await:<c>` note records as its consumer.
    pub row: Option<String>,
    /// The names this replay wrote, for the per-row file cap.
    pub scratch_names: BTreeSet<String>,
    /// `JobStepInput.scratch`: the host's path for each file an earlier row wrote.
    pub host_scratch: BTreeMap<String, String>,
    /// `<caller.cwd>/.gents/workflows/runs/<caller.tool_call_id>`; `None` when the input has no caller.
    pub run_dir: Option<String>,
    /// The spawn intents of the open window; the step re-emits them whole.
    pub spawns: Vec<Value>,
    /// `ceiling.targets`; empty when the input carries none, which allows no target.
    pub targets: Vec<String>,
    /// `ceiling.max_live`; 0 when the input carries none, which admits no panel item.
    pub max_live: u64,
    /// The latest line for the run's log, such as an `agent_type` override or a `log()` call:
    /// the progress `last_event`.
    pub last_event: Option<String>,
    /// Phase titles in first-seen order, at most [`MAX_PHASES`]: from `phase()` calls and agent
    /// `phase` options.
    pub phases: Vec<String>,
    /// The title of the latest `phase()` call.
    pub phase: Option<String>,
    /// An engine refusal (replay divergence), returned as the step's `fail`.
    pub refusal: Option<Value>,
    /// `ceiling.max_messages_per_slot`; 0 when the input carries none, which allows no message.
    pub max_messages: u64,
    /// `ceiling.remaining_spawns`; 0 when the input carries none, which refuses a fresh panel.
    pub remaining_spawns: u64,
    /// The `fail` the first `agent()` or `parallel()` gets under `dynamic: check`.
    pub spawn_denied: Option<Value>,
    /// The `message {slot, text}` effects this replay created: corrections and `resume_from`.
    pub messages: Vec<Value>,
    /// What each slot's `agent()` call returned in this replay, `(output, success)`.
    consumed: BTreeMap<String, (Value, bool)>,
    /// The slots a `resume_from` call re-targeted in this replay.
    resumed: BTreeSet<String>,
    seq: Sequencer,
}

impl Host {
    /// Notes a phase title, once, while the list is under [`MAX_PHASES`].
    fn see_phase(&mut self, title: &str) -> String {
        let title = capped(title, "phase");
        if self.phases.len() < MAX_PHASES && !self.phases.contains(&title) {
            self.phases.push(title.clone());
        }
        title
    }

    pub fn new(journal: Journal, children: BTreeMap<String, Child>, name: &str) -> Self {
        Host {
            journal,
            children,
            name: name.into(),
            spent: 0,
            budget: 0,
            address: String::new(),
            tag: String::new(),
            scratch: Vec::new(),
            answer: None,
            row: None,
            scratch_names: BTreeSet::new(),
            host_scratch: BTreeMap::new(),
            run_dir: None,
            spawns: Vec::new(),
            targets: Vec::new(),
            max_live: 0,
            last_event: None,
            phases: Vec::new(),
            phase: None,
            refusal: None,
            max_messages: 0,
            remaining_spawns: 0,
            spawn_denied: None,
            messages: Vec::new(),
            consumed: BTreeMap::new(),
            resumed: BTreeSet::new(),
            seq: Sequencer::default(),
        }
    }
}

const KINDS: &str =
    "user, back_off, no_progress, verification, infra, blocked, backoff, budget_exceeded";

pub type Shared = Rc<RefCell<Host>>;

/// The longest prefix of `text` within `max` bytes that ends on a char boundary.
pub fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}

/// `ChildState` is live | completed | failed | interrupted | other; the fake host's `running`
/// is live too.
pub fn terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "interrupted" | "other")
}

fn pending(cell: &SuspensionCell) -> Box<EvalAltResult> {
    sandbox::suspend(cell, Suspension::Pending)
}

/// The `(slot, attempt)` spawn intents a slot has passed: one per attempt (the first is 0), re-attempts included.
fn intents(h: &Host, slot: &str) -> u64 {
    h.children.get(slot).map_or(1, |c| c.attempt + 1)
}

/// An `agent()` or panel past `total` admits nothing and pauses the run, resumable with more.
fn budget_exceeded(h: &mut Host, charge: u64, cell: &SuspensionCell) -> Box<EvalAltResult> {
    // The budget the blocked step needs, for the resume that must raise it that far.
    if let Some(row) = h.row.clone() {
        h.journal.put(&need_id(&row), json!(h.spent + charge));
    }
    let message = format!(
        "{}/{}. Next: workflow resume {} --budget <n>{}",
        h.spent, h.budget, h.address, h.tag
    );
    let kind = "budget_exceeded".into();
    sandbox::suspend(cell, Suspension::Pause { kind, message })
}

/// Refuses a spawn the dynamic policy forbids: the step returns the refusal as its `fail`.
fn denied(h: &mut Host, cell: &SuspensionCell) -> Result<(), Box<EvalAltResult>> {
    match h.spawn_denied.clone() {
        Some(refusal) => {
            h.refusal = Some(refusal);
            Err(pending(cell))
        }
        None => Ok(()),
    }
}

/// A catchable `workflow_invalid_input` for an agent option.
fn invalid(what: &str) -> Box<EvalAltResult> {
    infrastructure_error(WORKFLOW_INVALID_INPUT, what, "workflow help agent")
}

/// An agent's `output_schema` option, compiled, with the value that keys its slot.
struct Schema {
    value: Value,
    validator: Validator,
}

fn schema_of(opts: &Map) -> Result<Option<Schema>, Box<EvalAltResult>> {
    let Some(raw) = opts.get("output_schema").filter(|v| !v.is_unit()) else {
        return Ok(None);
    };
    let value: Value = rhai::serde::from_dynamic(raw)?;
    let validator = output_schema::compile(&value).map_err(|what| invalid(&what))?;
    Ok(Some(Schema { value, validator }))
}

/// A terminal child's `(output, success)`, or `None` when the call still waits: no terminal
/// child yet, or a correction was just queued. The slot's `out:` note, when there is one,
/// is the value already consumed. With a schema the output is the validated value; a bad
/// reply gets up to `MAX_CORRECTIONS` uncharged messages, then fails.
fn read(
    h: &mut Host,
    slot: &str,
    child: Option<&Child>,
    validator: Option<&Validator>,
) -> Option<(Value, bool)> {
    let child = child?;
    if let Some(note) = h.journal.get(&out_id(slot)) {
        return Some((
            note["value"].clone(),
            note["success"].as_bool().unwrap_or(false),
        ));
    }
    if !terminal(&child.state) {
        return None;
    }
    let text = h.journal.child_text.get(slot).cloned().unwrap_or_default();
    let got = match (validator, child.state == "completed") {
        (Some(validator), true) => match output_schema::check(validator, &text) {
            Ok(value) => (value, true),
            Err(first) => correct(h, slot, &first)?,
        },
        (_, completed) => (json!(text), completed),
    };
    h.consumed.insert(slot.into(), got.clone());
    Some(got)
}

/// Queues the next correction for `slot` (`corr:<slot>#<n>` note plus a message) and returns
/// `None`, or, with none left, the failed read.
fn correct(h: &mut Host, slot: &str, first: &str) -> Option<(Value, bool)> {
    let sent = (1..=MAX_CORRECTIONS)
        .filter(|n| h.journal.get(&corr_id(slot, *n)).is_some())
        .count() as u64;
    if sent >= MAX_CORRECTIONS || sent >= h.max_messages {
        return Some((json!(output_schema::gave_up(first)), false));
    }
    let text = output_schema::correction(first);
    h.journal.put(
        &corr_id(slot, sent + 1),
        json!(output_schema::digest(&text)),
    );
    h.messages.push(json!({"slot": slot, "text": text}));
    None
}

/// `agent(prompt, #{resume_from: agent_id})`: sends `text` into the session an earlier
/// `agent()` call of this run read. The value that call consumed goes into `out:<slot>`
/// first, in the same output as the message; that call then replays from the note, and this
/// one returns the slot's latest terminal once its request is not the one the note covers.
/// One resume per slot: a second one is refused. It also counts against
/// `max_messages_per_slot` with the slot's corrections, as the host counts every message.
fn resume(
    h: &mut Host,
    cell: &SuspensionCell,
    from: &str,
    text: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let found = (h.children.iter())
        .find(|(_, c)| c.session_id == from)
        .map(|(slot, c)| (slot.clone(), c.clone()));
    let Some((slot, child)) = found else {
        return Err(invalid("resume_from names no agent this run started"));
    };
    if !h.resumed.insert(slot.clone()) {
        return Err(invalid("resume_from can re-target an agent once per run"));
    }
    if let Some(note) = h.journal.get(&out_id(&slot)) {
        if terminal(&child.state) && note["request_id"] != json!(child.request_id) {
            let text = h.journal.child_text.get(&slot).cloned().unwrap_or_default();
            return agent_result(&child, json!(text), child.state == "completed");
        }
        return Err(pending(cell));
    }
    let Some((value, success)) = h.consumed.get(&slot).cloned() else {
        return Err(invalid(
            "resume_from names an agent this script has not read yet",
        ));
    };
    let corrections = (1..=MAX_CORRECTIONS)
        .filter(|n| h.journal.get(&corr_id(&slot, *n)).is_some())
        .count() as u64;
    if corrections >= h.max_messages {
        return Err(invalid(
            "resume_from passes ceiling.max_messages_per_slot for this agent",
        ));
    }
    let note = json!({"value": value, "success": success, "request_id": child.request_id});
    h.journal.put(&out_id(&slot), note);
    h.messages.push(json!({"slot": slot, "text": text}));
    Err(pending(cell))
}

fn agent(
    host: &Shared,
    cell: &SuspensionCell,
    prompt: &str,
    opts: &Map,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    denied(&mut h, cell)?;
    let mapped = options::map(opts, &h.targets)?;
    let schema = schema_of(opts)?;
    let resume_from = match opts.get("resume_from") {
        None => None,
        Some(v) => match v.clone().try_cast::<ImmutableString>() {
            Some(from) => Some(from),
            None => return Err(invalid("resume_from must be a string")),
        },
    };
    h.last_event = mapped.note.or(h.last_event.take());
    if let Some(phase) = &mapped.phase {
        h.see_phase(phase);
    }
    let c = h.seq.call(HostFn::Agent).expect("agent takes a call index");
    if let Some(from) = resume_from {
        if schema.is_some() {
            return Err(invalid("output_schema cannot be combined with resume_from"));
        }
        return resume(&mut h, cell, &from, prompt);
    }
    let key = SlotKey {
        target: json!(&mapped.target),
        prompt: prompt.into(),
        title: (mapped.title)
            .unwrap_or_else(|| clip(prompt.lines().next().unwrap_or_default(), 256)),
        cwd: mapped.cwd,
        schema: schema.as_ref().map(|s| s.value.clone()),
    };
    let slot = slot_id(c, None, &key);
    let charge = intents(&h, &slot);
    if h.spent + charge > h.budget {
        return Err(budget_exceeded(&mut h, charge, cell));
    }
    h.spent += charge;
    let resolved = resolve(&h.children, &c.to_string(), &slot, &h.name).map(|c| c.cloned());
    let child = match resolved {
        Ok(child) => child,
        Err(fail) => {
            h.refusal = Some(fail);
            return Err(pending(cell));
        }
    };
    // A missing or live child gets its spawn (re-)emitted: the open window is served whole.
    let validator = schema.as_ref().map(|s| &s.validator);
    let Some((output, success)) = read(&mut h, &slot, child.as_ref(), validator) else {
        let refused = match &child {
            None if h.remaining_spawns == 0 => Some((
                WORKFLOW_POLICY_DENIED,
                "the agent needs 1 spawn and ceiling.remaining_spawns is 0",
            )),
            None if h.max_live == 0 => Some((
                WORKFLOW_LAUNCH_FAILED,
                "ceiling.max_live is 0, so the agent cannot start",
            )),
            _ => None,
        };
        if let Some((code, what)) = refused {
            let next = format!("{what}. Next: workflow status {}", h.address);
            h.refusal = Some(fail(code, &next));
            return Err(pending(cell));
        }
        let attempt = h.children.get(&slot).map_or(0, |c| c.attempt);
        let sent = spawn_prompt(prompt, &schema);
        emit(&mut h, slot, attempt, &sent, mapped.target, key);
        return Err(pending(cell));
    };
    agent_result(&child.expect("a read child exists"), output, success)
}

/// The prompt a spawn carries: `prompt`, plus the output contract when there is a schema.
fn spawn_prompt(prompt: &str, schema: &Option<Schema>) -> String {
    schema.as_ref().map_or_else(
        || prompt.into(),
        |s| output_schema::with_contract(prompt, &s.value),
    )
}

/// The map `agent()` returns for a terminal child.
fn agent_result(
    child: &Child,
    output: Value,
    success: bool,
) -> Result<Dynamic, Box<EvalAltResult>> {
    rhai::serde::to_dynamic(json!({
        "agent_id": child.session_id, "success": success, "output": output,
        "cancelled": child.state == "interrupted", "tokens_used": child.usage.total,
        "duration_ms": child.duration_ms
    }))
}

/// Holds the spawn intent of a slot; the step re-emits it until the child is terminal.
fn emit(h: &mut Host, slot: String, attempt: u64, prompt: &str, target: String, key: SlotKey) {
    let intent = Spawn {
        slot,
        attempt,
        target,
        prompt: prompt.into(),
        title: key.title,
        cwd: key.cwd,
    };
    h.spawns
        .push(serde_json::to_value(intent).expect("spawn serializes"));
}

/// One item of a panel, mapped and keyed, with the latest child of its slot.
struct Item {
    slot: String,
    prompt: String,
    target: String,
    key: SlotKey,
    /// `(output, success)` once the item's child is terminal and its reply is settled.
    done: Option<(Value, bool)>,
    child: Option<Child>,
}

/// `parallel([opts...])`: one call index, items `c.i`. It emits the admitted window whole
/// (items `[0, t + live_room)`, so each output holds the earlier ones) and is a barrier:
/// it returns only when every item is terminal, `()` in place of any that did not succeed.
fn parallel(
    host: &Shared,
    cell: &SuspensionCell,
    panel: Array,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    denied(&mut h, cell)?;
    let mut mapped = Vec::new();
    for item in panel {
        let opts = item.try_cast::<Map>().ok_or_else(|| {
            infrastructure_error(
                WORKFLOW_INVALID_INPUT,
                "parallel items must be maps",
                "workflow help host",
            )
        })?;
        let prompt = opts
            .get("prompt")
            .and_then(|p| p.clone().try_cast::<ImmutableString>());
        let Some(prompt) = prompt else {
            return Err(infrastructure_error(
                WORKFLOW_INVALID_INPUT,
                "every parallel item needs a prompt string",
                "workflow help host",
            ));
        };
        let opts_mapped = options::map(&opts, &h.targets)?;
        let schema = schema_of(&opts)?;
        h.last_event = opts_mapped.note.clone().or(h.last_event.take());
        if let Some(phase) = &opts_mapped.phase {
            h.see_phase(phase);
        }
        mapped.push((prompt.to_string(), opts_mapped, schema));
    }
    let c = h
        .seq
        .call(HostFn::Parallel)
        .expect("parallel takes a call index");
    let slotted: Vec<_> = (mapped.into_iter().enumerate())
        .map(|(i, (prompt, m, schema))| {
            let key = SlotKey {
                target: json!(&m.target),
                title: (m.title.clone())
                    .unwrap_or_else(|| clip(prompt.lines().next().unwrap_or_default(), 256)),
                prompt: prompt.clone(),
                cwd: m.cwd.clone(),
                schema: schema.as_ref().map(|s| s.value.clone()),
            };
            let slot = slot_id(c, Some(i as u64), &key);
            (prompt, m, schema, key, slot)
        })
        .collect();
    let charge: u64 = slotted.iter().map(|s| intents(&h, &s.4)).sum();
    if h.spent + charge > h.budget {
        return Err(budget_exceeded(&mut h, charge, cell));
    }
    h.spent += charge;
    let mut items = Vec::new();
    for (i, (prompt, m, schema, key, slot)) in slotted.into_iter().enumerate() {
        match resolve(&h.children, &format!("{c}.{i}"), &slot, &h.name) {
            Ok(child) => {
                let child = child.cloned();
                let validator = schema.as_ref().map(|s| &s.validator);
                let done = read(&mut h, &slot, child.as_ref(), validator);
                items.push(Item {
                    prompt: spawn_prompt(&prompt, &schema),
                    slot,
                    target: m.target,
                    key,
                    done,
                    child,
                });
            }
            Err(fail) => {
                h.refusal = Some(fail);
                return Err(pending(cell));
            }
        }
    }
    let fresh = items.iter().all(|i| i.child.is_none());
    let left = h.remaining_spawns;
    if fresh && items.len() as u64 > left {
        let what = format!(
            "the panel needs {} spawns and ceiling.remaining_spawns is {left}",
            items.len()
        );
        h.refusal = Some(fail(
            WORKFLOW_POLICY_DENIED,
            &format!("{what}. Next: workflow status {}", h.address),
        ));
        return Err(pending(cell));
    }
    let t = items.iter().filter(|i| i.done.is_some()).count();
    let own_live = (items.iter())
        .filter(|i| i.child.as_ref().is_some_and(|c| !terminal(&c.state)))
        .count();
    let live = h.children.values().filter(|c| !terminal(&c.state)).count();
    let room = h.max_live.saturating_sub((live - own_live) as u64);
    if t < items.len() {
        let admitted = window(items.len(), t, room);
        // `admitted` counts the `t` settled items, so `admitted == t` means none can start.
        if admitted == t && live == 0 {
            let what = "ceiling.max_live is 0, so no panel item can start";
            h.refusal = Some(fail(
                WORKFLOW_LAUNCH_FAILED,
                &format!("{what}. Next: workflow status {}", h.address),
            ));
            return Err(pending(cell));
        }
        for item in items.into_iter().take(admitted) {
            let attempt = item.child.as_ref().map_or(0, |c| c.attempt);
            emit(
                &mut h,
                item.slot,
                attempt,
                &item.prompt,
                item.target,
                item.key,
            );
        }
        return Err(pending(cell));
    }
    let mut results = Vec::new();
    for item in &items {
        let child = item.child.as_ref().expect("a settled item has a child");
        let (output, success) = item.done.clone().expect("a settled item is read");
        results.push(match success {
            true => agent_result(child, output, true)?,
            false => Dynamic::UNIT,
        });
    }
    Ok(results.into())
}

/// Dynamic to serde_json; an integer past i64 is already a float in rhai.
fn complete(cell: &SuspensionCell, v: &Dynamic) -> Result<Dynamic, Box<EvalAltResult>> {
    let value: Value = rhai::serde::from_dynamic(v)?;
    Err(sandbox::suspend(cell, Suspension::Complete(value)))
}

/// `budget()`: takes a call index and journals nothing. `total` is the current pin's, so a
/// resume that raises the budget shows through.
fn budget(host: &Shared) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    h.seq
        .call(HostFn::Budget)
        .expect("budget takes a call index");
    rhai::serde::to_dynamic(json!({
        "total": h.budget, "spent": h.spent, "reserved": 0,
        "remaining": h.budget.saturating_sub(h.spent)
    }))
}

/// `pause(kind, msg)`: not journaled, so it fires again on every resume.
fn pause(
    host: &Shared,
    cell: &SuspensionCell,
    kind: &str,
    msg: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
    if pause_reason(kind).is_none() {
        return Err(infrastructure_error(
            WORKFLOW_INVALID_INPUT,
            &format!("unknown pause kind {kind}; known: {KINDS}"),
            "workflow help host",
        ));
    }
    let message = format!("{msg}{}", host.borrow().tag);
    let kind = kind.into();
    Err(sandbox::suspend(cell, Suspension::Pause { kind, message }))
}

/// `await_user(kind, msg)`: the text of an `await:<c>` note if there is one; else the answer of
/// an answer row that no earlier `await_user` of the row consumed, journaled as `await:<c>`;
/// else it suspends with `msg` as the question and the answer comes with a resume.
fn await_user(
    host: &Shared,
    cell: &SuspensionCell,
    kind: &str,
    msg: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    let c = (h.seq.call(HostFn::AwaitUser)).expect("await_user takes a call index");
    if let Some(note) = h.journal.get(&await_id(c)) {
        return Ok(note["text"].as_str().unwrap_or_default().into());
    }
    let row = h.row.clone();
    let spent = (0..c).any(|i| {
        let noted = h.journal.get(&await_id(i));
        noted.is_some_and(|n| row.is_some() && n["row"].as_str() == row.as_deref())
    });
    if let (Some(answer), Some(row), false) = (h.answer.clone(), row, spent) {
        h.journal
            .put(&await_id(c), json!({"text": answer, "row": row}));
        return Ok(answer.into());
    }
    let message = format!("{msg}{}", h.tag);
    let ask = json!({"kind": kind, "message": message, "question": msg});
    Err(sandbox::suspend(cell, Suspension::AwaitUser(ask)))
}

/// `write_scratch_file(name, content)`: takes a call index; returns the file's path.
fn write_scratch_file(
    host: &Shared,
    name: &str,
    content: &str,
) -> Result<String, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    (h.seq.call(HostFn::WriteScratchFile)).expect("write_scratch_file takes a call index");
    scratch::write_scratch_file(&mut h, name, content)
}

/// `read_scratch_file(name)`: takes a call index; returns the file's content.
fn read_scratch_file(host: &Shared, name: &str) -> Result<String, Box<EvalAltResult>> {
    let mut h = host.borrow_mut();
    (h.seq.call(HostFn::ReadScratchFile)).expect("read_scratch_file takes a call index");
    scratch::read_scratch_file(&h, name)
}

/// Pure: `phase(title)` makes `title` the active phase.
fn phase(host: &Shared, title: &str) {
    let mut h = host.borrow_mut();
    h.phase = Some(h.see_phase(title));
}

/// Pure: `log(msg)` is the progress `last_event` until the next one.
fn log(host: &Shared, msg: &str) {
    host.borrow_mut().last_event = Some(capped(msg, "event"));
}

pub fn register(engine: &mut Engine, host: &Shared, cell: &SuspensionCell) {
    let (h, c) = (Rc::clone(host), Rc::clone(cell));
    engine.register_fn("agent", move |prompt: ImmutableString| {
        agent(&h, &c, &prompt, &Map::new())
    });
    let (h, c) = (Rc::clone(host), Rc::clone(cell));
    engine.register_fn("agent", move |prompt: ImmutableString, options: Map| {
        agent(&h, &c, &prompt, &options)
    });
    let (h, c) = (Rc::clone(host), Rc::clone(cell));
    engine.register_fn("parallel", move |panel: Array| parallel(&h, &c, panel));
    let h = Rc::clone(host);
    engine.register_fn("phase", move |title: ImmutableString| phase(&h, &title));
    let h = Rc::clone(host);
    engine.register_fn("log", move |msg: ImmutableString| log(&h, &msg));
    let h = Rc::clone(host);
    engine.register_fn("budget", move || budget(&h));
    let (h, c) = (Rc::clone(host), Rc::clone(cell));
    engine.register_fn(
        "pause",
        move |kind: ImmutableString, msg: ImmutableString| pause(&h, &c, &kind, &msg),
    );
    let (h, c) = (Rc::clone(host), Rc::clone(cell));
    engine.register_fn(
        "await_user",
        move |kind: ImmutableString, msg: ImmutableString| await_user(&h, &c, &kind, &msg),
    );
    let h = Rc::clone(host);
    engine.register_fn(
        "write_scratch_file",
        move |name: ImmutableString, content: ImmutableString| {
            write_scratch_file(&h, &name, &content)
        },
    );
    let h = Rc::clone(host);
    engine.register_fn("read_scratch_file", move |name: ImmutableString| {
        read_scratch_file(&h, &name)
    });
    engine.register_fn("fingerprint", |text: ImmutableString| {
        helpers::fingerprint(&text)
    });
    engine.register_fn("json_encode", |v: Dynamic| helpers::json_encode(&v));
    engine.register_fn("render_template", |name: ImmutableString, map: Map| {
        helpers::render_template(&name, &map)
    });
    engine.register_fn("git_diff_since", |rev: ImmutableString| {
        helpers::git_diff_since(&rev)
    });
    let c = Rc::clone(cell);
    engine.register_fn("complete", move |v: Dynamic| complete(&c, &v));
}
