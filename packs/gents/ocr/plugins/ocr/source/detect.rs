//! Format detection from magic bytes first and the file extension second, so a
//! renamed file is still read as what it is and a wrong extension fails loudly.
use crate::util::Zip;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Pdf,
    Epub,
    Docx,
    Pptx,
    Xlsx,
    Odt,
    Ods,
    Odp,
    Html,
    Markdown,
    Text,
    Csv,
    Tsv,
    Image(&'static str),
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pdf => "pdf",
            Kind::Epub => "epub",
            Kind::Docx => "docx",
            Kind::Pptx => "pptx",
            Kind::Xlsx => "xlsx",
            Kind::Odt => "odt",
            Kind::Ods => "ods",
            Kind::Odp => "odp",
            Kind::Html => "html",
            Kind::Markdown => "markdown",
            Kind::Text => "text",
            Kind::Csv => "csv",
            Kind::Tsv => "tsv",
            Kind::Image(name) => name,
        }
    }
}

/// The marker line that opens every document's Markdown.
pub fn header(source: &str, format: &str) -> String {
    format!(
        "<!-- document: {} ({format}) -->",
        source.replace('>', "&gt;")
    )
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn image_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else if data.starts_with(b"BM") && data.len() > 26 {
        Some("bmp")
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Some("tiff")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn zip_kind(data: &[u8]) -> Result<Kind, String> {
    let mut zip = Zip::open(data)?;
    if let Some(mime) = zip.read("mimetype")? {
        let mime = String::from_utf8_lossy(&mime);
        let mime = mime.trim();
        match mime {
            "application/epub+zip" => return Ok(Kind::Epub),
            m if m.starts_with("application/vnd.oasis.opendocument.text") => return Ok(Kind::Odt),
            m if m.starts_with("application/vnd.oasis.opendocument.spreadsheet") => {
                return Ok(Kind::Ods);
            }
            m if m.starts_with("application/vnd.oasis.opendocument.presentation") => {
                return Ok(Kind::Odp);
            }
            _ => {}
        }
    }
    if zip.has("word/document.xml") {
        Ok(Kind::Docx)
    } else if zip.has("ppt/presentation.xml") {
        Ok(Kind::Pptx)
    } else if zip.has("xl/workbook.xml") {
        Ok(Kind::Xlsx)
    } else if zip.has("META-INF/container.xml") {
        Ok(Kind::Epub)
    } else {
        Err("this zip archive is not an EPUB, DOCX, PPTX, XLSX, ODT, ODS or ODP file".into())
    }
}

pub fn detect(name: &str, data: &[u8]) -> Result<Kind, String> {
    let ext = extension(name);
    let head = &data[..data.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Ok(Kind::Pdf);
    }
    if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") {
        return zip_kind(data);
    }
    if let Some(kind) = image_kind(data) {
        return Ok(Kind::Image(kind));
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Err("legacy binary Office files (.doc, .xls, .ppt) are not supported; save the file as DOCX, XLSX or PPTX".into());
    }
    if data.starts_with(b"{\\rtf") {
        return Err("RTF files are not supported; save the file as DOCX, ODT or plain text".into());
    }
    if matches!(
        ext.as_str(),
        "pdf"
            | "epub"
            | "docx"
            | "pptx"
            | "xlsx"
            | "odt"
            | "ods"
            | "odp"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "tif"
            | "tiff"
            | "webp"
    ) {
        return Err(format!(
            "the file is named .{ext} but its content is not a valid {} file",
            ext.to_uppercase()
        ));
    }
    match ext.as_str() {
        "txt" | "text" | "log" => Ok(Kind::Text),
        "md" | "markdown" => Ok(Kind::Markdown),
        "csv" => Ok(Kind::Csv),
        "tsv" | "tab" => Ok(Kind::Tsv),
        "html" | "htm" | "xhtml" | "xht" => Ok(Kind::Html),
        _ => {
            let sniff = String::from_utf8_lossy(head).to_ascii_lowercase();
            let sniff = sniff.trim_start_matches('\u{feff}').trim_start();
            if sniff.starts_with("<!doctype html") || sniff.starts_with("<html") {
                Ok(Kind::Html)
            } else if ext.is_empty() {
                Err(
                    "cannot tell the format: the file has no extension and no recognised signature"
                        .into(),
                )
            } else {
                Err(format!(
                    "unsupported format .{ext}; supported are PDF, EPUB, DOCX, PPTX, XLSX, ODT, ODS, ODP, HTML, Markdown, text, CSV, TSV and PNG, JPEG, GIF, BMP, TIFF, WebP images"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_beat_a_wrong_extension() {
        assert_eq!(detect("scan.txt", b"%PDF-1.7\n").unwrap(), Kind::Pdf);
        assert_eq!(
            detect("x.bin", b"\x89PNG\r\n\x1a\nrest").unwrap(),
            Kind::Image("png")
        );
        assert_eq!(detect("page", b"<!DOCTYPE html><p>x").unwrap(), Kind::Html);
    }

    #[test]
    fn text_formats_by_extension_and_failures_say_why() {
        assert_eq!(detect("a.csv", b"a,b").unwrap(), Kind::Csv);
        assert_eq!(detect("a.md", b"# x").unwrap(), Kind::Markdown);
        assert!(
            detect("a.pdf", b"not a pdf")
                .unwrap_err()
                .contains("not a valid PDF")
        );
        assert!(
            detect("a.xyz", b"data")
                .unwrap_err()
                .contains("unsupported format .xyz")
        );
        assert!(
            detect("a.doc", &[0xD0, 0xCF, 0x11, 0xE0, 0])
                .unwrap_err()
                .contains("legacy binary Office")
        );
        assert!(
            detect("noext", b"data")
                .unwrap_err()
                .contains("no extension")
        );
    }

    #[test]
    fn headers_cannot_break_out_of_the_comment() {
        assert_eq!(
            header("a-->b.pdf", "pdf"),
            "<!-- document: a--&gt;b.pdf (pdf) -->"
        );
    }
}
