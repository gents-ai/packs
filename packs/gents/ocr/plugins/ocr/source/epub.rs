//! EPUB: the OPF spine gives the reading order, each XHTML chapter is converted
//! with the shared HTML walker, and images come from inside the archive.
use std::collections::HashMap;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::html::parse;
use crate::html_md::{Resolver, convert};
use crate::model::{DocAcc, Document};
use crate::util::{Zip, decode_text, resolve};
use crate::xml;

/// Font obfuscation is the only encryption a reader can ignore.
const FONT_OBFUSCATION: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

struct ZipResolver<'z, 'a> {
    zip: &'z mut Zip<'a>,
    base: String,
}

impl Resolver for ZipResolver<'_, '_> {
    fn image(&mut self, src: &str) -> Option<Rc<[u8]>> {
        self.zip
            .read_shared(&resolve(&self.base, src))
            .ok()
            .flatten()
    }
}

fn check_not_drm(zip: &mut Zip<'_>) -> Result<(), String> {
    let Some(text) = zip.read_text("META-INF/encryption.xml")? else {
        return Ok(());
    };
    let doc = xml::parse(&text)?;
    let protected = doc
        .descendants()
        .filter(|n| xml::is(*n, "EncryptionMethod"))
        .any(|n| xml::attr(n, "Algorithm").is_some_and(|a| !FONT_OBFUSCATION.contains(&a)));
    if protected {
        return Err("the EPUB is DRM-protected and cannot be read".into());
    }
    Ok(())
}

/// The spine's readable content files, in reading order, as archive paths.
fn spine(zip: &mut Zip<'_>) -> Result<Vec<String>, String> {
    let container = zip
        .read_text("META-INF/container.xml")?
        .ok_or("the EPUB has no META-INF/container.xml")?;
    let cdoc = xml::parse(&container)?;
    let opf_path = cdoc
        .descendants()
        .find(|n| xml::is(*n, "rootfile"))
        .and_then(|n| xml::attr(n, "full-path"))
        .ok_or("the EPUB container names no package file")?
        .to_string();
    let opf = zip
        .read_text(&opf_path)?
        .ok_or_else(|| format!("the EPUB package file {opf_path} is missing"))?;
    let doc = xml::parse(&opf)?;
    let mut manifest: HashMap<&str, (&str, &str)> = HashMap::new();
    for item in doc.descendants().filter(|n| xml::is(*n, "item")) {
        if let (Some(id), Some(href)) = (xml::attr(item, "id"), xml::attr(item, "href")) {
            manifest.insert(id, (href, xml::attr(item, "media-type").unwrap_or("")));
        }
    }
    let mut out = Vec::new();
    for itemref in doc.descendants().filter(|n| xml::is(*n, "itemref")) {
        if let Some((href, media)) = xml::attr(itemref, "idref").and_then(|id| manifest.get(id))
            && (media.contains("html") || media.contains("xml") || media.is_empty())
        {
            out.push(resolve(&opf_path, href));
        }
    }
    if out.is_empty() {
        return Err("the EPUB spine lists no readable chapters".into());
    }
    Ok(out)
}

pub fn convert_epub(ctx: &mut Ctx, source: &str, data: &[u8]) -> Result<Document, String> {
    let mut zip = Zip::open(data)?;
    check_not_drm(&mut zip)?;
    let chapters = spine(&mut zip)?;
    let total = chapters.len() as u32;
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, "epub"));
    let mut selected = 0;
    for (k, path) in chapters.iter().enumerate() {
        let n = k as u32 + 1;
        if !ctx.opts.selected(n) {
            continue;
        }
        selected += 1;
        let Some(bytes) = zip.read(path)? else {
            acc.warn(format!(
                "section {n}: the file {path} listed in the spine is missing from the archive"
            ));
            continue;
        };
        let text = decode_text(&bytes).into_owned();
        drop(bytes);
        let blocks = {
            let mut res = ZipResolver {
                zip: &mut zip,
                base: path.clone(),
            };
            convert(ctx, &mut acc, &mut res, Some(n), &parse(&text))
        };
        let body = crate::md::render(&blocks);
        let marker = format!("<!-- section {n}: {} -->", path.replace('>', "&gt;"));
        let chunk = if body.is_empty() {
            marker
        } else {
            format!("{marker}\n\n{body}")
        };
        if !ctx.append(&mut acc, &chunk) {
            acc.warn(format!("the output size limit was reached before section {n}; request pages=\"{n}-\" to continue"));
            break;
        }
    }
    if selected == 0 {
        acc.warn(format!(
            "pages selects nothing: the book has {total} section(s)"
        ));
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format: "epub",
        pages: total,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
    })
}
