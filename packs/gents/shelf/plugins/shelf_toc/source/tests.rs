use super::*;
use std::fs;

fn fixture(counts: &[u32]) -> (tempfile::TempDir, Value) {
    let dir = tempfile::tempdir().unwrap();
    let mut sources = Vec::new();
    let mut pages = Vec::new();
    for (i, count) in counts.iter().enumerate() {
        let name = format!("source-{i}.pdf");
        sources.push(Source {
            source: name.clone(),
            page_count: *count,
            asset: name.clone(),
            sha256: hash(b"pdf"),
        });
        for page in 1..=*count {
            pages.push(Page {
                scan_page: pages.len() as u32 + 1,
                source: name.clone(),
                page,
                markdown: format!(
                    "# Heading {page}\n\nOriginal prose on {name}, physical page {page}."
                ),
            });
        }
    }
    let book = BookInput {
        run_id: "run-a".into(),
        book_id: "book-a".into(),
        sources,
        pages,
        access: "local".into(),
        license: "private".into(),
    };
    let raw = serde_json::to_vec(&book).unwrap();
    fs::write(dir.path().join("source-book.json"), &raw).unwrap();
    let job = json!({"run_id":"run-a", "book_id":"book-a", "path":dir.path(), "book_file":"source-book.json", "book_hash":hash(&raw), "stage":"start"});
    (dir, job)
}
fn found(job: &Value) -> Value {
    let mut result = start(job).unwrap()["toc_find_job"].clone();
    result["toc_found"] = json!(true);
    result["confidence"] = json!(0.95);
    result["search_strategy_used"] = json!("grep_report");
    result["toc_page_range"] = json!({"start_page":2,"end_page":3});
    result["structure_summary"] =
        json!({"total_levels":3,"level_patterns":{"1":{"visual":"uppercase","numbering":"Roman"}}});
    result["structure_notes"] = json!({"continuation_pages":[3]});
    result["reasoning"] = json!("Both pages inspected; the second continues the contents.");
    let name = format!(
        "inspection-{}.json",
        &hash(result["stage_job_id"].as_str().unwrap().as_bytes())[..32]
    );
    let observations: Vec<_> = (2..=4).map(|n| json!({"page_num":n,"visual_observations":"Contents continuation and following page inspected."})).collect();
    fs::write(
        Path::new(job["path"].as_str().unwrap()).join(name),
        serde_json::to_vec(&json!({"current_page":4,"observations":observations})).unwrap(),
    )
    .unwrap();
    toc_found(&result).unwrap()["toc_extract_job"].clone()
}
fn linked(job: &Value, raw: Value, openings: &[u32]) -> Value {
    let mut extraction = found(job);
    extraction["extraction"] = json!({"entries":raw});
    let mut jobs = plan_links(&extraction).unwrap()["link_jobs"]
        .as_array()
        .unwrap()
        .clone();
    for (j, page) in jobs.iter_mut().zip(openings) {
        j["scan_page"] = json!(page);
        j["reasoning"] = json!(format!(
            "Heading and adjacent page evidence establish opening {page}."
        ));
    }
    jobs.reverse();
    join(&json!(jobs)).unwrap()["pattern_prepare_job"].clone()
}
#[test]
fn runtime_filled_string_counts_survive_object_and_group_handoffs() {
    let (_dir, job) = fixture(&[20]);
    let mut extraction = found(&job);
    for key in ["toc_start", "toc_end", "total_pages"] {
        extraction[key] = json!(extraction[key].to_string());
    }
    extraction["extraction"] = json!(
        json!({"entries":[
            {"title":"First", "level":1}, {"title":"Second", "level":1}
        ]})
        .to_string()
    );
    let mut jobs = dispatch(extraction).unwrap()["link_jobs"]
        .as_array()
        .unwrap()
        .clone();
    for (finding, page) in jobs.iter_mut().zip([4, 12]) {
        for key in ["toc_start", "toc_end", "total_pages", "expected_total"] {
            finding[key] = json!(finding[key].to_string());
        }
        finding["scan_page"] = json!(page);
        finding["reasoning"] = json!("The heading and adjacent prose confirm the opening.");
    }
    jobs.reverse();
    let result = dispatch(json!(jobs)).unwrap();
    assert_eq!(result["pattern_prepare_job"]["total_pages"], 20);
    jobs[0]["expected_total"] = json!("-1");
    assert!(
        dispatch(json!(jobs)).unwrap()["failure"]["reason"]
            .as_str()
            .unwrap()
            .contains("invalid bound expected_total")
    );
}

#[test]
fn preserved_hierarchy_owns_text_once_across_source_parts() {
    for counts in [vec![20], vec![8, 12], vec![4, 4, 12]] {
        let (_dir, job) = fixture(&counts);
        let raw = json!([
            {"title":"","entry_number":"I","level":1,"level_name":"part","printed_page_number":"1"},
            {"title":"Opening","entry_number":"1","level":2,"level_name":"chapter","printed_page_number":"1"},
            {"title":"Close reading","entry_number":null,"level":3,"level_name":"section","printed_page_number":"iv"},
            {"title":"Second chapter","entry_number":"2","level":2,"level_name":"chapter","printed_page_number":"8"},
            {"title":"Notes","level":1,"level_name":"notes","printed_page_number":"17"}
        ]);
        let linked = linked(&job, raw, &[4, 4, 6, 11, 18]);
        let mut pattern = pattern_prepare(&linked).unwrap()["pattern_job"].clone();
        pattern["analysis"] = json!({"discovered_patterns":[],"excluded_page_ranges":[{"start_page":18,"end_page":20}]});
        let boundaries = plan_discovery(&pattern).unwrap()["ready"].clone();
        let mut meta = start(&job).unwrap()["metadata_job"].clone();
        meta["metadata"] =
            json!({"title":"An unfamiliar book","author":"An Author","language":"en"});
        let metadata = metadata(&meta).unwrap()["ready"].clone();
        let prepare = join(&json!([metadata, boundaries])).unwrap()["classify_prepare_job"].clone();
        let mut classify = classify_prepare(&prepare).unwrap()["classify_job"].clone();
        let es = entries(&classify).unwrap();
        assert_eq!(es.len(), 7);
        assert_eq!(es[0].origin, "source");
        assert!(
            classify["classification_prompt"]
                .as_str()
                .unwrap()
                .contains(&es[0].key)
        );
        let mut classifications =
            json!({"classifications":{},"content_types":{},"audio_include":{},"reasoning":{}});
        for e in &es {
            let excluded = e.origin == "source" || e.level_name.as_deref() == Some("notes");
            classifications["classifications"][&e.key] = json!(if e.origin == "source" {
                "front_matter"
            } else if excluded {
                "back_matter"
            } else {
                "body"
            });
            classifications["content_types"][&e.key] = json!(if e.origin == "source" {
                "other"
            } else if excluded {
                "notes"
            } else {
                "body"
            });
            classifications["audio_include"][&e.key] = json!(!excluded);
            classifications["reasoning"][&e.key] = json!(if excluded {
                "Reference material is excluded from narration."
            } else {
                "Narrative text is included."
            });
        }
        classify["classification"] = classifications;
        let out = assemble(&classify).unwrap();
        let ch = &out["chapters"].as_array().unwrap()[1..];
        assert_eq!(ch[1]["entry_number"], "I");
        assert_eq!(ch[1]["owned_end_page"], 3);
        assert_eq!(ch[1]["end_page"], 17);
        assert_eq!(ch[2]["parent_key"], ch[1]["chapter_key"]);
        assert_eq!(ch[3]["parent_key"], ch[2]["chapter_key"]);
        assert_eq!(ch[4]["parent_key"], ch[1]["chapter_key"]);
        assert_eq!(ch[5]["audio_include"], false);
        assert_eq!(out["report"]["covered_pages"], 20);
        let spans: Vec<Value> = out["chapters"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|c| {
                serde_json::from_str::<Vec<Value>>(c["source_ranges_json"].as_str().unwrap())
                    .unwrap()
            })
            .collect();
        let physical: Vec<_> = spans
            .iter()
            .flat_map(|s| {
                (s["start_page"].as_u64().unwrap()..=s["end_page"].as_u64().unwrap())
                    .map(|p| (s["source"].clone(), p))
            })
            .collect();
        assert_eq!(physical.len(), 20);
        assert_eq!(
            physical
                .iter()
                .map(|(s, p)| format!("{s}:{p}"))
                .collect::<BTreeSet<_>>()
                .len(),
            20
        );
    }
}
#[test]
fn findings_cannot_reorder_contents_or_omit_members() {
    let (_dir, job) = fixture(&[20]);
    let mut extraction = found(&job);
    extraction["extraction"] =
        json!({"entries":[{"title":"First","level":1},{"title":"Second","level":1}]});
    let mut jobs = plan_links(&extraction).unwrap()["link_jobs"]
        .as_array()
        .unwrap()
        .clone();
    for (j, p) in jobs.iter_mut().zip([12, 4]) {
        j["scan_page"] = json!(p);
        j["reasoning"] = json!("Evidence.");
    }
    assert!(
        join(&json!(jobs))
            .unwrap_err()
            .contains("contradict ToC order")
    );
    assert!(
        join(&json!([jobs[0].clone()]))
            .unwrap_err()
            .contains("incomplete")
    );
    assert!(
        join(&json!([jobs[0].clone(), jobs[0].clone()]))
            .unwrap_err()
            .contains("duplicate")
    );
    jobs[1]["scan_page"] = json!(15);
    jobs[1]["book_hash"] = json!("another book");
    assert!(join(&json!(jobs)).unwrap_err().contains("mix book_hash"));
}
#[test]
fn ambiguous_siblings_and_missing_narration_decisions_block() {
    let (_dir, job) = fixture(&[20]);
    let es = vec![
        Entry {
            key: "a".into(),
            title: "One".into(),
            level: 1,
            entry_number: None,
            level_name: None,
            printed_page_number: None,
            scan_page: Some(4),
            reasoning: "evidence".into(),
            origin: "toc".into(),
        },
        Entry {
            key: "b".into(),
            title: "Two".into(),
            level: 1,
            entry_number: None,
            level_name: None,
            printed_page_number: None,
            scan_page: Some(4),
            reasoning: "evidence".into(),
            origin: "toc".into(),
        },
    ];
    assert!(
        boundary_ready(&job, es)
            .unwrap_err()
            .contains("within-page boundary")
    );
    let mut v = job.clone();
    v["entries_json"] = json!("[]");
    v["metadata_json"] = json!("{}");
    assert!(assemble(&v).is_err());
    assert!(classify_prepare(&v).is_err());
}
#[test]
fn source_identity_hash_and_coverage_are_enforced() {
    let (dir, job) = fixture(&[5, 7]);
    let mut wrong = job.clone();
    wrong["book_id"] = json!("another-book");
    assert!(
        BookInput::load(dir.path(), &wrong)
            .err()
            .unwrap()
            .contains("another book")
    );
    wrong = job.clone();
    wrong["book_hash"] = json!("tampered");
    assert!(
        BookInput::load(dir.path(), &wrong)
            .err()
            .unwrap()
            .contains("changed")
    );
    let mut book = BookInput::load(dir.path(), &job).unwrap();
    book.pages[5].page = 2;
    let raw = serde_json::to_vec(&book).unwrap();
    fs::write(dir.path().join("source-book.json"), &raw).unwrap();
    wrong["book_hash"] = json!(hash(&raw));
    assert!(
        BookInput::load(dir.path(), &wrong)
            .err()
            .unwrap()
            .contains("exact manifest")
    );
}
#[test]
fn roman_sequences_and_unresolved_discoveries_are_explicit() {
    assert_eq!(number("ix"), Some(9));
    assert_eq!(number("IIX"), None);
    let (_dir, job) = fixture(&[40]);
    let mut pattern = linked(
        &job,
        json!([{"title":"Opening","level":1,"level_name":"chapter","entry_number":"I"}]),
        &[4],
    );
    pattern["analysis"] = json!({"discovered_patterns":[{"pattern_type":"sequential","range_start":"I","range_end":"III","level":1,"level_name":"chapter","heading_format":"CHAPTER {number}"}],"excluded_page_ranges":[]});
    let out = plan_discovery(&pattern).unwrap();
    let jobs = out["discovery_jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(value(&jobs[0], "entry_json").unwrap()["entry_number"], "II");
    let mut findings = jobs.clone();
    for j in &mut findings {
        j["reasoning"] = json!("Could not locate opening");
    }
    assert!(
        join(&json!(findings))
            .unwrap_err()
            .contains("no verified opening")
    );
}
#[test]
fn gaps_cover_leading_middle_and_trailing_ranges() {
    let (_dir, job) = fixture(&[100]);
    let mut v = linked(
        &job,
        json!([{"title":"One","level":1},{"title":"Two","level":1}]),
        &[25, 55],
    );
    v["excluded_json"] = json!("[]");
    v["body_start"] = json!(4);
    v["body_end"] = json!(100);
    let out = plan_gaps(&v).unwrap();
    let gaps: Vec<_> = out["gap_jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| value(g, "gap_context_json").unwrap())
        .collect();
    assert_eq!(gaps.len(), 3);
    assert_eq!(
        (gaps[0]["gap_start"].clone(), gaps[0]["gap_end"].clone()),
        (json!(4), json!(24))
    );
    assert_eq!(
        (gaps[2]["gap_start"].clone(), gaps[2]["gap_end"].clone()),
        (json!(56), json!(100))
    );
    v["excluded_json"] = json!(r#"[{"start_page":56,"end_page":100}]"#);
    assert_eq!(
        plan_gaps(&v).unwrap()["gap_jobs"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn contents_requires_visual_continuation_evidence_and_preserves_host_path() {
    let (dir, mut job) = fixture(&[20]);
    job["path_original"] = json!("/operator/book-a");
    assert_eq!(
        start(&job).unwrap()["toc_find_job"]["path"],
        "/operator/book-a"
    );
    job.as_object_mut().unwrap().remove("path_original");
    let out = found(&job);
    let finder = start(&job).unwrap()["toc_find_job"].clone();
    let name = format!(
        "inspection-{}.json",
        &hash(finder["stage_job_id"].as_str().unwrap().as_bytes())[..32]
    );
    let mut result = finder;
    result["toc_found"] = json!(true);
    result["confidence"] = json!(0.95);
    result["search_strategy_used"] = json!("grep_report");
    result["toc_page_range"] = json!({"start_page":2,"end_page":3});
    result["structure_summary"] = json!({"total_levels":1,"level_patterns":{}});
    result["reasoning"] = json!("Contents found.");
    fs::write(
        dir.path().join(name),
        br#"{"observations":[{"page_num":2,"visual_observations":"Contents"}]}"#,
    )
    .unwrap();
    assert!(
        toc_found(&result)
            .unwrap_err()
            .contains("page 3 lacks visual evidence")
    );
    assert!(
        value(&out, "structure_notes_json").unwrap()["observations"]
            .as_array()
            .unwrap()
            .len()
            == 3
    );
}
