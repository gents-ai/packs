//! Generates one large input for measuring memory and time: a text file, CSV,
//! PDF, DOCX, EPUB or XLSX of a given size, written as a stream so the
//! generator itself stays small. Output is deterministic and is never
//! committed: run it into a temporary directory.
//!
//! Usage: cargo run --release --example gen_big -- <out-dir> <kind> <size-mib>
//!   kind: text | csv | pdf | docx | epub | xlsx
//! Prints the path of the file it wrote.
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const WORDS: [&str; 48] = [
    "the", "report", "shows", "that", "revenue", "grew", "across", "every", "region", "while",
    "costs", "stayed", "flat", "after", "the", "audit", "of", "north", "south", "east", "west",
    "quarter", "budget", "plan", "service", "latency", "queue", "database", "release", "team",
    "review", "owner", "risk", "metric", "target", "market", "supply", "demand", "forecast",
    "margin", "profit", "volume", "launch", "policy", "vendor", "ledger", "archive", "index",
];

/// A deterministic xorshift generator: the same bytes on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// `n` words and a few numbers, so the text deflates like prose, not like a repeat.
    fn words(&mut self, n: usize, out: &mut String) {
        for i in 0..n {
            if i > 0 {
                out.push(' ');
            }
            let r = self.next();
            if r.is_multiple_of(11) {
                out.push_str(&(r >> 20).to_string());
            } else {
                out.push_str(WORDS[(r >> 8) as usize % WORDS.len()]);
            }
        }
    }
}

fn zip_entry<W: Write + Seek>(zw: &mut ZipWriter<W>, name: &str, stored: bool) {
    let method = if stored {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    zw.start_file(
        name,
        SimpleFileOptions::default()
            .compression_method(method)
            .large_file(true),
    )
    .expect("start zip entry");
}

fn text(out: &mut impl Write, bytes: u64) {
    let (mut rng, mut done, mut line) = (Rng(0x9E37_79B9_7F4A_7C15), 0u64, String::new());
    let mut n = 0u64;
    while done < bytes {
        line.clear();
        rng.words(14, &mut line);
        if n.is_multiple_of(40) {
            line.push('\n');
        }
        line.push('\n');
        out.write_all(line.as_bytes()).expect("write");
        done += line.len() as u64;
        n += 1;
    }
}

fn csv(out: &mut impl Write, bytes: u64) {
    out.write_all(b"id,name,score,note\n").expect("write");
    let (mut rng, mut done, mut id, mut note) = (Rng(7), 0u64, 0u64, String::new());
    while done < bytes {
        note.clear();
        rng.words(6, &mut note);
        let row = format!("{id},item {id},{},{note}\n", rng.next() % 1000);
        out.write_all(row.as_bytes()).expect("write");
        done += row.len() as u64;
        id += 1;
    }
}

/// Pages of about 40 lines each, 100 pages per tree node, a classic cross-reference table.
fn pdf(out: &mut (impl Write + Seek), bytes: u64) {
    const PER_NODE: u64 = 100;
    const LINES: u64 = 40;
    let pages = (bytes / 4200).max(1);
    let nodes = pages.div_ceil(PER_NODE);
    let base = 4 + nodes;
    let (content, page) = (|k: u64| base + 2 * k, |k: u64| base + 2 * k + 1);
    let total = base + 2 * pages;
    let mut at = vec![0u64; total as usize];
    let mut put = |out: &mut dyn Write, pos: &mut u64, n: u64, body: &[u8]| {
        at[n as usize] = *pos;
        let text = format!("{n} 0 obj\n");
        out.write_all(text.as_bytes()).expect("write");
        out.write_all(body).expect("write");
        out.write_all(b"\nendobj\n").expect("write");
        *pos += (text.len() + body.len() + 8) as u64;
    };
    let mut pos = 0u64;
    let head = b"%PDF-1.7\n";
    out.write_all(head).expect("write");
    pos += head.len() as u64;
    put(out, &mut pos, 1, b"<< /Type /Catalog /Pages 3 0 R >>");
    put(
        out,
        &mut pos,
        2,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let mut rng = Rng(42);
    let (mut body, mut line) = (Vec::new(), String::new());
    for k in 0..pages {
        let mut stream = String::new();
        for i in 0..LINES {
            line.clear();
            rng.words(11, &mut line);
            stream.push_str(&format!(
                "BT /F1 10 Tf 50 {} Td ({line}) Tj ET\n",
                770 - i * 18
            ));
        }
        body.clear();
        body.extend_from_slice(
            format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()).as_bytes(),
        );
        put(out, &mut pos, content(k), &body);
        let leaf = 4 + k / PER_NODE;
        let dict = format!(
            "<< /Type /Page /Parent {leaf} 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 2 0 R >> >> >>",
            content(k)
        );
        put(out, &mut pos, page(k), dict.as_bytes());
    }
    for n in 0..nodes {
        let (lo, hi) = (n * PER_NODE, ((n + 1) * PER_NODE).min(pages));
        let kids: String = (lo..hi).map(|k| format!("{} 0 R ", page(k))).collect();
        let dict = format!(
            "<< /Type /Pages /Parent 3 0 R /Kids [{kids}] /Count {} >>",
            hi - lo
        );
        put(out, &mut pos, 4 + n, dict.as_bytes());
    }
    let kids: String = (0..nodes).map(|n| format!("{} 0 R ", 4 + n)).collect();
    put(
        out,
        &mut pos,
        3,
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").as_bytes(),
    );
    let mut table = format!("xref\n0 {total}\n0000000000 65535 f \n");
    for n in 1..total {
        table.push_str(&format!("{:010} 00000 n \n", at[n as usize]));
    }
    table.push_str(&format!(
        "trailer\n<< /Size {total} /Root 1 0 R >>\nstartxref\n{pos}\n%%EOF\n"
    ));
    out.write_all(table.as_bytes()).expect("write");
}

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const CT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>";

fn docx(out: impl Write + Seek, bytes: u64) {
    let mut zw = ZipWriter::new(out);
    zip_entry(&mut zw, "[Content_Types].xml", false);
    zw.write_all(CT.as_bytes()).expect("write");
    zip_entry(&mut zw, "word/styles.xml", false);
    zw.write_all(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:styles {W_NS}><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/></w:style></w:styles>").as_bytes()).expect("write");
    zip_entry(&mut zw, "word/document.xml", false);
    zw.write_all(
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document {W_NS}><w:body>").as_bytes(),
    )
    .expect("write");
    let (mut rng, mut done, mut n, mut para) = (Rng(11), 0u64, 0u64, String::new());
    while done < bytes {
        para.clear();
        rng.words(40, &mut para);
        let xml = if n % 200 == 0 {
            format!(
                "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Section {n}</w:t></w:r></w:p>"
            )
        } else {
            format!("<w:p><w:r><w:t>{para}</w:t></w:r></w:p>")
        };
        zw.write_all(xml.as_bytes()).expect("write");
        done += xml.len() as u64;
        n += 1;
    }
    zw.write_all(b"</w:body></w:document>").expect("write");
    zw.finish().expect("finish zip");
}

fn epub(out: impl Write + Seek, bytes: u64) {
    const CHAPTER: u64 = 256 * 1024;
    let chapters = (bytes / CHAPTER).max(1);
    let mut zw = ZipWriter::new(out);
    zip_entry(&mut zw, "mimetype", true);
    zw.write_all(b"application/epub+zip").expect("write");
    zip_entry(&mut zw, "META-INF/container.xml", false);
    zw.write_all(b"<?xml version=\"1.0\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>").expect("write");
    let items: String = (0..chapters)
        .map(|c| {
            format!(
                "<item id=\"c{c}\" href=\"text/ch{c}.xhtml\" media-type=\"application/xhtml+xml\"/>"
            )
        })
        .collect();
    let spine: String = (0..chapters)
        .map(|c| format!("<itemref idref=\"c{c}\"/>"))
        .collect();
    zip_entry(&mut zw, "OEBPS/content.opf", false);
    zw.write_all(format!("<?xml version=\"1.0\"?><package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"><manifest>{items}</manifest><spine>{spine}</spine></package>").as_bytes()).expect("write");
    let (mut rng, mut para) = (Rng(5), String::new());
    for c in 0..chapters {
        zip_entry(&mut zw, &format!("OEBPS/text/ch{c}.xhtml"), false);
        zw.write_all(format!("<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{c}</title></head><body><h1>Chapter {c}</h1>").as_bytes()).expect("write");
        let mut done = 0u64;
        while done < CHAPTER {
            para.clear();
            rng.words(60, &mut para);
            let p = format!("<p>{para}</p>\n");
            zw.write_all(p.as_bytes()).expect("write");
            done += p.len() as u64;
        }
        zw.write_all(b"</body></html>").expect("write");
    }
    zw.finish().expect("finish zip");
}

fn xlsx(out: impl Write + Seek, bytes: u64) {
    let ns = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
    let mut zw = ZipWriter::new(out);
    zip_entry(&mut zw, "[Content_Types].xml", false);
    zw.write_all(CT.as_bytes()).expect("write");
    zip_entry(&mut zw, "xl/workbook.xml", false);
    zw.write_all(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook {ns}><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>").as_bytes()).expect("write");
    zip_entry(&mut zw, "xl/_rels/workbook.xml.rels", false);
    zw.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"x/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>").expect("write");
    zip_entry(&mut zw, "xl/worksheets/sheet1.xml", false);
    zw.write_all(
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet {ns}><sheetData>").as_bytes(),
    )
    .expect("write");
    let (mut rng, mut done, mut r, mut note) = (Rng(3), 0u64, 1u64, String::new());
    while done < bytes {
        note.clear();
        rng.words(4, &mut note);
        let row = format!(
            "<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c><c r=\"B{r}\" t=\"inlineStr\"><is><t>{note}</t></is></c><c r=\"C{r}\"><v>{}</v></c></row>",
            rng.next() % 100_000
        );
        zw.write_all(row.as_bytes()).expect("write");
        done += row.len() as u64;
        r += 1;
    }
    zw.write_all(b"</sheetData></worksheet>").expect("write");
    zw.finish().expect("finish zip");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, kind, mib] = args.as_slice() else {
        eprintln!("usage: gen_big <out-dir> <text|csv|pdf|docx|epub|xlsx> <size-mib>");
        std::process::exit(2);
    };
    let bytes: u64 = mib.parse::<u64>().unwrap_or_else(|_| {
        eprintln!("size-mib must be a number");
        std::process::exit(2);
    }) * 1024
        * 1024;
    std::fs::create_dir_all(dir).expect("create the output directory");
    let ext = match kind.as_str() {
        "text" => "txt",
        k @ ("csv" | "pdf" | "docx" | "epub" | "xlsx") => k,
        other => {
            eprintln!("unknown kind {other}");
            std::process::exit(2);
        }
    };
    let path = Path::new(dir).join(format!("big.{ext}"));
    let mut out = BufWriter::with_capacity(1 << 20, File::create(&path).expect("create the file"));
    match kind.as_str() {
        "text" => text(&mut out, bytes),
        "csv" => csv(&mut out, bytes),
        "pdf" => pdf(&mut out, bytes),
        "docx" => docx(&mut out, bytes),
        "epub" => epub(&mut out, bytes),
        _ => xlsx(&mut out, bytes),
    }
    out.flush().expect("flush");
    println!("{}", path.display());
}
