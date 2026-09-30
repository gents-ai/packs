//! Per-call state: options, the lazily started OCR engine, the output budget
//! and the image parts, plus the figure pipeline every converter shares.
use std::time::Instant;

use base64::Engine as _;

use crate::input::{OcrMode, Options};
use crate::model::{Block, Budget, DocAcc, Figure, Part, json_len};
use crate::ocr::{Ocr, OcrLine};
use crate::pix::{Pix, probe};

/// Why OCR did not start: one sentence that also says how to continue.
pub const NO_TIME: &str =
    "the OCR time budget of this call ran out; call again with pages or files listing what remains";

/// A 64-bit hash of the encoded bytes plus their length: equal images share it
/// (not cryptographic: a collision would only skip one image as a repeat).
fn image_key(bytes: &[u8]) -> u128 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    (u128::from(h.finish()) << 64) | bytes.len() as u128
}

/// Figure images already at most this large pass through without re-encoding.
const PASSTHROUGH_BYTES: usize = 1_500_000;
const PART_JPEG_QUALITY: u8 = 85;

pub struct Ctx {
    pub opts: Options,
    pub ocr: Ocr,
    pub budget: Budget,
    pub parts: Vec<Part>,
}

/// The encoded original of a figure, when one exists, for zero-copy attachment.
pub struct Original<'a> {
    pub bytes: &'a [u8],
    pub mime: &'static str,
}

impl Ctx {
    pub fn new(opts: Options) -> Self {
        Self {
            opts,
            ocr: Ocr::new(Instant::now()),
            budget: Budget::default(),
            parts: Vec::new(),
        }
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
        let cost = json_len(&fig.text) + json_len(&fig.caption) + 160;
        if !self.budget.charge(cost) {
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
        if !acc.seen_images.insert(image_key(bytes)) {
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
