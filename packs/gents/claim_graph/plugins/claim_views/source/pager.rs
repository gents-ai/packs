//! Reading whole collections through the host's `data_calls`: consecutive
//! pages of 256 rows ordered by one field, up to 16 queries a round shared by
//! the collections still being read, the cursor and the rows read so far in
//! the round's `state`. This is a copy of the claim_export module; the two
//! crates stay independent, so a change to one is copied to the other.
use serde_json::{Map, Value, json};

/// The host's limits on one `data_calls` round and one query.
pub const MAX_QUERIES_PER_ROUND: usize = 16;
pub const PAGE: usize = 256;
/// The host refuses an offset past this.
pub const MAX_OFFSET: usize = 100_000;

/// One collection to read: the state key, the collection, the field that
/// orders the pages and the fields a query asks for.
pub struct Spec {
    pub key: &'static str,
    pub collection: &'static str,
    pub id_field: &'static str,
    pub fields: &'static [&'static str],
}

/// The state before the first round: every collection at offset 0.
pub fn start(specs: &[&Spec]) -> Value {
    let mut cursors = Map::new();
    let mut rows = Map::new();
    for spec in specs {
        cursors.insert(
            spec.key.to_owned(),
            json!({"next": 0, "done": false, "short": false}),
        );
        rows.insert(spec.key.to_owned(), json!([]));
    }
    json!({"cursors": cursors, "rows": rows})
}

fn query(spec: &Spec, filter: Value, offset: usize) -> Value {
    json!({
        "id": format!("{}@{offset}", spec.key),
        "query": {
            "collection": spec.collection,
            "filter": filter,
            "order": {spec.id_field: "ASC"},
            "limit": PAGE,
            "offset": offset,
            "fields": spec.fields,
        },
    })
}

fn done(state: &Value, spec: &Spec) -> bool {
    state["cursors"][spec.key]["done"].as_bool().unwrap_or(false)
}

/// The next round: consecutive pages for every collection still being read,
/// sharing the round's 16 queries. A collection whose last page came back
/// short is asked for one page only (the page that confirms the end).
pub fn ask<F>(specs: &[&Spec], filter: F, state: Value) -> Value
where
    F: Fn(&Spec) -> Value,
{
    let active: Vec<&Spec> = specs.iter().copied().filter(|s| !done(&state, s)).collect();
    let share = (MAX_QUERIES_PER_ROUND / active.len().max(1)).max(1);
    let mut requests = Vec::new();
    for spec in active {
        let cursor = &state["cursors"][spec.key];
        let next = cursor["next"].as_u64().unwrap_or(0) as usize;
        let pages = if cursor["short"].as_bool().unwrap_or(false) {
            1
        } else {
            share
        };
        for page in 0..pages {
            requests.push(query(spec, filter(spec), next + page * PAGE));
        }
    }
    json!({"data_calls": {"requests": requests, "state": state}})
}

/// Takes one round's answers into the state. Returns the new state and
/// whether every collection has been read to its end.
pub fn advance(
    specs: &[&Spec],
    state: &Value,
    results: &Map<String, Value>,
) -> Result<(Value, bool), String> {
    let mut state = state.clone();
    for spec in specs {
        if state["cursors"][spec.key]["done"].as_bool() != Some(false) {
            continue;
        }
        let mut pages: Vec<(usize, &Value)> = results
            .iter()
            .filter_map(|(id, result)| {
                let (key, offset) = id.split_once('@')?;
                (key == spec.key).then_some(())?;
                Some((offset.parse().ok()?, result))
            })
            .collect();
        pages.sort_by_key(|(offset, _)| *offset);
        if pages.is_empty() {
            return Err(format!(
                "the host returned no answer for the {} query",
                spec.collection
            ));
        }
        let mut next = state["cursors"][spec.key]["next"].as_u64().unwrap_or(0) as usize;
        let mut finished = false;
        let mut short = false;
        let mut taken = Vec::new();
        for (offset, result) in pages {
            if offset != next {
                break;
            }
            let rows = page_rows(spec, result)?;
            if rows.is_empty() {
                finished = true;
                break;
            }
            next += rows.len();
            // A page shorter than the limit may be the end or a page the host
            // cut at its output budget; the following page tells which.
            short = rows.len() < PAGE;
            taken.extend(rows.iter().cloned());
            if short {
                break;
            }
        }
        if !finished && next > MAX_OFFSET {
            return Err(format!(
                "{} holds more than {MAX_OFFSET} rows, more than one call can page",
                spec.collection
            ));
        }
        if let Some(stored) = state["rows"][spec.key].as_array_mut() {
            stored.extend(taken);
        }
        state["cursors"][spec.key] = json!({"next": next, "done": finished, "short": short});
    }
    let all_done = specs
        .iter()
        .all(|s| state["cursors"][s.key]["done"].as_bool() == Some(true));
    Ok((state, all_done))
}

fn page_rows<'a>(spec: &Spec, result: &'a Value) -> Result<&'a Vec<Value>, String> {
    if let Some(error) = result.get("error") {
        return Err(format!(
            "the {} query failed: {}",
            spec.collection,
            error.as_str().unwrap_or("unknown error")
        ));
    }
    result
        .pointer("/result/rows")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("the {} query returned no rows array", spec.collection))
}

/// The rows read so far for one collection.
pub fn rows<'a>(state: &'a Value, spec: &Spec) -> &'a [Value] {
    state["rows"][spec.key]
        .as_array()
        .map_or(&[], Vec::as_slice)
}
