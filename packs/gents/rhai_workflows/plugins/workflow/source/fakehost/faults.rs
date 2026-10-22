//! The faults of L2 section 4.22, each injectable from a test, and the determinism helper.
use super::adopt::Stepped;
use super::oracle::Answer;
use super::{Call, FakeHost, HostFacts, Kind};
use serde_json::{Value, json};

/// `resume` block that raises the run's budget.
pub fn resume_with_budget(row: &str, budget: u64) -> Value {
    json!({"row_doc_id": row, "budget": budget})
}

/// `resume` block for an engine digest change: continue on the current engine.
pub fn engine_changed(row: &str) -> Value {
    json!({"row_doc_id": row, "engine": "current"})
}

/// `host` with the ceiling fields in `patch` replaced: the ceiling narrowed mid-run.
pub fn narrowed(host: &HostFacts, patch: Value) -> HostFacts {
    let mut out = host.clone();
    for (k, v) in patch.as_object().expect("the patch is an object") {
        out.ceiling[k.as_str()] = v.clone();
    }
    out
}

impl FakeHost {
    /// Crash: the host forgets what it held in memory (step failure counts, the observed
    /// children) and keeps the durable maps, which it carries on from.
    pub fn crash(&mut self) {
        for chain in self.jobs.chains.values_mut() {
            chain.failures = 0;
            chain.observed.clear();
        }
    }

    /// The same step delivered twice. The row must still be running after the first.
    pub fn duplicate_step(
        &mut self,
        call: &Call,
        row: &str,
        mut guest: impl FnMut(Value) -> Value,
    ) -> Result<[Stepped; 2], String> {
        let first = self.step(call, row, &mut guest)?;
        Ok([first, self.step(call, row, &mut guest)?])
    }

    pub fn fail_child(&mut self, slot: &str, attempt: u64, text: &str) -> Result<(), String> {
        self.answer_child(slot, attempt, Answer::failed(text))
    }

    pub fn interrupt_child(&mut self, slot: &str, attempt: u64) -> Result<(), String> {
        self.answer_child(slot, attempt, Answer::interrupted())
    }

    /// A child whose request the inference queue refused: a failed terminal.
    pub fn queue_full(&mut self, slot: &str, attempt: u64) -> Result<(), String> {
        self.fail_child(slot, attempt, "QueueFull")
    }

    /// A non-job `stop` of `row`, possibly mid-panel (children live).
    pub fn stop_row(&mut self, host: &HostFacts, row: &str) -> Result<(), String> {
        self.control(host, row, "stop")
    }

    /// A non-job `pause` of `row`, possibly mid-panel.
    pub fn pause_row(&mut self, host: &HostFacts, row: &str) -> Result<(), String> {
        self.control(host, row, "pause")
    }

    fn control(&mut self, host: &HostFacts, row: &str, kind: &str) -> Result<(), String> {
        let call = Call {
            kind: Kind::ToolCall,
            input: json!({"argv": [kind]}),
            host: host.clone(),
        };
        let out = json!({"result": "ok", "control": [{"row_doc_id": row, "kind": kind}]});
        self.tool_call(&call, |_| out.clone()).map(|_| ())
    }

    /// P2P twin notes: the same `(chain_root_doc_id, note_id)` held under several `_docID`s,
    /// as two nodes' unique indexes allow. `twins` is `(doc_id, value_json)`, stored in order.
    pub fn twin_notes(&mut self, chain_root: &str, note_id: &str, twins: &[(&str, &str)]) {
        let rows = self.state.collections.entry("WfNote".into()).or_default();
        for (doc_id, value) in twins {
            rows.push(json!({"_docID": doc_id, "chain_root_doc_id": chain_root,
                             "note_id": note_id, "value_json": value}));
        }
    }
}

/// Runs `case` on a fresh host twice, then three times with `crash = true` (the case crashes
/// the host between its steps); every run must return the same effect trace.
pub fn deterministic(
    case: impl Fn(&mut FakeHost, bool) -> Vec<Value>,
) -> Result<Vec<Value>, String> {
    let run = |crash| case(&mut FakeHost::default(), crash);
    let first = run(false);
    for (n, crash) in [(2, false), (3, true), (4, true), (5, true)] {
        let again = run(crash);
        if again != first {
            return Err(format!(
                "run {n} (crash between steps: {crash}) differs from run 1:\n{first:#?}\n{again:#?}"
            ));
        }
    }
    Ok(first)
}
