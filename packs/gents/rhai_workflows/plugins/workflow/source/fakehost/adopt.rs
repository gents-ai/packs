//! Adoption, chains and controls of the fake host (root C1, C2, C8; L2 section 4.22). Not a
//! conformance oracle.
use super::admit::{self, Denied};
use super::state::JobRow;
use super::{Call, FakeHost, HostFacts, Kind};
use crate::wire::{JobPin, JobState};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A chain of rows (a run and its resumes) sharing one root, one handle and the children.
#[derive(Debug, Clone, Default)]
pub struct Chain {
    pub rows: Vec<String>,
    pub head: String,
    pub handle: String,
    pub args_digest: String,
    /// The chain root's arguments, copied onto every resume.
    pub root_call: Value,
    /// Consumed resume answers, in order.
    pub journal: Vec<Value>,
    pub scratch: BTreeMap<String, String>,
    pub keys: BTreeSet<(String, u64)>,
    /// Terminal children already shown to the guest.
    pub observed: BTreeSet<(String, u64)>,
    pub charged: u64,
    pub messages: BTreeMap<String, u64>,
    pub failures: u32,
}

/// What the host knows about a row beyond `JobRow`.
#[derive(Debug, Clone)]
pub struct RowInfo {
    pub session_id: String,
    pub origin: Value,
    /// The echoed `control` block, null unless the row resumes.
    pub control: Value,
    pub root_call: Option<Value>,
    /// Adoption order; the host's `adopted_at`.
    pub adopted_at: u64,
}

#[derive(Debug, Default)]
pub struct Jobs {
    pub chains: BTreeMap<String, Chain>,
    pub rows: BTreeMap<String, RowInfo>,
}

/// What the host did with one job-step output.
#[derive(Debug, Clone, PartialEq)]
pub struct Stepped {
    pub output: Value,
    /// The adopted or stepped row; `None` for a pinless step 0.
    pub row: Option<String>,
    /// The row is still running: a Continuation follows. A pinned terminal returns inline.
    pub continues: bool,
    /// Set when the host ended the row: `plugin_job_policy_denied`, `plugin_job_stalled` or
    /// `plugin_job_step_failed`.
    pub code: Option<String>,
}

const ROOT_KEYS: [&str; 4] = ["argv", "args", "path", "agent_budget"];

/// Exactly one of `result`, `fail`, `suspend`, `effects`.
fn shape(out: &Value) -> Result<(), String> {
    match ["result", "fail", "suspend", "effects"]
        .iter()
        .filter(|k| out.get(**k).is_some())
        .count()
    {
        1 => Ok(()),
        n => Err(format!(
            "an output with {n} of result, fail, suspend, effects"
        )),
    }
}

fn row_state(out: &Value) -> JobState {
    if out.get("result").is_some() {
        JobState::Completed
    } else if out.get("fail").is_some() {
        JobState::Failed
    } else if let Some(s) = out.get("suspend") {
        match s["kind"] == "await_user" {
            true => JobState::AwaitingUser,
            false => JobState::Paused,
        }
    } else {
        JobState::Running
    }
}

fn digest(v: &Value) -> String {
    format!("{:x}", Sha256::digest(v.to_string().as_bytes()))
}

/// `answer` > `engine_current` > `resume`, with the whole block as payload.
fn control_of(block: &Value) -> Value {
    let kind = match () {
        _ if block.get("answer").is_some() => "answer",
        _ if block["engine"] == "current" => "engine_current",
        _ => "resume",
    };
    json!({"kind": kind, "payload": block})
}

impl FakeHost {
    fn unique_handle(&self, session: &str, want: &str) -> String {
        let taken = |h: &str| {
            self.jobs
                .rows
                .iter()
                .any(|(id, i)| i.session_id == session && self.state.rows[id].handle == h)
        };
        let mut handle = want.to_string();
        let mut n = 1;
        while taken(&handle) {
            n += 1;
            handle = format!("{want}-{n}");
        }
        handle
    }

    fn end(&mut self, row: &str, state: JobState, result: Value) {
        let r = self.state.rows.get_mut(row).expect("known row");
        (r.state, r.result) = (state, Some(result));
    }

    /// Step 0 of a job call. A pin adopts a row (a resume beside it joins the prior chain);
    /// a pinless terminal is returned as is and adopts nothing.
    pub fn step0(
        &mut self,
        call: &Call,
        guest: impl FnMut(Value) -> Value,
    ) -> Result<Stepped, String> {
        let caller = &call.host.caller;
        let field = |k: &str| {
            caller[k]
                .as_str()
                .map(String::from)
                .ok_or(format!("caller has no {k}"))
        };
        let (session, tool_call) = (field("session_id")?, field("tool_call_id")?);
        let row = format!("row-{tool_call}");
        let mut host = call.host.clone();
        host.job = Some(
            json!({"row_doc_id": row, "chain_root_doc_id": row, "chain": [row], "handle": ""}),
        );
        host.origin = Value::Null; // unclassified at step 0; admission uses the computed one
        let out = self.drive(
            &Call {
                kind: call.kind,
                input: call.input.clone(),
                host: host.clone(),
            },
            guest,
        )?;
        shape(&out)?;

        let effects = out.get("effects");
        let from = |k: &str| effects.and_then(|e| e.get(k)).or(out.get(k));
        let Some(pin) = from("pin") else {
            if effects.is_some() || from("scratch").is_some() {
                return Err("a step-0 output with effects or scratch carries a pin".into());
            }
            let pinless = HostFacts {
                job: None,
                ..call.host.clone()
            };
            self.write(&out, call.kind, &pinless.stamps())?;
            return Ok(Stepped {
                output: out,
                row: None,
                continues: false,
                code: None,
            });
        };
        let mut pin: JobPin =
            serde_json::from_value(pin.clone()).map_err(|e| format!("bad pin: {e}"))?;
        let hex = pin.source_digest.strip_prefix("sha256:");
        if hex.is_none_or(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(format!(
                "bad pin: source_digest {} is not sha256:<64 hex>",
                pin.source_digest
            ));
        }

        let (root, mut chain, control, root_call) = match from("resume") {
            Some(block) => {
                let id = block["row_doc_id"]
                    .as_str()
                    .ok_or("resume has no row_doc_id")?;
                let prior = self.state.rows.get(id).ok_or(format!(
                    "plugin_job_resume_unknown: resume names unknown row {id}"
                ))?;
                let adopted = self.jobs.rows.get(id);
                if adopted
                    .ok_or(format!(
                        "plugin_job_resume_unknown: resume names {id}, a row never adopted"
                    ))?
                    .session_id
                    != session
                {
                    return Err(format!("resume names {id}, a row of another session"));
                }
                let head = &self.jobs.chains[&prior.chain_root].head;
                if head != id {
                    return Err(format!(
                        "plugin_job_resume_not_head: resume names {id}, which {head} already resumed"
                    ));
                }
                if prior.state == JobState::Running {
                    return Err(format!("resume names {id}, which is still running"));
                }
                let chain = self.jobs.chains[&prior.chain_root].clone();
                let rc = chain.root_call.clone();
                (prior.chain_root.clone(), chain, control_of(block), Some(rc))
            }
            None => {
                let root_call: Map<String, Value> = ROOT_KEYS
                    .iter()
                    .filter_map(|k| Some((k.to_string(), call.input.get(*k)?.clone())))
                    .collect();
                let root_call = Value::Object(root_call);
                let handle = self.unique_handle(&session, &pin.handle);
                let chain = Chain {
                    handle,
                    args_digest: digest(&root_call),
                    root_call,
                    ..Chain::default()
                };
                (row.clone(), chain, Value::Null, None)
            }
        };
        // The host owns every pin field but the guest's four; a resume inherits the handle.
        pin.handle = chain.handle.clone();
        pin.host = match json!({
            "origin": call.host.origin, "chain_root_doc_id": root, "args_digest": chain.args_digest,
            "cwd": caller["cwd"], "resume_of": control.get("payload").map(|b| b["row_doc_id"].clone())
        }) {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        if let Some(a) = control["payload"].get("answer") {
            chain.journal.push(json!({"row_doc_id": row, "answer": a}));
        }
        chain.rows.push(row.clone());
        chain.head = row.clone();

        host.job = Some(
            json!({"row_doc_id": row, "chain_root_doc_id": root, "chain": chain.rows, "handle": chain.handle}),
        );
        self.state.rows.insert(
            row.clone(),
            JobRow {
                row_doc_id: row.clone(),
                tool_call_id: tool_call,
                handle: chain.handle.clone(),
                state: JobState::Running,
                pin: Some(pin),
                chain_root: root.clone(),
                result: None,
            },
        );
        let info = RowInfo {
            session_id: session,
            origin: call.host.origin.clone(),
            control,
            root_call,
            adopted_at: self.jobs.rows.len() as u64 + 1,
        };
        self.jobs.rows.insert(row.clone(), info);
        let done = self.conclude(call.kind, &host, &row, &call.host.ceiling, &out, chain);
        if done.is_err() {
            self.state.rows.remove(&row);
            self.jobs.rows.remove(&row);
        }
        done
    }

    /// A later step of a running row: the host fields come from the durable maps.
    pub fn step(
        &mut self,
        call: &Call,
        row: &str,
        guest: impl FnMut(Value) -> Value,
    ) -> Result<Stepped, String> {
        let r = self
            .state
            .rows
            .get(row)
            .ok_or(format!("unknown row {row}"))?;
        if r.state != JobState::Running {
            return Err(format!("row {row} is not running"));
        }
        let (root, handle, pin) = (r.chain_root.clone(), r.handle.clone(), r.pin.clone());
        let info = (self.jobs.rows.get(row))
            .ok_or(format!("row {row} was never adopted"))?
            .clone();
        let mut chain = self.jobs.chains[&root].clone();
        let mut host = call.host.clone();
        host.job = Some(
            json!({"row_doc_id": row, "chain_root_doc_id": root, "chain": chain.rows, "handle": handle}),
        );
        host.control = info.control;
        host.root_call = info.root_call;
        host.scratch = json!(chain.scratch);
        host.origin = info.origin.clone();
        host.ceiling = call.host.ceiling.clone();
        if let Some(left) = host.ceiling["remaining_spawns"].as_u64() {
            host.ceiling["remaining_spawns"] = left.saturating_sub(chain.charged).into();
        }
        let mut input = call.input.clone();
        input["pin"] = serde_json::to_value(pin).expect("pin serializes");
        input["children"] = admit::view(&self.state, &chain);
        input["jobs"] = self.jobs_view(&info.session_id);
        chain.observed = chain
            .keys
            .iter()
            .filter(|k| self.state.children[*k].state != "running")
            .cloned()
            .collect();

        let drove = self.drive(
            &Call {
                kind: call.kind,
                input,
                host: host.clone(),
            },
            guest,
        );
        match drove.and_then(|out| shape(&out).map(|()| out)) {
            Ok(out) => self.conclude(call.kind, &host, row, &call.host.ceiling, &out, chain),
            Err(why) => {
                chain.failures += 1;
                let ended = chain.failures >= 3;
                let code = ended.then(|| "plugin_job_step_failed".to_string());
                if ended {
                    admit::kill(&mut self.state, &chain);
                    self.end(
                        row,
                        JobState::Failed,
                        json!({"fail": {"code": "plugin_job_step_failed", "message": why}}),
                    );
                }
                self.jobs.chains.insert(root, chain);
                Ok(Stepped {
                    output: Value::Null,
                    row: Some(row.into()),
                    continues: !ended,
                    code,
                })
            }
        }
    }

    /// The caller session's rows of this plugin, `state` in the JobProgress vocabulary.
    fn jobs_view(&self, session: &str) -> Value {
        let mut rows: Vec<_> = (self.jobs.rows.iter())
            .filter(|(_, i)| i.session_id == session)
            .collect();
        rows.sort_by_key(|(id, i)| (i.adopted_at, *id));
        Value::Array(rows.into_iter().map(|(id, i)| {
            let r = &self.state.rows[id];
            let resume_of = i.control.pointer("/payload/row_doc_id");
            json!({"row_doc_id": id, "tool_call_id": r.tool_call_id, "handle": r.handle, "state": r.state,
                "adopted_at": i.adopted_at, "resume_of": resume_of})
        }).collect())
    }

    /// Admits an output and, when admitted, commits its writes, scratch, children and row state.
    fn conclude(
        &mut self,
        kind: Kind,
        host: &HostFacts,
        row: &str,
        ceiling: &Value,
        out: &Value,
        mut chain: Chain,
    ) -> Result<Stepped, String> {
        let root = self.state.rows[row].chain_root.clone();
        let origin = self.jobs.rows[row].origin.clone();
        let plan = match admit::check(&self.state, &mut chain, ceiling, &origin, out) {
            Ok(plan) => plan,
            Err(Denied { code, why }) => {
                admit::kill(&mut self.state, &chain);
                self.end(
                    row,
                    JobState::Failed,
                    json!({"fail": {"code": code, "message": why}}),
                );
                self.jobs.chains.insert(root, chain);
                return Ok(Stepped {
                    output: out.clone(),
                    row: Some(row.into()),
                    continues: false,
                    code: Some(code.into()),
                });
            }
        };
        let scratch = scratch_files(host, out)?;
        self.write(out, kind, &host.stamps())?;
        write_scratch(scratch, &mut chain)?;
        admit::apply(&mut self.state, &root, &chain, &plan);
        chain.failures = 0;
        let state = row_state(out);
        let continues = state == JobState::Running;
        let result = ["result", "fail", "suspend"]
            .iter()
            .find_map(|k| out.get(k).cloned());
        let r = self.state.rows.get_mut(row).expect("known row");
        (r.state, r.result) = (state, result);
        self.jobs.chains.insert(root, chain);
        Ok(Stepped {
            output: out.clone(),
            row: Some(row.into()),
            continues,
            code: None,
        })
    }

    /// A non-job tool call: runs the guest, applies its writes, then its `control` requests
    /// (stop or pause a running row and kill its children). The calling row is never adopted.
    pub fn tool_call(
        &mut self,
        call: &Call,
        guest: impl FnMut(Value) -> Value,
    ) -> Result<Value, String> {
        let session = call.host.caller["session_id"].as_str().unwrap_or_default();
        let out = self.drive(call, guest)?;
        let mut asked = Vec::new();
        for c in out["control"].as_array().into_iter().flatten() {
            let id = c["row_doc_id"].as_str().unwrap_or_default();
            let status = match c["kind"].as_str() {
                Some("stop") => (JobState::Cancelled, "cancelled"),
                Some("pause") => (JobState::Paused, "paused"),
                other => return Err(format!("control kind {other:?} is not stop or pause")),
            };
            let row = self
                .state
                .rows
                .get(id)
                .ok_or(format!("control names unknown row {id}"))?;
            let adopted = self.jobs.rows.get(id);
            if adopted
                .ok_or(format!("control names {id}, a row never adopted"))?
                .session_id
                != session
            {
                return Err(format!("control names {id}, a row of another session"));
            }
            asked.push((id.to_string(), row.chain_root.clone(), status));
        }
        self.write(&out, call.kind, &call.host.stamps())?;
        for (id, root, (state, status)) in asked {
            if self.state.rows[&id].state == JobState::Running {
                admit::kill(&mut self.state, &self.jobs.chains[&root]);
                self.end(&id, state, json!({"status": status}));
            }
        }
        Ok(out)
    }
}

/// The `scratch` records of an output as (name, path, content), checked without touching disk:
/// the caller has a cwd and tool_call_id and every name is one path component.
fn scratch_files(host: &HostFacts, out: &Value) -> Result<Vec<(String, PathBuf, String)>, String> {
    let files = out.pointer("/effects/scratch").or(out.get("scratch"));
    let Some(files) = files.and_then(Value::as_array).filter(|f| !f.is_empty()) else {
        return Ok(Vec::new());
    };
    let (cwd, tool_call) = (
        host.caller["cwd"].as_str(),
        host.caller["tool_call_id"].as_str(),
    );
    let dir = Path::new(cwd.ok_or("caller has no cwd")?)
        .join(".gents/workflows/runs")
        .join(tool_call.ok_or("caller has no tool_call_id")?);
    files
        .iter()
        .map(|f| {
            let name = f["name"].as_str().unwrap_or_default();
            if Path::new(name).file_name().is_none_or(|n| n != name) {
                return Err(format!("scratch name {name:?} is not one path component"));
            }
            let content = f["content"].as_str().unwrap_or_default();
            Ok((name.into(), dir.join(name), content.into()))
        })
        .collect()
}

/// Writes checked `scratch` records under `<cwd>/.gents/workflows/runs/<tool_call_id>/<name>`.
fn write_scratch(files: Vec<(String, PathBuf, String)>, chain: &mut Chain) -> Result<(), String> {
    for (name, path, content) in files {
        std::fs::create_dir_all(path.parent().expect("scratch path has a parent"))
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
        chain.scratch.insert(name, path.to_string_lossy().into());
    }
    Ok(())
}
