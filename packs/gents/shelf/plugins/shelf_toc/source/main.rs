mod evidence;
mod handoff;
mod source;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    key: String,
    #[serde(default)]
    entry_number: Option<String>,
    title: String,
    level: u32,
    #[serde(default)]
    level_name: Option<String>,
    #[serde(default)]
    printed_page_number: Option<String>,
    #[serde(default)]
    scan_page: Option<u32>,
    #[serde(default)]
    reasoning: String,
    #[serde(default)]
    origin: String,
}
fn main() {
    let result = (|| -> Result<Value> {
        let mut raw = String::new();
        io::stdin()
            .take(4_000_001)
            .read_to_string(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() > 4_000_000 {
            return Err("structure handoff exceeds 4 MB".into());
        }
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if value.get("command").is_some() {
            return Ok(evidence::call(&value).unwrap_or_else(|error| json!({"error":error})));
        }
        dispatch(value)
    })();
    match result {
        Ok(v) => {
            serde_json::to_writer(io::stdout().lock(), &v).unwrap();
        }
        Err(e) => {
            eprintln!("shelf_toc: {e}");
            std::process::exit(1)
        }
    }
}
fn value(v: &Value, key: &str) -> Result<Value> {
    match &v[key] {
        Value::String(s) => serde_json::from_str(s).map_err(|e| format!("invalid {key}: {e}")),
        Value::Null => Err(format!("missing {key}")),
        other => Ok(other.clone()),
    }
}
fn entries(v: &Value) -> Result<Vec<Entry>> {
    serde_json::from_value(value(v, "entries_json")?).map_err(|e| e.to_string())
}
fn context(v: &Value) -> Result<Value> {
    let mut out = json!({});
    for k in ["run_id", "book_id", "path", "book_file", "book_hash"] {
        out[k] = json!(field(v, k)?);
    }
    for k in [
        "toc_start",
        "toc_end",
        "body_start",
        "body_end",
        "structure_summary_json",
        "structure_notes_json",
        "access",
        "license",
    ] {
        if !v[k].is_null() {
            out[k] = v[k].clone();
        }
    }
    if let Some(path) = v["path_original"].as_str().filter(|s| !s.is_empty()) {
        out["path"] = json!(path);
    }
    Ok(out)
}
fn job(v: &Value, stage: &str) -> Result<Value> {
    let mut out = context(v)?;
    out["stage"] = json!(stage);
    if stage != "toc_extract" {
        out.as_object_mut().unwrap().remove("structure_notes_json");
    }
    out["stage_job_id"] = json!(format!(
        "{}:{stage}:{}",
        field(v, "run_id")?,
        &hash(v.to_string().as_bytes())[..20]
    ));
    for (k, op) in [
        ("frontmatter_command", "frontmatter"),
        ("search_command", "search"),
        ("heading_command", "headings"),
        ("ocr_command", "ocr"),
        ("image_command", "image"),
    ] {
        out[k] = json!(op);
    }
    Ok(out)
}
fn failure(v: &Value, error: String) -> Value {
    json!({"failure":{"book_id":v["book_id"],"stage":v["stage"],"reason":error}})
}
fn normalize_bound_counts(v: &mut Value) -> Result<()> {
    if let Some(rows) = v.as_array_mut() {
        for row in rows {
            normalize_bound_counts(row)?;
        }
    } else {
        for key in [
            "toc_start",
            "toc_end",
            "body_start",
            "body_end",
            "total_pages",
            "expected_total",
        ] {
            if let Some(raw) = v[key].as_str() {
                let count = raw
                    .parse::<u32>()
                    .map_err(|_| format!("invalid bound {key}"))?;
                v[key] = json!(count);
            }
        }
    }
    Ok(())
}
fn dispatch(mut v: Value) -> Result<Value> {
    let result = normalize_bound_counts(&mut v).and_then(|()| {
        if v.is_array() {
            handoff::group(&v)
        } else {
            let v = handoff::hydrate(v.clone())?;
            match field(&v, "stage")? {
                "start" => start(&v),
                "toc_find" => toc_found(&v),
                "toc_extract" => plan_links(&v),
                "pattern_prepare" => pattern_prepare(&v),
                "pattern" => plan_discovery(&v),
                "gap_plan" => plan_gaps(&v),
                "metadata" => metadata(&v),
                "classify_prepare" => classify_prepare(&v),
                "classify" => classify(&v),
                "join_ready" => handoff::finish_group(&v),
                other => Err(format!("unknown structure stage {other}")),
            }
        }
    });
    match result {
        Ok(mut out) => match handoff::capture_jobs(&v, &mut out) {
            Ok(()) => Ok(out),
            Err(error) => Ok(failure(&v, error)),
        },
        Err(error) => Ok(failure(if v.is_array() { &v[0] } else { &v }, error)),
    }
}
fn start(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let mut finder = job(v, "toc_find")?;
    finder["total_pages"] = json!(book.pages.len());
    finder["finder_prompt"] = json!(finder_prompt(book.pages.len()));
    let mut meta = job(v, "metadata")?;
    meta["total_pages"] = finder["total_pages"].clone();
    Ok(json!({"toc_find_job":finder,"metadata_job":meta}))
}
fn toc_found(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    if v["toc_found"] != true {
        return Err("ToC was not found; preserve this finding for review rather than inventing chapter boundaries".into());
    }
    let (confidence, lo, hi, summary) = match toc_proposal(v, &book) {
        Ok(proposal) => proposal,
        Err(error) => return retry_toc(v, book.pages.len(), error),
    };
    let inspection_name = format!(
        "inspection-{}.json",
        &hash(field(v, "stage_job_id")?.as_bytes())[..32]
    );
    let root = Path::new(field(v, "path")?);
    if !root
        .join(&inspection_name)
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return retry_toc(v, book.pages.len(), "No visual inspection was recorded; inspect every contents page and the following page with load_page_image before finishing".into());
    }
    let inspection: Value = serde_json::from_slice(&read(root, &inspection_name, 512 * 1024)?)
        .map_err(|e| e.to_string())?;
    let observations = inspection["observations"]
        .as_array()
        .ok_or("missing visual observations")?;
    for page in lo..=hi.saturating_add(1).min(book.pages.len() as u64) {
        if !observations.iter().any(|o| {
            o["page_num"].as_u64() == Some(page)
                && o["visual_observations"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
        }) {
            return retry_toc(
                v,
                book.pages.len(),
                format!(
                    "ToC page {page} lacks visual evidence; inspect all contents pages and the following page before finishing"
                ),
            );
        }
    }
    let mut extract = job(v, "toc_extract")?;
    extract["toc_start"] = json!(lo);
    extract["toc_end"] = json!(hi);
    extract["structure_summary_json"] = json!(summary.to_string());
    extract["structure_notes_json"] = json!(inspection.to_string());
    extract["toc_pages_json"] =
        json!(serde_json::to_string(&book.pages[(lo - 1) as usize..hi as usize]).unwrap());
    extract["total_pages"] = json!(book.pages.len());
    Ok(
        json!({"toc_extract_job":extract,"toc":{"book_id":v["book_id"],"start_page":lo,"end_page":hi,"structure_summary_json":summary.to_string(),"structure_notes_json":inspection.to_string(),"confidence":confidence,"search_strategy_used":v["search_strategy_used"],"reasoning":v["reasoning"]}}),
    )
}
fn finder_prompt(total: usize) -> String {
    format!(
        "Find the complete table of contents in this {total}-page book. Begin with get_frontmatter_grep_report. Preserve hierarchy and continuation observations. Inspect every contents page and the first following non-contents page with load_page_image before writing the result."
    )
}
fn retry_toc(v: &Value, total: usize, error: String) -> Result<Value> {
    let attempt = v["retry_count"].as_u64().unwrap_or(0);
    if attempt >= 2 {
        return Err(format!(
            "ToC finding failed after {} attempts: {error}",
            attempt + 1
        ));
    }
    let mut next = job(v, "toc_find")?;
    next["total_pages"] = json!(total);
    next["retry_count"] = json!(attempt + 1);
    let previous =
        json!({"toc_page_range":v["toc_page_range"],"structure_summary":v["structure_summary"]});
    let previous: String = previous.to_string().chars().take(1000).collect();
    next["finder_prompt"] = json!(format!(
        "{}\n\nNative validation rejected the previous finding: {error}\nPrevious proposed range and hierarchy (may be truncated):\n{previous}\nThis is a new inspection attempt. Verify the proposed range against the source; inspect every contents page and its following page in this attempt. Correct the finding rather than skipping validation.",
        finder_prompt(total)
    ));
    Ok(json!({"toc_find_job":next}))
}
fn toc_proposal(v: &Value, book: &BookInput) -> Result<(f64, u64, u64, Value)> {
    let confidence = v["confidence"]
        .as_f64()
        .filter(|n| (0.0..=1.0).contains(n))
        .ok_or("invalid ToC confidence")?;
    field(v, "reasoning")?;
    let strategy = field(v, "search_strategy_used")?;
    if !["grep_report", "grep_with_scan", "not_found"].contains(&strategy) {
        return Err("unknown ToC search strategy".into());
    }
    let range = value(v, "toc_page_range")?;
    let lo = range["start_page"].as_u64().ok_or("missing ToC start")?;
    let hi = range["end_page"].as_u64().ok_or("missing ToC end")?;
    if lo == 0 || lo > hi || hi > book.pages.len() as u64 {
        return Err("ToC range outside source pages".into());
    }
    let summary = value(v, "structure_summary")?;
    if !(1..=3).contains(&summary["total_levels"].as_u64().unwrap_or(0))
        || !summary["level_patterns"].is_object()
    {
        return Err("ToC finding needs its hierarchy observations".into());
    }
    Ok((confidence, lo, hi, summary))
}
fn validate_entries(entries: &[Entry]) -> Result<()> {
    if entries.is_empty() || entries.len() > 256 {
        return Err(
            "ToC must have 1-256 entries; larger hierarchies require a nested fan-in".into(),
        );
    }
    let mut previous = 0;
    for e in entries {
        if !(1..=3).contains(&e.level)
            || e.level > previous + 1
            || (e.title.trim().is_empty()
                && e.entry_number
                    .as_deref()
                    .is_none_or(|s| s.trim().is_empty()))
        {
            return Err("ToC entries need a title or marker and a consistent hierarchy".into());
        }
        previous = e.level;
    }
    Ok(())
}
fn plan_links(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let raw = value(v, "extraction")?;
    let mut es: Vec<Entry> =
        serde_json::from_value(raw["entries"].clone()).map_err(|e| e.to_string())?;
    validate_entries(&es)?;
    for (i, e) in es.iter_mut().enumerate() {
        if !e.key.is_empty() || e.scan_page.is_some() {
            return Err("extraction cannot supply native identity or verified boundaries".into());
        }
        e.key = format!("{}:toc:{i:04}", field(v, "run_id")?);
        e.origin = "toc".into();
    }
    let keys: Vec<_> = es.iter().map(|e| &e.key).collect();
    let plan = hash(serde_json::to_string(&es).unwrap().as_bytes());
    let jobs: Vec<Value> = es
        .iter()
        .map(|e| {
            let mut j = job(v, "toc_link").unwrap();
            j["entry_id"] = json!(e.key);
            j["entry_json"] = json!(serde_json::to_string(e).unwrap());
            j["expected_total"] = json!(es.len());
            j["expected_ids_json"] = json!(serde_json::to_string(&keys).unwrap());
            j["plan_id"] = json!(plan);
            j["total_pages"] = json!(book.pages.len());
            j["stage_job_id"] = json!(e.key);
            j
        })
        .collect();
    let records:Vec<_>=es.iter().enumerate().map(|(i,e)|json!({"book_id":v["book_id"],"entry_id":e.key,"sort_order":i+1,"entry_number":e.entry_number,"title":e.title,"level":e.level,"level_name":e.level_name,"printed_page_number":e.printed_page_number})).collect();
    Ok(json!({"entries":records,"link_jobs":jobs}))
}
fn verified(e: &mut Entry, row: &Value, total: u32) -> Result<()> {
    let page = row["scan_page"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= u64::from(total))
        .ok_or_else(|| format!("entry {:?} has no verified opening", e.title))?;
    e.scan_page = Some(page as u32);
    e.reasoning = field(row, "reasoning")?.into();
    if e.origin == "discovered" {
        if let Some(title) = row["found_title"].as_str().filter(|s| !s.trim().is_empty()) {
            e.title = title.into();
        }
    }
    Ok(())
}
fn join(v: &Value) -> Result<Value> {
    let rows = v.as_array().ok_or("expected results")?;
    let first = rows.first().ok_or("empty result group")?;
    let expected = first["expected_total"]
        .as_u64()
        .ok_or("missing native expected_total")?;
    if expected == 0 || rows.len() as u64 != expected {
        return Err("incomplete stage results".into());
    }
    for row in rows {
        for k in [
            "run_id",
            "book_id",
            "stage",
            "path",
            "book_file",
            "book_hash",
            "plan_id",
            "expected_ids_json",
            "expected_total",
            "entries_json",
            "excluded_json",
            "total_pages",
        ] {
            if row[k] != first[k] {
                return Err(format!("stage results mix {k}"));
            }
        }
    }
    match field(first, "stage")? {
        "toc_link" | "discover" => {
            let keys: Vec<String> = serde_json::from_value(value(first, "expected_ids_json")?)
                .map_err(|e| e.to_string())?;
            let mut pending: BTreeSet<_> = keys.iter().cloned().collect();
            if pending.len() != expected as usize {
                return Err("invalid member manifest".into());
            }
            let total = u32::try_from(first["total_pages"].as_u64().ok_or("missing total pages")?)
                .map_err(|_| "invalid page count")?;
            if rows
                .iter()
                .any(|r| r["total_pages"] != first["total_pages"])
            {
                return Err("stage results mix total_pages".into());
            }
            let mut linked = Vec::new();
            for row in rows {
                let mut e: Entry =
                    serde_json::from_value(value(row, "entry_json")?).map_err(|e| e.to_string())?;
                if !pending.remove(field(row, "entry_id")?) || e.key != field(row, "entry_id")? {
                    return Err("duplicate or unplanned entry finding".into());
                }
                verified(&mut e, row, total)?;
                linked.push(e);
            }
            if field(first, "stage")? == "discover" {
                let mut old = entries(first)?;
                old.extend(linked);
                linked = old;
            }
            if field(first, "stage")? == "toc_link" {
                linked.sort_by_key(|e| keys.iter().position(|k| k == &e.key).unwrap());
                if linked
                    .windows(2)
                    .any(|pair| pair[0].scan_page > pair[1].scan_page)
                {
                    return Err(
                        "entry openings contradict ToC order; review boundary findings".into(),
                    );
                }
            } else {
                linked.sort_by_key(|e| (e.scan_page.unwrap(), e.level));
            }
            let mut next = job(
                first,
                if field(first, "stage")? == "toc_link" {
                    "pattern_prepare"
                } else {
                    "gap_plan"
                },
            )?;
            next["entries_json"] = json!(serde_json::to_string(&linked).unwrap());
            next["total_pages"] = json!(total);
            if field(first, "stage")? == "discover" {
                next["discovery_count"] = json!(expected);
                next["excluded_json"] = first["excluded_json"].clone();
            }
            Ok(
                json!({if field(first,"stage")?=="toc_link"{"pattern_prepare_job"}else{"gap_plan_job"}:next}),
            )
        }
        "gap" => {
            let keys: Vec<String> = serde_json::from_value(value(first, "expected_ids_json")?)
                .map_err(|e| e.to_string())?;
            let mut pending: BTreeSet<_> = keys.into_iter().collect();
            if pending.len() != expected as usize {
                return Err("invalid gap member manifest".into());
            }
            let mut es = entries(first)?;
            for row in rows {
                if !pending.remove(field(row, "gap_id")?) {
                    return Err("duplicate or unplanned gap result".into());
                }
                let fix = value(row, "fix")?;
                field(&fix, "reasoning")?;
                match field(&fix, "fix_type")? {
                    "flag_for_review" => {
                        return Err(
                            "a gap investigator flagged unresolved boundaries for review".into(),
                        );
                    }
                    "no_fix_needed" => {}
                    "correct_entry" => {
                        let key = field(&fix, "entry_doc_id")?;
                        let e = es
                            .iter_mut()
                            .find(|e| e.key == key)
                            .ok_or("gap fix targets an unknown entry")?;
                        verified(
                            e,
                            &json!({"scan_page":fix["new_scan_page"],"reasoning":fix["reasoning"]}),
                            first["total_pages"].as_u64().unwrap() as u32,
                        )?;
                    }
                    "add_entry" => {
                        let mut e = Entry {
                            key: format!("{}:gap", field(row, "gap_id")?),
                            entry_number: None,
                            title: field(&fix, "title")?.into(),
                            level: fix["level"].as_u64().ok_or("missing gap entry level")? as u32,
                            level_name: fix["level_name"].as_str().map(str::to_owned),
                            printed_page_number: None,
                            scan_page: None,
                            reasoning: String::new(),
                            origin: "validated".into(),
                        };
                        verified(&mut e, &fix, first["total_pages"].as_u64().unwrap() as u32)?;
                        es.push(e);
                    }
                    _ => return Err("unknown gap fix type".into()),
                }
            }
            if !pending.is_empty() {
                return Err("gap findings do not cover the planned set".into());
            }
            boundary_ready(first, es)
        }
        "book_ready" => {
            let mut kinds = BTreeMap::new();
            for row in rows {
                if kinds
                    .insert(field(row, "ready_kind")?, value(row, "payload_json")?)
                    .is_some()
                {
                    return Err("duplicate book prerequisite".into());
                }
            }
            let es = kinds
                .remove("boundaries")
                .ok_or("missing validated boundaries")?;
            let meta = kinds.remove("metadata").ok_or("missing book metadata")?;
            let boundary_context = rows
                .iter()
                .find(|r| r["ready_kind"] == "boundaries")
                .ok_or("missing boundary context")?;
            let mut classify = job(boundary_context, "classify_prepare")?;
            classify["entries_json"] = json!(es.to_string());
            classify["metadata_json"] = json!(meta.to_string());
            Ok(json!({"classify_prepare_job":classify}))
        }
        _ => Err("unknown grouped stage".into()),
    }
}
fn metadata(v: &Value) -> Result<Value> {
    let mut meta = value(v, "metadata")?;
    if meta["author"].is_null() {
        let authors = meta["authors"].as_array().ok_or("metadata needs authors")?;
        let names: Vec<_> = authors
            .iter()
            .map(|a| {
                a.as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("invalid author name")
            })
            .collect::<std::result::Result<_, _>>()?;
        meta["author"] = json!(names.join(", "));
    }
    for k in ["title", "author", "language"] {
        field(&meta, k)?;
    }
    ready(v, "metadata", meta)
}
fn ready(v: &Value, kind: &str, payload: Value) -> Result<Value> {
    let mut r = context(v)?;
    r["stage"] = json!("book_ready");
    r["ready_kind"] = json!(kind);
    r["ready_id"] = json!(format!("{}:ready:{kind}", field(v, "run_id")?));
    r["payload_json"] = json!(payload.to_string());
    r["expected_total"] = json!(2);
    r["plan_id"] = r["book_hash"].clone();
    r["expected_ids_json"] = json!("[\"metadata\",\"boundaries\"]");
    Ok(json!({"ready":r}))
}
fn number(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 15 {
        return None;
    }
    if let Ok(n) = s.parse() {
        return Some(n);
    }
    let mut n = 0;
    let mut previous = 0;
    for c in s.to_uppercase().chars().rev() {
        let x = match c {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => return None,
        };
        if x < previous {
            n -= x;
        } else {
            n += x;
            previous = x;
        }
    }
    if n > 0 && n <= 3999 && roman(n) == s.to_uppercase() {
        Some(n)
    } else {
        None
    }
}
fn roman(mut n: u32) -> String {
    let mut s = String::new();
    for (v, t) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while n >= v {
            s.push_str(t);
            n -= v;
        }
    }
    s
}
fn plan_discovery(v: &Value) -> Result<Value> {
    let es = entries(v)?;
    let analysis = value(v, "analysis")?;
    let patterns = analysis["discovered_patterns"]
        .as_array()
        .ok_or("missing discovered patterns")?;
    let excluded = analysis["excluded_page_ranges"]
        .as_array()
        .ok_or("missing excluded page ranges")?;
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    for range in excluded {
        let lo = range["start_page"].as_u64().unwrap_or(0);
        let hi = range["end_page"].as_u64().unwrap_or(0);
        if lo == 0 || lo > hi || hi > book.pages.len() as u64 {
            return Err("excluded range outside source pages".into());
        }
    }
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for p in patterns {
        if p["pattern_type"] != "sequential" {
            return Err(
                "non-sequential discovery needs an explicit target list; retain for review".into(),
            );
        }
        let start = field(p, "range_start")?;
        let end = field(p, "range_end")?;
        let a = number(start).ok_or("invalid sequence start")?;
        let b = number(end).ok_or("invalid sequence end")?;
        if a == 0 || a > b || b - a >= 256 {
            return Err("discovery sequence outside bounded fan-out".into());
        }
        let kind = field(p, "level_name")?;
        let level = p["level"]
            .as_u64()
            .filter(|n| (1..=3).contains(n))
            .ok_or("invalid discovery level")? as u32;
        for n in a..=b {
            let identifier = if start.chars().all(|c| c.is_ascii_digit()) {
                n.to_string()
            } else {
                roman(n)
            };
            if es.iter().any(|e| {
                e.level_name.as_deref() == Some(kind)
                    && e.entry_number.as_deref().and_then(number) == Some(n)
            }) {
                continue;
            }
            if !seen.insert((kind.to_owned(), n)) {
                return Err("overlapping discovery patterns".into());
            }
            let e = Entry {
                key: format!("{}:discover:{kind}:{identifier}", field(v, "run_id")?),
                entry_number: Some(identifier.clone()),
                title: format!("{kind} {identifier}"),
                level,
                level_name: Some(kind.into()),
                printed_page_number: None,
                scan_page: None,
                reasoning: String::new(),
                origin: "discovered".into(),
            };
            targets.push((e, p["heading_format"].clone()));
        }
    }
    if targets.is_empty() {
        return boundary_ready(v, es);
    }
    if targets.len() > 256 {
        return Err("discovery exceeds bounded fan-in".into());
    }
    let keys: Vec<_> = targets.iter().map(|(e, _)| &e.key).collect();
    let plan = hash(serde_json::to_string(&keys).unwrap().as_bytes());
    let jobs: Vec<_> = targets
        .iter()
        .map(|(e, format)| {
            let mut j = job(v, "discover").unwrap();
            j["entry_id"] = json!(e.key);
            j["stage_job_id"] = json!(e.key);
            j["entry_json"] = json!(serde_json::to_string(e).unwrap());
            j["heading_format"] = format.clone();
            j["entries_json"] = v["entries_json"].clone();
            j["expected_total"] = json!(targets.len());
            j["expected_ids_json"] = json!(serde_json::to_string(&keys).unwrap());
            j["plan_id"] = json!(plan);
            j["total_pages"] = json!(book.pages.len());
            j["excluded_json"] = json!(analysis["excluded_page_ranges"].to_string());
            j
        })
        .collect();
    Ok(json!({"discovery_jobs":jobs}))
}
fn plan_gaps(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let mut es = entries(v)?;
    es.sort_by_key(|e| (e.scan_page, e.level));
    let excluded = value(v, "excluded_json")?;
    validate_entries(&es)?;
    let body_start = v["body_start"]
        .as_u64()
        .ok_or("missing planned body start")? as u32;
    let body_end = v["body_end"].as_u64().ok_or("missing planned body end")? as u32;
    let is_excluded = |page: u32| {
        excluded.as_array().is_some_and(|rs| {
            rs.iter().any(|r| {
                page as u64 >= r["start_page"].as_u64().unwrap_or(0)
                    && page as u64 <= r["end_page"].as_u64().unwrap_or(0)
            })
        })
    };
    let mut gaps = Vec::new();
    let first = &es[0];
    let first_page = first.scan_page.ok_or("unresolved boundary")?;
    if first_page.saturating_sub(body_start) > 15 {
        gaps.push(json!({"gap_start":body_start,"gap_end":first_page-1,"entry_before":null,"entry_after":first,"body_start":body_start,"body_end":body_end,"total_pages":body_end}));
    }
    for pair in es.windows(2) {
        let a = pair[0].scan_page.ok_or("unresolved boundary")?;
        let b = pair[1].scan_page.ok_or("unresolved boundary")?;
        if b.saturating_sub(a) <= 15
            || excluded.as_array().is_some_and(|rs| {
                rs.iter().any(|r| {
                    a + 1 >= r["start_page"].as_u64().unwrap_or(0) as u32
                        && a + 1 <= r["end_page"].as_u64().unwrap_or(0) as u32
                })
            })
        {
            continue;
        }
        gaps.push(json!({"gap_start":a+1,"gap_end":b-1,"entry_before":pair[0],"entry_after":pair[1],"body_start":body_start,"body_end":body_end,"total_pages":book.pages.len()}));
    }
    let last = es.last().unwrap();
    let last_page = last.scan_page.ok_or("unresolved boundary")?;
    if body_end.saturating_sub(last_page) > 15 && !is_excluded(last_page + 1) {
        gaps.push(json!({"gap_start":last_page+1,"gap_end":body_end,"entry_before":last,"entry_after":null,"body_start":body_start,"body_end":body_end,"total_pages":body_end}));
    }
    if gaps.is_empty() {
        return boundary_ready(v, es);
    }
    let keys: Vec<_> = gaps
        .iter()
        .map(|g| {
            format!(
                "{}:gap:{}:{}",
                field(v, "run_id").unwrap(),
                g["gap_start"],
                g["gap_end"]
            )
        })
        .collect();
    let jobs: Vec<_> = gaps
        .iter()
        .zip(&keys)
        .map(|(g, key)| {
            let mut j = job(v, "gap").unwrap();
            j["gap_id"] = json!(key);
            j["stage_job_id"] = json!(key);
            j["gap_context_json"] = json!(g.to_string());
            j["entries_json"] = v["entries_json"].clone();
            j["total_pages"] = json!(book.pages.len());
            j["expected_total"] = json!(gaps.len());
            j["expected_ids_json"] = json!(serde_json::to_string(&keys).unwrap());
            j["plan_id"] = json!(hash(serde_json::to_string(&keys).unwrap().as_bytes()));
            j
        })
        .collect();
    Ok(json!({"gap_jobs":jobs}))
}
fn boundary_ready(v: &Value, mut es: Vec<Entry>) -> Result<Value> {
    es.sort_by_key(|e| (e.scan_page, e.level));
    validate_entries(&es)?;
    let mut original: Vec<_> = es.iter().filter(|e| e.origin == "toc").collect();
    original.sort_by(|a, b| a.key.cmp(&b.key));
    if original.windows(2).any(|p| p[0].scan_page > p[1].scan_page) {
        return Err("corrected boundaries contradict ToC order".into());
    }
    let mut keys = BTreeSet::new();
    let mut previous: Option<&Entry> = None;
    for e in &es {
        if !keys.insert(&e.key) || e.scan_page.is_none() || e.reasoning.trim().is_empty() {
            return Err("structure has duplicate or unverified entries".into());
        }
        if let Some(p) = previous {
            if p.scan_page == e.scan_page && e.level <= p.level {
                return Err("distinct siblings share a scan page; within-page boundary evidence is required".into());
            }
        }
        previous = Some(e);
    }
    ready(v, "boundaries", serde_json::to_value(es).unwrap())
}
fn pattern_prepare(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let es = entries(v)?;
    validate_entries(&es)?;
    let body_start = es
        .iter()
        .filter_map(|e| e.scan_page)
        .min()
        .ok_or("missing body start")? as u64;
    let body_end = es
        .iter()
        .filter_map(|e| e.scan_page)
        .max()
        .ok_or("missing body end")? as u64;
    let linked: BTreeSet<_> = es.iter().filter_map(|e| e.scan_page).collect();
    let candidates: Vec<_> = book
        .headings()
        .into_iter()
        .filter(|h| {
            let page = h["scan_page"].as_u64().unwrap();
            page >= body_start && page <= body_end && !linked.contains(&(page as u32))
        })
        .collect();
    let mut out = job(v, "pattern")?;
    out["entries_json"] = v["entries_json"].clone();
    out["total_pages"] = json!(book.pages.len());
    out["body_start"] = json!(body_start);
    out["body_end"] = json!(body_end);
    out["candidates_json"] = json!(serde_json::to_string(&candidates).unwrap());
    Ok(json!({"pattern_job":out}))
}
fn classify_prepare(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let mut es = entries(v)?;
    validate_entries(&es)?;
    let first = es[0].scan_page.ok_or("unverified opening")?;
    if first > 1 {
        let mut leading = Vec::new();
        let mut starts = vec![(1, "front", "Front matter", "front_matter")];
        let toc_start = v["toc_start"].as_u64().ok_or("missing ToC start")? as u32;
        let toc_end = v["toc_end"].as_u64().ok_or("missing ToC end")? as u32;
        if toc_start < first && toc_end < first {
            if toc_start == 1 {
                starts.clear();
            }
            starts.push((toc_start, "contents", "Contents", "table_of_contents"));
            if toc_end + 1 < first {
                starts.push((
                    toc_end + 1,
                    "front-after-toc",
                    "Front matter",
                    "front_matter",
                ));
            }
        }
        for (page, key, title, kind) in starts {
            leading.push(Entry {
                key: format!("{}:{key}", field(v, "run_id")?),
                title: title.into(),
                level: 1,
                entry_number: None,
                level_name: Some(kind.into()),
                printed_page_number: None,
                scan_page: Some(page),
                reasoning: if kind == "table_of_contents" {
                    "Contents range visually verified by the finder."
                } else {
                    "Leading source pages preserved before the first verified ToC entry."
                }
                .into(),
                origin: "source".into(),
            });
        }
        leading.extend(es);
        es = leading;
    }
    let mut lines = vec![
        format!("Total pages in book: {}", book.pages.len()),
        "Classify each entry with content_type, matter_type, and audio_include:".into(),
    ];
    for (i, e) in es.iter().enumerate() {
        let start = e.scan_page.ok_or("unverified chapter start")?;
        let owned_end = es
            .get(i + 1)
            .and_then(|n| n.scan_page)
            .map(|p| p - 1)
            .unwrap_or(book.pages.len() as u32);
        let end = es[i + 1..]
            .iter()
            .find(|n| n.level <= e.level)
            .and_then(|n| n.scan_page)
            .map(|p| p - 1)
            .unwrap_or(book.pages.len() as u32);
        let text = book
            .pages
            .iter()
            .filter(|p| p.scan_page >= start && p.scan_page <= owned_end)
            .map(|p| p.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = normalized.chars().take(200).collect();
        lines.push(format!(
            "{}. {:?} (pages {}-{}, level {} {}, word_count {}) [id: {}]\n   text: {}",
            i + 1,
            e.title,
            start,
            end.max(start),
            e.level,
            e.level_name.as_deref().unwrap_or(""),
            text.split_whitespace().count(),
            e.key,
            if snippet.is_empty() {
                "[no owned text]"
            } else {
                &snippet
            }
        ));
    }
    lines.push("Return JSON with classifications, content_types, audio_include, and reasoning. Each mapping must cover all listed entry IDs exactly, including Contents, blank front matter, and parent sections without owned text. Excluding an entry from narration means audio_include=false; it never means omitting the entry.".into());
    let mut out = job(v, "classify")?;
    out["entries_json"] = json!(serde_json::to_string(&es).unwrap());
    out["metadata_json"] = v["metadata_json"].clone();
    out["classification_prompt"] = json!(lines.join("\n"));
    Ok(json!({"classify_job":out}))
}
fn display_title(e: &Entry) -> String {
    if !e.title.trim().is_empty() {
        return e.title.clone();
    }
    let kind = e.level_name.as_deref().unwrap_or("section");
    let mut chars = kind.chars();
    let label = chars
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
        + chars.as_str();
    format!("{} {}", label, e.entry_number.as_deref().unwrap_or(""))
        .trim()
        .to_owned()
}
fn classify(v: &Value) -> Result<Value> {
    BookInput::load(Path::new(field(v, "path")?), v)?;
    let es = entries(v)?;
    validate_entries(&es)?;
    let validation = value(v, "classification").and_then(|c| validate_classification(&es, &c));
    if let Err(error) = validation {
        let attempt = v["retry_count"].as_u64().unwrap_or(0);
        if attempt >= 2 {
            return Err(format!(
                "classification failed after {} attempts: {error}",
                attempt + 1
            ));
        }
        let mut next = job(v, "classify")?;
        next["entries_json"] = v["entries_json"].clone();
        next["metadata_json"] = v["metadata_json"].clone();
        next["retry_count"] = json!(attempt + 1);
        next["classification_prompt"] = json!(format!(
            "{}\n\nNative validation rejected the previous result: {}\nPrevious result:\n{}\nSubmit a complete corrected result covering every listed entry ID in each mapping. Preserve all verified entries and boundaries.",
            field(v, "classification_prompt")?,
            error,
            v["classification"]
        ));
        return Ok(json!({"classify_job": next}));
    }
    assemble(v)
}
fn validate_classification(es: &[Entry], classifications: &Value) -> Result<()> {
    let expected: BTreeSet<_> = es.iter().map(|e| e.key.as_str()).collect();
    for k in [
        "classifications",
        "content_types",
        "audio_include",
        "reasoning",
    ] {
        let mapping = classifications[k]
            .as_object()
            .ok_or(format!("missing {k} mapping"))?;
        let actual = mapping.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(format!(
                "{k} does not cover the exact section set; missing IDs: {:?}; unexpected IDs: {:?}",
                expected.difference(&actual).collect::<Vec<_>>(),
                actual.difference(&expected).collect::<Vec<_>>()
            ));
        }
    }
    for e in es {
        let matter = classifications["classifications"][&e.key]
            .as_str()
            .ok_or("invalid matter classification")?;
        if !["front_matter", "body", "back_matter"].contains(&matter) {
            return Err(format!("invalid matter classification for {}", e.key));
        }
        let content = field(&classifications["content_types"], &e.key)?;
        if ![
            "body",
            "preface",
            "foreword",
            "introduction",
            "prologue",
            "epilogue",
            "afterword",
            "author_note",
            "dedication",
            "appendix",
            "index",
            "bibliography",
            "glossary",
            "notes",
            "endnotes",
            "acknowledgments",
            "about_author",
            "copyright",
            "illustrations_list",
            "other",
        ]
        .contains(&content)
        {
            return Err(format!("unknown content classification for {}", e.key));
        }
        if !classifications["audio_include"][&e.key].is_boolean() {
            return Err(format!("audio_include must be a Boolean for {}", e.key));
        }
        field(&classifications["reasoning"], &e.key)?;
    }
    Ok(())
}
fn assemble(v: &Value) -> Result<Value> {
    let book = BookInput::load(Path::new(field(v, "path")?), v)?;
    let es = entries(v)?;
    validate_entries(&es)?;
    let meta = value(v, "metadata_json")?;
    let classifications = value(v, "classification")?;
    validate_classification(&es, &classifications)?;
    let mut chapters = Vec::new();
    let mut stack: Vec<(u32, String)> = Vec::new();
    let mut covered = 0;
    for (i, e) in es.iter().enumerate() {
        let start = e.scan_page.ok_or("unverified chapter start")?;
        let owned_end = es
            .get(i + 1)
            .and_then(|n| n.scan_page)
            .map(|p| p - 1)
            .unwrap_or(book.pages.len() as u32);
        let end = es[i + 1..]
            .iter()
            .find(|n| n.level <= e.level)
            .and_then(|n| n.scan_page)
            .map(|p| p - 1)
            .unwrap_or(book.pages.len() as u32);
        while stack.last().is_some_and(|(level, _)| *level >= e.level) {
            stack.pop();
        }
        let parent = stack.last().map(|(_, key)| key.clone());
        stack.push((e.level, e.key.clone()));
        let matter = classifications["classifications"][&e.key].as_str().unwrap();
        let content = field(&classifications["content_types"], &e.key)?;
        let audio = classifications["audio_include"][&e.key].as_bool().unwrap();
        let reason = field(&classifications["reasoning"], &e.key)?;
        let pages: Vec<_> = book
            .pages
            .iter()
            .filter(|p| p.scan_page >= start && p.scan_page <= owned_end)
            .collect();
        covered += pages.len();
        let mut spans: Vec<Value> = Vec::new();
        for p in pages {
            if let Some(last) = spans.last_mut().filter(|last| {
                last["source"] == p.source
                    && last["end_page"].as_u64() == Some(u64::from(p.page - 1))
            }) {
                last["end_page"] = json!(p.page);
            } else {
                spans.push(json!({"source":p.source,"start_page":p.page,"end_page":p.page}));
            }
        }
        chapters.push(json!({"book_id":book.book_id,"chapter_key":e.key,"toc_entry_id":if e.origin=="toc"{Some(&e.key)}else{None},"sequence":i+1,"title":display_title(e),"toc_title":e.title,"entry_number":e.entry_number,"level":e.level,"level_name":e.level_name,"parent_key":parent,
            "matter_type":matter,"content_type":content,"audio_include":audio,"audio_include_reasoning":reason,"start_page":start,"end_page":end.max(start),"owned_end_page":owned_end,"source_ranges_json":serde_json::to_string(&spans).unwrap(),"review_notes":e.reasoning}));
    }
    if covered != book.pages.len() {
        return Err("chapter text ownership does not cover the exact source".into());
    }
    let sources: Vec<_> = book
        .sources
        .iter()
        .map(|s| json!({"source":s.source,"page_count":s.page_count,"sha256":s.sha256}))
        .collect();
    Ok(
        json!({"book":{"book_id":book.book_id,"title":field(&meta,"title")?,"author":field(&meta,"author")?,"language":field(&meta,"language")?,"source_book_file":v["book_file"],"source_book_hash":v["book_hash"],"metadata_json":meta.to_string(),"source_manifest":serde_json::to_string(&sources).unwrap(),"page_count":book.pages.len(),"chapter_count":chapters.len(),"review_notes":"ToC entries individually located; discovery and gap stages completed before chapter classification.","access":book.access,"license":book.license},
        "chapters":chapters,"report":{"book_id":book.book_id,"page_count":book.pages.len(),"chapter_count":es.len(),"covered_pages":covered,"review_notes":"Boundary evidence and narration decisions retained on each chapter.","quality_state":"boundaries_verified"}}),
    )
}

#[cfg(test)]
mod tests;
