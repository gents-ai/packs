//! OpenDocument text, spreadsheets and presentations (ODT, ODS, ODP) from
//! `content.xml`: headings, lists, tables, links, notes and pictures.
use std::collections::HashMap;

use roxmltree::Node;

use crate::ctx::Ctx;
use crate::detect::{Kind, header};
use crate::md::esc;
use crate::model::{Block, DocAcc, Document};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps, Units, with_marker};
use crate::src::Src;
use crate::table::TableWriter;
use crate::util::{MAX_DOM_BYTES, Zip, resolve};
use crate::xml::{attr, child, descendant, is, parse, text};

/// Repeated rows and columns are expanded at most this many times.
const MAX_REPEAT: usize = 50;

struct Walker<'z> {
    ctx: &'z mut Ctx,
    acc: &'z mut DocAcc,
    zip: &'z mut Zip,
    ordered: HashMap<String, bool>,
    blocks: Vec<Block>,
    notes: Vec<String>,
    unit: Option<u32>,
}

/// Inline text of a paragraph, headings and cells: spans, links, tabs, spaces, notes.
fn inline(node: Node<'_, '_>, notes: &mut Vec<String>, out: &mut String) {
    for c in node.children() {
        if c.is_text() {
            out.push_str(&esc(c.text().unwrap_or("")));
        } else if c.is_element() {
            match c.tag_name().name() {
                "s" => out.push_str(
                    &" ".repeat(
                        attr(c, "c")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1usize)
                            .min(64),
                    ),
                ),
                "tab" | "line-break" => out.push(' '),
                "a" => {
                    let mut inner = String::new();
                    inline(c, notes, &mut inner);
                    match attr(c, "href")
                        .filter(|h| h.starts_with("http") && !h.contains([' ', ')']))
                    {
                        Some(h) if !inner.trim().is_empty() => {
                            out.push_str(&format!("[{}]({h})", inner.trim()))
                        }
                        _ => out.push_str(&inner),
                    }
                }
                "note" => {
                    let body = child(c, "note-body")
                        .map(|b| text(b).trim().to_string())
                        .unwrap_or_default();
                    notes.push(body);
                    out.push_str(&format!("[^{}]", notes.len()));
                }
                "frame" | "annotation" | "bookmark" | "bookmark-start" | "bookmark-end" => {}
                _ => inline(c, notes, out),
            }
        }
    }
}

impl<'z> Walker<'z> {
    fn new(
        ctx: &'z mut Ctx,
        acc: &'z mut DocAcc,
        zip: &'z mut Zip,
        ordered: HashMap<String, bool>,
        unit: Option<u32>,
    ) -> Self {
        Self {
            ctx,
            acc,
            zip,
            ordered,
            blocks: Vec::new(),
            notes: Vec::new(),
            unit,
        }
    }
}

impl Walker<'_> {
    fn paragraph_text(&mut self, p: Node<'_, '_>) -> String {
        let mut s = String::new();
        inline(p, &mut self.notes, &mut s);
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn frames(&mut self, p: Node<'_, '_>) {
        for f in p.descendants().filter(|n| is(*n, "frame")) {
            if let Some(img) = child(f, "image") {
                let alt = child(f, "desc")
                    .or_else(|| child(f, "title"))
                    .map(|n| text(n).trim().to_string())
                    .unwrap_or_default();
                self.image(attr(img, "href").unwrap_or(""), alt);
            }
        }
    }

    fn image(&mut self, href: &str, alt: String) {
        if href.is_empty() {
            return;
        }
        let unit = self.unit;
        let caption = String::new();
        let block = match self.zip.read_shared(&resolve("content.xml", href)) {
            Ok(Some(bytes)) if crate::pix::probe(&bytes).is_some() => {
                self.ctx.figure_from_bytes(self.acc, unit, &bytes, caption)
            }
            Ok(Some(_)) => self.ctx.unreadable_figure(
                self.acc,
                unit,
                caption,
                "the image format is not PNG, JPEG, GIF, BMP, TIFF or WebP",
            ),
            _ => {
                self.ctx
                    .unreadable_figure(self.acc, unit, caption, "the image file was not found")
            }
        };
        Ctx::note_alt(self.acc, block.as_ref(), &alt);
        if let Some(b) = block {
            self.blocks.push(b);
        }
    }

    fn list(&mut self, list: Node<'_, '_>, depth: u8) {
        let ordered = attr(list, "style-name")
            .and_then(|s| self.ordered.get(s))
            .copied()
            .unwrap_or(false);
        let mut n = 0;
        for item in list.children().filter(|c| is(*c, "list-item")) {
            n += 1;
            let mut first = true;
            for c in item.children().filter(Node::is_element) {
                match c.tag_name().name() {
                    "p" | "h" => {
                        let t = self.paragraph_text(c);
                        if !t.is_empty() {
                            let marker = if ordered { format!("{n}.") } else { "-".into() };
                            self.blocks.push(if first {
                                Block::Item {
                                    depth,
                                    marker,
                                    text: t,
                                }
                            } else {
                                Block::Para(t)
                            });
                            first = false;
                        }
                        self.frames(c);
                    }
                    "list" => self.list(c, (depth + 1).min(6)),
                    _ => {}
                }
            }
        }
    }

    /// Cell texts of one row, with repeated cells expanded and trailing empties dropped.
    fn row_cells(&mut self, tr: Node<'_, '_>) -> Vec<String> {
        let mut cells = Vec::new();
        for tc in tr
            .children()
            .filter(|c| is(*c, "table-cell") || is(*c, "covered-table-cell"))
        {
            let parts: Vec<String> = tc
                .children()
                .filter(|p| is(*p, "p") || is(*p, "h"))
                .map(|p| self.paragraph_text(p))
                .collect();
            let t = parts.join(" ");
            let rep: usize = attr(tc, "number-columns-repeated")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1usize);
            let rep = rep.min(MAX_REPEAT);
            cells.extend(std::iter::repeat_n(t, rep));
        }
        while cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        cells
    }

    fn table(&mut self, tbl: Node<'_, '_>) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for tr in tbl.descendants().filter(|n| is(*n, "table-row")) {
            if tr.ancestors().skip(1).filter(|a| is(*a, "table")).count() > 1 {
                continue;
            }
            let cells = self.row_cells(tr);
            let rep: usize = attr(tr, "number-rows-repeated")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1usize);
            if cells.iter().any(|c| !c.is_empty()) {
                for _ in 0..rep.min(MAX_REPEAT) {
                    rows.push(cells.clone());
                }
            }
        }
        rows
    }

    fn body(&mut self, node: Node<'_, '_>) {
        for c in node.children().filter(Node::is_element) {
            if self.acc.truncated {
                return;
            }
            match c.tag_name().name() {
                "h" => {
                    let level = attr(c, "outline-level")
                        .and_then(|l| l.parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 6);
                    let t = self.paragraph_text(c);
                    if !t.is_empty() {
                        self.blocks.push(Block::Heading(level, t));
                    }
                    self.frames(c);
                }
                "p" => {
                    let t = self.paragraph_text(c);
                    if !t.is_empty() {
                        self.blocks.push(Block::Para(t));
                    }
                    self.frames(c);
                }
                "list" => self.list(c, 0),
                "table" => {
                    let rows = self.table(c);
                    if !rows.is_empty() {
                        self.blocks.push(Block::Table(rows));
                    }
                }
                "frame" => {
                    self.frames_of(c);
                    for tb in c.descendants().filter(|n| is(*n, "text-box")) {
                        self.body(tb);
                    }
                }
                "section" | "text-box" | "page-thread" | "sequence-decls" => self.body(c),
                _ => {}
            }
        }
    }

    fn frames_of(&mut self, frame: Node<'_, '_>) {
        if let Some(img) = child(frame, "image") {
            let alt = child(frame, "desc")
                .or_else(|| child(frame, "title"))
                .map(|n| text(n).trim().to_string())
                .unwrap_or_default();
            self.image(attr(img, "href").unwrap_or(""), alt);
        }
    }
}

/// The sheets of a spreadsheet, one unit each, written row by row.
struct Sheets<'a, 'd> {
    ctx: &'a mut Ctx,
    acc: &'a mut DocAcc,
    zip: Zip,
    tables: Vec<Node<'d, 'd>>,
    /// The sheet to read next, the row to start at and the header of the table.
    unit: u32,
    row: u64,
    hdr: String,
}

impl Steps for Sheets<'_, '_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.ctx, &mut *self.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: self.unit,
            pos: self.row,
            st: Some(serde_json::json!({ "hdr": self.hdr })),
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        loop {
            let Some(&sheet) = self.tables.get((self.unit - 1) as usize) else {
                return Ok(None);
            };
            let n = self.unit;
            if !self.ctx.opts.selected(n) {
                self.unit += 1;
                continue;
            }
            let rows = Walker::new(
                &mut *self.ctx,
                &mut *self.acc,
                &mut self.zip,
                HashMap::new(),
                Some(n),
            )
            .table(sheet);
            let fresh = self.row == 0;
            let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
            let budget = self.ctx.budget.remaining().saturating_sub(4096);
            let mut tw = TableWriter::new(cols, budget, fresh);
            let mut at = (self.row as usize).min(rows.len());
            if fresh && let Some(first) = rows.first() {
                self.hdr = TableWriter::header_of(cols, first);
            }
            while let Some(r) = rows.get(at) {
                if !tw.row(r) {
                    break;
                }
                at += 1;
            }
            if tw.rows == 0 && at < rows.len() {
                if self.ctx.emitted > 0 {
                    return Ok(Some(Step::Wait));
                }
                self.acc.warn(format!(
                    "sheet {n}: a row larger than the output limit was skipped"
                ));
                self.row = at as u64 + 1;
                continue;
            }
            let name = attr(sheet, "name").unwrap_or("").replace('>', "&gt;");
            let marker = format!("<!-- sheet {n}: {name} -->");
            let md = tw.finish();
            let body = match (fresh, md.is_empty()) {
                (true, true) => with_marker(&marker, "(empty sheet)"),
                (true, false) => with_marker(&marker, &md),
                (false, _) => md,
            };
            if at < rows.len() {
                let snap = Snap {
                    unit: n,
                    pos: at as u64,
                    st: Some(serde_json::json!({ "hdr": self.hdr })),
                };
                return Ok(Some(Step::Stop(body, snap, 1)));
            }
            self.unit += 1;
            self.row = 0;
            self.hdr.clear();
            return Ok(Some(Step::Chunk(body)));
        }
    }
}

/// The text of `content.xml`, which is parsed as one tree and so is bounded.
fn content_text(zip: &mut Zip) -> Result<String, String> {
    let content = zip
        .read_limited("content.xml", MAX_DOM_BYTES)
        .map_err(|_| {
            format!(
                "the document text is over the {} MiB this reader handles; export it as CSV, XLSX or text",
                MAX_DOM_BYTES / 1024 / 1024
            )
        })?
        .ok_or("the file has no content.xml")?;
    Ok(crate::util::decode_text(&content).into_owned())
}

/// The slides or sheets of a presentation or spreadsheet nodes below `body`.
fn unit_nodes<'d>(kind: Kind, body: Node<'d, 'd>) -> Vec<Node<'d, 'd>> {
    let (parent, tag) = match kind {
        Kind::Odp => ("presentation", "page"),
        Kind::Ods => ("spreadsheet", "table"),
        _ => return Vec::new(),
    };
    descendant(body, parent)
        .map(|p| p.children().filter(|c| is(*c, tag)).collect())
        .unwrap_or_default()
}

/// The number of slides or sheets (1 for a text document), without reading them.
pub fn unit_count(kind: Kind, src: &Src) -> Result<u32, String> {
    if kind == Kind::Odt {
        return Ok(1);
    }
    let text = content_text(&mut Zip::open(src.reopen()?)?)?;
    let doc = parse(&text)?;
    let body = child(doc.root_element(), "body").ok_or("the file has no document body")?;
    Ok(unit_nodes(kind, body).len() as u32)
}

pub fn convert_odf(
    ctx: &mut Ctx,
    source: &str,
    kind: Kind,
    src: &Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let mut zip = Zip::open(src.reopen()?)?;
    let text = content_text(&mut zip)?;
    let doc = parse(&text)?;
    let office_body = child(doc.root_element(), "body").ok_or("the file has no document body")?;
    let mut ordered = HashMap::new();
    for ls in doc.descendants().filter(|n| is(*n, "list-style")) {
        if let Some(name) = attr(ls, "name") {
            ordered.insert(
                name.to_string(),
                descendant(ls, "list-level-style-number").is_some(),
            );
        }
    }
    let format = kind.name();
    let pages = unit_nodes(kind, office_body);
    let total = if kind == Kind::Odt {
        1
    } else {
        pages.len() as u32
    };
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &header(source, format));
            if kind != Kind::Odt && ctx.opts.selects_none(total) {
                acc.warn(format!(
                    "pages selects nothing: the document has {total} page(s)"
                ));
            }
        }
    }
    let skip = resume.map_or(0, |r| r.skip);
    let unit = resume.map_or(1, |r| r.unit.max(1));
    let mut table_header = None;
    let next = match kind {
        Kind::Odt => {
            let text_body = child(office_body, "text");
            let mut reader = Units::new(
                &mut *ctx,
                &mut acc,
                unit,
                1,
                |ctx: &mut Ctx, acc: &mut DocAcc, _| {
                    let mut w = Walker::new(ctx, acc, &mut zip, ordered.clone(), Some(1));
                    if let Some(text_body) = text_body {
                        w.body(text_body);
                    }
                    for (i, note) in std::mem::take(&mut w.notes).into_iter().enumerate() {
                        w.blocks
                            .push(Block::Raw(format!("[^{}]: {}", i + 1, esc(&note))));
                    }
                    crate::md::attach_captions(&mut w.blocks, w.acc);
                    Ok(crate::md::render(&w.blocks))
                },
            )
            .unfiltered();
            slicer::run(&mut reader, skip)?
        }
        Kind::Odp => {
            let mut reader = Units::new(
                &mut *ctx,
                &mut acc,
                unit,
                total,
                |ctx: &mut Ctx, acc: &mut DocAcc, n| {
                    let mut w = Walker::new(ctx, acc, &mut zip, ordered.clone(), Some(n));
                    w.body(pages[(n - 1) as usize]);
                    crate::md::attach_captions(&mut w.blocks, w.acc);
                    Ok(with_marker(
                        &format!("<!-- slide {n} -->"),
                        &crate::md::render(&w.blocks),
                    ))
                },
            );
            slicer::run(&mut reader, skip)?
        }
        _ => {
            table_header = resume
                .and_then(|r| r.st.as_ref())
                .and_then(|v| v.get("hdr"))
                .and_then(|h| h.as_str())
                .filter(|h| !h.is_empty())
                .map(str::to_string);
            let mut reader = Sheets {
                ctx: &mut *ctx,
                acc: &mut acc,
                zip,
                tables: pages,
                unit,
                row: resume.map_or(0, |r| r.pos),
                hdr: table_header.clone().unwrap_or_default(),
            };
            slicer::run(&mut reader, skip)?
        }
    };
    let mut out = ctx.document(acc, source, format, total, resume, next, parts_from);
    out.table_header = table_header;
    Ok(out)
}
