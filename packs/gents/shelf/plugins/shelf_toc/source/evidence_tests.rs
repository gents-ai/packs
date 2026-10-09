use super::*;
use std::fs;
fn fixture() -> (tempfile::TempDir, Value) {
    let dir = tempfile::tempdir().unwrap();
    let book = BookInput {
        run_id: "run-a".into(),
        book_id: "work-a".into(),
        access: "local_only".into(),
        license: "unknown".into(),
        sources: vec![Source {
            source: "scan.pdf".into(),
            page_count: 10,
            asset: "scan.pdf".into(),
            sha256: hash(b"pdf"),
        }],
        pages: (1..=10)
            .map(|n| Page {
                scan_page: n,
                source: "scan.pdf".into(),
                page: n,
                markdown: match n {
                    2 => "# Contents\n\nChapter 1 1\nChapter 2 4".into(),
                    4 => "# Déjà vu (\n\nDéjà vu ( begins the narrative.\n\n## Subtitle".into(),
                    5 | 7 => "Déjà vu (\n\nMore prose.".into(),
                    8 => "# Another opening".into(),
                    _ => "Plain narrative.".into(),
                },
            })
            .collect(),
    };
    let raw = serde_json::to_vec(&book).unwrap();
    fs::write(dir.path().join("source-book.json"), &raw).unwrap();
    let v = json!({"path":dir.path(),"book_file":"source-book.json","book_hash":hash(&raw),"book_id":"work-a","run_id":"run-a","stage_job_id":"entry-a","command":"search"});
    (dir, v)
}
#[test]
fn regex_fallback_unicode_context_and_clusters_preserve_scan_coordinates() {
    let (_dir, mut v) = fixture();
    v["query"] = json!("Déjà vu (");
    let out = call(&v).unwrap();
    assert_eq!(out["matches"].as_array().unwrap().len(), 3);
    assert_eq!(out["clusters"][0]["start_page"], 4);
    assert_eq!(out["clusters"][0]["end_page"], 7);
    assert_eq!(out["matches"][0]["match_count"], 2);
    assert!(
        out["matches"][0]["context_snippets"][0]
            .as_str()
            .unwrap()
            .contains("Déjà")
    );
    v["start_page"] = json!(5);
    v["end_page"] = json!(6);
    assert_eq!(call(&v).unwrap()["matches"].as_array().unwrap().len(), 1);
}
#[test]
fn headings_exclude_contents_and_use_first_heading_per_page() {
    let (_dir, mut v) = fixture();
    v["command"] = json!("headings");
    v["toc_start"] = json!("2");
    v["toc_end"] = json!("2");
    let out = call(&v).unwrap();
    let headings = out["headings"].as_array().unwrap();
    assert_eq!(headings.len(), 2);
    assert_eq!(headings[0]["scan_page"], 4);
    assert_eq!(headings[1]["scan_page"], 8);
    v["command"] = json!("frontmatter");
    assert_eq!(
        call(&v).unwrap()["report"]["categorized_pages"]["toc"],
        json!([2])
    );
    v["command"] = json!("ocr");
    v["page_num"] = json!(0);
    assert!(call(&v).is_err());
    v["page_num"] = json!(8);
    assert_eq!(call(&v).unwrap()["source_page"], 8);
}
#[test]
fn different_book_or_modified_snapshot_is_refused() {
    let (dir, mut v) = fixture();
    v["book_id"] = json!("work-b");
    assert!(call(&v).unwrap_err().contains("another book"));
    v["book_id"] = json!("work-a");
    fs::write(dir.path().join("source-book.json"), b"{}").unwrap();
    assert!(call(&v).unwrap_err().contains("changed"));
}
#[test]
fn visual_findings_are_persisted_and_observations_are_required_between_pages() {
    let (dir, mut v) = fixture();
    v["command"] = json!("image");
    v["page_num"] = json!(4);
    v["model_calls"] = json!(true);
    v["state"] = json!({"book_hash":v["book_hash"],"stage_job_id":v["stage_job_id"],"scan_page":4,"previous_page":null});
    v["model_results"] = json!({"page":{"text":"Chapter One begins here. Printed label 1."}});
    let out = call(&v).unwrap();
    assert_eq!(out["current_page"], 4);
    assert!(
        dir.path()
            .join(out["inspection_file"].as_str().unwrap())
            .exists()
    );
    v.as_object_mut().unwrap().remove("model_results");
    v["page_num"] = json!(5);
    assert!(call(&v).unwrap_err().contains("current_page_observations"));
    v["current_page_observations"] = json!("Chapter One opening visually confirmed.");
    v["state"] = json!({"book_hash":v["book_hash"],"stage_job_id":v["stage_job_id"],"scan_page":5,"previous_page":4});
    v["model_results"] = json!({"page":{"text":"Continuation of body prose. Printed label 2."}});
    let out = call(&v).unwrap();
    assert_eq!(out["observations"].as_array().unwrap().len(), 3);
    v["stage_job_id"] = json!("different-entry");
    assert!(call(&v).unwrap_err().contains("does not match"));
}
#[test]
fn failed_vision_call_does_not_advance_current_page() {
    let (dir, mut v) = fixture();
    v["command"] = json!("image");
    v["page_num"] = json!(4);
    v["model_calls"] = json!(true);
    v["state"] = json!({"book_hash":v["book_hash"],"stage_job_id":v["stage_job_id"],"scan_page":4,"previous_page":null});
    v["model_results"] = json!({"page":{"error":"backend unavailable"}});
    assert!(call(&v).unwrap_err().contains("vision inspection failed"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
