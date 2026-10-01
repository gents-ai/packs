//! End-to-end tests of `run` over the committed fixtures and built inputs:
//! every format, directory runs, loud failures, the output cap and limits.
use std::io::Write;

use serde_json::{Value, json};

use super::{run, run_at};

fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn call(input: Value) -> Result<Value, String> {
    run(&input.to_string()).map(|out| serde_json::from_str(&out).expect("the plugin prints JSON"))
}

fn docs(v: &Value) -> &Vec<Value> {
    v.get("response").unwrap_or(v)["documents"]
        .as_array()
        .expect("documents array")
}

fn read(file: &str) -> Value {
    call(json!({"path": fixtures(), "files": [file], "ocr": "never"}))
        .unwrap_or_else(|e| panic!("{file}: {e}"))
}

#[test]
fn every_format_reads_its_fixture() {
    let cases = [
        ("text.pdf", "pdf", "# Annual Report"),
        ("book.epub", "epub", "# The Long Road"),
        ("report.docx", "docx", "# Quarterly Plan"),
        ("deck.pptx", "pptx", "## Product Launch"),
        ("sheet.xlsx", "xlsx", "| North | 42.5 | 2024-01-01 |"),
        ("notes.odt", "odt", "# Meeting Notes"),
        ("budget.ods", "ods", "| Laptops | 1200 |"),
        ("slides.odp", "odp", "Second slide"),
        ("page.html", "html", "# Status Page"),
        ("notes.md", "markdown", "- two"),
        ("data.csv", "csv", "| Ana | 9 |"),
        ("plain.txt", "text", "second line"),
    ];
    for (file, format, needle) in cases {
        let out = read(file);
        let doc = &docs(&out)[0];
        assert_eq!(doc["format"], format, "{file}");
        assert_eq!(doc["source"], file);
        let md = doc["markdown"].as_str().unwrap();
        assert!(
            md.starts_with(&format!("<!-- document: {file} ({format}) -->")),
            "{file}: {md}"
        );
        assert!(md.contains(needle), "{file}: {md}");
    }
}

#[test]
fn pages_select_units_and_report_the_total() {
    let out =
        call(json!({"path": fixtures(), "files": ["deck.pptx"], "pages": "2", "ocr": "never"}))
            .unwrap();
    let doc = &docs(&out)[0];
    assert_eq!(doc["pages"], 2);
    let md = doc["markdown"].as_str().unwrap();
    assert!(
        md.contains("<!-- slide 2 -->") && !md.contains("<!-- slide 1 -->"),
        "{md}"
    );
    assert_eq!(doc["figures"][0]["page"], 2);
}

#[test]
fn a_directory_run_reports_each_failure_and_keeps_going() {
    let out = call(json!({"path": format!("{}/tree", fixtures()), "ocr": "never"})).unwrap();
    let names: Vec<&str> = docs(&out)
        .iter()
        .map(|d| d["source"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "broken.pdf",
            "data.csv",
            "img/chart.png",
            "page.html",
            "skip.xyz"
        ]
    );
    let by = |n: &str| {
        docs(&out)
            .iter()
            .find(|d| d["source"] == n)
            .unwrap()
            .clone()
    };
    assert!(
        by("broken.pdf")["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("failed: the PDF is corrupt")
    );
    assert!(
        by("skip.xyz")["warnings"][0]
            .as_str()
            .unwrap()
            .contains("unsupported format .xyz")
    );
    assert_eq!(by("data.csv")["format"], "csv");
    assert!(
        by("page.html")["markdown"]
            .as_str()
            .unwrap()
            .contains("[Figure fig-1]")
    );
}

#[test]
fn a_single_bad_file_fails_with_one_sentence() {
    for (file, needle) in [
        ("tree/broken.pdf", "corrupt or truncated"),
        ("tree/skip.xyz", "unsupported format .xyz"),
    ] {
        let err = call(json!({"path": fixtures(), "files": [file]})).unwrap_err();
        assert!(err.contains(needle) && !err.contains('\n'), "{err}");
    }
    assert!(
        call(json!({"path": "/no/such/dir"}))
            .unwrap_err()
            .contains("cannot read")
    );
    assert!(
        call(json!({"name": "a.txt", "data_base64": "%%%"}))
            .unwrap_err()
            .contains("base64")
    );
    assert!(
        call(json!({"name": "a.pdf", "data_base64": "aGVsbG8="}))
            .unwrap_err()
            .contains("not a valid PDF")
    );
    assert!(
        call(json!({"path": fixtures(), "files": ["text.pdf"], "max_image_px": 5}))
            .unwrap_err()
            .contains("max_image_px")
    );
    assert!(call(json!({})).unwrap_err().contains("path"));
}

#[test]
fn all_documents_failing_is_an_error() {
    let err = call(json!({"path": fixtures(), "files": ["tree/broken.pdf", "tree/skip.xyz"]}))
        .unwrap_err();
    assert!(err.contains("no document could be read"), "{err}");
}

#[test]
fn inline_data_is_read_without_a_path() {
    let out = call(json!({"name": "dir/data.csv", "data_base64": "YSxiCjEsMgo="})).unwrap();
    assert_eq!(docs(&out)[0]["source"], "data.csv");
    assert!(
        docs(&out)[0]["markdown"]
            .as_str()
            .unwrap()
            .contains("| 1 | 2 |")
    );
}

#[test]
fn figure_images_use_the_response_and_parts_shape() {
    let plain =
        call(json!({"path": fixtures(), "files": ["report.docx"], "ocr": "never"})).unwrap();
    assert!(
        plain.get("parts").is_none() && plain["documents"][0]["figures"][0].get("part").is_none()
    );
    let with = call(json!({"path": fixtures(), "files": ["report.docx"], "ocr": "never", "figure_images": true})).unwrap();
    let part = &with["parts"][0];
    assert_eq!(
        (part["type"].as_str(), part["mimeType"].as_str()),
        (Some("image"), Some("image/png"))
    );
    assert!(part["data"].as_str().unwrap().len() > 1000);
    assert_eq!(with["response"]["documents"][0]["figures"][0]["part"], 0);
    assert!(
        with["response"]["documents"][0]["markdown"]
            .as_str()
            .unwrap()
            .contains("(image attached)")
    );
    // No image attached, so there is nothing to convert and the plain shape is kept.
    let none = call(json!({"path": fixtures(), "files": ["text.pdf"], "ocr": "never", "figure_images": true, "min_figure_px": 4000})).unwrap();
    assert!(none.get("parts").is_none());
}

#[test]
fn a_large_table_is_cut_at_the_output_limit_and_continues_with_a_cursor() {
    let mut csv = String::from("id,text\n");
    for i in 0..150_000 {
        csv.push_str(&format!("{i},row number {i} with some padding text\n"));
    }
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(csv)
    };
    let input = json!({"name": "big.csv", "data_base64": b64});
    let raw = run(&input.to_string()).unwrap();
    assert!(raw.len() < 4 * 1024 * 1024, "{} bytes", raw.len());
    let out: Value = serde_json::from_str(&raw).unwrap();
    assert!(docs(&out)[0]["warnings"].as_array().unwrap().is_empty());
    let cursor = out["next"]["cursor"].as_str().unwrap().to_string();
    let mut again = input.clone();
    again["cursor"] = json!(cursor);
    let second: Value = serde_json::from_str(&run(&again.to_string()).unwrap()).unwrap();
    assert_eq!(
        docs(&second)[0]["table_header"],
        "| id | text |\n| --- | --- |"
    );
    assert_eq!(docs(&second)[0]["joint"], "line");
}

fn zip_with(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(data).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn inline(name: &str, data: &[u8]) -> Result<Value, String> {
    use base64::Engine as _;
    call(
        json!({"name": name, "data_base64": base64::engine::general_purpose::STANDARD.encode(data)}),
    )
}

#[test]
fn a_zip_bomb_is_refused_before_it_is_expanded() {
    let bomb = zip_with(&[("word/document.xml", vec![b' '; 97 * 1024 * 1024])]);
    assert!(bomb.len() < 1024 * 1024);
    let err = inline("bomb.docx", &bomb).unwrap_err();
    assert!(err.contains("expands"), "{err}");
}

#[test]
fn a_drm_protected_epub_fails_loudly() {
    let container = br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf"/></rootfiles></container>"#.to_vec();
    let enc = br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/></enc:EncryptedData></encryption>"#.to_vec();
    let epub = zip_with(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container),
        ("META-INF/encryption.xml", enc),
    ]);
    assert!(
        inline("locked.epub", &epub)
            .unwrap_err()
            .contains("DRM-protected")
    );
}

#[test]
fn legacy_office_and_rtf_are_refused_with_the_way_out() {
    assert!(
        inline("old.doc", &[0xD0, 0xCF, 0x11, 0xE0, 1, 2, 3])
            .unwrap_err()
            .contains("save the file as DOCX")
    );
    assert!(
        inline("a.rtf", b"{\\rtf1 hi}")
            .unwrap_err()
            .contains("RTF files are not supported")
    );
}

#[test]
fn html_images_never_leave_the_bound_directory() {
    let dir = std::env::temp_dir().join(format!("ocr-plugin-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::copy(format!("{}/chart.png", fixtures()), dir.join("outside.png")).unwrap();
    std::fs::write(
        dir.join("inner/page.html"),
        r#"<img src="../outside.png" alt="Escape">"#,
    )
    .unwrap();
    let out = call(json!({"path": dir.join("inner").to_str().unwrap(), "ocr": "never"})).unwrap();
    let md = docs(&out)[0]["markdown"].as_str().unwrap().to_string();
    std::fs::remove_dir_all(&dir).ok();
    assert!(md.contains("the image file was not found"), "{md}");
}

#[test]
fn ocr_reads_a_scanned_page_and_a_figure_natively() {
    let scan = call(json!({"path": fixtures(), "files": ["scan.pdf", "letter.png"]})).unwrap();
    for d in docs(&scan) {
        let md = d["markdown"].as_str().unwrap().to_lowercase();
        assert!(
            md.contains("quick brown fox") && md.contains("quarterly results"),
            "{md}"
        );
    }
    let docx = call(json!({"path": fixtures(), "files": ["report.docx"]})).unwrap();
    let fig = &docs(&docx)[0]["figures"][0];
    assert_eq!(fig["ocr"], true);
    assert!(
        fig["text"].as_str().unwrap().contains("Revenue by region"),
        "{fig}"
    );
}

#[test]
fn docx_lists_follow_their_style_and_a_caption_paragraph_names_the_figure() {
    let ns = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"";
    let para = |style: &str, t: &str| {
        format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>")
    };
    let body = format!(
        "<w:document {ns}><w:body>{}{}{}<w:p><w:r><w:drawing><wp:inline><wp:docPr id=\"1\" name=\"Picture 1\" descr=\"chart.png\"/><a:graphic><a:graphicData><a:blip r:embed=\"rId5\"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>{}</w:body></w:document>",
        para("Bullets", "alpha"),
        para("Steps", "first"),
        para("Steps", "second"),
        para("Normal", "Figure 1. Revenue chart"),
    );
    let styles = format!(
        "<w:styles {ns}><w:style w:styleId=\"Bullets\"><w:name w:val=\"List Bullet\"/><w:pPr><w:numPr><w:numId w:val=\"1\"/></w:numPr></w:pPr></w:style><w:style w:styleId=\"Steps\"><w:name w:val=\"List Number\"/><w:pPr><w:numPr><w:numId w:val=\"2\"/></w:numPr></w:pPr></w:style></w:styles>"
    );
    let numbering = format!(
        "<w:numbering {ns}><w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"decimal\"/></w:lvl></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num></w:numbering>"
    );
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId5\" Type=\"image\" Target=\"media/chart.png\"/></Relationships>";
    let chart = std::fs::read(format!("{}/chart.png", fixtures())).unwrap();
    let docx = zip_with(&[
        ("word/document.xml", body.into_bytes()),
        ("word/styles.xml", styles.into_bytes()),
        ("word/numbering.xml", numbering.into_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
        ("word/media/chart.png", chart),
    ]);
    use base64::Engine as _;
    let out = call(json!({"name": "s.docx", "ocr": "never", "data_base64": base64::engine::general_purpose::STANDARD.encode(docx)})).unwrap();
    let md = docs(&out)[0]["markdown"].as_str().unwrap();
    assert!(md.contains("- alpha\n\n1. first\n2. second"), "{md}");
    assert!(
        md.contains("**[Figure fig-1]** Revenue chart")
            || md.contains("**[Figure fig-1]** Figure 1. Revenue chart"),
        "{md}"
    );
    assert!(!md.contains("\n\nFigure 1. Revenue chart"), "{md}");
    assert_eq!(
        docs(&out)[0]["figures"][0]["caption"],
        "Figure 1. Revenue chart"
    );
}

/// Deterministic xorshift, so a failure names the same mutation every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn corrupted_inputs_never_panic() {
    use base64::Engine as _;
    let files = [
        "text.pdf",
        "scan.pdf",
        "book.epub",
        "report.docx",
        "deck.pptx",
        "sheet.xlsx",
        "notes.odt",
        "budget.ods",
        "slides.odp",
        "chart.png",
        "page.html",
        "data.csv",
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for file in files {
        let original = std::fs::read(format!("{}/{file}", fixtures())).unwrap();
        for round in 0..40 {
            let mut bytes = original.clone();
            if round % 4 == 0 {
                bytes.truncate((rng.next() as usize) % bytes.len().max(1));
            } else {
                for _ in 0..1 + rng.next() % 8 {
                    let at = (rng.next() as usize) % bytes.len();
                    bytes[at] = rng.next() as u8;
                }
            }
            let input = json!({"name": file, "ocr": "never", "data_base64": base64::engine::general_purpose::STANDARD.encode(&bytes)}).to_string();
            let outcome = std::panic::catch_unwind(|| run(&input));
            assert!(outcome.is_ok(), "{file} round {round} panicked");
        }
    }
}

/// A call whose OCR time budget has already run out.
fn exhausted(input: Value) -> Result<Value, String> {
    let long_ago = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(10_000))
        .expect("clock is far enough from its origin");
    run_at(&input.to_string(), long_ago)
        .map(|out| serde_json::from_str(&out).expect("the plugin prints JSON"))
}

#[test]
fn images_stop_at_the_ocr_time_budget_instead_of_running_out_the_clock() {
    // One image: a loud single sentence that says how to continue.
    let err = exhausted(json!({"path": fixtures(), "files": ["letter.png"]})).unwrap_err();
    assert!(
        err.contains("OCR time budget of this call ran out") && err.contains("call again"),
        "{err}"
    );
    // A directory: the image is reported unread and the call stops with a cursor at the next file.
    let out =
        exhausted(json!({"path": fixtures(), "files": ["letter.png", "plain.txt", "chart.png"]}))
            .unwrap();
    assert_eq!(docs(&out).len(), 1);
    let w = docs(&out)[0]["warnings"][0].as_str().unwrap().to_string();
    assert!(w.starts_with("not read: the OCR time budget"), "{w}");
    assert_eq!(docs(&out)[0]["markdown"], "");
    assert_eq!(out["next"]["source"], "plain.txt");
    assert!(out["next"]["cursor"].as_str().unwrap().starts_with("ocr1."));
}

#[test]
fn a_figure_repeated_across_chapters_is_read_once() {
    let container = br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf"/></rootfiles></container>"#.to_vec();
    let opf = br#"<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/><item id="b" href="b.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#.to_vec();
    let page = |t: &str| {
        format!("<html><body><p>{t}</p><img src=\"logo.png\" alt=\"Logo\"/></body></html>")
            .into_bytes()
    };
    let epub = zip_with(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container),
        ("c.opf", opf),
        ("a.xhtml", page("one")),
        ("b.xhtml", page("two")),
        (
            "logo.png",
            std::fs::read(format!("{}/chart.png", fixtures())).unwrap(),
        ),
    ]);
    use base64::Engine as _;
    let out = call(json!({"name": "b.epub", "ocr": "never", "data_base64": base64::engine::general_purpose::STANDARD.encode(epub)})).unwrap();
    let doc = &docs(&out)[0];
    assert_eq!(doc["figures"].as_array().unwrap().len(), 1, "{doc}");
    assert!(
        doc["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("1 image(s) already listed earlier"),
        "{doc}"
    );
}

#[test]
fn a_hostile_pdf_in_a_directory_is_reported_and_the_rest_is_read() {
    let out = call(json!({"path": format!("{}/hostile", fixtures())})).unwrap();
    assert_eq!(docs(&out).len(), 2);
    let by = |n: &str| {
        docs(&out)
            .iter()
            .find(|d| d["source"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(
        by("hostile.pdf")["warnings"][0],
        "page 1: an image of 40000x40000 pixels is over the 50000000 pixel limit and was skipped"
    );
    assert!(
        by("hostile.pdf")["markdown"]
            .as_str()
            .unwrap()
            .contains("readable text")
    );
    assert!(
        by("note.txt")["markdown"]
            .as_str()
            .unwrap()
            .contains("readable note")
    );
}

#[test]
fn files_outside_the_bound_directory_are_refused_with_the_entry_named() {
    for bad in ["../secret.pdf", "/etc/passwd", "a/../../b.pdf"] {
        let err = call(json!({"path": fixtures(), "files": [bad]})).unwrap_err();
        assert_eq!(
            err,
            format!("files entry {bad:?} must be a relative path inside the bound directory")
        );
    }
}
