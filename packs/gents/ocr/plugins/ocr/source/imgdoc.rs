//! Image files: a single page read by OCR and laid out like a scanned page.
use crate::ctx::{Ctx, Original, PASSTHROUGH_BYTES};
use crate::detect::header;
use crate::input::OcrMode;
use crate::layout::{Out, layout};
use crate::model::{DocAcc, Document, Figure};
use crate::pdfocr::ocr_items;
use crate::pix::Pix;
use crate::remote::{self, JPEG_QUALITY, REMOTE_MAX_PX, Read};
use crate::src::Src;

pub fn convert(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    src: &mut Src,
) -> Result<Document, String> {
    if ctx.opts.ocr == OcrMode::Never {
        return Err("ocr is never, and an image has no text layer to read".into());
    }
    if !ctx.ocr.has_time() {
        return Err(crate::ctx::NO_TIME.into());
    }
    let full = Pix::decode_from(&mut *src)?;
    let dims = (full.w, full.h);
    // The full image is kept only when a remote request may be made from it.
    let (pix, mut remote_src) = if ctx.remote.collecting() {
        (full.clone().fit(ctx.opts.max_image_px)?, Some(full))
    } else {
        (full.fit(ctx.opts.max_image_px)?, None)
    };
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, format));
    let (lines, remote_md) = if ctx.remote.active() {
        let mut builtin = |ctx: &mut Ctx| ctx.ocr_page_scored(&pix);
        let mut encode = || {
            let full = remote_src.take().ok_or("the image is not available")?;
            full.fit(REMOTE_MAX_PX)?.jpeg(JPEG_QUALITY)
        };
        match remote::read_page(ctx, &mut acc, 1, true, &mut builtin, &mut encode) {
            Read::Remote(md) => (Vec::new(), Some(md)),
            Read::Pending => (Vec::new(), Some(String::new())),
            Read::Lines(lines) => (lines, None),
            Read::Builtin | Read::Wait => (ctx.ocr_page(&pix)?, None),
        }
    } else {
        (ctx.ocr_page(&pix)?, None)
    };
    let mut blocks = Vec::new();
    if ctx.opts.figure_images {
        let id = "fig-1".to_string();
        let mime = match format {
            "png" => Some("image/png"),
            "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        };
        // Only an image small enough to pass through is read again as bytes.
        let original = if src.len() <= PASSTHROUGH_BYTES as u64 {
            src.head(src.len() as usize)
                .map_err(|e| format!("cannot read the file: {e}"))?
        } else {
            Vec::new()
        };
        let part = ctx.attach(
            &mut acc,
            &id,
            &pix,
            dims,
            mime.filter(|_| !original.is_empty()).map(|mime| Original {
                bytes: &original,
                mime,
            }),
        );
        let fig = Figure {
            id,
            page: Some(1),
            caption: "source image".into(),
            width: dims.0,
            height: dims.1,
            ocr: true,
            part,
            note: "(its text follows as the page text)".into(),
            ..Figure::default()
        };
        if let Some(b) = ctx.keep(&mut acc, fig) {
            blocks.push(b);
        }
    }
    let items = ocr_items(&lines);
    if items.is_empty() && remote_md.as_ref().is_none_or(|md| md.trim().is_empty()) {
        acc.warn("no text was found in the image");
    }
    for o in layout(&items) {
        if let Out::Block(b) = o {
            blocks.push(b);
        }
    }
    let mut body = crate::md::render(&blocks);
    if let Some(md) = remote_md.filter(|md| !md.trim().is_empty()) {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&md);
    }
    if let Some(w) = ctx.remote.take_read_warning() {
        acc.warn(w);
    }
    if !ctx.append(&mut acc, &body) {
        acc.warn("the output size limit was reached and the page text was cut");
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format,
        pages: 1,
        markdown: acc.md,
        figures: acc.figures,
        warnings,
        unread_pages: acc.unread_pages,
        joint: None,
        table_header: None,
        next: None,
    })
}
