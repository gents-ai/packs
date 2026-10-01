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
fn off_and_a_host_without_model_calls_leave_the_output_unchanged() {
    let plain = call(&json!({"path": fixtures(), "files": ["scan.pdf", "letter.png"]})).unwrap();
    for extra in [
        json!({"remote_ocr": "off", "model_calls": true}),
        json!({"remote_ocr": "force"}),
        json!({"remote_ocr": "auto"}),
    ] {
        let mut input = json!({"path": fixtures(), "files": ["scan.pdf", "letter.png"]});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(call(&input).unwrap(), plain, "{extra}");
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
    let mut r = Remote::new(
        &serde_json::from_value::<Input>(json!({"path": "/x"})).unwrap(),
        RemoteOcr::Force,
    )
    .unwrap();
    assert!(r.collecting() && !r.stops_before(1));
    let page = vec![7u8; 600_000];
    let mut added = 0;
    while r.add(format!("f0p{added}"), &page) == Add::Added {
        added += 1;
    }
    assert!((3..MAX_REQUESTS).contains(&added), "{added}");
    assert!(r.stops_before(99) && r.stops_before_file(3));
    assert_eq!(r.add("big".into(), &vec![0u8; 3_000_000]), Add::TooLarge);
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
    assert_eq!(r.carried().as_millis(), 1500);
    assert!(!r.collecting() && r.stops_before(7) && !r.stops_before(1));
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
