//! claim_gate: the mechanical gate of the claim graph. A `ClaimBatch` created
//! event delivers the batch; the plugin runs the research gate's rules
//! (`validate.sh`, `quotes.sh`, `edge.sh`) on it, asks the store through
//! `data_calls` whether its ids are new and its references known, and
//! returns a `GateReceipt` plus, on a pass, every row of the batch, which the
//! host writes in one transaction. A batch that breaks a rule is a `fail`
//! receipt, never a failed call; only malformed host input exits non-zero
//! (one sentence on stderr). See TOOL.md.
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{Value, json};

mod batch;
mod clock;
mod quotes;
mod rows;
mod rules;
mod store;
#[cfg(test)]
mod tests;

use batch::Batch;
use store::{Found, State};

fn main() {
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        fail(&format!("reading stdin: {e}"));
    }
    match run(&raw) {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            if let Err(e) = stdout
                .write_all(out.to_string().as_bytes())
                .and_then(|()| stdout.write_all(b"\n"))
            {
                fail(&format!("writing the result: {e}"));
            }
        }
        Err(e) => fail(&e),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("claim_gate: {message}");
    std::process::exit(1);
}

/// One round: the request batch of the next store lookups, or the ports
/// object. `Err` is malformed host input.
pub fn run(raw: &str) -> Result<Value, String> {
    let input: Value = serde_json::from_str(raw).map_err(|e| format!("invalid input: {e}"))?;
    let input = input.as_object().ok_or("the input must be a JSON object")?;
    let batch_id = batch::text_field(input, "batch_id")?
        .ok_or("batch_id is required: the host correlates every output with it")?;
    let checked_at = match input.get("gate_now") {
        None | Some(Value::Null) => clock::now_iso(),
        Some(Value::String(now)) => now.clone(),
        Some(_) => return Err("gate_now must be a string".into()),
    };
    let batch = batch::read(input)?;
    let finish = |verdict| Ok(rows::result(&batch_id, &batch, verdict, &checked_at));

    let plan = store::plan(&batch);
    let mut state = match input.get("data_results") {
        None | Some(Value::Null) => {
            // First round: the rules and files that need no store. A failure
            // here is final and reports the quotes too; a quote alone waits for
            // the store, whose supersedes and retracts edges may exempt it.
            let mut errors = rules::check(&batch, None);
            if batch.is_empty() {
                errors.push(
                    "ERROR: the batch carries no claims, edges, entities or perspectives"
                        .to_owned(),
                );
            }
            if plan.len() > store::MAX_QUERIES {
                errors.push(format!(
                    "ERROR: the batch needs {} store queries, more than the {} one call may ask; split it",
                    plan.len(),
                    store::MAX_QUERIES
                ));
            }
            let files_missing = batch
                .source_root
                .as_deref()
                .is_some_and(|root| !quotes::missing_files(Path::new(root), &batch.claims).is_empty());
            if !errors.is_empty() || files_missing {
                return finish(local_verdict(&batch, errors, &Found::default()));
            }
            if input.get("data_calls") != Some(&Value::Bool(true)) {
                return Err(
                    "the host offers this plugin no data_calls; declare data with query on Claim, Edge, Entity and Perspective in the manifest"
                        .into(),
                );
            }
            State::default()
        }
        Some(results) => {
            let mut state = State::from_json(input.get("state"))?;
            let failures = state.absorb(&plan, results)?;
            if !failures.is_empty() {
                return finish(local_verdict(&batch, failures, &state.found));
            }
            state
        }
    };
    if let Some(requests) = state.next_requests(&plan) {
        return Ok(json!({"data_calls": {"requests": requests, "state": state.to_json()}}));
    }
    let errors = rules::check(&batch, Some(&state.found));
    finish(local_verdict(&batch, errors, &state.found))
}

/// `errors` plus the file and quote checks when the batch names a source root.
fn local_verdict(batch: &Batch, mut errors: Vec<String>, found: &Found) -> rows::Verdict {
    let Some(root) = batch.source_root.as_deref().map(Path::new) else {
        return rows::Verdict {
            errors,
            quotes_checked: 0,
            quotes_skipped: batch.claims.len() as u64,
        };
    };
    errors.extend(quotes::missing_files(root, &batch.claims));
    let mut historical = found.historical.clone();
    for edge in &batch.edges {
        if matches!(edge.str("type"), Some("supersedes" | "retracts")) {
            if let Some(to) = edge.str("to") {
                historical.insert(to.to_owned());
            }
        }
    }
    let report = quotes::check_quotes(root, &batch.claims, &historical);
    errors.extend(report.errors);
    rows::Verdict {
        errors,
        quotes_checked: report.checked,
        quotes_skipped: report.skipped,
    }
}

