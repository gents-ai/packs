//! HTML and XHTML files, with images read from beside the file when present.
use std::path::{Component, Path, PathBuf};

use crate::ctx::Ctx;
use crate::detect::header;
use crate::html::parse;
use crate::html_md::{Resolver, convert};
use crate::model::{DocAcc, Document};
use crate::util::decode_text;

const MAX_IMAGE_FILE: u64 = 64 * 1024 * 1024;

/// Reads images relative to the HTML file, never outside `root`.
pub struct DirResolver {
    pub base: Option<PathBuf>,
    pub root: PathBuf,
}

impl Resolver for DirResolver {
    fn image(&mut self, src: &str) -> Option<Vec<u8>> {
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
        std::fs::read(path).ok()
    }
}

pub fn convert_html(
    ctx: &mut Ctx,
    source: &str,
    data: &[u8],
    res: &mut dyn Resolver,
) -> Result<Document, String> {
    let text = decode_text(data);
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, "html"));
    let blocks = convert(ctx, &mut acc, res, Some(1), &parse(&text));
    if !ctx.append(&mut acc, &crate::md::render(&blocks)) {
        acc.warn("the output size limit was reached and the document was cut at that point");
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format: "html",
        pages: 1,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}
