//! Output model shared by every format converter: documents, figures, the
//! Markdown block tree and the output budget that keeps one call inside the
//! host's output ceiling.
use serde::Serialize;

use crate::resume::Resume;

/// The host's output ceiling is 4 MiB; content stops at this many JSON bytes so
/// the envelope, figure records and warnings always fit under it.
pub const OUTPUT_CAP_BYTES: usize = 3_800_000;
/// Warnings beyond this many per document collapse into one summary line.
const MAX_WARNINGS: usize = 40;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Figure {
    pub id: String,
    pub page: Option<u32>,
    pub caption: String,
    pub text: String,
    pub width: u32,
    pub height: u32,
    pub ocr: bool,
    /// Index into the top-level image parts when the figure image is attached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<usize>,
    /// Why the figure has no text, shown in the Markdown block only.
    #[serde(skip)]
    pub note: String,
}

#[derive(Serialize)]
pub struct Document {
    pub source: String,
    pub format: &'static str,
    pub pages: u32,
    pub markdown: String,
    pub figures: Vec<Figure>,
    pub warnings: Vec<String>,
    /// Failed page reads are distinct from successfully read blank pages.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread_pages: Vec<u32>,
    /// Only on a document that continues an earlier call: how its Markdown
    /// joins the earlier part: `blank` (a blank line), `line` (a line break) or
    /// `none` (the earlier part was cut inside a line, so the two are one line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joint: Option<&'static str>,
    /// Only on a continuation inside a table: the header rows of that table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_header: Option<String>,
    /// Where the next call continues this document, when it was not finished.
    #[serde(skip)]
    pub next: Option<Resume>,
}

#[derive(Serialize)]
pub struct Part {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime: &'static str,
}

/// One Markdown block. Converters build these; `md::render` is the only writer.
#[derive(Debug, Clone)]
pub enum Block {
    Heading(u8, String),
    Para(String),
    Item {
        depth: u8,
        marker: String,
        text: String,
    },
    Table(Vec<Vec<String>>),
    Code(String),
    Quote(Vec<Block>),
    /// A paragraph the source marks as a caption (Word "Caption" style).
    Caption(String),
    Figure(Box<Figure>),
    Rule,
    /// Verbatim line, such as a page marker comment.
    Raw(String),
}

/// Bytes a string takes once JSON-escaped, quotes included.
pub fn json_len(s: &str) -> usize {
    2 + s
        .bytes()
        .map(|b| match b {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' => 2,
            0..=0x1f => 6,
            _ => 1,
        })
        .sum::<usize>()
}

/// Bytes one character takes once JSON-escaped, quotes not included.
pub fn json_char_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// What one document has accumulated so far.
#[derive(Default)]
pub struct DocAcc {
    pub md: String,
    pub figures: Vec<Figure>,
    pub warnings: Vec<String>,
    pub unread_pages: Vec<u32>,
    pub next_fig: u32,
    pub small_skipped: u32,
    pub repeated_skipped: u32,
    /// Hashes of the images already turned into figures in this document, with
    /// the order they were added in, which a continuation carries over.
    pub seen_images: std::collections::HashSet<u128>,
    pub seen_order: Vec<u128>,
    /// Alt text per figure id, used as the caption only when no real caption is found.
    pub alts: std::collections::HashMap<String, String>,
    /// Set once the output budget refused more content.
    pub truncated: bool,
    pub(crate) suppressed: usize,
}

/// Images remembered per document; past this an image used again is listed again.
pub const MAX_SEEN_IMAGES: usize = 256;

impl DocAcc {
    /// Remembers an image; false when it was already listed earlier in the document.
    pub fn see(&mut self, key: u128) -> bool {
        if self.seen_images.contains(&key) {
            return false;
        }
        if self.seen_order.len() < MAX_SEEN_IMAGES {
            self.seen_images.insert(key);
            self.seen_order.push(key);
        }
        true
    }

    /// Restores the memory of listed images a continuation was issued with.
    pub fn restore_seen(&mut self, seen: &[String]) {
        for k in seen.iter().filter_map(|h| u128::from_str_radix(h, 16).ok()) {
            self.see(k);
        }
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(message.into());
        } else {
            self.suppressed += 1;
        }
    }

    /// Final warnings, with the suppressed count made visible.
    pub fn finish_warnings(&mut self) -> Vec<String> {
        if self.small_skipped > 0 {
            let n = self.small_skipped;
            self.warnings.push(format!(
                "{n} image(s) smaller than min_figure_px were skipped as decorative"
            ));
        }
        if self.repeated_skipped > 0 {
            let n = self.repeated_skipped;
            self.warnings.push(format!(
                "{n} image(s) already listed earlier in the document were listed once and skipped"
            ));
        }
        if self.suppressed > 0 {
            let n = self.suppressed;
            self.warnings
                .push(format!("{n} more warning(s) were suppressed"));
        }
        std::mem::take(&mut self.warnings)
    }
}

/// Tracks the JSON bytes this call has produced against its cap, by default [`OUTPUT_CAP_BYTES`].
pub struct Budget {
    used: usize,
    cap: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(OUTPUT_CAP_BYTES)
    }
}

impl Budget {
    pub fn new(cap: usize) -> Self {
        Self { used: 0, cap }
    }

    pub fn used(&self) -> usize {
        self.used
    }

    /// Returns bytes charged for something that was taken back out of the result.
    pub fn refund(&mut self, bytes: usize) {
        self.used = self.used.saturating_sub(bytes);
    }

    pub fn fits(&self, bytes: usize) -> bool {
        self.used + bytes <= self.cap
    }

    pub fn charge(&mut self, bytes: usize) -> bool {
        if !self.fits(bytes) {
            return false;
        }
        self.used += bytes;
        true
    }

    pub fn remaining(&self) -> usize {
        self.cap.saturating_sub(self.used)
    }
}
