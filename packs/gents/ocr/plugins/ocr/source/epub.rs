//! EPUB: the OPF spine gives the reading order, each XHTML chapter is converted
//! with the shared HTML walker, and images come from inside the archive.
use std::collections::HashMap;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::html_doc::{Reader, WHOLE_BYTES};
use crate::html_md::Resolver;
use crate::model::{DocAcc, Document};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps, with_marker};
use crate::src::Src;
use crate::util::{Zip, decode_text, resolve, skip_bytes};
use crate::xml;

/// Font obfuscation is the only encryption a reader can ignore.
const FONT_OBFUSCATION: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

struct ZipResolver<'z> {
    zip: &'z mut Zip,
    base: String,
}

impl Resolver for ZipResolver<'_> {
    fn image(&mut self, src: &str) -> Option<Rc<[u8]>> {
        self.zip
            .read_shared(&resolve(&self.base, src))
            .ok()
            .flatten()
    }
}

fn check_not_drm(zip: &mut Zip) -> Result<(), String> {
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
fn spine(zip: &mut Zip) -> Result<Vec<String>, String> {
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

/// A chapter reader that puts the chapter's marker in front of its first text.
struct Marked<'r, 'a> {
    inner: Reader<'a>,
    marker: &'r str,
    pending: bool,
}

impl Steps for Marked<'_, '_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        self.inner.parts()
    }

    fn snapshot(&self) -> Snap {
        self.inner.snapshot()
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        let step = self.inner.step()?;
        if !self.pending {
            return Ok(step);
        }
        Ok(match step {
            Some(Step::Chunk(body)) => {
                self.pending = false;
                Some(Step::Chunk(with_marker(self.marker, &body)))
            }
            None => {
                self.pending = false;
                Some(Step::Chunk(self.marker.to_string()))
            }
            other => other,
        })
    }
}

/// The number of reading-order sections, without reading them.
pub fn section_count(src: &Src) -> Result<u32, String> {
    let mut zip = Zip::open(src.reopen()?)?;
    check_not_drm(&mut zip)?;
    Ok(spine(&mut zip)?.len() as u32)
}

pub fn convert_epub(
    ctx: &mut Ctx,
    source: &str,
    src: &Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let mut zip = Zip::open(src.reopen()?)?;
    let mut stream_zip = zip.fork()?;
    check_not_drm(&mut zip)?;
    let chapters = spine(&mut zip)?;
    let total = chapters.len() as u32;
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &header(source, "epub"));
            if ctx.opts.selects_none(total) {
                acc.warn(format!(
                    "pages selects nothing: the book has {total} section(s)"
                ));
            }
        }
    }
    let (mut pos, mut skip) = resume.map_or((0, 0), |r| (r.pos, r.skip));
    let mut next = None;
    for n in resume.map_or(1, |r| r.unit.max(1))..=total {
        if !ctx.opts.selected(n) {
            continue;
        }
        let path = &chapters[(n - 1) as usize];
        let Some(size) = zip.size_of(path) else {
            acc.warn(format!(
                "section {n}: the file {path} listed in the spine is missing from the archive"
            ));
            continue;
        };
        let marker = format!("<!-- section {n}: {} -->", path.replace('>', "&gt;"));
        let mut res = ZipResolver {
            zip: &mut zip,
            base: path.clone(),
        };
        let reader = if size <= WHOLE_BYTES {
            let bytes = stream_zip
                .read_limited(path, WHOLE_BYTES)?
                .unwrap_or_default();
            let text = decode_text(&bytes).into_owned();
            Reader::whole(ctx, &mut acc, &mut res, n, text)
        } else {
            let mut entry = stream_zip
                .stream(path)?
                .ok_or_else(|| format!("section {n}: {path} is missing from the archive"))?;
            // A deflated entry cannot seek: the part already delivered is read past once per call.
            skip_bytes(&mut entry, pos)?;
            Reader::chunked(ctx, &mut acc, &mut res, n, Box::new(entry), pos)
        };
        let mut marked = Marked {
            inner: reader,
            marker: &marker,
            pending: pos == 0,
        };
        next = slicer::run(&mut marked, std::mem::take(&mut skip))?;
        pos = 0;
        if next.is_some() {
            break;
        }
    }
    Ok(ctx.document(acc, source, "epub", total, resume, next, parts_from))
}
