//! The lazy PDF reader on files built to exercise what the classic writer in
//! `pdfgen` does not: object streams, cross-reference streams with a PNG
//! predictor, inherited page attributes, nested page trees, incremental
//! updates and files the reader must hand to the whole-file path.
use crate::ctx::Ctx;
use crate::input::{Options, PageSel};
use crate::pdf::convert;
use crate::pdfgen::{PageSpec, deflate, pdf, text};
use crate::pdflazy::{Lazy, WINDOW_PAGES};
use crate::src::Src;

fn opts(pages: Option<&str>) -> Options {
    Options {
        pages: pages.map(|p| PageSel::parse(p).unwrap()),
        ..Options::default()
    }
}

fn read(bytes: Vec<u8>, pages: Option<&str>) -> Result<String, String> {
    let mut ctx = Ctx::new(opts(pages));
    convert(&mut ctx, "t.pdf", &mut Src::mem(bytes), None).map(|d| d.markdown)
}

fn flate_obj(n: u32, extra: &str, data: &[u8]) -> Vec<u8> {
    let packed = deflate(data);
    let mut out = format!(
        "{n} 0 obj\n<< {extra} /Filter /FlateDecode /Length {} >>\nstream\n",
        packed.len()
    )
    .into_bytes();
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    out
}

fn content(s: &str) -> Vec<u8> {
    format!("BT /F1 12 Tf 72 700 Td ({s}) Tj ET").into_bytes()
}

/// Pages 1 to 3 under two page-tree nodes; the font and the page size are
/// inherited from the root; the tree and the font live in an object stream and
/// the index is a cross-reference stream with a PNG Up predictor.
fn modern() -> Vec<u8> {
    let page =
        |c: u32, parent: u32| format!("<< /Type /Page /Parent {parent} 0 R /Contents {c} 0 R >>");
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 3 /Resources << /Font << /F1 8 0 R >> >> /MediaBox [0 0 612 792] >>".to_string(),
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /Count 2 >>".to_string(),
        "<< /Type /Pages /Parent 2 0 R /Kids [7 0 R] /Count 1 >>".to_string(),
        page(9, 3),
        page(10, 3),
        page(11, 4),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let (mut header, mut body) = (String::new(), String::new());
    for (i, b) in bodies.iter().enumerate() {
        header.push_str(&format!("{} {} ", i + 1, body.len()));
        body.push_str(b);
        body.push('\n');
    }
    let first = header.len();
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut at = [0usize; 14];
    for (n, s) in [(9, "Alpha page"), (10, "Beta page"), (11, "Gamma page")] {
        at[n] = out.len();
        out.extend(flate_obj(n as u32, "", &content(s)));
    }
    at[12] = out.len();
    out.extend(flate_obj(
        12,
        &format!("/Type /ObjStm /N 8 /First {first}"),
        format!("{header}{body}").as_bytes(),
    ));
    at[13] = out.len();
    // type 0 free, 1 at an offset, 2 inside object stream 12 at an index
    let mut rows: Vec<[u8; 7]> = vec![[0; 7]];
    for (n, &off) in at.iter().enumerate().skip(1) {
        let (t, a, b) = match n {
            1..=8 => (2u8, 12u32, (n - 1) as u16),
            _ => (1, off as u32, 0),
        };
        let mut r = [0u8; 7];
        r[0] = t;
        r[1..5].copy_from_slice(&a.to_be_bytes());
        r[5..7].copy_from_slice(&b.to_be_bytes());
        rows.push(r);
    }
    let mut predicted = Vec::new();
    let mut prev = [0u8; 7];
    for r in &rows {
        predicted.push(2);
        predicted.extend((0..7).map(|c| r[c].wrapping_sub(prev[c])));
        prev = *r;
    }
    out.extend(flate_obj(
        13,
        "/Type /XRef /Size 14 /W [1 4 2] /Root 1 0 R /DecodeParms << /Predictor 12 /Columns 7 >>",
        &predicted,
    ));
    out.extend(format!("startxref\n{}\n%%EOF\n", at[13]).as_bytes());
    out
}

#[test]
fn object_streams_xref_streams_and_inherited_attributes_are_read() {
    let md = read(modern(), None).unwrap();
    for (n, word) in [(1, "Alpha page"), (2, "Beta page"), (3, "Gamma page")] {
        assert!(md.contains(&format!("<!-- page {n} -->\n\n{word}")), "{md}");
    }
    let only = read(modern(), Some("3")).unwrap();
    assert!(
        only.contains("Gamma page") && !only.contains("Alpha page"),
        "{only}"
    );
}

fn spec(s: &str) -> PageSpec<'static> {
    PageSpec {
        w: 612.0,
        h: 792.0,
        content: text("F1", 12.0, 72.0, 700.0, s),
        images: Vec::new(),
    }
}

#[test]
fn an_incremental_update_replaces_an_object() {
    let base = pdf(&[spec("old one"), spec("old two"), spec("old three")]);
    let xref_at = {
        let at = base.windows(9).rposition(|w| w == b"startxref").unwrap();
        String::from_utf8_lossy(&base[at + 10..])
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap()
    };
    // Object 8 is the content of page 2 in `pdfgen`'s numbering.
    let mut file = base.clone();
    let body = text("F1", 12.0, 72.0, 700.0, "new two");
    let new_at = file.len();
    file.extend(
        format!(
            "8 0 obj\n<< /Length {} >>\nstream\n{body}\nendstream\nendobj\n",
            body.len()
        )
        .as_bytes(),
    );
    let table_at = file.len();
    file.extend(format!("xref\n8 1\n{new_at:010} 00000 n \ntrailer\n<< /Size 12 /Root 1 0 R /Prev {xref_at} >>\nstartxref\n{table_at}\n%%EOF\n").as_bytes());
    let md = read(file, None).unwrap();
    assert!(
        md.contains("new two") && md.contains("old one") && !md.contains("old two"),
        "{md}"
    );
}

#[test]
fn a_window_holds_the_selected_pages_and_the_next_one_continues() {
    let specs: Vec<_> = (1..=40).map(|i| spec(&format!("page text {i}"))).collect();
    let mut lazy = Lazy::open(Src::mem(pdf(&specs))).unwrap();
    assert_eq!(lazy.total, 40);
    let first = lazy.window(&opts(None), 1, 1).unwrap();
    assert_eq!(first.numbers, (1..=WINDOW_PAGES as u32).collect::<Vec<_>>());
    assert_eq!(first.after, WINDOW_PAGES as u32 + 1);
    let picked = lazy.window(&opts(Some("5,30-31,40")), 1, 1).unwrap();
    assert_eq!(picked.numbers, vec![5, 30, 31, 40]);
    assert!(picked.after > 40);
    let rest = lazy.window(&opts(None), 33, 1).unwrap();
    assert_eq!(rest.numbers, (33..=40).collect::<Vec<_>>());
}

#[test]
fn damaged_and_encrypted_files_go_to_the_whole_file_reader() {
    let base = pdf(&[spec("still readable")]);
    let at = base.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut damaged = base[..at].to_vec();
    damaged.extend(b"startxref\n7\n%%EOF\n");
    assert!(Lazy::open(Src::mem(damaged.clone())).is_err());
    assert!(read(damaged, None).unwrap().contains("still readable"));

    let mut locked = base.clone();
    let trailer = locked.windows(5).rposition(|w| w == b"/Root").unwrap();
    locked.splice(trailer..trailer, b"/Encrypt 3 0 R ".iter().copied());
    let err = Lazy::open(Src::mem(locked)).err().unwrap();
    assert!(err.contains("encrypted"), "{err}");
}

#[test]
fn a_page_tree_that_contains_itself_ends_with_an_error() {
    let mut file = pdf(&[spec("a"), spec("b")]);
    let kids = file.windows(10).position(|w| w == b"/Kids [7 0").unwrap();
    // The root now lists itself first; the file keeps its length.
    file[kids + 7..kids + 12].copy_from_slice(b"2 0 R");
    let err = read(file, None).unwrap_err();
    assert!(err.contains("nested too deeply"), "{err}");
}
