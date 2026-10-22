//! The round services (L2 section 4.22): `data_calls` queries and `job_calls` text paging.
use super::state::State;
use crate::wire::{
    ChildOutputRequest, ChildOutputResponse, ChildOutputSlice, DataQuery, DataQueryResponse,
    JobResultRequest, JobResultResponse, JobState,
};
use serde_json::{Map, Value, json};
use std::cmp::Ordering;

/// The most rows one query page may ask for (root C3).
const MAX_LIMIT: u64 = 256;

/// Answers each request `{id, ...}` with `{result}` or `{error}`, keyed by id.
pub fn serve(requests: &Value, mut one: impl FnMut(&Value) -> Result<Value, String>) -> Value {
    let mut out = Map::new();
    for r in requests.as_array().into_iter().flatten() {
        let answer = match one(r) {
            Ok(result) => json!({"result": result}),
            Err(e) => json!({"error": e}),
        };
        out.insert(r["id"].as_str().unwrap_or_default().into(), answer);
    }
    Value::Object(out)
}

fn matches(
    row: &Value,
    filter: &Map<String, Value>,
    node_did: Option<&str>,
) -> Result<bool, String> {
    for (field, cond) in filter {
        if field == "_and" {
            let all = cond.as_array().ok_or("filter _and is not an array")?;
            for f in all {
                let f = f
                    .as_object()
                    .ok_or("a filter _and entry is not an object")?;
                if !matches(row, f, node_did)? {
                    return Ok(false);
                }
            }
            continue;
        }
        if field.starts_with('_') {
            return Err(format!("filter combinator {field} is not supported"));
        }
        let ops = cond
            .as_object()
            .ok_or_else(|| format!("filter for {field} is not an operator object"))?;
        let have = row.get(field).unwrap_or(&Value::Null);
        for (op, want) in ops {
            let want = match want.as_str() {
                Some("$caller.node_did") => {
                    json!(node_did.ok_or("$caller.node_did has no value in this call")?)
                }
                _ => want.clone(),
            };
            let ok = match op.as_str() {
                "_eq" => *have == want,
                "_in" => want.as_array().is_some_and(|a| a.contains(have)),
                other => return Err(format!("filter operator {other} is not supported")),
            };
            if !ok {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn compare(a: &Value, b: &Value) -> Ordering {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

/// `data_calls.query`: filter subset, order, offset, limit up to 256, field projection.
pub fn query(state: &State, q: &DataQuery, node_did: Option<&str>) -> Result<Value, String> {
    let limit = u64::from(q.limit);
    if limit > MAX_LIMIT {
        return Err(format!("limit {limit} passes the {MAX_LIMIT} row page cap"));
    }
    let mut rows = Vec::new();
    for r in state.collections.get(&q.collection).into_iter().flatten() {
        if matches(r, &q.filter, node_did)? {
            rows.push(r);
        }
    }
    let mut order: Vec<(&str, bool)> = Vec::new();
    for (f, dir) in q.order.iter().flatten() {
        match dir.as_str() {
            Some("ASC" | "DESC") => order.push((f.as_str(), dir == "DESC")),
            _ => return Err(format!("order direction {dir} must be ASC or DESC")),
        }
    }
    rows.sort_by(|a, b| {
        order.iter().fold(Ordering::Equal, |acc, (f, desc)| {
            let c = compare(&a[*f], &b[*f]);
            acc.then(if *desc { c.reverse() } else { c })
        })
    });
    let page = rows
        .into_iter()
        .skip(q.offset.unwrap_or(0) as usize)
        .take(q.limit as usize)
        .map(|r| {
            (q.fields.iter())
                .filter_map(|f| Some((f.clone(), r.get(f)?.clone())))
                .collect::<Map<String, Value>>()
        })
        .collect();
    Ok(json!(DataQueryResponse { rows: page }))
}

/// One window of `text`: the slice and whether the text continues past it.
/// ponytail: byte offsets; a window that splits a multi-byte char reads with a replacement char.
fn window(text: &str, offset: u64, limit: u64) -> (String, bool) {
    let bytes = text.as_bytes();
    let start = (offset as usize).min(bytes.len());
    let end = start.saturating_add(limit as usize).min(bytes.len());
    (
        String::from_utf8_lossy(&bytes[start..end]).into_owned(),
        end < bytes.len(),
    )
}

/// `job_calls.child_output`: each slot's latest attempt.
pub fn child_output(state: &State, req: &ChildOutputRequest) -> Result<Value, String> {
    let mut slots = std::collections::BTreeMap::new();
    for slot in &req.slots {
        let (_, child) = state
            .children
            .range((slot.clone(), 0)..=(slot.clone(), u64::MAX))
            .next_back()
            .ok_or_else(|| format!("no child for slot {slot}"))?;
        if child.state != "completed" {
            return Err(format!(
                "slot {slot:?} has no output yet (it is {:?}); read it once its children state is completed",
                child.state
            ));
        }
        let (text, truncated) = window(&child.text, req.offset, req.limit);
        slots.insert(slot.clone(), ChildOutputSlice { text, truncated });
    }
    Ok(json!(ChildOutputResponse { slots }))
}

/// `job_calls.result`: a terminal row's stored result, serialized and paged.
pub fn row_result(state: &State, req: &JobResultRequest) -> Result<Value, String> {
    let id = &req.row_doc_id;
    let row = state.rows.get(id).ok_or_else(|| format!("no row {id}"))?;
    if matches!(
        row.state,
        JobState::Running | JobState::Paused | JobState::AwaitingUser
    ) {
        return Err(format!(
            "not_terminal: {id} is not finished. Next: workflow status {id}"
        ));
    }
    let stored = (row.result.as_ref())
        .ok_or_else(|| format!("row {id} has no stored result"))?
        .to_string();
    let (result, truncated) = window(&stored, req.offset, req.limit);
    Ok(json!(JobResultResponse {
        row_doc_id: id.clone(),
        state: row.state.clone(),
        result,
        offset: req.offset,
        total_bytes: stored.len() as u64,
        truncated,
    }))
}
