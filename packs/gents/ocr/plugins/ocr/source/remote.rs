//! Optional remote OCR: scanned pages and image files can be read by a vision
//! model the host reaches for the plugin (the pack's `remote_ocr` slot). The
//! plugin never sees the endpoint or its key. It speaks the host's model call
//! protocol: when the input says `model_calls` it may answer a call with
//! `{"model_calls": {"requests": [...], "state": ...}}`; the host runs the
//! requests and calls again with the same input plus `model_results` (by
//! request id) and the `state` echoed back.
//!
//! One call is two rounds at most. Round one reads the file as usual, but a
//! page that wants remote OCR is rendered into a request instead, until the
//! batch is full; where the call stopped goes into `state`. Round two reads the
//! same file again, takes the answers for those pages and stops at the same
//! place, returning the usual cursor for the rest. Pages whose request failed,
//! or that got no answer, fall back to the built-in OCR with a warning.
use std::collections::HashMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ctx::Ctx;
use crate::html::{El, Node};
use crate::html_md::{Resolver, convert};
use crate::input::{Input, RemoteOcr};
use crate::model::DocAcc;
use crate::ocr::{OcrLine, Stats, unreliable};

/// Pages (requests) one round carries: with a few seconds per page on a GPU
/// backend this keeps a round well inside the call's wall clock.
const MAX_REQUESTS: usize = 12;
/// Base64 bytes of images one round carries, so the request JSON stays inside
/// the plugin's 4 MiB output limit with room for the prompts.
const REQUEST_BYTES_CAP: usize = 3_000_000;
/// A round is full once this little room is left for another page image.
const SOFT_BYTES: usize = REQUEST_BYTES_CAP - 700_000;
/// Longest side, in pixels, of a page image sent to the backend. The Chandra
/// project renders pages at 192 dpi, about 2300 pixels for a letter page; 2048
/// keeps a request near half a megabyte.
pub const REMOTE_MAX_PX: u32 = 2048;
/// The render scale for a PDF page: 192 dpi from the 72 points per inch.
pub const REMOTE_SCALE: f32 = 192.0 / 72.0;
pub const JPEG_QUALITY: u8 = 80;
/// Output tokens one page may use. A dense page is a few thousand tokens of HTML.
const MAX_TOKENS: u32 = 8192;
/// Bytes of a state the plugin accepts back: it only ever writes a few dozen.
const MAX_STATE_BYTES: usize = 4096;

/// Chandra's own page prompt (`OCR_PROMPT` of its `prompts.py`), asking for the
/// page as HTML with a fixed tag set; any OpenAI-compatible vision model that
/// follows it, or that answers in Markdown, works. Read from the project's
/// repository through a summarizing fetch, so the wording is not verified byte
/// for byte against a release.
const PROMPT: &str = "OCR this image to HTML.

Only use these tags [math, br, i, b, u, del, sup, sub, table, tr, td, p, th, div, pre, h1, h2, h3, h4, h5, ul, ol, li, input, a, span, img, hr, tbody, small, caption, strong, thead, big, code, chem], and these attributes [class, colspan, rowspan, display, checked, type, border, value, style, href, alt, align, data-bbox, data-label].

Guidelines:
* Inline math: Surround math with <math>...</math> tags. Math expressions should be rendered in KaTeX-compatible LaTeX. Use display for block math.
* Tables: Use colspan and rowspan attributes to match table structure.
* Formatting: Maintain consistent formatting with the image, including spacing, indentation, subscripts/superscripts, and special characters.
* Images: Include a description of any images in the alt attribute of an <img> tag. Do not fill out the src property. Describe in detail inside the div tag. Also convert charts to high fidelity data, and convert diagrams to mermaid.
* Forms: Mark checkboxes and radio buttons properly.
* Text: join lines together properly into paragraphs using <p>...</p> tags. Use <br> tags for line breaks within paragraphs, but only when absolutely necessary to maintain meaning.
* Chemistry: Use <chem>...</chem> tags for chemical formulas with reactive SMILES.
* Lists: Preserve indents and proper list markers.
* Use the simplest possible HTML structure that accurately represents the content of the block.
* Make sure the text is accurate and easy for a human to read and interpret. Reading order should be correct and natural.";

#[derive(Serialize)]
struct Image {
    mime: &'static str,
    data_base64: String,
}

#[derive(Serialize)]
struct Request {
    id: String,
    prompt: &'static str,
    images: Vec<Image>,
    max_tokens: u32,
}

/// The host's answer to one request.
pub enum Answer {
    Text(String),
    Error(String),
}

/// What a round leaves for the next one.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
struct RoundState {
    /// The file index and unit the call stopped at; round two stops there too.
    stop: Option<(u32, u32)>,
    /// Milliseconds of plugin time spent so far, so the call keeps one clock.
    ms: u64,
}

/// What became of a request offered to [`Remote::add`].
#[derive(Debug, PartialEq)]
pub enum Add {
    Added,
    /// The round holds as many pages as it can: the page waits for the next call.
    Full,
    /// One image alone is over the round's size.
    TooLarge,
}

/// The remote side of one call.
pub struct Remote {
    mode: RemoteOcr,
    /// The answers of the previous round; `None` in round one.
    answers: Option<HashMap<String, Answer>>,
    state: RoundState,
    file: u32,
    requests: Vec<Request>,
    bytes: usize,
    /// Pages of the current document read remotely, for its warning.
    read: Vec<u32>,
}

/// Where the requests and the read list stood when a unit began.
pub struct Mark {
    requests: usize,
    bytes: usize,
    read: usize,
}

impl Remote {
    /// Remote OCR is off: no request is ever made.
    pub fn off() -> Self {
        Self {
            mode: RemoteOcr::Off,
            answers: None,
            state: RoundState::default(),
            file: 0,
            requests: Vec::new(),
            bytes: 0,
            read: Vec::new(),
        }
    }

    /// The remote side of a call, from the host's fields. `mode` is the
    /// effective mode (off when the host offered no model calls).
    pub fn new(input: &Input, mode: RemoteOcr) -> Result<Self, String> {
        let state = match &input.state {
            None | Some(Value::Null) => RoundState::default(),
            Some(v) if v.to_string().len() > MAX_STATE_BYTES => {
                return Err("the state is larger than this plugin writes".into());
            }
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| format!("the state is not one this plugin returned: {e}"))?,
        };
        let answers = input.model_results.as_ref().map(|m| {
            m.iter()
                .map(|(id, v)| (id.clone(), answer_of(v)))
                .collect::<HashMap<_, _>>()
        });
        Ok(Self {
            mode,
            answers,
            state,
            ..Self::off()
        })
    }

    pub fn active(&self) -> bool {
        self.mode != RemoteOcr::Off
    }

    pub fn force(&self) -> bool {
        self.mode == RemoteOcr::Force
    }

    /// Whether this round may still make requests (the first round).
    pub fn collecting(&self) -> bool {
        self.active() && self.answers.is_none()
    }

    /// The plugin time earlier rounds of this call used.
    pub fn carried(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.state.ms)
    }

    pub fn set_file(&mut self, index: usize) {
        self.file = index as u32;
    }

    /// The request id of a unit of the current file.
    pub fn id(&self, unit: u32) -> String {
        format!("f{}p{unit}", self.file)
    }

    pub fn take_answer(&mut self, id: &str) -> Option<Answer> {
        self.answers.as_mut()?.remove(id)
    }

    fn full(&self) -> bool {
        self.requests.len() >= MAX_REQUESTS || self.bytes >= SOFT_BYTES
    }

    /// Whether the call stops before reading `unit` of the current file: the
    /// round is full (round one) or this is where round one stopped (round two).
    pub fn stops_before(&self, unit: u32) -> bool {
        if self.collecting() {
            return self.full();
        }
        self.active()
            && self
                .state
                .stop
                .is_some_and(|(f, u)| f == self.file && unit >= u)
    }

    /// The same question before a whole file `index`.
    pub fn stops_before_file(&self, index: usize) -> bool {
        if self.collecting() {
            return self.full();
        }
        self.active() && self.state.stop == Some((index as u32, 0))
    }

    /// Adds a page image to the round.
    pub fn add(&mut self, id: String, jpeg: &[u8]) -> Add {
        let data_base64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        if data_base64.len() > REQUEST_BYTES_CAP {
            return Add::TooLarge;
        }
        if self.bytes + data_base64.len() > REQUEST_BYTES_CAP {
            return Add::Full;
        }
        self.bytes += data_base64.len();
        self.requests.push(Request {
            id,
            prompt: PROMPT,
            images: vec![Image {
                mime: "image/jpeg",
                data_base64,
            }],
            max_tokens: MAX_TOKENS,
        });
        Add::Added
    }

    pub fn has_requests(&self) -> bool {
        !self.requests.is_empty()
    }

    pub fn note_read(&mut self, unit: u32) {
        self.read.push(unit);
    }

    /// The pages of the finished document that were read remotely, as a warning.
    pub fn take_read_warning(&mut self) -> Option<String> {
        let pages = std::mem::take(&mut self.read);
        let list = match pages.len() {
            0 => return None,
            1..=12 => pages
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            n => format!("{n} pages"),
        };
        Some(format!(
            "page(s) {list} were read by the remote OCR backend, which gives text only (no line boxes)"
        ))
    }

    pub fn mark(&self) -> Mark {
        Mark {
            requests: self.requests.len(),
            bytes: self.bytes,
            read: self.read.len(),
        }
    }

    pub fn rewind(&mut self, mark: &Mark) {
        self.requests.truncate(mark.requests);
        self.bytes = mark.bytes;
        self.read.truncate(mark.read);
    }

    /// The answer that asks the host to run this round's requests; `stop` is
    /// where the call stopped and `ms` the plugin time used so far.
    pub fn round(&self, stop: Option<(u32, u32)>, ms: u64) -> Result<String, String> {
        let state = serde_json::to_value(RoundState { stop, ms })
            .map_err(|e| format!("cannot write the state: {e}"))?;
        serde_json::to_string(&json!({"model_calls": {"requests": self.requests, "state": state}}))
            .map_err(|e| format!("cannot write the model requests: {e}"))
    }
}

fn answer_of(v: &Value) -> Answer {
    match (v["text"].as_str(), v["error"].as_str()) {
        (Some(t), _) => Answer::Text(t.to_string()),
        (None, Some(e)) => Answer::Error(e.to_string()),
        _ => Answer::Error("the answer had no text".into()),
    }
}

/// The warning for a page the backend could not read.
pub fn fallback_warning(unit: u32, reason: &str) -> String {
    format!("p.{unit}: remote OCR unavailable ({reason}); read with built-in OCR")
}

/// The built-in OCR's lines and what it dropped, or why it failed.
pub type Scored = Result<(Vec<OcrLine>, Stats), String>;

/// What reading a page or image through the remote path produced.
pub enum Read {
    /// The backend's text, as Markdown.
    Remote(String),
    /// The built-in OCR's lines, which read well enough or are all there is.
    Lines(Vec<OcrLine>),
    /// A request was made; the page is filled in by the next round.
    Pending,
    /// The round is full: leave the page for the next call.
    Wait,
    /// Read it with the built-in OCR as usual.
    Builtin,
}

/// The one place a page or image that needs OCR chooses between the backend
/// and the built-in OCR. `builtin` reads it with the built-in OCR and `encode`
/// renders it as a JPEG for a request; both run only when needed.
pub fn read_page(
    ctx: &mut Ctx,
    acc: &mut DocAcc,
    unit: u32,
    inked: bool,
    builtin: &mut dyn FnMut(&mut Ctx) -> Scored,
    encode: &mut dyn FnMut() -> Result<Vec<u8>, String>,
) -> Read {
    let id = ctx.remote.id(unit);
    match ctx.remote.take_answer(&id) {
        Some(Answer::Text(text)) => {
            let md = markdown(ctx, acc, unit, &text);
            ctx.remote.note_read(unit);
            return Read::Remote(md);
        }
        Some(Answer::Error(why)) => {
            acc.warn(fallback_warning(unit, &why));
            return Read::Builtin;
        }
        None => {}
    }
    let mut lines = None;
    if !ctx.remote.force() {
        // Auto: the built-in OCR reads first and only a poor read goes remote.
        // vertexia: the built-in read is repeated in round two for pages that read well; carrying their text in the state would save it.
        if let Ok((read, stats)) = builtin(ctx) {
            if unreliable(&read, stats, inked).is_none() {
                return Read::Lines(read);
            }
            lines = Some(read);
        }
    }
    let fallback = |lines: Option<Vec<OcrLine>>| lines.map_or(Read::Builtin, Read::Lines);
    if !ctx.remote.collecting() {
        acc.warn(fallback_warning(unit, "no answer was received"));
        return fallback(lines);
    }
    let jpeg = match encode() {
        Ok(j) => j,
        Err(why) => {
            acc.warn(fallback_warning(unit, &why));
            return fallback(lines);
        }
    };
    match ctx.remote.add(id, &jpeg) {
        Add::Added => Read::Pending,
        Add::Full => Read::Wait,
        Add::TooLarge => {
            acc.warn(fallback_warning(
                unit,
                "the page image is too large for one request",
            ));
            fallback(lines)
        }
    }
}

/// No image of the backend's HTML is ever a file, so none resolves.
struct NoImages;

impl Resolver for NoImages {
    fn image(&mut self, _: &str) -> Option<std::rc::Rc<[u8]>> {
        None
    }
}

/// Markdown from the backend's answer: HTML (Chandra's format) goes through
/// the HTML converter, anything else is taken as Markdown already.
pub fn markdown(ctx: &mut Ctx, acc: &mut DocAcc, unit: u32, text: &str) -> String {
    let text = unfence(text);
    if !looks_like_html(text) {
        return text.to_string();
    }
    let mut nodes = crate::html::parse(text);
    describe_images(&mut nodes);
    let blocks = convert(ctx, acc, &mut NoImages, Some(unit), &nodes);
    crate::md::render(&blocks)
}

/// The text inside a code fence some models wrap their answer in.
fn unfence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let body = rest.split_once('\n').map_or("", |(_, b)| b);
    body.strip_suffix("```").unwrap_or(body).trim()
}

fn looks_like_html(text: &str) -> bool {
    text.match_indices('<').any(|(i, _)| {
        text[i + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
    })
}

/// An `<img>` the backend described in `alt` (it never fills `src`) becomes an
/// italic paragraph, so the description is kept instead of the empty image.
fn describe_images(nodes: &mut [Node]) {
    for node in nodes {
        let Node::El(el) = node else { continue };
        let alt = (el.tag == "img" && el.attr("src").is_none_or(str::is_empty))
            .then(|| el.attr("alt").unwrap_or("").trim().to_string())
            .filter(|a| !a.is_empty());
        if let Some(alt) = alt {
            *node = Node::El(El {
                tag: "p".into(),
                attrs: Vec::new(),
                kids: vec![Node::El(El {
                    tag: "em".into(),
                    attrs: Vec::new(),
                    kids: vec![Node::Text(format!("Figure: {alt}"))],
                })],
            });
        } else {
            describe_images(&mut el.kids);
        }
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
