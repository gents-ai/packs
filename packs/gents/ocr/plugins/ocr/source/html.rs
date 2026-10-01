//! A forgiving HTML and XHTML parser: tags, attributes, entities, void and
//! implied-close elements, bounded nesting. It builds a small tree that the
//! Markdown converter walks; it never fails, it recovers like a browser would.

#[derive(Debug, Clone)]
pub enum Node {
    Text(String),
    El(El),
}

#[derive(Debug, Clone, Default)]
pub struct El {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Node>,
}

impl El {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Open elements nest at most this deep; deeper tags are dropped, their content kept.
const MAX_DEPTH: usize = 200;
const MAX_NODES: usize = 4_000_000;

const VOID: [&str; 16] = [
    "br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "source", "track",
    "wbr", "param", "command", "keygen",
];
const RAW: [&str; 3] = ["script", "style", "textarea"];
const BLOCKS: [&str; 26] = [
    "p",
    "div",
    "ul",
    "ol",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "hr",
    "section",
    "article",
    "aside",
    "header",
    "footer",
    "nav",
    "main",
    "figure",
    "dl",
    "form",
    "fieldset",
    "address",
];

fn closes(top: &str, new: &str) -> bool {
    match (top, new) {
        ("p", n) => BLOCKS.contains(&n),
        ("li", "li") => true,
        ("dt" | "dd", "dt" | "dd") => true,
        ("td" | "th", "td" | "th" | "tr" | "tbody" | "thead" | "tfoot") => true,
        ("tr", "tr" | "tbody" | "thead" | "tfoot") => true,
        ("thead" | "tbody" | "tfoot", "thead" | "tbody" | "tfoot") => true,
        ("option", "option") => true,
        _ => false,
    }
}

pub fn decode_entities(s: &str) -> String {
    if s.contains('&') {
        html_escape::decode_html_entities(s).into_owned()
    } else {
        s.to_string()
    }
}

/// Parses a document or fragment into its top-level nodes.
pub fn parse(src: &str) -> Vec<Node> {
    let b = src.as_bytes();
    let mut stack: Vec<El> = vec![El::default()];
    let mut i = 0;
    let mut nodes = 0usize;
    while i < b.len() && nodes < MAX_NODES {
        if b[i] != b'<' {
            let end = src[i..].find('<').map_or(b.len(), |p| i + p);
            push_text(&mut stack, &src[i..end]);
            i = end;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(b.len(), |p| i + p + 3);
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            push_raw_text(&mut stack, &body[..end]);
            i += 9 + end + 3.min(body.len() - end);
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            i = rest.find('>').map_or(b.len(), |p| i + p + 1);
        } else if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').unwrap_or(after.len());
            let name = local_name(after[..end].trim());
            close(&mut stack, &name);
            i += 2 + end + 1;
        } else if rest.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic) {
            let (el, consumed, self_closing) = start_tag(rest);
            i += consumed;
            nodes += 1;
            open(&mut stack, el, self_closing);
            let tag = stack_tag_after_open(&stack);
            if let Some(tag) = tag.filter(|t| RAW.contains(&t.as_str())) {
                let lower = src[i..].to_ascii_lowercase();
                let end = lower.find(&format!("</{tag}")).unwrap_or(lower.len());
                if tag == "textarea" {
                    push_raw_text(&mut stack, &src[i..i + end]);
                }
                i += end;
            }
        } else {
            push_text(&mut stack, "<");
            i += 1;
        }
    }
    while stack.len() > 1 {
        pop(&mut stack);
    }
    stack.pop().map(|root| root.kids).unwrap_or_default()
}

fn stack_tag_after_open(stack: &[El]) -> Option<String> {
    stack
        .last()
        .filter(|_| stack.len() > 1)
        .map(|e| e.tag.clone())
}

fn local_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>')
        .collect();
    name.rsplit(':').next().unwrap_or("").to_ascii_lowercase()
}

fn push_text(stack: &mut [El], text: &str) {
    if text.is_empty() {
        return;
    }
    push_raw_text(stack, &decode_entities(text));
}

fn push_raw_text(stack: &mut [El], text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(top) = stack.last_mut() {
        match top.kids.last_mut() {
            Some(Node::Text(t)) => t.push_str(text),
            _ => top.kids.push(Node::Text(text.to_string())),
        }
    }
}

fn pop(stack: &mut Vec<El>) {
    if stack.len() > 1
        && let Some(el) = stack.pop()
        && let Some(parent) = stack.last_mut()
    {
        parent.kids.push(Node::El(el));
    }
}

fn open(stack: &mut Vec<El>, el: El, self_closing: bool) {
    while stack.len() > 1 && stack.last().is_some_and(|t| closes(&t.tag, &el.tag)) {
        pop(stack);
    }
    if VOID.contains(&el.tag.as_str()) || self_closing {
        if let Some(top) = stack.last_mut() {
            top.kids.push(Node::El(el));
        }
        return;
    }
    // A tag past the depth bound is dropped; its content stays in the parent.
    if stack.len() < MAX_DEPTH {
        stack.push(el);
    }
}

fn close(stack: &mut Vec<El>, name: &str) {
    if let Some(pos) = stack.iter().rposition(|e| e.tag == name)
        && pos > 0
    {
        while stack.len() > pos {
            pop(stack);
        }
    }
}

/// Parses one start tag at the head of `s`; returns the element, the bytes
/// consumed and whether it was written self-closing.
fn start_tag(s: &str) -> (El, usize, bool) {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let mut el = El {
        tag: local_name(&s[1..i]),
        ..El::default()
    };
    let mut self_closing = false;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'>' => {
                i += 1;
                break;
            }
            b'/' => {
                self_closing = b.get(i + 1) == Some(&b'>');
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len()
                    && !b[i].is_ascii_whitespace()
                    && b[i] != b'='
                    && b[i] != b'>'
                    && b[i] != b'/'
                {
                    i += 1;
                }
                let name = local_name(&s[start..i]);
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut value = String::new();
                if b.get(i) == Some(&b'=') {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    match b.get(i) {
                        Some(&q) if q == b'"' || q == b'\'' => {
                            let vstart = i + 1;
                            let vend = s[vstart..].find(q as char).map_or(b.len(), |p| vstart + p);
                            value = decode_entities(&s[vstart..vend]);
                            i = (vend + 1).min(b.len());
                        }
                        _ => {
                            let vstart = i;
                            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                                i += 1;
                            }
                            value = decode_entities(&s[vstart..i]);
                        }
                    }
                }
                if !name.is_empty() && el.attr(&name).is_none() {
                    el.attrs.push((name, value));
                }
            }
        }
    }
    (el, i, self_closing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(nodes: &[Node]) -> String {
        nodes
            .iter()
            .map(|n| match n {
                Node::Text(t) => t.clone(),
                Node::El(e) => format!("<{}>{}</{}>", e.tag, shape(&e.kids), e.tag),
            })
            .collect()
    }

    #[test]
    fn implied_closes_and_void_elements() {
        assert_eq!(
            shape(&parse("<ul><li>a<li>b</ul><p>x<p>y<br>z")),
            "<ul><li>a</li><li>b</li></ul><p>x</p><p>y<br></br>z</p>"
        );
        assert_eq!(
            shape(&parse("<table><tr><td>1<td>2<tr><td>3</table>")),
            "<table><tr><td>1</td><td>2</td></tr><tr><td>3</td></tr></table>"
        );
    }

    #[test]
    fn entities_comments_scripts_and_namespaces() {
        assert_eq!(
            shape(&parse(
                "a &amp; b &lt;<!-- hidden --><script>x<y</script><svg:rect/>c"
            )),
            "a & b <<script></script><rect></rect>c"
        );
        let nodes = parse(r#"<IMG SRC='a.png' alt="x &amp; y" epub:type=z>"#);
        let Node::El(e) = &nodes[0] else {
            panic!("expected an element")
        };
        assert_eq!(
            (e.tag.as_str(), e.attr("src"), e.attr("alt"), e.attr("type")),
            ("img", Some("a.png"), Some("x & y"), Some("z"))
        );
    }

    #[test]
    fn stray_end_tags_and_unterminated_input_recover() {
        assert_eq!(shape(&parse("a</b></div>c<p>open")), "ac<p>open</p>");
        assert_eq!(shape(&parse("<p attr=\"unterminated")), "<p></p>");
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = "<div>".repeat(5000) + "text";
        let nodes = parse(&deep);
        fn depth(n: &[Node]) -> usize {
            n.iter()
                .map(|x| {
                    if let Node::El(e) = x {
                        1 + depth(&e.kids)
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0)
        }
        assert!(depth(&nodes) <= MAX_DEPTH);
    }
}
