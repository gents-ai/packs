//! PPTX: one section per slide, titles as headings, text boxes, bullets,
//! tables, pictures with their alt text, and speaker notes.
use std::collections::HashMap;

use roxmltree::Node;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::md::esc;
use crate::model::{Block, DocAcc, Document};
use crate::util::{Zip, resolve};
use crate::xml::{attr, child, descendant, is, parse};

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

struct Rels(HashMap<String, (String, String)>);

fn rels_of(zip: &mut Zip<'_>, part: &str) -> Result<Rels, String> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let path = format!("{dir}/_rels/{file}.rels");
    let mut map = HashMap::new();
    if let Some(src) = zip.read_text(&path)? {
        let doc = parse(&src)?;
        for r in doc.descendants().filter(|n| is(*n, "Relationship")) {
            if let (Some(id), Some(target)) = (attr(r, "Id"), attr(r, "Target"))
                && attr(r, "TargetMode") != Some("External")
            {
                map.insert(
                    id.to_string(),
                    (
                        resolve(part, target),
                        attr(r, "Type").unwrap_or("").to_string(),
                    ),
                );
            }
        }
    }
    Ok(Rels(map))
}

fn paragraph_text(p: Node<'_, '_>) -> String {
    let mut out = String::new();
    for n in p.descendants().filter(Node::is_element) {
        match n.tag_name().name() {
            "t" => out.push_str(n.text().unwrap_or("")),
            "br" => out.push(' '),
            _ => {}
        }
    }
    out
}

struct Slide<'z, 'a> {
    ctx: &'z mut Ctx,
    acc: &'z mut DocAcc,
    zip: &'z mut Zip<'a>,
    rels: Rels,
    unit: u32,
    blocks: Vec<Block>,
}

impl Slide<'_, '_> {
    fn shape(&mut self, sp: Node<'_, '_>) {
        let ph = descendant(sp, "ph").and_then(|p| attr(p, "type"));
        if matches!(ph, Some("sldNum" | "dt" | "ftr")) {
            return;
        }
        let Some(tx) = child(sp, "txBody") else {
            return;
        };
        let title = matches!(ph, Some("title" | "ctrTitle"));
        for p in tx.children().filter(|n| is(*n, "p")) {
            let text = paragraph_text(p)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() {
                continue;
            }
            let ppr = child(p, "pPr");
            let bullet = ppr
                .is_some_and(|x| child(x, "buChar").is_some() || child(x, "buAutoNum").is_some());
            let numbered = ppr.is_some_and(|x| child(x, "buAutoNum").is_some());
            let depth = ppr
                .and_then(|x| attr(x, "lvl"))
                .and_then(|l| l.parse::<u8>().ok())
                .unwrap_or(0)
                .min(6);
            self.blocks.push(if title {
                Block::Heading(2, esc(&text))
            } else if bullet {
                Block::Item {
                    depth,
                    marker: if numbered { "1.".into() } else { "-".into() },
                    text: esc(&text),
                }
            } else {
                Block::Para(esc(&text))
            });
        }
    }

    fn picture(&mut self, pic: Node<'_, '_>) {
        let Some(rid) = descendant(pic, "blip").and_then(|b| b.attribute((R_NS, "embed"))) else {
            return;
        };
        let alt = descendant(pic, "cNvPr")
            .and_then(|c| {
                attr(c, "descr")
                    .filter(|s| !s.is_empty())
                    .or_else(|| attr(c, "title"))
            })
            .unwrap_or("")
            .to_string();
        let Some((path, _)) = self.rels.0.get(rid).cloned() else {
            return;
        };
        let unit = Some(self.unit);
        let block = match self.zip.read(&path) {
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

    fn table(&mut self, tbl: Node<'_, '_>) {
        let mut rows = Vec::new();
        for tr in tbl.children().filter(|n| is(*n, "tr")) {
            let mut cells = Vec::new();
            for tc in tr.children().filter(|n| is(*n, "tc")) {
                let t: Vec<String> = tc
                    .descendants()
                    .filter(|p| is(*p, "p"))
                    .map(paragraph_text)
                    .collect();
                cells.push(t.join(" ").split_whitespace().collect::<Vec<_>>().join(" "));
            }
            rows.push(cells);
        }
        rows.retain(|r| r.iter().any(|c| !c.is_empty()));
        if !rows.is_empty() {
            self.blocks.push(Block::Table(rows));
        }
    }

    fn tree(&mut self, node: Node<'_, '_>) {
        let mut shapes: Vec<Node<'_, '_>> = node.children().filter(Node::is_element).collect();
        // The title leads the slide whatever its z-order.
        shapes.sort_by_key(|s| {
            !(is(*s, "sp")
                && descendant(*s, "ph")
                    .and_then(|p| attr(p, "type"))
                    .is_some_and(|t| t == "title" || t == "ctrTitle"))
        });
        for s in shapes {
            if self.acc.truncated {
                return;
            }
            match s.tag_name().name() {
                "sp" => self.shape(s),
                "pic" => self.picture(s),
                "grpSp" => self.tree(s),
                "graphicFrame" => {
                    if let Some(tbl) = descendant(s, "tbl") {
                        self.table(tbl);
                    }
                }
                "AlternateContent" => {
                    if let Some(choice) = child(s, "Choice") {
                        self.tree(choice);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn convert_pptx(ctx: &mut Ctx, source: &str, data: &[u8]) -> Result<Document, String> {
    let mut zip = Zip::open(data)?;
    let pres_path = "ppt/presentation.xml";
    let pres = zip
        .read_text(pres_path)?
        .ok_or("the PPTX has no ppt/presentation.xml")?;
    let pdoc = parse(&pres)?;
    let pres_rels = rels_of(&mut zip, pres_path)?;
    let slides: Vec<String> = pdoc
        .descendants()
        .filter(|n| is(*n, "sldId"))
        .filter_map(|n| n.attribute((R_NS, "id")))
        .filter_map(|rid| pres_rels.0.get(rid).map(|(p, _)| p.clone()))
        .collect();
    if slides.is_empty() {
        return Err("the PPTX has no slides".into());
    }
    let total = slides.len() as u32;
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, "pptx"));
    let mut selected = 0;
    for (k, path) in slides.iter().enumerate() {
        let n = k as u32 + 1;
        if !ctx.opts.selected(n) {
            continue;
        }
        selected += 1;
        let Some(src) = zip.read_text(path)? else {
            acc.warn(format!("slide {n}: {path} is missing from the archive"));
            continue;
        };
        let doc = parse(&src)?;
        let rels = rels_of(&mut zip, path)?;
        let notes_path = rels
            .0
            .values()
            .find(|(_, t)| t.ends_with("/notesSlide"))
            .map(|(p, _)| p.clone());
        let blocks = {
            let mut slide = Slide {
                ctx: &mut *ctx,
                acc: &mut acc,
                zip: &mut zip,
                rels,
                unit: n,
                blocks: Vec::new(),
            };
            if let Some(tree) = descendant(doc.root_element(), "spTree") {
                slide.tree(tree);
            }
            slide.blocks
        };
        let mut blocks = blocks;
        if let Some(np) = notes_path
            && let Some(nsrc) = zip.read_text(&np)?
        {
            let ndoc = parse(&nsrc)?;
            let notes: Vec<String> = ndoc
                .descendants()
                .filter(|s| {
                    is(*s, "sp")
                        && descendant(*s, "ph").and_then(|p| attr(p, "type")) == Some("body")
                })
                .flat_map(|s| {
                    s.descendants()
                        .filter(|p| is(*p, "p"))
                        .map(paragraph_text)
                        .collect::<Vec<_>>()
                })
                .filter(|t| !t.trim().is_empty())
                .collect();
            if !notes.is_empty() {
                blocks.push(Block::Para(format!("*Notes:* {}", esc(&notes.join(" ")))));
            }
        }
        let body = crate::md::render(&blocks);
        let marker = format!("<!-- slide {n} -->");
        if !ctx.append(
            &mut acc,
            &if body.is_empty() {
                marker
            } else {
                format!("{marker}\n\n{body}")
            },
        ) {
            acc.warn(format!("the output size limit was reached before slide {n}; request pages=\"{n}-\" to continue"));
            break;
        }
    }
    if selected == 0 {
        acc.warn(format!(
            "pages selects nothing: the deck has {total} slide(s)"
        ));
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format: "pptx",
        pages: total,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}
