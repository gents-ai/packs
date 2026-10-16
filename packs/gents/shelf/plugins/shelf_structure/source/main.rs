use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, Read};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    source: String,
    page_count: u32,
}
fn main() {
    let result = (|| -> Result<Value, String> {
        let mut raw = String::new();
        io::stdin()
            .take(4_000_001)
            .read_to_string(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() > 4_000_000 {
            return Err("proposal exceeds 4 MB; reduce outline notes".into());
        }
        let mut input: Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid proposal: {e}"))?;
        if let Some(fields) = input.as_object_mut() {
            fields.retain(|_, v| !v.is_null());
        }
        if input.is_array() {
            if input[0].get("chunk_ref").is_some() {
                finish_signal(input)
            } else {
                source_ready(input)
            }
        } else {
            Err("Shelf barriers require grouped completion receipts".into())
        }
    })();
    match result {
        Ok(value) => {
            if let Err(e) = serde_json::to_writer(io::stdout().lock(), &value) {
                eprintln!("shelf_structure: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("shelf_structure: {e}");
            std::process::exit(1);
        }
    }
}

fn finish_signal(input: Value) -> Result<Value, String> {
    let rows = input.as_array().ok_or("expected review receipts")?;
    let first = rows.first().ok_or("empty review group")?;
    let expected = first["expected_total"]
        .as_u64()
        .ok_or("missing expected_total")?;
    if expected == 0 || rows.len() as u64 != expected {
        return Err("review group is incomplete".into());
    }
    let mut result = serde_json::Map::new();
    for field in ["run_id", "book_id", "path", "plan"] {
        let value = first[field]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or(format!("missing {field}"))?;
        if rows.iter().any(|row| row[field] != value) {
            return Err(format!("review group mixes {field}"));
        }
        result.insert(field.into(), json!(value));
    }
    let mut members = std::collections::BTreeSet::new();
    for row in rows {
        let key = row["chunk_ref"].as_str().ok_or("missing chunk_ref")?;
        if row["expected_total"].as_u64() != Some(expected) || !members.insert(key) {
            return Err("review group has conflicting totals or duplicate members".into());
        }
    }
    Ok(Value::Object(result))
}

fn canonical_count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let text = value.as_str()?;
        if text.is_empty()
            || !text.bytes().all(|c| c.is_ascii_digit())
            || (text.len() > 1 && text.starts_with('0'))
        {
            return None;
        }
        text.parse().ok()
    })
}

fn source_ready(input: Value) -> Result<Value, String> {
    let rows = input.as_array().ok_or("expected source receipts")?;
    let first = rows.first().ok_or("empty source group")?;
    let expected = canonical_count(&first["expected_total"]).ok_or("missing planned count")?;
    if expected == 0 || rows.len() as u64 != expected {
        return Err("source group is incomplete".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        for key in [
            "run_id",
            "book_id",
            "sources_json",
            "expected_total",
            "access",
            "license",
        ] {
            if row[key] != first[key] || row[key].is_null() {
                return Err(format!("source group mixes {key}"));
            }
        }
        let chunk = canonical_count(&row["chunk"]).ok_or("missing chunk")?;
        if chunk >= expected || !seen.insert(chunk) {
            return Err("source group has duplicate or unplanned chunks".into());
        }
        if row["extraction_state"] != "complete"
            || row["error"].as_str().is_some_and(|e| !e.is_empty())
        {
            return Err(format!(
                "chunk {chunk} extraction failed; inspect ShelfExtract before retrying"
            ));
        }
        if row["format"] != "pdf" {
            return Err("the reviewed scan workflow requires PDFs; normalize other formats through source-text intake".into());
        }
    }
    let sources: Vec<Source> = serde_json::from_str(
        first["sources_json"]
            .as_str()
            .ok_or("missing source manifest")?,
    )
    .map_err(|e| e.to_string())?;
    if sources.is_empty() || sources.iter().any(|s| s.page_count == 0) {
        return Err("invalid native source manifest".into());
    }
    Ok(
        json!({"run_id":first["run_id"],"book_id":first["book_id"],"sources_json":first["sources_json"],"expected_total":expected,"access":first["access"],"license":first["license"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_barrier_rejects_missing_duplicate_failed_and_mixed_source_chunks() {
        let one = json!({"run_id":"r","book_id":"b","sources_json":"[{\"source\":\"scan.pdf\",\"page_count\":40}]","expected_total":2,"chunk":0,"access":"local_only","license":"unknown","extraction_state":"complete","format":"pdf"});
        let mut two = one.clone();
        two["chunk"] = json!(1);
        let good = json!([one, two]);
        assert_eq!(source_ready(good.clone()).unwrap()["book_id"], "b");
        assert!(source_ready(json!([good[0]])).is_err());
        assert!(source_ready(json!([good[0], good[0]])).is_err());
        for (key, value) in [
            ("sources_json", json!("[]")),
            ("extraction_state", json!("failed")),
            ("format", json!("epub")),
            ("chunk", json!(2)),
        ] {
            let mut bad = good.clone();
            bad[1][key] = value;
            assert!(source_ready(bad).is_err(), "{key}");
        }
    }
    #[test]
    fn grouped_reviews_require_distinct_members_and_one_book_path() {
        let first = json!({"run_id":"edition","book_id":"book","path":"/exports/book",
            "plan":"plan.json","chunk_ref":"c0000","expected_total":2});
        let mut second = first.clone();
        second["chunk_ref"] = json!("c0001");
        let ready = finish_signal(json!([first, second])).unwrap();
        assert_eq!(
            ready,
            json!({"run_id":"edition","book_id":"book","path":"/exports/book","plan":"plan.json"})
        );
        assert!(finish_signal(json!([first])).is_err());
        assert!(finish_signal(json!([first, first])).is_err());
        second["book_id"] = json!("another-book");
        assert!(finish_signal(json!([first, second])).is_err());
    }
}
