//! PDF pages read by OCR: a page without a usable text layer is rendered, read,
//! and the lines become layout items with the heading rule below.
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};
use hayro_interpret::InterpreterSettings;
use hayro_syntax::page::Page;

use crate::ctx::Ctx;
use crate::layout::{Item, Span};
use crate::ocr::{OcrLine, Stats};
use crate::pix::Pix;
use crate::remote::{JPEG_QUALITY, REMOTE_MAX_PX, REMOTE_SCALE};

/// Pages are rendered at most this many times larger than their point size for OCR.
const MAX_RENDER_SCALE: f32 = 4.0;

/// Renders the page at `scale` times its point size on white.
fn render_page<'a>(
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    page: &'a Page<'a>,
    scale: f32,
) -> hayro::vello_cpu::Pixmap {
    let render_settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
        ..RenderSettings::default()
    };
    render(page, cache, settings, &render_settings)
}

/// Renders the page and reads it with the built-in OCR: the lines and what the read dropped.
pub fn ocr_lines<'a>(
    ctx: &mut Ctx,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    page: &'a Page<'a>,
) -> Result<(Vec<OcrLine>, Stats), String> {
    let (w, h) = page.render_dimensions();
    let scale = (ctx.opts.max_image_px as f32 / w.max(h).max(1.0)).min(MAX_RENDER_SCALE);
    let pixmap = render_page(cache, settings, page, scale);
    let (pw, ph) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let gray: Vec<u8> = pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
        })
        .collect();
    ctx.ocr_page_scored(&Pix::gray(pw, ph, gray))
}

/// Renders the page for the remote OCR backend: colour, at the backend's
/// resolution, as a JPEG.
pub fn remote_jpeg<'a>(
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    page: &'a Page<'a>,
) -> Result<Vec<u8>, String> {
    let (w, h) = page.render_dimensions();
    let scale = REMOTE_SCALE.min(REMOTE_MAX_PX as f32 / w.max(h).max(1.0));
    let pixmap = render_page(cache, settings, page, scale);
    let rgb: Vec<u8> = pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Pix::rgb(u32::from(pixmap.width()), u32::from(pixmap.height()), rgb).jpeg(JPEG_QUALITY)
}

/// A line is a heading candidate on OCR output only when it is this many times
/// taller than the median line. Line boxes vary with ascenders and descenders,
/// so a smaller difference says nothing about the font size.
const OCR_HEADING_RATIO: f32 = 1.25;
/// A heading candidate is a short line, never a wrapped paragraph.
const OCR_HEADING_WORDS: usize = 12;
/// A heading stands alone: the nearest line above and below it (where they
/// overlap it horizontally) is at least this many median line heights away.
const OCR_HEADING_ISOLATION: f32 = 0.6;

/// OCR lines as layout spans. Every line gets the median size, which keeps the
/// paragraph logic steady, except a short, isolated line clearly taller than the
/// median: it keeps its own size and may become a heading. Anything else that is
/// only a little taller is a box that grew with ascenders, not a heading.
pub fn ocr_items(lines: &[crate::ocr::OcrLine]) -> Vec<Item> {
    let height = |l: &crate::ocr::OcrLine| (l.bottom - l.top).max(1.0);
    let mut heights: Vec<f32> = lines.iter().map(height).collect();
    heights.sort_by(f32::total_cmp);
    let median = heights.get(heights.len() / 2).copied().unwrap_or(1.0);
    let isolated = |l: &crate::ocr::OcrLine| {
        let beside = |o: &&crate::ocr::OcrLine| o.left < l.right && o.right > l.left;
        let above = lines
            .iter()
            .filter(beside)
            .filter(|o| o.bottom <= l.top + 1.0 && !std::ptr::eq(*o, l))
            .map(|o| l.top - o.bottom)
            .fold(f32::MAX, f32::min);
        let below = lines
            .iter()
            .filter(beside)
            .filter(|o| o.top >= l.bottom - 1.0 && !std::ptr::eq(*o, l))
            .map(|o| o.top - l.bottom)
            .fold(f32::MAX, f32::min);
        above.min(below) >= OCR_HEADING_ISOLATION * median
    };
    lines
        .iter()
        .map(|l| {
            let own = height(l);
            let heading = own >= OCR_HEADING_RATIO * median
                && l.text.split_whitespace().count() <= OCR_HEADING_WORDS
                && isolated(l);
            Item::Text(Span {
                text: l.text.clone(),
                x0: l.left,
                x1: l.right,
                top: l.top,
                bottom: l.bottom,
                size: if heading { own } else { median } * 0.8,
            })
        })
        .collect()
}
