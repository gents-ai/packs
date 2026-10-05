//! Reading QR codes and common 1D and 2D barcodes. The picture is tried as it
//! is, then with its contrast stretched, then inverted, then shrunk (a large
//! photo) or enlarged (a tiny code); the first attempt that finds a code wins
//! and is named in the result. Positions are mapped back to the input image.
use std::collections::HashSet;

use rxing::{BarcodeFormat, DecodeHints};

use crate::hash::luma_milli;
use crate::model::Img;

/// Codes listed in a result.
pub const MAX_CODES: usize = 100;
/// Longest side attempted at full size; larger images are also tried shrunk.
const SHRINK_ABOVE: u32 = 2000;
/// Images shorter than this on their longest side are also tried enlarged.
const GROW_BELOW: u32 = 400;

/// One code found.
#[derive(Debug, Clone, PartialEq)]
pub struct Code {
    /// Stable format name such as `qr_code` or `ean_13`.
    pub format: &'static str,
    /// The decoded text.
    pub text: String,
    /// The corner or end points the reader reported, in input pixels.
    pub points: Vec<[i64; 2]>,
    /// The box around the points: x, y, width, height.
    pub bbox: [i64; 4],
}

/// The codes found and how.
pub struct Found {
    /// The codes, top to bottom then left to right, without duplicates.
    pub codes: Vec<Code>,
    /// Which attempt found them: `plain`, `stretched`, `inverted`, `shrunk` or `enlarged`.
    pub attempt: &'static str,
    /// Codes found beyond [`MAX_CODES`], not listed.
    pub omitted: usize,
}

// MaxiCode, RSS-14 and RSS Expanded are left out: those readers panic on some damaged pictures.
const FORMATS: [(&str, BarcodeFormat); 13] = [
    ("qr_code", BarcodeFormat::QR_CODE),
    ("aztec", BarcodeFormat::AZTEC),
    ("data_matrix", BarcodeFormat::DATA_MATRIX),
    ("pdf_417", BarcodeFormat::PDF_417),
    ("code_128", BarcodeFormat::CODE_128),
    ("code_39", BarcodeFormat::CODE_39),
    ("code_93", BarcodeFormat::CODE_93),
    ("codabar", BarcodeFormat::CODABAR),
    ("ean_8", BarcodeFormat::EAN_8),
    ("ean_13", BarcodeFormat::EAN_13),
    ("upc_a", BarcodeFormat::UPC_A),
    ("upc_e", BarcodeFormat::UPC_E),
    ("itf", BarcodeFormat::ITF),
];

/// The format names accepted in a request.
pub fn format_names() -> Vec<&'static str> {
    FORMATS.iter().map(|(n, _)| *n).collect()
}

/// Parses a list of format names into reader formats.
pub fn parse_formats(names: &[String]) -> Result<Vec<BarcodeFormat>, String> {
    names
        .iter()
        .map(|n| {
            FORMATS
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, f)| *f)
                .ok_or_else(|| {
                    format!(
                        "{n:?} is not a code format; use one of {}",
                        format_names().join(", ")
                    )
                })
        })
        .collect()
}

fn name_of(f: &BarcodeFormat) -> &'static str {
    FORMATS
        .iter()
        .find(|(_, k)| k == f)
        .map_or("other", |(n, _)| n)
}

#[derive(Clone)]
struct Gray {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn gray_of(img: &Img) -> Gray {
    Gray {
        w: img.w,
        h: img.h,
        px: img
            .px
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (luma_milli(p) / 1000) as u8)
            .collect(),
    }
}

/// Spreads the 1st to 99th percentile of the grays over the full range; `None` when already wide or flat.
fn stretch(g: &Gray) -> Option<Gray> {
    let mut hist = [0u64; 256];
    for &v in &g.px {
        hist[usize::from(v)] += 1;
    }
    let total = g.px.len() as u64;
    let at = |target: u64| {
        let mut run = 0;
        (0..256)
            .find(|&i| {
                run += hist[i];
                run >= target
            })
            .unwrap_or(255)
    };
    let (lo, hi) = (at(total / 100) as i32, at(total - total / 100) as i32);
    if hi - lo < 16 || (lo <= 8 && hi >= 247) {
        return None;
    }
    let px =
        g.px.iter()
            .map(|&v| ((i32::from(v) - lo) * 255 / (hi - lo)).clamp(0, 255) as u8)
            .collect();
    Some(Gray { w: g.w, h: g.h, px })
}

fn invert(g: &Gray) -> Gray {
    Gray {
        w: g.w,
        h: g.h,
        px: g.px.iter().map(|&v| 255 - v).collect(),
    }
}

/// Box-averages by the integer factor `k`.
fn shrink(g: &Gray, k: u32) -> Gray {
    let (w, h) = ((g.w / k).max(1), (g.h / k).max(1));
    let mut px = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut n) = (0u32, 0u32);
            for yy in y * k..((y + 1) * k).min(g.h) {
                for xx in x * k..((x + 1) * k).min(g.w) {
                    sum += u32::from(g.px[yy as usize * g.w as usize + xx as usize]);
                    n += 1;
                }
            }
            px.push((sum / n.max(1)) as u8);
        }
    }
    Gray { w, h, px }
}

/// Repeats every pixel `k` times in each direction.
fn grow(g: &Gray, k: u32) -> Gray {
    let (w, h) = (g.w * k, g.h * k);
    let mut px = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            px.push(g.px[(y / k) as usize * g.w as usize + (x / k) as usize]);
        }
    }
    Gray { w, h, px }
}

fn run(g: &Gray, formats: &[BarcodeFormat], scale: f64) -> Vec<Code> {
    let mut hints = DecodeHints::default();
    let wanted: HashSet<BarcodeFormat> = if formats.is_empty() {
        FORMATS.iter().map(|(_, f)| *f).collect()
    } else {
        formats.iter().copied().collect()
    };
    hints.PossibleFormats = Some(wanted);
    let Ok(results) =
        rxing::helpers::detect_multiple_in_luma_with_hints(g.px.clone(), g.w, g.h, &mut hints)
    else {
        return Vec::new();
    };
    results
        .iter()
        .map(|r| {
            let points: Vec<[i64; 2]> = r
                .getPoints()
                .iter()
                .map(|p| {
                    [
                        (f64::from(p.x) / scale).round() as i64,
                        (f64::from(p.y) / scale).round() as i64,
                    ]
                })
                .collect();
            let (x0, x1) = (
                points.iter().map(|p| p[0]).min().unwrap_or(0),
                points.iter().map(|p| p[0]).max().unwrap_or(0),
            );
            let (y0, y1) = (
                points.iter().map(|p| p[1]).min().unwrap_or(0),
                points.iter().map(|p| p[1]).max().unwrap_or(0),
            );
            Code {
                format: name_of(r.getBarcodeFormat()),
                text: r.getText().to_owned(),
                points,
                bbox: [x0, y0, x1 - x0, y1 - y0],
            }
        })
        .collect()
}

/// Finds the codes in `img`, limited to `formats` when it is not empty.
pub fn decode(img: &Img, formats: &[BarcodeFormat]) -> Found {
    let plain = gray_of(img);
    let stretched = stretch(&plain);
    let base = stretched.as_ref().unwrap_or(&plain);
    let longest = img.w.max(img.h);
    let mut attempts: Vec<(&'static str, Gray, f64)> = Vec::new();
    // A large picture is tried shrunk first: the full-size passes cost the most.
    if longest > SHRINK_ABOVE {
        let k = longest.div_ceil(SHRINK_ABOVE);
        attempts.push(("shrunk", shrink(base, k), 1.0 / f64::from(k)));
    }
    attempts.push(("plain", plain.clone(), 1.0));
    if let Some(s) = &stretched {
        attempts.push(("stretched", s.clone(), 1.0));
    }
    attempts.push(("inverted", invert(base), 1.0));
    if longest < GROW_BELOW {
        let k = (600 / longest.max(1)).clamp(2, 8);
        attempts.push(("enlarged", grow(base, k), f64::from(k)));
    }
    for (attempt, g, scale) in attempts {
        let mut codes = run(&g, formats, scale);
        if codes.is_empty() {
            continue;
        }
        let mut seen = HashSet::new();
        codes.retain(|c| seen.insert((c.format, c.text.clone())));
        codes.sort_by(|a, b| {
            (a.bbox[1], a.bbox[0], a.format, &a.text)
                .cmp(&(b.bbox[1], b.bbox[0], b.format, &b.text))
        });
        let omitted = codes.len().saturating_sub(MAX_CODES);
        codes.truncate(MAX_CODES);
        return Found {
            codes,
            attempt,
            omitted,
        };
    }
    Found {
        codes: Vec::new(),
        attempt: "none",
        omitted: 0,
    }
}
