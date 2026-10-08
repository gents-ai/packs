use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    run_id: String,
    title: String,
    author: String,
    language: String,
    sources_json: String,
    entries_json: String,
    review_notes: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    source: String,
    page_count: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    title: String,
    source: String,
    page: u32,
    level: u32,
    matter_type: String,
    content_type: String,
    #[serde(default)]
    review_notes: String,
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
        let input: Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid proposal: {e}"))?;
        if input.is_array() {
            finish_signal(input)
        } else {
            assemble(serde_json::from_value(input).map_err(|e| format!("invalid proposal: {e}"))?)
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

/// Sources retain operator order. A leaf owns the half-open interval up to the
/// next leaf; container headings sharing a start must be resolved before this boundary.
fn assemble(p: Proposal) -> Result<Value, String> {
    for (name, value) in [
        ("run_id", &p.run_id),
        ("title", &p.title),
        ("author", &p.author),
        ("language", &p.language),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} must not be empty"));
        }
    }
    let sources: Vec<Source> =
        serde_json::from_str(&p.sources_json).map_err(|e| format!("sources_json: {e}"))?;
    let entries: Vec<Entry> =
        serde_json::from_str(&p.entries_json).map_err(|e| format!("entries_json: {e}"))?;
    if sources.is_empty() || sources.len() > 1000 || entries.is_empty() || entries.len() > 10000 {
        return Err("need 1-1000 sources and 1-10000 section starts".into());
    }
    let mut offsets = BTreeMap::new();
    let mut total = 0u32;
    for s in &sources {
        if s.source.trim().is_empty()
            || s.page_count == 0
            || offsets
                .insert(s.source.as_str(), (total, s.page_count))
                .is_some()
        {
            return Err("sources need distinct names and positive page counts".into());
        }
        total = total
            .checked_add(s.page_count)
            .filter(|n| *n <= 1_000_000)
            .ok_or("book exceeds one million scan pages")?;
    }
    let mut starts = Vec::new();
    for e in entries {
        let &(offset, count) = offsets
            .get(e.source.as_str())
            .ok_or_else(|| format!("unknown source {:?}", e.source))?;
        if e.page == 0 || e.page > count {
            return Err(format!("{} page {} is outside 1-{count}", e.source, e.page));
        }
        if e.title.trim().is_empty()
            || !(1..=6).contains(&e.level)
            || !["front_matter", "body", "back_matter"].contains(&e.matter_type.as_str())
            || e.content_type.trim().is_empty()
        {
            return Err("each entry needs a nonempty title and content_type, level 1-6, and matter_type exactly front_matter, body, or back_matter".into());
        }
        starts.push((offset + e.page, e));
    }
    starts.sort_by_key(|(page, _)| *page);
    if starts.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("two entries share a start page; resolve container and child headings into one leaf entry".into());
    }
    if starts[0].0 > 1 {
        starts.insert(
            0,
            (
                1,
                Entry {
                    title: "Front matter".into(),
                    source: sources[0].source.clone(),
                    page: 1,
                    level: 1,
                    matter_type: "front_matter".into(),
                    content_type: "unclassified".into(),
                    review_notes:
                        "Leading source pages preserved; no explicit section start was supplied."
                            .into(),
                },
            ),
        );
    }
    let mut chapters = Vec::new();
    let mut covered = 0u32;
    for (i, (start, e)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|(n, _)| n - 1).unwrap_or(total);
        let mut ranges = Vec::new();
        for s in &sources {
            let offset = offsets[s.source.as_str()].0;
            let first = (*start).max(offset + 1);
            let last = end.min(offset + s.page_count);
            if first <= last {
                ranges.push(
                    json!({"source":s.source,"start_page":first-offset,"end_page":last-offset}),
                );
            }
        }
        covered += end - start + 1;
        chapters.push(json!({"book_id":p.run_id,"chapter_key":format!("{}:{:05}",p.run_id,i+1),"sequence":i+1,
            "title":e.title,"level":e.level,"matter_type":e.matter_type,"content_type":e.content_type,
            "start_page":start,"end_page":end,"source_ranges_json":serde_json::to_string(&ranges).unwrap(),"review_notes":e.review_notes}));
    }
    if covered != total {
        return Err("chapter ranges do not cover the source pages".into());
    }
    Ok(
        json!({"book":{"book_id":p.run_id,"title":p.title,"author":p.author,"language":p.language,
        "source_manifest":serde_json::to_string(&sources).unwrap(),"page_count":total,"chapter_count":chapters.len(),"review_notes":p.review_notes},
        "report":{"book_id":p.run_id,"page_count":total,"chapter_count":chapters.len(),"covered_pages":covered,"review_notes":p.review_notes},"chapters":chapters}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn proposal() -> Proposal {
        Proposal {run_id:"book".into(),title:"Title".into(),author:"Author".into(),language:"en".into(),
        sources_json:r#"[{"source":"part-1.pdf","page_count":4},{"source":"part-2.pdf","page_count":6}]"#.into(),
        entries_json:r#"[{"title":"One","source":"part-1.pdf","page":3,"level":1,"matter_type":"body","content_type":"chapter"},{"title":"Two","source":"part-2.pdf","page":3,"level":1,"matter_type":"body","content_type":"chapter"}]"#.into(),review_notes:String::new()}
    }
    #[test]
    fn multipart_chapter_keeps_both_sources_and_all_frontmatter() {
        let out = assemble(proposal()).unwrap();
        assert_eq!(out["report"]["covered_pages"], 10);
        assert_eq!(out["chapters"][0]["start_page"], 1);
        assert_eq!(out["chapters"][0]["end_page"], 2);
        let ranges: Value =
            serde_json::from_str(out["chapters"][1]["source_ranges_json"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            ranges,
            json!([{"source":"part-1.pdf","start_page":3,"end_page":4},{"source":"part-2.pdf","start_page":1,"end_page":2}])
        );
        assert_eq!(out["chapters"][2]["end_page"], 10);
    }
    #[test]
    fn rejects_unknown_out_of_range_and_duplicate_starts() {
        for replacement in ["missing.pdf", "part-2.pdf"] {
            let mut p = proposal();
            p.sources_json = p.sources_json.replace("part-1.pdf", replacement);
            assert!(assemble(p).is_err());
        }
        let mut p = proposal();
        p.entries_json = p.entries_json.replace("\"page\":3", "\"page\":0");
        assert!(assemble(p).is_err());
        let mut p = proposal();
        p.entries_json = p.entries_json.replace("part-2.pdf", "part-1.pdf");
        assert!(assemble(p).is_err());
    }
}
