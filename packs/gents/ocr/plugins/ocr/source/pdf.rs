//! PDF conversion. Born-digital pages are read from their text layer (glyph
//! positions through the `hayro` interpreter) and laid out in reading order;
//! pages without a usable text layer are rendered and read by OCR.
//! Embedded raster images become figures.
use std::collections::HashSet;

use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};
use hayro_interpret::font::Glyph;
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, GlyphDrawMode, Image, ImageData,
    InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use hayro_syntax::page::Page;
use hayro_syntax::{LoadPdfError, Pdf};
use kurbo::{Affine, BezPath, Point, Rect};

use crate::ctx::Ctx;
use crate::input::OcrMode;
use crate::layout::{FigBox, Item, Out, Span, layout, take_captions};
use crate::model::{DocAcc, Document};
use crate::pix::{MAX_DECODE_PIXELS, Pix};

/// A page image at least this share of the page area is a scan background, not a figure.
const BACKGROUND_SHARE: f32 = 0.85;
/// Scan-like pages with fewer readable characters than this are OCR'd despite a text layer.
const STAMP_CHARS: usize = 25;
const MAX_FIGURES_PER_PAGE: usize = 32;
/// Decoded figure images of one page are held up to this many bytes; further
/// ones are skipped and reported, so a page of huge images stays inside memory.
const MAX_PAGE_IMAGE_BYTES: usize = 256 * 1024 * 1024;
/// A gap wider than this many em starts a new span: a word gap, even a stretched
/// one, is narrower, while the gap between table cells or columns is wider, and
/// the layout needs the cells apart to see the table.
const WORD_GAP_MAX: f32 = 0.8;
/// The same after a list marker or a section number.
const MARKER_GAP_MAX: f32 = 2.5;
/// The first selected pages sampled to find running headers and footers.
const FURNITURE_SAMPLE: usize = 8;
/// Share of the page height, at the top and at the bottom, where they can sit.
const EDGE_BAND: f32 = 0.08;
/// Pages are rendered at most this many times larger than their point size for OCR.
const MAX_RENDER_SCALE: f32 = 4.0;

struct Cur {
    text: String,
    x0: f32,
    x1: f32,
    last_x: f32,
    adv_known: bool,
    base: f32,
    size: f32,
    /// No earlier span sits on this baseline.
    line_start: bool,
}

/// Running headers and footers: short lines repeated near the page edges,
/// compared with digits folded so page numbers match.
#[derive(Default)]
struct Furniture {
    top: HashSet<String>,
    bottom: HashSet<String>,
    removed: usize,
}

fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.trim().chars() {
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Which edge band a span sits in: Some(true) top, Some(false) bottom.
fn edge(s: &Span, page_h: f32) -> Option<bool> {
    let cy = (s.top + s.bottom) / 2.0;
    if cy < EDGE_BAND * page_h {
        Some(true)
    } else if cy > (1.0 - EDGE_BAND) * page_h {
        Some(false)
    } else {
        None
    }
}

impl Furniture {
    /// Keeps the lines that appear in the edge bands of most sampled pages.
    fn learn(samples: &[Vec<(bool, String)>]) -> Self {
        let mut counts: std::collections::HashMap<(bool, &str), usize> =
            std::collections::HashMap::new();
        for page in samples {
            let distinct: HashSet<(bool, &str)> =
                page.iter().map(|(top, t)| (*top, t.as_str())).collect();
            for key in distinct {
                *counts.entry(key).or_default() += 1;
            }
        }
        let need = (samples.len() * 6 / 10).max(2);
        let mut f = Self::default();
        for ((top, text), n) in counts {
            if n >= need && !text.is_empty() {
                if top { &mut f.top } else { &mut f.bottom }.insert(text.to_string());
            }
        }
        f
    }

    fn drop_from(&mut self, spans: &mut Vec<Span>, page_h: f32) {
        if self.top.is_empty() && self.bottom.is_empty() {
            return;
        }
        let before = spans.len();
        spans.retain(|s| match edge(s, page_h) {
            Some(true) => !self.top.contains(&fold(&s.text)),
            Some(false) => !self.bottom.contains(&fold(&s.text)),
            None => true,
        });
        self.removed += before - spans.len();
    }
}

struct PageImage {
    pix: Pix,
    source: (u32, u32),
    bbox: (f32, f32, f32, f32),
}

struct Collector {
    w: f32,
    h: f32,
    spans: Vec<Span>,
    cur: Option<Cur>,
    recent: Vec<(String, f32, f32)>,
    mapped: usize,
    unmapped: usize,
    rotated: usize,
    paths: usize,
    any_image: bool,
    background: bool,
    small: u32,
    repeated: u32,
    images: Vec<PageImage>,
    /// Bytes of the decoded figure images gathered for this page so far.
    image_bytes: usize,
    over_budget: u32,
    /// The size of the first image over the decode limit on this page.
    oversized: Option<(u32, u32)>,
    seen: HashSet<u128>,
    min_px: u32,
    max_px: u32,
    want_images: bool,
}

impl Collector {
    fn flush(&mut self) {
        if let Some(c) = self.cur.take() {
            let text = c.text.trim().to_string();
            if !text.is_empty() {
                self.spans.push(Span {
                    text,
                    x0: c.x0,
                    x1: c.x1,
                    top: c.base - 0.8 * c.size,
                    bottom: c.base + 0.2 * c.size,
                    size: c.size,
                });
            }
        }
    }

    fn glyph(&mut self, s: &str, x: f32, base: f32, adv: Option<f32>, size: f32) {
        // Literal spaces are ignored: producers often draw them with no width
        // and justify lines by moving glyphs, so only the geometry tells a
        // word gap from a letter gap.
        if s.trim().is_empty()
            || self.recent.iter().any(|(t, rx, ry)| {
                t == s && (rx - x).abs() < 0.2 * size && (ry - base).abs() < 0.2 * size
            })
        {
            return;
        }
        if self.recent.len() >= 16 {
            self.recent.remove(0);
        }
        self.recent.push((s.to_string(), x, base));
        let end = x + adv.unwrap_or(0.5 * size);
        if let Some(c) = &mut self.cur {
            let gap = if c.adv_known {
                x - c.x1
            } else {
                (x - c.last_x) - 0.6 * size
            };
            let same_line = (base - c.base).abs() <= 0.45 * c.size.max(size);
            if same_line
                && gap >= -0.5 * size
                && gap <= gap_max(&c.text, c.line_start) * size.max(c.size)
            {
                if gap > 0.15 * size && !c.text.ends_with(' ') && s != " " {
                    c.text.push(' ');
                }
                if !(s == " " && c.text.ends_with(' ')) {
                    c.text.push_str(s);
                }
                c.x1 = end;
                c.last_x = x;
                c.adv_known = adv.is_some();
                c.size = c.size.max(size);
                return;
            }
        }
        self.flush();
        let line_start = !self
            .spans
            .iter()
            .rev()
            .take(6)
            .any(|p| ((p.bottom - 0.2 * p.size) - base).abs() <= 0.45 * size);
        self.cur = Some(Cur {
            line_start,
            text: s.to_string(),
            x0: x,
            x1: end,
            last_x: x,
            adv_known: adv.is_some(),
            base,
            size,
        });
    }
}

impl<'a> Device<'a> for Collector {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.paths += 1;
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        let full = transform * glyph_transform;
        let origin = full * Point::new(0.0, 0.0);
        // One em in glyph space is 1000 units, so this vector is the font size in device pixels.
        let em = full * Point::new(1000.0, 0.0) - origin;
        let size = em.hypot() as f32;
        let (x, base) = (origin.x as f32, origin.y as f32);
        let Some(text) = glyph.as_unicode() else {
            self.unmapped += 1;
            return;
        };
        let text = match text {
            BfString::Char(c) => c.to_string(),
            BfString::String(s) => s,
        };
        let text = expand_ligatures(
            &text
                .chars()
                .filter(|c| *c != '\u{ad}' && (!c.is_control() || c.is_whitespace()))
                .map(|c| if c.is_whitespace() { ' ' } else { c })
                .collect::<String>(),
        );
        if text.is_empty()
            || size < 0.5
            || x < -size
            || base < -size
            || x > self.w + size
            || base > self.h + size
        {
            return;
        }
        if em.y.abs() > 0.3 * em.x.abs() || em.x < 0.0 {
            self.rotated += text.chars().filter(|c| !c.is_whitespace()).count();
            return;
        }
        let adv = match glyph {
            Glyph::Outline(g) => g.advance_width().map(|a| a / 1000.0 * size),
            Glyph::Type3(_) => None,
        };
        self.mapped += text.chars().filter(|c| !c.is_whitespace()).count();
        self.glyph(&text, x, base, adv, size);
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        self.any_image = true;
        // The declared size is read before any pixel is decoded: a crafted
        // file can declare a huge image in a few bytes.
        if u64::from(image.width()) * u64::from(image.height()) > MAX_DECODE_PIXELS {
            self.oversized
                .get_or_insert((image.width(), image.height()));
            return;
        }
        if !self.want_images {
            return;
        }
        // The transform maps the image's own pixel grid onto the page.
        let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
        let corners =
            [(0.0, 0.0), (iw, 0.0), (0.0, ih), (iw, ih)].map(|(u, v)| transform * Point::new(u, v));
        let x0 = corners.iter().map(|p| p.x as f32).fold(f32::MAX, f32::min);
        let x1 = corners.iter().map(|p| p.x as f32).fold(f32::MIN, f32::max);
        let top = corners.iter().map(|p| p.y as f32).fold(f32::MAX, f32::min);
        let bottom = corners.iter().map(|p| p.y as f32).fold(f32::MIN, f32::max);
        let share = ((x1.min(self.w) - x0.max(0.0)).max(0.0)
            * (bottom.min(self.h) - top.max(0.0)).max(0.0))
            / (self.w * self.h).max(1.0);
        if share >= BACKGROUND_SHARE {
            self.background = true;
            return;
        }
        let Image::Raster(raster) = image else { return };
        if raster.width().min(raster.height()) < self.min_px {
            self.small += 1;
            return;
        }
        if self.images.len() >= MAX_FIGURES_PER_PAGE || !self.seen.insert(raster.cache_key()) {
            self.repeated += 1;
            return;
        }
        if self.image_bytes >= MAX_PAGE_IMAGE_BYTES {
            self.over_budget += 1;
            return;
        }
        let source = (raster.width(), raster.height());
        let mut pix = None;
        raster.with_rgba(
            |data, alpha| {
                pix = Some(match data {
                    ImageData::Rgb(d) => {
                        let (w, h) = (d.width, d.height);
                        Pix::rgb(
                            w,
                            h,
                            flatten(
                                d.data,
                                3,
                                alpha
                                    .as_ref()
                                    .filter(|a| a.width == w && a.height == h)
                                    .map(|a| a.data.as_slice()),
                            ),
                        )
                    }
                    ImageData::Luma(d) => {
                        let (w, h) = (d.width, d.height);
                        Pix::gray(
                            w,
                            h,
                            flatten(
                                d.data,
                                1,
                                alpha
                                    .as_ref()
                                    .filter(|a| a.width == w && a.height == h)
                                    .map(|a| a.data.as_slice()),
                            ),
                        )
                    }
                });
            },
            Some((self.max_px, self.max_px)),
        );
        if let Some(pix) = pix.and_then(|p| p.fit(self.max_px).ok()) {
            self.image_bytes += pix.data.len();
            self.images.push(PageImage {
                pix,
                source,
                bbox: (x0, x1, top, bottom),
            });
        }
    }
}

/// The widest gap, in em, that still continues the span so far. A list bullet
/// or a section number at the start of a line is often followed by a wide tab,
/// and that is not a cell gap; a number inside a line is a cell and stays apart.
fn gap_max(so_far: &str, line_start: bool) -> f32 {
    const BULLETS: &str =
        "\u{2022}\u{25aa}\u{25e6}\u{2023}\u{25cf}\u{25cb}\u{25a0}\u{25a1}\u{2013}\u{2014}\u{b7}*-";
    let number = so_far.trim_end_matches(['.', ')']);
    let numbering = !number.is_empty()
        && number.len() <= 8
        && number.chars().all(|c| c.is_ascii_digit() || c == '.')
        && number.chars().next().is_some_and(|c| c.is_ascii_digit())
        && (line_start || number.contains('.'));
    let bullet =
        line_start && so_far.chars().count() == 1 && so_far.chars().all(|c| BULLETS.contains(c));
    if numbering || bullet {
        MARKER_GAP_MAX
    } else {
        WORD_GAP_MAX
    }
}

/// Replaces the typographic ligature characters fonts map to with their letters.
fn expand_ligatures(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            c => out.push(c),
        }
    }
    out
}

/// Composites interleaved channels over white when the image has a soft mask.
fn flatten(mut data: Vec<u8>, channels: usize, alpha: Option<&[u8]>) -> Vec<u8> {
    if let Some(alpha) = alpha {
        for (px, &a) in data.chunks_exact_mut(channels).zip(alpha) {
            for v in px {
                *v = ((u32::from(*v) * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8;
            }
        }
    }
    data
}

struct State<'a> {
    interp: InterpreterCache<'a>,
    render: RenderCache<'a>,
    settings: InterpreterSettings,
    seen: HashSet<u128>,
    furniture: Furniture,
}

pub fn convert(ctx: &mut Ctx, source: &str, data: Vec<u8>) -> Result<Document, String> {
    let pdf = Pdf::new(data).map_err(|e| match e {
        LoadPdfError::Decryption(_) => {
            "the PDF is password-protected and cannot be opened without the password".to_string()
        }
        LoadPdfError::Invalid => "the PDF is corrupt or truncated and cannot be read".to_string(),
    })?;
    let pages = pdf.pages();
    let total = pages.len() as u32;
    if total == 0 {
        return Err("the PDF has no pages".into());
    }
    let mut acc = DocAcc::default();
    let mut state = State {
        interp: InterpreterCache::new(),
        render: RenderCache::new(),
        settings: InterpreterSettings::default(),
        seen: HashSet::new(),
        furniture: Furniture::default(),
    };
    ctx.append(&mut acc, &crate::detect::header(source, "pdf"));
    let picks: Vec<u32> = (1..=total).filter(|&n| ctx.opts.selected(n)).collect();
    if picks.len() >= 3 {
        let mut samples = Vec::new();
        for &n in picks.iter().take(FURNITURE_SAMPLE) {
            let p = &pages[(n - 1) as usize];
            let col = collect(
                &pdf,
                p,
                &mut state,
                ctx.opts.min_figure_px,
                ctx.opts.max_image_px,
                false,
            );
            if col.mapped >= 3 {
                samples.push(
                    col.spans
                        .iter()
                        .filter_map(|s| edge(s, col.h).map(|top| (top, fold(&s.text))))
                        .collect::<Vec<_>>(),
                );
            }
        }
        if samples.len() >= 3 {
            state.furniture = Furniture::learn(&samples);
        }
    }
    let mut selected = 0u32;
    for n in 1..=total {
        if !ctx.opts.selected(n) {
            continue;
        }
        selected += 1;
        let body = page(ctx, &mut acc, &pdf, &pages[(n - 1) as usize], n, &mut state);
        let chunk = if body.is_empty() {
            format!("<!-- page {n} -->")
        } else {
            format!("<!-- page {n} -->\n\n{body}")
        };
        if !ctx.append(&mut acc, &chunk) {
            acc.warn(format!("the output size limit was reached before page {n}; request pages=\"{n}-\" to continue"));
            break;
        }
    }
    if selected == 0 {
        acc.warn(format!(
            "pages selects nothing: the document has {total} page(s)"
        ));
    }
    if state.furniture.removed > 0 {
        acc.warn(format!("{} repeated header or footer line(s) near the page edges (running titles, page numbers) were left out", state.furniture.removed));
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format: "pdf",
        pages: total,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}

/// Runs the interpreter over one page and gathers its text, images and counts.
fn collect<'a>(
    pdf: &'a Pdf,
    page: &'a Page<'a>,
    st: &mut State<'a>,
    min_px: u32,
    max_px: u32,
    want_images: bool,
) -> Collector {
    let (w, h) = page.render_dimensions();
    let initial = page.initial_transform(true).to_kurbo();
    let mut context = Context::new(
        initial,
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        &st.interp,
        pdf.xref(),
        st.settings.clone(),
    );
    let mut col = Collector {
        w,
        h,
        spans: Vec::new(),
        cur: None,
        recent: Vec::new(),
        mapped: 0,
        unmapped: 0,
        rotated: 0,
        paths: 0,
        any_image: false,
        background: false,
        small: 0,
        repeated: 0,
        images: Vec::new(),
        image_bytes: 0,
        over_budget: 0,
        oversized: None,
        seen: std::mem::take(&mut st.seen),
        min_px,
        max_px,
        want_images,
    };
    interpret_page(page, &mut context, &mut col);
    col.flush();
    st.seen = std::mem::take(&mut col.seen);
    col
}

fn page<'a>(
    ctx: &mut Ctx,
    acc: &mut DocAcc,
    pdf: &'a Pdf,
    page: &'a Page<'a>,
    n: u32,
    st: &mut State<'a>,
) -> String {
    let h = page.render_dimensions().1;
    let mut col = collect(
        pdf,
        page,
        st,
        ctx.opts.min_figure_px,
        ctx.opts.max_image_px,
        true,
    );
    acc.small_skipped += col.small;
    acc.repeated_skipped += col.repeated;
    if let Some((w, h)) = col.oversized {
        acc.warn(format!("page {n}: an image of {w}x{h} pixels is over the {MAX_DECODE_PIXELS} pixel limit and was skipped"));
    }
    if col.over_budget > 0 {
        acc.warn(format!("page {n}: {} figure image(s) were skipped because the page's images passed the {} MiB memory cap", col.over_budget, MAX_PAGE_IMAGE_BYTES / 1024 / 1024));
    }

    let text_ok = col.mapped >= 3 && col.unmapped * 3 <= col.mapped;
    if text_ok {
        st.furniture.drop_from(&mut col.spans, h);
    }
    let visual = col.any_image || col.paths > 0;
    let want_ocr = match ctx.opts.ocr {
        OcrMode::Always => true,
        OcrMode::Never => false,
        OcrMode::Auto => (!text_ok && visual) || (col.background && col.mapped < STAMP_CHARS),
    };
    if !want_ocr && !text_ok && visual && ctx.opts.ocr == OcrMode::Never {
        acc.warn(format!(
            "page {n}: no usable text layer and ocr is never, so the page was not read"
        ));
    }
    if want_ocr {
        // Rendering decodes every image of the page, so a page holding an
        // oversized one is never rendered.
        let rendered = match col.oversized {
            Some(_) => Err("the page holds an image too large to render safely".to_string()),
            None => ocr_page(ctx, st, page),
        };
        match rendered {
            Ok(items) => return blocks_to_md(ctx, acc, n, Vec::new(), items),
            Err(why) if text_ok => acc.warn(format!(
                "page {n}: OCR was not possible ({why}); the text layer was used"
            )),
            Err(why) => acc.warn(format!(
                "page {n}: no readable text layer and OCR was not possible: {why}"
            )),
        }
    }
    if text_ok {
        if col.unmapped > 0 {
            acc.warn(format!(
                "page {n}: {} glyph(s) had no Unicode mapping and were left out",
                col.unmapped
            ));
        }
    } else if col.mapped > 0 || col.unmapped > 0 {
        acc.warn(format!("page {n}: the text layer is mostly unreadable ({} of {} glyphs had no Unicode mapping)", col.unmapped, col.unmapped + col.mapped));
    }
    if col.rotated > 0 {
        acc.warn(format!(
            "page {n}: {} rotated character(s) were not read; only horizontal text is extracted",
            col.rotated
        ));
    }
    let mut items: Vec<Item> = if text_ok {
        col.spans.into_iter().map(Item::Text).collect()
    } else {
        Vec::new()
    };
    let images = std::mem::take(&mut col.images);
    for (i, img) in images.iter().enumerate() {
        let (x0, x1, top, bottom) = img.bbox;
        items.push(Item::Fig(FigBox {
            x0,
            x1,
            top,
            bottom,
            index: i,
        }));
    }
    blocks_to_md(ctx, acc, n, images, items)
}

/// Lays out items and renders them, reading each figure image as it is reached.
fn blocks_to_md(
    ctx: &mut Ctx,
    acc: &mut DocAcc,
    n: u32,
    images: Vec<PageImage>,
    items: Vec<Item>,
) -> String {
    let mut out = layout(&items);
    let captions = take_captions(&mut out, images.len());
    let mut blocks = Vec::with_capacity(out.len());
    for o in out {
        match o {
            Out::Block(b) => blocks.push(b),
            Out::Fig(i) => {
                let img = &images[i];
                if let Some(b) = ctx.figure_from_pix(
                    acc,
                    Some(n),
                    &img.pix,
                    img.source,
                    None,
                    captions[i].clone(),
                ) {
                    blocks.push(b);
                }
            }
        }
    }
    crate::md::render(&blocks)
}

/// Renders the page and reads it with OCR, returning the lines as layout items.
fn ocr_page<'a>(ctx: &mut Ctx, st: &State<'a>, page: &'a Page<'a>) -> Result<Vec<Item>, String> {
    let (w, h) = page.render_dimensions();
    let scale = (ctx.opts.max_image_px as f32 / w.max(h).max(1.0)).min(MAX_RENDER_SCALE);
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
        ..RenderSettings::default()
    };
    let pixmap = render(page, &st.render, &st.settings, &settings);
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
    let lines = ctx.ocr_page(&Pix::gray(pw, ph, gray))?;
    Ok(ocr_items(&lines))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Options;
    use crate::pdfgen::{Img, PageSpec, draw, pdf, scan, text};

    fn run(data: Vec<u8>, opts: Options) -> (Document, Ctx) {
        let mut ctx = Ctx::new(opts);
        let doc = convert(&mut ctx, "t.pdf", data).unwrap();
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
            "BT /F1 12 Tf 0 1 -1 0 300 100 Tm (Sideways words on an upright page.) Tj ET\n"
                .to_string(),
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
        ctx.ocr = crate::ocr::Ocr::new(long_ago);
        let doc = convert(&mut ctx, "t.pdf", pdf(&[spec])).unwrap();
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
        assert_eq!(
            doc.warnings,
            vec!["page 1: an image of 40000x40000 pixels is over the 50000000 pixel limit and was skipped".to_string()]
        );
        // A scanned page of that size cannot be rendered for OCR: said, not attempted.
        let scan = PageSpec {
            w: 595.0,
            h: 842.0,
            content: draw("Im1", 0.0, 0.0, 595.0, 842.0),
            images: vec![("Im1", &huge)],
        };
        let (doc, _) = run(pdf(&[scan]), Options::default());
        assert_eq!(doc.warnings.len(), 2, "{:?}", doc.warnings);
        assert!(
            doc.warnings[1].contains("OCR was not possible: the page holds an image too large"),
            "{:?}",
            doc.warnings
        );
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
        let err = convert(&mut ctx, "x.pdf", b"%PDF-1.4 garbage".to_vec())
            .err()
            .unwrap();
        assert!(err.contains("corrupt or truncated"), "{err}");
    }
}
