//! A PDF read through seeks: the cross-reference data is indexed once (a
//! 16-byte entry per object, never the file), the page tree is walked on
//! demand, and a window of pages is turned into a small PDF made of copies of
//! just the objects those pages use. The reader then opens that small file, so
//! the memory of a call follows the window, not the size of the file.
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::rc::Rc;

use crate::input::Options;
use crate::pdfdecode::decode_stream;
use crate::pdfobj::{Fail, Kind, Val, is_space, parse_value, skip_ws};
use crate::src::Src;

/// Objects beyond this many are refused: the index costs 16 bytes each.
const MAX_OBJECTS: usize = 16_000_000;
/// Cross-reference sections followed through `Prev`.
const MAX_SECTIONS: usize = 4096;
/// The text of one object (its dictionary, not its stream) is at most this large.
const MAX_OBJECT_TEXT: usize = 64 * 1024 * 1024;
/// A stream copied into a window is at most this large; a larger one is left empty.
pub const STREAM_CAP: u64 = 128 * 1024 * 1024;
/// All copies of one window stay under this; further streams are left empty.
const POOL_HARD_CAP: usize = 512 * 1024 * 1024;
/// Decoded object streams stay cached up to this many bytes.
const OBJSTM_CACHE: usize = 64 * 1024 * 1024;
pub(crate) const MAX_DECODED: usize = 256 * 1024 * 1024;
/// A window holds at most this many pages, and stops growing past [`WINDOW_BYTES`] of copies.
pub const WINDOW_PAGES: usize = 16;
pub const WINDOW_BYTES: usize = 48 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
/// Page attributes a page inherits from the nodes above it.
const INHERITED: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];
// vertexia: annotations and form fields are not copied into a window, so they
// are not read; copying them needs their back references (P, Parent) cut.
/// Page keys that lead away from the page (back to the tree, to annotations
/// and to structure): never copied, or one page would pull in the whole file.
const NOT_COPIED: [&str; 5] = ["Parent", "Annots", "B", "Thumb", "StructParents"];

#[derive(Clone, Copy, Default)]
enum Entry {
    #[default]
    Unset,
    Free,
    Offset(u64, u16),
    InStream(u32, u32),
}

struct ObjStm {
    data: Vec<u8>,
    first: usize,
    index: Vec<(u32, usize)>,
}

/// Where the data of a stream sits in the file.
#[derive(Clone, Copy)]
pub struct StreamAt {
    pub offset: u64,
    pub len: u64,
}

/// One object: its value text (a dictionary for a page) and its stream, if any.
pub struct RawObj {
    pub rev: u16,
    pub text: Vec<u8>,
    pub val: Val,
    pub stream: Option<StreamAt>,
}

pub struct Lazy {
    src: Src,
    entries: Vec<Entry>,
    root: u32,
    pub total: u32,
    cache: Vec<(u32, Rc<ObjStm>)>,
    cache_bytes: usize,
}

/// A small PDF holding the pages of a window, and what to tell the caller.
pub struct Window {
    pub bytes: Vec<u8>,
    /// The 1-based page numbers in the window, in order.
    pub numbers: Vec<u32>,
    /// The page to continue with after this window; past the total when none is left.
    pub after: u32,
    pub warnings: Vec<String>,
}

fn io_err(e: std::io::Error) -> String {
    format!("cannot read the file: {e}")
}

fn bad_xref(why: impl std::fmt::Display) -> String {
    format!("the cross-reference data is unreadable: {why}")
}

fn num(token: &[u8]) -> Option<u64> {
    std::str::from_utf8(token).ok()?.parse().ok()
}

/// Reads the next white-space separated token; false at the end of the input.
fn token(rd: &mut impl BufRead, out: &mut Vec<u8>) -> std::io::Result<bool> {
    out.clear();
    loop {
        let buf = rd.fill_buf()?;
        if buf.is_empty() {
            return Ok(!out.is_empty());
        }
        let mut used = 0;
        let mut done = false;
        for &b in buf {
            used += 1;
            if is_space(b) {
                if !out.is_empty() {
                    done = true;
                    break;
                }
            } else {
                out.push(b);
            }
        }
        rd.consume(used);
        if done {
            return Ok(true);
        }
    }
}

impl Lazy {
    /// Indexes the cross-reference data. An error means the file needs the
    /// whole-file reader (damaged index, encryption, an unsupported filter).
    pub fn open(src: Src) -> Result<Self, String> {
        let len = src.len();
        let mut me = Self {
            src,
            entries: Vec::new(),
            root: 0,
            total: 0,
            cache: Vec::new(),
            cache_bytes: 0,
        };
        let tail = me
            .src
            .read_at(len.saturating_sub(2048), 2048)
            .map_err(io_err)?;
        let at = tail
            .windows(9)
            .rposition(|w| w == b"startxref")
            .ok_or_else(|| bad_xref("no startxref"))?;
        let mut rest = &tail[at + 9..];
        while rest.first().is_some_and(|&b| is_space(b)) {
            rest = &rest[1..];
        }
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let mut next = Some(num(&rest[..digits]).ok_or_else(|| bad_xref("no offset"))?);
        let mut seen = HashSet::new();
        let mut root = None;
        while let Some(off) = next {
            if off >= len || !seen.insert(off) || seen.len() > MAX_SECTIONS {
                return Err(bad_xref("a section is out of the file or loops"));
            }
            let trailer = me.section(off)?;
            if trailer.get("Encrypt").is_some() {
                return Err("the PDF is encrypted".into());
            }
            root = root.or(trailer.get("Root").and_then(Val::reference));
            if let Some(stm) = trailer.get("XRefStm").and_then(Val::int) {
                me.section(u64::try_from(stm).map_err(|_| bad_xref("bad XRefStm"))?)?;
            }
            next = trailer
                .get("Prev")
                .and_then(Val::int)
                .and_then(|p| u64::try_from(p).ok());
        }
        me.root = root.ok_or_else(|| bad_xref("no Root"))?.0;
        me.total = me.count_pages()?;
        Ok(me)
    }

    fn set(&mut self, i: usize, e: Entry) -> Result<(), String> {
        if i >= MAX_OBJECTS {
            return Err(bad_xref("too many objects"));
        }
        if i >= self.entries.len() {
            self.entries.resize(i + 1, Entry::Unset);
        }
        if matches!(self.entries[i], Entry::Unset) {
            self.entries[i] = e;
        }
        Ok(())
    }

    /// Reads the section at `off` into the index and returns its trailer dictionary.
    fn section(&mut self, off: u64) -> Result<Val, String> {
        let head = self.src.read_at(off, 8).map_err(io_err)?;
        if head.starts_with(b"xref") {
            self.table(off + 4)
        } else {
            self.xref_stream(off)
        }
    }

    /// A classic table: subsections of `offset generation n|f` rows, then the trailer.
    fn table(&mut self, off: u64) -> Result<Val, String> {
        self.src.seek(SeekFrom::Start(off)).map_err(io_err)?;
        let mut rd = BufReader::with_capacity(64 * 1024, &mut self.src);
        let mut tok = Vec::new();
        let mut rows: Vec<(usize, Entry)> = Vec::new();
        loop {
            if !token(&mut rd, &mut tok).map_err(io_err)? {
                return Err(bad_xref("the table has no trailer"));
            }
            if tok == b"trailer" {
                break;
            }
            let start = num(&tok).ok_or_else(|| bad_xref("a subsection start"))? as usize;
            token(&mut rd, &mut tok).map_err(io_err)?;
            let count = num(&tok).ok_or_else(|| bad_xref("a subsection count"))? as usize;
            if start.saturating_add(count) > MAX_OBJECTS {
                return Err(bad_xref("too many objects"));
            }
            for i in 0..count {
                let mut field = || -> Result<Vec<u8>, String> {
                    token(&mut rd, &mut tok).map_err(io_err)?;
                    Ok(tok.clone())
                };
                let (a, g, k) = (field()?, field()?, field()?);
                let e = match k.as_slice() {
                    b"n" => Entry::Offset(
                        num(&a).ok_or_else(|| bad_xref("an offset"))?,
                        num(&g).unwrap_or(0) as u16,
                    ),
                    b"f" => Entry::Free,
                    _ => return Err(bad_xref("a row is neither n nor f")),
                };
                rows.push((start + i, e));
            }
        }
        let mut text = Vec::new();
        (&mut rd)
            .take(1 << 20)
            .read_to_end(&mut text)
            .map_err(io_err)?;
        drop(rd);
        for (i, e) in rows {
            self.set(i, e)?;
        }
        parse_value(&text, 0, true).map_err(|_| bad_xref("the trailer"))
    }

    /// A cross-reference stream: packed rows described by `W` and `Index`.
    fn xref_stream(&mut self, off: u64) -> Result<Val, String> {
        let obj = self.object_at(0, off, 0, true)?;
        let at = obj.stream.ok_or_else(|| bad_xref("no stream"))?;
        let raw = self.read_stream(at)?;
        let data = decode_stream(&obj.val, raw)?;
        let ints = |key: &str| -> Option<Vec<usize>> {
            match &obj.val.get(key)?.kind {
                Kind::Arr(items) => items
                    .iter()
                    .map(|v| v.int().map(|n| n.max(0) as usize))
                    .collect(),
                _ => None,
            }
        };
        let w = ints("W").filter(|w| w.len() == 3 && w.iter().all(|&n| n <= 8));
        let w = w.ok_or_else(|| bad_xref("W"))?;
        let size = obj.val.get("Size").and_then(Val::int).unwrap_or(0).max(0) as usize;
        let index = ints("Index").unwrap_or_else(|| vec![0, size]);
        let width = w[0] + w[1] + w[2];
        if width == 0 {
            return Err(bad_xref("empty rows"));
        }
        let field = |row: &[u8], from: usize, n: usize| {
            row[from..from + n]
                .iter()
                .fold(0u64, |a, &b| (a << 8) | u64::from(b))
        };
        let mut rows = data.chunks_exact(width);
        for pair in index.as_chunks::<2>().0 {
            for i in 0..pair[1] {
                let Some(row) = rows.next() else {
                    return Err(bad_xref("the stream ends early"));
                };
                let kind = if w[0] == 0 { 1 } else { field(row, 0, w[0]) };
                let (a, b) = (field(row, w[0], w[1]), field(row, w[0] + w[1], w[2]));
                let e = match kind {
                    0 => Entry::Free,
                    1 => Entry::Offset(a, b as u16),
                    2 => Entry::InStream(a as u32, b as u32),
                    _ => continue,
                };
                self.set(pair[0] + i, e)?;
            }
        }
        Ok(obj.val)
    }

    /// One object, `None` when the index has no live entry for it.
    pub fn object(&mut self, n: u32) -> Result<Option<RawObj>, String> {
        match self.entries.get(n as usize).copied().unwrap_or_default() {
            Entry::Offset(off, rev) => self.object_at(n, off, rev, true).map(Some),
            Entry::InStream(stm, idx) => {
                let os = self.objstm(stm)?;
                let at = match os.index.get(idx as usize) {
                    Some(&(m, _)) if m == n => idx as usize,
                    _ => os
                        .index
                        .iter()
                        .position(|&(m, _)| m == n)
                        .ok_or_else(|| format!("object {n} is not in its object stream"))?,
                };
                let from = os.first + os.index[at].1;
                let to = os
                    .index
                    .get(at + 1)
                    .map_or(os.data.len(), |&(_, o)| os.first + o);
                let text = os
                    .data
                    .get(from..to.max(from))
                    .ok_or_else(|| format!("object {n} is outside its object stream"))?
                    .to_vec();
                let val =
                    parse_value(&text, 0, true).map_err(|_| format!("object {n} is damaged"))?;
                Ok(Some(RawObj {
                    rev: 0,
                    text,
                    val,
                    stream: None,
                }))
            }
            _ => Ok(None),
        }
    }

    /// The object at `off` in the file, checked against `n` unless `n` is 0.
    fn object_at(&mut self, n: u32, off: u64, rev: u16, need_len: bool) -> Result<RawObj, String> {
        let damaged = || format!("object {n} is damaged");
        let mut want = 2048usize;
        loop {
            let buf = self.src.read_at(off, want).map_err(io_err)?;
            let eof = off + buf.len() as u64 >= self.src.len();
            let parsed = (|| -> Result<(Val, Option<usize>), Fail> {
                let a = parse_value(&buf, 0, eof)?;
                let b = parse_value(&buf, a.end, eof)?;
                let c = parse_value(&buf, b.end, eof)?;
                let header_ok = a.int().is_some_and(|x| n == 0 || x == i64::from(n))
                    && b.int().is_some()
                    && matches!(&c.kind, Kind::Word(w) if w == b"obj");
                if !header_ok {
                    return Err(Fail::Bad);
                }
                let val = parse_value(&buf, c.end, eof)?;
                let after = skip_ws(&buf, val.end, eof)?;
                if buf[after..].starts_with(b"stream") {
                    let mut ds = after + 6;
                    match (buf.get(ds), buf.get(ds + 1)) {
                        (Some(b'\r'), Some(b'\n')) => ds += 2,
                        (Some(b'\n' | b'\r'), _) => ds += 1,
                        (None, _) | (Some(_), None) if !eof => return Err(Fail::Incomplete),
                        _ => {}
                    }
                    return Ok((val, Some(ds)));
                }
                if !eof && buf.len() < after + 6 {
                    return Err(Fail::Incomplete);
                }
                Ok((val, None))
            })();
            match parsed {
                Ok((val, ds)) => {
                    let text = buf[val.start..val.end].to_vec();
                    let val = parse_value(&text, 0, true).map_err(|_| damaged())?;
                    let stream = match ds {
                        Some(ds) if need_len => Some(self.stream_at(off + ds as u64, &val)?),
                        Some(ds) => Some(StreamAt {
                            offset: off + ds as u64,
                            len: 0,
                        }),
                        None => None,
                    };
                    return Ok(RawObj {
                        rev,
                        text,
                        val,
                        stream,
                    });
                }
                Err(Fail::Incomplete) if !eof && want < MAX_OBJECT_TEXT => want *= 4,
                Err(_) => return Err(damaged()),
            }
        }
    }

    /// The length of the stream whose data starts at `offset`: the declared one
    /// when `endstream` follows it, else found by scanning for `endstream`.
    fn stream_at(&mut self, offset: u64, dict: &Val) -> Result<StreamAt, String> {
        let declared = match dict.get("Length") {
            Some(v) => match v.kind {
                Kind::Int(n) => u64::try_from(n).ok(),
                Kind::Ref(r, _) => self.integer(r)?,
                _ => None,
            },
            None => None,
        };
        let file_len = self.src.len();
        if let Some(len) = declared.filter(|&l| offset + l <= file_len) {
            let tail = self.src.read_at(offset + len, 32).map_err(io_err)?;
            let skip = tail.iter().take_while(|&&b| is_space(b)).count();
            if tail[skip..].starts_with(b"endstream") || tail[skip..].starts_with(b"endobj") {
                return Ok(StreamAt { offset, len });
            }
        }
        const BLOCK: usize = 1 << 20;
        let (mut at, needle) = (offset, b"endstream");
        while at < file_len {
            let block = self.src.read_at(at, BLOCK + needle.len()).map_err(io_err)?;
            if let Some(i) = block.windows(needle.len()).position(|w| w == needle) {
                let mut len = at + i as u64 - offset;
                let before = self
                    .src
                    .read_at(offset + len.saturating_sub(2), 2)
                    .map_err(io_err)?;
                len -= if before.ends_with(b"\r\n") {
                    2
                } else {
                    u64::from(before.ends_with(b"\n") || before.ends_with(b"\r"))
                };
                return Ok(StreamAt { offset, len });
            }
            at += BLOCK as u64;
        }
        Err("a stream has no end".into())
    }

    /// The integer an indirect object holds (a stream length).
    fn integer(&mut self, n: u32) -> Result<Option<u64>, String> {
        let obj = match self.entries.get(n as usize).copied().unwrap_or_default() {
            // The length of a length object is never needed, which also ends a loop of them.
            Entry::Offset(off, rev) => Some(self.object_at(n, off, rev, false)?),
            _ => self.object(n)?,
        };
        Ok(obj
            .and_then(|o| o.val.int())
            .and_then(|v| u64::try_from(v).ok()))
    }

    pub fn read_stream(&mut self, at: StreamAt) -> Result<Vec<u8>, String> {
        if at.len > STREAM_CAP {
            return Err("a stream is over the size the reader copies".into());
        }
        self.src.read_at(at.offset, at.len as usize).map_err(io_err)
    }

    fn objstm(&mut self, n: u32) -> Result<Rc<ObjStm>, String> {
        if let Some((_, hit)) = self.cache.iter().find(|(m, _)| *m == n) {
            return Ok(Rc::clone(hit));
        }
        let obj = self
            .object(n)?
            .ok_or_else(|| format!("object stream {n} is missing"))?;
        let at = obj
            .stream
            .ok_or_else(|| format!("object stream {n} has no data"))?;
        let raw = self.read_stream(at)?;
        let data = decode_stream(&obj.val, raw)?;
        let count = obj.val.get("N").and_then(Val::int).unwrap_or(0).max(0) as usize;
        let first = obj.val.get("First").and_then(Val::int).unwrap_or(0).max(0) as usize;
        let head = data
            .get(..first)
            .ok_or_else(|| format!("object stream {n} is damaged"))?;
        let numbers: Vec<usize> = head
            .split(|&b| is_space(b))
            .filter(|t| !t.is_empty())
            .map_while(|t| num(t).map(|v| v as usize))
            .collect();
        let index = numbers
            .as_chunks::<2>()
            .0
            .iter()
            .take(count)
            .map(|p| (p[0] as u32, p[1]))
            .collect();
        let os = Rc::new(ObjStm { data, first, index });
        self.cache_bytes += os.data.len();
        self.cache.push((n, Rc::clone(&os)));
        while self.cache_bytes > OBJSTM_CACHE && self.cache.len() > 1 {
            let (_, old) = self.cache.remove(0);
            self.cache_bytes -= old.data.len();
        }
        Ok(os)
    }

    fn count_pages(&mut self) -> Result<u32, String> {
        let root = self
            .object(self.root)?
            .ok_or_else(|| bad_xref("the Root is free"))?;
        let (pages, _) = root
            .val
            .get("Pages")
            .and_then(Val::reference)
            .ok_or_else(|| bad_xref("no Pages"))?;
        let node = self
            .object(pages)?
            .ok_or_else(|| bad_xref("Pages is free"))?;
        let count = node.val.get("Count").and_then(Val::int).unwrap_or(0);
        u32::try_from(count)
            .ok()
            .filter(|&c| c > 0)
            .ok_or_else(|| bad_xref("the page count"))
    }
}

/// Page attributes inherited from the nodes above, as raw value text.
#[derive(Clone, Default)]
struct Inherit(Vec<(&'static str, Vec<u8>)>);

impl Inherit {
    fn below(&self, text: &[u8], val: &Val) -> Self {
        let mut out = self.0.clone();
        for key in INHERITED {
            if let Some(v) = val.get(key) {
                out.retain(|(k, _)| *k != key);
                out.push((key, text[v.start..v.end].to_vec()));
            }
        }
        Self(out)
    }
}

enum Node {
    Tree {
        kids: Vec<(u32, u16)>,
        count: Option<u32>,
        inh: Inherit,
    },
    /// A page; `obj` is `None` when the object could not be read.
    Leaf {
        num: u32,
        obj: Result<RawObj, String>,
        inh: Inherit,
    },
}

impl Lazy {
    fn node(&mut self, (num, _): (u32, u16), above: &Inherit) -> Node {
        let obj = match self.object(num) {
            Ok(Some(o)) => o,
            Ok(None) => {
                return Node::Leaf {
                    num,
                    obj: Err("the page object is missing".into()),
                    inh: above.clone(),
                };
            }
            Err(e) => {
                return Node::Leaf {
                    num,
                    obj: Err(e),
                    inh: above.clone(),
                };
            }
        };
        let inh = above.below(&obj.text, &obj.val);
        match obj.val.get("Kids").map(|k| &k.kind) {
            Some(Kind::Arr(items)) => Node::Tree {
                kids: items.iter().filter_map(Val::reference).collect(),
                count: obj
                    .val
                    .get("Count")
                    .and_then(Val::int)
                    .and_then(|c| u32::try_from(c).ok()),
                inh,
            },
            _ => Node::Leaf {
                num,
                obj: Ok(obj),
                inh,
            },
        }
    }

    fn root_pages(&mut self) -> Result<(u32, u16), String> {
        let root = self
            .object(self.root)?
            .ok_or_else(|| bad_xref("the Root is free"))?;
        root.val
            .get("Pages")
            .and_then(Val::reference)
            .ok_or_else(|| bad_xref("no Pages"))
    }
}

/// An in-order walk over the leaves of the page tree.
struct Walk {
    stack: Vec<Frame>,
    /// The number of the next leaf.
    pos: u32,
}

struct Frame {
    kids: Vec<(u32, u16)>,
    idx: usize,
    inh: Inherit,
}

struct Leaf {
    num: u32,
    obj: Result<RawObj, String>,
    inh: Inherit,
}

impl Walk {
    fn start(lazy: &mut Lazy) -> Result<Self, String> {
        let root = lazy.root_pages()?;
        let frame = match lazy.node(root, &Inherit::default()) {
            Node::Tree { kids, inh, .. } => Frame { kids, idx: 0, inh },
            Node::Leaf { .. } => Frame {
                kids: vec![root],
                idx: 0,
                inh: Inherit::default(),
            },
        };
        Ok(Self {
            stack: vec![frame],
            pos: 1,
        })
    }

    fn push(&mut self, kids: Vec<(u32, u16)>, inh: Inherit) -> Result<(), String> {
        if self.stack.len() >= MAX_DEPTH {
            return Err("the page tree is nested too deeply".into());
        }
        self.stack.push(Frame { kids, idx: 0, inh });
        Ok(())
    }

    /// Positions the walk so the next leaf is page `n`, skipping whole
    /// subtrees by their page counts.
    fn seek(&mut self, lazy: &mut Lazy, n: u32) -> Result<(), String> {
        *self = Self::start(lazy)?;
        loop {
            let Some(top) = self.stack.last_mut() else {
                return Ok(());
            };
            let Some(&kid) = top.kids.get(top.idx) else {
                self.stack.pop();
                continue;
            };
            match lazy.node(kid, &top.inh) {
                Node::Tree { kids, count, inh } => {
                    top.idx += 1;
                    match count {
                        Some(c) if self.pos.saturating_add(c) <= n => self.pos += c,
                        _ => self.push(kids, inh)?,
                    }
                }
                Node::Leaf { .. } if self.pos == n => return Ok(()),
                Node::Leaf { .. } => {
                    top.idx += 1;
                    self.pos += 1;
                }
            }
        }
    }

    fn next(&mut self, lazy: &mut Lazy) -> Result<Option<(u32, Leaf)>, String> {
        loop {
            let Some(top) = self.stack.last_mut() else {
                return Ok(None);
            };
            let Some(&kid) = top.kids.get(top.idx) else {
                self.stack.pop();
                continue;
            };
            top.idx += 1;
            match lazy.node(kid, &top.inh) {
                Node::Tree { kids, inh, .. } => self.push(kids, inh)?,
                Node::Leaf { num, obj, inh } => {
                    let n = self.pos;
                    self.pos += 1;
                    return Ok(Some((n, Leaf { num, obj, inh })));
                }
            }
        }
    }
}

/// `/Name` with the bytes a name cannot hold written as `#xx`.
fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        if b.is_ascii_graphic() && !b"()<>[]{}/%#".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

/// The copies of one window, written as the body of a PDF file.
struct Pool {
    data: Vec<u8>,
    at: BTreeMap<u32, (usize, u16)>,
    seen: HashSet<u32>,
    warnings: Vec<String>,
}

impl Pool {
    fn new() -> Self {
        Self {
            data: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            at: BTreeMap::new(),
            seen: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    fn put(&mut self, num: u32, rev: u16, text: &[u8], stream: Option<&[u8]>) {
        self.at.insert(num, (self.data.len(), rev));
        self.data
            .extend_from_slice(format!("{num} {rev} obj\n").as_bytes());
        self.data.extend_from_slice(text);
        if let Some(s) = stream {
            self.data.extend_from_slice(b"\nstream\n");
            self.data.extend_from_slice(s);
            self.data.extend_from_slice(b"\nendstream");
        }
        self.data.extend_from_slice(b"\nendobj\n");
    }

    /// Copies every object reachable from `roots` that is not copied yet.
    fn closure(&mut self, lazy: &mut Lazy, roots: Vec<(u32, u16)>, page: u32) {
        let mut stack = roots;
        while let Some((n, _)) = stack.pop() {
            if !self.seen.insert(n) {
                continue;
            }
            let obj = match lazy.object(n) {
                Ok(Some(o)) => o,
                Ok(None) => continue,
                Err(e) => {
                    self.warnings.push(format!("page {page}: {e}"));
                    continue;
                }
            };
            if let Kind::Dict(entries) = &obj.val.kind {
                for (k, v) in entries.iter().filter(|(k, _)| k != b"Length") {
                    let _ = k;
                    v.refs(&mut stack);
                }
            } else {
                obj.val.refs(&mut stack);
            }
            let Some(at) = obj.stream else {
                self.put(n, obj.rev, &obj.text, None);
                continue;
            };
            let room = POOL_HARD_CAP.saturating_sub(self.data.len()) as u64;
            let data = if at.len > STREAM_CAP.min(room) {
                None
            } else {
                lazy.read_stream(at).ok()
            };
            let Some(data) = data else {
                self.warnings.push(format!(
                    "page {page}: a stream of {} MiB is over the size the reader copies and was left out",
                    at.len / 1024 / 1024
                ));
                self.put(n, obj.rev, b"<< /Length 0 >>", Some(b""));
                continue;
            };
            let mut text = obj.text.clone();
            let len = data.len().to_string();
            if let Some(v) = obj.val.get("Length") {
                text.splice(v.start..v.end, len.bytes());
            }
            self.put(n, obj.rev, &text, Some(&data));
        }
    }

    /// Writes page `n` with its inherited attributes and its parent, then
    /// everything it uses.
    fn page(&mut self, lazy: &mut Lazy, n: u32, leaf: Leaf, parent: u32) {
        let mut roots = Vec::new();
        let mut text = b"<< /Type /Page".to_vec();
        match &leaf.obj {
            Ok(obj) => {
                if let Kind::Dict(entries) = &obj.val.kind {
                    for (k, v) in entries {
                        if NOT_COPIED.iter().any(|x| x.as_bytes() == k.as_slice()) || k == b"Type" {
                            continue;
                        }
                        text.push(b' ');
                        write_name(&mut text, k.as_slice());
                        text.push(b' ');
                        text.extend_from_slice(&obj.text[v.start..v.end]);
                        v.refs(&mut roots);
                    }
                }
                for (k, raw) in &leaf.inh.0 {
                    if obj.val.get(k).is_none() {
                        text.extend_from_slice(format!(" /{k} ").as_bytes());
                        text.extend_from_slice(raw);
                        if let Ok(v) = parse_value(raw, 0, true) {
                            v.refs(&mut roots);
                        }
                    }
                }
            }
            Err(why) => {
                self.warnings
                    .push(format!("page {n}: the page could not be read: {why}"));
                text.extend_from_slice(b" /MediaBox [0 0 612 792]");
            }
        }
        text.extend_from_slice(format!(" /Parent {parent} 0 R >>").as_bytes());
        let rev = leaf.obj.as_ref().map_or(0, |o| o.rev);
        self.seen.insert(leaf.num);
        self.put(leaf.num, rev, &text, None);
        self.closure(lazy, roots, n);
    }

    /// Adds the page tree and the cross-reference table and returns the file.
    fn finish(mut self, kids: &[u32], pages_num: u32) -> Vec<u8> {
        let list: String = kids.iter().map(|k| format!("{k} 0 R ")).collect();
        self.put(
            pages_num,
            0,
            format!("<< /Type /Pages /Kids [{list}] /Count {} >>", kids.len()).as_bytes(),
            None,
        );
        self.put(
            pages_num + 1,
            0,
            format!("<< /Type /Catalog /Pages {pages_num} 0 R >>").as_bytes(),
            None,
        );
        let xref_at = self.data.len();
        let mut table = String::from("xref\n0 1\n0000000000 65535 f \n");
        let entries: Vec<(u32, (usize, u16))> = self.at.iter().map(|(&n, &e)| (n, e)).collect();
        let mut i = 0;
        while i < entries.len() {
            let run = entries[i..]
                .iter()
                .enumerate()
                .take_while(|(k, (n, _))| *n == entries[i].0 + *k as u32)
                .count();
            table.push_str(&format!("{} {run}\n", entries[i].0));
            for (_, (off, rev)) in &entries[i..i + run] {
                table.push_str(&format!("{off:010} {rev:05} n \n"));
            }
            i += run;
        }
        table.push_str(&format!(
            "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            pages_num + 2,
            pages_num + 1
        ));
        self.data.extend_from_slice(table.as_bytes());
        self.data
    }
}

impl Lazy {
    /// A small PDF of the selected pages from `from` on: up to
    /// [`WINDOW_PAGES`] of them, at least `min_pages` unless the document ends,
    /// and no more once the copies pass [`WINDOW_BYTES`].
    pub fn window(
        &mut self,
        opts: &Options,
        from: u32,
        min_pages: usize,
    ) -> Result<Window, String> {
        let total = self.total;
        let pages_num = self.entries.len() as u32;
        let mut walk = Walk::start(self)?;
        let mut pool = Pool::new();
        let (mut numbers, mut kids) = (Vec::new(), Vec::new());
        let mut cur = from;
        while numbers.len() < WINDOW_PAGES
            && !(numbers.len() >= min_pages && pool.data.len() >= WINDOW_BYTES)
        {
            let Some(n) = opts.next_selected(cur, total) else {
                cur = total + 1;
                break;
            };
            if walk.pos != n {
                walk.seek(self, n)?;
            }
            let Some((m, leaf)) = walk.next(self)? else {
                cur = total + 1;
                break;
            };
            kids.push(leaf.num);
            pool.page(self, m, leaf, pages_num);
            numbers.push(m);
            cur = m + 1;
        }
        let warnings = std::mem::take(&mut pool.warnings);
        Ok(Window {
            bytes: pool.finish(&kids, pages_num),
            numbers,
            after: cur,
            warnings,
        })
    }
}
