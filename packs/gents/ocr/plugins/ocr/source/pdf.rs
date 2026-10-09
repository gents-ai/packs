//! PDF conversion. Born-digital pages are read from their text layer (glyph
//! positions through the `hayro` interpreter) and laid out in reading order;
//! pages without a usable text layer are rendered and read by OCR.
//! Embedded raster images become figures.
use std::collections::HashSet;

use hayro::RenderCache;
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
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::input::OcrMode;
use crate::layout::{FigBox, Item, Out, Span, layout, take_captions};
use crate::model::{DocAcc, Document};
use crate::pdflazy::Lazy;
use crate::pdfocr::{ocr_items, ocr_lines, remote_jpeg};
use crate::pix::Pix;

/// 600-dpi book scans can exceed 50 megapixels. A 64-megapixel RGBA decode
/// remains below the 256 MiB per-page image budget; rendering still scales to
/// max_image_px, and the plugin retains its 2048 MiB execution memory ceiling.
const MAX_DECODE_PIXELS: u64 = 64_000_000;
use crate::remote::{self, Read};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps, with_marker};
use crate::src::Src;

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
    /// `None` until the running headers and footers have been looked for.
    furniture: Option<Furniture>,
}

impl State<'_> {
    fn new(furniture: Option<Furniture>) -> Self {
        State {
            interp: InterpreterCache::new(),
            render: RenderCache::new(),
            settings: InterpreterSettings::default(),
            seen: HashSet::new(),
            furniture,
        }
    }
}

/// A PDF the lazy reader cannot index (a damaged cross-reference table,
/// encryption) is read whole up to this size, which lets `hayro` repair it.
const WHOLE_LIMIT: u64 = 256 * 1024 * 1024;

fn load_error(e: LoadPdfError) -> String {
    match e {
        LoadPdfError::Decryption(_) => {
            "the PDF is password-protected and cannot be opened without the password".to_string()
        }
        LoadPdfError::Invalid => "the PDF is corrupt or truncated and cannot be read".to_string(),
    }
}

/// How a PDF is read: windows of pages copied out of the file, or the whole file.
enum Backend {
    Lazy(Box<Lazy>),
    Whole(Pdf),
}

impl Backend {
    fn open(src: &mut Src) -> Result<Self, String> {
        let why = match Lazy::open(src.reopen()?) {
            Ok(lazy) => return Ok(Self::Lazy(Box::new(lazy))),
            Err(why) => why,
        };
        if src.len() > WHOLE_LIMIT {
            return Err(format!(
                "the PDF cannot be indexed ({why}) and is over the {} MiB that can be repaired; save a repaired copy with a PDF tool",
                WHOLE_LIMIT / 1024 / 1024
            ));
        }
        let data = src
            .head(src.len() as usize)
            .map_err(|e| format!("cannot read the file: {e}"))?;
        Pdf::new(data).map(Self::Whole).map_err(load_error)
    }

    fn total(&self) -> u32 {
        match self {
            Self::Lazy(l) => l.total,
            Self::Whole(p) => p.pages().len() as u32,
        }
    }
}

/// The number of pages of a PDF, without reading its pages.
pub fn page_count(src: &mut Src) -> Result<u32, String> {
    Ok(Backend::open(src)?.total())
}

/// What a call carries to the next: the running headers and footers.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    top: Vec<String>,
    bottom: Vec<String>,
}

impl Saved {
    fn of(f: &Furniture) -> Self {
        let sorted = |s: &HashSet<String>| {
            let mut v: Vec<String> = s.iter().cloned().collect();
            v.sort();
            v
        };
        Self {
            top: sorted(&f.top),
            bottom: sorted(&f.bottom),
        }
    }

    fn furniture(self) -> Furniture {
        Furniture {
            top: self.top.into_iter().collect(),
            bottom: self.bottom.into_iter().collect(),
            removed: 0,
        }
    }
}

/// The pages of one window as a stream of units.
struct Pages<'a> {
    ctx: &'a mut Ctx,
    acc: &'a mut DocAcc,
    pdf: &'a Pdf,
    /// Page number and index in the file opened, in reading order.
    entries: &'a [(u32, usize)],
    /// The page to continue with once these are done.
    after: u32,
    i: usize,
    st: State<'a>,
}

impl Steps for Pages<'_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.ctx, &mut *self.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: self.entries.get(self.i).map_or(self.after, |e| e.0),
            pos: 0,
            st: self
                .st
                .furniture
                .as_ref()
                .and_then(|f| serde_json::to_value(Saved::of(f)).ok()),
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        let Some(&(n, idx)) = self.entries.get(self.i) else {
            return Ok(None);
        };
        let pdf: &Pdf = self.pdf;
        let body = page(self.ctx, self.acc, pdf, &pdf.pages()[idx], n, &mut self.st);
        if self.ctx.waiting {
            return Ok(Some(Step::Wait));
        }
        self.i += 1;
        Ok(Some(Step::Chunk(with_marker(
            &format!("<!-- page {n} -->"),
            &body,
        ))))
    }
}

/// Learns the running headers and footers from the first pages of `entries`.
fn learn_furniture<'a>(ctx: &Ctx, pdf: &'a Pdf, entries: &[(u32, usize)], st: &mut State<'a>) {
    let mut samples = Vec::new();
    if entries.len() >= 3 {
        for &(_, idx) in entries.iter().take(FURNITURE_SAMPLE) {
            let col = collect(
                pdf,
                &pdf.pages()[idx],
                st,
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
    }
    st.furniture = Some(if samples.len() >= 3 {
        Furniture::learn(&samples)
    } else {
        Furniture::default()
    });
}

/// Reads the pages `entries` of `pdf` through the slicer. The furniture moves
/// in and out so the next window keeps what this one learned.
fn read_window(
    ctx: &mut Ctx,
    acc: &mut DocAcc,
    pdf: &Pdf,
    entries: &[(u32, usize)],
    after: u32,
    furniture: &mut Option<Furniture>,
    skip: &mut u64,
) -> Result<Option<Resume>, String> {
    let mut st = State::new(furniture.take());
    if st.furniture.is_none() {
        learn_furniture(ctx, pdf, entries, &mut st);
    }
    let mut reader = Pages {
        ctx,
        acc,
        pdf,
        entries,
        after,
        i: 0,
        st,
    };
    let next = slicer::run(&mut reader, std::mem::take(skip))?;
    *furniture = reader.st.furniture.take();
    Ok(next)
}

pub fn convert(
    ctx: &mut Ctx,
    source: &str,
    src: &mut Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let backend = Backend::open(src)?;
    let total = backend.total();
    if total == 0 {
        return Err("the PDF has no pages".into());
    }
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &crate::detect::header(source, "pdf"));
            if ctx.opts.selects_none(total) {
                acc.warn(format!(
                    "pages selects nothing: the document has {total} page(s)"
                ));
            }
        }
    }
    let mut furniture = resume
        .and_then(|r| r.st.clone())
        .and_then(|v| serde_json::from_value::<Saved>(v).ok())
        .map(Saved::furniture);
    let mut skip = resume.map_or(0, |r| r.skip);
    let mut from = resume.map_or(1, |r| r.unit.max(1));
    let mut removed = 0;
    let next = match backend {
        Backend::Whole(pdf) => {
            let entries: Vec<(u32, usize)> = (from..=total)
                .filter(|&n| ctx.opts.selected(n))
                .map(|n| (n, (n - 1) as usize))
                .collect();
            let next = read_window(
                ctx,
                &mut acc,
                &pdf,
                &entries,
                total + 1,
                &mut furniture,
                &mut skip,
            )?;
            removed = furniture.as_ref().map_or(0, |f| f.removed);
            next
        }
        Backend::Lazy(mut lazy) => loop {
            let min = if furniture.is_none() {
                FURNITURE_SAMPLE
            } else {
                1
            };
            let win = lazy.window(&ctx.opts, from, min)?;
            for w in win.warnings {
                acc.warn(w);
            }
            if win.numbers.is_empty() {
                break None;
            }
            let pdf = Pdf::new(win.bytes).map_err(load_error)?;
            let entries: Vec<(u32, usize)> = win.numbers.iter().copied().zip(0usize..).collect();
            let next = read_window(
                ctx,
                &mut acc,
                &pdf,
                &entries,
                win.after,
                &mut furniture,
                &mut skip,
            )?;
            removed += furniture
                .as_mut()
                .map_or(0, |f| std::mem::take(&mut f.removed));
            if next.is_some() || win.after > total {
                break next;
            }
            from = win.after;
        },
    };
    if let Some(w) = ctx.remote.take_read_warning() {
        acc.warn(w);
    }
    if removed > 0 {
        acc.warn(format!("{removed} repeated header or footer line(s) near the page edges (running titles, page numbers) were left out"));
    }
    Ok(ctx.document(acc, source, "pdf", total, resume, next, parts_from))
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
    if ctx.emitted > 0 && ctx.remote.stops_before(n, ctx.budget.remaining()) {
        ctx.waiting = true;
        return String::new();
    }
    let h = page.render_dimensions().1;
    st.seen.clone_from(&acc.seen_images);
    let mut col = collect(
        pdf,
        page,
        st,
        ctx.opts.min_figure_px,
        ctx.opts.max_image_px,
        true,
    );
    for key in st.seen.drain() {
        acc.see(key);
    }
    acc.small_skipped += col.small;
    acc.repeated_skipped += col.repeated;
    if let Some((w, h)) = col.oversized {
        acc.warn(format!("page {n}: an image of {w}x{h} pixels is over the {MAX_DECODE_PIXELS} pixel limit and was skipped"));
    }
    if col.over_budget > 0 {
        acc.warn(format!("page {n}: {} figure image(s) were skipped because the page's images passed the {} MiB memory cap", col.over_budget, MAX_PAGE_IMAGE_BYTES / 1024 / 1024));
    }

    let text_ok = col.mapped >= 3 && col.unmapped * 3 <= col.mapped;
    if text_ok && let Some(f) = st.furniture.as_mut() {
        f.drop_from(&mut col.spans, h);
    }
    let visual = col.any_image || col.paths > 0;
    let scanned = (!text_ok && visual) || (col.background && col.mapped < STAMP_CHARS);
    let want_ocr = match ctx.opts.ocr {
        OcrMode::Always => true,
        OcrMode::Never => false,
        OcrMode::Auto => scanned,
    };
    if !want_ocr && !text_ok && visual && ctx.opts.ocr == OcrMode::Never {
        acc.warn(format!(
            "page {n}: no usable text layer and ocr is never, so the page was not read"
        ));
    }
    if want_ocr && ctx.must_wait() {
        ctx.waiting = true;
        return String::new();
    }
    if want_ocr {
        // Only a page that is scanned (never one with a text layer) may go to the remote backend.
        if scanned
            && ctx.opts.ocr != OcrMode::Never
            && ctx.remote.active()
            && col.oversized.is_none()
        {
            let (cache, settings) = (&st.render, &st.settings);
            let mut builtin = |ctx: &mut Ctx| ocr_lines(ctx, cache, settings, page);
            let mut encode = || remote_jpeg(cache, settings, page);
            match remote::read_page(ctx, acc, n, visual, &mut builtin, &mut encode) {
                Read::Remote(md) => return md,
                Read::Lines(lines) => {
                    return blocks_to_md(ctx, acc, n, Vec::new(), ocr_items(&lines));
                }
                Read::Pending => return String::new(),
                Read::Wait => {
                    ctx.waiting = true;
                    return String::new();
                }
                Read::Builtin => {}
            }
        }
        // Rendering decodes every image of the page, so a page holding an
        // oversized one is never rendered.
        let rendered = match col.oversized {
            Some(_) => Err("the page holds an image too large to render safely".to_string()),
            None => ocr_lines(ctx, &st.render, &st.settings, page).map(|(l, _)| ocr_items(&l)),
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

#[cfg(test)]
#[path = "pdf_tests.rs"]
mod tests;
