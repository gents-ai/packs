//! XLSX: every worksheet as a Markdown table, streamed row by row so a large
//! sheet is never held in memory. Shared strings, inline strings, booleans,
//! errors and date-formatted numbers are read as the values a person sees.
use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use std::io::{BufRead, BufReader};

use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::detect::header;
use crate::model::{DocAcc, Document};
use crate::resume::Resume;
use crate::slicer::{self, Snap, Step, Steps, with_marker};
use crate::src::Src;
use crate::table::TableWriter;
use crate::util::{Zip, resolve, skip_bytes};
use crate::xml::{attr, is, parse};

const MAX_COLS: usize = 16_384;
/// The shared strings kept in memory take at most about this many bytes; later ones are left out.
// vertexia: an in-memory table; a seekable index of the strings lifts the bound.
const SHARED_BYTES: usize = 192 * 1024 * 1024;
const STREAM_BUFFER: usize = 64 * 1024;
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn local(e: &BytesStart<'_>) -> String {
    e.local_name().into_inner().to_string()
}

fn attribute(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().into_inner() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

fn xml_err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what} is malformed: {e}")
}

/// Shared strings in index order; rich-text runs are joined, phonetic hints
/// dropped. Streamed, and kept up to [`SHARED_BYTES`]; the second value is how
/// many strings did not fit.
fn shared_strings(zip: &mut Zip) -> Result<(Vec<String>, usize), String> {
    let Some(entry) = zip.stream("xl/sharedStrings.xml")? else {
        return Ok((Vec::new(), 0));
    };
    let mut reader = Reader::from_reader(BufReader::with_capacity(STREAM_BUFFER, entry));
    let (mut kept, mut dropped, mut used) = (Vec::new(), 0usize, 0usize);
    let mut cur = String::new();
    let mut push = |s: String, out: &mut Vec<String>| {
        used += s.len() + std::mem::size_of::<String>();
        if used <= SHARED_BYTES {
            out.push(s);
        } else {
            dropped += 1;
        }
    };
    let out = &mut kept;
    let (mut in_t, mut in_phonetic) = (false, false);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_err("xl/sharedStrings.xml", e))?
        {
            Event::Start(e) => match local(&e).as_str() {
                "t" if !in_phonetic => in_t = true,
                "rPh" => in_phonetic = true,
                _ => {}
            },
            Event::End(e) => match e.local_name().into_inner() {
                "t" => in_t = false,
                "rPh" => in_phonetic = false,
                "si" => push(std::mem::take(&mut cur), out),
                _ => {}
            },
            Event::Empty(e) if local(&e) == "si" => push(String::new(), out),
            Event::Text(t) if in_t => cur.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_t => {
                if let Some(c) = r.resolve_char_ref().ok().flatten() {
                    cur.push(c);
                } else {
                    cur.push_str(match r.xml10_content().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((kept, dropped))
}

/// Whether each cell style (by index) formats its number as a date or time.
fn date_styles(zip: &mut Zip) -> Result<Vec<bool>, String> {
    let Some(src) = zip.read_text("xl/styles.xml")? else {
        return Ok(Vec::new());
    };
    let doc = parse(&src)?;
    let custom: HashMap<u32, String> = doc
        .descendants()
        .filter(|n| is(*n, "numFmt"))
        .filter_map(|n| {
            Some((
                attr(n, "numFmtId")?.parse().ok()?,
                attr(n, "formatCode")?.to_string(),
            ))
        })
        .collect();
    let is_date = |id: u32| match custom.get(&id) {
        Some(code) => {
            let mut plain = String::new();
            let (mut quoted, mut bracket) = (false, false);
            for c in code.chars() {
                match c {
                    '"' => quoted = !quoted,
                    '[' if !quoted => bracket = true,
                    ']' if !quoted => bracket = false,
                    c if !quoted && !bracket => plain.push(c.to_ascii_lowercase()),
                    _ => {}
                }
            }
            plain.contains(['y', 'd', 'h', 's']) || plain.contains('m') && !plain.contains('0')
        }
        None => matches!(id, 14..=22 | 27..=36 | 45..=47 | 50..=58),
    };
    let xfs = doc.descendants().find(|n| is(*n, "cellXfs"));
    Ok(xfs
        .map(|x| {
            x.children()
                .filter(|c| is(*c, "xf"))
                .map(|c| {
                    is_date(
                        attr(c, "numFmtId")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Civil date from days since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Formats an Excel serial date; serials below 1 are times of day only.
pub fn serial_to_string(serial: f64) -> String {
    if !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return serial.to_string();
    }
    let mut days = serial.floor() as i64;
    let secs = ((serial - serial.floor()) * 86_400.0).round() as i64;
    if secs >= 86_400 {
        days += 1;
    }
    let secs = secs % 86_400;
    let time = format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    );
    if serial < 1.0 {
        return time;
    }
    // Excel counts 1900 as a leap year, so serials from 61 on are one day ahead.
    let unix_days = days - if days >= 61 { 25_569 } else { 25_568 };
    let (y, m, d) = civil(unix_days);
    if secs == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {time}")
    }
}

fn col_index(cell_ref: &str) -> Option<usize> {
    let letters: String = cell_ref
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    if letters.is_empty() {
        return None;
    }
    let n = letters.to_ascii_uppercase().bytes().fold(0usize, |a, b| {
        a.saturating_mul(26)
            .saturating_add(usize::from(b - b'A' + 1))
    });
    Some(n - 1)
}

/// What a call carries to the next inside one sheet.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct SheetSt {
    col0: usize,
    cols: usize,
    /// The header and rule lines of the table, for a slice that starts below them.
    hdr: String,
}

struct SheetOut {
    md: String,
    drawings: bool,
    /// The sheet has more rows: where they start in the part, and what to carry.
    more: Option<(u64, SheetSt)>,
    /// The first row does not fit beside what this call already holds.
    deferred: bool,
    skipped_rows: usize,
}

/// Shared strings and date styles, which every cell lookup needs.
struct Lookup<'a> {
    shared: &'a [String],
    dates: &'a [bool],
}

/// Reads rows from `rd`, which is positioned `start` bytes into the part (at
/// the first byte when `at` is `None`), until the sheet ends or the table
/// has used `budget` bytes. With `can_defer`, a first row that does not fit
/// leaves the whole sheet for the next call.
fn read_sheet<R: BufRead>(
    rd: R,
    lookup: &Lookup<'_>,
    budget: usize,
    at: Option<(u64, &SheetSt)>,
    can_defer: bool,
) -> Result<SheetOut, String> {
    let (start, mut state) = at.map_or((0, SheetSt::default()), |(p, s)| (p, s.clone()));
    let mut reader = Reader::from_reader(rd);
    // A slice starts in the middle of the part, so its closing tags have no opening ones.
    reader.config_mut().check_end_names = false;
    reader.config_mut().allow_unmatched_ends = true;
    let mut writer: Option<TableWriter> = None;
    let (mut row, mut row_started): (Vec<String>, bool) = (Vec::new(), false);
    let (mut col, mut kind, mut style, mut value, mut in_v, mut in_t, mut in_inline) = (
        0usize,
        String::new(),
        0usize,
        String::new(),
        false,
        false,
        false,
    );
    let (mut drawings, mut deferred, mut skipped_rows) = (false, false, 0usize);
    let mut more = None;
    let mut row_at = start;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let before = start + reader.buffer_position();
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_err("the worksheet", e))?
        {
            Event::Start(e) | Event::Empty(e) if local(&e) == "dimension" => {
                if let Some(r) = attribute(&e, "ref") {
                    let (a, b) = r.split_once(':').unwrap_or((&r, &r));
                    if let (Some(x0), Some(x1)) = (col_index(a), col_index(b)) {
                        state.col0 = x0;
                        state.cols = (x1 + 1 - x0).min(MAX_COLS);
                    }
                }
            }
            Event::Start(e) => match local(&e).as_str() {
                "row" => {
                    row.clear();
                    row_started = true;
                    row_at = before;
                }
                "c" => {
                    kind = attribute(&e, "t").unwrap_or_default();
                    style = attribute(&e, "s").and_then(|s| s.parse().ok()).unwrap_or(0);
                    col = attribute(&e, "r")
                        .and_then(|r| col_index(&r))
                        .unwrap_or(state.col0 + row.len());
                    value.clear();
                }
                "v" => in_v = true,
                "is" => in_inline = true,
                "t" if in_inline => in_t = true,
                "drawing" | "legacyDrawing" => drawings = true,
                _ => {}
            },
            Event::Empty(e) if matches!(local(&e).as_str(), "drawing" | "legacyDrawing") => {
                drawings = true
            }
            Event::Text(t) if in_v || in_t => value.push_str(&t.xml10_content()),
            Event::End(e) => match e.local_name().into_inner() {
                "v" => in_v = false,
                "t" => in_t = false,
                "is" => in_inline = false,
                "c" if row_started => {
                    let text = match kind.as_str() {
                        "s" => value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| lookup.shared.get(i))
                            .cloned()
                            .unwrap_or_default(),
                        "b" => if value.trim() == "1" { "TRUE" } else { "FALSE" }.to_string(),
                        "str" | "inlineStr" | "e" | "d" => value.clone(),
                        _ if lookup.dates.get(style).copied().unwrap_or(false) => value
                            .trim()
                            .parse::<f64>()
                            .map(serial_to_string)
                            .unwrap_or_else(|_| value.clone()),
                        _ => value.trim().to_string(),
                    };
                    let idx = col.saturating_sub(state.col0);
                    if idx < MAX_COLS {
                        if row.len() <= idx {
                            row.resize(idx + 1, String::new());
                        }
                        row[idx] = text;
                    }
                }
                "row" => {
                    row_started = false;
                    if !row.iter().any(|c| !c.trim().is_empty()) {
                        continue;
                    }
                    let w = writer.get_or_insert_with(|| {
                        if state.cols == 0 {
                            state.cols = row.len();
                        }
                        TableWriter::new(state.cols, budget, start == 0)
                    });
                    if w.rows == 0 && start == 0 {
                        state.hdr = TableWriter::header_of(state.cols, &row);
                    }
                    if !w.row(&row) {
                        if w.rows == 0 && can_defer {
                            deferred = true;
                        } else if w.rows == 0 {
                            skipped_rows += 1;
                            w.reset_full();
                            continue;
                        } else {
                            more = Some((row_at, state.clone()));
                        }
                        break;
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let md = writer.map_or_else(String::new, TableWriter::finish);
    Ok(SheetOut {
        md,
        drawings,
        more,
        deferred,
        skipped_rows,
    })
}

struct Sheet {
    name: String,
    path: String,
    hidden: bool,
}

/// The worksheets of a workbook, one unit each, read row by row.
struct Book<'a> {
    ctx: &'a mut Ctx,
    acc: &'a mut DocAcc,
    zip: Zip,
    sheets: Vec<Sheet>,
    shared: Vec<String>,
    dates: Vec<bool>,
    /// The sheet to read next, and where in it.
    unit: u32,
    pos: u64,
    st: Option<SheetSt>,
}

impl Steps for Book<'_> {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.ctx, &mut *self.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: self.unit,
            pos: self.pos,
            st: self.st.as_ref().and_then(|s| serde_json::to_value(s).ok()),
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        loop {
            let Some(sheet) = self.sheets.get((self.unit - 1) as usize) else {
                return Ok(None);
            };
            let n = self.unit;
            if !self.ctx.opts.selected(n) {
                self.unit += 1;
                continue;
            }
            if sheet.hidden {
                self.acc.warn(format!(
                    "sheet {n} ({}) is hidden and was skipped",
                    sheet.name
                ));
                self.unit += 1;
                continue;
            }
            let Some(mut entry) = self.zip.stream(&sheet.path)? else {
                self.acc.warn(format!(
                    "sheet {n} ({}): {} is missing from the archive",
                    sheet.name, sheet.path
                ));
                self.unit += 1;
                continue;
            };
            let fresh = self.pos == 0;
            if !fresh {
                skip_bytes(&mut entry, self.pos)?;
            }
            let budget = self.ctx.table_budget();
            let lookup = Lookup {
                shared: &self.shared,
                dates: &self.dates,
            };
            let at = (!fresh).then(|| (self.pos, self.st.clone().unwrap_or_default()));
            let out = read_sheet(
                BufReader::with_capacity(STREAM_BUFFER, entry),
                &lookup,
                budget,
                at.as_ref().map(|(p, s)| (*p, s)),
                self.ctx.emitted > 0,
            )?;
            let name = sheet.name.clone();
            if out.deferred {
                return Ok(Some(Step::Wait));
            }
            if out.skipped_rows > 0 {
                self.acc.warn(format!(
                    "sheet {n} ({name}): {} row(s) larger than the output limit were skipped",
                    out.skipped_rows
                ));
            }
            if out.drawings && out.more.is_none() {
                self.acc.warn(format!(
                    "sheet {n} ({name}) has charts or images that are not read"
                ));
            }
            let marker = format!("<!-- sheet {n}: {} -->", name.replace('>', "&gt;"));
            let body = match (fresh, out.md.is_empty()) {
                (true, true) => with_marker(&marker, "(empty sheet)"),
                (true, false) => with_marker(&marker, &out.md),
                (false, _) => out.md,
            };
            return Ok(Some(match out.more {
                Some((pos, st)) => {
                    let snap = Snap {
                        unit: n,
                        pos,
                        st: serde_json::to_value(&st).ok(),
                    };
                    Step::Stop(body, snap, 1)
                }
                None => {
                    self.unit += 1;
                    self.pos = 0;
                    self.st = None;
                    Step::Chunk(body)
                }
            }));
        }
    }
}

/// The worksheets in workbook order.
fn sheet_list(zip: &mut Zip) -> Result<Vec<Sheet>, String> {
    let wb_src = zip
        .read_text("xl/workbook.xml")?
        .ok_or("the XLSX has no xl/workbook.xml")?;
    let wb = parse(&wb_src)?;
    let mut rels = HashMap::new();
    if let Some(src) = zip.read_text("xl/_rels/workbook.xml.rels")? {
        let doc = parse(&src)?;
        for r in doc.descendants().filter(|n| is(*n, "Relationship")) {
            if let (Some(id), Some(t)) = (attr(r, "Id"), attr(r, "Target")) {
                rels.insert(id.to_string(), resolve("xl/workbook.xml", t));
            }
        }
    }
    let sheets: Vec<Sheet> = wb
        .descendants()
        .filter(|n| is(*n, "sheet"))
        .filter_map(|n| {
            Some(Sheet {
                name: attr(n, "name")?.to_string(),
                path: rels.get(n.attribute((R_NS, "id"))?)?.clone(),
                hidden: matches!(attr(n, "state"), Some("hidden" | "veryHidden")),
            })
        })
        .collect();
    if sheets.is_empty() {
        return Err("the XLSX has no worksheets".into());
    }
    Ok(sheets)
}

/// The number of worksheets, without reading them.
pub fn sheet_count(src: &Src) -> Result<u32, String> {
    Ok(sheet_list(&mut Zip::open(src.reopen()?)?)?.len() as u32)
}

pub fn convert_xlsx(
    ctx: &mut Ctx,
    source: &str,
    src: &Src,
    resume: Option<&Resume>,
) -> Result<Document, String> {
    let mut zip = Zip::open(src.reopen()?)?;
    let sheets = sheet_list(&mut zip)?;
    let (shared, dropped) = shared_strings(&mut zip)?;
    let dates = date_styles(&mut zip)?;
    let total = sheets.len() as u32;
    let mut acc = DocAcc::default();
    let parts_from = ctx.parts.len();
    match resume {
        Some(r) => {
            acc.next_fig = r.fig;
            acc.restore_seen(&r.seen);
        }
        None => {
            ctx.append(&mut acc, &header(source, "xlsx"));
            if ctx.opts.selects_none(total) {
                acc.warn(format!(
                    "pages selects nothing: the workbook has {total} sheet(s)"
                ));
            }
        }
    }
    if dropped > 0 {
        acc.warn(format!(
            "{dropped} shared string(s) are over the {} MiB the reader keeps; cells that use them are empty",
            SHARED_BYTES / 1024 / 1024
        ));
    }
    let st: Option<SheetSt> = resume
        .and_then(|r| r.st.clone())
        .and_then(|v| serde_json::from_value(v).ok());
    let table_header = st.as_ref().map(|s| s.hdr.clone()).filter(|h| !h.is_empty());
    let next = {
        let mut book = Book {
            ctx: &mut *ctx,
            acc: &mut acc,
            zip,
            sheets,
            shared,
            dates,
            unit: resume.map_or(1, |r| r.unit.max(1)),
            pos: resume.map_or(0, |r| r.pos),
            st,
        };
        slicer::run(&mut book, resume.map_or(0, |r| r.skip))?
    };
    let mut doc = ctx.document(acc, source, "xlsx", total, resume, next, parts_from);
    doc.table_header = table_header;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_dates_follow_excel() {
        assert_eq!(serial_to_string(1.0), "1900-01-01");
        assert_eq!(serial_to_string(59.0), "1900-02-28");
        assert_eq!(serial_to_string(61.0), "1900-03-01");
        assert_eq!(serial_to_string(45292.0), "2024-01-01");
        assert_eq!(serial_to_string(45292.75), "2024-01-01 18:00:00");
        assert_eq!(serial_to_string(0.5), "12:00:00");
    }

    #[test]
    fn column_letters_map_to_indexes() {
        assert_eq!(col_index("A1"), Some(0));
        assert_eq!(col_index("AA10"), Some(26));
        assert_eq!(col_index("12"), None);
    }

    #[test]
    fn sheet_rows_become_a_table_with_shared_strings_and_dates() {
        let xml = br#"<worksheet><dimension ref="A1:C3"/><sheetData>
            <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c></row>
            <row r="2"><c r="A2" t="inlineStr"><is><t>x</t></is></c><c r="B2"><v>3.5</v></c><c r="C2" s="1"><v>45292</v></c></row>
            <row r="3"><c r="A3" t="b"><v>1</v></c><c r="C3" t="e"><v>#DIV/0!</v></c></row></sheetData></worksheet>"#;
        let shared = vec!["Name".to_string(), "Qty".to_string(), "When".to_string()];
        let lookup = Lookup {
            shared: &shared,
            dates: &[false, true],
        };
        let out = read_sheet(&xml[..], &lookup, 100_000, None, false).unwrap();
        assert_eq!(
            out.md,
            "| Name | Qty | When |\n| --- | --- | --- |\n| x | 3.5 | 2024-01-01 |\n| TRUE |  | #DIV/0! |"
        );
        assert!(out.more.is_none() && !out.drawings);
    }
}
