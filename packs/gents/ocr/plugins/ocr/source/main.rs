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
mod pix;
mod pptx;
mod table;
#[cfg(test)]
mod tests;
mod text;
mod util;
mod xlsx;
mod xml;

use ctx::Ctx;
use detect::Kind;
use input::{Input, OcrMode};
use model::{Document, OUTPUT_CAP_BYTES, Part};

/// One input file may be at most this large.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const READ_CHUNK: usize = 8 * 1024 * 1024;
const MAX_INLINE_BASE64: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_DEPTH: usize = 16;
/// Once this little budget is left, further documents are reported unread.
const BUDGET_FLOOR: usize = 16 * 1024;

enum Data {
    Path(PathBuf),
    Inline(Vec<u8>),
}

struct Source {
    name: String,
    data: Data,
}

#[derive(Serialize)]
struct Plain<'a> {
    documents: &'a [Document],
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

/// Runs a request whose wall clock (for the OCR time budget) started at `started`.
fn run_at(raw: &str, started: Instant) -> Result<String, String> {
    let input: Input = serde_json::from_str(raw).map_err(|e| format!("invalid input: {e}"))?;
    input.validate_source()?;
    let opts = input.options()?;
    let figure_images = opts.figure_images;
    let mut ctx = Ctx::new(opts);
    ctx.ocr = ocr::Ocr::new(started);
    let (sources, root) = sources(&input)?;
    let single = sources.len() == 1;
    let mut docs = Vec::with_capacity(sources.len());
    let mut failed = 0usize;
    for src in sources {
        if !single && ctx.budget.remaining() < BUDGET_FLOOR {
            docs.push(unread(&src.name, "not read: the output size limit was reached; call again with files listing the remaining documents"));
            failed += 1;
            continue;
        }
        if !single
            && ctx.opts.ocr != OcrMode::Never
            && detect::is_image_name(&src.name)
            && !ctx.ocr.has_time()
        {
            docs.push(unread(&src.name, &format!("not read: {}", ctx::NO_TIME)));
            failed += 1;
            continue;
        }
        let name = src.name.clone();
        match process(&mut ctx, src, &root) {
            Ok(doc) => docs.push(doc),
            Err(why) if single => return Err(format!("{name}: {why}")),
            Err(why) => {
                docs.push(unread(&name, &format!("failed: {why}")));
                failed += 1;
            }
        }
    }
    if failed == docs.len() {
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
    let json = if figure_images && !ctx.parts.is_empty() {
        serde_json::to_string(&WithImages {
            response: Plain { documents: &docs },
            parts: &ctx.parts,
        })
    } else {
        serde_json::to_string(&Plain { documents: &docs })
    }
    .map_err(|e| format!("serializing the result: {e}"))?;
    if json.len() > OUTPUT_CAP_BYTES + 400_000 {
        return Err(format!(
            "the result is {} bytes, over the output limit; request fewer pages or files",
            json.len()
        ));
    }
    Ok(json)
}

fn unread(name: &str, why: &str) -> Document {
    Document {
        source: name.to_string(),
        format: "unknown",
        pages: 0,
        markdown: String::new(),
        figures: Vec::new(),
        warnings: vec![why.to_string()],
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

/// Reads a whole file in fixed chunks. One read call for a file this large asks
/// the runtime for one buffer of that size, which it refuses well below the
/// memory limit; chunks keep every call small.
fn read_file(path: &Path, len: u64) -> Result<Vec<u8>, String> {
    let fail = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::OutOfMemory {
            read_limit_error(len)
        } else {
            format!("cannot read the file: {e}")
        }
    };
    let mut file = std::fs::File::open(path).map_err(fail)?;
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(len as usize)
        .map_err(|_| read_limit_error(len))?;
    let mut chunk = vec![0u8; READ_CHUNK.min(len as usize).max(1)];
    loop {
        let n = file.read(&mut chunk).map_err(fail)?;
        if n == 0 {
            return Ok(buf);
        }
        buf.try_reserve(n).map_err(|_| read_limit_error(len))?;
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// The sentence for a file that does not fit in the plugin's memory.
fn read_limit_error(len: u64) -> String {
    format!(
        "the file is {} MiB, more than the plugin can hold in memory (limit {} MiB); split the file, or read part of a PDF with the pages option",
        len / 1024 / 1024,
        MAX_FILE_BYTES / 1024 / 1024
    )
}

fn process(ctx: &mut Ctx, src: Source, root: &Path) -> Result<Document, String> {
    let Source { name, data } = src;
    let (bytes, path): (Vec<u8>, Option<PathBuf>) = match data {
        Data::Inline(b) => (b, None),
        Data::Path(p) => {
            let meta = std::fs::metadata(&p).map_err(|e| format!("cannot read the file: {e}"))?;
            if !meta.is_file() {
                return Err("not a file".into());
            }
            if meta.len() > MAX_FILE_BYTES {
                return Err(read_limit_error(meta.len()));
            }
            (read_file(&p, meta.len())?, Some(p))
        }
    };
    let kind = detect::detect(&name, &bytes)?;
    let name = name.as_str();
    match kind {
        Kind::Pdf => pdf::convert(ctx, name, bytes),
        Kind::Image(format) => imgdoc::convert(ctx, name, format, &bytes),
        Kind::Html => {
            let base = path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            html_doc::convert_html(
                ctx,
                name,
                &bytes,
                &mut html_doc::DirResolver {
                    base,
                    root: root.to_path_buf(),
                },
            )
        }
        Kind::Text => text::convert_text(ctx, name, "text", &bytes),
        Kind::Markdown => text::convert_text(ctx, name, "markdown", &bytes),
        Kind::Csv => text::convert_csv(ctx, name, "csv", &bytes, false),
        Kind::Tsv => text::convert_csv(ctx, name, "tsv", &bytes, true),
        Kind::Epub => epub::convert_epub(ctx, name, &bytes),
        Kind::Docx => docx::convert_docx(ctx, name, &bytes),
        Kind::Pptx => pptx::convert_pptx(ctx, name, &bytes),
        Kind::Xlsx => xlsx::convert_xlsx(ctx, name, &bytes),
        Kind::Odt | Kind::Ods | Kind::Odp => odf::convert_odf(ctx, name, kind, &bytes),
    }
}
