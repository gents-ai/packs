//! DOCX: body paragraphs with heading styles, lists, tables, links, footnotes
//! and images (with their alt text as the figure caption).
use std::collections::HashMap;

use roxmltree::Node;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::md::esc;
use crate::model::{Block, DocAcc, Document};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps};
use crate::src::Src;
use crate::util::{Entry, Zip, resolve};
use crate::xml::{attr, child, is, parse, text};
use crate::xmlfrag::{Frags, attribute, container, scan};
use serde::{Deserialize, Serialize};

const PART: &str = "word/document.xml";

struct Style {
    /// Lowercase display name.
    name: String,
    based_on: Option<String>,
    /// The (numId, ilvl) list the style itself applies.
    num: Option<(String, String)>,
}

#[derive(Default)]
struct Styles {
    by_id: HashMap<String, Style>,
}

impl Styles {
    fn heading_level(&self, id: &str) -> Option<u8> {
        let mut cur = id.to_string();
        for _ in 0..6 {
            let Style { name, based_on, .. } = self.by_id.get(&cur)?;
            let compact = name.replace(' ', "");
            if let Some(n) = compact
                .strip_prefix("heading")
                .and_then(|n| n.parse::<u8>().ok())
            {
                return Some(n.clamp(1, 6));
            }
            match compact.as_str() {
                "title" => return Some(1),
                "subtitle" => return Some(2),
                _ => {}
            }
            cur = based_on.clone()?;
        }
        None
    }

    /// Whether the style is, or derives from, the Word "Title" style.
    fn is_title(&self, id: &str) -> bool {
        let mut cur = id.to_string();
        for _ in 0..6 {
            let Some(Style { name, based_on, .. }) = self.by_id.get(&cur) else {
                return false;
            };
            if name == "title" {
                return true;
            }
            match based_on {
                Some(b) => cur = b.clone(),
                None => return false,
            }
        }
        false
    }

    fn name(&self, id: &str) -> String {
        self.by_id
            .get(id)
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    /// The list a paragraph style applies, looking through `basedOn`.
    fn list_of(&self, id: &str) -> Option<(String, String)> {
        let mut cur = id.to_string();
        for _ in 0..6 {
            let style = self.by_id.get(&cur)?;
            if style.num.is_some() {
                return style.num.clone();
            }
            cur = style.based_on.clone()?;
        }
        None
    }
}

#[derive(Default)]
struct Numbering {
    /// numId -> abstractNumId
    nums: HashMap<String, String>,
    /// (abstractNumId, ilvl) -> ordered?
    ordered: HashMap<(String, String), bool>,
}

struct Rel {
    target: String,
    external: bool,
}

struct Walker<'z> {
    ctx: &'z mut Ctx,
    acc: &'z mut DocAcc,
    zip: &'z mut Zip,
    styles: Styles,
    numbering: Numbering,
    rels: HashMap<String, Rel>,
    counters: HashMap<String, Vec<u32>>,
    footnotes: HashMap<String, String>,
    used_notes: Vec<String>,
    blocks: Vec<Block>,
    /// Consecutive "List Number" paragraphs so far.
    style_run: u32,
    has_title: bool,
}

/// Source bytes of XML read into one batch of body elements.
const BATCH_BYTES: usize = 256 * 1024;
const FOOTNOTE_BATCH: usize = 1024 * 1024;

fn parse_styles(zip: &mut Zip) -> Result<Styles, String> {
    let mut styles = Styles::default();
    if let Some(src) = zip.read_text("word/styles.xml")? {
        let doc = parse(&src)?;
        for s in doc.descendants().filter(|n| is(*n, "style")) {
            let Some(id) = attr(s, "styleId") else {
                continue;
            };
            let name = child(s, "name")
                .and_then(|n| attr(n, "val"))
                .unwrap_or("")
                .to_lowercase();
            let based_on = child(s, "basedOn")
                .and_then(|n| attr(n, "val"))
                .map(str::to_string);
            let num = child(s, "pPr")
                .and_then(|p| child(p, "numPr"))
                .and_then(|n| {
                    let num_id = child(n, "numId").and_then(|v| attr(v, "val"))?;
                    let ilvl = child(n, "ilvl").and_then(|v| attr(v, "val")).unwrap_or("0");
                    Some((num_id.to_string(), ilvl.to_string()))
                });
            styles.by_id.insert(
                id.to_string(),
                Style {
                    name,
                    based_on,
                    num,
                },
            );
        }
    }
    Ok(styles)
}

fn parse_numbering(zip: &mut Zip) -> Result<Numbering, String> {
    let mut numbering = Numbering::default();
    if let Some(src) = zip.read_text("word/numbering.xml")? {
        let doc = parse(&src)?;
        for a in doc.descendants().filter(|n| is(*n, "abstractNum")) {
            let Some(aid) = attr(a, "abstractNumId") else {
                continue;
            };
            for lvl in a.children().filter(|n| is(*n, "lvl")) {
                let fmt = child(lvl, "numFmt")
                    .and_then(|n| attr(n, "val"))
                    .unwrap_or("bullet");
                numbering.ordered.insert(
                    (
                        aid.to_string(),
                        attr(lvl, "ilvl").unwrap_or("0").to_string(),
                    ),
                    fmt != "bullet" && fmt != "none",
                );
            }
        }
        for n in doc.descendants().filter(|n| is(*n, "num")) {
            if let (Some(id), Some(aid)) = (
                attr(n, "numId"),
                child(n, "abstractNumId").and_then(|a| attr(a, "val")),
            ) {
                numbering.nums.insert(id.to_string(), aid.to_string());
            }
        }
    }
    Ok(numbering)
}

fn parse_rels(zip: &mut Zip) -> Result<HashMap<String, Rel>, String> {
    let mut rels = HashMap::new();
    if let Some(src) = zip.read_text("word/_rels/document.xml.rels")? {
        let doc = parse(&src)?;
        for r in doc.descendants().filter(|n| is(*n, "Relationship")) {
            if let (Some(id), Some(target)) = (attr(r, "Id"), attr(r, "Target")) {
                rels.insert(
                    id.to_string(),
                    Rel {
                        target: target.to_string(),
                        external: attr(r, "TargetMode") == Some("External"),
                    },
                );
            }
        }
    }
    Ok(rels)
}

/// Footnote texts kept in memory at most this many bytes in all.
const NOTES_CAP_BYTES: usize = 8 * 1024 * 1024;

/// Footnote texts by id, read as a stream; ones past the memory cap are counted, not kept.
fn parse_footnotes(zip: &mut Zip) -> Result<(HashMap<String, String>, usize), String> {
    let mut notes = HashMap::new();
    let (mut bytes, mut dropped) = (0usize, 0usize);
    let Some(entry) = zip.stream("word/footnotes.xml")? else {
        return Ok((notes, 0));
    };
    let mut frags = Frags::open(entry, &[("footnotes", 0)], &[], None)?;
    while let Some(batch) = frags.next_batch(FOOTNOTE_BATCH)? {
        let doc = parse(&batch.xml)?;
        for n in container(&doc, batch.depth)
            .children()
            .filter(|n| is(*n, "footnote"))
        {
            if matches!(
                attr(n, "type"),
                Some("separator" | "continuationSeparator" | "continuationNotice")
            ) {
                continue;
            }
            if let Some(id) = attr(n, "id") {
                let t: Vec<String> = n
                    .descendants()
                    .filter(|p| is(*p, "p"))
                    .map(|p| text_of_para(p))
                    .collect();
                let t = t.join(" ").trim().to_string();
                if bytes + t.len() > NOTES_CAP_BYTES {
                    dropped += 1;
                    continue;
                }
                bytes += t.len();
                notes.insert(id.to_string(), t);
            }
        }
    }
    Ok((notes, dropped))
}

/// Plain text of a paragraph's runs, for table cells and notes.
fn text_of_para(p: Node<'_, '_>) -> String {
    let mut out = String::new();
    for n in p.descendants() {
        if is(n, "t") || (n.is_element() && n.tag_name().name() == "delText") {
            if !n.ancestors().any(|a| is(a, "del")) {
                out.push_str(n.text().unwrap_or(""));
            }
        } else if is(n, "tab") || is(n, "br") {
            out.push(' ');
        }
    }
    out
}

#[derive(Clone, PartialEq)]
struct Run {
    text: String,
    bold: bool,
    italic: bool,
    link: Option<String>,
}

fn flag(rpr: Option<Node<'_, '_>>, name: &str) -> bool {
    rpr.and_then(|r| child(r, name))
        .is_some_and(|n| !matches!(attr(n, "val"), Some("0" | "false" | "off")))
}

fn emit_runs(runs: &[Run]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < runs.len() {
        let mut j = i;
        let mut group = String::new();
        while j < runs.len()
            && runs[j].bold == runs[i].bold
            && runs[j].italic == runs[i].italic
            && runs[j].link == runs[i].link
        {
            group.push_str(&runs[j].text);
            j += 1;
        }
        let core = group.trim();
        if core.is_empty() {
            out.push_str(&group);
        } else {
            if group.starts_with(' ') {
                out.push(' ');
            }
            let mut piece = esc(core);
            if runs[i].bold {
                piece = format!("**{piece}**");
            }
            if runs[i].italic {
                piece = format!("*{piece}*");
            }
            if let Some(url) = &runs[i].link {
                piece = format!("[{piece}]({url})");
            }
            out.push_str(&piece);
            if group.ends_with(' ') {
                out.push(' ');
            }
        }
        i = j;
    }
    out
}

impl Walker<'_> {
    fn collect_runs(
        &mut self,
        node: Node<'_, '_>,
        link: Option<&str>,
        runs: &mut Vec<Run>,
        images: &mut Vec<(String, String)>,
    ) {
        for c in node.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "r" => self.run(c, link, runs, images),
                "hyperlink" => {
                    let url = attr(c, "id")
                        .and_then(|id| self.rels.get(id))
                        .filter(|r| r.external)
                        .map(|r| r.target.clone())
                        .filter(|u| {
                            (u.starts_with("http") || u.starts_with("mailto:"))
                                && !u.contains([' ', ')'])
                        });
                    self.collect_runs(c, url.as_deref().or(link), runs, images);
                }
                "ins" | "smartTag" | "fldSimple" | "sdt" | "sdtContent" | "customXml" => {
                    self.collect_runs(c, link, runs, images)
                }
                "oMath" | "oMathPara" => runs.push(Run {
                    text: format!(" {} ", text(c).trim()),
                    bold: false,
                    italic: false,
                    link: None,
                }),
                _ => {}
            }
        }
    }

    fn run(
        &mut self,
        r: Node<'_, '_>,
        link: Option<&str>,
        runs: &mut Vec<Run>,
        images: &mut Vec<(String, String)>,
    ) {
        let rpr = child(r, "rPr");
        let (bold, italic) = (flag(rpr, "b"), flag(rpr, "i"));
        let mut t = String::new();
        for n in r.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "t" => t.push_str(n.text().unwrap_or("")),
                "tab" | "br" | "cr" | "noBreakHyphen" => t.push(' '),
                "footnoteReference" => {
                    if let Some(id) = attr(n, "id")
                        && self.footnotes.contains_key(id)
                    {
                        if !self.used_notes.iter().any(|u| u == id) {
                            self.used_notes.push(id.to_string());
                        }
                        t.push_str(&format!("[^{id}]"));
                    }
                }
                "drawing" | "pict" | "object" => {
                    for blip in n
                        .descendants()
                        .filter(|d| is(*d, "blip") || is(*d, "imagedata"))
                    {
                        if let Some(rid) = attr(blip, "embed").or_else(|| attr(blip, "id")) {
                            let alt = n
                                .descendants()
                                .find(|d| is(*d, "docPr"))
                                .and_then(|d| {
                                    attr(d, "descr")
                                        .filter(|s| !s.is_empty())
                                        .or_else(|| attr(d, "title"))
                                        .filter(|s| !s.is_empty())
                                })
                                .unwrap_or("");
                            images.push((rid.to_string(), alt.to_string()));
                        }
                    }
                    for tb in n.descendants().filter(|d| is(*d, "txbxContent")) {
                        t.push(' ');
                        t.push_str(&text(tb));
                    }
                }
                _ => {}
            }
        }
        if !t.is_empty() {
            runs.push(Run {
                text: t,
                bold,
                italic,
                link: link.map(str::to_string),
            });
        }
    }

    /// The list a paragraph belongs to: its own numbering, else its style's.
    fn list_of(&self, ppr: Option<Node<'_, '_>>, style_id: &str) -> Option<(String, String)> {
        let own = ppr.and_then(|x| child(x, "numPr")).and_then(|n| {
            let num_id = child(n, "numId").and_then(|v| attr(v, "val"))?;
            let ilvl = child(n, "ilvl").and_then(|v| attr(v, "val")).unwrap_or("0");
            Some((num_id.to_string(), ilvl.to_string()))
        });
        own.or_else(|| self.styles.list_of(style_id))
            .filter(|(id, _)| id != "0")
    }

    fn list_marker(&mut self, num_id: &str, ilvl: &str) -> Option<(u8, String)> {
        let level: usize = ilvl.parse().unwrap_or(0).min(6);
        let ordered = self
            .numbering
            .nums
            .get(num_id)
            .and_then(|a| self.numbering.ordered.get(&(a.clone(), ilvl.to_string())))
            .copied()
            .unwrap_or(false);
        let counters = self.counters.entry(num_id.to_string()).or_default();
        counters.resize(level + 1, 0);
        counters[level] += 1;
        counters.truncate(level + 1);
        Some((
            level as u8,
            if ordered {
                format!("{}.", counters[level])
            } else {
                "-".to_string()
            },
        ))
    }

    fn paragraph(&mut self, p: Node<'_, '_>, unit: Option<u32>) {
        let ppr = child(p, "pPr");
        let style_id = ppr
            .and_then(|x| child(x, "pStyle"))
            .and_then(|s| attr(s, "val"))
            .unwrap_or("");
        let mut runs = Vec::new();
        let mut images = Vec::new();
        self.collect_runs(p, None, &mut runs, &mut images);
        let body = emit_runs(&runs);
        let body = body.trim();
        if !body.is_empty() {
            let outline = ppr
                .and_then(|x| child(x, "outlineLvl"))
                .and_then(|n| attr(n, "val"))
                .and_then(|v| v.parse::<u8>().ok())
                .map(|l| l + 1);
            let is_title = self.styles.is_title(style_id);
            // With a document title at #, section headings start one level below it.
            let shift = u8::from(self.has_title && !is_title);
            let subtitle = self.has_title && self.styles.name(style_id) == "subtitle";
            let level = self
                .styles
                .heading_level(style_id)
                .or(outline)
                .map(|l| l + shift)
                .filter(|_| !subtitle);
            let list = self
                .list_of(ppr, style_id)
                .and_then(|(id, lvl)| self.list_marker(&id, &lvl));
            let style_name = self.styles.name(style_id);
            if let Some(level) = level {
                let plain: Vec<Run> = runs
                    .iter()
                    .map(|r| Run {
                        bold: false,
                        italic: false,
                        link: None,
                        ..r.clone()
                    })
                    .collect();
                self.blocks.push(Block::Heading(
                    level.min(6),
                    emit_runs(&plain).trim().to_string(),
                ));
            } else if let Some((depth, marker)) = list {
                self.blocks.push(Block::Item {
                    depth,
                    marker,
                    text: body.to_string(),
                });
            } else if style_name.starts_with("list bullet") || style_name.starts_with("list number")
            {
                // A list style with no numbering definition behind it: count the run ourselves.
                let numbered = style_name.starts_with("list number");
                self.style_run = if numbered { self.style_run + 1 } else { 0 };
                let marker = if numbered {
                    format!("{}.", self.style_run)
                } else {
                    "-".to_string()
                };
                self.blocks.push(Block::Item {
                    depth: 0,
                    marker,
                    text: body.to_string(),
                });
            } else if style_name == "caption" {
                self.blocks.push(Block::Caption(body.to_string()));
            } else if style_name == "quote" || style_name == "intense quote" {
                self.blocks
                    .push(Block::Quote(vec![Block::Para(body.to_string())]));
            } else {
                self.blocks.push(Block::Para(body.to_string()));
            }
        }
        for (rid, alt) in images {
            self.image(&rid, alt, unit);
        }
    }

    fn image(&mut self, rid: &str, alt: String, unit: Option<u32>) {
        if self.ctx.must_wait() {
            self.ctx.waiting = true;
            return;
        }
        let Some(rel) = self.rels.get(rid) else {
            return;
        };
        if rel.external {
            self.acc.warn(format!(
                "an image links to {} outside the document and was not fetched",
                rel.target.chars().take(80).collect::<String>()
            ));
            return;
        }
        let path = resolve(PART, &rel.target);
        let caption = String::new();
        let block = match self.zip.read_shared(&path) {
            Ok(Some(bytes)) if crate::pix::probe(&bytes).is_some() => {
                self.ctx.figure_from_bytes(self.acc, unit, &bytes, caption)
            }
            Ok(Some(_)) => self.ctx.unreadable_figure(
                self.acc,
                unit,
                caption,
                "the image format is not PNG, JPEG, GIF, BMP, TIFF or WebP",
            ),
            Ok(None) | Err(_) => {
                self.ctx
                    .unreadable_figure(self.acc, unit, caption, "the image file was not found")
            }
        };
        Ctx::note_alt(self.acc, block.as_ref(), &alt);
        if let Some(b) = block {
            self.blocks.push(b);
        }
    }

    fn table(&mut self, tbl: Node<'_, '_>) {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for tr in tbl.children().filter(|n| is(*n, "tr")) {
            let mut cells = Vec::new();
            for tc in tr.children().filter(|n| is(*n, "tc")) {
                let t: Vec<String> = tc
                    .descendants()
                    .filter(|p| is(*p, "p"))
                    .map(text_of_para)
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                cells.push(t.join(" ").split_whitespace().collect::<Vec<_>>().join(" "));
                let span: usize = child(tc, "tcPr")
                    .and_then(|p| child(p, "gridSpan"))
                    .and_then(|g| attr(g, "val"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1usize)
                    .clamp(1, 64);
                cells.extend(std::iter::repeat_n(String::new(), span - 1));
            }
            rows.push(cells);
        }
        rows.retain(|r| r.iter().any(|c| !c.is_empty()));
        if !rows.is_empty() {
            self.blocks.push(Block::Table(rows));
        }
    }

    fn body(&mut self, node: Node<'_, '_>, unit: Option<u32>) {
        for c in node.children().filter(Node::is_element) {
            if self.acc.truncated || self.ctx.waiting {
                return;
            }
            match c.tag_name().name() {
                "p" => self.paragraph(c, unit),
                "tbl" => self.table(c),
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.body(content, unit);
                    }
                }
                "AlternateContent" => {
                    if let Some(choice) = child(c, "Choice") {
                        self.body(choice, unit);
                    }
                }
                _ => {}
            }
        }
    }
}

/// What a call carries to the next: list counters, the notes cited so far and
/// the heading shift, so a slice reads exactly as it would inside one call.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct State {
    counters: HashMap<String, Vec<u32>>,
    used_notes: Vec<String>,
    style_run: u32,
    has_title: bool,
}

/// A pending caption or list item ties a block to the next one, so a batch is
/// only cut where none does.
pub fn safe_cut(last: Option<&Block>) -> bool {
    match last {
        Some(Block::Item { .. } | Block::Figure(_) | Block::Caption(_)) => false,
        Some(Block::Para(t)) => !crate::md::caption_start(t.trim_matches('*')),
        _ => true,
    }
}

/// Whether any paragraph uses a Title style: a one-pass scan of the body.
fn has_title_style(zip: &mut Zip, styles: &Styles) -> Result<bool, String> {
    let Some(entry) = zip.stream(PART)? else {
        return Ok(false);
    };
    let mut found = false;
    scan(entry, |name, end, raw| {
        if name == "pStyle" && !end && attribute(raw, "val").is_some_and(|id| styles.is_title(&id))
        {
            found = true;
        }
        !found
    })?;
    Ok(found)
}

/// The body of a DOCX as a stream of batches, then its footnotes.
struct Reader<'a> {
    w: Walker<'a>,
    frags: Frags<Entry<'a>>,
    /// Packed position of the next batch.
    pos: u64,
    ended: bool,
    notes_done: bool,
}

impl Reader<'_> {
    fn state(&self) -> State {
        State {
            counters: self.w.counters.clone(),
            used_notes: self.w.used_notes.clone(),
            style_run: self.w.style_run,
            has_title: self.w.has_title,
        }
    }
}

impl Steps for Reader<'_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.w.ctx, &mut *self.w.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: 1,
            pos: self.pos,
            st: serde_json::to_value(self.state()).ok(),
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        if self.ended {
            if std::mem::replace(&mut self.notes_done, true) {
                return Ok(None);
            }
            let notes: Vec<Block> = std::mem::take(&mut self.w.used_notes)
                .iter()
                .filter_map(|id| {
                    self.w
                        .footnotes
                        .get(id)
                        .map(|t| Block::Raw(format!("[^{id}]: {}", esc(t))))
                })
                .collect();
            return Ok(Some(Step::Chunk(crate::md::render(&notes))));
        }
        loop {
            let Some(batch) = self.frags.next_batch(BATCH_BYTES)? else {
                self.ended = true;
                break;
            };
            let doc = parse(&batch.xml)?;
            self.w.body(container(&doc, batch.depth), Some(1));
            self.pos = batch.next;
            if batch.last {
                self.ended = true;
                break;
            }
            if self.w.ctx.waiting || safe_cut(self.w.blocks.last()) {
                break;
            }
        }
        if self.w.ctx.waiting {
            return Ok(Some(Step::Wait));
        }
        crate::md::attach_captions(&mut self.w.blocks, self.w.acc);
        let blocks = std::mem::take(&mut self.w.blocks);
        Ok(Some(Step::Chunk(crate::md::render(&blocks))))
    }
}

pub fn convert_docx(
    ctx: &mut Ctx,
    source: &str,
    src: &Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let mut zip = Zip::open(src.reopen()?)?;
    let mut main = zip.fork()?;
    if !zip.has(PART) {
        return Err("the DOCX has no word/document.xml".into());
    }
    let styles = parse_styles(&mut zip)?;
    let numbering = parse_numbering(&mut zip)?;
    let rels = parse_rels(&mut zip)?;
    let (footnotes, notes_dropped) = parse_footnotes(&mut zip)?;
    let saved: State = resume
        .and_then(|r| r.st.clone())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let has_title = match resume {
        Some(_) => saved.has_title,
        None => has_title_style(&mut main, &styles)?,
    };
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &header(source, "docx"));
            if notes_dropped > 0 {
                acc.warn(format!("{notes_dropped} footnote(s) were left out: the notes of this document are over the {} MiB the reader keeps", NOTES_CAP_BYTES / 1024 / 1024));
            }
        }
    }
    let entry = main
        .stream(PART)?
        .ok_or("the DOCX has no word/document.xml")?;
    let start = resume.map(|r| r.pos).filter(|p| *p > 0);
    let frags = Frags::open(entry, &[("document", 0), ("body", 0)], &[], start)?;
    let next = {
        let mut reader = Reader {
            w: Walker {
                ctx: &mut *ctx,
                acc: &mut acc,
                zip: &mut zip,
                styles,
                numbering,
                rels,
                counters: saved.counters,
                footnotes,
                used_notes: saved.used_notes,
                blocks: Vec::new(),
                style_run: saved.style_run,
                has_title,
            },
            frags,
            pos: start.unwrap_or(0),
            ended: false,
            notes_done: false,
        };
        slicer::run(&mut reader, resume.map_or(0, |r| r.skip))?
    };
    Ok(ctx.document(acc, source, "docx", 1, resume, next, parts_from))
}
