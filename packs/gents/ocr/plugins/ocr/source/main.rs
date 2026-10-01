//! ocr plugin: reads documents and images into Markdown an LLM can use,
//! including the text inside figures. One JSON value on stdin, one on stdout;
//! a failure is one sentence on stderr and a non-zero exit (see TOOL.md).
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use serde::Serialize;

mod ctx;
mod detect;
mod docx;
mod epub;
mod graph;
mod html;
mod html_doc;
mod html_md;
mod imgdoc;
mod input;
mod layout;
mod md;
mod model;
mod ocr;
mod odf;
mod pdf;
#[cfg(test)]
mod pdfgen;
mod pdflazy;
#[cfg(test)]
mod pdflazy_tests;
mod pdfobj;
mod pix;
mod plan;
mod pptx;
mod resume;
#[cfg(test)]
mod slice_tests;
mod slicer;
mod src;
mod table;
#[cfg(test)]
mod tests;
mod text;
mod textio;
mod util;
mod xlsx;
mod xml;
mod xmlfrag;

use ctx::Ctx;
use detect::Kind;
use input::{Input, Mode, OcrMode};
use model::{Document, OUTPUT_CAP_BYTES, Part};
use resume::{Payload, Resume};
use src::Src;

const MAX_INLINE_BASE64: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_DEPTH: usize = 16;
/// Once this little budget is left, the call stops and returns a cursor for the rest.
const BUDGET_FLOOR: usize = 16 * 1024;

enum Data {
    Path(PathBuf),
    Inline(Vec<u8>),
}

pub struct Source {
    pub name: String,
    data: Data,
}

impl Source {
    fn open(self) -> Result<Src, String> {
        match self.data {
            Data::Path(p) => Src::open(&p),
            Data::Inline(b) => Ok(Src::mem(b)),
        }
    }

    /// The directory of the file, for formats that read files beside it.
    fn dir(&self) -> Option<PathBuf> {
        match &self.data {
            Data::Path(p) => p.parent().map(Path::to_path_buf),
            Data::Inline(_) => None,
        }
    }
}

#[derive(Serialize)]
struct Next {
    cursor: String,
    source: String,
}

#[derive(Serialize)]
struct Plain<'a> {
    documents: &'a [Document],
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<&'a Next>,
}

#[derive(Serialize)]
struct WithImages<'a> {
    response: Plain<'a>,
    parts: &'a [Part],
}

fn main() {
    let mut raw = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut raw) {
        fail(&format!("reading stdin: {err}"));
    }
    match run(&raw) {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            if let Err(err) = stdout
                .write_all(out.as_bytes())
                .and_then(|()| stdout.write_all(b"\n"))
            {
                fail(&format!("writing the result: {err}"));
            }
        }
        Err(err) => fail(&err),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("ocr: {message}");
    std::process::exit(1);
}

fn run(raw: &str) -> Result<String, String> {
    run_at(raw, Instant::now())
}

/// A source that matches a cursor: the same file list, the same file, unchanged.
fn check_cursor(p: &Payload, req: u64, sources: &[Source]) -> Result<(), String> {
    let gone = "the files changed since the cursor was returned; start again without a cursor";
    if p.req != req {
        return Err("the cursor belongs to a different request (files, pages, ocr or image options changed); repeat the request it came from".into());
    }
    match sources.get(p.file as usize) {
        Some(s) if s.name == p.name => Ok(()),
        _ => Err(gone.into()),
    }
}

/// Runs a request whose wall clock started at `started`.
fn run_at(raw: &str, started: Instant) -> Result<String, String> {
    if let Some(out) = graph::run(raw, started) {
        return out;
    }
    let input: Input = serde_json::from_str(raw).map_err(|e| format!("invalid input: {e}"))?;
    input.validate_source()?;
    let opts = input.options()?;
    let figure_images = opts.figure_images;
    let (sources, root) = sources(&input)?;
    let names: Vec<String> = sources.iter().map(|s| s.name.clone()).collect();
    let req = input.fingerprint(&names);
    if input.mode == Some(Mode::Plan) {
        return plan::run(&opts, sources, started);
    }
    let mut resume: Option<Resume> = None;
    let mut first = 0usize;
    if let Some(cursor) = &input.cursor {
        let p = resume::decode(cursor)?;
        check_cursor(&p, req, &sources)?;
        first = p.file as usize;
        resume = Some(p.at.clone()).filter(|r| !r.is_start());
        let mut probe = open_shared(&sources[first])?;
        let same = probe.len() == p.len
            && probe
                .fingerprint()
                .map_err(|e| format!("cannot read the file: {e}"))?
                == p.fp;
        if !same {
            return Err(format!(
                "{} changed since the cursor was returned; start again without a cursor",
                p.name
            ));
        }
    }
    let mut ctx = Ctx::started(opts, started);
    let single = sources.len() == 1;
    let mut docs = Vec::with_capacity(sources.len().min(1024));
    let mut failed = 0usize;
    let mut next: Option<Next> = None;
    for (i, src) in sources.into_iter().enumerate().skip(first) {
        let continuing = resume.take();
        let stop = continuing.is_none()
            && !docs.is_empty()
            && (ctx.budget.remaining() < BUDGET_FLOOR
                || !ctx.clock.fits()
                || (ctx.opts.ocr != OcrMode::Never
                    && detect::is_image_name(&src.name)
                    && !ctx.ocr.has_time()));
        if stop {
            next = Some(issue(req, i, &src, None)?);
            break;
        }
        if !single
            && continuing.is_none()
            && ctx.opts.ocr != OcrMode::Never
            && detect::is_image_name(&src.name)
            && !ctx.ocr.has_time()
        {
            docs.push(unread(&src.name, &format!("not read: {}", ctx::NO_TIME)));
            failed += 1;
            continue;
        }
        let name = src.name.clone();
        let fresh = continuing.is_none();
        let before = ctx.emitted;
        match process(&mut ctx, src, &root, continuing.as_ref()) {
            Ok((mut doc, mut srcf)) => {
                if let Some(at) = doc.next.take() {
                    let fp = srcf
                        .fingerprint()
                        .map_err(|e| format!("cannot read the file: {e}"))?;
                    let cursor = resume::encode(&Payload {
                        v: 1,
                        req,
                        file: i as u32,
                        name: name.clone(),
                        len: srcf.len(),
                        fp,
                        at,
                    })?;
                    next = Some(Next {
                        cursor,
                        source: name,
                    });
                    // Nothing of this file fit beside what earlier files filled: leave it whole.
                    if !(fresh && ctx.emitted == before && !docs.is_empty()) {
                        docs.push(doc);
                    }
                    break;
                }
                docs.push(doc);
            }
            Err(why) if single => return Err(format!("{name}: {why}")),
            Err(why) => {
                docs.push(unread(&name, &format!("failed: {why}")));
                failed += 1;
            }
        }
    }
    if failed == docs.len() && next.is_none() {
        let reasons: Vec<&str> = docs
            .iter()
            .flat_map(|d| d.warnings.iter().map(String::as_str))
            .take(3)
            .collect();
        return Err(format!(
            "no document could be read ({})",
            reasons.join("; ")
        ));
    }
    let plain = Plain {
        documents: &docs,
        next: next.as_ref(),
    };
    let json = if figure_images && !ctx.parts.is_empty() {
        serde_json::to_string(&WithImages {
            response: plain,
            parts: &ctx.parts,
        })
    } else {
        serde_json::to_string(&plain)
    }
    .map_err(|e| format!("serializing the result: {e}"))?;
    if json.len() > OUTPUT_CAP_BYTES + 400_000 {
        return Err(format!(
            "the result is {} bytes, over the output limit; lower max_bytes or read fewer pages or files",
            json.len()
        ));
    }
    Ok(json)
}

/// A cursor that resumes at the start of file `i`.
fn issue(req: u64, i: usize, src: &Source, at: Option<Resume>) -> Result<Next, String> {
    let mut s = open_shared(src)?;
    let cursor = resume::encode(&Payload {
        v: 1,
        req,
        file: i as u32,
        name: src.name.clone(),
        len: s.len(),
        fp: s
            .fingerprint()
            .map_err(|e| format!("cannot read the file: {e}"))?,
        at: at.unwrap_or_default(),
    })?;
    Ok(Next {
        cursor,
        source: src.name.clone(),
    })
}

/// Opens a source without consuming it.
fn open_shared(src: &Source) -> Result<Src, String> {
    match &src.data {
        Data::Path(p) => Src::open(p),
        Data::Inline(b) => Ok(Src::mem(b.clone())),
    }
}

fn unread(name: &str, why: &str) -> Document {
    Document {
        source: name.to_string(),
        format: "unknown",
        pages: 0,
        markdown: String::new(),
        figures: Vec::new(),
        warnings: vec![why.to_string()],
        joint: None,
        table_header: None,
        next: None,
    }
}

/// The documents a request names, and the directory they may read images from.
fn sources(input: &Input) -> Result<(Vec<Source>, PathBuf), String> {
    if let (Some(b64), Some(name)) = (&input.data_base64, &input.name) {
        if b64.len() > MAX_INLINE_BASE64 {
            return Err(format!(
                "data_base64 is over the {} MiB limit; bind the file with path instead",
                MAX_INLINE_BASE64 / 1024 / 1024
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("data_base64 is not valid base64: {e}"))?;
        let name = Path::new(name)
            .file_name()
            .map_or_else(|| name.clone(), |n| n.to_string_lossy().into_owned());
        return Ok((
            vec![Source {
                name,
                data: Data::Inline(bytes),
            }],
            PathBuf::new(),
        ));
    }
    let path = Path::new(input.path.as_deref().unwrap_or(""));
    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "cannot read {}: {e}; the path must be bound for this call",
            path.display()
        )
    })?;
    if meta.is_file() {
        if input.files.is_some() {
            return Err("files lists paths inside a directory, but path is a file".into());
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return Ok((
            vec![Source {
                name,
                data: Data::Path(path.to_path_buf()),
            }],
            root,
        ));
    }
    let mut out = Vec::new();
    match &input.files {
        Some(files) => {
            for f in files {
                out.push(Source {
                    name: f.clone(),
                    data: Data::Path(path.join(f)),
                });
            }
        }
        None => walk(path, path, 0, &mut out)?,
    }
    if out.is_empty() {
        return Err(format!("{} contains no files to read", path.display()));
    }
    Ok((out, path.to_path_buf()))
}

/// Lists files below `dir` in name order, skipping hidden entries.
fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Source>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            walk(root, &p, depth + 1, out)?;
        } else if meta.is_file() {
            if out.len() >= MAX_FILES {
                return Err(format!(
                    "{} holds more than {MAX_FILES} files; list the ones to read in files",
                    root.display()
                ));
            }
            let name = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(Source {
                name,
                data: Data::Path(p),
            });
        }
    }
    Ok(())
}

/// Converts one source. The opened file is handed back, so a cursor can fingerprint it.
fn process(
    ctx: &mut Ctx,
    source: Source,
    root: &Path,
    resume: Option<&Resume>,
) -> Result<(Document, Src), String> {
    let name = source.name.clone();
    let base = source.dir();
    let mut src = source.open()?;
    let kind = detect::detect(&name, &mut src)?;
    let name = name.as_str();
    let doc = match kind {
        Kind::Pdf => pdf::convert(ctx, name, &mut src, resume)?,
        Kind::Image(format) => imgdoc::convert(ctx, name, format, &mut src)?,
        Kind::Html => html_doc::convert_html(
            ctx,
            name,
            &mut src,
            &mut html_doc::DirResolver {
                base,
                root: root.to_path_buf(),
            },
            resume,
        )?,
        Kind::Text => text::convert_text(ctx, name, "text", &mut src, resume)?,
        Kind::Markdown => text::convert_text(ctx, name, "markdown", &mut src, resume)?,
        Kind::Csv => text::convert_csv(ctx, name, "csv", &mut src, false, resume)?,
        Kind::Tsv => text::convert_csv(ctx, name, "tsv", &mut src, true, resume)?,
        Kind::Epub => epub::convert_epub(ctx, name, &src, resume)?,
        Kind::Docx => docx::convert_docx(ctx, name, &src, resume)?,
        Kind::Pptx => pptx::convert_pptx(ctx, name, &src, resume)?,
        Kind::Xlsx => xlsx::convert_xlsx(ctx, name, &src, resume)?,
        Kind::Odt | Kind::Ods | Kind::Odp => odf::convert_odf(ctx, name, kind, &src, resume)?,
    };
    Ok((doc, src))
}
