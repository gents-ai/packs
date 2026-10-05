//! A contact sheet: several images in one grid, each shrunk to fit its cell
//! and labelled with its name. An image that cannot be read gets a crossed-out
//! cell and its error in the result, so the sheet always covers every input.
use crate::draw::{self, GLYPH, Rgba};
use crate::geom::fit_dims;
use crate::load::load;
use crate::model::Img;
use crate::resize::{Filter, resample};
use crate::src::Source;

/// Images one montage holds.
pub const MAX_IMAGES: usize = 400;

/// How the sheet is laid out.
pub struct Spec {
    /// Columns; the nearest square grid when absent.
    pub cols: Option<u32>,
    /// Side of each square cell in pixels.
    pub cell: u32,
    /// Space between cells and around the sheet.
    pub gap: u32,
    /// Whether each cell carries its image's name.
    pub labels: bool,
    /// Sheet background.
    pub background: Rgba,
}

/// Where one image went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// 1-based position, row-major.
    pub n: usize,
    /// The image's name.
    pub source: String,
    pub col: u32,
    pub row: u32,
    /// Top-left of the drawn image on the sheet.
    pub x: u32,
    pub y: u32,
    /// Size of the drawn image.
    pub width: u32,
    pub height: u32,
    /// Why the image could not be drawn.
    pub error: Option<String>,
}

/// The finished sheet.
pub struct Sheet {
    pub img: Img,
    pub cells: Vec<Placed>,
    pub warnings: Vec<String>,
}

fn label_scale(cell: u32) -> u32 {
    if cell >= 192 { 2 } else { 1 }
}

/// The name cut to `max` characters with `...` at the end when it is longer.
fn shorten(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_owned();
    }
    let keep = max.saturating_sub(3);
    format!(
        "{}{}",
        name.chars().take(keep).collect::<String>(),
        ".".repeat(max.min(3))
    )
}

/// Builds the sheet for `sources`.
pub fn make(sources: &[Source], spec: &Spec, frame: u32, turn: bool) -> Result<Sheet, String> {
    let n = sources.len();
    if n == 0 || n > MAX_IMAGES {
        return Err(format!(
            "a montage holds between 1 and {MAX_IMAGES} images; list the ones to include in files"
        ));
    }
    let cols = spec
        .cols
        .unwrap_or_else(|| (n as f64).sqrt().ceil() as u32)
        .clamp(1, n as u32);
    let rows = (n as u32).div_ceil(cols);
    let scale = label_scale(spec.cell);
    let label_h = if spec.labels { GLYPH * scale + 4 } else { 0 };
    let (cw, ch) = (spec.cell, spec.cell + label_h);
    let width = u64::from(spec.gap) + u64::from(cols) * (u64::from(cw) + u64::from(spec.gap));
    let height = u64::from(spec.gap) + u64::from(rows) * (u64::from(ch) + u64::from(spec.gap));
    let too_big = || {
        format!(
            "the sheet would be {width}x{height} pixels, over the image size limit; use a smaller cell or fewer images"
        )
    };
    let (w32, h32) = (
        u32::try_from(width).map_err(|_| too_big())?,
        u32::try_from(height).map_err(|_| too_big())?,
    );
    let mut sheet = Img::filled(w32, h32, spec.background).map_err(|_| too_big())?;
    let mut cells = Vec::with_capacity(n);
    let mut replaced = 0;
    for (i, src) in sources.iter().enumerate() {
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        let (ox, oy) = (
            spec.gap + col * (cw + spec.gap),
            spec.gap + row * (ch + spec.gap),
        );
        let mut placed = Placed {
            n: i + 1,
            source: src.name.clone(),
            col,
            row,
            x: ox,
            y: oy,
            width: 0,
            height: 0,
            error: None,
        };
        match load(src, frame, turn, Some(spec.cell)) {
            Ok(img) => {
                let (tw, th) = if img.w > spec.cell || img.h > spec.cell {
                    fit_dims(img.w, img.h, Some(spec.cell), Some(spec.cell))
                } else {
                    (img.w, img.h)
                };
                let thumb = resample(img, tw, th, Filter::Lanczos3)?;
                let (x, y) = (ox + (spec.cell - tw) / 2, oy + (spec.cell - th) / 2);
                for yy in 0..th {
                    for xx in 0..tw {
                        let c = thumb.get(xx, yy);
                        draw::blend(&mut sheet, i64::from(x + xx), i64::from(y + yy), c);
                    }
                }
                (placed.x, placed.y, placed.width, placed.height) = (x, y, tw, th);
            }
            Err(e) => {
                let side = i64::from(spec.cell);
                let (x, y) = (i64::from(ox), i64::from(oy));
                draw::fill_rect(&mut sheet, x, y, spec.cell, spec.cell, [221, 221, 221, 255]);
                draw::line(
                    &mut sheet,
                    [x, y],
                    [x + side - 1, y + side - 1],
                    2,
                    [200, 0, 0, 255],
                );
                draw::line(
                    &mut sheet,
                    [x + side - 1, y],
                    [x, y + side - 1],
                    2,
                    [200, 0, 0, 255],
                );
                placed.width = spec.cell;
                placed.height = spec.cell;
                placed.error = Some(e);
            }
        }
        if spec.labels {
            let max = (spec.cell / (GLYPH * scale)) as usize;
            let text = shorten(&src.name, max.max(1));
            replaced += draw::text(
                &mut sheet,
                i64::from(ox),
                i64::from(oy + spec.cell + 2),
                &text,
                scale,
                [0, 0, 0, 255],
                None,
            );
        }
        cells.push(placed);
    }
    let mut warnings = Vec::new();
    if replaced > 0 {
        warnings.push(format!(
            "{replaced} label characters outside plain ASCII were drawn as ?"
        ));
    }
    Ok(Sheet {
        img: sheet,
        cells,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures as fx;
    use crate::src::Data;
    use std::sync::Arc;

    fn solid(name: &str, w: u32, h: u32, c: [u8; 4]) -> Source {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(c));
        Source {
            name: name.into(),
            data: Data::Mem(Arc::new(fx::png(&img))),
        }
    }

    fn spec(cell: u32, gap: u32, labels: bool) -> Spec {
        Spec {
            cols: None,
            cell,
            gap,
            labels,
            background: [255, 255, 255, 255],
        }
    }

    #[test]
    fn four_images_make_a_two_by_two_grid_at_known_positions() {
        let s = [
            solid("a.png", 16, 16, [255, 0, 0, 255]),
            solid("b.png", 16, 16, [0, 255, 0, 255]),
            solid("c.png", 16, 16, [0, 0, 255, 255]),
            solid("d.png", 16, 16, [9, 9, 9, 255]),
        ];
        let sheet = make(&s, &spec(16, 2, false), 0, true).unwrap();
        // 2 + 2 * (16 + 2) = 38 in both directions.
        assert_eq!((sheet.img.w, sheet.img.h), (38, 38));
        assert_eq!(sheet.img.get(2, 2), [255, 0, 0, 255]);
        assert_eq!(sheet.img.get(18 + 2, 2), [0, 255, 0, 255]);
        assert_eq!(sheet.img.get(2, 20), [0, 0, 255, 255]);
        assert_eq!(sheet.img.get(20, 20), [9, 9, 9, 255]);
        assert_eq!(
            sheet.img.get(0, 0),
            [255, 255, 255, 255],
            "the gap shows the background"
        );
        assert_eq!(sheet.img.get(18, 5), [255, 255, 255, 255]);
        let d = &sheet.cells[3];
        assert_eq!(
            (d.n, d.col, d.row, d.x, d.y, d.width, d.height),
            (4, 1, 1, 20, 20, 16, 16)
        );
        assert!(sheet.cells.iter().all(|c| c.error.is_none()));
    }

    #[test]
    fn larger_images_are_fitted_and_smaller_ones_centred_not_enlarged() {
        let s = [
            solid("wide.png", 64, 32, [255, 0, 0, 255]),
            solid("tiny.png", 4, 4, [0, 0, 255, 255]),
        ];
        let sheet = make(
            &s,
            &Spec {
                cols: Some(2),
                ..spec(16, 0, false)
            },
            0,
            true,
        )
        .unwrap();
        let (w, t) = (&sheet.cells[0], &sheet.cells[1]);
        assert_eq!((w.width, w.height, w.x, w.y), (16, 8, 0, 4));
        assert_eq!((t.width, t.height, t.x, t.y), (4, 4, 16 + 6, 6));
        assert_eq!(sheet.img.get(8, 8), [255, 0, 0, 255]);
        assert_eq!(
            sheet.img.get(8, 2),
            [255, 255, 255, 255],
            "letterbox above the wide image"
        );
        assert_eq!(sheet.img.get(24, 8), [0, 0, 255, 255]);
    }

    #[test]
    fn labels_add_a_strip_under_each_cell_with_the_name() {
        let s = [solid("ab.png", 16, 16, [255, 0, 0, 255])];
        let with = make(&s, &spec(64, 4, true), 0, true).unwrap();
        let without = make(&s, &spec(64, 4, false), 0, true).unwrap();
        assert_eq!(
            with.img.h,
            without.img.h + 12,
            "strip of 8 px glyphs plus 4 px padding"
        );
        let strip_has_ink =
            (0..64).any(|x| (0..8).any(|y| with.img.get(4 + x, 4 + 64 + 2 + y) == [0, 0, 0, 255]));
        assert!(strip_has_ink);
        assert_eq!(
            with.img.get(4 + 63, 4 + 64 + 2),
            [255, 255, 255, 255],
            "no ink past the 6 characters"
        );
    }

    #[test]
    fn a_long_name_is_shortened_with_dots() {
        assert_eq!(shorten("short.png", 20), "short.png");
        assert_eq!(shorten("a_very_long_file_name.png", 10), "a_very_...");
        assert_eq!(shorten("abcdef", 3), "...");
        assert_eq!(shorten("abcdef", 2), "..");
    }

    #[test]
    fn an_unreadable_image_gets_a_crossed_cell_and_its_error() {
        let bad = Source {
            name: "broken.png".into(),
            data: Data::Mem(Arc::new(b"not an image at all".to_vec())),
        };
        let s = [solid("ok.png", 16, 16, [0, 200, 0, 255]), bad];
        let sheet = make(&s, &spec(16, 2, false), 0, true).unwrap();
        assert!(sheet.cells[0].error.is_none());
        let e = sheet.cells[1].error.as_deref().unwrap();
        assert!(e.contains("not a PNG"), "{e}");
        assert_eq!(
            sheet.img.get(20 + 8, 2 + 8),
            [200, 0, 0, 255],
            "the cross passes through the middle"
        );
        assert_eq!(
            sheet.img.get(20 + 1, 2 + 7),
            [221, 221, 221, 255],
            "the rest of the cell is gray"
        );
    }

    #[test]
    fn column_count_and_limits_are_checked() {
        let one = [solid("a.png", 4, 4, [0, 0, 0, 255])];
        let sheet = make(
            &one,
            &Spec {
                cols: Some(5),
                ..spec(16, 2, false)
            },
            0,
            true,
        )
        .unwrap();
        assert_eq!(sheet.img.w, 2 + 18, "columns never exceed the image count");
        let many: Vec<Source> = (0..MAX_IMAGES + 1)
            .map(|i| solid(&format!("{i}.png"), 2, 2, [0, 0, 0, 255]))
            .collect();
        assert!(
            make(&many, &spec(16, 2, false), 0, true)
                .err()
                .unwrap()
                .contains("between 1 and 400")
        );
        assert!(make(&[], &spec(16, 2, false), 0, true).is_err());
        let huge = make(
            &many[..300],
            &Spec {
                cols: Some(100),
                ..spec(1024, 256, false)
            },
            0,
            true,
        )
        .err()
        .unwrap();
        assert!(huge.contains("over the image size limit"), "{huge}");
    }

    #[test]
    fn label_text_doubles_at_a_cell_of_192() {
        assert_eq!([191, 192, 193].map(label_scale), [1, 2, 2]);
        // The strip under a cell is the glyph height plus 4 px of padding.
        let s = [solid("a.png", 8, 8, [0, 0, 0, 255])];
        let strip = |cell| {
            let with = make(&s, &spec(cell, 2, true), 0, true).unwrap();
            let without = make(&s, &spec(cell, 2, false), 0, true).unwrap();
            with.img.h - without.img.h
        };
        assert_eq!([191, 192, 193].map(strip), [12, 20, 20]);
    }

    #[test]
    fn a_montage_of_exactly_the_cap_is_made_and_one_more_is_refused() {
        let at = |n: usize| -> Vec<Source> {
            (0..n)
                .map(|i| solid(&format!("{i}.png"), 2, 2, [0, 0, 0, 255]))
                .collect()
        };
        let sheet = make(&at(MAX_IMAGES), &spec(16, 2, false), 0, true).unwrap();
        assert_eq!(sheet.cells.len(), MAX_IMAGES);
        assert!(sheet.cells.iter().all(|c| c.error.is_none()));
        let e = make(&at(MAX_IMAGES + 1), &spec(16, 2, false), 0, true)
            .err()
            .unwrap();
        assert!(e.contains("between 1 and 400"), "{e}");
    }

    #[test]
    fn non_ascii_names_are_noted() {
        let s = [solid("caf\u{e9}.png", 4, 4, [0, 0, 0, 255])];
        let sheet = make(&s, &spec(64, 2, true), 0, true).unwrap();
        assert_eq!(sheet.warnings.len(), 1);
        assert!(sheet.warnings[0].contains("1 label characters"));
    }
}
