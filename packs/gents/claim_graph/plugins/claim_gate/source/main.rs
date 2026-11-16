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
mod profile;
mod quotes;
mod rows;
mod rules;
mod store;
#[cfg(test)]
mod tests;

use batch::Batch;
use profile::Profile;
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
    // The host resends the same input every round, so the inline profile is
    // parsed again rather than carried in `state`.
    let inline = batch.profile.as_deref().map(profile::parse_inline);
    let inline_ok = match &inline {
        Some(Ok(profile)) => Some(profile),
        _ => None,
    };
    let finish = |verdict, admitted: Option<&Profile>| {
        Ok(rows::result(&batch_id, &batch, verdict, admitted, &checked_at))
    };

    let resolvers: Vec<String> = inline_ok
        .map(|p| p.list("resolver_perspectives").map(str::to_owned).collect())
        .unwrap_or_default();
    let plan = store::plan(&batch, &resolvers);
    let mut state = match input.get("data_results") {
        None | Some(Value::Null) => {
            // First round: the rules and files that need no store. A failure
            // here is final and reports the quotes too; a quote alone waits for
            // the store, whose supersedes and retracts edges may exempt it.
            // Without an inline profile the vocabulary rules wait for the
            // stored one.
            let mut errors = Vec::new();
            if batch.graph_id.is_none() {
                errors.push("ERROR: the batch names no graph_id".to_owned());
            }
            if let Some(Err(profile_errors)) = &inline {
                errors.extend(profile_errors.iter().cloned());
            }
            errors.extend(rules::check(&batch, inline_ok, None));
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
                let verdict = local_verdict(&batch, errors, None, inline_ok, None);
                return finish(verdict, None);
            }
            if input.get("data_calls") != Some(&Value::Bool(true)) {
                return Err(
                    "the host offers this plugin no data_calls; declare data with query on ClaimGraph, Claim, Edge, Entity, Perspective and Source in the manifest"
                        .into(),
                );
            }
            State::default()
        }
        Some(results) => {
            let mut state = State::from_json(input.get("state"))?;
            let failures = state.absorb(&batch, &plan, results)?;
            if !failures.is_empty() {
                let verdict = local_verdict(&batch, failures, Some(&state.found), None, None);
                return finish(verdict, None);
            }
            state
        }
    };
    let graph_id = batch
        .graph_id
        .as_deref()
        .ok_or("the batch names no graph_id but the round went on")?;
    if let Some(requests) = state.next_requests(graph_id, &plan) {
        return Ok(json!({"data_calls": {"requests": requests, "state": state.to_json()}}));
    }

    // Last round: the graph's profile, then every rule against the store.
    let mut errors = Vec::new();
    let stored;
    let (profile, version) = match inline_ok {
        Some(profile) => (Some(profile), Some(state.found.latest_version() + 1)),
        None => match &state.found.profile {
            None => {
                errors.push(format!(
                    "ERROR: unknown graph {graph_id}: the first batch must carry a profile"
                ));
                (None, None)
            }
            Some(row) => match profile::parse_stored(graph_id, row) {
                Ok((profile, version)) => {
                    stored = profile;
                    (Some(&stored), Some(version))
                }
                Err(row_errors) => {
                    errors.extend(row_errors);
                    (None, None)
                }
            },
        },
    };
    if let Some(profile) = inline_ok {
        errors.extend(rules::check_resolvers(&batch, profile, &state.found));
    }
    errors.extend(rules::check(&batch, profile, Some(&state.found)));
    let verdict = local_verdict(&batch, errors, Some(&state.found), profile, version);
    finish(verdict, inline_ok)
}

/// `errors` plus the file and quote checks: file `from`s against the source
/// root when the batch names one, `source:` `from`s against the stored
/// sources once `found` (the store's answers) is known. Quotes are checked
/// only under a resolved profile, whose `questions_from_prefix` exempts
/// question files.
fn local_verdict(
    batch: &Batch,
    mut errors: Vec<String>,
    found: Option<&Found>,
    profile: Option<&Profile>,
    profile_version: Option<i64>,
) -> rows::Verdict {
    let root = batch.source_root.as_deref().map(Path::new);
    if let Some(root) = root {
        errors.extend(quotes::missing_files(root, &batch.claims));
    }
    let Some(profile) = profile else {
        return rows::Verdict {
            profile_version,
            errors,
            quotes_checked: 0,
            quotes_skipped: batch.claims.len() as u64,
        };
    };
    let mut historical = found.map(|f| f.historical.clone()).unwrap_or_default();
    for edge in &batch.edges {
        if matches!(edge.str("type"), Some("supersedes" | "retracts")) {
            if let Some(to) = edge.str("to") {
                historical.insert(to.to_owned());
            }
        }
    }
    let prefix = profile.text("questions_from_prefix").filter(|p| !p.is_empty());
    let places = quotes::Places { root, store: found };
    let report = quotes::check_quotes(&places, &batch.claims, &historical, prefix);
    errors.extend(report.errors);
    rows::Verdict {
        profile_version,
        errors,
        quotes_checked: report.checked,
        quotes_skipped: report.skipped,
    }
}
