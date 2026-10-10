use super::*;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write};
const PROMPT: &str = "Inspect this scanned book page as visual evidence for book structure. Describe exactly what is visible: heading text and numbering; whether this is a table of contents and its continuation; all visible contents entries with indentation/hierarchy and printed page labels; body text start or continuation; running headers, footers and printed page label; illustrations or blank regions. Preserve spelling, Roman numerals, case, and order. Separate observations from uncertain interpretations. Do not infer missing titles or chapter boundaries from a page label alone.";
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    current_page: Option<u32>,
    observations: Vec<Value>,
}
struct Guard(std::path::PathBuf);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn ledger_name(v: &Value) -> Result<String> {
    Ok(format!(
        "inspection-{}.json",
        &hash(field(v, "stage_job_id")?.as_bytes())[..32]
    ))
}
fn ledger(root: &Path, name: &str) -> Result<Ledger> {
    if !root.join(name).try_exists().map_err(|e| e.to_string())? {
        return Ok(Ledger::default());
    }
    serde_json::from_slice(&read(root, name, 512 * 1024)?).map_err(|e| e.to_string())
}
pub fn inspect(root: &Path, v: &Value, book: &BookInput) -> Result<Value> {
    let scan = integer(v, "page_num")?.ok_or("page_num is required")?;
    let page = book.page(scan)?;
    let name = ledger_name(v)?;
    let lock = root.join(format!("{name}.lock"));
    let _lock = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map_err(
            |_| "image inspection already active for this stage; inspect one page at a time",
        )?;
    let _guard = Guard(lock);
    let mut ledger = ledger(root, &name)?;
    if v["model_results"].is_object() {
        let state = &v["state"];
        if state["book_hash"] != v["book_hash"]
            || state["stage_job_id"] != v["stage_job_id"]
            || state["scan_page"] != scan
            || state["previous_page"] != json!(ledger.current_page)
        {
            return Err("visual result does not match the pending book/page inspection".into());
        }
        let answer = &v["model_results"]["page"];
        if !answer["error"].is_null() {
            return Err(format!(
                "vision inspection failed: {}; retry load_page_image",
                answer["error"]
            ));
        }
        let text = field(answer, "text")?;
        if text.len() > 128 * 1024 {
            return Err("vision observation exceeds evidence budget".into());
        }
        if let Some(previous) = ledger.current_page {
            ledger.observations.push(
                json!({"page_num":previous,"observations":field(v,"current_page_observations")?}),
            );
        }
        ledger
            .observations
            .push(json!({"page_num":scan,"visual_observations":text}));
        ledger.current_page = Some(scan);
        let raw = serde_json::to_vec(&ledger).unwrap();
        if raw.len() > 512 * 1024 {
            return Err("inspection ledger full; finish this finding or start a new stage".into());
        }
        let pending = root.join(format!("{name}.pending"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|e| e.to_string())?;
        file.write_all(&raw).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(pending, root.join(&name)).map_err(|e| e.to_string())?;
        return Ok(
            json!({"current_page":scan,"source":page.source,"source_page":page.page,"visual_observations":text,"observations":ledger.observations,"inspection_file":name,"message":"A bound vision model inspected the rendered page. These are its visual observations. Record your interpretation in current_page_observations before inspecting another page."}),
        );
    }
    if ledger.current_page.is_some()
        && v["current_page_observations"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
    {
        return Err(format!(
            "Document observations of current page {} in current_page_observations, then call load_page_image for page {scan}.",
            ledger.current_page.unwrap()
        ));
    }
    if v["model_calls"] != true {
        return Err("load_page_image requires the pack's page_vision model binding".into());
    }
    let source = book
        .sources
        .iter()
        .find(|s| s.source == page.source)
        .ok_or("source asset missing")?;
    let bytes = read(root, &source.asset, 1024 * 1024 * 1024)?;
    if hash(&bytes) != source.sha256 {
        return Err("source PDF changed; create a new book discovery run".into());
    }
    let pdf =
        hayro_syntax::Pdf::new(bytes).map_err(|e| format!("cannot load source PDF: {e:?}"))?;
    if pdf.pages().len() != source.page_count as usize {
        return Err("source PDF count differs from OCR manifest".into());
    }
    let image = jpeg(&pdf.pages()[(page.page - 1) as usize])?;
    let data = base64::engine::general_purpose::STANDARD.encode(image);
    if data.len() > 3_000_000 {
        return Err("rendered page exceeds vision request budget".into());
    }
    let question = v["question"].as_str().unwrap_or("");
    if question.len() > 8000 {
        return Err("inspection question too long".into());
    }
    Ok(
        json!({"model_calls":{"requests":[{"id":"page","prompt":format!("{PROMPT}\n\nPhysical scan page: {scan}.\nInspection task: {question}"),"images":[{"mime":"image/jpeg","data_base64":data}],"max_tokens":4096}],"state":{"book_hash":v["book_hash"],"stage_job_id":v["stage_job_id"],"scan_page":scan,"previous_page":ledger.current_page}}}),
    )
}
fn jpeg<'a>(page: &'a hayro_syntax::page::Page<'a>) -> Result<Vec<u8>> {
    let (w, h) = page.render_dimensions();
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Err("invalid PDF page dimensions".into());
    }
    let scale = (2048.0 / w.max(h)).min(192.0 / 72.0);
    let settings = hayro::RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        ..Default::default()
    };
    let cache = hayro::RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &hayro_interpret::InterpreterSettings::default(),
        &settings,
    );
    let rgb: Vec<_> = pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode(
            &rgb,
            u32::from(pixmap.width()),
            u32::from(pixmap.height()),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(out)
}
