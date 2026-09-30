//! Plain text, Markdown and delimited text (CSV, TSV).
use std::borrow::Cow;

use crate::ctx::Ctx;
use crate::detect::header;
use crate::model::{DocAcc, Document};
use crate::table::TableWriter;
use crate::util::decode_text;

/// Text and Markdown pass through with line endings normalised and NULs removed.
pub fn convert_text(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    data: &[u8],
) -> Result<Document, String> {
    let decoded = decode_text(data);
    // Most files have neither carriage returns nor NULs: then no copy is made.
    let text: Cow<'_, str> = if decoded.contains(['\r', '\0']) {
        Cow::Owned(decoded.replace("\r\n", "\n").replace(['\r', '\0'], "\n"))
    } else {
        decoded
    };
    let text = text.trim_matches('\n');
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, format));
    let kept = ctx.append_prefix(&mut acc, text);
    if kept < text.len() {
        let lines = text[..kept].bytes().filter(|&b| b == b'\n').count() + 1;
        let rest = text[kept..].bytes().filter(|&b| b == b'\n').count() + 1;
        acc.warn(format!("the output size limit cut the document after {lines} line(s) ({kept} of {} bytes); about {rest} more line(s) were not read, split the file to read the rest", text.len()));
    }
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format,
        pages: 1,
        markdown: acc.md,
        figures: Vec::new(),
        warnings,
    })
}

/// Picks the delimiter that splits the first line into the most fields.
fn sniff(first_line: &str, default: char) -> char {
    let mut best = (default, 0usize);
    for d in [',', ';', '\t', '|'] {
        let n = first_line.matches(d).count();
        if n > best.1 {
            best = (d, n);
        }
    }
    best.0
}

/// Reads one RFC 4180 record (quotes, doubled quotes, embedded newlines) from `s`.
fn record(s: &str, delim: char) -> (Vec<String>, &str) {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek().is_some_and(|&(_, n)| n == '"') => {
                field.push('"');
                chars.next();
            }
            '"' if in_quotes => in_quotes = false,
            '"' if field.is_empty() => in_quotes = true,
            c if c == delim && !in_quotes => fields.push(std::mem::take(&mut field)),
            '\n' if !in_quotes => {
                fields.push(field);
                return (fields, &s[i + 1..]);
            }
            '\r' if !in_quotes => {}
            c => field.push(c),
        }
    }
    fields.push(field);
    (fields, "")
}

pub fn convert_csv(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    data: &[u8],
    tab: bool,
) -> Result<Document, String> {
    let text = decode_text(data);
    let mut rest: &str = text.trim_start_matches('\u{feff}');
    let delim = sniff(
        rest.lines().next().unwrap_or(""),
        if tab { '\t' } else { ',' },
    );
    let mut acc = DocAcc::default();
    ctx.append(&mut acc, &header(source, format));
    let mut writer = TableWriter::new(1, 0);
    let mut first = true;
    while !rest.is_empty() {
        let (fields, next) = record(rest, delim);
        rest = next;
        if fields.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        if first {
            writer = TableWriter::new(fields.len(), ctx.budget.remaining().saturating_sub(4096));
            first = false;
        }
        if !writer.row(&fields) {
            break;
        }
    }
    if first {
        return Err("the file has no rows".into());
    }
    let written = writer.rows;
    if writer.full {
        let remaining = rest.bytes().filter(|&b| b == b'\n').count() + 1;
        acc.warn(format!("the output size limit cut the table after {written} row(s); about {remaining} more row(s) were not read"));
    }
    ctx.append(&mut acc, &writer.finish());
    let warnings = acc.finish_warnings();
    Ok(Document {
        source: source.to_string(),
        format,
        pages: 1,
        markdown: acc.md,
        figures: Vec::new(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Options;

    fn csv(data: &str) -> String {
        convert_csv(
            &mut Ctx::new(Options::default()),
            "t.csv",
            "csv",
            data.as_bytes(),
            false,
        )
        .unwrap()
        .markdown
    }

    #[test]
    fn quoted_fields_and_sniffed_delimiters() {
        assert_eq!(
            csv("a,b\n\"x, y\",\"he said \"\"hi\"\"\"\n"),
            "<!-- document: t.csv (csv) -->\n\n| a | b |\n| --- | --- |\n| x, y | he said \"hi\" |"
        );
        assert!(csv("a;b;c\n1;2;3").contains("| a | b | c |"));
        assert!(csv("a\tb\r\n1\t2\r\n").contains("| 1 | 2 |"));
    }

    #[test]
    fn embedded_newlines_stay_in_the_cell() {
        assert!(csv("a,b\n\"line1\nline2\",z").contains("| line1 line2 | z |"));
    }

    #[test]
    fn empty_input_fails_loudly() {
        assert!(
            convert_csv(
                &mut Ctx::new(Options::default()),
                "e.csv",
                "csv",
                b"\n\n",
                false
            )
            .is_err()
        );
    }

    #[test]
    fn text_over_the_output_cap_is_cut_at_a_line_not_dropped() {
        let line = "a line of the long book\n";
        let data = line.repeat(crate::model::OUTPUT_CAP_BYTES / line.len() + 5000);
        let doc = convert_text(
            &mut Ctx::new(Options::default()),
            "big.txt",
            "text",
            data.as_bytes(),
        )
        .unwrap();
        let body = crate::model::json_len(&doc.markdown);
        assert!(
            body > crate::model::OUTPUT_CAP_BYTES - 1000 && body <= crate::model::OUTPUT_CAP_BYTES,
            "{body}"
        );
        assert!(doc.markdown.ends_with("a line of the long book"));
        assert_eq!(doc.warnings.len(), 1, "{:?}", doc.warnings);
        assert!(
            doc.warnings[0].contains("cut the document after")
                && doc.warnings[0].contains("split the file"),
            "{:?}",
            doc.warnings
        );
    }

    #[test]
    fn one_line_longer_than_the_cap_is_cut_on_a_character_boundary() {
        let data = "\u{e9}".repeat(crate::model::OUTPUT_CAP_BYTES);
        let doc = convert_text(
            &mut Ctx::new(Options::default()),
            "line.txt",
            "text",
            data.as_bytes(),
        )
        .unwrap();
        assert!(
            doc.markdown.len() > 1_000_000
                && doc.markdown.chars().all(|c| c == '\u{e9}' || c.is_ascii())
        );
        assert_eq!(doc.warnings.len(), 1);
    }

    #[test]
    fn text_is_normalised() {
        let doc = convert_text(
            &mut Ctx::new(Options::default()),
            "a.txt",
            "text",
            b"\xEF\xBB\xBFone\r\ntwo\r\n",
        )
        .unwrap();
        assert_eq!(doc.markdown, "<!-- document: a.txt (text) -->\n\none\ntwo");
    }
}
