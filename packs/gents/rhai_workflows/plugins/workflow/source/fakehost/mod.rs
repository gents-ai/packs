//! The native fake host (L2 section 4.22): drives any guest `FnMut(Value) -> Value` through
//! the round protocol of gents `plugin/rounds.rs`, with no wasm and no gents. This part holds
//! the durable state, the round services and the host-owned input fields, the child oracles,
//! the faults and the command line of the `fakehost` example.
pub mod admit;
pub mod adopt;
pub mod faults;
pub mod oracle;
pub mod services;
pub mod state;

use crate::rounds::MAX_ROUNDS;
use crate::wire::{DataCall, JobCall, Mount};
use serde_json::Value;
use state::State;
use std::collections::BTreeMap;

/// One round request as the contract shapes it; anything else is the host's refusal.
fn parse<T: serde::de::DeserializeOwned>(r: &Value, service: &str) -> Result<T, String> {
    serde_json::from_value(r.clone())
        .map_err(|e| format!("a {service} request is not the contract shape ({e})"))
}

/// What kind of invocation a guest call is; it decides which services are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    JobStep,
    /// A non-job tool call (`list`, `keep`, `result`, ...).
    ToolCall,
    Callback,
    /// A catalog call: read-only.
    Catalog,
}

/// The host-overwritten (dagger) fields of `JobStepInput`. `None` removes the key.
#[derive(Debug, Clone, Default)]
pub struct HostFacts {
    pub job: Option<Value>,
    pub caller: Value,
    pub ceiling: Value,
    pub mounts: Value,
    pub control: Value,
    pub root_call: Option<Value>,
    pub scratch: Value,
    pub verified_script: Option<Value>,
    pub origin: Value,
}

impl HostFacts {
    /// `input` with every host field replaced by the host's value; the rest is echoed.
    fn overlay(&self, mut input: Value) -> Result<Value, String> {
        let o = input
            .as_object_mut()
            .ok_or("the call input is not an object")?;
        let mut set = |k: &str, v: Option<&Value>| match v {
            Some(v) => o.insert(k.into(), v.clone()),
            None => o.remove(k),
        };
        set("job", self.job.as_ref());
        set("caller", Some(&self.caller));
        set("ceiling", Some(&self.ceiling));
        set("mounts", Some(&self.mounts));
        set("control", Some(&self.control));
        set("root_call", self.root_call.as_ref());
        set("scratch", Some(&self.scratch));
        set("verified_script", self.verified_script.as_ref());
        set("origin", Some(&self.origin));
        Ok(input)
    }

    /// The stamp values this call context has (root C3); a stamp not in the map has none.
    fn stamps(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut put = |name: &str, v: Option<&Value>| {
            if let Some(s) = v.and_then(Value::as_str) {
                out.insert(name.to_string(), s.to_string());
            }
        };
        for name in ["node_did", "session_id", "request_doc_id"] {
            put(name, self.caller.get(name));
        }
        put("tool_call_doc_id", self.caller.get("tool_call_id"));
        if let Some(job) = &self.job {
            put("job_row_doc_id", job.get("row_doc_id"));
            put("chain_root_doc_id", job.get("chain_root_doc_id"));
        }
        out
    }

    fn node_did(&self) -> Option<&str> {
        self.caller.get("node_did").and_then(Value::as_str)
    }
}

/// One guest call: the C9 arguments (and anything else the host echoes) plus the host facts.
#[derive(Debug, Clone)]
pub struct Call {
    pub kind: Kind,
    pub input: Value,
    pub host: HostFacts,
}

#[derive(Debug, Default)]
pub struct FakeHost {
    pub state: State,
    pub jobs: adopt::Jobs,
}

/// One pack that ships an asset class, for `mounts`.
#[derive(Debug, Clone)]
pub struct PackAssets {
    pub pack: String,
    pub class: String,
    pub digest: String,
    pub trusted: bool,
}

/// The `mounts` host field: a temp dir per asset-shipping pack (guest path = host path) with
/// its configurable `trusted` flag, plus the project tier entry when `project` is a path.
pub fn mounts(packs: &[PackAssets], project: Option<&str>) -> std::io::Result<Value> {
    let mut out = Vec::new();
    for p in packs {
        let dir = std::env::temp_dir().join(format!(
            "wf-mount-{}-{}-{}",
            std::process::id(),
            p.pack,
            p.class
        ));
        std::fs::create_dir_all(&dir)?;
        out.push(Mount {
            pack: p.pack.clone(),
            class: p.class.clone(),
            digest: p.digest.clone(),
            path: dir.to_string_lossy().into(),
            trusted: p.trusted,
        });
    }
    out.extend(project.map(|path| Mount {
        pack: String::new(),
        class: "project".into(),
        digest: String::new(),
        path: path.into(),
        trusted: false,
    }));
    Ok(serde_json::to_value(out).expect("mounts serialize"))
}

impl FakeHost {
    /// Runs `guest` to a final output, serving one service per round (at most 64 rounds), then
    /// applies the output's `writes` atomically. A host failure is an `Err`.
    pub fn run(&mut self, call: &Call, guest: impl FnMut(Value) -> Value) -> Result<Value, String> {
        let out = self.drive(call, guest)?;
        self.write(&out, call.kind, &call.host.stamps())?;
        Ok(out)
    }

    /// The rounds of `run`, without applying the output.
    pub(crate) fn drive(
        &self,
        call: &Call,
        mut guest: impl FnMut(Value) -> Value,
    ) -> Result<Value, String> {
        let mut base = call.host.overlay(call.input.clone())?;
        let o = base
            .as_object_mut()
            .ok_or("the call input is not an object")?;
        for k in [
            "state",
            "data_results",
            "job_results",
            "data_calls",
            "job_calls",
        ] {
            o.remove(k);
        }
        let job_offered = matches!(call.kind, Kind::JobStep | Kind::ToolCall);
        o.insert("data_calls".into(), true.into());
        if job_offered {
            o.insert("job_calls".into(), true.into());
        }

        let mut next = base.clone();
        let mut served = 0;
        let out = loop {
            let out = guest(next);
            let data = out.get("data_calls");
            let job = out.get("job_calls").filter(|_| job_offered);
            let (service, asked) = match (data, job) {
                (None, None) => break out,
                (Some(d), None) => ("data", d),
                (None, Some(j)) => ("job", j),
                _ => return Err("one round asks for exactly one of data_calls or job_calls".into()),
            };
            if served == MAX_ROUNDS {
                return Err(format!("the guest asked for more than {MAX_ROUNDS} rounds"));
            }
            served += 1;
            let did = call.host.node_did();
            let session = call.host.caller["session_id"].as_str().unwrap_or_default();
            let results = services::serve(&asked["requests"], |r| match service {
                "data" => {
                    let call: DataCall = parse(r, "data_calls")?;
                    services::query(&self.state, &call.query, did)
                }
                _ => match parse(r, "job_calls")? {
                    JobCall::ChildOutput { child_output, .. } => {
                        services::child_output(&self.state, &child_output)
                    }
                    JobCall::Result { result, .. } => {
                        // ProcessControlScope: only a row of the caller's own session.
                        let id = &result.row_doc_id;
                        if (self.jobs.rows.get(id)).is_none_or(|i| i.session_id != session) {
                            return Err(format!("row {id} is not in this call's scope"));
                        }
                        services::row_result(&self.state, &result)
                    }
                },
            });
            next = base.clone();
            next[format!("{service}_results")] = results;
            next["state"] = asked["state"].clone();
        };

        Ok(out)
    }

    /// Applies one output's `writes` under `stamps`; a catalog call refuses them.
    pub(crate) fn write(
        &mut self,
        out: &Value,
        kind: Kind,
        stamps: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let writes = out.pointer("/effects/writes").or(out.get("writes"));
        if let Some(writes) = writes.filter(|w| w.as_array().is_some_and(|a| !a.is_empty())) {
            if kind == Kind::Catalog {
                return Err("a catalog call is read-only; its writes are refused".into());
            }
            self.state.apply_writes(writes, stamps)?;
        }
        Ok(())
    }
}

/// The command line of `examples/fakehost.rs`, kept here so tests reach it.
pub mod cli {
    use super::oracle::{Labels, spawns};
    use super::{Call, FakeHost, HostFacts, Kind};
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};

    const MAX_STEPS: usize = 64;

    #[derive(Debug, PartialEq)]
    pub struct Args {
        pub script: PathBuf,
        pub args: Option<PathBuf>,
        pub oracle: Option<PathBuf>,
        pub trace: bool,
    }

    /// `--script x.rhai [--args a.json] [--oracle labels.json] [--trace]`.
    pub fn parse(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut argv = argv.into_iter();
        let (mut script, mut args, mut oracle, mut trace) = (None, None, None, false);
        while let Some(flag) = argv.next() {
            let mut value = || {
                let v = argv.next().map(PathBuf::from);
                v.ok_or(format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--script" => script = Some(value()?),
                "--args" => args = Some(value()?),
                "--oracle" => oracle = Some(value()?),
                "--trace" => trace = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Args {
            script: script.ok_or("--script is required")?,
            args,
            oracle,
            trace,
        })
    }

    /// Host facts for a single job call of a demo session.
    pub fn facts(cwd: &str) -> HostFacts {
        HostFacts {
            caller: json!({
                "session_id": "cli", "request_id": "q", "request_doc_id": "qd",
                "tool_call_id": "t1", "node_did": "did:cli", "cwd": cwd
            }),
            ceiling: json!({
                "targets": ["wf-read-only"], "max_live": 64, "remaining_spawns": 1024,
                "max_messages_per_slot": 8, "result_bytes": 65536,
                "dynamic": "run", "dynamic_targets": ["wf-read-only"]
            }),
            mounts: json!([]),
            scratch: json!({}),
            origin: json!("installed"),
            ..HostFacts::default()
        }
    }

    /// Runs the script through `engine` on a fresh fake host, answering its spawns from the
    /// oracle file (canned without one), and returns the rendered run.
    pub fn run(a: &Args, mut engine: impl FnMut(Value) -> Value) -> Result<String, String> {
        let read =
            |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
        let json_of = |p: &Path| {
            serde_json::from_str::<Value>(&read(p)?).map_err(|e| format!("{}: {e}", p.display()))
        };
        let labels = match &a.oracle {
            Some(p) => Labels::from_json(&json_of(p)?)?,
            None => Labels::default(),
        };
        let mut input = json!({
            "protocol": 1, "argv": ["run", a.script.to_string_lossy()], "script": read(&a.script)?
        });
        if let Some(p) = &a.args {
            input["args"] = json_of(p)?;
        }
        let cwd = std::env::temp_dir().join(format!("wf-fakehost-cli-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
        let call = Call {
            kind: Kind::JobStep,
            input,
            host: facts(&cwd.to_string_lossy()),
        };

        let mut host = FakeHost::default();
        let mut step = host.step0(&call, &mut engine)?;
        let (mut outs, mut asked) = (vec![step.output.clone()], spawns(&step.output));
        for _ in 0..MAX_STEPS {
            let Some(row) = step.row.clone().filter(|_| step.continues) else {
                return Ok(render(&outs, a.trace));
            };
            host.settle(&asked, |s| labels.answer(s));
            step = host.step(&call, &row, &mut engine)?;
            asked.extend(spawns(&step.output));
            outs.push(step.output.clone());
        }
        Err(format!("the run did not end within {MAX_STEPS} steps"))
    }

    /// One line per fact: with `trace`, each step's spawn, message and interrupt effects; always
    /// the progress frames, the WfNote writes and the result, fail or suspend.
    pub fn render(outs: &[Value], trace: bool) -> String {
        let mut lines = Vec::new();
        for (i, out) in outs.iter().enumerate() {
            let fx = out.get("effects").unwrap_or(out);
            let list = |k: &str| fx[k].as_array().cloned().unwrap_or_default();
            if trace {
                for k in ["spawn", "message", "interrupt"] {
                    lines.extend(list(k).iter().map(|e| format!("step {i} {k} {e}")));
                }
            }
            if let Some(p) = fx.get("progress") {
                lines.push(format!("step {i} progress {p}"));
            }
            for w in list("writes")
                .iter()
                .filter(|w| w["collection"] == "WfNote")
            {
                let f = &w["fields"];
                lines.push(format!(
                    "step {i} note {} {}",
                    f["note_id"], f["value_json"]
                ));
            }
            for k in ["result", "fail", "suspend"] {
                lines.extend(out.get(k).map(|v| format!("{k} {v}")));
            }
        }
        lines.join("\n")
    }
}
