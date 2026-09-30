//! Converts a parsed HTML or XHTML tree into Markdown blocks: headings, lists,
//! tables, code, quotes, links and figures, with images resolved through the
//! caller so the same walker serves HTML files, EPUB chapters and more.
use base64::Engine as _;

use crate::ctx::Ctx;
use crate::html::{El, Node};
use crate::md::esc;
use crate::model::{Block, DocAcc};

/// Supplies the bytes of an image an element refers to.
pub trait Resolver {
    fn image(&mut self, src: &str) -> Option<Vec<u8>>;
}

const SKIP: [&str; 13] = [
    "script", "style", "head", "template", "noscript", "iframe", "object", "video", "audio",
    "canvas", "map", "select", "button",
];
const SPECIAL: [&str; 21] = [
    "img",
    "figure",
    "table",
    "ul",
    "ol",
    "pre",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "div",
    "hr",
    "dl",
    "svg",
    "section",
    "article",
    "li",
];
const INLINE_BLOCKISH: [&str; 14] = [
    "p",
    "div",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "dd",
    "dt",
    "section",
    "article",
    "blockquote",
    "table",
];
/// A data: image is decoded only up to this many encoded bytes.
const MAX_DATA_URI: usize = 48 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Inl {
    fmt: bool,
    br: &'static str,
}

pub struct Conv<'a> {
    ctx: &'a mut Ctx,
    acc: &'a mut DocAcc,
    res: &'a mut dyn Resolver,
    unit: Option<u32>,
    blocks: Vec<Block>,
    inline: String,
    pending: Option<(u8, String)>,
    lists: Vec<(bool, u32)>,
    svg: u32,
}

/// Converts `nodes` into blocks; figures found on the way are recorded in `acc`.
pub fn convert(
    ctx: &mut Ctx,
    acc: &mut DocAcc,
    res: &mut dyn Resolver,
    unit: Option<u32>,
    nodes: &[Node],
) -> Vec<Block> {
    let mut conv = Conv {
        ctx,
        acc,
        res,
        unit,
        blocks: Vec::new(),
        inline: String::new(),
        pending: None,
        lists: Vec::new(),
        svg: 0,
    };
    conv.walk_children(nodes);
    conv.flush();
    if conv.svg > 0 {
        let n = conv.svg;
        conv.acc.warn(format!("{n} inline SVG graphic(s) were not read; export them as PNG or JPEG to have them described"));
    }
    let mut blocks = std::mem::take(&mut conv.blocks);
    drop(conv);
    crate::md::attach_captions(&mut blocks, &mut acc.figures);
    blocks
}

fn has_special(el: &El) -> bool {
    el.kids
        .iter()
        .any(|k| matches!(k, Node::El(e) if SPECIAL.contains(&e.tag.as_str()) || has_special(e)))
}

fn text_of(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::El(e) if SKIP.contains(&e.tag.as_str()) => {}
            Node::El(e) if e.tag == "br" => out.push('\n'),
            Node::El(e) => text_of(&e.kids, out),
        }
    }
}

fn plain(el: &El) -> String {
    let mut s = String::new();
    text_of(&el.kids, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Conv<'_> {
    fn flush(&mut self) {
        let raw = std::mem::take(&mut self.inline);
        let text = raw.trim();
        let item = self.pending.take();
        if text.is_empty() {
            self.pending = item;
            return;
        }
        let text = text.to_string();
        self.blocks.push(match item {
            Some((depth, marker)) => Block::Item {
                depth,
                marker,
                text,
            },
            None => Block::Para(text),
        });
    }

    fn text(&mut self, t: &str) {
        for ch in t.chars() {
            if ch.is_whitespace() && ch != '\u{a0}' {
                if !self.inline.is_empty() && !self.inline.ends_with([' ', '\n']) {
                    self.inline.push(' ');
                }
            } else if ch != '\u{ad}' {
                self.inline.push(if ch == '\u{a0}' { ' ' } else { ch });
            }
        }
    }

    fn walk_children(&mut self, kids: &[Node]) {
        for k in kids {
            if self.acc.truncated {
                return;
            }
            match k {
                Node::Text(t) => {
                    let escaped = esc(t);
                    self.text(&escaped);
                }
                Node::El(e) => self.element(e),
            }
        }
    }

    fn element(&mut self, el: &El) {
        let tag = el.tag.as_str();
        match tag {
            t if SKIP.contains(&t) => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = tag.as_bytes()[1] - b'0';
                let text = self.inline_string(
                    &el.kids,
                    Inl {
                        fmt: false,
                        br: " ",
                    },
                );
                if !text.is_empty() {
                    self.blocks.push(Block::Heading(level, text));
                }
            }
            "br" => self.inline.push_str("  \n"),
            "hr" => {
                self.flush();
                self.blocks.push(Block::Rule);
            }
            "ul" | "ol" => {
                self.flush();
                let start = el
                    .attr("start")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(1u32);
                self.lists.push((tag == "ol", start));
                self.walk_children(&el.kids);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1).min(6) as u8;
                let marker = match self.lists.last_mut() {
                    Some((true, n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "-".to_string(),
                };
                self.pending = Some((depth, marker));
                self.walk_children(&el.kids);
                self.flush();
                self.pending = None;
            }
            "dt" => {
                self.flush();
                let text = self.inline_string(
                    &el.kids,
                    Inl {
                        fmt: false,
                        br: " ",
                    },
                );
                if !text.is_empty() {
                    self.blocks.push(Block::Para(format!("**{text}**")));
                }
            }
            "pre" => {
                self.flush();
                let mut code = String::new();
                text_of(&el.kids, &mut code);
                if !code.trim().is_empty() {
                    self.blocks.push(Block::Code(code));
                }
            }
            "blockquote" => {
                self.flush();
                let start = self.blocks.len();
                self.walk_children(&el.kids);
                self.flush();
                let inner = self.blocks.split_off(start);
                if !inner.is_empty() {
                    self.blocks.push(Block::Quote(inner));
                }
            }
            "table" => {
                self.flush();
                self.table(el);
            }
            "img" | "image" => {
                self.flush();
                let alt = el
                    .attr("alt")
                    .or_else(|| el.attr("title"))
                    .unwrap_or("")
                    .to_string();
                self.image(el, alt);
            }
            "figure" => {
                self.flush();
                self.figure(el);
            }
            "svg" => {
                self.flush();
                match find(el, "image") {
                    // A cover page is often an SVG wrapping one raster image.
                    Some(img) => self.image(img, String::new()),
                    None => self.svg += 1,
                }
            }
            "p" | "div" | "section" | "article" | "aside" | "header" | "footer" | "nav"
            | "main" | "body" | "html" | "form" | "fieldset" | "address" | "dl" | "dd"
            | "center" | "details" | "summary" | "figcaption" | "caption" => {
                self.flush();
                self.walk_children(&el.kids);
                self.flush();
            }
            _ if has_special(el) => self.walk_children(&el.kids),
            _ => {
                let text = self.inline_string(
                    std::slice::from_ref(&Node::El(el.clone())),
                    Inl {
                        fmt: true,
                        br: "  \n",
                    },
                );
                self.text_raw(&text);
            }
        }
    }

    /// Appends already-collapsed inline Markdown.
    fn text_raw(&mut self, s: &str) {
        let lead = s.starts_with(' ');
        if lead && !self.inline.is_empty() && !self.inline.ends_with([' ', '\n']) {
            self.inline.push(' ');
        }
        self.inline.push_str(s.trim_start_matches(' '));
    }

    fn inline_string(&mut self, nodes: &[Node], mode: Inl) -> String {
        let mut out = String::new();
        inline_nodes(nodes, mode, &mut out);
        out.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(" \n", "\n")
    }

    fn table(&mut self, el: &El) {
        let mut rows: Vec<Vec<String>> = Vec::new();
        collect_rows(el, &mut rows, 0);
        rows.retain(|r| r.iter().any(|c| !c.is_empty()));
        if let Some(caption) = child(el, "caption") {
            let text = plain(caption);
            if !text.is_empty() {
                self.blocks.push(Block::Para(format!("*{}*", esc(&text))));
            }
        }
        if !rows.is_empty() {
            self.blocks.push(Block::Table(rows));
        }
    }

    fn figure(&mut self, el: &El) {
        let caption = find(el, "figcaption").map(plain).unwrap_or_default();
        let mut imgs = Vec::new();
        collect_imgs(el, &mut imgs, 0);
        if imgs.is_empty() {
            self.walk_children(&el.kids);
            self.flush();
            return;
        }
        for (n, img) in imgs.into_iter().enumerate() {
            let alt = img
                .attr("alt")
                .or_else(|| img.attr("title"))
                .unwrap_or("")
                .to_string();
            let cap = if n == 0 && !caption.is_empty() {
                caption.clone()
            } else {
                alt
            };
            self.image(img, cap);
        }
    }

    fn image(&mut self, el: &El, caption: String) {
        let src = el
            .attr("src")
            .or_else(|| el.attr("href"))
            .unwrap_or("")
            .trim();
        if src.is_empty() {
            return;
        }
        let bytes = if let Some(data) = src.strip_prefix("data:") {
            data_uri(data)
        } else if src.contains("://") {
            self.acc.warn(format!(
                "image {} is an external link and was not fetched (the plugin has no network)",
                short(src)
            ));
            None
        } else {
            self.res.image(src)
        };
        let caption = caption.trim().to_string();
        let block = match bytes {
            Some(b) if !is_svg(src, &b) => {
                self.ctx.figure_from_bytes(self.acc, self.unit, &b, caption)
            }
            Some(_) => self.ctx.unreadable_figure(
                self.acc,
                self.unit,
                caption,
                "SVG vector graphics are not rendered",
            ),
            None if !caption.is_empty() => self.ctx.unreadable_figure(
                self.acc,
                self.unit,
                caption,
                "the image file was not found",
            ),
            None => {
                self.acc.warn(format!("image {} was not found", short(src)));
                None
            }
        };
        if let Some(b) = block {
            self.blocks.push(b);
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(80).collect()
}

fn is_svg(src: &str, bytes: &[u8]) -> bool {
    src.split(['?', '#'])
        .next()
        .is_some_and(|p| p.to_ascii_lowercase().ends_with(".svg"))
        || bytes.starts_with(b"<svg")
        || bytes.starts_with(b"<?xml")
}

fn data_uri(data: &str) -> Option<Vec<u8>> {
    let (meta, body) = data.split_once(',')?;
    if !meta.ends_with(";base64") || body.len() > MAX_DATA_URI {
        return None;
    }
    let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .ok()
}

fn child<'e>(el: &'e El, tag: &str) -> Option<&'e El> {
    el.kids.iter().find_map(|k| match k {
        Node::El(e) if e.tag == tag => Some(e),
        _ => None,
    })
}

fn find<'e>(el: &'e El, tag: &str) -> Option<&'e El> {
    el.kids.iter().find_map(|k| match k {
        Node::El(e) if e.tag == tag => Some(e),
        Node::El(e) => find(e, tag),
        Node::Text(_) => None,
    })
}

fn collect_imgs<'e>(el: &'e El, out: &mut Vec<&'e El>, depth: u32) {
    if depth > 6 {
        return;
    }
    for k in &el.kids {
        if let Node::El(e) = k {
            if e.tag == "img" {
                out.push(e);
            } else {
                collect_imgs(e, out, depth + 1);
            }
        }
    }
}

fn collect_rows(el: &El, rows: &mut Vec<Vec<String>>, depth: u32) {
    if depth > 4 {
        return;
    }
    for k in &el.kids {
        let Node::El(e) = k else { continue };
        match e.tag.as_str() {
            "tr" => {
                let mut cells = Vec::new();
                for c in &e.kids {
                    if let Node::El(ce) = c
                        && (ce.tag == "td" || ce.tag == "th")
                    {
                        cells.push(plain(ce));
                        let span: usize = ce
                            .attr("colspan")
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(1usize)
                            .clamp(1, 64);
                        cells.extend(std::iter::repeat_n(String::new(), span - 1));
                    }
                }
                rows.push(cells);
            }
            "thead" | "tbody" | "tfoot" => collect_rows(e, rows, depth + 1),
            _ => {}
        }
    }
}

/// Writes the inline Markdown of `nodes`; block-like children become spaces.
fn inline_nodes(nodes: &[Node], mode: Inl, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(&esc(t).replace(['\u{a0}', '\u{ad}'], " ")),
            Node::El(e) => inline_el(e, mode, out),
        }
    }
}

fn wrap(mode: Inl, e: &El, open: &str, close: &str, out: &mut String) {
    let mut inner = String::new();
    inline_nodes(&e.kids, mode, &mut inner);
    let core = inner.trim();
    if core.is_empty() || !mode.fmt {
        out.push_str(&inner);
        return;
    }
    if inner.starts_with(char::is_whitespace) {
        out.push(' ');
    }
    out.push_str(open);
    out.push_str(core);
    out.push_str(close);
    if inner.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

fn inline_el(e: &El, mode: Inl, out: &mut String) {
    match e.tag.as_str() {
        t if SKIP.contains(&t) => {}
        "br" => out.push_str(mode.br),
        "img" | "svg" | "image" => {}
        "strong" | "b" => wrap(mode, e, "**", "**", out),
        "em" | "i" | "cite" | "dfn" => wrap(mode, e, "*", "*", out),
        "del" | "s" | "strike" => wrap(mode, e, "~~", "~~", out),
        "code" | "tt" | "kbd" | "samp" => {
            let mut raw = String::new();
            text_of(&e.kids, &mut raw);
            let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if raw.is_empty() {
                return;
            }
            if mode.fmt && !raw.contains('`') {
                out.push('`');
                out.push_str(&raw);
                out.push('`');
            } else {
                out.push_str(&esc(&raw));
            }
        }
        "a" => {
            let href = e.attr("href").unwrap_or("").trim();
            let external = ["http://", "https://", "mailto:"]
                .iter()
                .any(|p| href.starts_with(p));
            let mut inner = String::new();
            inline_nodes(&e.kids, mode, &mut inner);
            let text = inner.trim();
            if mode.fmt && external && !text.is_empty() && !href.contains([' ', ')']) {
                out.push_str(&format!("[{text}]({href})"));
            } else {
                out.push_str(&inner);
            }
        }
        t if INLINE_BLOCKISH.contains(&t) => {
            out.push(' ');
            inline_nodes(&e.kids, mode, out);
            out.push(' ');
        }
        _ => inline_nodes(&e.kids, mode, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse;
    use crate::input::Options;

    struct Files;
    impl Resolver for Files {
        fn image(&mut self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }

    fn md(html: &str) -> String {
        let mut ctx = Ctx::new(Options::default());
        let mut acc = DocAcc::default();
        let blocks = convert(&mut ctx, &mut acc, &mut Files, Some(1), &parse(html));
        crate::md::render(&blocks)
    }

    #[test]
    fn headings_paragraphs_and_inline_formatting() {
        assert_eq!(
            md(
                "<h1>Title</h1><p>Some <b>bold</b> and <em>it</em> with <a href='https://x.io/a'>a link</a> and <code>x_1</code>.</p>"
            ),
            "# Title\n\nSome **bold** and *it* with [a link](https://x.io/a) and `x_1`."
        );
    }

    #[test]
    fn nested_lists_keep_order_and_depth() {
        assert_eq!(
            md(
                "<ul><li>one<ul><li>inner</li></ul></li><li>two</li></ul><ol start=3><li>c</li><li>d</li></ol>"
            ),
            "- one\n  - inner\n- two\n\n3. c\n4. d"
        );
    }

    #[test]
    fn tables_become_markdown_tables() {
        assert_eq!(
            md(
                "<table><caption>Sales</caption><tr><th>Name</th><th>Qty</th></tr><tr><td>Pen</td><td>3</td></tr><tr><td colspan=2>x|y</td></tr></table>"
            ),
            "*Sales*\n\n| Name | Qty |\n| --- | --- |\n| Pen | 3 |\n| x\\|y |  |"
        );
    }

    #[test]
    fn quote_code_and_rule() {
        assert_eq!(
            md("<blockquote><p>wise</p></blockquote><pre>a\n  b</pre><hr>"),
            "> wise\n\n```\na\n  b\n```\n\n---"
        );
    }

    #[test]
    fn missing_image_with_alt_is_still_listed() {
        let text = md("<p>before</p><img src='gone.png' alt='A chart'><p>after</p>");
        assert!(text.contains("**[Figure fig-1]** A chart"), "{text}");
        assert!(text.contains("the image file was not found"), "{text}");
    }

    #[test]
    fn scripts_and_head_are_skipped() {
        assert_eq!(
            md("<head><title>T</title></head><body><script>var x</script><p>only this</p></body>"),
            "only this"
        );
    }
}
