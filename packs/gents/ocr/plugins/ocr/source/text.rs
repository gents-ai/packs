//! Plain text, Markdown and delimited text (CSV, TSV), read in windows so a
//! file of any size costs one window of memory, and a call that stops at the
//! output limit leaves the byte offset where the next one continues.
use crate::ctx::Ctx;
use crate::detect::header;
use crate::model::{DocAcc, Document, json_char_len};
use crate::resume::Resume;
use crate::src::Src;
use crate::table::TableWriter;
use crate::textio::{Stream, Window, read_window, sniff, window_bytes};

/// Margin under the budget for the joiner and the quotes around a chunk.
const CHUNK_OVERHEAD: usize = 4;

fn io_err(e: std::io::Error) -> String {
    format!("cannot read the file: {e}")
}

/// What a window of text gives: the normalised text to deliver, where the
/// rest starts in the window, and the line breaks on either side of the cut.
struct Cut {
    text: String,
    /// Offset in the window's text where the next read starts; the text length when done.
    end: usize,
    /// Line breaks that ended the delivered text (before trimming), and that began it.
    trailing: usize,
    leading: usize,
    done: bool,
}

/// Normalises line endings (CRLF and CR become LF, NUL becomes LF), drops the
/// line breaks that open the window and takes as much as fits `room` JSON
/// bytes, cut at the end of a line, or at a character when one line alone is
/// too long. At the end of the file trailing line breaks are dropped.
fn cut_text(w: &Window, room: usize) -> Cut {
    let mut out = String::new();
    let mut cost = 0usize;
    let mut leading = 0usize;
    let mut last_nl: Option<(usize, usize)> = None;
    let mut chars = w.text.char_indices().peekable();
    let mut end = w.text.len();
    let mut stopped = false;
    while let Some((i, c)) = chars.next() {
        let mut next_off = i + c.len_utf8();
        let ch = match c {
            '\r' => match chars.peek() {
                Some(&(_, '\n')) => {
                    chars.next();
                    next_off += 1;
                    '\n'
                }
                None if !w.eof => {
                    end = i;
                    stopped = true;
                    break;
                }
                _ => '\n',
            },
            '\0' => '\n',
            c => c,
        };
        if ch == '\n' && out.is_empty() {
            leading += 1;
            continue;
        }
        let c_cost = json_char_len(ch);
        if cost + c_cost > room {
            end = i;
            stopped = true;
            break;
        }
        out.push(ch);
        cost += c_cost;
        if ch == '\n' {
            last_nl = Some((out.len(), next_off));
        }
    }
    let done = !stopped && w.eof;
    if !done
        && (stopped || !w.eof)
        && let Some((olen, off)) = last_nl
    {
        out.truncate(olen);
        end = off;
    }
    let trimmed = out.trim_end_matches('\n').len();
    let trailing = out.len() - trimmed;
    out.truncate(trimmed);
    Cut {
        text: out,
        end,
        trailing,
        leading,
        done,
    }
}

/// Text and Markdown pass through with line endings normalised and NULs removed.
pub fn convert_text(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    src: &mut Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let st = sniff(src).map_err(io_err)?;
    let pos = resume.map_or(st.bom, |r| r.pos.max(st.bom));
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    if resume.is_none() {
        ctx.append(&mut acc, &header(source, format));
    }
    let room = ctx.budget.remaining().saturating_sub(CHUNK_OVERHEAD);
    let w = read_window(src, st, pos, window_bytes(st.enc, room)).map_err(io_err)?;
    let cut = cut_text(&w, room);
    if !cut.text.is_empty() {
        ctx.append(&mut acc, &cut.text);
        ctx.emitted += 1;
    }
    let next = (!cut.done).then(|| Resume {
        unit: 1,
        pos: pos + w.raw_of(cut.end),
        joint: cut.trailing.min(2) as u8,
        ..Resume::default()
    });
    let mut doc = ctx.document(acc, source, format, 1, resume, next, parts_from);
    if let Some(r) = resume {
        let n = usize::from(r.joint) + cut.leading;
        doc.joint = Some(match n {
            0 => "none",
            1 => "line",
            _ => "blank",
        });
    }
    Ok(doc)
}

/// Picks the delimiter that splits the first line into the most fields.
fn sniff_delimiter(first_line: &str, default: char) -> char {
    let mut best = (default, 0usize);
    for d in [',', ';', '\t', '|'] {
        let n = first_line.matches(d).count();
        if n > best.1 {
            best = (d, n);
        }
    }
    best.0
}

/// One RFC 4180 record (quotes, doubled quotes, embedded newlines) from the
/// start of `s`: its fields, the bytes it took and whether a line break ended it.
fn record(s: &str, delim: char) -> (Vec<String>, usize, bool) {
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
                return (fields, i + 1, true);
            }
            '\r' if !in_quotes => {}
            c => field.push(c),
        }
    }
    fields.push(field);
    (fields, s.len(), false)
}

/// The first non-empty record of the file and the delimiter that reads it.
fn first_record(src: &mut Src, st: Stream, default: char) -> Result<(char, Vec<String>), String> {
    let w = read_window(src, st, st.bom, 256 * 1024).map_err(io_err)?;
    let mut rest = w.text.as_str();
    let delim = sniff_delimiter(rest.lines().next().unwrap_or(""), default);
    while !rest.is_empty() {
        let (fields, used, _) = record(rest, delim);
        rest = &rest[used..];
        if fields.iter().any(|f| !f.trim().is_empty()) {
            return Ok((delim, fields));
        }
    }
    Err("the file has no rows".into())
}

pub fn convert_csv(
    ctx: &mut Ctx,
    source: &str,
    format: &'static str,
    src: &mut Src,
    tab: bool,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let st = sniff(src).map_err(io_err)?;
    let (delim, first) = first_record(src, st, if tab { '\t' } else { ',' })?;
    let cols = first.len();
    let pos = resume.map_or(st.bom, |r| r.pos.max(st.bom));
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    if resume.is_none() {
        ctx.append(&mut acc, &header(source, format));
    }
    let budget = ctx.table_budget();
    let mut writer = TableWriter::new(cols, budget, resume.is_none());
    let w = read_window(src, st, pos, window_bytes(st.enc, budget)).map_err(io_err)?;
    let mut rest = w.text.as_str();
    let mut consumed = 0usize;
    let mut finished = true;
    while !rest.is_empty() {
        let (fields, used, terminated) = record(rest, delim);
        if !terminated && !w.eof && writer.rows > 0 {
            finished = false;
            break;
        }
        if fields.iter().all(|f| f.trim().is_empty()) {
            rest = &rest[used..];
            consumed += used;
            continue;
        }
        if !writer.row(&fields) {
            if writer.rows == 0 {
                acc.warn(format!(
                    "a row of {used} bytes is larger than the output limit and was skipped"
                ));
                writer.reset_full();
                rest = &rest[used..];
                consumed += used;
                continue;
            }
            finished = false;
            break;
        }
        rest = &rest[used..];
        consumed += used;
    }
    let done = finished && w.eof;
    let next = (!done).then(|| Resume {
        unit: 1,
        pos: pos + w.raw_of(consumed),
        joint: 1,
        ..Resume::default()
    });
    let table_header = resume.map(|_| TableWriter::header_of(cols, &first));
    if writer.rows > 0 {
        ctx.append(&mut acc, &writer.finish());
        ctx.emitted += 1;
    }
    let mut doc = ctx.document(acc, source, format, 1, resume, next, parts_from);
    doc.table_header = table_header;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Options;

    fn opts(max_bytes: usize) -> Options {
        Options {
            max_bytes,
            ..Options::default()
        }
    }

    fn text_doc(data: &[u8], max_bytes: usize, at: Option<&Resume>) -> Document {
        convert_text(
            &mut Ctx::new(opts(max_bytes)),
            "t.txt",
            "text",
            &mut Src::mem(data.to_vec()),
            at,
        )
        .unwrap()
    }

    fn csv_doc(data: &str, max_bytes: usize, at: Option<&Resume>) -> Document {
        convert_csv(
            &mut Ctx::new(opts(max_bytes)),
            "t.csv",
            "csv",
            &mut Src::mem(data.as_bytes().to_vec()),
            false,
            at,
        )
        .unwrap()
    }

    fn csv(data: &str) -> String {
        csv_doc(data, crate::model::OUTPUT_CAP_BYTES, None).markdown
    }

    /// Reads a document call after call and joins the slices as their `joint` says.
    fn joined(read: impl Fn(Option<&Resume>) -> Document) -> (String, usize) {
        let (mut out, mut calls, mut at) = (String::new(), 0, None::<Resume>);
        loop {
            let doc = read(at.as_ref());
            calls += 1;
            match (doc.joint, out.is_empty()) {
                (Some("blank"), false) => out.push_str("\n\n"),
                (Some("line"), false) => out.push('\n'),
                _ => {}
            }
            out.push_str(&doc.markdown);
            match doc.next {
                Some(next) => at = Some(next),
                None => return (out, calls),
            }
            assert!(calls < 10_000, "no progress");
        }
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
                &mut Src::mem(b"\n\n".to_vec()),
                false,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn text_over_the_output_cap_continues_at_a_line_and_rejoins_exactly() {
        let line = "a line of the long book\n";
        let data = line.repeat(crate::model::OUTPUT_CAP_BYTES / line.len() + 5000);
        let doc = text_doc(data.as_bytes(), crate::model::OUTPUT_CAP_BYTES, None);
        let body = crate::model::json_len(&doc.markdown);
        assert!(
            body > crate::model::OUTPUT_CAP_BYTES - 1000 && body <= crate::model::OUTPUT_CAP_BYTES,
            "{body}"
        );
        assert!(doc.markdown.ends_with("a line of the long book"));
        assert!(doc.warnings.is_empty() && doc.next.is_some());
        let (all, calls) =
            joined(|at| text_doc(data.as_bytes(), crate::model::OUTPUT_CAP_BYTES, at));
        assert_eq!(calls, 2);
        assert_eq!(
            all,
            format!("<!-- document: t.txt (text) -->\n\n{}", data.trim_end())
        );
    }

    #[test]
    fn one_line_longer_than_a_call_continues_on_a_character_boundary() {
        let data = "\u{e9}".repeat(10_000);
        let (all, calls) = joined(|at| text_doc(data.as_bytes(), 4096, at));
        assert!(calls > 2);
        assert_eq!(all, format!("<!-- document: t.txt (text) -->\n\n{data}"));
    }

    #[test]
    fn slices_rejoin_with_blank_lines_crlf_and_windows_1252() {
        let mut data = Vec::new();
        for i in 0..4000 {
            data.extend_from_slice(format!("paragraph {i} caf\u{e9}\r\n\r\n").as_bytes());
        }
        let latin1: Vec<u8> = String::from_utf8(data.clone())
            .unwrap()
            .chars()
            .map(|c| if c == '\u{e9}' { 0xE9 } else { c as u8 })
            .collect();
        for bytes in [data, latin1] {
            let (all, calls) = joined(|at| text_doc(&bytes, 4096, at));
            let whole = text_doc(&bytes, crate::model::OUTPUT_CAP_BYTES, None);
            assert!(whole.next.is_none() && calls > 5);
            assert_eq!(all, whole.markdown);
        }
    }

    #[test]
    fn text_is_normalised() {
        let doc = text_doc(b"\xEF\xBB\xBFone\r\ntwo\r\n", 4096, None);
        assert_eq!(doc.markdown, "<!-- document: t.txt (text) -->\n\none\ntwo");
    }

    #[test]
    fn a_table_continues_by_rows_and_names_its_header() {
        let mut data = String::from("id,text\n");
        for i in 0..3000 {
            data.push_str(&format!("{i},row number {i}\n"));
        }
        let whole = csv_doc(&data, crate::model::OUTPUT_CAP_BYTES, None);
        assert!(whole.next.is_none() && whole.table_header.is_none());
        let first = csv_doc(&data, 8192, None);
        let at = first.next.clone().expect("the first slice stops early");
        let second = csv_doc(&data, 8192, Some(&at));
        assert_eq!(
            second.table_header.as_deref(),
            Some("| id | text |\n| --- | --- |")
        );
        assert_eq!(second.joint, Some("line"));
        assert!(!second.markdown.contains("| --- |") && !second.markdown.contains("document:"));
        let (all, calls) = joined(|at| csv_doc(&data, 8192, at));
        assert!(calls > 3);
        assert_eq!(all, whole.markdown);
    }
}
