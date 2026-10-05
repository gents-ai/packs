//! Tests for reading codes: each format with a known payload, rotated,
//! inverted, low-contrast, damaged, tiny, huge, and several in one picture.
use super::codes::*;
use super::fixtures as fx;
use super::geom;
use super::model::Img;
use rxing::BarcodeFormat as F;

fn img(i: &image::RgbaImage) -> Img {
    Img::from_raw(i.width(), i.height(), i.as_raw().clone()).unwrap()
}

fn texts(f: &Found) -> Vec<(&'static str, String)> {
    f.codes.iter().map(|c| (c.format, c.text.clone())).collect()
}

/// Puts `code` on a larger white page at (`x`, `y`).
fn on_page(code: &Img, pw: u32, ph: u32, x: u32, y: u32) -> Img {
    let mut page = Img::filled(pw, ph, [255, 255, 255, 255]).unwrap();
    for cy in 0..code.h {
        for cx in 0..code.w {
            let (s, d) = (code.at(cx, cy), page.at(x + cx, y + cy));
            page.px[d..d + 4].copy_from_slice(&code.px[s..s + 4]);
        }
    }
    page
}

#[test]
fn a_plain_qr_code_gives_its_payload_position_and_attempt() {
    let code = img(&fx::qr("https://example.org/a?b=1", 4));
    let page = on_page(&code, 300, 260, 50, 30);
    let f = decode(&page, &[]);
    assert_eq!(texts(&f), vec![("qr_code", "https://example.org/a?b=1".to_string())]);
    assert_eq!(f.attempt, "plain");
    let b = f.codes[0].bbox;
    // The finder patterns sit inside the code's own square, which starts 16 px (the quiet zone) in.
    assert!(b[0] >= 50 + 16 && b[1] >= 30 + 16, "{b:?}");
    assert!(b[0] + b[2] <= 50 + i64::from(code.w) && b[1] + b[3] <= 30 + i64::from(code.h), "{b:?}");
    assert!(b[2] > 40 && b[3] > 40, "{b:?}");
}

#[test]
fn the_payload_survives_rotation_by_quarter_turns() {
    let code = img(&fx::qr("ROTATE-ME-123", 5));
    for deg in [90, 180, 270] {
        let r = geom::rotate(&code, deg).unwrap();
        assert_eq!(texts(&decode(&r, &[])), vec![("qr_code", "ROTATE-ME-123".to_string())], "{deg} degrees");
    }
}

#[test]
fn a_light_on_dark_code_is_read_by_the_inverted_attempt() {
    let code = img(&fx::qr("INVERTED", 5));
    let mut inv = code.clone();
    for p in inv.px.chunks_exact_mut(4) {
        for c in &mut p[..3] {
            *c = 255 - *c;
        }
    }
    let f = decode(&inv, &[]);
    assert_eq!(texts(&f), vec![("qr_code", "INVERTED".to_string())]);
    assert!(f.attempt == "plain" || f.attempt == "inverted", "{}", f.attempt);
}

#[test]
fn a_low_contrast_code_is_read_after_stretching() {
    let code = img(&fx::qr("LOW-CONTRAST", 5));
    let mut faint = code.clone();
    for p in faint.px.chunks_exact_mut(4) {
        let v = 120 + (u32::from(p[0]) * 40 / 255) as u8;
        p[0] = v;
        p[1] = v;
        p[2] = v;
    }
    let f = decode(&faint, &[]);
    assert_eq!(texts(&f), vec![("qr_code", "LOW-CONTRAST".to_string())]);
}

#[test]
fn a_damaged_code_is_read_through_error_correction() {
    let mut code = img(&fx::qr("DAMAGED-BUT-READABLE-0123456789", 6));
    // Paint over a patch in the data area, well under the 15% a medium level repairs.
    let (w, h) = (code.w, code.h);
    for y in h / 2..h / 2 + h / 12 {
        for x in w / 2..w / 2 + w / 8 {
            let i = code.at(x, y);
            code.px[i..i + 3].copy_from_slice(&[0, 0, 0]);
        }
    }
    assert_eq!(texts(&decode(&code, &[])), vec![("qr_code", "DAMAGED-BUT-READABLE-0123456789".to_string())]);
}

#[test]
fn a_destroyed_code_is_reported_as_none_not_an_error() {
    let mut code = img(&fx::qr("GONE", 5));
    for p in code.px.chunks_exact_mut(4) {
        p[..3].copy_from_slice(&[255, 255, 255]);
    }
    let f = decode(&code, &[]);
    assert!(f.codes.is_empty());
    assert_eq!(f.attempt, "none");
    let noise = img(&fx::noise(200, 200, 5));
    assert!(decode(&noise, &[]).codes.is_empty());
}

#[test]
fn one_dimensional_and_matrix_formats_decode_to_their_payloads() {
    let cases = [
        (F::CODE_128, "ABC-12345", "code_128", 360, 90),
        (F::EAN_13, "5901234123457", "ean_13", 300, 90),
        (F::EAN_8, "96385074", "ean_8", 240, 90),
        (F::UPC_A, "036000291452", "upc_a", 300, 90),
        (F::CODE_39, "CODE39", "code_39", 360, 90),
        (F::ITF, "123456789012", "itf", 360, 90),
        (F::DATA_MATRIX, "DM-PAYLOAD-77", "data_matrix", 120, 120),
        (F::AZTEC, "AZTEC-PAYLOAD", "aztec", 120, 120),
        (F::PDF_417, "PDF417 PAYLOAD 123", "pdf_417", 400, 120),
    ];
    for (format, text, name, w, h) in cases {
        let page = on_page(&img(&fx::barcode(format, text, w, h)), w as u32 + 80, h as u32 + 80, 40, 40);
        let f = decode(&page, &[]);
        assert_eq!(texts(&f), vec![(name, text.to_string())], "{name}");
    }
}

#[test]
fn a_vertical_barcode_is_read_after_a_quarter_turn() {
    let code = on_page(&img(&fx::barcode(F::CODE_128, "VERTICAL-1", 320, 90)), 400, 170, 40, 40);
    let turned = geom::rotate(&code, 90).unwrap();
    assert!(texts(&decode(&turned, &[])).contains(&("code_128", "VERTICAL-1".to_string())));
}

#[test]
fn several_codes_in_one_picture_are_all_returned_top_to_bottom() {
    let a = img(&fx::qr("FIRST", 4));
    let b = img(&fx::qr("SECOND", 4));
    let c = img(&fx::qr("THIRD", 4));
    let mut page = on_page(&a, 700, 420, 20, 20);
    for (code, x, y) in [(&b, 360, 40), (&c, 100, 250)] {
        for cy in 0..code.h {
            for cx in 0..code.w {
                let (s, d) = (code.at(cx, cy), page.at(x + cx, y + cy));
                page.px[d..d + 4].copy_from_slice(&code.px[s..s + 4]);
            }
        }
    }
    let f = decode(&page, &[]);
    let got: Vec<String> = f.codes.iter().map(|c| c.text.clone()).collect();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[2], "THIRD", "the lowest code comes last");
    assert!(got.contains(&"FIRST".to_string()) && got.contains(&"SECOND".to_string()));
}

#[test]
fn a_format_filter_restricts_what_is_read() {
    let qr = on_page(&img(&fx::qr("ONLY-QR", 4)), 260, 260, 20, 20);
    assert_eq!(decode(&qr, &parse_formats(&["qr_code".into()]).unwrap()).codes.len(), 1);
    assert!(decode(&qr, &parse_formats(&["ean_13".into(), "code_128".into()]).unwrap()).codes.is_empty());
    let e = parse_formats(&["qr".into()]).err().unwrap();
    assert!(e.contains("not a code format") && e.contains("qr_code"), "{e}");
}

#[test]
fn a_tiny_code_is_read_after_enlarging() {
    let code = img(&fx::qr("TINY", 1));
    assert!(code.w < 60);
    let f = decode(&code, &[]);
    assert_eq!(texts(&f), vec![("qr_code", "TINY".to_string())]);
    assert!(f.codes[0].bbox[0] >= 0 && f.codes[0].bbox[0] + f.codes[0].bbox[2] <= i64::from(code.w), "{:?}", f.codes[0].bbox);
}

#[test]
fn a_large_picture_is_read_and_positions_map_back_to_its_pixels() {
    let code = img(&fx::qr("BIG-PAGE", 12));
    let page = on_page(&code, 3200, 2400, 2000, 1400);
    let f = decode(&page, &[]);
    assert_eq!(texts(&f), vec![("qr_code", "BIG-PAGE".to_string())]);
    let b = f.codes[0].bbox;
    assert!(b[0] >= 2000 && b[1] >= 1400 && b[0] + b[2] <= 2000 + i64::from(code.w) && b[1] + b[3] <= 1400 + i64::from(code.h), "{b:?} via {}", f.attempt);
}

#[test]
fn duplicate_codes_are_listed_once() {
    let code = img(&fx::qr("TWICE", 4));
    let mut page = on_page(&code, 600, 260, 20, 20);
    for cy in 0..code.h {
        for cx in 0..code.w {
            let (s, d) = (code.at(cx, cy), page.at(300 + cx, 20 + cy));
            page.px[d..d + 4].copy_from_slice(&code.px[s..s + 4]);
        }
    }
    assert_eq!(decode(&page, &[]).codes.len(), 1);
}
