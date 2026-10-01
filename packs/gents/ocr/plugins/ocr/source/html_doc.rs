//! HTML and XHTML: a file up to [`WHOLE_BYTES`] is converted as one tree; a
//! larger one is read in chunks cut after block elements, each converted on its
//! own, so a page of any size costs one chunk of memory. Images are read from
//! beside the file when present.
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::html::parse;
use crate::html_md::{Resolver, convert};
use crate::model::{Block, DocAcc, Document};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps};
use crate::src::Src;
use crate::util::decode_text;

const MAX_IMAGE_FILE: u64 = 64 * 1024 * 1024;
/// Markup up to this size is parsed as one tree (output identical to the
/// original whole-file reader); beyond it the chunked reader takes over.
pub const WHOLE_BYTES: u64 = 8 * 1024 * 1024;
/// About this much markup is converted at a time by the chunked reader.
const CHUNK_BYTES: usize = 1024 * 1024;

/// Reads images relative to the HTML file, never outside `root`.
pub struct DirResolver {
    pub base: Option<PathBuf>,
    pub root: PathBuf,
}

impl Resolver for DirResolver {
    fn image(&mut self, src: &str) -> Option<Rc<[u8]>> {
        let base = self.base.as_ref()?;
        let rel = crate::util::percent_decode(src.split(['#', '?']).next().unwrap_or(""));
        let mut path = base.clone();
        for comp in Path::new(&rel).components() {
            match comp {
                Component::Normal(c) => path.push(c),
                Component::ParentDir => {
                    path.pop();
                }
                _ => {}
            }
        }
        if !path.starts_with(&self.root) || std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_FILE {
            return None;
        }
        std::fs::read(path).ok().map(Rc::from)
    }
}

const CONTAINERS: [&str; 15] = [
    "div",
    "section",
    "article",
    "table",
    "ul",
    "ol",
    "blockquote",
    "figure",
    "dl",
    "pre",
    "main",
    "nav",
    "aside",
    "form",
    "footer",
];
const LEAVES: [&str; 12] = [
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "dt",
    "dd",
    "tr",
    "figcaption",
];

/// Where to cut a window of markup that is not the end of the file: after the
/// last block element that closes outside every container, else after the
/// last block element of any depth, else at the last line break or character.
fn cut_point(buf: &[u8]) -> usize {
    let (mut depth, mut safe, mut any) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while let Some(off) = buf[i..].iter().position(|&c| c == b'<') {
        let lt = i + off;
        let rest = &buf[lt..];
        if rest.starts_with(b"<!--") {
            match find(rest, b"-->") {
                Some(e) => i = lt + e + 3,
                None => break,
            }
            continue;
        }
        let Some(gt) = tag_end(rest) else { break };
        let body = &rest[1..gt];
        let closing = body.first() == Some(&b'/');
        let name: String = body[usize::from(closing)..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase() as char)
            .collect();
        i = lt + gt + 1;
        if !closing && (name == "script" || name == "style") {
            let close = format!("</{name}");
            match find_ci(&buf[i..], close.as_bytes()) {
                Some(e) => i += e,
                None => break,
            }
            continue;
        }
        let container = CONTAINERS.contains(&name.as_str());
        if closing && (container || LEAVES.contains(&name.as_str())) {
            if container {
                depth = depth.saturating_sub(1);
            }
            any = i;
            if depth == 0 {
                safe = i;
            }
        } else if !closing && container && !body.ends_with(b"/") {
            depth += 1;
        }
    }
    let floor = buf.len() / 4;
    let cut = if safe > floor {
        safe
    } else if any > 0 {
        any
    } else {
        buf.iter()
            .rposition(|&c| c == b'\n')
            .map_or(buf.len(), |p| p + 1)
    };
    let mut cut = cut.min(buf.len());
    while cut > 0 && cut < buf.len() && (buf[cut] & 0xC0) == 0x80 {
        cut -= 1;
    }
    cut.max(1)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// Offset of the `>` that ends the tag at the start of `rest`, past quoted values.
fn tag_end(rest: &[u8]) -> Option<usize> {
    let mut quote = 0u8;
    for (i, &c) in rest.iter().enumerate().skip(1) {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(i);
        }
    }
    None
}

/// Markup read in chunks from a stream.
pub struct Chunks<R> {
    rd: R,
    buf: Vec<u8>,
    eof: bool,
    /// Bytes handed out so far, counted from the start of the unit.
    pub consumed: u64,
}

impl<R: Read> Chunks<R> {
    pub fn new(rd: R) -> Self {
        Self {
            rd,
            buf: Vec::new(),
            eof: false,
            consumed: 0,
        }
    }

    /// The next chunk of markup as text, `None` at the end.
    pub fn next_chunk(&mut self) -> Result<Option<String>, String> {
        while !self.eof && self.buf.len() < CHUNK_BYTES {
            let old = self.buf.len();
            self.buf.resize(CHUNK_BYTES, 0);
            let n = self
                .rd
                .read(&mut self.buf[old..])
                .map_err(|e| format!("cannot read the document: {e}"))?;
            self.buf.truncate(old + n);
            self.eof = n == 0;
        }
        if self.buf.is_empty() {
            return Ok(None);
        }
        let cut = if self.eof {
            self.buf.len()
        } else {
            cut_point(&self.buf)
        };
        let rest = self.buf.split_off(cut);
        let chunk = std::mem::replace(&mut self.buf, rest);
        self.consumed += chunk.len() as u64;
        let body = chunk.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&chunk);
        Ok(Some(String::from_utf8_lossy(body).into_owned()))
    }
}

/// An HTML document as one unit (small) or as chunks (large).
pub struct Reader<'a> {
    pub ctx: &'a mut Ctx,
    pub acc: &'a mut DocAcc,
    pub res: &'a mut dyn Resolver,
    pub unit: u32,
    /// The unit's whole text, when it is small.
    whole: Option<String>,
    chunks: Option<Chunks<Box<dyn Read + 'a>>>,
    done: bool,
}

impl<'a> Reader<'a> {
    /// A small unit converted as one tree.
    pub fn whole(
        ctx: &'a mut Ctx,
        acc: &'a mut DocAcc,
        res: &'a mut dyn Resolver,
        unit: u32,
        text: String,
    ) -> Self {
        Self {
            ctx,
            acc,
            res,
            unit,
            whole: Some(text),
            chunks: None,
            done: false,
        }
    }

    /// A large unit read from `rd`, which is already positioned `start` bytes in.
    pub fn chunked(
        ctx: &'a mut Ctx,
        acc: &'a mut DocAcc,
        res: &'a mut dyn Resolver,
        unit: u32,
        rd: Box<dyn Read + 'a>,
        start: u64,
    ) -> Self {
        let mut chunks = Chunks::new(rd);
        chunks.consumed = start;
        Self {
            ctx,
            acc,
            res,
            unit,
            whole: None,
            chunks: Some(chunks),
            done: false,
        }
    }

    fn render(&mut self, text: &str) -> String {
        let blocks: Vec<Block> =
            convert(self.ctx, self.acc, self.res, Some(self.unit), &parse(text));
        crate::md::render(&blocks)
    }
}

impl Steps for Reader<'_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.ctx, &mut *self.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: self.unit,
            pos: self.chunks.as_ref().map_or(0, |c| c.consumed),
            st: None,
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        if self.done {
            return Ok(None);
        }
        let text = if let Some(t) = self.whole.take() {
            self.done = true;
            t
        } else if let Some(c) = self.chunks.as_mut() {
            match c.next_chunk()? {
                Some(t) => t,
                None => {
                    self.done = true;
                    return Ok(None);
                }
            }
        } else {
            return Ok(None);
        };
        let md = self.render(&text);
        if self.ctx.waiting {
            return Ok(Some(Step::Wait));
        }
        Ok(Some(Step::Chunk(md)))
    }
}

pub fn convert_html(
    ctx: &mut Ctx,
    source: &str,
    src: &mut Src,
    res: &mut dyn Resolver,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &header(source, "html"));
        }
    }
    let skip = resume.map_or(0, |r| r.skip);
    let next = if src.len() <= WHOLE_BYTES {
        let bytes = src
            .head(src.len() as usize)
            .map_err(|e| format!("cannot read the file: {e}"))?;
        let text = decode_text(&bytes).into_owned();
        let mut reader = Reader::whole(ctx, &mut acc, res, 1, text);
        slicer::run(&mut reader, skip)?
    } else {
        use std::io::Seek;
        let start = resume.map_or(0, |r| r.pos);
        src.seek(std::io::SeekFrom::Start(start))
            .map_err(|e| format!("cannot read the file: {e}"))?;
        let mut reader = Reader::chunked(ctx, &mut acc, res, 1, Box::new(&mut *src), start);
        slicer::run(&mut reader, skip)?
    };
    Ok(ctx.document(acc, source, "html", 1, resume, next, parts_from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_cut_after_a_block_outside_every_container() {
        let html = "<html><body><p>one</p><div><p>two</p></div><p>three</p><p>four and then some more text";
        let cut = cut_point(html.as_bytes());
        assert_eq!(
            &html[..cut],
            "<html><body><p>one</p><div><p>two</p></div><p>three</p>"
        );
    }

    #[test]
    fn a_page_wrapped_in_one_div_is_still_cut_at_a_block() {
        let html = format!("<div id=\"all\">{}<p>tail", "<p>para</p>".repeat(50));
        let cut = cut_point(html.as_bytes());
        assert!(html[..cut].ends_with("</p>") && cut > 100, "{cut}");
    }

    #[test]
    fn scripts_and_comments_do_not_confuse_the_cut() {
        let html = "<p>a</p><script>var s = '</p>';</script><!-- </p> --><p>b</p><p>c";
        let cut = cut_point(html.as_bytes());
        assert_eq!(
            &html[..cut],
            "<p>a</p><script>var s = '</p>';</script><!-- </p> --><p>b</p>"
        );
    }

    #[test]
    fn plain_text_without_tags_is_cut_at_a_line() {
        assert_eq!(cut_point(b"first line\nsecond li"), 11);
        let cut = cut_point("\u{4e2d}\u{4e2d}".as_bytes());
        assert!(cut == 6 || cut == 3);
    }

    #[test]
    fn chunks_cover_the_input_exactly() {
        let page = format!(
            "<html><body>{}</body></html>",
            "<p>paragraph of text</p>".repeat(200_000)
        );
        let mut chunks = Chunks::new(page.as_bytes());
        let mut joined = String::new();
        let mut n = 0;
        while let Some(c) = chunks.next_chunk().unwrap() {
            joined.push_str(&c);
            n += 1;
        }
        assert!(n >= 4);
        assert_eq!(joined, page);
        assert_eq!(chunks.consumed, page.len() as u64);
    }
}
