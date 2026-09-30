//! Image files: a single page read by OCR and laid out like a scanned page.
use crate::ctx::{Ctx, Original};
use crate::detect::header;
use crate::input::OcrMode;
use crate::layout::{Out, layout};
use crate::model::{DocAcc, Document, Figure};
use crate::pdf::ocr_items;
use crate::pix::Pix;

pub fn convert(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    data: &[u8],
) -> Result<Document, String> {
    if ctx.opts.ocr == OcrMode::Never {
        return Err("ocr is never, and an image has no text layer to read".into());
    }
    if !ctx.ocr.has_time() {
        return Err(crate::ctx::NO_TIME.into());
    }
    let full = Pix::decode(data)?;
    let dims = (full.w, full.h);
    let pix = full.fit(ctx.opts.max_image_px)?;
    let lines = ctx.ocr_page(&pix)?;
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, format));
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
        let part = ctx.attach(
            &mut acc,
            &id,
            &pix,
            dims,
            mime.map(|mime| Original { bytes: data, mime }),
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
    if items.is_empty() {
        acc.warn("no text was found in the image");
    }
    for o in layout(&items) {
        if let Out::Block(b) = o {
            blocks.push(b);
        }
    }
    let body = crate::md::render(&blocks);
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
    })
}
