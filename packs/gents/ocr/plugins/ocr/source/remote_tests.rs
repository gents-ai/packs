//! The remote OCR path end to end: stubbed model results over scanned pages
//! and images, the fallbacks, the modes, and cursors across rounds.
use serde_json::{Value, json};

use super::*;
use crate::input::Input;

fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn call(input: &Value) -> Result<Value, String> {
    crate::run(&input.to_string()).map(|out| serde_json::from_str(&out).expect("JSON"))
}

/// A directory holding a scanned PDF of `pages` blank pages: each page is an
/// image with no text, which the built-in OCR reads as nothing.
fn blank_scan(name: &str, pages: usize) -> String {
    use crate::pdfgen::{Img, PageSpec, draw, pdf};
    let img = Img {
        w: 600,
        h: 800,
        gray: true,
        data: vec![255; 600 * 800],
        flate: true,
    };
    let specs: Vec<PageSpec> = (0..pages)
        .map(|_| PageSpec {
            w: 300.0,
            h: 400.0,
            content: draw("Im1", 0.0, 0.0, 300.0, 400.0),
            images: vec![("Im1", &img)],
        })
        .collect();
    let dir = std::env::temp_dir().join(format!("ocr-remote-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("scan.pdf"), pdf(&specs)).unwrap();
    dir.to_str().unwrap().to_string()
}

fn doc(out: &Value) -> &Value {
    &out["documents"][0]
}

/// Follows model call rounds and cursors to the end, answering each request
/// with `answer`; returns the markdown of every call, the warnings and the rounds.
fn drive(base: &Value, answer: impl Fn(&Value) -> Value) -> (String, Vec<String>, usize) {
    let (mut md, mut warnings, mut rounds) = (String::new(), Vec::new(), 0);
    let mut input = base.clone();
    for _ in 0..40 {
        let out = call(&input).unwrap();
        if let Some(mc) = out.get("model_calls") {
            rounds += 1;
            let results: serde_json::Map<String, Value> = mc["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| (r["id"].as_str().unwrap().to_string(), answer(r)))
                .collect();
            input["model_results"] = Value::Object(results);
            input["state"] = mc["state"].clone();
            continue;
        }
        md.push_str(doc(&out)["markdown"].as_str().unwrap());
        md.push('\n');
        warnings.extend(
            doc(&out)["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_string()),
        );
        input = base.clone();
        match out["next"]["cursor"].as_str() {
            Some(c) => input["cursor"] = json!(c),
            None => return (md, warnings, rounds),
        }
    }
    panic!("the call did not finish");
}

fn say(text: &str) -> impl Fn(&Value) -> Value + '_ {
    move |r| json!({"text": format!("<p>{text} {}</p>", r["id"].as_str().unwrap())})
}

fn force(path: &str, file: &str) -> Value {
    json!({"path": path, "files": [file], "model_calls": true, "remote_ocr": "force"})
}

#[test]
fn force_sends_the_page_and_the_answer_becomes_its_markdown() {
    let base = force(&fixtures(), "scan.pdf");
    let round = call(&base).unwrap();
    let req = &round["model_calls"]["requests"][0];
    assert_eq!(req["id"], "f0p1");
    assert_eq!(req["images"][0]["mime"], "image/jpeg");
    assert!(req["images"][0]["data_base64"].as_str().unwrap().len() > 1000);
    assert!(
        req["prompt"]
            .as_str()
            .unwrap()
            .starts_with("OCR this image to HTML")
    );
    assert_eq!(
        round["model_calls"]["requests"].as_array().unwrap().len(),
        1
    );
    let (md, warnings, rounds) = drive(&base, say("remote text"));
    assert_eq!(rounds, 1);
    assert!(md.contains("<!-- page 1 -->\n\nremote text f0p1"), "{md}");
    assert!(!md.to_lowercase().contains("quick brown fox"), "{md}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("read by the remote OCR backend")),
        "{warnings:?}"
    );
}

#[test]
fn refine_checks_even_a_readable_scan_against_its_image_and_draft() {
    let mut base = force(&fixtures(), "scan.pdf");
    base["remote_ocr"] = json!("refine");
    let round = call(&base).unwrap();
    let req = &round["model_calls"]["requests"][0];
    let prompt = req["prompt"].as_str().unwrap();
    assert!(prompt.contains("image is the authoritative source"));
    let draft: Value =
        serde_json::from_str(prompt.split_once("OCR_DRAFT_JSON:\n").unwrap().1).unwrap();
    assert!(
        draft["text"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("quick brown fox")
    );
    assert_eq!(draft["truncated"], false);
    assert_eq!(req["images"][0]["mime"], "image/jpeg");
    let (md, warnings, rounds) = drive(
        &base,
        |_| json!({"text":json!({"html":"<p>Image-confirmed text.</p>"}).to_string()}),
    );
    assert_eq!(rounds, 1);
    assert!(md.contains("<!-- page 1 -->\n\nImage-confirmed text."));
    assert!(!md.to_lowercase().contains("quick brown fox"));
    assert!(warnings.iter().any(|w| w.contains("remote OCR backend")));
}

#[test]
fn transcribe_uses_the_same_contract_without_a_draft() {
    let mut base = force(&fixtures(), "scan.pdf");
    base["remote_ocr"] = json!("transcribe");
    let round = call(&base).unwrap();
    let prompt = round["model_calls"]["requests"][0]["prompt"]
        .as_str()
        .unwrap();
    let draft: Value =
        serde_json::from_str(prompt.split_once("OCR_DRAFT_JSON:\n").unwrap().1).unwrap();
    assert_eq!(draft["text"], "");
    assert_eq!(draft["truncated"], false);
    assert!(prompt.contains("image is the authoritative source"));
    let (md, warnings, _) = drive(
        &base,
        |_| json!({"text":"{\"html\":\"<p>Image-only transcription.</p>\"}"}),
    );
    assert!(md.contains("Image-only transcription."));
    assert!(warnings.iter().any(|w| w.contains("remote OCR backend")));
}

#[test]
fn refine_does_not_publish_model_commentary_or_malformed_transcriptions() {
    let mut base = force(&fixtures(), "scan.pdf");
    base["remote_ocr"] = json!("refine");
    for answer in [
        "I will transcribe faithfully. <p>invented text</p>",
        "{\"html\":\"<p>invented text</p>\",\"commentary\":\"extra\"}",
        "{\"html\":42}",
    ] {
        let (md, warnings, _) = drive(&base, |_| json!({"text":answer}));
        assert!(md.to_lowercase().contains("quick brown fox"));
        assert!(!md.contains("invented text"));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("expected one JSON object containing only html"))
        );
    }
}

#[test]
fn page_numbers_with_degenerate_spacing_do_not_count_as_remote_transcriptions() {
    let answer = format!("<p>44 {}</p>", "&nbsp;".repeat(4096));
    let (md, warnings, _) = drive(&force(&fixtures(), "scan.pdf"), |_| json!({"text":answer}));
    assert!(md.to_lowercase().contains("quick brown fox"));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("almost entirely whitespace"))
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.contains("were read by the remote OCR backend"))
    );
    assert!(!degenerate_whitespace("44"));
    assert!(!degenerate_whitespace("<p>Short title.</p>"));
}

#[test]
fn refine_bounds_drafts_and_counts_their_serialized_bytes_with_images() {
    let line = OcrLine {
        text: "\"\\\n".repeat(DRAFT_BYTES_CAP / 3),
        left: 0.,
        top: 0.,
        right: 1.,
        bottom: 1.,
    };
    let overflow = OcrLine {
        text: "not included".into(),
        ..line.clone()
    };
    let prompt = draft_prompt(&[line.clone(), overflow]);
    let draft: Value =
        serde_json::from_str(prompt.split_once("OCR_DRAFT_JSON:\n").unwrap().1).unwrap();
    assert!(draft["text"].as_str().unwrap().len() <= DRAFT_BYTES_CAP);
    assert_eq!(draft["truncated"], true);
    let input: Input = serde_json::from_value(json!({"path":"/x"})).unwrap();
    let mut remote = Remote::new(&input, RemoteOcr::Refine).unwrap();
    let jpeg = vec![7u8; 600_000];
    while remote.add_page(
        format!("p{}", remote.requests.len()),
        &jpeg,
        usize::MAX,
        &[line.clone()],
    ) == Add::Added
    {}
    let bytes: usize = remote
        .requests
        .iter()
        .map(|r| serde_json::to_vec(r).unwrap().len())
        .sum();
    assert_eq!(remote.bytes, bytes);
    assert!(bytes <= REQUEST_BYTES_CAP);
}

#[test]
fn refine_and_force_cannot_share_continuation_cursors() {
    let dir = blank_scan("refine-cursor", MAX_REQUESTS + 1);
    let mut base = force(&dir, "scan.pdf");
    base["remote_ocr"] = json!("refine");
    let round = call(&base).unwrap();
    base["state"] = round["model_calls"]["state"].clone();
    base["model_results"] = Value::Object(
        round["model_calls"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["id"].as_str().unwrap().into(),
                    json!({"text":"{\"html\":\"<p>read page</p>\"}"}),
                )
            })
            .collect(),
    );
    let out = call(&base).unwrap();
    let cursor = out["next"]["cursor"].as_str().unwrap();
    let input = json!({"path":dir,"files":["scan.pdf"],"model_calls":true,"remote_ocr":"force","cursor":cursor});
    assert!(call(&input).unwrap_err().contains("request"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn html_answers_become_markdown_and_fences_are_removed() {
    let html = "```html\n<h1>Title</h1><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table><img alt=\"a bar chart\"></div>\n```";
    let (md, _, _) = drive(&force(&fixtures(), "scan.pdf"), |_| json!({"text": html}));
    assert!(md.contains("# Title"), "{md}");
    assert!(md.contains("| A | B |") && md.contains("| 1 | 2 |"), "{md}");
    assert!(md.contains("*Figure: a bar chart*"), "{md}");
    let (md, _, _) = drive(
        &force(&fixtures(), "scan.pdf"),
        |_| json!({"text": "# Plain\n\nmarkdown"}),
    );
    assert!(md.contains("# Plain\n\nmarkdown"), "{md}");
}

#[test]
fn a_failed_request_falls_back_to_the_built_in_ocr_with_a_warning() {
    let (md, warnings, _) = drive(
        &force(&fixtures(), "scan.pdf"),
        |_| json!({"error": "the endpoint did not answer"}),
    );
    assert!(md.to_lowercase().contains("quick brown fox"), "{md}");
    assert!(
        warnings.contains(
            &"p.1: remote OCR unavailable (the endpoint did not answer); read with built-in OCR"
                .to_string()
        ),
        "{warnings:?}"
    );
}

#[test]
fn a_missing_answer_falls_back_with_a_warning() {
    let mut input = force(&fixtures(), "scan.pdf");
    input["model_results"] = json!({});
    let out = call(&input).unwrap();
    assert!(
        doc(&out)["markdown"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("quick brown fox")
    );
    assert!(
        doc(&out)["warnings"][0]
            .as_str()
            .unwrap()
            .contains("no answer was received")
    );
}

#[test]
fn auto_keeps_a_page_the_built_in_ocr_reads_well() {
    let plain = call(&json!({"path": fixtures(), "files": ["scan.pdf"]})).unwrap();
    let auto =
        call(&json!({"path": fixtures(), "files": ["scan.pdf"], "model_calls": true})).unwrap();
    assert_eq!(plain, auto);
    assert!(auto.get("model_calls").is_none());
}

#[test]
fn auto_sends_a_page_the_built_in_ocr_finds_nothing_on() {
    let dir = blank_scan("auto", 1);
    let base = json!({"path": dir, "model_calls": true});
    let round = call(&base).unwrap();
    assert_eq!(round["model_calls"]["requests"][0]["id"], "f0p1");
    let (md, _, rounds) = drive(&base, say("blank"));
    assert_eq!(rounds, 1);
    assert!(md.contains("blank f0p1"), "{md}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn off_preserves_output_and_an_unbound_requested_slot_is_reported() {
    let plain = call(&json!({"path": fixtures(), "files": ["scan.pdf", "letter.png"]})).unwrap();
    for extra in [
        json!({"remote_ocr": "off", "model_calls": true}),
        json!({"remote_ocr": "force"}),
        json!({"remote_ocr": "auto"}),
        json!({"remote_ocr": "refine"}),
        json!({"remote_ocr": "transcribe"}),
        // A caller's own state and answers mean nothing to an unbound slot,
        // and a plugin clock shifted by them would stop the read at once.
        json!({"state": {"ms": 900_000}, "model_results": {"f0p1": {"text": "x"}}}),
        json!({"state": {"stop": "bad"}, "model_results": {}, "remote_ocr": "force"}),
        json!({"state": {"ms": 900_000}, "remote_ocr": "off", "model_calls": true}),
    ] {
        let mut input = json!({"path": fixtures(), "files": ["scan.pdf", "letter.png"]});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut actual = call(&input).unwrap();
        if ["auto", "force", "refine", "transcribe"]
            .iter()
            .any(|mode| extra["remote_ocr"] == *mode)
        {
            for doc in actual["documents"].as_array_mut().unwrap() {
                assert!(
                    doc["warnings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|w| w.as_str().unwrap().contains("no model slot"))
                );
                doc["warnings"] = json!([]);
            }
        }
        assert_eq!(actual, plain, "{extra}");
    }
}

#[test]
fn text_layer_pages_and_other_formats_never_go_remote() {
    for file in ["text.pdf", "report.docx", "book.epub", "deck.pptx"] {
        let out = call(&force(&fixtures(), file)).unwrap();
        assert!(out.get("model_calls").is_none(), "{file}");
    }
    // Even OCR on every page leaves a page with a text layer to its text.
    let mut input = force(&fixtures(), "text.pdf");
    input["ocr"] = json!("always");
    assert!(call(&input).unwrap().get("model_calls").is_none());
    // ocr never turns every OCR path off, remote included.
    let mut input = force(&fixtures(), "scan.pdf");
    input["ocr"] = json!("never");
    assert!(call(&input).unwrap().get("model_calls").is_none());
}

#[test]
fn an_image_file_is_one_request() {
    let base = force(&fixtures(), "letter.png");
    assert_eq!(
        call(&base).unwrap()["model_calls"]["requests"][0]["id"],
        "f0p1"
    );
    let (md, warnings, _) = drive(&base, say("image text"));
    assert!(md.contains("image text f0p1"), "{md}");
    assert!(
        warnings.iter().any(|w| w.contains("remote OCR backend")),
        "{warnings:?}"
    );
    let (md, warnings, _) = drive(&base, |_| json!({"error": "down"}));
    assert!(md.to_lowercase().contains("quarterly results"), "{md}");
    assert!(
        warnings[0].starts_with("p.1: remote OCR unavailable (down)"),
        "{warnings:?}"
    );
}

#[test]
fn a_long_scan_is_read_in_batches_with_a_cursor_across_rounds() {
    let dir = blank_scan("batches", 30);
    let base = json!({"path": dir, "model_calls": true, "remote_ocr": "force"});
    let first = call(&base).unwrap();
    let ids: Vec<&str> = first["model_calls"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), MAX_REQUESTS);
    assert_eq!(ids[0], "f0p1");
    assert_eq!(first["model_calls"]["state"]["stop"], json!([0, 13]));
    // Round two reads exactly those pages and returns a cursor for the rest.
    let mut second = base.clone();
    second["model_results"] = Value::Object(
        ids.iter()
            .map(|id| ((*id).to_string(), json!({"text": format!("text {id}")})))
            .collect(),
    );
    second["state"] = first["model_calls"]["state"].clone();
    let out = call(&second).unwrap();
    let md = doc(&out)["markdown"].as_str().unwrap();
    assert!(
        md.contains("text f0p12") && !md.contains("<!-- page 13 -->"),
        "{md}"
    );
    assert!(out["next"]["cursor"].is_string());
    // Following the cursors reads every page once, in order.
    let (all, _, rounds) = drive(&base, say("t"));
    assert_eq!(rounds, 3);
    let order: Vec<usize> = (1..=30)
        .map(|n| {
            all.find(&format!("t f0p{n}</p>"))
                .or_else(|| all.find(&format!("t f0p{n}\n")))
                .unwrap_or_else(|| panic!("page {n} missing: {all}"))
        })
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_bad_state_is_refused() {
    let mut input = force(&fixtures(), "scan.pdf");
    input["model_results"] = json!({});
    input["state"] = json!({"stop": "here"});
    assert!(call(&input).unwrap_err().contains("state"));
}

#[test]
fn the_round_limits_bound_requests_and_bytes() {
    const ROOM: usize = usize::MAX;
    let mut r = Remote::new(
        &serde_json::from_value::<Input>(json!({"path": "/x"})).unwrap(),
        RemoteOcr::Force,
    )
    .unwrap();
    assert!(r.collecting() && !r.stops_before(1, ROOM));
    let page = vec![7u8; 600_000];
    let mut added = 0;
    while r.add(format!("f0p{added}"), &page, ROOM) == Add::Added {
        added += 1;
    }
    assert!((3..MAX_REQUESTS).contains(&added), "{added}");
    assert!(r.stops_before(99, ROOM) && r.stops_before_file(3, ROOM));
    let mut empty = Remote::off();
    empty.mode = RemoteOcr::Force;
    assert_eq!(
        empty.add("big".into(), &vec![0u8; 3_000_000], ROOM),
        Add::TooLarge
    );
    let mark = r.mark();
    r.note_read(4);
    r.rewind(&r.mark());
    let before = r.requests.len();
    r.rewind(&Mark {
        requests: 1,
        ..mark
    });
    assert_eq!((before, r.requests.len()), (added, 1));
}

#[test]
fn only_the_documented_state_round_trips() {
    let state = RoundState {
        stop: Some((2, 7)),
        ms: 1500,
    };
    let v = serde_json::to_value(&state).unwrap();
    assert_eq!(serde_json::from_value::<RoundState>(v).unwrap(), state);
    let input: Input = serde_json::from_value(
        json!({"path": "/x", "state": {"stop": [2, 7], "ms": 1500}, "model_results": {}}),
    )
    .unwrap();
    let mut r = Remote::new(&input, RemoteOcr::Auto).unwrap();
    r.set_file(2);
    assert_eq!(r.spent(860).as_millis(), 1500);
    assert!(!r.collecting() && r.stops_before(7, usize::MAX) && !r.stops_before(1, usize::MAX));
}

#[test]
fn a_graph_node_passes_the_model_call_round_through_and_reads_the_answers() {
    let chunk = json!({"run_id": "r", "chunk": 0, "path": fixtures(), "files": ["scan.pdf"],
        "source": "scan.pdf", "format": "pdf", "model_calls": true, "remote_ocr": "force"});
    let round = call(&chunk).unwrap();
    assert_eq!(round["model_calls"]["requests"][0]["id"], "f0p1");
    let mut answered = chunk.clone();
    answered["model_results"] = json!({"f0p1": {"text": "<h2>Remote page</h2>"}});
    answered["state"] = round["model_calls"]["state"].clone();
    let out = call(&answered).unwrap();
    assert_eq!(out["document"]["complete"], true);
    assert!(
        out["pages"][0]["markdown"]
            .as_str()
            .unwrap()
            .contains("## Remote page")
    );
}

#[test]
fn a_round_never_holds_more_than_the_request_limit() {
    let mut r = Remote::new(
        &serde_json::from_value::<Input>(json!({"path": "/x"})).unwrap(),
        RemoteOcr::Force,
    )
    .unwrap();
    let tiny = [1u8; 100];
    for n in 0..MAX_REQUESTS {
        assert_eq!(r.add(format!("f0p{n}"), &tiny, usize::MAX), Add::Added);
    }
    assert_eq!(r.add("f0p99".into(), &tiny, usize::MAX), Add::Full);
    assert_eq!(r.requests.len(), MAX_REQUESTS);
}

#[test]
fn a_small_output_budget_asks_for_few_pages_and_each_page_once() {
    let dir = blank_scan("budget", 8);
    // Answers a little over the planned size, so a round holding three pages overflows.
    let body = "word ".repeat(2400);
    for max_bytes in [4096usize, 20_000] {
        let base = json!({"path": dir, "model_calls": true, "remote_ocr": "force", "max_bytes": max_bytes});
        let asked = std::cell::RefCell::new(Vec::new());
        let (md, _, _) = drive(&base, |r| {
            let id = r["id"].as_str().unwrap().to_string();
            let text = format!("<p>{id} {body}</p>");
            asked.borrow_mut().push(id);
            json!({"text": text})
        });
        let mut asked = asked.into_inner();
        asked.sort();
        let before = asked.len();
        asked.dedup();
        assert_eq!(
            (max_bytes, asked.len()),
            (max_bytes, before),
            "a page was requested twice"
        );
        assert_eq!(asked.len(), 8);
        for n in 1..=8 {
            assert_eq!(
                md.matches(&format!("f0p{n} word")).count(),
                1,
                "{max_bytes}: page {n} in {md}"
            );
        }
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_page_larger_than_the_budget_is_cut_from_the_one_answer() {
    let dir = blank_scan("cut", 2);
    let paras: String = (1..=300)
        .map(|n| format!("<p>para {n} of the page</p>"))
        .collect();
    let base = json!({"path": dir, "model_calls": true, "remote_ocr": "force", "max_bytes": 4096});
    let asked = std::cell::RefCell::new(0usize);
    let (md, _, _) = drive(&base, |_| {
        *asked.borrow_mut() += 1;
        json!({"text": paras})
    });
    assert_eq!(asked.into_inner(), 2);
    for n in 1..=300 {
        assert_eq!(
            md.matches(&format!("para {n} of the page")).count(),
            2,
            "paragraph {n} (once per page)"
        );
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn undelivered_answers_travel_in_the_cursor_and_are_read_from_it() {
    let input: Input = serde_json::from_value(json!({
        "path": "/x",
        "model_results": {"f0p2": {"text": "two"}, "f0p3": {"text": "three"}, "f0p4": {"error": "down"}}
    }))
    .unwrap();
    let mut r = Remote::new(&input, RemoteOcr::Force).unwrap();
    let mark = r.mark();
    assert!(matches!(r.take_answer("f0p2"), Some(Answer::Text(t)) if t == "two"));
    assert!(matches!(r.take_answer("f0p4"), Some(Answer::Error(_))));
    // Taken back with its unit: both unread answers are carried, the error is not.
    r.rewind(&mark);
    let carried = r.undelivered(None);
    assert_eq!(
        carried,
        vec![
            ("f0p2".into(), "two".into()),
            ("f0p3".into(), "three".into())
        ]
    );
    // A page cut in the middle keeps its text, once, first.
    assert!(r.take_answer("f0p3").is_some());
    assert_eq!(
        r.undelivered(Some("f0p3")),
        vec![
            ("f0p3".into(), "three".into()),
            ("f0p2".into(), "two".into())
        ]
    );
    // The next call reads them before asking again.
    let next = Remote::new(
        &serde_json::from_value::<Input>(json!({"path": "/x"})).unwrap(),
        RemoteOcr::Force,
    )
    .unwrap()
    .with_carried(carried);
    assert!(next.collecting());
    let mut next = next;
    assert!(matches!(next.take_answer("f0p3"), Some(Answer::Text(t)) if t == "three"));
}

#[test]
fn a_failed_answer_leaves_an_eighth_of_the_clock_for_the_fallback() {
    let with = |results: Value, state: Value| {
        let input: Input =
            serde_json::from_value(json!({"path": "/x", "model_results": results, "state": state}))
                .unwrap();
        Remote::new(&input, RemoteOcr::Auto).unwrap()
    };
    let ok = with(json!({"f0p1": {"text": "a"}}), json!({"ms": 2000}));
    assert_eq!(ok.spent(800).as_millis(), 2000);
    let failed = with(
        json!({"f0p1": {"text": "a"}, "f0p2": {"error": "wall clock"}}),
        json!({"ms": 2000}),
    );
    assert_eq!(failed.spent(800).as_millis(), 700_000);
    // Round one's own time still counts when it is more.
    let late = with(json!({"f0p2": {"error": "x"}}), json!({"ms": 790_000}));
    assert_eq!(late.spent(800).as_millis(), 790_000);
    assert_eq!(Remote::off().spent(800).as_millis(), 0);
}

#[test]
fn graph_continues_vision_batches_before_publishing_one_complete_chunk() {
    let dir = blank_scan("graph_batches", 20);
    let mut input = json!({"run_id":"r","book_id":"book","chunk":0,"chunk_id":"r:0",
        "expected_total":1,"sources_json":"[{\"source\":\"scan.pdf\",\"page_count\":20}]",
        "path":dir,"files":["scan.pdf"],"source":"scan.pdf","format":"pdf","pages":"1-20",
        "remote_ocr":"force","model_calls":true});
    let mut continuations = 0;
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..12 {
        let out = call(&input).unwrap();
        if let Some(mc) = out.get("model_calls") {
            let results = mc["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let id = r["id"].as_str().unwrap();
                    assert!(seen.insert(id.to_string()), "paid for the same page twice");
                    (
                        id.to_string(),
                        json!({"text":format!("Transcription for {id}")}),
                    )
                })
                .collect();
            input["model_results"] = Value::Object(results);
            input["state"] = mc["state"].clone();
        } else if out["continuation"].is_object() {
            assert!(out["document"].is_null());
            assert!(out["pages"].as_array().unwrap().is_empty());
            input = out["continuation"].clone();
            input["run_id"] = json!("r");
            input["model_calls"] = json!(true);
            continuations += 1;
        } else {
            assert!(continuations > 0);
            assert_eq!(out["document"]["complete"], true, "{out}");
            assert_eq!(out["document"]["book_id"], "book");
            assert_eq!(seen.len(), 20);
            let pages = out["pages"].as_array().unwrap();
            assert_eq!(pages.len(), 20);
            for (i, p) in pages.iter().enumerate() {
                assert_eq!(p["page"], i + 1);
                assert!(
                    p["markdown"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("Transcription for f0p{}", i + 1))
                );
            }
            return;
        }
    }
    panic!("graph never completed");
}

#[test]
fn empty_or_refused_vision_answers_use_bundled_text() {
    for answer in ["", "<p> </p>", "I cannot transcribe this image."] {
        let (md, warnings, _) = drive(&force(&fixtures(), "scan.pdf"), |_| json!({"text":answer}));
        assert!(md.to_lowercase().contains("quick brown fox"), "{md}");
        assert!(
            warnings.iter().any(|w| w.contains("empty or refused")),
            "{warnings:?}"
        );
    }
}
