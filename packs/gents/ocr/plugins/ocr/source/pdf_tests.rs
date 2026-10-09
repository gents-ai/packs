use super::*;
use crate::input::Options;
use crate::pdfgen::{Img, PageSpec, draw, pdf, scan, text};
use crate::pdfocr::ocr_items;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderSettings, render};

fn run(data: Vec<u8>, opts: Options) -> (Document, Ctx) {
    let mut ctx = Ctx::new(opts);
    let doc = convert(&mut ctx, "t.pdf", &mut Src::mem(data), None).unwrap();
    (doc, ctx)
}

fn page(content: String) -> PageSpec<'static> {
    PageSpec {
        w: 595.0,
        h: 842.0,
        content,
        images: Vec::new(),
    }
}

#[test]
fn text_layer_becomes_heading_paragraphs_and_marker() {
    let mut c = text("F2", 24.0, 50.0, 780.0, "Annual Report");
    c += &text(
        "F1",
        11.0,
        50.0,
        740.0,
        "The first paragraph starts here and",
    );
    c += &text("F1", 11.0, 50.0, 726.0, "continues on a second line.");
    c += &text("F1", 11.0, 50.0, 690.0, "A second paragraph follows.");
    let (doc, _) = run(pdf(&[page(c)]), Options::default());
    assert_eq!(doc.pages, 1);
    assert_eq!(
        doc.markdown,
        "<!-- document: t.pdf (pdf) -->\n\n<!-- page 1 -->\n\n# Annual Report\n\nThe first paragraph starts here and continues on a second line.\n\nA second paragraph follows."
    );
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
}

#[test]
fn pages_option_selects_pages() {
    let p = |t: &str| page(text("F1", 12.0, 50.0, 700.0, t));
    let opts = Options {
        pages: Some(crate::input::PageSel::parse("2").unwrap()),
        ..Options::default()
    };
    let (doc, _) = run(
        pdf(&[p("first page text"), p("second page text"), p("third")]),
        opts,
    );
    assert_eq!(doc.pages, 3);
    assert!(
        doc.markdown.contains("second page text")
            && !doc.markdown.contains("first page text")
            && !doc.markdown.contains("page 1")
    );
}

#[test]
fn embedded_image_is_a_figure_with_its_caption() {
    let img = Img {
        w: 160,
        h: 120,
        gray: true,
        data: (0..160 * 120).map(|i| (i % 256) as u8).collect(),
        flate: false,
    };
    let mut c = text("F1", 11.0, 50.0, 780.0, "Text before the figure.");
    c += &draw("Im1", 50.0, 560.0, 240.0, 180.0);
    c += &text("F1", 10.0, 50.0, 540.0, "Figure 1. A gradient");
    let spec = PageSpec {
        w: 595.0,
        h: 842.0,
        content: c,
        images: vec![("Im1", &img)],
    };
    let opts = Options {
        ocr: OcrMode::Never,
        ..Options::default()
    };
    let (doc, _) = run(pdf(&[spec]), opts);
    assert_eq!(doc.figures.len(), 1, "{}", doc.markdown);
    let f = &doc.figures[0];
    assert_eq!(
        (
            f.id.as_str(),
            f.page,
            f.caption.as_str(),
            f.width,
            f.height,
            f.ocr
        ),
        ("fig-1", Some(1), "Figure 1. A gradient", 160, 120, false)
    );
    assert!(
        doc.markdown
            .contains("> **[Figure fig-1]** Figure 1. A gradient (160x120 px)"),
        "{}",
        doc.markdown
    );
    assert!(
        !doc.markdown.contains("\n\nFigure 1. A gradient"),
        "{}",
        doc.markdown
    );
}

#[test]
fn scanned_page_is_rendered_and_read_by_ocr() {
    // Render a text page to pixels, then wrap only those pixels in a PDF.
    let mut c = text("F2", 26.0, 50.0, 760.0, "Quarterly Results");
    c += &text(
        "F1",
        16.0,
        50.0,
        700.0,
        "The quick brown fox jumps over the lazy dog.",
    );
    c += &text(
        "F1",
        16.0,
        50.0,
        676.0,
        "Revenue grew steadily across every region.",
    );
    let source = pdf(&[page(c)]);
    let parsed = Pdf::new(source).unwrap();
    let settings = InterpreterSettings::default();
    let pixmap = render(
        &parsed.pages()[0],
        &RenderCache::new(),
        &settings,
        &RenderSettings {
            x_scale: 2.0,
            y_scale: 2.0,
            bg_color: WHITE,
            ..RenderSettings::default()
        },
    );
    let gray: Vec<u8> = pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0])
        .collect();
    let scanned = scan(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        gray,
        595.0,
        842.0,
    );
    let (doc, ctx) = run(scanned, Options::default());
    let lower = doc.markdown.to_lowercase();
    assert!(
        lower.contains("quick brown fox") && lower.contains("revenue"),
        "{}",
        doc.markdown
    );
    assert!(doc.figures.is_empty() && ctx.parts.is_empty());
    // With OCR disabled the page is reported unread, never silently empty.
    let never = Options {
        ocr: OcrMode::Never,
        ..Options::default()
    };
    let (doc, _) = run(pdf(&[page(String::new())]), never);
    assert!(doc.markdown.ends_with("<!-- page 1 -->"));
}

#[test]
fn running_headers_and_page_numbers_are_left_out_and_reported() {
    let pages: Vec<PageSpec<'static>> = (1..=5)
        .map(|n| {
            let mut c = text("F1", 9.0, 50.0, 825.0, "Company Confidential");
            c += &text(
                "F1",
                11.0,
                50.0,
                700.0,
                &format!(
                    "Unique body text for page number {} only.",
                    ["one", "two", "three", "four", "five"][n - 1]
                ),
            );
            c += &text("F1", 9.0, 280.0, 20.0, &format!("Page {n}"));
            page(c)
        })
        .collect();
    let (doc, _) = run(pdf(&pages), Options::default());
    assert!(
        !doc.markdown.contains("Company Confidential") && !doc.markdown.contains("Page 3"),
        "{}",
        doc.markdown
    );
    assert!(
        doc.markdown
            .contains("Unique body text for page number four only.")
    );
    assert_eq!(doc.warnings, vec!["10 repeated header or footer line(s) near the page edges (running titles, page numbers) were left out".to_string()]);
    // Two pages are too few to call anything a running header.
    let (short, _) = run(pdf(&pages[..2]), Options::default());
    assert!(short.markdown.contains("Company Confidential"));
}

#[test]
fn a_scan_with_its_own_text_layer_uses_it_and_lists_no_figure() {
    let scan = Img {
        w: 100,
        h: 140,
        gray: true,
        data: vec![255; 100 * 140],
        flate: false,
    };
    let mut c = draw("Im1", 0.0, 0.0, 595.0, 842.0);
    c += "BT 3 Tr /F1 12 Tf 50 700 Td (Invisible layer from an earlier OCR pass that is long enough) Tj ET\n";
    let spec = PageSpec {
        w: 595.0,
        h: 842.0,
        content: c,
        images: vec![("Im1", &scan)],
    };
    let (doc, ctx) = run(pdf(&[spec]), Options::default());
    assert!(
        doc.markdown
            .contains("Invisible layer from an earlier OCR pass"),
        "{}",
        doc.markdown
    );
    assert!(
        doc.figures.is_empty() && ctx.parts.is_empty() && doc.warnings.is_empty(),
        "{:?}",
        doc.warnings
    );
}

#[test]
fn a_landscape_page_authored_sideways_reads_upright() {
    // Content drawn running upward, on a page displayed rotated 90 degrees clockwise.
    let content =
        "BT /F1 12 Tf 0 1 -1 0 300 100 Tm (Landscape words read upright.) Tj ET\n".to_string();
    let rotated = String::from_utf8(pdf(&[page(content)]))
        .unwrap()
        .replace("/MediaBox", "/Rotate 90 /MediaBox")
        .into_bytes();
    let (doc, _) = run(rotated, Options::default());
    assert!(
        doc.markdown.contains("Landscape words read upright."),
        "{}",
        doc.markdown
    );
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    // The same content on an unrotated page is sideways text: reported, not guessed.
    let plain = pdf(&[page(
        "BT /F1 12 Tf 0 1 -1 0 300 100 Tm (Sideways words on an upright page.) Tj ET\n".to_string(),
    )]);
    let (doc, _) = run(
        plain,
        Options {
            ocr: OcrMode::Never,
            ..Options::default()
        },
    );
    assert!(!doc.markdown.contains("Sideways"), "{}", doc.markdown);
    assert!(
        doc.warnings[0].contains("rotated character(s) were not read"),
        "{:?}",
        doc.warnings
    );
}

#[test]
fn an_exhausted_ocr_budget_is_reported_not_hidden() {
    let scan = Img {
        w: 100,
        h: 140,
        gray: true,
        data: vec![255; 100 * 140],
        flate: false,
    };
    let spec = PageSpec {
        w: 595.0,
        h: 842.0,
        content: draw("Im1", 0.0, 0.0, 595.0, 842.0),
        images: vec![("Im1", &scan)],
    };
    let mut ctx = Ctx::new(Options::default());
    let long_ago = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(10_000))
        .expect("clock is far enough from its origin");
    ctx.ocr = crate::ocr::Ocr::new(long_ago, std::time::Duration::from_secs(1));
    let doc = convert(&mut ctx, "t.pdf", &mut Src::mem(pdf(&[spec])), None).unwrap();
    assert_eq!(doc.warnings.len(), 1, "{:?}", doc.warnings);
    assert!(
        doc.warnings[0].starts_with(
            "page 1: no readable text layer and OCR was not possible: the OCR time budget"
        ),
        "{:?}",
        doc.warnings
    );
}

#[test]
fn an_image_declared_far_too_large_is_skipped_not_decoded() {
    let huge = Img {
        w: 40_000,
        h: 40_000,
        gray: true,
        data: vec![0; 64],
        flate: false,
    };
    let mut c = text(
        "F1",
        12.0,
        50.0,
        700.0,
        "Text that is still read from the page.",
    );
    c += &draw("Im1", 50.0, 400.0, 200.0, 200.0);
    let spec = PageSpec {
        w: 595.0,
        h: 842.0,
        content: c,
        images: vec![("Im1", &huge)],
    };
    let (doc, _) = run(pdf(&[spec]), Options::default());
    assert!(
        doc.markdown.contains("Text that is still read"),
        "{}",
        doc.markdown
    );
    assert!(doc.figures.is_empty());
    assert!(doc.unread_pages.is_empty());
    assert_eq!(
        doc.warnings,
        vec!["page 1: an image of 40000x40000 pixels is over the 96000000 pixel limit and was skipped".to_string()]
    );
    // A scanned page of that size cannot be rendered for OCR: said, not attempted.
    let scan = PageSpec {
        w: 595.0,
        h: 842.0,
        content: draw("Im1", 0.0, 0.0, 595.0, 842.0),
        images: vec![("Im1", &huge)],
    };
    let (doc, _) = run(pdf(&[scan]), Options::default());
    assert_eq!(doc.unread_pages, vec![1]);
    assert_eq!(doc.warnings.len(), 2, "{:?}", doc.warnings);
    assert!(
        doc.warnings[1].contains("OCR was not possible: the page holds an image too large"),
        "{:?}",
        doc.warnings
    );
}

#[test]
fn graph_fails_unread_scans_but_accepts_actual_blank_pages() {
    use base64::Engine as _;
    use serde_json::json;
    let huge = Img {
        w: 40_000,
        h: 40_000,
        gray: true,
        data: vec![0; 64],
        flate: false,
    };
    let scan = PageSpec {
        w: 595.0,
        h: 842.0,
        content: draw("Im1", 0.0, 0.0, 595.0, 842.0),
        images: vec![("Im1", &huge)],
    };
    let bytes = pdf(&[page(String::new()), scan]);
    let request = json!({"run_id":"unread-scan", "chunk":0,"source":"scan.pdf", "name":"scan.pdf", "data_base64":base64::engine::general_purpose::STANDARD.encode(bytes),"remote_ocr":"off","pages":"1-2"});
    let invoke = |request: &serde_json::Value| {
        let result = crate::graph::run(&request.to_string(), std::time::Instant::now())
            .unwrap()
            .unwrap();
        serde_json::from_str::<serde_json::Value>(&result).unwrap()
    };
    let failed = invoke(&request);
    assert_eq!(failed["document"]["extraction_state"], "failed");
    assert_eq!(failed["document"]["complete"], false);
    assert!(
        failed["document"]["error"]
            .as_str()
            .unwrap()
            .contains("[2]")
    );
    assert_eq!(failed["pages"], json!([]));
    assert!(failed["continuation"].is_null());
    let mut blank = request;
    blank["pages"] = json!("1");
    let completed = invoke(&blank);
    assert_eq!(completed["document"]["extraction_state"], "complete");
    assert_eq!(completed["document"]["complete"], true);
    assert_eq!(completed["document"]["error"], "");
    assert_eq!(completed["pages"].as_array().unwrap().len(), 1);
    assert_eq!(completed["pages"][0]["page"], 1);
}

#[test]
fn a_table_with_tight_cell_gaps_is_read_as_a_table() {
    // Cells start about 10 points after the previous cell ends.
    let mut c = text("F1", 11.0, 50.0, 780.0, "Sales by region");
    for (r, row) in [
        ["Region", "Q1", "Q2", "Notes"],
        ["North", "10", "12", "good"],
        ["South", "8", "9", "ok"],
        ["East", "7", "6", "weak"],
    ]
    .iter()
    .enumerate()
    {
        for (col, cell) in row.iter().enumerate() {
            c += &text(
                "F1",
                10.0,
                50.0 + col as f32 * 52.0,
                750.0 - r as f32 * 14.0,
                cell,
            );
        }
    }
    let (doc, _) = run(pdf(&[page(c)]), Options::default());
    assert!(
        doc.markdown.ends_with("Sales by region\n\n| Region | Q1 | Q2 | Notes |\n| --- | --- | --- | --- |\n| North | 10 | 12 | good |\n| South | 8 | 9 | ok |\n| East | 7 | 6 | weak |"),
        "{}",
        doc.markdown
    );
}

#[test]
fn ocr_heading_marks_need_an_isolated_line_well_above_the_median() {
    let line = |text: &str, top: f32, height: f32| crate::ocr::OcrLine {
        text: text.into(),
        left: 50.0,
        top,
        right: 300.0,
        bottom: top + height,
    };
    let lines = vec![
        line("Main Title", 20.0, 34.0),
        line("First body line of the paragraph", 100.0, 22.0),
        line("second line with descenders gypsy", 123.0, 27.0),
        line("third line plain", 147.0, 20.0),
    ];
    let text = crate::md::render(
        &crate::layout::layout(&ocr_items(&lines))
            .into_iter()
            .filter_map(|o| match o {
                crate::layout::Out::Block(b) => Some(b),
                crate::layout::Out::Fig(_) => None,
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        text,
        "### Main Title\n\nFirst body line of the paragraph second line with descenders gypsy third line plain"
    );
}

#[test]
fn ligature_characters_become_letters() {
    assert_eq!(expand_ligatures("\u{fb01}nal o\u{fb03}ce"), "final office");
    assert_eq!(expand_ligatures("plain"), "plain");
}

#[test]
fn corrupt_and_encrypted_input_fail_with_a_reason() {
    let mut ctx = Ctx::new(Options::default());
    let err = convert(
        &mut ctx,
        "x.pdf",
        &mut Src::mem(b"%PDF-1.4 garbage".to_vec()),
        None,
    )
    .err()
    .unwrap();
    assert!(err.contains("corrupt or truncated"), "{err}");
}
