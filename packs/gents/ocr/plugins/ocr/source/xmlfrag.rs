//! Streams the children of one element of a large XML part as small pieces,
//! each a well-formed document that the tree-based readers parse on their own.
//! Memory follows the size of a piece, never the size of the part, and the
//! position after each piece is a byte offset a later call resumes at.
//!
//! The scanner tracks tags only: text is skipped unread, comments, processing
//! instructions and CDATA are stepped over, and a DTD is refused (the tree
//! parser refuses it as well), so no entity can be defined.
use std::io::{self, Read};

/// A single child element may be at most this large; it is held whole.
const MAX_PIECE_BYTES: u64 = 64 * 1024 * 1024;
const READ_CHUNK: usize = 64 * 1024;
/// Transparent elements may nest this deep (the depth shares bits with the offset in a position).
const MAX_EXTRA: u64 = 15;

fn err(msg: &str) -> String {
    format!("the document XML is malformed: {msg}")
}

fn io_err(e: io::Error) -> String {
    format!("cannot read the document part: {e}")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    End,
    Empty,
}

/// One tag seen by the scanner.
struct Tag {
    kind: Kind,
    local: String,
    /// Absolute offsets in the stream: the `<` and the byte after the `>`.
    start: u64,
    end: u64,
}

struct Lexer<R> {
    rd: R,
    buf: Vec<u8>,
    pos: usize,
    /// Stream offset of `buf[0]`.
    base: u64,
    eof: bool,
    /// Bytes from this absolute offset on must stay in the buffer.
    keep: Option<u64>,
}

impl<R: Read> Lexer<R> {
    fn new(rd: R) -> Self {
        Self {
            rd,
            buf: Vec::new(),
            pos: 0,
            base: 0,
            eof: false,
            keep: None,
        }
    }

    fn abs(&self, at: usize) -> u64 {
        self.base + at as u64
    }

    /// Drops what is behind the scan position and the kept region.
    fn compact(&mut self) {
        let floor = match self.keep {
            Some(k) => (k.saturating_sub(self.base) as usize).min(self.pos),
            None => self.pos,
        };
        if floor > 0 {
            self.buf.drain(..floor);
            self.base += floor as u64;
            self.pos -= floor;
        }
    }

    /// Appends the next chunk of the stream; false at its end. Indices into the
    /// buffer stay valid: only [`Lexer::compact`] moves them.
    fn fill(&mut self) -> Result<bool, String> {
        if self.eof {
            return Ok(false);
        }
        if self.buf.len() as u64 > MAX_PIECE_BYTES + READ_CHUNK as u64 {
            return Err(format!(
                "one element of the XML part is larger than {} MiB and cannot be read",
                MAX_PIECE_BYTES / 1024 / 1024
            ));
        }
        let old = self.buf.len();
        self.buf.resize(old + READ_CHUNK, 0);
        let n = self.rd.read(&mut self.buf[old..]).map_err(io_err)?;
        self.buf.truncate(old + n);
        if n == 0 {
            self.eof = true;
        }
        Ok(n > 0)
    }

    /// Index in the buffer of `needle` at or after `from`, reading as needed.
    fn find(&mut self, needle: &[u8], mut from: usize) -> Result<Option<usize>, String> {
        loop {
            if let Some(i) = self.buf[from.min(self.buf.len())..]
                .windows(needle.len())
                .position(|w| w == needle)
            {
                return Ok(Some(from.min(self.buf.len()) + i));
            }
            // A match may straddle what was read so far.
            from = self.buf.len().saturating_sub(needle.len() - 1).max(from);
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    /// Moves to the next `<`, discarding the text before it (text inside a kept
    /// piece stays buffered); false at the end of the stream.
    fn skip_text(&mut self) -> Result<bool, String> {
        loop {
            if let Some(i) = self.buf[self.pos..].iter().position(|&c| c == b'<') {
                self.pos += i;
                return Ok(true);
            }
            self.pos = self.buf.len();
            self.compact();
            if !self.fill()? {
                return Ok(false);
            }
        }
    }

    /// The end (exclusive) of a start or end tag that begins at `self.pos`,
    /// stepping over quoted attribute values.
    fn tag_end(&mut self) -> Result<usize, String> {
        let mut i = self.pos + 1;
        let mut quote = 0u8;
        loop {
            while i >= self.buf.len() {
                if !self.fill()? {
                    return Err(err("the part ends inside a tag"));
                }
            }
            let c = self.buf[i];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'>' {
                return Ok(i + 1);
            }
            i += 1;
        }
    }

    fn need(&mut self, n: usize) -> Result<bool, String> {
        while self.buf.len() < self.pos + n {
            if !self.fill()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The next start, end or empty-element tag, or `None` at the end of the part.
    fn next_tag(&mut self) -> Result<Option<Tag>, String> {
        loop {
            self.compact();
            if !self.skip_text()? {
                return Ok(None);
            }
            if !self.need(2)? {
                return Err(err("the part ends inside a tag"));
            }
            match self.buf[self.pos + 1] {
                b'?' => {
                    let Some(i) = self.find(b"?>", self.pos)? else {
                        return Err(err("a processing instruction is not closed"));
                    };
                    self.pos = i + 2;
                }
                b'!' => {
                    self.need(9)?;
                    let rest = &self.buf[self.pos..];
                    let (open, close): (&[u8], &[u8]) = if rest.starts_with(b"<!--") {
                        (b"<!--", b"-->")
                    } else if rest.starts_with(b"<![CDATA[") {
                        (b"<![CDATA[", b"]]>")
                    } else {
                        return Err(err("a document type declaration is not allowed"));
                    };
                    let Some(i) = self.find(close, self.pos + open.len())? else {
                        return Err(err("a comment or CDATA section is not closed"));
                    };
                    self.pos = i + close.len();
                }
                b'/' => {
                    let end = self.tag_end()?;
                    let local = local_name(&self.buf[self.pos + 2..end - 1]);
                    let tag = Tag {
                        kind: Kind::End,
                        local,
                        start: self.abs(self.pos),
                        end: self.abs(end),
                    };
                    self.pos = end;
                    return Ok(Some(tag));
                }
                _ => {
                    let end = self.tag_end()?;
                    let empty = self.buf[end - 2] == b'/';
                    let inner_end = if empty { end - 2 } else { end - 1 };
                    let local = local_name(&self.buf[self.pos + 1..inner_end]);
                    let tag = Tag {
                        kind: if empty { Kind::Empty } else { Kind::Start },
                        local,
                        start: self.abs(self.pos),
                        end: self.abs(end),
                    };
                    self.pos = end;
                    return Ok(Some(tag));
                }
            }
        }
    }

    /// The bytes between two absolute offsets, which must still be buffered.
    fn slice(&self, from: u64, to: u64) -> &[u8] {
        &self.buf[(from - self.base) as usize..(to - self.base) as usize]
    }

    /// Discards input up to the absolute offset `to`.
    fn skip_to(&mut self, to: u64) -> Result<(), String> {
        loop {
            let have_end = self.base + self.buf.len() as u64;
            if to <= have_end {
                self.pos = (to - self.base) as usize;
                return Ok(());
            }
            self.pos = self.buf.len();
            self.compact();
            if !self.fill()? {
                return Err(err("the part is shorter than the position to resume at"));
            }
        }
    }
}

/// The element name of a tag body (what follows `<` or `</`) without its prefix.
fn local_name(body: &[u8]) -> String {
    let end = body
        .iter()
        .position(|c| c.is_ascii_whitespace() || *c == b'/' || *c == b'>')
        .unwrap_or(body.len());
    let name = &body[..end];
    let local = name
        .iter()
        .rposition(|&c| c == b':')
        .map_or(name, |i| &name[i + 1..]);
    String::from_utf8_lossy(local).into_owned()
}

/// The value of attribute `name` (matched without its prefix) in a raw start tag.
pub fn attribute(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let mut rest = text.as_ref();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq]
            .rsplit(|c: char| c.is_whitespace() || c == '<')
            .next()?;
        let key = key.rsplit(':').next().unwrap_or(key);
        let after = &rest[eq + 1..];
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = after[1..].find(quote)?;
        let value = &after[1..1 + close];
        if key == name {
            return Some(
                quick_xml::escape::unescape(value)
                    .map_or_else(|_| value.to_string(), |v| v.into_owned()),
            );
        }
        rest = &after[close + 2..];
    }
    None
}

/// A position inside the part: a byte offset and how many transparent
/// elements are open, packed into one number a cursor can carry.
pub fn pack(offset: u64, extra: u64) -> u64 {
    offset << 4 | extra
}

fn unpack(pos: u64) -> (u64, u64) {
    (pos >> 4, pos & 0xF)
}

/// Runs `f` on every tag of a part until it returns false; used for cheap
/// whole-part questions (is there a title style, how many slides).
pub fn scan<R: Read>(rd: R, mut f: impl FnMut(&str, bool, &[u8]) -> bool) -> Result<(), String> {
    let mut lx = Lexer::new(rd);
    loop {
        let Some(tag) = lx.next_tag()? else {
            return Ok(());
        };
        // Only start tags are kept readable; their raw bytes are right behind the scan position.
        let start = (tag.start - lx.base) as usize;
        let raw = lx.buf[start..(tag.end - lx.base) as usize].to_vec();
        if !f(&tag.local, tag.kind == Kind::End, &raw) {
            return Ok(());
        }
    }
}

/// A run of child elements as one document ready for a tree parse.
pub struct Batch {
    /// `<root ...><a><b>` piece piece ... `</b></a></root>`: the pieces inside
    /// the wrapper elements that stand in for the path to the container.
    pub xml: String,
    /// Depth of the container: the pieces are the children of the element this many levels down from the root.
    pub depth: usize,
    /// Packed position of the first piece (tests check it), and of the byte after the last one.
    #[cfg(test)]
    pub pos: u64,
    pub next: u64,
    /// Whether the container's end was reached.
    pub last: bool,
}

/// Children of the element at the end of a path, streamed in batches.
pub struct Frags<R> {
    lx: Lexer<R>,
    open_tags: String,
    close_tags: String,
    depth: usize,
    transparent: Vec<String>,
    extra: u64,
    done: bool,
    /// The raw start tags of the path elements below the root (the sheet's own attributes, for example).
    pub path_tags: Vec<Vec<u8>>,
}

impl<R: Read> Frags<R> {
    /// Opens the part at the element named by `path`: local names from the root
    /// down, each with how many same-named siblings come first. `transparent`
    /// names elements whose children are yielded as if they were the container's.
    /// With `resume` (a position from [`Batch::next`]) the scan continues there.
    pub fn open(
        rd: R,
        path: &[(&str, usize)],
        transparent: &[&str],
        resume: Option<u64>,
    ) -> Result<Self, String> {
        let mut lx = Lexer::new(rd);
        if lx.need(2).map_err(|e| e.to_string())?
            && matches!(&lx.buf[..2], [0xFF, 0xFE] | [0xFE, 0xFF])
        {
            return Err(err("UTF-16 XML parts are not supported"));
        }
        // The root start tag, with the namespace declarations every piece needs.
        let tag = lx
            .next_tag()?
            .ok_or_else(|| err("the part has no root element"))?;
        let root = match tag.kind {
            Kind::Start => tag,
            Kind::Empty => return Err(err("the part has no content")),
            Kind::End => return Err(err("an end tag comes before the root element")),
        };
        if path.is_empty() || root.local != path[0].0 {
            return Err(format!(
                "the document XML has the root element {} where {} is expected",
                root.local,
                path.first().map_or("", |p| p.0)
            ));
        }
        let root_raw = String::from_utf8_lossy(lx.slice(root.start, root.end)).into_owned();
        let mut open_tags = root_raw;
        let mut close_tags = format!("</{}>", qname(&open_tags));
        for (name, _) in &path[1..] {
            open_tags.push_str(&format!("<{name}>"));
            close_tags.insert_str(0, &format!("</{name}>"));
        }
        let mut frags = Self {
            lx,
            open_tags,
            close_tags,
            depth: path.len(),
            transparent: transparent.iter().map(|s| (*s).to_string()).collect(),
            extra: 0,
            done: false,
            path_tags: Vec::new(),
        };
        if let Some(pos) = resume {
            let (off, extra) = unpack(pos);
            frags.extra = extra;
            frags.lx.skip_to(off)?;
            return Ok(frags);
        }
        frags.descend(path)?;
        Ok(frags)
    }

    /// Finds the container below the root by skipping every other subtree.
    fn descend(&mut self, path: &[(&str, usize)]) -> Result<(), String> {
        let mut level = 1;
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        while level < path.len() {
            let tag = self
                .lx
                .next_tag()?
                .ok_or_else(|| err("the element to read was not found"))?;
            match tag.kind {
                Kind::End => return Err(err("the element to read was not found")),
                Kind::Empty => {
                    *seen.entry(tag.local).or_default() += 1;
                }
                Kind::Start => {
                    let count = seen.entry(tag.local.clone()).or_default();
                    let hit = tag.local == path[level].0 && *count == path[level].1;
                    *count += 1;
                    if hit {
                        self.path_tags
                            .push(self.lx.slice(tag.start, tag.end).to_vec());
                        level += 1;
                        seen.clear();
                    } else {
                        self.skip_subtree()?;
                    }
                }
            }
        }
        Ok(())
    }

    fn skip_subtree(&mut self) -> Result<(), String> {
        let mut depth = 1usize;
        while depth > 0 {
            match self
                .lx
                .next_tag()?
                .ok_or_else(|| err("the part ends inside an element"))?
                .kind
            {
                Kind::Start => depth += 1,
                Kind::End => depth -= 1,
                Kind::Empty => {}
            }
        }
        Ok(())
    }

    /// The next run of child elements, about `max_bytes` of XML, or `None` when there are no more.
    pub fn next_batch(&mut self, max_bytes: usize) -> Result<Option<Batch>, String> {
        if self.done {
            return Ok(None);
        }
        #[cfg(test)]
        let first_pos = pack(self.lx.abs(self.lx.pos), self.extra);
        let mut xml = self.open_tags.clone();
        let head = xml.len();
        let mut last = false;
        while xml.len() - head < max_bytes {
            let Some(tag) = self.lx.next_tag()? else {
                return Err(err("the part ends before its container is closed"));
            };
            match tag.kind {
                Kind::End => {
                    if self.extra > 0 {
                        self.extra -= 1;
                        continue;
                    }
                    self.done = true;
                    last = true;
                    break;
                }
                Kind::Start if self.transparent.contains(&tag.local) => {
                    if self.extra >= MAX_EXTRA {
                        return Err(err("elements are nested too deeply"));
                    }
                    self.extra += 1;
                }
                Kind::Empty if self.transparent.contains(&tag.local) => {}
                Kind::Empty => {
                    xml.push_str(&String::from_utf8_lossy(self.lx.slice(tag.start, tag.end)));
                }
                Kind::Start => {
                    self.lx.keep = Some(tag.start);
                    let mut depth = 1usize;
                    while depth > 0 {
                        let t = self
                            .lx
                            .next_tag()?
                            .ok_or_else(|| err("the part ends inside an element"))?;
                        match t.kind {
                            Kind::Start => depth += 1,
                            Kind::End => depth -= 1,
                            Kind::Empty => {}
                        }
                        if depth == 0 {
                            xml.push_str(&String::from_utf8_lossy(self.lx.slice(tag.start, t.end)));
                        }
                    }
                    self.lx.keep = None;
                }
            }
        }
        let end = self.lx.abs(self.lx.pos);
        if last && xml.len() == head {
            return Ok(None);
        }
        xml.push_str(&self.close_tags);
        Ok(Some(Batch {
            xml,
            depth: self.depth,
            #[cfg(test)]
            pos: first_pos,
            next: pack(end, self.extra),
            last,
        }))
    }
}

/// `w:document` from a raw start tag.
fn qname(raw: &str) -> String {
    let body = raw.trim_start_matches('<');
    let end = body
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(body.len());
    body[..end].to_string()
}

/// The container element of a parsed batch: the node whose children are the pieces.
pub fn container<'a, 'i>(
    doc: &'a roxmltree::Document<'i>,
    depth: usize,
) -> roxmltree::Node<'a, 'i> {
    let mut node = doc.root_element();
    for _ in 1..depth {
        node = node
            .children()
            .find(roxmltree::Node::is_element)
            .unwrap_or(node);
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<?xml version=\"1.0\"?><!-- c --><w:document xmlns:w=\"urn:w\" xmlns:r=\"urn:r\"><w:head><w:p>skip</w:p></w:head><w:body><w:p r:id=\"1\">a &lt; b</w:p>\n<w:sect><w:p>inner</w:p></w:sect><w:p><![CDATA[<x>]]></w:p><w:br/><w:p>last</w:p></w:body><w:tail/></w:document>";

    fn batches(doc: &str, transparent: &[&str], max: usize) -> Vec<(String, u64, u64)> {
        let mut f = Frags::open(
            doc.as_bytes(),
            &[("document", 0), ("body", 0)],
            transparent,
            None,
        )
        .unwrap();
        let mut out = Vec::new();
        while let Some(b) = f.next_batch(max).unwrap() {
            out.push((b.xml, b.pos, b.next));
        }
        out
    }

    fn texts(doc: &str, transparent: &[&str], max: usize) -> Vec<String> {
        batches(doc, transparent, max)
            .into_iter()
            .flat_map(|(xml, _, _)| {
                let d = crate::xml::parse(&xml).unwrap();
                let c = container(&d, 2);
                c.children()
                    .filter(|n| n.is_element())
                    .map(|n| format!("{}:{}", n.tag_name().name(), crate::xml::text(n)))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn children_of_the_container_come_out_whole_and_parse_with_their_namespaces() {
        assert_eq!(
            texts(DOC, &[], 1 << 20),
            ["p:a < b", "sect:inner", "p:<x>", "br:", "p:last"]
        );
        let all = batches(DOC, &[], 1 << 20);
        assert!(
            all[0].0.starts_with("<w:document xmlns:w=\"urn:w\"")
                && all[0].0.ends_with("</w:document>")
        );
        let d = crate::xml::parse(&all[0].0).unwrap();
        let first = container(&d, 2)
            .children()
            .find(|n| n.is_element())
            .unwrap();
        assert_eq!(first.attribute(("urn:r", "id")), Some("1"));
    }

    #[test]
    fn transparent_elements_are_entered() {
        assert_eq!(
            texts(DOC, &["sect"], 1 << 20),
            ["p:a < b", "p:inner", "p:<x>", "br:", "p:last"]
        );
    }

    #[test]
    fn small_batches_resume_exactly_where_they_stopped() {
        let whole = texts(DOC, &["sect"], 1 << 20);
        let mut got = Vec::new();
        let mut f = Frags::open(
            DOC.as_bytes(),
            &[("document", 0), ("body", 0)],
            &["sect"],
            None,
        )
        .unwrap();
        let mut resume_at = None;
        // One piece per call, resuming from the packed position each time.
        loop {
            if let Some(r) = resume_at {
                f = Frags::open(
                    DOC.as_bytes(),
                    &[("document", 0), ("body", 0)],
                    &["sect"],
                    Some(r),
                )
                .unwrap();
            }
            let Some(b) = f.next_batch(1).unwrap() else {
                break;
            };
            let d = crate::xml::parse(&b.xml).unwrap();
            got.extend(
                container(&d, 2)
                    .children()
                    .filter(|n| n.is_element())
                    .map(|n| format!("{}:{}", n.tag_name().name(), crate::xml::text(n))),
            );
            resume_at = Some(b.next);
            if b.last {
                break;
            }
        }
        assert_eq!(got, whole);
    }

    #[test]
    fn the_nth_sibling_is_selected_and_its_attributes_are_kept() {
        let doc = "<r xmlns:t=\"urn:t\"><s><t:table t:name=\"A &amp; B\"><t:row>1</t:row></t:table><t:table t:name=\"C\"><t:row>2</t:row><t:row>3</t:row></t:table></s></r>";
        let mut f = Frags::open(
            doc.as_bytes(),
            &[("r", 0), ("s", 0), ("table", 1)],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(attribute(&f.path_tags[1], "name").as_deref(), Some("C"));
        let b = f.next_batch(1 << 20).unwrap().unwrap();
        assert!(b.xml.contains("<t:row>2</t:row><t:row>3</t:row>") && b.last);
        let f = Frags::open(
            doc.as_bytes(),
            &[("r", 0), ("s", 0), ("table", 0)],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(attribute(&f.path_tags[1], "name").as_deref(), Some("A & B"));
    }

    #[test]
    fn a_dtd_a_missing_path_and_a_truncated_part_are_refused() {
        let dtd = "<!DOCTYPE x [<!ENTITY a \"b\">]><r><b/></r>";
        assert!(Frags::open(dtd.as_bytes(), &[("r", 0), ("b", 0)], &[], None).is_err());
        assert!(Frags::open(DOC.as_bytes(), &[("document", 0), ("nope", 0)], &[], None).is_err());
        let cut = &DOC[..DOC.len() - 30];
        let mut f =
            Frags::open(cut.as_bytes(), &[("document", 0), ("body", 0)], &[], None).unwrap();
        let mut failed = false;
        for _ in 0..10 {
            match f.next_batch(1) {
                Err(_) => {
                    failed = true;
                    break;
                }
                Ok(None) => break,
                Ok(Some(_)) => {}
            }
        }
        assert!(failed);
    }

    #[test]
    fn scan_visits_every_tag_with_its_attributes() {
        let mut names = Vec::new();
        scan(DOC.as_bytes(), |n, end, raw| {
            if n == "p" && !end {
                names.push(attribute(raw, "id"));
            }
            true
        })
        .unwrap();
        assert_eq!(names, [None, Some("1".to_string()), None, None, None]);
    }

    #[test]
    fn pieces_larger_than_the_read_chunk_survive_buffer_refills() {
        let big = "x".repeat(300_000);
        let doc = format!("<d><b><p>{big}</p><p>tail</p></b></d>");
        let got = texts_of(&doc);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), 300_002);
    }

    fn texts_of(doc: &str) -> Vec<String> {
        let mut f = Frags::open(doc.as_bytes(), &[("d", 0), ("b", 0)], &[], None).unwrap();
        let mut out = Vec::new();
        while let Some(b) = f.next_batch(1).unwrap() {
            let d = crate::xml::parse(&b.xml).unwrap();
            out.extend(
                container(&d, 2)
                    .children()
                    .filter(|n| n.is_element())
                    .map(|n| format!("p:{}", crate::xml::text(n))),
            );
        }
        out
    }
}
