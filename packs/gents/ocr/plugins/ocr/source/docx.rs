//! DOCX: body paragraphs with heading styles, lists, tables, links, footnotes
//! and images (with their alt text as the figure caption).
use std::collections::HashMap;

use roxmltree::Node;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::md::esc;
use crate::model::{Block, DocAcc, Document};
use crate::util::{Zip, resolve};
use crate::xml::{attr, child, descendant, is, parse, text};

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

struct Walker<'z, 'a> {
    ctx: &'z mut Ctx,
    acc: &'z mut DocAcc,
    zip: &'z mut Zip<'a>,
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

fn parse_styles(zip: &mut Zip<'_>) -> Result<Styles, String> {
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

fn parse_numbering(zip: &mut Zip<'_>) -> Result<Numbering, String> {
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

fn parse_rels(zip: &mut Zip<'_>) -> Result<HashMap<String, Rel>, String> {
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

fn parse_footnotes(zip: &mut Zip<'_>) -> Result<HashMap<String, String>, String> {
    let mut notes = HashMap::new();
    if let Some(src) = zip.read_text("word/footnotes.xml")? {
        let doc = parse(&src)?;
        for n in doc.descendants().filter(|n| is(*n, "footnote")) {
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
                notes.insert(id.to_string(), t.join(" ").trim().to_string());
            }
        }
    }
    Ok(notes)
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

impl Walker<'_, '_> {
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
            if self.acc.truncated {
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

pub fn convert_docx(ctx: &mut Ctx, source: &str, data: &[u8]) -> Result<Document, String> {
    let mut zip = Zip::open(data)?;
    let src = zip
        .read_text(PART)?
        .ok_or("the DOCX has no word/document.xml")?;
    let doc = parse(&src)?;
    let body = descendant(doc.root_element(), "body").ok_or("the DOCX has no document body")?;
    let styles = parse_styles(&mut zip)?;
    let numbering = parse_numbering(&mut zip)?;
    let rels = parse_rels(&mut zip)?;
    let footnotes = parse_footnotes(&mut zip)?;
    let has_title = doc
        .descendants()
        .any(|n| is(n, "pStyle") && attr(n, "val").is_some_and(|id| styles.is_title(id)));
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, "docx"));
    let mut w = Walker {
        ctx: &mut *ctx,
        acc: &mut acc,
        zip: &mut zip,
        styles,
        numbering,
        rels,
        counters: HashMap::new(),
        footnotes,
        used_notes: Vec::new(),
        blocks: Vec::new(),
        style_run: 0,
        has_title,
    };
    w.body(body, Some(1));
    crate::md::attach_captions(&mut w.blocks, w.acc);
    for id in std::mem::take(&mut w.used_notes) {
        if let Some(t) = w.footnotes.get(&id) {
            w.blocks.push(Block::Raw(format!("[^{id}]: {}", esc(t))));
        }
    }
    let blocks = std::mem::take(&mut w.blocks);
    drop(w);
    if !ctx.append(&mut acc, &crate::md::render(&blocks)) {
        acc.warn("the output size limit was reached and the document was cut at that point");
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format: "docx",
        pages: 1,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}
