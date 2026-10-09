use super::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
pub(super) const CSS: &str = r#"
html { color-scheme: light dark; }
body { font-family: Georgia, serif; line-height: 1.5; margin: 5%; }
h1,h2,h3,h4,h5,h6 { line-height: 1.2; break-after: avoid; font-weight: normal; }
h1 { font-size: 1.8em; margin: 2em 0 1.5em; text-align: center; }
h2 { font-size: 1.4em; margin-top: 1.8em; }
p { margin: .65em 0; orphans: 2; widows: 2; }
blockquote { margin: 1em 1.5em; font-size: .95em; }
nav ol { list-style: none; padding-left: 1.2em; }
nav > ol { padding-left: 0; }
nav li { margin: .6em 0; }
nav a { text-decoration: none; }
table { border-collapse: collapse; width: 100%; margin: 1em 0; font-size: .9em; }
th,td { padding: .35em .55em; text-align: left; vertical-align: top; border-bottom: 1px solid #aaa; }
th { font-weight: bold; }
pre { white-space: pre-wrap; }
.back_matter { font-size: .92em; }
details { margin-top: 2em; }

"#;
pub(super) fn valid_id(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
pub(super) fn page_id(source: &str, page: u64) -> String {
    format!("scan-{:x}-{page}", Sha256::digest(source.as_bytes()))
}
pub(super) fn markdown(md: &str) -> Result<String, String> {
    xml(md)?;
    let parser = Parser::new_ext(
        md,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES,
    );
    let events: Vec<_> = parser
        .filter_map(|event| match event {
            Event::Html(text) | Event::InlineHtml(text) => {
                if matches!(
                    text.trim().to_ascii_lowercase().as_str(),
                    "<br>" | "<br/>" | "<br />"
                ) {
                    Some(Event::HardBreak)
                } else {
                    Some(Event::Text(text))
                }
            }
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let safe = dest_url.starts_with('#')
                    || dest_url.starts_with("https://")
                    || dest_url.starts_with("http://")
                    || dest_url.starts_with("mailto:");
                Some(Event::Start(Tag::Link {
                    link_type,
                    dest_url: if safe { dest_url } else { "#".into() },
                    title,
                    id,
                }))
            }
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                id,
                classes,
                attrs,
            }) => Some(Event::Start(Tag::Heading {
                level: HeadingLevel::H2,
                id,
                classes,
                attrs,
            })),
            Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                Some(Event::End(TagEnd::Heading(HeadingLevel::H2)))
            }
            x => Some(x),
        })
        .collect();
    let visible = events.iter().any(|event| match event {
        Event::Text(text) | Event::Code(text) | Event::FootnoteReference(text) => {
            !text.trim().is_empty()
        }
        Event::Rule | Event::HardBreak | Event::Start(Tag::Table(_)) => true,
        _ => false,
    });
    if !md.trim().is_empty() && !visible {
        return Ok(format!("<p>{}</p>\n", xml(md)?));
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    Ok(out)
}
pub(super) fn navigation(book: &Manuscript, names: &[String]) -> Result<String, String> {
    fn level(
        book: &Manuscript,
        names: &[String],
        index: &mut usize,
        depth: u64,
    ) -> Result<String, String> {
        let mut out = String::from("<ol>");
        while *index < book.chapters.len() && book.chapters[*index].level >= depth {
            let i = *index;
            let ch = &book.chapters[i];
            out.push_str(&format!(
                r#"<li><a href="{}">{}</a>"#,
                names[i],
                xml(&ch.title)?
            ));
            *index += 1;
            if *index < book.chapters.len() && book.chapters[*index].level > ch.level {
                out.push_str(&level(book, names, index, book.chapters[*index].level)?);
            }
            out.push_str("</li>");
            if *index < book.chapters.len() && book.chapters[*index].level < depth {
                break;
            }
        }
        out.push_str("</ol>");
        Ok(out)
    }
    level(book, names, &mut 0, 1)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_markers_without_list_or_heading_text_remain_visible() {
        for source in ["37.", "8)", "-", "+", "*", "#", "##"] {
            assert_eq!(markdown(source).unwrap(), format!("<p>{source}</p>\n"));
        }
        let list = markdown("37. A reference.").unwrap();
        assert!(list.contains("<ol start=\"37\">"));
        assert!(list.contains("A reference."));
        assert!(markdown("---").unwrap().contains("<hr />"));
    }

    #[test]
    fn keeps_tables_emphasis_and_quotes_as_xhtml() {
        let text=markdown("| Heading | Value |\n| --- | --- |\n| **Bold** | *Italic* |\n\n> A quotation.\n\n<script>bad</script>").unwrap();
        assert!(text.contains("<table>"));
        assert!(text.contains("<strong>Bold</strong>"));
        assert!(text.contains("<em>Italic</em>"));
        assert!(text.contains("<blockquote>"));
        assert!(!text.contains("<script>"));
        roxmltree::Document::parse(&format!("<div>{text}</div>")).unwrap();
    }
    #[test]
    fn renders_ocr_line_breaks_without_allowing_html_attributes() {
        let text = markdown("First<br>Second<br />Third<br onclick=\"bad()\">Fourth").unwrap();
        assert_eq!(text.matches("<br />").count(), 2);
        assert!(text.contains("&lt;br onclick="));
        roxmltree::Document::parse(&format!("<div>{text}</div>")).unwrap();
    }
}
