mod render;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Component, Path},
};
use zip::{ZipWriter, write::SimpleFileOptions};

const MAX_MANUSCRIPT: u64 = 16 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    run_id: Option<String>,
    book_id: Option<String>,
    edition_id: Option<String>,
    path: String,
    path_original: Option<String>,
    #[serde(default = "default_manuscript")]
    manuscript: String,
    output: String,
}
fn default_manuscript() -> String {
    "manuscript.json".into()
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manuscript {
    identifier: String,
    title: String,
    author: String,
    language: String,
    modified: String,
    chapters: Vec<Chapter>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Chapter {
    title: String,
    #[serde(default)]
    paragraphs: Vec<String>,
    #[serde(default)]
    id: String,
    #[serde(default = "one")]
    level: u64,
    #[serde(default)]
    matter_type: String,
    #[serde(default)]
    blocks: Vec<Block>,
}
fn one() -> u64 { 1 }
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Block {
    id: String,
    markdown: String,
    sources: Vec<SourceSpan>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceSpan {
    source: String,
    page: u64,
    start_byte: usize,
    end_byte: usize,
}

fn main() {
    let result = (|| -> Result<Value, String> {
        let mut raw = String::new();
        io::stdin()
            .take(65537)
            .read_to_string(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() > 65536 {
            return Err("request too large; pass a manuscript file name".into());
        }
        let request = serde_json::from_str(&raw).map_err(|e| format!("invalid request: {e}"))?;
        run(request)
    })();
    match result {
        Ok(value) => {
            if let Err(e) = serde_json::to_writer(io::stdout().lock(), &value) {
                eprintln!("shelf_export: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("shelf_export: {e}");
            std::process::exit(1);
        }
    }
}

fn filename(value: &str) -> Result<(), String> {
    let mut parts = Path::new(value).components();
    if value.is_empty()
        || value.contains(['/', '\\'])
        || !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
    {
        return Err("use simple manuscript and output file names inside path".into());
    }
    Ok(())
}

fn run(request: Request) -> Result<Value, String> {
    if request.run_id.is_some()
        && [&request.run_id, &request.book_id, &request.edition_id]
            .iter()
            .any(|v| v.as_deref().is_none_or(|s| s.trim().is_empty()))
    {
        return Err("an export job requires nonempty run_id, book_id and edition_id".into());
    }
    filename(&request.manuscript)?;
    filename(&request.output)?;
    if !request.output.ends_with(".epub") {
        return Err("output must end with .epub".into());
    }
    // The host canonicalizes the binding. WASI cannot resolve its unshared ancestors.
    let root = Path::new(&request.path);
    if !root.is_dir() {
        return Err("path must name an accessible bound folder".into());
    }
    let source = root.join(&request.manuscript);
    let source_meta = fs::symlink_metadata(&source).map_err(|e| format!("open manuscript: {e}"))?;
    if !source_meta.file_type().is_file() {
        return Err("manuscript must be a regular file directly inside the bound folder".into());
    }
    let mut raw = Vec::new();
    fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(MAX_MANUSCRIPT + 1)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() as u64 > MAX_MANUSCRIPT {
        return Err("manuscript exceeds 16 MiB; export a smaller edition".into());
    }
    let manuscript: Manuscript = serde_json::from_slice(&raw)
        .map_err(|e| format!("invalid manuscript: {e}; see shelf_export help"))?;
    let bytes = epub(&manuscript)?;
    let output = root.join(&request.output);
    // Callback delivery can repeat after a file was written but before its receipt
    // committed. Identical bytes are reusable; different editions and symlinks are refused.
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
    {
        Ok(mut file) => {
            if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = fs::remove_file(&output);
                return Err(format!(
                    "write edition: {error}; retry with an available output folder"
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&output).map_err(|e| e.to_string())?;
            if !meta.file_type().is_file()
                || meta.len() != bytes.len() as u64
                || fs::read(&output).map_err(|e| e.to_string())? != bytes
            {
                return Err(
                    "output already exists with different content; choose a new output file name"
                        .into(),
                );
            }
        }
        Err(error) => {
            return Err(format!(
                "create edition: {error}; choose an available output folder"
            ));
        }
    }
    let mut receipt = json!({"path": Path::new(request.path_original.as_deref().unwrap_or(&request.path)).join(&request.output),
        "sha256":format!("{:x}",Sha256::digest(&bytes)),"chapter_count":manuscript.chapters.len(),"bytes":bytes.len()});
    if request.run_id.is_some() {
        receipt.as_object_mut().unwrap().remove("bytes");
        receipt["book_id"] = json!(request.book_id);
        receipt["edition_id"] = json!(request.edition_id);
        Ok(receipt)
    } else {
        Ok(receipt)
    }
}

fn xml(text: &str) -> Result<String, String> {
    if text.chars().any(|c| !matches!(c, '\u{9}' | '\u{a}' | '\u{d}' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        return Err("manuscript contains characters XML cannot represent; correct the source text".into());
    }
    Ok(text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}

fn timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 20
        || [4, 7, 10, 13, 16, 19]
            .into_iter()
            .zip(b"--T::Z")
            .any(|(i, c)| b[i] != *c)
        || b.iter()
            .enumerate()
            .any(|(i, c)| ![4, 7, 10, 13, 16, 19].contains(&i) && !c.is_ascii_digit())
    {
        return false;
    }
    let n = |a: usize, z: usize| s[a..z].parse::<u32>().unwrap_or(0);
    let year = n(0, 4);
    let month = n(5, 7);
    let day = n(8, 10);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days && n(11, 13) < 24 && n(14, 16) < 60 && n(17, 19) < 60
}

fn epub(book: &Manuscript) -> Result<Vec<u8>, String> {
    for (name, value) in [
        ("identifier", &book.identifier),
        ("title", &book.title),
        ("author", &book.author),
        ("language", &book.language),
    ] {
        if value.trim().is_empty() {
            return Err(format!("manuscript {name} must not be empty"));
        }
    }
    if !timestamp(&book.modified) {
        return Err("modified must be a valid UTC timestamp YYYY-MM-DDTHH:MM:SSZ".into());
    }
    if book.chapters.is_empty() || book.chapters.len() > 10000 {
        return Err("provide 1 to 10000 ordered chapters".into());
    }
    let title = xml(&book.title)?;
    let language = xml(&book.language)?;
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let mut add = |name: &str, content: &str| -> Result<(), String> {
        archive
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .map_err(|e| e.to_string())?;
        archive
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())
    };
    // EPUB readers require mimetype first, stored without compression or an extra field.
    add("mimetype", "application/epub+zip")?;
    add(
        "META-INF/container.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    )?;
    let names: Vec<String> = book.chapters.iter().enumerate().map(|(i,c)| {
        if c.id.is_empty() { format!("chapter-{}.xhtml",i+1) } else { format!("{}.xhtml",c.id) }
    }).collect();
    let mut ids = std::collections::BTreeSet::new();
    for (chapter,name) in book.chapters.iter().zip(&names) {
        filename(name)?;
        if !ids.insert(name.clone()) || (!chapter.id.is_empty() && !render::valid_id(&chapter.id)) {return Err("section IDs must be unique XML IDs".into());}
        for b in &chapter.blocks {
            if !render::valid_id(&b.id) || !ids.insert(b.id.clone()) {return Err("passage IDs must be unique XML IDs".into());}
            for source in &b.sources {if source.source.is_empty() || source.page==0 || source.end_byte<source.start_byte {return Err("invalid source span".into());}}
        }
    }
    let navigation=render::navigation(book,&names)?;
    let mut manifest=String::new();
    let mut spine=String::from(r#"<itemref idref="nav"/>"#);
    let mut pages=std::collections::BTreeSet::new();
    let mut page_list=String::new();
    let mut citations=Vec::new();
    add("OEBPS/style.css",render::CSS)?;
    for (i,chapter) in book.chapters.iter().enumerate() {
        if chapter.title.trim().is_empty() || (chapter.paragraphs.is_empty() && chapter.blocks.is_empty()) || !(1..=6).contains(&chapter.level) {
            return Err(format!("chapter {} needs a title, level 1-6, and content",i+1));
        }
        let mut content=String::new();
        let is_contents=chapter.title.trim().eq_ignore_ascii_case("contents") || chapter.title.trim().eq_ignore_ascii_case("table of contents");
        for block in &chapter.blocks {
            let mut markers=String::new();
            for source in &block.sources {
                if pages.insert((source.source.clone(),source.page)) {
                    let anchor=render::page_id(&source.source,source.page);
                    markers.push_str(&format!(r#"<span epub:type="pagebreak" role="doc-pagebreak" id="{anchor}" aria-label="Scan page {}"/>"#,source.page));
                    page_list.push_str(&format!(r#"<li><a href="{}#{anchor}">{} — scan {}</a></li>"#,names[i],xml(&source.source)?,source.page));
                }
            }
            citations.push(json!({"passage_id":block.id,"chapter_id":chapter.id,"chapter_title":chapter.title,"epub_href":format!("OEBPS/{}#{}",names[i],block.id),"markdown":block.markdown,"source_spans":block.sources}));
            let text=render::markdown(&block.markdown)?;
            content.push_str(&format!(r#"<div id="{}" class="passage{}">{markers}{text}</div>"#,block.id,if is_contents {" source-contents"}else{""}));
        }
        if is_contents {content=format!(r#"<nav aria-label="Contents">{navigation}</nav><details><summary>Original contents and printed page numbers</summary>{content}</details>"#);}
        for p in &chapter.paragraphs {if p.trim().is_empty(){return Err("empty paragraph".into());}content.push_str(&format!("<p>{}</p>",xml(p)?));}
        add(&format!("OEBPS/{}",names[i]),&format!(r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{language}"><head><title>{}</title><link rel="stylesheet" type="text/css" href="style.css"/></head><body class="{}"><section epub:type="chapter" id="section"><h1>{}</h1>{content}</section></body></html>"#,xml(&chapter.title)?,xml(&chapter.matter_type)?,xml(&chapter.title)?))?;
        manifest.push_str(&format!(r#"<item id="c{i}" href="{}" media-type="application/xhtml+xml"/>"#,names[i]));
        spine.push_str(&format!(r#"<itemref idref="c{i}"/>"#));
    }
    add("OEBPS/citations.json",&serde_json::to_string(&json!({"edition_id":book.identifier,"passages":citations})).map_err(|e|e.to_string())?)?;
    let page_nav=if page_list.is_empty(){String::new()}else{format!(r#"<nav epub:type="page-list" hidden="hidden"><h2>Source scan pages</h2><ol>{page_list}</ol></nav>"#)};
    add("OEBPS/nav.xhtml",&format!(r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{language}"><head><title>Contents</title><link rel="stylesheet" type="text/css" href="style.css"/></head><body><nav epub:type="toc" id="toc"><h1>Contents</h1>{navigation}</nav>{page_nav}</body></html>"#))?;
    add("OEBPS/package.opf",&format!(r#"<?xml version="1.0" encoding="UTF-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id" xml:lang="{language}"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">{}</dc:identifier><dc:title>{title}</dc:title><dc:creator>{}</dc:creator><dc:language>{language}</dc:language><meta property="dcterms:modified">{}</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="css" href="style.css" media-type="text/css"/><item id="citations" href="citations.json" media-type="application/json"/>{manifest}</manifest><spine>{spine}</spine></package>"#,xml(&book.identifier)?,xml(&book.author)?,book.modified))?;
    archive
        .finish()
        .map(|c| c.into_inner())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn book() -> Manuscript {
        serde_json::from_str(r#"{"identifier":"book:1","title":"A & B","author":"Zoë","language":"en","modified":"2026-10-08T00:00:00Z","chapters":[{"title":"<One>","paragraphs":["Don't lose <words> & symbols."]},{"title":"Two","paragraphs":["尾声"]}]}"#).unwrap()
    }
    #[test]
    fn readable_epub_has_ordered_spine_navigation_and_literal_unicode_text() {
        let bytes = epub(&book()).unwrap();
        assert_eq!(bytes, epub(&book()).unwrap());
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        {
            let mut first = zip.by_index(0).unwrap();
            assert_eq!(first.name(), "mimetype");
            assert_eq!(first.compression(), zip::CompressionMethod::Stored);
            let mut s = String::new();
            first.read_to_string(&mut s).unwrap();
            assert_eq!(s, "application/epub+zip");
        }
        for name in [
            "META-INF/container.xml",
            "OEBPS/package.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/chapter-1.xhtml",
            "OEBPS/chapter-2.xhtml",
        ] {
            let mut s = String::new();
            zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
            let doc = roxmltree::Document::parse(&s).unwrap();
            if name.ends_with("package.opf") {
                assert_eq!(
                    doc.descendants()
                        .filter(|n| n.has_tag_name(("http://www.idpf.org/2007/opf", "itemref")))
                        .map(|n| n.attribute("idref").unwrap())
                        .collect::<Vec<_>>(),
                    ["nav", "c0", "c1"]
                );
            }
            if name.ends_with("chapter-1.xhtml") {
                assert!(
                    doc.descendants()
                        .any(|n| n.text() == Some("Don't lose <words> & symbols."))
                );
            }
            if name.ends_with("chapter-2.xhtml") {
                assert!(doc.descendants().any(|n| n.text() == Some("尾声")));
            }
        }
    }
    #[test]
    fn rejects_invalid_or_empty_content() {
        let mut b = book();
        b.chapters.clear();
        assert!(epub(&b).is_err());
        let mut b = book();
        b.chapters[0].paragraphs[0] = "bad\0text".into();
        assert!(epub(&b).is_err());
        for s in [
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "今天",
        ] {
            assert!(!timestamp(s));
        }
        assert!(timestamp("2024-02-29T23:59:59Z"));
    }
    #[test]
    fn export_preserves_existing_editions_and_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = include_str!("../tests/fixtures/manuscript.json");
        fs::write(dir.path().join("manuscript.json"), fixture).unwrap();
        let req = || Request {
            run_id: None,
            book_id: None,
            edition_id: None,
            path: dir.path().to_string_lossy().into_owned(),
            path_original: None,
            manuscript: default_manuscript(),
            output: "edition.epub".into(),
        };
        let receipt = run(req()).unwrap();
        assert_eq!(receipt["chapter_count"], 2);
        let before = fs::read(dir.path().join("edition.epub")).unwrap();
        assert_eq!(run(req()).unwrap(), receipt);
        fs::write(
            dir.path().join("manuscript.json"),
            fixture.replace("Fixture Book", "Different Book"),
        )
        .unwrap();
        assert!(run(req()).is_err());
        assert_eq!(before, fs::read(dir.path().join("edition.epub")).unwrap());
        for name in [
            "../escape.epub",
            "/tmp/escape.epub",
            "a/b.epub",
            "..",
            "a\\b.epub",
        ] {
            assert!(filename(name).is_err());
        }
    }
}
