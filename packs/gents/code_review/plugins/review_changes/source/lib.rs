//! review_changes: the review-changes workflow as a gents job plugin.
//!
//! One invocation is one job step: the host passes `JobStepInput` (its view of the run,
//! above all the `children` slots and their states) and this plugin answers with one
//! `JobStepOutput`. There is no script and no replay: the phase is read off the slots that
//! exist (`recon`, `scan-<lens>`, `verify-<i>`, `triage`) and each step either spawns the
//! next panel, sends a correction, waits, or completes. A child's text is read through one
//! `job_calls.child_output` round when a phase needs it.
pub mod output_schema;
mod review;
mod wire;

use review::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use wire::*;

/// The workflow's identity (name, description, when to use, version). Its sha256 is the
/// pin's `script_id` and `source_digest`; the host classifies a call without `argv`,
/// `script` or `path` as `installed` from the artifact's own provenance, so no file of
/// this name ships with the pack.
pub const META: &str = include_str!("workflow.json");
const DEFAULT_BUDGET: u64 = 128;
const MAX_BUDGET: u64 = 1024;
const HANDLE: &str = "review-changes";
const TITLE: &str = "Review a git diff through four lenses, adversarially verify every candidate, and triage";
const PHASES: [&str; 4] = ["Recon", "Scan", "Verify", "Triage"];
const REPORT: &str = "review.md";

/// The longest prefix of `text` within `max` bytes that ends on a char boundary.
pub fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}

/// A `fail` output.
pub fn fail(code: &str, message: &str) -> Value {
    json!({ "fail": { "code": code, "message": message } })
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// One step: the output for `input`, or a `fail` when the input is not a job step.
pub fn step(input: Value) -> Value {
    match run(input) {
        Ok(v) | Err(v) => v,
    }
}

fn out(o: JobStepOutput) -> Value {
    serde_json::to_value(o).expect("output serializes")
}

fn run(raw: Value) -> Result<Value, Value> {
    let input: JobStepInput = serde_json::from_value(raw)
        .map_err(|e| fail("workflow_invalid_input", &format!("the input is not a job step ({e})")))?;
    let texts = read_texts(&input)?;
    let budget = input
        .pin
        .as_ref()
        .map(|p| p.agent_budget)
        .or(input.agent_budget)
        .unwrap_or(DEFAULT_BUDGET);
    let carried = input
        .state
        .as_ref()
        .and_then(|state| serde_json::from_value(state.clone()).ok())
        .unwrap_or_default();
    let ctx = Ctx {
        args: Args::from(input.args.as_ref()),
        input,
        budget,
        texts,
        carried: RefCell::new(carried),
    };
    if ctx.input.pin.is_none() {
        return ctx.step0();
    }
    ctx.advance()
}

/// The previous round's `child_output` answer, keyed by slot.
fn read_texts(input: &JobStepInput) -> Result<BTreeMap<String, ChildOutputSlice>, Value> {
    let Some(results) = input.job_results.as_ref().and_then(|r| r.get("text")) else {
        return Ok(BTreeMap::new());
    };
    match serde_json::from_value::<RoundResult<ChildOutputResponse>>(results.clone()) {
        Ok(RoundResult::Result { result }) => Ok(result.slots),
        Ok(RoundResult::Error { error }) => Err(fail(
            "workflow_launch_failed",
            &format!("the host could not read a child's output ({error})"),
        )),
        Err(e) => Err(fail(
            "workflow_invalid_input",
            &format!("the child_output answer is not of the asked shape ({e})"),
        )),
    }
}

/// How one child's reply was judged against its output schema.
enum Judged {
    Ok(Value),
    Failed(String),
    /// Send the correction; the slot stays unjudged until its next reply.
    Correct(Message),
}

/// What this plugin carries between steps in the host's opaque `state`: the slots it has
/// judged (their parsed reply, or why they failed) and the corrections sent per slot. A judged
/// slot's text is never fetched again. A state another version wrote parses as empty.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Carried {
    judged: BTreeMap<String, Judgement>,
    corrections: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Judgement {
    Ok { output: Value },
    Failed { why: String },
}

struct Ctx {
    input: JobStepInput,
    args: Args,
    budget: u64,
    texts: BTreeMap<String, ChildOutputSlice>,
    carried: RefCell<Carried>,
}

impl Ctx {
    fn step0(&self) -> Result<Value, Value> {
        if !(1..=MAX_BUDGET).contains(&self.budget) {
            return Err(fail(
                "workflow_invalid_input",
                &format!("agent_budget must be between 1 and {MAX_BUDGET}"),
            ));
        }
        let digest = sha256_hex(META);
        let pin = JobPin {
            script_id: digest.clone(),
            source_digest: format!("sha256:{digest}"),
            agent_budget: self.budget,
            handle: HANDLE.into(),
            host: Default::default(),
        };
        if self.budget < MIN_BUDGET {
            return Ok(out(JobStepOutput::Suspend {
                suspend: Suspend {
                    kind: SuspendKind::Pause,
                    reason: Some(PauseReason::BudgetExceeded),
                    message: format!(
                        "paused before any agent ran: agent_budget {} is below the {MIN_BUDGET} this review needs (recon, four scans, up to {MAX_VERIFY} verifiers, triage). Nothing to do now; the operator decides whether to start a run with a larger budget.",
                        self.budget
                    ),
                },
                progress: self.progress(JobState::Paused, Some(PauseReason::BudgetExceeded), 0, "paused: agent_budget too low"),
                pin: Some(pin),
                state: None,
            }));
        }
        let recon = self.spawn("recon", RECON_TARGET, recon_prompt(&self.args.base, &self.args.head), recon_schema());
        Ok(self.effects(Some(pin), vec![recon], Vec::new(), Vec::new(), 0, "spawned recon"))
    }

    fn advance(&self) -> Result<Value, Value> {
        let kids = &self.input.children;
        let live = |slot: &str| kids.get(slot).is_some_and(Child::live);
        let any_live = |prefix: &str| kids.iter().any(|(k, c)| k.starts_with(prefix) && c.live());

        // Recon.
        if !kids.contains_key("recon") {
            let recon = self.spawn("recon", RECON_TARGET, recon_prompt(&self.args.base, &self.args.head), recon_schema());
            return Ok(self.spawn_or_wait(vec![recon], 0));
        }
        if live("recon") {
            return Ok(self.wait(0));
        }
        // One round fetches every completed child's text: the host echoes only the latest
        // round's answers, so asking phase by phase would drop the earlier texts and loop.
        self.need()?;
        let recon = match self.judge("recon", &recon_schema(), "recon") {
            Judged::Correct(m) => return Ok(self.correct(vec![m], 0)),
            Judged::Failed(why) => return Ok(self.blocked(&why)),
            Judged::Ok(o) => o,
        };
        let problem = recon_problem(&recon);
        if !problem.is_empty() {
            return Ok(self.blocked(&problem));
        }
        let pin = Pin::from_recon(&recon);
        let name_only = recon["name_only"].as_str().unwrap_or_default();
        let diff_id = fingerprint(&json_encode(&json!([pin.to_value(), name_only])));
        let paths = changed_paths(name_only);
        if paths.is_empty() != (pin.base_tree == pin.head_tree) {
            return Ok(self.blocked("recon's listings disagree with the pinned trees"));
        }
        if paths.is_empty() {
            let report = format!(
                "# Review\n\nNo changed paths between {} and {}.\n",
                pin.base_sha, pin.head_sha
            );
            let result = json!({
                "status": "complete",
                "base_sha": pin.base_sha,
                "head_sha": pin.head_sha,
                "diff_id": diff_id,
                "counts": empty_counts(),
                "findings": [],
                "report": { "report": "No changed paths.", "merge_recommendation": "none" },
                "limitations": [],
                "path": self.report_path(),
            });
            return Ok(self.complete(result, report));
        }

        // Scan: one slot per lens.
        let scan_slots: Vec<String> = LENSES.iter().map(scan_slot).collect();
        let planned: Vec<Spawn> = LENSES
            .iter()
            .map(|lens| self.spawn(&scan_slot(lens), SCAN_TARGET, scan_prompt(lens, &pin, &self.args.focus, &paths), scan_schema()))
            .collect();
        let pending: Vec<Spawn> = planned.into_iter().filter(|s| !kids.contains_key(&s.slot)).collect();
        if !pending.is_empty() || any_live("scan-") {
            return Ok(self.spawn_or_wait(pending, 1));
        }
        let scans = self.judge_all(&scan_slots, &scan_schema(), "scan", 1)?;
        let (candidates, mut limitations) = select_candidates(&scans, &paths);
        let all_blocked = limitations.len() == LENSES.len();
        let groups = group_candidates(&candidates);
        let sent = groups.len().min(MAX_VERIFY);

        // Verify: one slot per group, at most MAX_VERIFY.
        let verify_slots: Vec<String> = (0..sent).map(verify_slot).collect();
        let pending: Vec<Spawn> = (0..sent)
            .filter(|gi| !kids.contains_key(&verify_slot(*gi)))
            .map(|gi| self.spawn(&verify_slot(gi), VERIFY_TARGET, verify_prompt(&groups[gi], &pin), verify_schema()))
            .collect();
        if !pending.is_empty() || any_live("verify-") {
            return Ok(self.spawn_or_wait(pending, 2));
        }
        let verdicts = self.judge_all(&verify_slots, &verify_schema(), "verification", 2)?;
        let verified = verify_groups(&groups, &verdicts, &pin);
        limitations.extend(verified.limitations);
        let summary = counts_text(
            candidates.len(),
            verified.confirmed.len(),
            verified.refuted,
            verified.unverified.len(),
            verified.high_priority,
        );

        // Triage.
        if !kids.contains_key("triage") {
            let triage = self.spawn(
                "triage",
                TRIAGE_TARGET,
                triage_prompt(&pin, &summary, &verified.confirmed, &limitations),
                triage_schema(),
            );
            return Ok(self.spawn_or_wait(vec![triage], 3));
        }
        if live("triage") {
            return Ok(self.wait(3));
        }
        let (report, recommendation) = match self.judge("triage", &triage_schema(), "triage") {
            Judged::Correct(m) => return Ok(self.correct(vec![m], 3)),
            Judged::Ok(o) => (
                o["report"].as_str().unwrap_or_default().to_owned(),
                o["merge_recommendation"].as_str().unwrap_or_default().to_owned(),
            ),
            Judged::Failed(_) => {
                limitations.push("triage did not succeed".into());
                (
                    "Triage did not succeed; the confirmed findings below stand on their own.".to_owned(),
                    "none".to_owned(),
                )
            }
        };
        let markdown = render_report(&pin, &summary, &recommendation, &report, &verified.confirmed, &limitations);
        let result = json!({
            "status": if all_blocked { "blocked" } else { "complete" },
            "base_sha": pin.base_sha,
            "head_sha": pin.head_sha,
            "diff_id": diff_id,
            "counts": {
                "candidate": candidates.len(),
                "confirmed": verified.confirmed.len(),
                "refuted": verified.refuted,
                "unverified": verified.unverified.len(),
                "high_priority": verified.high_priority,
            },
            "findings": verified.confirmed,
            "report": { "report": report, "merge_recommendation": recommendation },
            "limitations": limitations,
            "path": self.report_path(),
        });
        Ok(self.complete(result, markdown))
    }

    /// Judges every slot; corrections, when any, are the step's output.
    fn judge_all(&self, slots: &[String], schema: &Value, what: &str, phase: usize) -> Result<Vec<Result<Value, String>>, Value> {
        let mut judged = Vec::new();
        let mut corrections: Vec<Message> = Vec::new();
        for slot in slots {
            match self.judge(slot, schema, what) {
                Judged::Ok(v) => judged.push(Ok(v)),
                Judged::Failed(why) => judged.push(Err(why)),
                Judged::Correct(m) => corrections.push(m),
            }
        }
        if !corrections.is_empty() {
            return Err(self.correct(corrections, phase));
        }
        Ok(judged)
    }

    /// One terminal slot's reply against `schema`: the carried judgement when the slot has
    /// one, else this input's text, judged and carried forward.
    fn judge(&self, slot: &str, schema: &Value, what: &str) -> Judged {
        if let Some(prior) = self.carried.borrow().judged.get(slot) {
            return match prior {
                Judgement::Ok { output } => Judged::Ok(output.clone()),
                Judgement::Failed { why } => Judged::Failed(why.clone()),
            };
        }
        let judged = self.judge_fresh(slot, schema, what);
        let mut carried = self.carried.borrow_mut();
        match &judged {
            Judged::Ok(output) => {
                carried.judged.insert(slot.into(), Judgement::Ok { output: output.clone() });
            }
            Judged::Failed(why) => {
                carried.judged.insert(slot.into(), Judgement::Failed { why: why.clone() });
            }
            Judged::Correct(_) => *carried.corrections.entry(slot.into()).or_insert(0) += 1,
        }
        judged
    }

    fn judge_fresh(&self, slot: &str, schema: &Value, what: &str) -> Judged {
        let Some(child) = self.input.children.get(slot) else {
            return Judged::Failed(format!("the {what} did not succeed"));
        };
        if child.state != "completed" {
            return Judged::Failed(format!("the {what} did not succeed"));
        }
        let Some(slice) = self.texts.get(slot) else {
            return Judged::Failed(format!("the {what} reply was not readable"));
        };
        if slice.truncated || child.truncated {
            return Judged::Failed(format!("the {what} reply exceeded result_bytes"));
        }
        let validator = output_schema::compile(schema).expect("a constant schema compiles");
        match output_schema::check(&validator, &slice.text) {
            Ok(v) => Judged::Ok(v),
            Err(first_error) => {
                let sent = self.carried.borrow().corrections.get(slot).copied().unwrap_or(0);
                if sent >= output_schema::MAX_CORRECTIONS {
                    return Judged::Failed(output_schema::gave_up(&first_error));
                }
                Judged::Correct(Message {
                    slot: slot.to_owned(),
                    text: output_schema::correction(&first_error),
                })
            }
        }
    }

    /// Asks the host, in one round, for the text of every completed child this input lacks
    /// and no carried judgement already covers.
    fn need(&self) -> Result<(), Value> {
        let carried = self.carried.borrow();
        let missing: Vec<String> = self
            .input
            .children
            .iter()
            .filter(|(slot, c)| {
                c.state == "completed"
                    && !self.texts.contains_key(*slot)
                    && !carried.judged.contains_key(*slot)
            })
            .map(|(slot, _)| slot.clone())
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let request = JobCall {
            id: "text".into(),
            child_output: ChildOutputRequest {
                slots: missing,
                offset: 0,
                limit: self.input.ceiling.result_bytes,
            },
        };
        Err(json!({ "job_calls": { "requests": [request], "state": { "rounds": 1 } } }))
    }

    /// A slot's first attempt is 0; re-attempts are the host's resume path, not this plugin's.
    fn spawn(&self, slot: &str, target: &str, prompt: String, schema: Value) -> Spawn {
        Spawn {
            slot: slot.to_owned(),
            attempt: 0,
            target: target.to_owned(),
            prompt: output_schema::with_contract(&prompt, &schema),
            title: slot.to_owned(),
            cwd: self.args.repository.clone(),
        }
    }

    /// Spawns as many of `pending` as the live ceiling allows; with nothing to spawn, waits on
    /// the live children, and with no room and nothing live, pauses on the spawn budget.
    fn spawn_or_wait(&self, pending: Vec<Spawn>, phase: usize) -> Value {
        if pending.is_empty() {
            return self.wait(phase);
        }
        let live = self.input.children.values().filter(|c| c.live()).count() as u64;
        let ceiling = &self.input.ceiling;
        let room = ceiling.max_live.saturating_sub(live).min(ceiling.remaining_spawns) as usize;
        let batch: Vec<Spawn> = pending.into_iter().take(room).collect();
        if batch.is_empty() {
            if live > 0 {
                return self.wait(phase);
            }
            return out(JobStepOutput::Suspend {
                suspend: Suspend {
                    kind: SuspendKind::Pause,
                    reason: Some(PauseReason::BudgetExceeded),
                    message: "paused: the spawn budget ran out before the review finished. Nothing to do now; the operator decides whether to start a run with a larger budget.".into(),
                },
                progress: self.progress(JobState::Paused, Some(PauseReason::BudgetExceeded), phase, "paused: spawn budget exhausted"),
                pin: None,
                state: self.state(),
            });
        }
        let event = format!("spawned {}", batch.iter().map(|s| s.slot.as_str()).collect::<Vec<_>>().join(", "));
        self.effects(None, batch, Vec::new(), Vec::new(), phase, &event)
    }

    fn wait(&self, phase: usize) -> Value {
        self.effects(None, Vec::new(), Vec::new(), Vec::new(), phase, "waiting for children")
    }

    fn correct(&self, corrections: Vec<Message>, phase: usize) -> Value {
        let slots: Vec<&str> = corrections.iter().map(|m| m.slot.as_str()).collect();
        let event = format!("correcting {}", slots.join(", "));
        self.effects(None, Vec::new(), corrections, Vec::new(), phase, &event)
    }

    /// The carried state for the next step.
    fn state(&self) -> Option<Value> {
        Some(serde_json::to_value(&*self.carried.borrow()).expect("state serializes"))
    }

    fn effects(&self, pin: Option<JobPin>, spawn: Vec<Spawn>, message: Vec<Message>, scratch: Vec<ScratchFile>, phase: usize, event: &str) -> Value {
        out(JobStepOutput::Effects {
            effects: Effects {
                pin,
                spawn: some(spawn),
                message: some(message),
                progress: Some(self.progress(JobState::Running, None, phase, event)),
                scratch: some(scratch),
                state: self.state(),
            },
        })
    }

    fn blocked(&self, reason: &str) -> Value {
        let (result, report) = review::blocked(reason, &self.report_path());
        self.complete(result, report)
    }

    fn complete(&self, result: Value, report: String) -> Value {
        out(JobStepOutput::Result {
            result,
            scratch: Some(vec![ScratchFile { name: REPORT.into(), content: report }]),
            progress: self.progress(JobState::Completed, None, PHASES.len(), "completed"),
        })
    }

    /// Where the host writes this row's scratch files.
    fn report_path(&self) -> String {
        let cwd = self
            .input
            .pin
            .as_ref()
            .and_then(|p| p.host.get("cwd"))
            .and_then(Value::as_str)
            .unwrap_or(&self.input.caller.cwd);
        format!("{cwd}/.gents/workflows/runs/{}/{REPORT}", self.input.caller.tool_call_id)
    }

    /// The progress frame: `phase` is the active phase index, or `PHASES.len()` when all are done.
    fn progress(&self, state: JobState, pause_reason: Option<PauseReason>, phase: usize, event: &str) -> JobProgress {
        let kids = &self.input.children;
        let spent = kids.len() as u64;
        JobProgress {
            title: TITLE.into(),
            state,
            pause_reason,
            phases: PHASES
                .iter()
                .enumerate()
                .map(|(i, title)| Phase {
                    title: (*title).into(),
                    state: if i < phase {
                        PhaseState::Done
                    } else if i == phase {
                        PhaseState::Active
                    } else {
                        PhaseState::Pending
                    },
                })
                .collect(),
            active: kids
                .iter()
                .filter(|(_, c)| c.live())
                .map(|(slot, c)| ActiveChild {
                    slot: slot.clone(),
                    title: c.title.clone(),
                    session_id: c.session_id.clone(),
                })
                .collect(),
            spawned: spent,
            budget: ProgressBudget {
                total: self.budget,
                spent,
                remaining: self.budget.saturating_sub(spent),
            },
            last_event: clip(event, 256),
            last_event_at: Value::Null,
            observed_terminal_children: kids.values().filter(|c| !c.live()).count() as u64,
            revision: 0,
        }
    }
}

fn some<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(children: Value, pin: bool) -> Value {
        let mut v = json!({
            "protocol": 1,
            "job": { "row_doc_id": "row", "chain_root_doc_id": "row", "chain": ["row"], "handle": "review-changes" },
            "caller": { "session_id": "s", "request_id": "r", "request_doc_id": "rd", "tool_call_id": "call_1",
                        "node_did": "did:key:z", "agent_id": "default", "request_hop": 1, "correlation": null,
                        "cwd": "/work", "started_by_job": false },
            "argv": [], "args": { "base": "HEAD~1", "head": "HEAD" }, "agent_budget": 32,
            "pin": null, "origin": null,
            "ceiling": { "targets": [], "max_live": 8, "remaining_spawns": 32, "max_messages_per_slot": 4,
                         "result_bytes": 65536, "dynamic": "check", "dynamic_targets": [] },
            "children": children, "control": null, "scratch": {}, "jobs": []
        });
        if pin {
            v["pin"] = json!({ "script_id": sha256_hex(META), "source_digest": format!("sha256:{}", sha256_hex(META)),
                                "agent_budget": 32, "handle": "review-changes", "cwd": "/work", "origin": "installed" });
            v["origin"] = json!("installed");
        }
        v
    }

    fn child(state: &str) -> Value {
        json!({ "attempt": 0, "session_id": "cs", "request_id": "cr", "title": "t", "state": state,
                "output_bytes": 10, "truncated": false, "usage": { "input": 1, "output": 1, "total": 2 }, "duration_ms": 5 })
    }

    #[test]
    fn step0_pins_and_spawns_recon() {
        let out = step(input(json!({}), false));
        let effects = &out["effects"];
        assert_eq!(effects["pin"]["source_digest"], format!("sha256:{}", sha256_hex(META)));
        assert_eq!(effects["spawn"][0]["slot"], "recon");
        assert_eq!(effects["spawn"][0]["target"], RECON_TARGET);
        assert!(effects["spawn"][0]["prompt"].as_str().unwrap().contains("<output-contract>"));
        assert_eq!(effects["progress"]["state"], "running");
    }

    #[test]
    fn step0_pauses_below_the_budget_floor() {
        let mut i = input(json!({}), false);
        i["agent_budget"] = json!(8);
        let out = step(i);
        assert_eq!(out["suspend"]["reason"], "budget_exceeded");
        assert_eq!(out["progress"]["pause_reason"], "budget_exceeded");
        assert!(out["pin"].is_object());
    }

    #[test]
    fn a_live_recon_only_reports_progress() {
        let out = step(input(json!({ "recon": child("live") }), true));
        let effects = out["effects"].as_object().unwrap();
        assert!(!effects.contains_key("spawn") && !effects.contains_key("message"));
        assert_eq!(effects["progress"]["active"][0]["slot"], "recon");
    }

    #[test]
    fn a_completed_recon_without_text_asks_for_it() {
        let out = step(input(json!({ "recon": child("completed") }), true));
        assert_eq!(out["job_calls"]["requests"][0]["child_output"]["slots"], json!(["recon"]));
        assert_eq!(out["job_calls"]["requests"][0]["child_output"]["limit"], 65536);
    }

    #[test]
    fn a_bad_recon_reply_is_corrected_then_blocks() {
        let mut i = input(json!({ "recon": child("completed") }), true);
        i["job_results"] = json!({ "text": { "result": { "slots": { "recon": { "text": "no json here" } } } } });
        let out = step(i.clone());
        assert_eq!(out["effects"]["message"][0]["slot"], "recon");
        assert!(out["effects"].get("scratch").is_none());
        assert_eq!(out["effects"]["state"]["corrections"]["recon"], 1);
        i["state"] = json!({ "corrections": { "recon": 2 } });
        let out = step(i);
        assert_eq!(out["result"]["status"], "blocked");
        assert_eq!(out["result"]["path"], "/work/.gents/workflows/runs/call_1/review.md");
        assert_eq!(out["scratch"][0]["name"], "review.md");
    }

    #[test]
    fn a_good_recon_spawns_four_scans() {
        let sha = "a".repeat(40);
        let tree = "b".repeat(40);
        let recon = json!({ "base_sha": sha, "head_sha": "c".repeat(40), "base_tree": tree, "head_tree": "d".repeat(40),
            "name_only": "src/lib.rs\n", "stat": " src/lib.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n" });
        let mut i = input(json!({ "recon": child("completed") }), true);
        i["job_results"] = json!({ "text": { "result": { "slots": { "recon": { "text": format!("```json\n{recon}\n```") } } } } });
        let out = step(i);
        let spawns = out["effects"]["spawn"].as_array().unwrap();
        assert_eq!(spawns.len(), 4);
        assert_eq!(spawns[0]["slot"], "scan-correctness");
        assert_eq!(out["effects"]["progress"]["phases"][1]["state"], "active");
    }

    #[test]
    fn a_judged_slot_is_not_fetched_again() {
        let sha = "a".repeat(40);
        let recon = json!({ "base_sha": sha, "head_sha": "c".repeat(40), "base_tree": "b".repeat(40), "head_tree": "d".repeat(40),
            "name_only": "src/lib.rs\n", "stat": " src/lib.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n" });
        let mut i = input(json!({ "recon": child("completed") }), true);
        i["state"] = json!({ "judged": { "recon": { "ok": { "output": recon } } } });
        let out = step(i);
        assert!(out.get("job_calls").is_none(), "no text round for a judged slot: {out}");
        assert_eq!(out["effects"]["spawn"].as_array().unwrap().len(), 4);
        assert_eq!(out["effects"]["state"]["judged"]["recon"]["ok"]["output"]["base_sha"], sha);
    }
}
