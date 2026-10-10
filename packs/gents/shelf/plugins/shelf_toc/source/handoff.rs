use super::*;
use std::{fs, io::Write};

const RESULT_STAGES: &[&str] = &[
    "toc_find",
    "toc_extract",
    "pattern",
    "metadata",
    "classify",
    "toc_link",
    "discover",
    "gap",
];
const JOB_OUTPUTS: &[&str] = &[
    "toc_find_job",
    "metadata_job",
    "toc_extract_job",
    "link_jobs",
    "pattern_job",
    "discovery_jobs",
    "gap_jobs",
    "classify_job",
];

pub fn capture_jobs(input: &Value, output: &mut Value) -> Result<()> {
    for key in JOB_OUTPUTS {
        if output[*key].is_null() {
            continue;
        }
        let root = Path::new(field(input, "path")?);
        if let Some(jobs) = output[*key].as_array_mut() {
            for job in jobs {
                capture(root, job)?;
            }
        } else {
            capture(root, &mut output[*key])?;
        }
    }
    Ok(())
}
pub fn capture(root: &Path, job: &mut Value) -> Result<()> {
    job.as_object_mut()
        .ok_or("expected stage job")?
        .remove("job_file");
    job.as_object_mut().unwrap().remove("job_hash");
    let name = format!(
        "job-{}.json",
        &hash(field(job, "stage_job_id")?.as_bytes())[..32]
    );
    let raw = serde_json::to_vec(job).map_err(|e| e.to_string())?;
    if raw.len() > 8 * 1024 * 1024 {
        return Err("stage input exceeds 8 MB".into());
    }
    let target = root.join(&name);
    if target.try_exists().map_err(|e| e.to_string())? {
        if read(root, &name, 8 * 1024 * 1024)? != raw {
            return Err("immutable stage input changed; start a new discovery run".into());
        }
    } else {
        let pending = root.join(format!("{name}.pending"));
        let mut file = fs::File::create(&pending).map_err(|e| e.to_string())?;
        file.write_all(&raw).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(pending, target).map_err(|e| e.to_string())?;
    }
    job["job_file"] = json!(name);
    job["job_hash"] = json!(hash(&raw));
    Ok(())
}
pub fn hydrate(input: Value) -> Result<Value> {
    if !RESULT_STAGES.contains(&field(&input, "stage")?) {
        return Ok(input);
    }
    let root = Path::new(field(&input, "path")?);
    let raw = read(root, field(&input, "job_file")?, 8 * 1024 * 1024)?;
    if hash(&raw) != field(&input, "job_hash")? {
        return Err("stage evidence changed; start a new discovery run".into());
    }
    let mut job: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    for key in [
        "run_id",
        "book_id",
        "book_file",
        "book_hash",
        "stage",
        "stage_job_id",
    ] {
        if input[key] != job[key] {
            return Err(format!("finding belongs to another stage: {key}"));
        }
    }
    for key in ["expected_total", "plan_id"] {
        if !input[key].is_null() && input[key] != job[key] {
            return Err(format!("finding changed planned {key}"));
        }
    }
    let result_fields: &[&str] = match field(&job, "stage")? {
        "toc_find" => &[
            "toc_found",
            "toc_page_range",
            "confidence",
            "search_strategy_used",
            "reasoning",
            "structure_summary",
        ],
        "toc_extract" => &["extraction"],
        "toc_link" => &["scan_page", "reasoning"],
        "pattern" => &["analysis"],
        "discover" => &["scan_page", "found_title", "reasoning"],
        "gap" => &["fix"],
        "metadata" => &["metadata"],
        "classify" => &["classification"],
        _ => unreachable!(),
    };
    for key in result_fields {
        job[*key] = input[*key].clone();
    }
    job["path"] = input["path"].clone();
    if !input["path_original"].is_null() {
        job["path_original"] = input["path_original"].clone();
    }
    Ok(job)
}
pub fn group(input: &Value) -> Result<Value> {
    let rows = input.as_array().ok_or("expected finding group")?;
    let first = rows.first().ok_or("empty finding group")?;
    if first["stage"] == "book_ready" {
        return join(input);
    }
    let total = first["expected_total"]
        .as_u64()
        .ok_or("missing planned count")?;
    if total == 0 || total != rows.len() as u64 {
        return Err("incomplete finding group".into());
    }
    let mut ids = BTreeSet::new();
    for row in rows {
        for key in [
            "run_id",
            "book_id",
            "path",
            "book_file",
            "book_hash",
            "stage",
            "plan_id",
            "expected_total",
        ] {
            if row[key] != first[key] {
                return Err(format!("finding group mixes {key}"));
            }
        }
        field(row, "job_file")?;
        field(row, "job_hash")?;
        if !ids.insert(field(row, "stage_job_id")?) {
            return Err("duplicate finding in group".into());
        }
    }
    let mut next = context(first)?;
    next["stage"] = json!("join_ready");
    next["stage_job_id"] = json!(format!(
        "{}:join:{}",
        field(first, "run_id")?,
        &hash(input.to_string().as_bytes())[..20]
    ));
    next["findings_json"] = json!(input.to_string());
    Ok(json!({"join_job":next}))
}
pub fn finish_group(input: &Value) -> Result<Value> {
    let mut rows = value(input, "findings_json")?;
    let original = input["path_original"]
        .as_str()
        .unwrap_or(field(input, "path")?);
    for row in rows.as_array_mut().ok_or("expected findings array")? {
        if field(row, "path")? != original {
            return Err("group references another book folder".into());
        }
        row["path"] = input["path"].clone();
        row["path_original"] = json!(original);
        normalize_bound_counts(row)?;
        *row = hydrate(row.take())?;
    }
    join(&rows)
}
