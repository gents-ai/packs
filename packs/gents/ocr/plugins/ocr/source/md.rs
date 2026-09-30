//! The one Markdown writer: every converter produces [`Block`]s and this module
//! turns them into text, so headings, lists, tables and figures look the same
//! whatever the source format.
use crate::model::{Block, DocAcc, Figure};

/// Escapes characters that would be read as Markdown syntax inside running text.
pub fn esc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1).copied();
        match c {
            '\\' | '*' | '`' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            '_' if !(prev.is_some_and(char::is_alphanumeric)
                && next.is_some_and(char::is_alphanumeric)) =>
            {
                out.push('\\');
                out.push('_');
            }
            '<' if next.is_some_and(|n| n.is_ascii_alphabetic() || n == '/' || n == '!') => {
                out.push('\\');
                out.push('<');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Escapes a table cell: pipes and line breaks would end the cell.
pub fn cell(text: &str) -> String {
    esc(text.trim())
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

/// Escapes a line start that would otherwise open a heading, list or quote.
fn guard_line_start(line: &str) -> String {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let numbered =
        digits > 0 && digits < 10 && matches!(t.as_bytes().get(digits), Some(b'.' | b')'));
    if t.starts_with('#')
        || t.starts_with('>')
        || t.starts_with("- ")
        || t.starts_with("+ ")
        || t.starts_with("---")
        || numbered
    {
        format!("\\{line}")
    } else {
        line.to_string()
    }
}

/// Whether a paragraph opens like a figure caption: "Figure 3", "Fig. 2", "Chart 1".
pub fn caption_start(text: &str) -> bool {
    let t = text.trim_start().to_lowercase();
    [
        "figure",
        "fig.",
        "fig ",
        "chart",
        "diagram",
        "image",
        "photo",
        "plate",
        "illustration",
        "graph",
        "abb.",
        "figura",
    ]
    .iter()
    .any(|w| {
        t.strip_prefix(w).is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '\u{a0}', '.']);
            rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
    })
}

/// Removes the backslash escapes `esc` added, for text kept plain in a figure record.
pub fn unescape(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A caption that is only a file name says nothing; figures drop it.
pub fn is_file_name(caption: &str) -> bool {
    let c = caption.trim().to_ascii_lowercase();
    !c.contains(char::is_whitespace)
        && [
            ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".tif", ".tiff", ".webp", ".svg", ".emf",
            ".wmf",
        ]
        .iter()
        .any(|e| c.ends_with(e))
}

/// Gives each caption-less figure the "Figure N ..." paragraph (or caption
/// paragraph) next to it, removing that paragraph from the flow and updating the
/// figure record; figures still without a caption then take their alt text.
pub fn attach_captions(blocks: &mut Vec<Block>, acc: &mut DocAcc) {
    let mut k = 0;
    while k < blocks.len() {
        let empty = matches!(&blocks[k], Block::Figure(f) if f.caption.trim().is_empty());
        if empty {
            let caption_at = |i: usize| match blocks.get(i) {
                Some(Block::Para(t)) if caption_start(t.trim_matches('*')) => {
                    Some(unescape(t.trim_matches('*')))
                }
                Some(Block::Caption(t)) => Some(unescape(t.trim_matches('*'))),
                _ => None,
            };
            let found = caption_at(k + 1)
                .map(|c| (k + 1, c))
                .or_else(|| k.checked_sub(1).and_then(|p| caption_at(p).map(|c| (p, c))));
            if let Some((at, caption)) = found {
                set_caption(&mut blocks[k], &mut acc.figures, caption);
                blocks.remove(at);
                if at < k {
                    k -= 1;
                }
            }
        }
        k += 1;
    }
    for b in blocks.iter_mut() {
        if let Block::Caption(t) = b {
            let text = std::mem::take(t);
            *b = Block::Para(text);
            continue;
        }
        let id = match b {
            Block::Figure(f) if f.caption.trim().is_empty() => f.id.clone(),
            _ => continue,
        };
        if let Some(alt) = acc.alts.remove(&id).filter(|a| !is_file_name(a)) {
            set_caption(b, &mut acc.figures, alt);
        }
    }
}

fn set_caption(block: &mut Block, figures: &mut [Figure], caption: String) {
    if let Block::Figure(f) = block {
        f.caption.clone_from(&caption);
        if let Some(rec) = figures.iter_mut().find(|r| r.id == f.id) {
            rec.caption = caption;
        }
    }
}

pub fn render(blocks: &[Block]) -> String {
    let mut out = String::new();
    // The open list levels as (marker width, ordered): a child is indented by
    // its parents' marker widths so strict Markdown parsers nest it, and items
    // of one list sit on adjacent lines.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for block in blocks {
        let item = match block {
            Block::Item { depth, marker, .. } => Some((usize::from(*depth), marker != "-")),
            _ => None,
        };
        if !out.is_empty() {
            // A bullet list followed by a numbered one is two lists.
            let tight = item.is_some_and(|(depth, ordered)| {
                !open.is_empty() && (depth > 0 || open[0].1 == ordered)
            });
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        if item.is_none() {
            open.clear();
        }
        match block {
            Block::Heading(level, text) => {
                out.push_str(&"#".repeat(usize::from((*level).clamp(1, 6))));
                out.push(' ');
                out.push_str(text.trim());
            }
            Block::Para(text) | Block::Caption(text) => {
                let lines: Vec<String> = text.trim().lines().map(guard_line_start).collect();
                out.push_str(&lines.join("\n"));
            }
            Block::Item {
                depth,
                marker,
                text,
            } => {
                let d = usize::from(*depth);
                open.resize(d, (2, false));
                out.push_str(&" ".repeat(open.iter().map(|o| o.0).sum()));
                open.push((marker.chars().count() + 1, marker != "-"));
                out.push_str(marker);
                out.push(' ');
                out.push_str(text.trim().replace('\n', " ").as_str());
            }
            Block::Table(rows) => table(rows, &mut out),
            Block::Code(code) => {
                let fence = if code.contains("```") { "~~~~" } else { "```" };
                out.push_str(fence);
                out.push('\n');
                out.push_str(code.trim_end_matches('\n'));
                out.push('\n');
                out.push_str(fence);
            }
            Block::Quote(inner) => {
                let body = render(inner);
                let quoted: Vec<String> = body
                    .lines()
                    .map(|l| {
                        if l.is_empty() {
                            ">".to_string()
                        } else {
                            format!("> {l}")
                        }
                    })
                    .collect();
                out.push_str(&quoted.join("\n"));
            }
            Block::Figure(fig) => figure(fig, &mut out),
            Block::Rule => out.push_str("---"),
            Block::Raw(line) => out.push_str(line),
        }
    }
    out
}

fn table(rows: &[Vec<String>], out: &mut String) {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    for (i, row) in rows.iter().enumerate() {
        out.push('|');
        for c in 0..cols {
            out.push(' ');
            out.push_str(&cell(row.get(c).map_or("", String::as_str)));
            out.push_str(" |");
        }
        out.push('\n');
        if i == 0 {
            out.push('|');
            out.push_str(&" --- |".repeat(cols));
            out.push('\n');
        }
    }
    out.pop();
}

/// A figure is one blockquote: id and caption, then the text read inside it.
fn figure(fig: &Figure, out: &mut String) {
    let caption = if fig.caption.trim().is_empty() {
        "image"
    } else {
        fig.caption.trim()
    };
    let size = if fig.width > 0 {
        format!(" ({}x{} px)", fig.width, fig.height)
    } else {
        String::new()
    };
    out.push_str(&format!(
        "> **[Figure {}]** {}{}",
        fig.id,
        caption.replace('\n', " "),
        size
    ));
    if fig.part.is_some() {
        out.push_str(" (image attached)");
    }
    if !fig.text.trim().is_empty() {
        out.push_str("\n>\n> Text in figure:");
        for line in fig.text.lines() {
            out.push_str("\n> ");
            out.push_str(line);
        }
    } else if !fig.note.is_empty() {
        out.push_str("\n>\n> ");
        out.push_str(&fig.note);
    } else if fig.ocr {
        out.push_str("\n>\n> (no text found in the figure)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_syntax_but_keeps_snake_case() {
        assert_eq!(esc("a*b my_var _x_"), "a\\*b my_var \\_x\\_");
    }

    #[test]
    fn table_pads_ragged_rows_and_escapes_pipes() {
        let rows = vec![vec!["a".into(), "b|c".into()], vec!["1".into()]];
        assert_eq!(
            render(&[Block::Table(rows)]),
            "| a | b\\|c |\n| --- | --- |\n| 1 |  |"
        );
    }

    #[test]
    fn list_items_are_tight_and_quotes_prefixed() {
        let items = vec![
            Block::Item {
                depth: 0,
                marker: "-".into(),
                text: "one".into(),
            },
            Block::Item {
                depth: 1,
                marker: "-".into(),
                text: "two".into(),
            },
        ];
        assert_eq!(render(&items), "- one\n  - two");
        assert_eq!(
            render(&[Block::Quote(vec![Block::Para("hi".into())])]),
            "> hi"
        );
    }

    #[test]
    fn nested_items_are_indented_by_the_parent_marker_width() {
        let item = |depth, marker: &str, text: &str| Block::Item {
            depth,
            marker: marker.into(),
            text: text.into(),
        };
        assert_eq!(
            render(&[
                item(0, "1.", "a"),
                item(1, "-", "b"),
                item(2, "-", "c"),
                item(0, "2.", "d"),
            ]),
            "1. a\n   - b\n     - c\n2. d"
        );
    }

    #[test]
    fn alt_text_is_only_a_fallback_caption() {
        let mut acc = DocAcc {
            figures: vec![fig("fig-1", ""), fig("fig-2", "")],
            ..DocAcc::default()
        };
        acc.alts.insert("fig-1".into(), "Bar chart".into());
        acc.alts.insert("fig-2".into(), "Other".into());
        let mut blocks = vec![
            Block::Figure(Box::new(acc.figures[0].clone())),
            Block::Caption("Sales by quarter".into()),
            Block::Figure(Box::new(acc.figures[1].clone())),
        ];
        attach_captions(&mut blocks, &mut acc);
        assert_eq!(acc.figures[0].caption, "Sales by quarter");
        assert_eq!(acc.figures[1].caption, "Other");
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn paragraph_line_starts_are_guarded() {
        assert_eq!(
            render(&[Block::Para("# not a heading".into())]),
            "\\# not a heading"
        );
        assert_eq!(
            render(&[Block::Para("2024. a year".into())]),
            "\\2024. a year"
        );
    }

    fn fig(id: &str, caption: &str) -> Figure {
        Figure {
            id: id.into(),
            caption: caption.into(),
            ..Figure::default()
        }
    }

    #[test]
    fn caption_paragraphs_attach_to_captionless_figures() {
        let mut acc = DocAcc {
            figures: vec![
                fig("fig-1", ""),
                fig("fig-2", "Own caption"),
                fig("fig-3", ""),
            ],
            ..DocAcc::default()
        };
        let figures = acc.figures.clone();
        let mut blocks = vec![
            Block::Para("Intro.".into()),
            Block::Figure(Box::new(figures[0].clone())),
            Block::Para("Figure 1. Latency by region".into()),
            Block::Figure(Box::new(figures[1].clone())),
            Block::Para("Figure 9. Not used".into()),
            Block::Para("*Fig. 3: Throughput*".into()),
            Block::Figure(Box::new(figures[2].clone())),
        ];
        attach_captions(&mut blocks, &mut acc);
        let figures = acc.figures;
        assert_eq!(figures[0].caption, "Figure 1. Latency by region");
        assert_eq!(figures[1].caption, "Own caption");
        assert_eq!(figures[2].caption, "Fig. 3: Throughput");
        assert_eq!(blocks.len(), 5, "{blocks:?}");
        assert!(matches!(&blocks[3], Block::Para(t) if t.contains("Figure 9")));
    }

    #[test]
    fn file_name_captions_are_recognised() {
        assert!(is_file_name("chart.PNG") && is_file_name(" photo.jpeg "));
        assert!(!is_file_name("a chart.png of sales") && !is_file_name("Revenue"));
    }

    #[test]
    fn figure_block_lists_caption_and_text() {
        let fig = Figure {
            id: "fig-1".into(),
            caption: "Revenue".into(),
            text: "Q1\nQ2".into(),
            width: 10,
            height: 20,
            ocr: true,
            ..Figure::default()
        };
        assert_eq!(
            render(&[Block::Figure(Box::new(fig))]),
            "> **[Figure fig-1]** Revenue (10x20 px)\n>\n> Text in figure:\n> Q1\n> Q2"
        );
    }
}
