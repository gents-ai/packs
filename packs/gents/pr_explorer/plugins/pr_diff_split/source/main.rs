//! pr_diff_split: reads a `git diff` patch the pr-explorer agent wrote into
//! `<root>/.pr-explorer/patches/`, splits it per file, and returns one
//! `PrExplorerDiff` insert per file (the host commits them with the tool
//! call) beside a compact file list for the model. A last call with
//! `remove_patch` deletes the patch once its diffs are stored. One JSON value
//! on stdin, one on stdout; stderr carries diagnostics (see TOOL.md).
use std::io::Read;
use std::path::Path;

use serde_json::{json, Map, Value};

mod split;

const COLLECTION: &str = "PrExplorerDiff";
/// The host commits at most this many writes per call (`plugin/data.rs`).
const MAX_WRITES: usize = 100;
const DEFAULT_MAX_FILE_BYTES: usize = 256 * 1024;
/// Serialized writes one call may carry in total: under the host's 4 MiB
/// output ceiling with room for the file list.
const DEFAULT_MAX_BATCH_BYTES: usize = 3 * 1024 * 1024;
/// Files past this ordinal are listed as dropped, not written.
const MAX_FILES: usize = 2000;
const MAX_PATCH_BYTES: u64 = 512 * 1024 * 1024;
/// The only folder this plugin works in: it may delete files there, so it
/// refuses any other bound folder even when the host would allow it.
const PATCH_DIR: &str = ".pr-explorer/patches";

fn main() {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("pr_diff_split: reading stdin: {err}");
        std::process::exit(1);
    }
    match run(&input) {
        Ok(output) => println!("{output}"),
        Err(err) => {
            eprintln!("pr_diff_split: {err}");
            std::process::exit(1);
        }
    }
}

struct Args {
    dir: String,
    patch: String,
    remove_patch: bool,
    analysis_id: String,
    offset: usize,
    max_files: usize,
    max_file_bytes: usize,
    max_batch_bytes: usize,
}

fn uint(obj: &Map<String, Value>, key: &str, default: usize) -> Result<usize, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("{key:?} must be a non-negative integer")),
    }
}

fn text(obj: &Map<String, Value>, key: &str) -> Result<String, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key:?} must be a string"))
}

fn parse_args(input: &str) -> Result<Args, String> {
    let value: Value =
        serde_json::from_str(input).map_err(|err| format!("invalid JSON input: {err}"))?;
    let obj = value.as_object().ok_or("input must be a JSON object")?;
    let analysis_id = text(obj, "analysis_id")?;
    if analysis_id.is_empty()
        || analysis_id.len() > 64
        || !analysis_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err("\"analysis_id\" must be 1-64 characters of [A-Za-z0-9._-]".into());
    }
    let patch = text(obj, "patch")?;
    if !is_patch_name(&patch) {
        return Err(format!(
            "\"patch\" must be a file name of [A-Za-z0-9._-] ending in .patch (no folders), got {patch:?}"
        ));
    }
    let remove_patch = match obj.get("remove_patch") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err("\"remove_patch\" must be a boolean".into()),
    };
    let max_files = uint(obj, "max_files", MAX_WRITES)?;
    if !(1..=MAX_WRITES).contains(&max_files) {
        return Err(format!("\"max_files\" must be 1..={MAX_WRITES}"));
    }
    let max_file_bytes = uint(obj, "max_file_bytes", DEFAULT_MAX_FILE_BYTES)?;
    if !(1024..=DEFAULT_MAX_FILE_BYTES).contains(&max_file_bytes) {
        return Err(format!("\"max_file_bytes\" must be 1024..={DEFAULT_MAX_FILE_BYTES}"));
    }
    Ok(Args {
        dir: text(obj, "dir")?,
        patch,
        remove_patch,
        analysis_id,
        offset: uint(obj, "offset", 0)?,
        max_files,
        max_file_bytes,
        max_batch_bytes: DEFAULT_MAX_BATCH_BYTES,
    })
}

/// `name.patch`: one file name, no folder, no leading dot.
fn is_patch_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".patch") else {
        return false;
    };
    !stem.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// The bound folder must be `…/.pr-explorer/patches`.
fn is_patch_dir(dir: &Path) -> bool {
    dir.is_absolute() && dir.ends_with(PATCH_DIR)
}

fn run(input: &str) -> Result<Value, String> {
    let args = parse_args(input)?;
    let dir = Path::new(&args.dir);
    if !is_patch_dir(dir) || !dir.is_dir() {
        return Err(format!(
            "\"dir\" must be the bound <root>/{PATCH_DIR} folder, got {:?}",
            args.dir
        ));
    }
    let path = dir.join(&args.patch);
    if args.remove_patch {
        return remove(&path, &args);
    }
    let size = std::fs::metadata(&path)
        .map_err(|err| format!("reading {}: {err}", args.patch))?
        .len();
    if size > MAX_PATCH_BYTES {
        return Err(format!("{} is {size} bytes; the bound is {MAX_PATCH_BYTES}", args.patch));
    }
    let bytes = std::fs::read(&path).map_err(|err| format!("reading {}: {err}", args.patch))?;
    let patch = String::from_utf8_lossy(&bytes);
    Ok(page(&split::split(&patch), &args))
}

/// Deletes the patch once its diffs are stored; a patch already gone is not
/// an error (a repeated cleanup call).
fn remove(path: &Path, args: &Args) -> Result<Value, String> {
    let removed = match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(format!("removing {}: {err}", args.patch)),
    };
    Ok(json!({ "analysis_id": args.analysis_id, "patch": args.patch, "removed": removed }))
}

/// The page of `files` starting at `args.offset`: writes for the files it
/// takes, a summary of every file for the model, and where the next call
/// starts (null when this page ends the analysis).
fn page(files: &[split::FileDiff], args: &Args) -> Value {
    let kept = files.len().min(MAX_FILES);
    let start = args.offset.min(kept);
    let mut writes = Vec::new();
    let mut listed = Vec::new();
    let mut batch_bytes = 0usize;
    let mut next = start;
    for (ordinal, file) in files.iter().enumerate().take(kept).skip(start) {
        if next - start >= args.max_files {
            break;
        }
        let bounded = split::bound(&file.diff, args.max_file_bytes);
        let diff = bounded.unwrap_or(&file.diff);
        let write = json!({
            "op": "insert",
            "collection": COLLECTION,
            "fields": {
                "file_key": format!("{}:{}", args.analysis_id, ordinal),
                "analysis_id": args.analysis_id,
                "ordinal": ordinal,
                "path": file.path,
                "old_path": file.old_path,
                "change": file.change,
                "binary": file.binary,
                "additions": file.additions,
                "deletions": file.deletions,
                "diff": diff,
                "diff_bytes": file.diff.len(),
                "truncated": bounded.is_some(),
            },
            "on_conflict": "ignore",
        });
        let size = write.to_string().len();
        if next > start && batch_bytes + size > args.max_batch_bytes {
            break;
        }
        batch_bytes += size;
        next += 1;
        writes.push(write);
        listed.push(json!({
            "ordinal": ordinal,
            "path": file.path,
            "old_path": file.old_path,
            "change": file.change,
            "binary": file.binary,
            "additions": file.additions,
            "deletions": file.deletions,
            "truncated": bounded.is_some(),
        }));
    }
    let dropped: Vec<&str> = files[kept..].iter().map(|f| f.path.as_str()).collect();
    json!({
        "analysis_id": args.analysis_id,
        "total_files": files.len(),
        "offset": start,
        "next_offset": if next < kept { json!(next) } else { Value::Null },
        "files": listed,
        "dropped_over_limit": dropped.len(),
        "writes": writes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, diff_len: usize) -> split::FileDiff {
        split::FileDiff {
            path: path.into(),
            old_path: path.into(),
            change: "M",
            binary: false,
            additions: 1,
            deletions: 0,
            diff: "x\n".repeat(diff_len / 2),
        }
    }

    fn args(offset: usize, max_files: usize) -> Args {
        Args {
            dir: "/r/.pr-explorer/patches".into(),
            patch: "p.patch".into(),
            remove_patch: false,
            analysis_id: "abc".into(),
            offset,
            max_files,
            max_file_bytes: 1024,
            max_batch_bytes: 4096,
        }
    }

    #[test]
    fn pages_by_count_and_reports_the_next_offset() {
        let files: Vec<_> = (0..5).map(|i| file(&format!("f{i}"), 10)).collect();
        let first = page(&files, &args(0, 2));
        assert_eq!(first["next_offset"], 2);
        assert_eq!(first["writes"].as_array().unwrap().len(), 2);
        let last = page(&files, &args(4, 2));
        assert_eq!(last["next_offset"], Value::Null);
        assert_eq!(last["files"][0]["ordinal"], 4);
        let past = page(&files, &args(9, 2));
        assert_eq!(past["writes"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn bounds_each_file_and_marks_it_truncated() {
        let out = page(&[file("big", 4000)], &args(0, 10));
        let fields = &out["writes"][0]["fields"];
        assert_eq!(fields["truncated"], true);
        assert_eq!(fields["diff_bytes"], 4000);
        assert!(fields["diff"].as_str().unwrap().len() <= 1024);
        assert_eq!(out["files"][0]["truncated"], true);
    }

    #[test]
    fn a_batch_stops_at_its_byte_budget_but_always_takes_one_file() {
        let files: Vec<_> = (0..4).map(|i| file(&format!("f{i}"), 1000)).collect();
        let out = page(&files, &args(0, 100));
        // Each write serializes to ~1.7 KB (500 "x\n" lines escape to 1500
        // bytes, plus fields): two fit the 4096-byte budget, a third not.
        assert_eq!(out["next_offset"], 2);
        let one = page(&[file("huge", 1000)], &Args { max_batch_bytes: 10, ..args(0, 100) });
        assert_eq!(one["writes"].as_array().unwrap().len(), 1, "a page always takes one file");
    }

    #[test]
    fn writes_are_idempotent_inserts_into_the_declared_collection() {
        let out = page(&[file("a", 2)], &args(0, 1));
        assert_eq!(out["writes"][0]["op"], "insert");
        assert_eq!(out["writes"][0]["collection"], COLLECTION);
        assert_eq!(out["writes"][0]["on_conflict"], "ignore");
        // on_conflict resolves on the collection's one unique key
        assert_eq!(out["writes"][0]["fields"]["file_key"], "abc:0");
        assert!(out["files"][0].get("diff").is_none(), "the model never gets diff text");
    }

    #[test]
    fn refuses_patch_names_that_are_paths_and_bad_ids() {
        let ok = r#"{"dir":"/r/.pr-explorer/patches","patch":"abc-1.patch","analysis_id":"a"}"#;
        assert!(parse_args(ok).is_ok());
        for name in ["../x.patch", "/etc/x.patch", "a/b.patch", ".hidden.patch", "x.diff", ".patch", "x"] {
            let input = format!(r#"{{"dir":"/r/.pr-explorer/patches","patch":"{name}","analysis_id":"a"}}"#);
            assert!(parse_args(&input).is_err(), "{name}");
        }
        let bad_id = r#"{"dir":"/r/.pr-explorer/patches","patch":"p.patch","analysis_id":"a b"}"#;
        assert!(parse_args(bad_id).is_err());
        let too_many = r#"{"dir":"/r/.pr-explorer/patches","patch":"p.patch","analysis_id":"a","max_files":101}"#;
        assert!(parse_args(too_many).is_err());
        let bad_flag = r#"{"dir":"/r/.pr-explorer/patches","patch":"p.patch","analysis_id":"a","remove_patch":"yes"}"#;
        assert!(parse_args(bad_flag).is_err());
    }

    #[test]
    fn works_only_in_a_pr_explorer_patches_folder() {
        assert!(is_patch_dir(Path::new("/Users/me/Repos/.pr-explorer/patches")));
        assert!(!is_patch_dir(Path::new("/Users/me/Repos/packs")));
        assert!(!is_patch_dir(Path::new("/Users/me/Repos/packs/.git")));
        assert!(!is_patch_dir(Path::new(".pr-explorer/patches")));
        assert!(!is_patch_dir(Path::new("/x/.pr-explorer/patches/sub")));
        let elsewhere = r#"{"dir":"/","patch":"p.patch","analysis_id":"a","remove_patch":true}"#;
        assert!(run(elsewhere).unwrap_err().contains(".pr-explorer/patches"));
    }

    #[test]
    fn remove_deletes_only_the_named_patch_and_tolerates_a_repeat() {
        let dir = std::env::current_dir().unwrap().join("target/test-tmp/remove/.pr-explorer/patches");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-1.patch"), "x").unwrap();
        std::fs::write(dir.join("other.patch"), "y").unwrap();
        let input = format!(
            r#"{{"dir":{:?},"patch":"a-1.patch","analysis_id":"a-1","remove_patch":true}}"#,
            dir.to_str().unwrap()
        );
        let first = run(&input).unwrap();
        assert_eq!(first, json!({"analysis_id": "a-1", "patch": "a-1.patch", "removed": true}));
        assert!(!dir.join("a-1.patch").exists());
        assert!(dir.join("other.patch").exists(), "only the named patch goes");
        assert_eq!(run(&input).unwrap()["removed"], false);
        std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap()).unwrap();
    }
}
