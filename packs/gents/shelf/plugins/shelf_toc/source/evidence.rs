#[path = "evidence_visual.rs"]
mod visual;
use crate::source::*;
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
fn integer(v: &Value, key: &str) -> Result<Option<u32>> {
    if v[key].is_null() {
        return Ok(None);
    }
    let n = if let Some(s) = v[key].as_str() {
        s.parse::<u64>().map_err(|_| format!("invalid {key}"))?
    } else {
        v[key].as_u64().ok_or(format!("invalid {key}"))?
    };
    Ok(Some(
        u32::try_from(n).map_err(|_| format!("invalid {key}"))?,
    ))
}
fn range(v: &Value, book: &BookInput) -> Result<(u32, u32)> {
    let start = integer(v, "start_page")?.unwrap_or(1);
    let end = integer(v, "end_page")?.unwrap_or(book.pages.len() as u32);
    if start == 0 || start > end || end > book.pages.len() as u32 {
        return Err("range outside this book; use physical scan pages starting at 1".into());
    }
    Ok((start, end))
}
pub(crate) fn call(v: &Value) -> Result<Value> {
    let root = Path::new(field(v, "path")?);
    let book = BookInput::load(root, v)?;
    let out = match field(v, "command")? {
        "frontmatter" => frontmatter(&book),
        "search" => search(v, &book)?,
        "headings" => headings(v, &book)?,
        "ocr" => {
            let page = book.page(integer(v, "page_num")?.ok_or("page_num is required")?)?;
            json!({"scan_page":page.scan_page,"source":page.source,"source_page":page.page,"markdown":page.markdown})
        }
        "image" => return visual::inspect(root, v, &book),
        _ => return Err("unknown evidence operation".into()),
    };
    if out.to_string().len() > 1_000_000 {
        return Err("evidence exceeds response budget; call again with a narrower start_page/end_page range".into());
    }
    Ok(out)
}
fn clusters(pages: &[u32], min: usize, gap: u32) -> Vec<Value> {
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for &p in pages {
        if let Some(last) = groups.last_mut().filter(|g| p - g.last().unwrap() <= gap) {
            last.push(p);
        } else {
            groups.push(vec![p]);
        }
    }
    groups
        .into_iter()
        .filter(|g| g.len() >= min)
        .map(|g| json!({"start_page":g[0],"end_page":g.last(),"page_count":g.len()}))
        .collect()
}
fn frontmatter(book: &BookInput) -> Value {
    let categories = [
        (
            "toc",
            r"\b(?:Table of Contents|Contents|Order of Battle|List of Chapters|Chapter Overview|Synopsis)\b",
        ),
        (
            "front_matter",
            r"\b(?:Preface|Author's Note|Foreword|Introduction|Prologue|Acknowledgments|Acknowledgements|Dedication|Dedicated to|About the Author)\b",
        ),
        (
            "structure",
            r"\b(?:Chapter|Part|Section)\s+\d+|^(?:Chapter|Part)\s+[IVX]+",
        ),
        (
            "back_matter",
            r"\b(?:Appendix|Bibliography|Works Cited|References|Index|Epilogue|Afterword|Endnotes|Notes)\b",
        ),
    ];
    let mut categorized: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    let mut details = BTreeMap::new();
    for p in book.pages.iter().take(50) {
        let mut hits = BTreeMap::new();
        for (category, pattern) in categories {
            let re = RegexBuilder::new(pattern)
                .case_insensitive(true)
                .multi_line(true)
                .build()
                .unwrap();
            let words: BTreeSet<_> = re
                .find_iter(&p.markdown)
                .map(|m| m.as_str().to_owned())
                .collect();
            if !words.is_empty() {
                categorized.entry(category).or_default().push(p.scan_page);
                hits.insert(category, words);
            }
        }
        if !hits.is_empty() {
            details.insert(p.scan_page.to_string(), hits);
        }
    }
    let structure = categorized.get("structure").cloned().unwrap_or_default();
    json!({"report":{"search_range":format!("1-{}",book.pages.len().min(50)),"total_pages_searched":book.pages.len().min(50),"pages_with_data":book.pages.len().min(50),"categorized_pages":categorized,"page_details":details,"failed_pages":[]},"structure_clusters":clusters(&structure,3,2),"message":"Inspect ToC keyword pages first, then dense Chapter/Part clusters. Keyword hits are candidates; visually verify the contents and its continuation pages."})
}
fn search(v: &Value, book: &BookInput) -> Result<Value> {
    let query = field(v, "query")?;
    if query.len() > 4096 {
        return Err("query too long".into());
    }
    let re = RegexBuilder::new(query)
        .case_insensitive(true)
        .size_limit(1_000_000)
        .build()
        .or_else(|_| {
            RegexBuilder::new(&regex::escape(query))
                .case_insensitive(true)
                .build()
        })
        .map_err(|e| e.to_string())?;
    let (lo, hi) = range(v, book)?;
    let mut matches = Vec::new();
    for p in book
        .pages
        .iter()
        .filter(|p| p.scan_page >= lo && p.scan_page <= hi)
    {
        let indices: Vec<_> = re.find_iter(&p.markdown).collect();
        if indices.is_empty() {
            continue;
        }
        let snippets: Vec<_> = indices
            .iter()
            .take(3)
            .map(|m| {
                let mut a = m.start().saturating_sub(50);
                let mut b = (m.end() + 50).min(p.markdown.len());
                while !p.markdown.is_char_boundary(a) {
                    a += 1;
                }
                while !p.markdown.is_char_boundary(b) {
                    b -= 1;
                }
                format!("...{}...", p.markdown[a..b].trim().replace('\n', " "))
            })
            .collect();
        matches.push(json!({"scan_page":p.scan_page,"match_count":indices.len(),"context_snippets":snippets,"in_back_matter":p.scan_page>=book.pages.len() as u32*4/5}));
    }
    let pages: Vec<_> = matches
        .iter()
        .map(|m| m["scan_page"].as_u64().unwrap() as u32)
        .collect();
    Ok(
        json!({"query":query,"matches":matches,"clusters":clusters(&pages,2,3),"message":"Running-header clusters suggest openings at their first page. The last 20% is a back-matter hint, not a classification. Check candidate and adjacent pages before writing a finding."}),
    )
}
fn headings(v: &Value, book: &BookInput) -> Result<Value> {
    let (lo, hi) = range(v, book)?;
    let toc_start = integer(v, "toc_start")?.unwrap_or(0);
    let toc_end = integer(v, "toc_end")?.unwrap_or(0);
    let mut pages = BTreeSet::new();
    let results: Vec<_> = book
        .headings()
        .into_iter()
        .filter(|h| {
            let page = h["scan_page"].as_u64().unwrap() as u32;
            page >= lo
                && page <= hi
                && !(toc_start > 0 && page >= toc_start && page <= toc_end)
                && pages.insert(page)
        })
        .collect();
    Ok(
        json!({"headings":results,"message":"These OCR headings are candidates, not verified chapter starts."}),
    )
}
#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
