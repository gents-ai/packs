//! Generates the test fixtures (and, with --bench, larger benchmark inputs).
//!
//! Usage: cargo run --release --example gen_fixtures -- <out-dir> [--bench]
//!
//! Everything is built with the crate's own dependencies, so it runs the same
//! on Linux and macOS, and every byte is deterministic: zip entries carry a
//! fixed timestamp and no random ids are used. The OCR-facing images are
//! rendered from generated PDF pages with the same renderer the plugin uses.
use std::io::{Cursor, Write};
use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[allow(dead_code)]
#[path = "../source/pdfgen.rs"]
mod pdfgen;
use pdfgen::{Img, PageSpec, draw, pdf, scan, text};

fn write(dir: &Path, name: &str, data: &[u8]) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture directory");
    }
    std::fs::write(&path, data).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("{:>9} bytes  {}", data.len(), path.display());
}

/// Zip with a fixed timestamp; `stored_first` is written uncompressed first (EPUB mimetype).
fn zip_of(entries: &[(&str, Vec<u8>)], stored_first: Option<&str>) -> Vec<u8> {
    let mut zw = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut put = |name: &str, data: &[u8], opts: SimpleFileOptions| {
        zw.start_file(name, opts).expect("start zip entry");
        zw.write_all(data).expect("write zip entry");
    };
    if let Some(first) = stored_first
        && let Some((_, d)) = entries.iter().find(|(n, _)| *n == first)
    {
        put(first, d, stored);
    }
    for (name, data) in entries {
        if Some(*name) != stored_first {
            put(name, data, deflated);
        }
    }
    zw.finish().expect("finish zip").into_inner()
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct Gray {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

/// Renders page 1 of a generated PDF to 8-bit gray.
fn render_gray(pdf_bytes: Vec<u8>, scale: f32) -> Gray {
    let parsed = Pdf::new(pdf_bytes).expect("generated PDF parses");
    let settings = InterpreterSettings::default();
    let rs = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
        ..RenderSettings::default()
    };
    let pix = render(&parsed.pages()[0], &RenderCache::new(), &settings, &rs);
    let data = pix
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0])
        .collect();
    Gray {
        w: u32::from(pix.width()),
        h: u32::from(pix.height()),
        data,
    }
}

fn png(g: &Gray) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&g.data, g.w, g.h, image::ExtendedColorType::L8)
        .expect("encode png");
    out
}

use image::ImageEncoder;

fn page(w: f32, h: f32, content: String) -> PageSpec<'static> {
    PageSpec {
        w,
        h,
        content,
        images: Vec::new(),
    }
}

/// A chart-like image: a title and three labelled values.
fn chart_png() -> Vec<u8> {
    let mut c = text("F2", 16.0, 14.0, 150.0, "Revenue by region");
    c += &text("F1", 13.0, 14.0, 110.0, "North 42 million");
    c += &text("F1", 13.0, 14.0, 84.0, "South 37 million");
    c += &text("F1", 13.0, 14.0, 58.0, "East 29 million");
    png(&render_gray(pdf(&[page(260.0, 180.0, c)]), 2.0))
}

/// A scanned letter: heading and two paragraphs as pixels.
fn letter_gray() -> Gray {
    let mut c = text("F2", 22.0, 48.0, 500.0, "Quarterly Results");
    c += &text(
        "F1",
        13.0,
        48.0,
        460.0,
        "The quick brown fox jumps over the lazy dog.",
    );
    c += &text(
        "F1",
        13.0,
        48.0,
        440.0,
        "Revenue grew steadily across every region.",
    );
    c += &text(
        "F1",
        13.0,
        48.0,
        400.0,
        "Costs stayed flat while demand increased.",
    );
    render_gray(pdf(&[page(420.0, 560.0, c)]), 2.0)
}

fn text_pdf() -> Vec<u8> {
    let mut p1 = text("F2", 24.0, 56.0, 770.0, "Annual Report");
    p1 += &text(
        "F1",
        11.0,
        56.0,
        730.0,
        "This report summarises the year. Revenue rose in every region and the",
    );
    p1 += &text(
        "F1",
        11.0,
        56.0,
        716.0,
        "team shipped the new platform on schedule.",
    );
    p1 += &text(
        "F1",
        11.0,
        56.0,
        686.0,
        "Costs were held flat while headcount grew by ten percent.",
    );
    p1 += &text("F2", 15.0, 56.0, 640.0, "Highlights");
    p1 += &text(
        "F1",
        11.0,
        56.0,
        614.0,
        "- Record revenue in the northern region",
    );
    p1 += &text("F1", 11.0, 56.0, 598.0, "- Platform launch completed");
    p1 += &text(
        "F1",
        11.0,
        56.0,
        582.0,
        "- Customer satisfaction at 94 percent",
    );
    let mut p2 = text("F2", 15.0, 56.0, 770.0, "Results by region");
    for (r, row) in [
        ["Region", "Revenue", "Growth"],
        ["North", "42", "12%"],
        ["South", "37", "8%"],
        ["East", "29", "5%"],
    ]
    .iter()
    .enumerate()
    {
        for (c, cell) in row.iter().enumerate() {
            p2 += &text(
                "F1",
                11.0,
                56.0 + c as f32 * 120.0,
                730.0 - r as f32 * 16.0,
                cell,
            );
        }
    }
    let chart = Img {
        w: 200,
        h: 120,
        gray: true,
        data: (0..200 * 120)
            .map(|i| ((i % 200) * 255 / 200) as u8)
            .collect(),
        flate: true,
    };
    p2 += &draw("Im1", 56.0, 520.0, 250.0, 150.0);
    p2 += &text("F1", 10.0, 56.0, 500.0, "Figure 1. Gradient test chart");
    pdf(&[
        page(595.0, 842.0, p1),
        PageSpec {
            w: 595.0,
            h: 842.0,
            content: p2,
            images: vec![("Im1", &chart)],
        },
    ])
}

/// Layout cases of a real document: a table whose cells sit about 9 points
/// apart, a figure caption followed by body text, and a two-column page whose
/// figure sits in the left column.
fn layout_pdf() -> Vec<u8> {
    let mut p1 = text("F2", 18.0, 56.0, 780.0, "Sales review");
    p1 += &text(
        "F1",
        11.0,
        56.0,
        752.0,
        "The regions are compared below, cell by cell.",
    );
    for (r, row) in [
        ["Region", "Q1", "Q2", "Notes"],
        ["North", "10", "12", "good"],
        ["South", "8", "9", "ok"],
        ["East", "7", "6", "weak"],
    ]
    .iter()
    .enumerate()
    {
        for (c, cell) in row.iter().enumerate() {
            let x = 56.0 + [0.0, 41.0, 63.0, 86.0][c];
            p1 += &text("F1", 10.0, x, 720.0 - r as f32 * 14.0, cell);
        }
    }
    p1 += &text("F1", 11.0, 56.0, 640.0, "Paragraph after the table.");
    p1 += &draw("Im1", 56.0, 480.0, 160.0, 120.0);
    p1 += &text("F1", 11.0, 56.0, 462.0, "Figure 1. Sales chart");
    p1 += &text("F1", 11.0, 56.0, 450.0, "Tail paragraph after the caption.");
    let chart = Img {
        w: 160,
        h: 120,
        gray: true,
        data: (0..160 * 120)
            .map(|i| ((i % 160) * 255 / 160) as u8)
            .collect(),
        flate: true,
    };
    let chart2 = Img {
        data: (0..160 * 120)
            .map(|i| ((i / 160) * 255 / 120) as u8)
            .collect(),
        ..chart_like(&chart)
    };
    let mut p2 = text("F2", 16.0, 56.0, 780.0, "Two column page");
    p2 += &text("F1", 11.0, 56.0, 750.0, "Left intro paragraph of the page.");
    p2 += &draw("Im1", 56.0, 600.0, 160.0, 120.0);
    p2 += &text("F1", 11.0, 56.0, 582.0, "Figure 2. Left column chart");
    p2 += &text("F1", 11.0, 56.0, 552.0, "Left text after the figure.");
    for (i, t) in [
        "Right column first paragraph.",
        "Right column second paragraph.",
        "Right column third paragraph.",
    ]
    .iter()
    .enumerate()
    {
        p2 += &text("F1", 11.0, 330.0, 750.0 - i as f32 * 40.0, t);
    }
    pdf(&[
        PageSpec {
            w: 595.0,
            h: 842.0,
            content: p1,
            images: vec![("Im1", &chart)],
        },
        PageSpec {
            w: 595.0,
            h: 842.0,
            content: p2,
            images: vec![("Im1", &chart2)],
        },
    ])
}

fn chart_like(img: &Img) -> Img {
    Img {
        w: img.w,
        h: img.h,
        gray: img.gray,
        data: Vec::new(),
        flate: img.flate,
    }
}

/// A few bytes that declare a 40000 x 40000 pixel image: the plugin must skip
/// it with a warning, never decode it.
fn hostile_pdf() -> Vec<u8> {
    let huge = Img {
        w: 40_000,
        h: 40_000,
        gray: true,
        data: vec![0; 64],
        flate: false,
    };
    let mut c = text("F1", 12.0, 56.0, 760.0, "This page also has readable text.");
    c += &draw("Im1", 56.0, 400.0, 300.0, 300.0);
    pdf(&[PageSpec {
        w: 595.0,
        h: 842.0,
        content: c,
        images: vec![("Im1", &huge)],
    }])
}

/// A two-column scanned page as pixels: a title, then a column of each.
fn columns_gray() -> Gray {
    let mut c = text("F2", 26.0, 120.0, 520.0, "Main Title");
    c += &text("F2", 14.0, 40.0, 470.0, "Introduction");
    c += &text("F2", 14.0, 230.0, 470.0, "Results");
    for (i, t) in [
        "The left column describes",
        "the method in some detail",
        "and ends with this line.",
    ]
    .iter()
    .enumerate()
    {
        c += &text("F1", 13.0, 40.0, 445.0 - i as f32 * 18.0, t);
    }
    for (i, t) in [
        "The right column reports",
        "the results of the study",
        "and must be read second.",
    ]
    .iter()
    .enumerate()
    {
        c += &text("F1", 13.0, 230.0, 445.0 - i as f32 * 18.0, t);
    }
    render_gray(pdf(&[page(420.0, 560.0, c)]), 2.0)
}

fn epub(figure: &[u8]) -> Vec<u8> {
    let ch1 = r##"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>One</title></head><body>
<h1>The Long Road</h1>
<p>It began with a <em>small</em> idea and a <strong>large</strong> map. Read more at <a href="https://example.com/road">the project page</a>.</p>
<ul><li>Pack the maps</li><li>Check the weather<ul><li>Rain expected</li></ul></li></ul>
<ol><li>Start early<ol><li>Pack the car</li></ol></li><li>Drive north</li></ol>
<p>The road was long<sup><a epub:type="noteref" href="#n1">1</a></sup> and quiet.</p>
<aside epub:type="footnote" id="n1"><p>Measured in miles, not hours.</p></aside>
<figure><img src="../images/chart.png" alt="Bar chart"/><figcaption>Figure 1. Revenue by region</figcaption></figure>
</body></html>"##;
    let ch2 = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Two</title></head><body>
<h2>Second Stop</h2>
<p>The table below lists the stops.</p>
<table><tr><th>Stop</th><th>Miles</th></tr><tr><td>Harbor</td><td>12</td></tr><tr><td>Ridge</td><td>31</td></tr></table>
<blockquote><p>Every road ends somewhere.</p></blockquote>
</body></html>"#;
    let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>The Long Road</dc:title><dc:identifier id="id">fixture-book</dc:identifier><dc:language>en</dc:language></metadata>
<manifest><item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/><item id="img" href="images/chart.png" media-type="image/png"/></manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#;
    let container = r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    zip_of(
        &[
            ("mimetype", b"application/epub+zip".to_vec()),
            ("META-INF/container.xml", container.as_bytes().to_vec()),
            ("OEBPS/content.opf", opf.as_bytes().to_vec()),
            ("OEBPS/text/ch1.xhtml", ch1.as_bytes().to_vec()),
            ("OEBPS/text/ch2.xhtml", ch2.as_bytes().to_vec()),
            ("OEBPS/images/chart.png", figure.to_vec()),
        ],
        Some("mimetype"),
    )
}

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"";
const CT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/></Types>";

fn docx(figure: &[u8]) -> Vec<u8> {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W_NS}><w:body>
<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Plan Report</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Plan</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Revenue is </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>up</w:t></w:r><w:r><w:t xml:space="preserve"> and costs are </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>flat</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Hire two engineers</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Ship the platform</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Budget</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Hiring</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>120</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Cloud</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>45</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Revenue chart"/><a:graphic><a:graphicData><a:blip r:embed="rId5"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Caption"/></w:pPr><w:r><w:t>Figure 1 - Revenue by region</w:t></w:r></w:p>
</w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W_NS}><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="Caption"><w:name w:val="caption"/></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W_NS}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/chart.png"/></Relationships>"#;
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    zip_of(
        &[
            ("[Content_Types].xml", CT.as_bytes().to_vec()),
            ("_rels/.rels", root_rels.as_bytes().to_vec()),
            ("word/document.xml", body.into_bytes()),
            ("word/styles.xml", styles.into_bytes()),
            ("word/numbering.xml", numbering.into_bytes()),
            ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
            ("word/media/chart.png", figure.to_vec()),
        ],
        None,
    )
}

fn pptx(figure: &[u8]) -> Vec<u8> {
    let ns = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
    let shape = |ph: &str, paras: &str| {
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"s\"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/>{paras}</p:txBody></p:sp>"
        )
    };
    let para = |t: &str| format!("<a:p><a:r><a:t>{}</a:t></a:r></a:p>", xml(t));
    let bullet = |t: &str| {
        format!(
            "<a:p><a:pPr lvl=\"0\"><a:buChar char=\"-\"/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p>",
            xml(t)
        )
    };
    let slide1 = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:sld {ns}><p:cSld><p:spTree>{}{}</p:spTree></p:cSld></p:sld>",
        shape(
            "",
            &(bullet("Launch in March") + &bullet("Budget approved"))
        ),
        shape("<p:ph type=\"title\"/>", &para("Product Launch"))
    );
    let table = "<p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Quarter</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>Units</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Q1</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>120</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>";
    let pic = "<p:pic><p:nvPicPr><p:cNvPr id=\"5\" name=\"Picture\" descr=\"Sales chart\"/></p:nvPicPr><p:blipFill><a:blip r:embed=\"rId2\"/></p:blipFill></p:pic>";
    let slide2 = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:sld {ns}><p:cSld><p:spTree>{}{table}{pic}</p:spTree></p:cSld></p:sld>",
        shape("<p:ph type=\"title\"/>", &para("Sales"))
    );
    let notes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:notes {ns}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:notes>",
        shape(
            "<p:ph type=\"body\"/>",
            &para("Mention the pilot customers")
        )
    );
    let pres = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:presentation {ns}><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/><p:sldId id=\"257\" r:id=\"rId2\"/></p:sldIdLst></p:presentation>"
    );
    let rel = |items: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{items}</Relationships>"
        )
    };
    let t = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    zip_of(
        &[
            ("[Content_Types].xml", CT.as_bytes().to_vec()),
            ("ppt/presentation.xml", pres.into_bytes()),
            ("ppt/_rels/presentation.xml.rels", rel(&format!("<Relationship Id=\"rId1\" Type=\"{t}/slide\" Target=\"slides/slide1.xml\"/><Relationship Id=\"rId2\" Type=\"{t}/slide\" Target=\"slides/slide2.xml\"/>")).into_bytes()),
            ("ppt/slides/slide1.xml", slide1.into_bytes()),
            ("ppt/slides/slide2.xml", slide2.into_bytes()),
            ("ppt/slides/_rels/slide2.xml.rels", rel(&format!("<Relationship Id=\"rId2\" Type=\"{t}/image\" Target=\"../media/chart.png\"/><Relationship Id=\"rId3\" Type=\"{t}/notesSlide\" Target=\"../notesSlides/notesSlide2.xml\"/>")).into_bytes()),
            ("ppt/notesSlides/notesSlide2.xml", notes.into_bytes()),
            ("ppt/media/chart.png", figure.to_vec()),
        ],
        None,
    )
}

fn xlsx() -> Vec<u8> {
    let ns = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook {ns}><sheets><sheet name=\"Sales\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Notes\" sheetId=\"2\" r:id=\"rId2\"/></sheets></workbook>"
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"x/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"x/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>";
    let shared = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><sst {ns}><si><t>Region</t></si><si><t>Revenue</t></si><si><t>Date</t></si><si><t>North</t></si><si><t>South</t></si><si><t>Remember to re-run after the audit.</t></si></sst>"
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><styleSheet {ns}><cellXfs count=\"2\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/></cellXfs></styleSheet>"
    );
    let sheet1 = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet {ns}><dimension ref=\"A1:C3\"/><sheetData><row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c><c r=\"C1\" t=\"s\"><v>2</v></c></row><row r=\"2\"><c r=\"A2\" t=\"s\"><v>3</v></c><c r=\"B2\"><v>42.5</v></c><c r=\"C2\" s=\"1\"><v>45292</v></c></row><row r=\"3\"><c r=\"A3\" t=\"s\"><v>4</v></c><c r=\"B3\"><v>37</v></c><c r=\"C3\" s=\"1\"><v>45323</v></c></row></sheetData></worksheet>"
    );
    let sheet2 = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet {ns}><sheetData><row r=\"1\"><c r=\"A1\" t=\"s\"><v>5</v></c></row></sheetData></worksheet>"
    );
    zip_of(
        &[
            ("[Content_Types].xml", CT.as_bytes().to_vec()),
            ("xl/workbook.xml", workbook.into_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes().to_vec()),
            ("xl/sharedStrings.xml", shared.into_bytes()),
            ("xl/styles.xml", styles.into_bytes()),
            ("xl/worksheets/sheet1.xml", sheet1.into_bytes()),
            ("xl/worksheets/sheet2.xml", sheet2.into_bytes()),
        ],
        None,
    )
}

fn odf(mime: &str, content: &str, figure: Option<&[u8]>) -> Vec<u8> {
    let mut entries = vec![
        ("mimetype", mime.as_bytes().to_vec()),
        ("content.xml", content.as_bytes().to_vec()),
    ];
    if let Some(f) = figure {
        entries.push(("Pictures/chart.png", f.to_vec()));
    }
    zip_of(&entries, Some("mimetype"))
}

const ODF_NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\"";

fn odt(figure: &[u8]) -> Vec<u8> {
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content {ODF_NS}><office:automatic-styles><text:list-style style:name=\"L2\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\"><text:list-level-style-number text:level=\"1\"/></text:list-style></office:automatic-styles><office:body><office:text>\
<text:h text:outline-level=\"1\">Meeting Notes</text:h><text:p>We agreed on <text:span>three</text:span> things<text:note><text:note-body><text:p>Decided on Monday.</text:p></text:note-body></text:note>.</text:p>\
<text:list text:style-name=\"L2\"><text:list-item><text:p>First action</text:p></text:list-item><text:list-item><text:p>Second action</text:p></text:list-item></text:list>\
<table:table><table:table-row><table:table-cell><text:p>Owner</text:p></table:table-cell><table:table-cell><text:p>Task</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell><text:p>Ana</text:p></table:table-cell><table:table-cell><text:p>Draft</text:p></table:table-cell></table:table-row></table:table>\
<text:p><draw:frame draw:name=\"Chart\"><draw:image xlink:href=\"Pictures/chart.png\"/><svg:desc>Revenue chart</svg:desc></draw:frame></text:p>\
</office:text></office:body></office:document-content>"
    );
    odf(
        "application/vnd.oasis.opendocument.text",
        &content,
        Some(figure),
    )
}

fn ods() -> Vec<u8> {
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content {ODF_NS}><office:body><office:spreadsheet><table:table table:name=\"Budget\">\
<table:table-row><table:table-cell><text:p>Item</text:p></table:table-cell><table:table-cell><text:p>Cost</text:p></table:table-cell></table:table-row>\
<table:table-row><table:table-cell><text:p>Laptops</text:p></table:table-cell><table:table-cell><text:p>1200</text:p></table:table-cell></table:table-row>\
<table:table-row table:number-rows-repeated=\"1000000\"><table:table-cell table:number-columns-repeated=\"1024\"/></table:table-row>\
</table:table></office:spreadsheet></office:body></office:document-content>"
    );
    odf(
        "application/vnd.oasis.opendocument.spreadsheet",
        &content,
        None,
    )
}

fn odp() -> Vec<u8> {
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content {ODF_NS}><office:body><office:presentation>\
<draw:page draw:name=\"p1\"><draw:frame><draw:text-box><text:p>Welcome slide</text:p></draw:text-box></draw:frame></draw:page>\
<draw:page draw:name=\"p2\"><draw:frame><draw:text-box><text:p>Second slide</text:p></draw:text-box></draw:frame></draw:page>\
</office:presentation></office:body></office:document-content>"
    );
    odf(
        "application/vnd.oasis.opendocument.presentation",
        &content,
        None,
    )
}

const HTML_PAGE: &str = "<!DOCTYPE html>\n<html><head><title>Status</title></head><body>\n<h1>Status Page</h1>\n<p>All systems are <b>operational</b>. See the <a href=\"https://example.com/status\">live board</a>.</p>\n<ol><li>Database</li><li>Queue</li></ol>\n<table><tr><th>Service</th><th>State</th></tr><tr><td>API</td><td>up</td></tr></table>\n<img src=\"img/chart.png\" alt=\"Latency chart\">\n</body></html>\n";

/// Lorem-style sentences for the large benchmark inputs.
fn sentence(i: usize) -> String {
    const WORDS: [&str; 24] = [
        "system", "design", "memory", "network", "reader", "engine", "buffer", "stream", "table",
        "record", "value", "index", "query", "plan", "cache", "queue", "model", "graph", "token",
        "vector", "kernel", "driver", "socket", "thread",
    ];
    (0..14)
        .map(|k| WORDS[(i * 7 + k * 13 + k * k) % WORDS.len()])
        .collect::<Vec<_>>()
        .join(" ")
        + "."
}

fn bench_pdf(pages: usize) -> Vec<u8> {
    let specs: Vec<PageSpec<'static>> = (0..pages)
        .map(|p| {
            let mut c = text("F2", 18.0, 56.0, 790.0, &format!("Chapter {}", p + 1));
            for l in 0..52 {
                c += &text(
                    "F1",
                    10.0,
                    56.0,
                    760.0 - l as f32 * 13.5,
                    &sentence(p * 52 + l),
                );
            }
            page(595.0, 842.0, c)
        })
        .collect();
    pdf(&specs)
}

fn bench_epub(chapters: usize, paragraphs: usize, figure: &[u8]) -> Vec<u8> {
    let mut entries: Vec<(String, Vec<u8>)> =
        vec![("mimetype".into(), b"application/epub+zip".to_vec())];
    entries.push(("META-INF/container.xml".into(), br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.to_vec()));
    let mut manifest = String::new();
    let mut spine = String::new();
    for c in 0..chapters {
        let mut body = format!(
            "<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><body><h1>Chapter {}</h1>",
            c + 1
        );
        for p in 0..paragraphs {
            body += &format!(
                "<p>{} {} <em>{}</em></p>",
                sentence(c * paragraphs + p),
                sentence(p),
                sentence(c)
            );
        }
        body += "</body></html>";
        entries.push((format!("OEBPS/c{c}.xhtml"), body.into_bytes()));
        manifest += &format!(
            "<item id=\"c{c}\" href=\"c{c}.xhtml\" media-type=\"application/xhtml+xml\"/>"
        );
        spine += &format!("<itemref idref=\"c{c}\"/>");
    }
    entries.push(("OEBPS/chart.png".into(), figure.to_vec()));
    entries.push(("OEBPS/content.opf".into(), format!("<?xml version=\"1.0\"?><package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"><manifest>{manifest}</manifest><spine>{spine}</spine></package>").into_bytes()));
    let refs: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    zip_of(&refs, Some("mimetype"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: gen_fixtures <out-dir> [--bench]");
    let bench = args.next().as_deref() == Some("--bench");
    let dir = Path::new(&out);
    let figure = chart_png();
    let letter = letter_gray();
    if bench {
        write(dir, "text-100p.pdf", &bench_pdf(100));
        write(dir, "book-large.epub", &bench_epub(40, 600, &figure));
        write(
            dir,
            "scan-page.pdf",
            &scan(letter.w, letter.h, letter.data.clone(), 420.0, 560.0),
        );
        write(dir, "scan-page.png", &png(&letter));
        // A dense A4 page of 52 text lines, as a scanner would deliver it.
        let dense = render_gray(bench_pdf(1), 2.0);
        write(
            dir,
            "scan-dense.pdf",
            &scan(dense.w, dense.h, dense.data.clone(), 595.0, 842.0),
        );
        return;
    }
    write(dir, "text.pdf", &text_pdf());
    write(
        dir,
        "scan.pdf",
        &scan(letter.w, letter.h, letter.data.clone(), 420.0, 560.0),
    );
    write(dir, "letter.png", &png(&letter));
    write(dir, "chart.png", &figure);
    write(dir, "book.epub", &epub(&figure));
    write(dir, "report.docx", &docx(&figure));
    write(dir, "deck.pptx", &pptx(&figure));
    write(dir, "sheet.xlsx", &xlsx());
    write(dir, "notes.odt", &odt(&figure));
    write(dir, "budget.ods", &ods());
    write(dir, "slides.odp", &odp());
    write(dir, "layout.pdf", &layout_pdf());
    write(dir, "columns.png", &png(&columns_gray()));
    write(dir, "page.html", HTML_PAGE.as_bytes());
    write(dir, "notes.md", b"# Notes\r\n\r\n- one\r\n- two\r\n");
    write(dir, "data.csv", b"name,score\nAna,9\n\"Bo, Jr.\",7\n");
    write(dir, "plain.txt", b"Plain text\nsecond line\n");
    // Inputs that take several calls with a small max_bytes.
    write(dir, "many.pdf", &bench_pdf(6));
    let long: String = (0..400).map(|i| format!("{}\n", sentence(i))).collect();
    write(dir, "long.txt", long.as_bytes());
    // A directory for the bind cases: a page with a sibling image, a sheet, a
    // corrupt PDF and a file type the plugin does not read.
    write(dir, "tree/page.html", HTML_PAGE.as_bytes());
    write(dir, "tree/img/chart.png", &figure);
    write(dir, "tree/data.csv", b"name,score\nAna,9\n");
    write(dir, "tree/broken.pdf", b"%PDF-1.4\nthis is not a pdf body");
    write(dir, "tree/skip.xyz", b"opaque");
    // A directory whose PDF declares an absurd image size next to a readable file.
    write(dir, "hostile/hostile.pdf", &hostile_pdf());
    write(
        dir,
        "hostile/note.txt",
        b"A readable note beside the hostile PDF.\n",
    );
}
