//! A read that stops at the output limit and is continued with `next.cursor`
//! gives, joined, exactly what one unlimited read gives: for every format,
//! on the committed fixtures and on built inputs far over the limit.
use std::io::Write;

use base64::Engine as _;
use serde_json::{Value, json};

use super::run;

fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

struct Joined {
    markdown: String,
    figures: Vec<Value>,
    pages: Value,
    calls: usize,
}

fn call(input: &Value) -> Value {
    let out: Value = serde_json::from_str(&run(&input.to_string()).expect("the call succeeds"))
        .expect("the plugin prints JSON");
    out.get("response").cloned().unwrap_or(out)
}

/// Reads one file call after call by following `next.cursor`, joining each
/// continuation to what came before as its `joint` says.
fn sliced(mut input: Value) -> Joined {
    let mut joined = Joined {
        markdown: String::new(),
        figures: Vec::new(),
        pages: Value::Null,
        calls: 0,
    };
    loop {
        let out = call(&input);
        let docs = out["documents"].as_array().expect("documents");
        assert_eq!(docs.len(), 1);
        let doc = &docs[0];
        assert!(doc["format"] != "unknown", "{doc}");
        match doc["joint"].as_str() {
            Some("blank") => joined.markdown.push_str("\n\n"),
            Some("line") => joined.markdown.push('\n'),
            _ => {}
        }
        joined
            .markdown
            .push_str(doc["markdown"].as_str().expect("markdown"));
        joined
            .figures
            .extend(doc["figures"].as_array().cloned().unwrap_or_default());
        joined.pages = doc["pages"].clone();
        joined.calls += 1;
        assert!(joined.calls < 5000, "the cursor does not advance");
        match out["next"]["cursor"].as_str() {
            Some(cursor) => input["cursor"] = json!(cursor),
            None => return joined,
        }
    }
}

fn whole(input: &Value) -> Joined {
    let out = call(input);
    assert!(out["next"].is_null(), "the one-shot read must finish");
    let doc = &out["documents"][0];
    Joined {
        markdown: doc["markdown"].as_str().expect("markdown").to_string(),
        figures: doc["figures"].as_array().cloned().unwrap_or_default(),
        pages: doc["pages"].clone(),
        calls: 1,
    }
}

fn same_when_sliced(label: &str, input: Value, min_calls: usize) {
    let one = whole(&input);
    let mut small = input.clone();
    small["max_bytes"] = json!(4096);
    let many = sliced(small);
    assert!(many.calls >= min_calls, "{label}: {} call(s)", many.calls);
    assert_eq!(many.markdown, one.markdown, "{label}");
    assert_eq!(many.figures, one.figures, "{label}");
    assert_eq!(many.pages, one.pages, "{label}");
}

#[test]
fn every_fixture_reads_the_same_in_slices() {
    for (file, min_calls) in [
        ("long.txt", 8),
        ("many.pdf", 4),
        ("text.pdf", 1),
        ("layout.pdf", 1),
        ("report.docx", 1),
        ("deck.pptx", 1),
        ("sheet.xlsx", 1),
        ("book.epub", 1),
        ("notes.odt", 1),
        ("budget.ods", 1),
        ("slides.odp", 1),
        ("page.html", 1),
        ("notes.md", 1),
        ("data.csv", 1),
        ("plain.txt", 1),
    ] {
        let input =
            json!({"path": fixtures(), "files": [file], "ocr": "never", "figure_images": false});
        same_when_sliced(file, input, min_calls);
    }
}

fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(data).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn inline(name: &str, data: &[u8]) -> Value {
    json!({
        "name": name,
        "data_base64": base64::engine::general_purpose::STANDARD.encode(data),
        "ocr": "never",
    })
}

const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

#[test]
fn a_long_docx_with_lists_and_tables_reads_the_same_in_slices() {
    let mut body = String::new();
    for i in 0..12_000 {
        body.push_str(&match i % 10 {
            0 => format!("<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Section {i}</w:t></w:r></w:p>"),
            1 | 2 => format!("<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>item {i}</w:t></w:r></w:p>"),
            3 => format!("<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a{i}</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b{i}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"),
            _ => format!("<w:p><w:r><w:t>paragraph number {i} with a few words in it</w:t></w:r></w:p>"),
        });
    }
    let doc = format!("<w:document {W}><w:body>{body}</w:body></w:document>");
    let styles = format!(
        "<w:styles {W}><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/></w:style></w:styles>"
    );
    let numbering = format!(
        "<w:numbering {W}><w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num></w:numbering>"
    );
    let data = zip_of(&[
        ("word/document.xml", doc.into_bytes()),
        ("word/styles.xml", styles.into_bytes()),
        ("word/numbering.xml", numbering.into_bytes()),
    ]);
    same_when_sliced("long.docx", inline("long.docx", &data), 100);
}

#[test]
fn a_long_xlsx_reads_the_same_in_slices_and_names_its_header() {
    let ns = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
    let mut sheet = format!("<worksheet {ns}><dimension ref=\"A1:C3000\"/><sheetData>");
    for r in 1..=3000 {
        sheet.push_str(&format!("<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c><c r=\"B{r}\" t=\"inlineStr\"><is><t>name {r}</t></is></c><c r=\"C{r}\"><v>{}</v></c></row>", r * 7));
    }
    sheet.push_str("</sheetData></worksheet>");
    let data = zip_of(&[
        ("xl/workbook.xml", format!("<workbook {ns}><sheets><sheet name=\"A\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"B\" sheetId=\"2\" r:id=\"rId2\"/></sheets></workbook>").into_bytes()),
        ("xl/_rels/workbook.xml.rels", b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"x/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"x/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>".to_vec()),
        ("xl/worksheets/sheet1.xml", sheet.into_bytes()),
        ("xl/worksheets/sheet2.xml", format!("<worksheet {ns}><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>tail</t></is></c></row></sheetData></worksheet>").into_bytes()),
    ]);
    let input = inline("long.xlsx", &data);
    same_when_sliced("long.xlsx", input.clone(), 20);
    let mut small = input;
    small["max_bytes"] = json!(4096);
    let first = call(&small);
    let mut again = small.clone();
    again["cursor"] = first["next"]["cursor"].clone();
    let second = call(&again);
    assert_eq!(
        second["documents"][0]["table_header"],
        "| 1 | name 1 | 7 |\n| --- | --- | --- |"
    );
}

#[test]
fn a_long_epub_and_pptx_read_the_same_in_slices() {
    let container = br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf"/></rootfiles></container>"#.to_vec();
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("mimetype".into(), b"application/epub+zip".to_vec()),
        ("META-INF/container.xml".into(), container),
    ];
    for c in 0..40 {
        manifest.push_str(&format!(
            "<item id=\"c{c}\" href=\"c{c}.xhtml\" media-type=\"application/xhtml+xml\"/>"
        ));
        spine.push_str(&format!("<itemref idref=\"c{c}\"/>"));
        let paras: String = (0..30)
            .map(|p| format!("<p>chapter {c} paragraph {p} has some words</p>"))
            .collect();
        entries.push((
            format!("c{c}.xhtml"),
            format!("<html><body><h1>Chapter {c}</h1>{paras}</body></html>").into_bytes(),
        ));
    }
    entries.push(("c.opf".into(), format!("<package xmlns=\"http://www.idpf.org/2007/opf\"><manifest>{manifest}</manifest><spine>{spine}</spine></package>").into_bytes()));
    let refs: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    same_when_sliced("long.epub", inline("long.epub", &zip_of(&refs)), 8);

    let ns = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
    let mut slides = String::new();
    let mut rels = String::new();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for n in 1..=60 {
        slides.push_str(&format!("<p:sldId id=\"{}\" r:id=\"s{n}\"/>", 255 + n));
        rels.push_str(&format!(
            "<Relationship Id=\"s{n}\" Type=\"x/slide\" Target=\"slides/slide{n}.xml\"/>"
        ));
        let body: String = (0..12)
            .map(|l| format!("<a:p><a:r><a:t>slide {n} line {l} text</a:t></a:r></a:p>"))
            .collect();
        entries.push((format!("ppt/slides/slide{n}.xml"), format!("<p:sld {ns}><p:cSld><p:spTree><p:sp><p:txBody>{body}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>").into_bytes()));
    }
    entries.push((
        "ppt/presentation.xml".into(),
        format!("<p:presentation {ns}><p:sldIdLst>{slides}</p:sldIdLst></p:presentation>")
            .into_bytes(),
    ));
    entries.push(("ppt/_rels/presentation.xml.rels".into(), format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}</Relationships>").into_bytes()));
    let refs: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    same_when_sliced("long.pptx", inline("long.pptx", &zip_of(&refs)), 3);
}

#[test]
fn a_cursor_is_refused_for_another_request_and_for_a_changed_file() {
    let input = json!({"path": fixtures(), "files": ["long.txt"], "max_bytes": 4096});
    let first = call(&input);
    let cursor = first["next"]["cursor"].clone();
    let mut other = input.clone();
    other["cursor"] = cursor.clone();
    other["files"] = json!(["many.pdf"]);
    let err = run(&other.to_string()).unwrap_err();
    assert!(err.contains("different request"), "{err}");
    let mut broken = input;
    broken["cursor"] = json!("ocr1.not-a-cursor");
    assert!(
        run(&broken.to_string())
            .unwrap_err()
            .contains("not one this plugin returned")
    );
}

#[test]
fn an_epub_chapter_over_the_whole_tree_limit_is_read_in_chunks_and_continues() {
    let container = br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf"/></rootfiles></container>"#.to_vec();
    let opf = br#"<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#.to_vec();
    let paras = 330_000;
    let mut chapter = String::from("<html><body>");
    let mut expected =
        String::from("<!-- document: big.epub (epub) -->\n\n<!-- section 1: a.xhtml -->");
    for i in 0..paras {
        chapter.push_str(&format!("<p>paragraph number {i}</p>\n"));
        expected.push_str(&format!("\n\nparagraph number {i}"));
    }
    chapter.push_str("</body></html>");
    assert!(chapter.len() > 8 * 1024 * 1024);
    let data = zip_of(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container),
        ("c.opf", opf),
        ("a.xhtml", chapter.into_bytes()),
    ]);
    let joined = sliced(inline("big.epub", &data));
    assert!(joined.calls >= 2, "{} call(s)", joined.calls);
    assert_eq!(joined.markdown, expected);
}

#[test]
fn a_long_ods_reads_the_same_in_slices_and_names_its_header() {
    let ns = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"";
    let mut rows = String::from(
        "<table:table-row><table:table-cell><text:p>Item</text:p></table:table-cell><table:table-cell><text:p>Cost</text:p></table:table-cell></table:table-row>",
    );
    for i in 0..2500 {
        rows.push_str(&format!("<table:table-row><table:table-cell><text:p>thing {i}</text:p></table:table-cell><table:table-cell><text:p>{}</text:p></table:table-cell></table:table-row>", i * 3));
    }
    let content = format!(
        "<office:document-content {ns}><office:body><office:spreadsheet><table:table table:name=\"A\">{rows}</table:table><table:table table:name=\"B\"><table:table-row><table:table-cell><text:p>end</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"
    );
    let data = zip_of(&[
        (
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
        ),
        ("content.xml", content.into_bytes()),
    ]);
    let input = inline("long.ods", &data);
    same_when_sliced("long.ods", input.clone(), 15);
    let mut small = input;
    small["max_bytes"] = json!(4096);
    let first = call(&small);
    small["cursor"] = first["next"]["cursor"].clone();
    let second = call(&small);
    assert_eq!(
        second["documents"][0]["table_header"],
        "| Item | Cost |\n| --- | --- |"
    );
}
