use super::*;
use regex::Regex;
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn plain(s: &str) -> &str {
    s.trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim()
}
fn edge_label(s: &str) -> String {
    let mut text = plain(s);
    if let Some((first, rest)) = text.split_once(char::is_whitespace) {
        if first.chars().all(|c| c.is_ascii_digit()) {
            text = rest.trim_start();
        }
    }
    if let Some((rest, last)) = text.rsplit_once(char::is_whitespace) {
        if last.chars().all(|c| c.is_ascii_digit()) {
            text = rest.trim_end();
        }
    }
    norm(text)
}
fn prose(s: &str) -> bool {
    let s = s.trim();
    !s.starts_with(['#', '|', '>', '!', '-', '*'])
        && !s.chars().next().is_some_and(|c| c.is_numeric())
}
fn audit(edits: &mut Vec<Value>, rule: &str, old: &str, new: &str, sources: &[Span]) {
    edits.push(json!({"rule":rule,"old_text":old,"new_text":new,"sources":sources}));
}
pub(super) fn vocabulary(book: &Value) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    if let Some(chs) = book["chapters"].as_array() {
        for ch in chs {
            if let Some(pages) = ch["pages"].as_array() {
                for p in pages {
                    for word in p["markdown"]
                        .as_str()
                        .unwrap_or("")
                        .split(|c: char| !c.is_alphabetic())
                    {
                        if word.len() > 2 {
                            words.insert(word.to_lowercase());
                        }
                    }
                }
            }
        }
    }
    words
}
/// Shelf's page-join rule is restricted to prose. Hyphens are removed only if the
/// combined word also occurs unbroken in this source; ambiguous compounds remain.
fn join(a: &str, b: &str, words: &BTreeSet<String>) -> String {
    let a = a.trim_end();
    let b = b.trim_start();
    if let Some(stem) = a.strip_suffix('-') {
        let left = stem
            .rsplit(|c: char| !c.is_alphabetic())
            .next()
            .unwrap_or("");
        let right = b.split(|c: char| !c.is_alphabetic()).next().unwrap_or("");
        if !left.is_empty()
            && !right.is_empty()
            && words.contains(&format!("{left}{right}").to_lowercase())
        {
            return format!("{stem}{b}");
        }
        return format!("{a}{b}");
    }
    format!("{a} {b}")
}
fn parts(text: &str, source: &str, page: u64) -> Vec<Block> {
    let re = Regex::new(r"\n[ \t]*\n").unwrap();
    let mut start = 0;
    let mut ranges = vec![];
    for m in re.find_iter(text) {
        ranges.push((start, m.start()));
        start = m.end();
    }
    ranges.push((start, text.len()));
    ranges
        .into_iter()
        .filter_map(|(a, z)| {
            let s = &text[a..z];
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            let off = a + s.len() - s.trim_start().len();
            Some(Block {
                id: String::new(),
                markdown: trimmed.into(),
                sources: vec![Span {
                    source: source.into(),
                    page,
                    start_byte: off,
                    end_byte: off + trimmed.len(),
                }],
            })
        })
        .collect()
}
pub(super) fn section(
    book: &str,
    title: &str,
    book_title: &str,
    pages: &[Value],
    words: &BTreeSet<String>,
    edits: &mut Vec<Value>,
) -> Result<Vec<Block>> {
    let numbered = Regex::new(r"(?i)^(?:chapter\s+)?([0-9]+)[.:\s-]+\s*(.+)$").unwrap();
    let captures = numbered.captures(title);
    let number = captures.as_ref().and_then(|c| c.get(1)).map(|m| m.as_str());
    let short = captures
        .as_ref()
        .and_then(|c| c.get(2))
        .map_or(title, |m| m.as_str());
    let short = short.split(" (").next().unwrap_or(short);
    let (main_title, subtitle) = short.split_once(':').unwrap_or((short, ""));
    let headers = [
        norm(short),
        norm(main_title),
        norm(book_title),
        norm(book_title.split(':').next().unwrap_or(book_title)),
    ];
    let mut header_pages: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (pi, page) in pages.iter().enumerate() {
        let raw = page["markdown"].as_str().ok_or("page text required")?;
        let candidates: Vec<_> = parts(raw, "", 0)
            .into_iter()
            .filter(|b| !b.markdown.starts_with("<!-- page "))
            .collect();
        let leading = if pi == 0 { 3 } else { 1 };
        for block in candidates
            .iter()
            .take(leading)
            .chain(candidates.iter().rev().take(2))
        {
            let label = edge_label(&block.markdown);
            if !label.is_empty() && headers.contains(&label) {
                header_pages.entry(label).or_default().insert(pi);
            }
        }
    }
    let mut out: Vec<Block> = vec![];
    for (pi, p) in pages.iter().enumerate() {
        let source = field(p, "source")?;
        let page = p["page"].as_u64().ok_or("page number required")?;
        let mut bs = parts(
            p["markdown"].as_str().ok_or("page text required")?,
            source,
            page,
        );
        bs.retain(|b| !b.markdown.starts_with("<!-- page "));
        // Folios are only recognized at the bottom, never by scanning body paragraphs.
        if bs.last().is_some_and(|b| {
            plain(&b.markdown).chars().all(|c| c.is_ascii_digit()) && plain(&b.markdown).len() < 6
        }) {
            let b = bs.pop().unwrap();
            audit(edits, "footer_folio", &b.markdown, "", &b.sources);
        }
        if pi > 0 {
            for top in [true, false] {
                let idx = if top { 0 } else { bs.len().saturating_sub(1) };
                if let Some(b) = bs.get(idx) {
                    let candidate = edge_label(&b.markdown);
                    if candidate.chars().any(char::is_alphabetic)
                        && header_pages
                            .get(&candidate)
                            .is_some_and(|pages| pages.len() >= 2)
                    {
                        let b = bs.remove(idx);
                        audit(edits, "running_header", &b.markdown, "", &b.sources);
                    }
                }
            }
        }
        // A displaced drop cap appears between the first line of prose and its continuation.
        let mut i = 1;
        while i + 1 < bs.len() {
            let cap = plain(&bs[i].markdown).to_string();
            if cap.len() == 1
                && cap.as_bytes()[0].is_ascii_uppercase()
                && prose(&bs[i - 1].markdown)
                && bs[i - 1].markdown.len() > 25
                && bs[i - 1]
                    .markdown
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase())
                && bs[i + 1]
                    .markdown
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_lowercase())
            {
                let next = bs.remove(i + 1);
                let letter = bs.remove(i);
                let prev = &mut bs[i - 1];
                let old = format!(
                    "{}\n\n{}\n\n{}",
                    prev.markdown, letter.markdown, next.markdown
                );
                prev.markdown = join(&format!("{cap}{}", prev.markdown), &next.markdown, words);
                prev.sources.extend(letter.sources);
                prev.sources.extend(next.sources);
                audit(
                    edits,
                    "displaced_drop_cap",
                    &old,
                    &prev.markdown,
                    &prev.sources,
                );
            } else {
                i += 1;
            }
        }
        if pi == 0 {
            for _ in 0..3 {
                let Some(b) = bs.first() else {
                    break;
                };
                let n = norm(plain(&b.markdown));
                if (!n.is_empty() && [norm(short), norm(main_title), norm(subtitle)].contains(&n))
                    || number.is_some_and(|numb| {
                        n == format!("chapter{numb}") || (b.markdown.starts_with('#') && n == numb)
                    })
                {
                    let b = bs.remove(0);
                    audit(edits, "section_heading", &b.markdown, "", &b.sources);
                } else {
                    break;
                }
            }
        }
        for b in &mut bs {
            if prose(&b.markdown) && b.markdown.contains('\n') {
                let old = b.markdown.clone();
                let mut lines = old.lines();
                let mut text = lines.next().unwrap_or("").to_string();
                for line in lines {
                    text = join(&text, line, words);
                }
                b.markdown = text;
                if old != b.markdown {
                    audit(edits, "prose_line_wrap", &old, &b.markdown, &b.sources);
                }
            }
        }
        if let (Some(prev), Some(next)) = (out.last_mut(), bs.first()) {
            let tail = prev.markdown.trim_end().chars().last().unwrap_or('.');
            if prose(&prev.markdown)
                && prose(&next.markdown)
                && !".!?\"'”’。！？".contains(tail)
                && next
                    .markdown
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_lowercase())
            {
                let next = bs.remove(0);
                let old = format!("{}\n\n{}", prev.markdown, next.markdown);
                prev.markdown = join(&prev.markdown, &next.markdown, words);
                prev.sources.extend(next.sources);
                audit(
                    edits,
                    "page_continuation",
                    &old,
                    &prev.markdown,
                    &prev.sources,
                );
            }
        }
        if bs.is_empty() {
            bs.push(Block {
                id: String::new(),
                markdown: String::new(),
                sources: vec![Span {
                    source: source.into(),
                    page,
                    start_byte: 0,
                    end_byte: p["markdown"].as_str().unwrap().len(),
                }],
            });
        }
        out.extend(bs);
    }
    for b in &mut out {
        b.id = format!(
            "p-{}",
            &hash(format!("{book}:{}", serde_json::to_string(&b.sources).unwrap()).as_bytes())
                [..24]
        );
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joins_drop_cap_and_pages_without_erasing_tables_or_provenance() {
        let pages = vec![
            json!({"source":"book.pdf","page":32,"markdown":"<!-- page 32 -->\n\nCHAPTER 1\n\n### METAMORPHOSIS\n\nN THE FIRST PART OF THE CENTURY, a se-\n\n# I\n\nries of changes began. The series affected\n\n19"}),
            json!({"source":"book.pdf","page":33,"markdown":"Metamorphosis\n\npeople everywhere.\n\n| Year | Count |\n| --- | --- |\n| 1945 | 20 |\n\n20"}),
        ];
        let mut edits = vec![];
        let words = ["series".into()].into_iter().collect();
        let out = section(
            "book",
            "1. Metamorphosis",
            "Book",
            &pages,
            &words,
            &mut edits,
        )
        .unwrap();
        assert_eq!(
            out[0].markdown,
            "IN THE FIRST PART OF THE CENTURY, a series of changes began. The series affected people everywhere."
        );
        assert_eq!(out[0].sources.last().unwrap().page, 33);
        assert!(out[1].markdown.contains("| 1945 | 20 |"));
        for b in &out {
            for s in &b.sources {
                let raw = pages.iter().find(|p| p["page"] == s.page).unwrap()["markdown"]
                    .as_str()
                    .unwrap();
                assert!(!raw[s.start_byte..s.end_byte].is_empty());
            }
        }
        assert_eq!(
            out[0].id,
            section(
                "book",
                "1. Metamorphosis",
                "Book",
                &pages,
                &words,
                &mut vec![]
            )
            .unwrap()[0]
                .id
        );
    }
    #[test]
    fn preserves_unique_edge_headings_and_numbers_attached_to_words() {
        let pages = vec![
            json!({"source":"x","page":1,"markdown":"Opening paragraph.\n\nRoute\n\nMore text."}),
            json!({"source":"x","page":2,"markdown":"Route66\n\nA road name is substantive text.\n\nRésumé"}),
        ];
        let mut edits = vec![];
        let out = section("b", "Route", "Résumé", &pages, &BTreeSet::new(), &mut edits).unwrap();
        assert!(out.iter().any(|b| b.markdown.contains("Route66")));
        assert!(out.iter().any(|b| b.markdown == "Résumé"));
        assert!(!edits.iter().any(|e| e["rule"] == "running_header"));
    }
    #[test]
    fn recognizes_chapter_prefixes_split_subtitles_and_edge_folios() {
        let pages = vec![
            json!({"source":"book.pdf","page":1,"markdown":"CHAPTER 1\n\n## Hot War, Cold War\n\nChina's Conflicts\n\nOpening text."}),
            json!({"source":"book.pdf","page":2,"markdown":"**28 CHINA'S GOOD WAR**\n\nBody text.\n\n## Hot War, Cold War\n\nMore body text.\n\nHot War, Cold War 29"}),
            json!({"source":"book.pdf","page":3,"markdown":"**30 CHINA'S GOOD WAR**\n\nNext page."}),
        ];
        let mut edits = vec![];
        let out = section(
            "b",
            "Chapter 1: Hot War, Cold War: China's Conflicts",
            "China's Good War",
            &pages,
            &BTreeSet::new(),
            &mut edits,
        )
        .unwrap();
        assert_eq!(out[0].markdown, "Opening text.");
        assert_eq!(
            out.iter()
                .filter(|b| b.markdown == "## Hot War, Cold War")
                .count(),
            1
        );
        assert_eq!(
            edits
                .iter()
                .filter(|e| e["rule"] == "running_header")
                .count(),
            3
        );
        assert_eq!(
            edits
                .iter()
                .filter(|e| e["rule"] == "section_heading")
                .count(),
            3
        );
        assert!(out.iter().any(|b| b.markdown == "More body text."));
    }
    #[test]
    fn removes_numbered_chapter_heading_but_keeps_body_numbers() {
        let pages = vec![
            json!({"source":"x","page":1,"markdown":"# 1\n\n## Origins\n\n1\n\nA numbered example."}),
        ];
        let out = section(
            "b",
            "1. Origins",
            "Book",
            &pages,
            &BTreeSet::new(),
            &mut vec![],
        )
        .unwrap();
        assert_eq!(out[0].markdown, "1");
        assert_eq!(out[1].markdown, "A numbered example.");
    }
    #[test]
    fn preserves_legitimate_compounds_and_new_sections() {
        let words = BTreeSet::new();
        assert_eq!(join("well-", "known", &words), "well-known");
        let pages = vec![
            json!({"source":"x","page":1,"markdown":"A paragraph ends.\n\n1"}),
            json!({"source":"x","page":2,"markdown":"## A heading\n\nText."}),
        ];
        let out = section("b", "Title", "B", &pages, &words, &mut vec![]).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[1].markdown.starts_with("##"));
    }
}
