//! Per-call state: options, the lazily started OCR engine, the output budget
//! and the image parts, plus the figure pipeline every converter shares.
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::input::{OcrMode, Options};
use crate::model::{Block, Budget, DocAcc, Document, Figure, Part, json_len};
use crate::ocr::{Ocr, OcrLine};
use crate::pix::{Pix, probe};
use crate::resume::Resume;

/// Why OCR did not start: one sentence that also says how to continue.
pub const NO_TIME: &str =
    "the OCR time budget of this call ran out; call again with pages or files listing what remains";

/// The call's wall clock: whether another piece of work, expected to take as
/// long as a multiple of the slowest so far, still finishes inside the limit.
pub struct Clock {
    started: Instant,
    limit: Duration,
    slowest: Duration,
}

/// The next piece of work is assumed to take up to this many times the slowest so far.
const SLOWEST_FACTOR: f64 = 1.5;

impl Clock {
    pub fn new(started: Instant, limit: Duration) -> Self {
        Self {
            started,
            limit,
            slowest: Duration::ZERO,
        }
    }

    pub fn fits(&self) -> bool {
        self.started.elapsed() + self.slowest.mul_f64(SLOWEST_FACTOR) < self.limit
    }

    pub fn record(&mut self, took: Duration) {
        self.slowest = self.slowest.max(took);
    }
}

/// A 64-bit hash of the encoded bytes plus their length: equal images share it
/// (not cryptographic: a collision would only skip one image as a repeat).
fn image_key(bytes: &[u8]) -> u128 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    (u128::from(h.finish()) << 64) | bytes.len() as u128
}

/// What a figure record adds to the output, in JSON bytes.
fn figure_cost(fig: &Figure) -> usize {
    json_len(&fig.text) + json_len(&fig.caption) + 160
}

/// Figure images already at most this large pass through without re-encoding.
pub const PASSTHROUGH_BYTES: usize = 1_500_000;
const PART_JPEG_QUALITY: u8 = 85;

pub struct Ctx {
    pub opts: Options,
    pub ocr: Ocr,
    pub budget: Budget,
    pub parts: Vec<Part>,
    /// The wall clock of the call, checked between units.
    pub clock: Clock,
    /// Units delivered in this call, across documents.
    pub emitted: usize,
    /// Set when a figure was left unread because it must wait for the next call.
    pub waiting: bool,
}

/// The state of a document and the call when a unit began, so a unit that does
/// not fit can be taken back out and left for the next call.
pub struct Mark {
    md: usize,
    figs: usize,
    warns: usize,
    suppressed: usize,
    pub next_fig: u32,
    small: u32,
    repeated: u32,
    seen: usize,
    parts: usize,
    used: usize,
}

/// What became of a unit offered to [`Ctx::emit`].
pub enum Emit {
    Added,
    /// It did not fit and was taken back out: the next call starts at it.
    Deferred,
    /// It was the first content of the call and larger than the whole limit: the
    /// start of it was delivered up to `skip` bytes, with `joint` line breaks (0 to 2) before the rest.
    Cut {
        skip: u64,
        joint: u8,
    },
}

/// The encoded original of a figure, when one exists, for zero-copy attachment.
pub struct Original<'a> {
    pub bytes: &'a [u8],
    pub mime: &'static str,
}

impl Ctx {
    #[cfg(test)]
    pub fn new(opts: Options) -> Self {
        Self::started(opts, Instant::now())
    }

    /// A call whose wall clock began at `started`.
    pub fn started(opts: Options, started: Instant) -> Self {
        let limit = Duration::from_secs(opts.max_seconds);
        Self {
            ocr: Ocr::new(started, limit),
            budget: Budget::new(opts.max_bytes),
            parts: Vec::new(),
            clock: Clock::new(started, limit),
            emitted: 0,
            waiting: false,
            opts,
        }
    }

    pub fn mark(&self, acc: &DocAcc) -> Mark {
        Mark {
            md: acc.md.len(),
            figs: acc.figures.len(),
            warns: acc.warnings.len(),
            suppressed: acc.suppressed,
            next_fig: acc.next_fig,
            small: acc.small_skipped,
            repeated: acc.repeated_skipped,
            seen: acc.seen_order.len(),
            parts: self.parts.len(),
            used: self.budget.used(),
        }
    }

    /// The JSON bytes a table may use: what is left, less room for the marker
    /// and notes around it (an eighth of a small budget, 4 KiB of a large one).
    pub fn table_budget(&self) -> usize {
        let left = self.budget.remaining();
        left.saturating_sub((left / 8).min(4096))
    }

    /// Takes everything a unit added since `mark` back out.
    pub fn rewind(&mut self, acc: &mut DocAcc, mark: &Mark) {
        acc.md.truncate(mark.md);
        acc.figures.truncate(mark.figs);
        acc.warnings.truncate(mark.warns);
        acc.suppressed = mark.suppressed;
        acc.next_fig = mark.next_fig;
        acc.small_skipped = mark.small;
        acc.repeated_skipped = mark.repeated;
        for k in acc.seen_order.drain(mark.seen..) {
            acc.seen_images.remove(&k);
        }
        acc.alts.retain(|id, _| {
            id.strip_prefix("fig-")
                .and_then(|n| n.parse::<u32>().ok())
                .is_some_and(|n| n <= mark.next_fig)
        });
        acc.truncated = false;
        self.parts.truncate(mark.parts);
        self.budget
            .refund(self.budget.used().saturating_sub(mark.used));
    }

    /// The continuation that starts the unit `mark` was taken before.
    pub fn resume_at(&self, acc: &DocAcc, mark: &Mark, unit: u32, pos: u64) -> Resume {
        Resume {
            unit,
            pos,
            skip: 0,
            joint: 2,
            fig: mark.next_fig,
            seen: acc.seen_order[..mark.seen]
                .iter()
                .map(|k| format!("{k:032x}"))
                .collect(),
            st: None,
        }
    }

    /// The continuation that starts after everything the document holds now.
    pub fn resume_now(&self, acc: &DocAcc, unit: u32, pos: u64) -> Resume {
        let mark = self.mark(acc);
        self.resume_at(acc, &mark, unit, pos)
    }

    /// Whether a unit that needs OCR must wait for the next call: the wall clock
    /// has no room for it, and this call already delivered something.
    pub fn must_wait(&self) -> bool {
        self.emitted > 0 && self.opts.ocr != OcrMode::Never && !self.ocr.has_time()
    }

    /// Offers one finished unit of Markdown. `skip` is how much of it an earlier
    /// call already delivered. A unit that does not fit is left for the next
    /// call, unless it is the first content of this one, which is cut at a line.
    pub fn emit(&mut self, acc: &mut DocAcc, mark: &Mark, chunk: &str, skip: u64) -> Emit {
        let after = chunk.get(skip as usize..).unwrap_or("");
        let rest = after.trim_start_matches('\n');
        let lead = chunk.len() - rest.len();
        if rest.is_empty() || self.append(acc, rest) {
            self.emitted += 1;
            return Emit::Added;
        }
        if self.emitted > 0 {
            self.rewind(acc, mark);
            return Emit::Deferred;
        }
        // The refused attempt above marked the document full; the prefix gets its own try.
        acc.truncated = false;
        let kept = self.append_prefix(acc, rest);
        self.emitted += 1;
        let at = lead + kept;
        let head_nl = chunk[..at].len() - chunk[..at].trim_end_matches('\n').len();
        let tail_nl = chunk[at..].len() - chunk[at..].trim_start_matches('\n').len();
        let joint = (head_nl + tail_nl).min(2) as u8;
        Emit::Cut {
            skip: at as u64,
            joint,
        }
    }

    /// Builds the finished document of a call: warnings made final, and, for a
    /// document that continues an earlier call or stops early, only the figures
    /// that appear in the delivered text.
    #[allow(clippy::too_many_arguments)] // each converter hands over the same eight facts
    pub fn document(
        &mut self,
        mut acc: DocAcc,
        source: &str,
        format: &'static str,
        pages: u32,
        from: Option<&Resume>,
        next: Option<Resume>,
        parts_from: usize,
    ) -> Document {
        let warnings = acc.finish_warnings();
        if from.is_some_and(|r| r.skip > 0) || next.as_ref().is_some_and(|r| r.skip > 0) {
            self.retain_visible(&mut acc, parts_from);
        }
        Document {
            source: source.to_string(),
            format,
            pages,
            markdown: acc.md,
            figures: acc.figures,
            warnings,
            joint: from.map(Resume::joint_name),
            table_header: None,
            next,
        }
    }

    /// Drops the figures (and their attached images) whose block is not in the
    /// delivered part of a unit that was cut, refunding what they were charged.
    fn retain_visible(&mut self, acc: &mut DocAcc, parts_from: usize) {
        let mut kept = Vec::new();
        let mut gone_parts = Vec::new();
        for fig in std::mem::take(&mut acc.figures) {
            if acc.md.contains(&format!("[Figure {}]", fig.id)) {
                kept.push(fig);
                continue;
            }
            self.budget.refund(figure_cost(&fig));
            if let Some(p) = fig.part {
                gone_parts.push(p);
            }
        }
        if !gone_parts.is_empty() {
            let mut remap = Vec::new();
            let mut parts = Vec::new();
            for (i, part) in std::mem::take(&mut self.parts).into_iter().enumerate() {
                if i >= parts_from && gone_parts.contains(&i) {
                    self.budget.refund(part.data.len() + 64);
                    remap.push(None);
                } else {
                    remap.push(Some(parts.len()));
                    parts.push(part);
                }
            }
            self.parts = parts;
            for fig in &mut kept {
                fig.part = fig.part.and_then(|p| remap.get(p).copied().flatten());
            }
        }
        acc.figures = kept;
    }

    /// Appends one chunk of Markdown to a document if the output budget allows.
    pub fn append(&mut self, acc: &mut DocAcc, chunk: &str) -> bool {
        if chunk.is_empty() {
            return true;
        }
        if acc.truncated || !self.budget.charge(json_len(chunk) + 2) {
            acc.truncated = true;
            return false;
        }
        if !acc.md.is_empty() {
            acc.md.push_str("\n\n");
        }
        acc.md.push_str(chunk);
        true
    }

    /// Appends `text`, or as much of its start as the output budget allows,
    /// cut at a line end (at a character boundary when one line alone is too
    /// long). Returns the bytes of `text` that were kept.
    pub fn append_prefix(&mut self, acc: &mut DocAcc, text: &str) -> usize {
        // A chunk costs its JSON size plus the two bytes of the blank-line joiner.
        let room = self.budget.remaining().saturating_sub(2 + 2);
        let kept = if text.len() <= room && json_len(text) - 2 <= room {
            text.len()
        } else {
            let mut used = 0usize;
            let mut end = 0usize;
            let mut at = 0usize;
            'lines: for line in text.split_inclusive('\n') {
                let cost = json_len(line) - 2;
                if used + cost <= room {
                    used += cost;
                    at += line.len();
                    end = at;
                    continue;
                }
                if end == 0 {
                    for (i, c) in line.char_indices() {
                        let c_cost = json_len(&line[i..i + c.len_utf8()]) - 2;
                        if used + c_cost > room {
                            break 'lines;
                        }
                        used += c_cost;
                        end = i + c.len_utf8();
                    }
                }
                break;
            }
            end
        };
        let head = text[..kept].trim_end_matches('\n');
        if !head.is_empty() && !self.append(acc, head) {
            return 0;
        }
        if kept < text.len() {
            acc.truncated = true;
        }
        kept
    }

    /// Records a finished figure in the document and returns its Markdown block.
    pub fn keep(&mut self, acc: &mut DocAcc, mut fig: Figure) -> Option<Block> {
        if crate::md::is_file_name(&fig.caption) {
            fig.caption.clear();
        }
        if !self.budget.charge(figure_cost(&fig)) {
            acc.truncated = true;
            return None;
        }
        acc.figures.push(fig.clone());
        Some(Block::Figure(Box::new(fig)))
    }

    /// Remembers the alt text of a just-made figure, used as its caption only
    /// when no caption paragraph is found next to it (see `md::attach_captions`).
    pub fn note_alt(acc: &mut DocAcc, block: Option<&Block>, alt: &str) {
        if let Some(Block::Figure(f)) = block
            && !alt.trim().is_empty()
        {
            acc.alts.insert(f.id.clone(), alt.trim().to_string());
        }
    }

    fn next_id(acc: &mut DocAcc) -> String {
        acc.next_fig += 1;
        format!("fig-{}", acc.next_fig)
    }

    /// A figure the plugin cannot decode (vector or legacy format): listed with
    /// its caption so the model knows it exists, never silently dropped.
    pub fn unreadable_figure(
        &mut self,
        acc: &mut DocAcc,
        unit: Option<u32>,
        caption: String,
        why: &str,
    ) -> Option<Block> {
        let fig = Figure {
            id: Self::next_id(acc),
            page: unit,
            caption,
            note: format!("(image not read: {why})"),
            ..Figure::default()
        };
        self.keep(acc, fig)
    }

    /// Builds a figure from encoded image bytes (EPUB, Office, HTML and image files).
    pub fn figure_from_bytes(
        &mut self,
        acc: &mut DocAcc,
        unit: Option<u32>,
        bytes: &[u8],
        caption: String,
    ) -> Option<Block> {
        let Some((w, h, format)) = probe(bytes) else {
            return self.unreadable_figure(
                acc,
                unit,
                caption,
                "format is not PNG, JPEG, GIF, BMP, TIFF or WebP",
            );
        };
        if w.min(h) < self.opts.min_figure_px {
            acc.small_skipped += 1;
            return None;
        }
        let mime = match format {
            image::ImageFormat::Png => Some("image/png"),
            image::ImageFormat::Jpeg => Some("image/jpeg"),
            image::ImageFormat::Gif => Some("image/gif"),
            image::ImageFormat::WebP => Some("image/webp"),
            _ => None,
        };
        if !acc.see(image_key(bytes)) {
            acc.repeated_skipped += 1;
            return None;
        }
        let pix = match Pix::decode(bytes).and_then(|p| p.fit(self.opts.max_image_px)) {
            Ok(p) => p,
            Err(why) => return self.unreadable_figure(acc, unit, caption, &why),
        };
        let original = mime.map(|mime| Original { bytes, mime });
        self.figure_from_pix(acc, unit, &pix, (w, h), original, caption)
    }

    /// Builds a figure from decoded pixels: OCR, then the optional attached image.
    pub fn figure_from_pix(
        &mut self,
        acc: &mut DocAcc,
        unit: Option<u32>,
        pix: &Pix,
        source: (u32, u32),
        original: Option<Original<'_>>,
        caption: String,
    ) -> Option<Block> {
        let id = Self::next_id(acc);
        let (width, height) = source;
        let mut fig = Figure {
            id,
            page: unit,
            caption,
            width,
            height,
            ..Figure::default()
        };
        match self.opts.ocr {
            OcrMode::Never => fig.note = "(text not read: ocr is never)".into(),
            _ if !self.ocr.has_time() => {
                fig.note = "(text not read: the OCR time budget of this call ran out)".into();
                acc.warn(format!("{}: text not read because the OCR time budget ran out; request fewer pages or files", fig.id));
            }
            _ => match self.ocr.read(pix) {
                Ok(lines) => {
                    fig.text = lines
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    fig.ocr = true;
                }
                Err(why) => {
                    fig.note = "(text not read: OCR failed)".into();
                    acc.warn(format!("{}: OCR failed: {why}", fig.id));
                }
            },
        }
        if self.opts.figure_images {
            fig.part = self.attach(acc, &fig.id, pix, source, original);
        }
        self.keep(acc, fig)
    }

    pub fn attach(
        &mut self,
        acc: &mut DocAcc,
        id: &str,
        pix: &Pix,
        source: (u32, u32),
        original: Option<Original<'_>>,
    ) -> Option<usize> {
        let (data, mime): (Vec<u8>, &'static str) = match original {
            Some(o)
                if source.0.max(source.1) <= self.opts.max_image_px
                    && o.bytes.len() <= PASSTHROUGH_BYTES =>
            {
                (o.bytes.to_vec(), o.mime)
            }
            _ => match pix.jpeg(PART_JPEG_QUALITY) {
                Ok(jpeg) => (jpeg, "image/jpeg"),
                Err(why) => {
                    acc.warn(format!("{id}: image not attached: {why}"));
                    return None;
                }
            },
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        if !self.budget.charge(encoded.len() + 64) {
            acc.warn(format!(
                "{id}: image not attached because the output size limit was reached"
            ));
            return None;
        }
        self.parts.push(Part {
            kind: "image",
            data: encoded,
            mime,
        });
        Some(self.parts.len() - 1)
    }

    /// OCR of a whole page or image, honouring the mode and the time budget.
    pub fn ocr_page(&mut self, pix: &Pix) -> Result<Vec<OcrLine>, String> {
        if !self.ocr.has_time() {
            return Err(NO_TIME.into());
        }
        self.ocr.read(pix)
    }
}
