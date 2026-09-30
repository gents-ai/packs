//! OpenDocument text, spreadsheets and presentations (ODT, ODS, ODP) from
//! `content.xml`: headings, lists, tables, links, notes and pictures.
use std::collections::HashMap;

use roxmltree::Node;

use crate::ctx::Ctx;
use crate::detect::{Kind, header};
use crate::md::esc;
use crate::model::{Block, DocAcc, Document};
use crate::table::TableWriter;
use crate::util::{Zip, resolve};
use crate::xml::{attr, child, descendant, is, parse, text};

/// Repeated rows and columns are expanded at most this many times.
const MAX_REPEAT: usize = 50;

struct Walker<'z, 'a> {
    ctx: &'z mut Ctx,
    acc: &'z mut DocAcc,
    zip: &'z mut Zip<'a>,
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

impl Walker<'_, '_> {
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
        let block = match self.zip.read(&resolve("content.xml", href)) {
            Ok(Some(bytes)) if crate::pix::probe(&bytes).is_some() => {
                self.ctx.figure_from_bytes(self.acc, unit, &bytes, alt)
            }
            Ok(Some(_)) => self.ctx.unreadable_figure(
                self.acc,
                unit,
                alt,
                "the image format is not PNG, JPEG, GIF, BMP, TIFF or WebP",
            ),
            _ => self
                .ctx
                .unreadable_figure(self.acc, unit, alt, "the image file was not found"),
        };
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

pub fn convert_odf(
    ctx: &mut Ctx,
    source: &str,
    kind: Kind,
    data: &[u8],
) -> Result<Document, String> {
    let mut zip = Zip::open(data)?;
    let src = zip
        .read_text("content.xml")?
        .ok_or("the file has no content.xml")?;
    let doc = parse(&src)?;
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
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, format));
    let mut units: Vec<(String, Vec<Block>)> = Vec::new();
    let mut tables: Vec<(String, String, bool, usize)> = Vec::new();
    let mut total = 1u32;
    {
        let mut w = Walker {
            ctx: &mut *ctx,
            acc: &mut acc,
            zip: &mut zip,
            ordered,
            blocks: Vec::new(),
            notes: Vec::new(),
            unit: Some(1),
        };
        match kind {
            Kind::Odt => {
                if let Some(text_body) = child(office_body, "text") {
                    w.body(text_body);
                }
                for (i, note) in std::mem::take(&mut w.notes).into_iter().enumerate() {
                    w.blocks
                        .push(Block::Raw(format!("[^{}]: {}", i + 1, esc(&note))));
                }
                crate::md::attach_captions(&mut w.blocks, &mut w.acc.figures);
                units.push((String::new(), std::mem::take(&mut w.blocks)));
            }
            Kind::Odp => {
                let pages: Vec<Node<'_, '_>> = descendant(office_body, "presentation")
                    .map(|p| p.children().filter(|c| is(*c, "page")).collect())
                    .unwrap_or_default();
                total = pages.len() as u32;
                for (k, page) in pages.into_iter().enumerate() {
                    let n = k as u32 + 1;
                    if !w.ctx.opts.selected(n) {
                        continue;
                    }
                    w.unit = Some(n);
                    w.body(page);
                    crate::md::attach_captions(&mut w.blocks, &mut w.acc.figures);
                    units.push((format!("<!-- slide {n} -->"), std::mem::take(&mut w.blocks)));
                }
            }
            _ => {
                let sheets: Vec<Node<'_, '_>> = descendant(office_body, "spreadsheet")
                    .map(|p| p.children().filter(|c| is(*c, "table")).collect())
                    .unwrap_or_default();
                total = sheets.len() as u32;
                for (k, sheet) in sheets.into_iter().enumerate() {
                    let n = k as u32 + 1;
                    if !w.ctx.opts.selected(n) {
                        continue;
                    }
                    let rows = w.table(sheet);
                    let name = attr(sheet, "name").unwrap_or("").replace('>', "&gt;");
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let mut tw =
                        TableWriter::new(cols, w.ctx.budget.remaining().saturating_sub(4096));
                    for r in &rows {
                        if !tw.row(r) {
                            break;
                        }
                    }
                    tables.push((
                        format!("<!-- sheet {n}: {name} -->"),
                        String::new(),
                        tw.full,
                        tw.rows,
                    ));
                    let md = tw.finish();
                    if let Some(last) = tables.last_mut() {
                        last.1 = md;
                    }
                }
            }
        }
    }
    let mut selected = units.len() + tables.len();
    if matches!(kind, Kind::Odt) {
        selected = 1;
    }
    for (marker, blocks) in units {
        let body = crate::md::render(&blocks);
        let chunk = match (marker.is_empty(), body.is_empty()) {
            (true, _) => body,
            (false, true) => marker,
            (false, false) => format!("{marker}\n\n{body}"),
        };
        if !ctx.append(&mut acc, &chunk) {
            acc.warn("the output size limit was reached and the rest of the document was cut");
            break;
        }
    }
    for (marker, md, full, rows) in tables {
        if full {
            acc.warn(format!(
                "{marker}: the output size limit cut the table after {rows} row(s)"
            ));
        }
        let chunk = if md.is_empty() {
            format!("{marker}\n\n(empty sheet)")
        } else {
            format!("{marker}\n\n{md}")
        };
        if !ctx.append(&mut acc, &chunk) {
            acc.warn("the output size limit was reached and the remaining sheets were cut");
            break;
        }
    }
    if selected == 0 {
        acc.warn(format!(
            "pages selects nothing: the document has {total} page(s)"
        ));
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format,
        pages: total,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}
