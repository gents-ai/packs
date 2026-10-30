//! Plain text from a fetched body, without an HTML library.
//!
//! HTML: the content of `script`, `style`, `nav`, `footer`, `noscript`,
//! `template`, `svg` and `head` is dropped (the `<title>` is read separately),
//! comments and declarations are dropped, block elements end a line, inline
//! tags vanish as a browser renders them, the common entities are decoded and
//! `&nbsp;` becomes a space. Within a line, runs of whitespace become one
//! space; lines are trimmed and empty lines dropped, so the text is one
//! paragraph, heading, list item or table cell per line and a quote copied
//! from a paragraph is a substring of one line.

/// The most characters a Source keeps.
pub const MAX_TEXT_CHARS: usize = 200_000;

/// Elements whose whole content is not page text.
const DROPPED: [&str; 8] = ["script", "style", "nav", "footer", "noscript", "template", "svg", "head"];
/// Elements whose content is code or markup to the end even without a closing tag.
const RAW: [&str; 2] = ["script", "style"];
/// Elements that start or end a line.
const BLOCKS: [&str; 44] = [
    "address", "article", "aside", "blockquote", "body", "br", "caption", "dd", "details", "dialog",
    "div", "dl", "dt", "fieldset", "figcaption", "figure", "form", "h1", "h2", "h3", "h4", "h5",
    "h6", "header", "hr", "html", "li", "main", "ol", "p", "pre", "section", "summary", "table",
    "tbody", "td", "tfoot", "th", "thead", "title", "tr", "ul", "option", "legend",
];

/// The text kept and whether it was cut at [`MAX_TEXT_CHARS`].
pub fn cap(text: String) -> (String, bool) {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((at, _)) => (text[..at].to_owned(), true),
        None => (text, false),
    }
}

/// The `<title>`'s text, decoded and on one line; `None` when absent or blank.
pub fn title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<title").map(|at| at + from) {
        let after = lower.as_bytes().get(at + 6).copied();
        if !matches!(after, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
            from = at + 6;
            continue;
        }
        let open_end = tag_end(html, at)?;
        let close = lower[open_end..].find("</title").map(|c| c + open_end)?;
        let text = collapse(&decode(&html[open_end..close]));
        return (!text.is_empty()).then_some(text);
    }
    None
}

/// The page text of an HTML document.
pub fn html_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len() / 2);
    let mut text_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &lower[i..];
        let skip_to = if rest.starts_with("<!--") {
            Some(rest.find("-->").map_or(bytes.len(), |e| i + e + 3))
        } else if rest.starts_with("<![cdata[") {
            Some(rest.find("]]>").map_or(bytes.len(), |e| i + e + 3))
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            Some(rest.find('>').map_or(bytes.len(), |e| i + e + 1))
        } else {
            None
        };
        if let Some(end) = skip_to {
            push_text(&mut out, &html[text_start..i]);
            i = end;
            text_start = i;
            continue;
        }
        let Some((name, closing)) = tag_name(&lower[i + 1..]) else {
            i += 1;
            continue;
        };
        let Some(end) = tag_end(html, i) else {
            break;
        };
        push_text(&mut out, &html[text_start..i]);
        let self_closing = html[..end].ends_with("/>");
        let mut next = end;
        if !closing && !self_closing && DROPPED.contains(&name) {
            match find_close(&lower, end, name) {
                Some(after) => next = after,
                None if RAW.contains(&name) => next = bytes.len(),
                None => {}
            }
        }
        if BLOCKS.contains(&name) || DROPPED.contains(&name) {
            out.push('\n');
        }
        i = next;
        text_start = i;
    }
    if text_start < bytes.len() {
        push_text(&mut out, &html[text_start..]);
    }
    lines(&out)
}

fn push_text(out: &mut String, raw: &str) {
    // A line break in the markup is whitespace; only block elements end a line.
    if !raw.is_empty() {
        out.push_str(&decode(raw).replace(['\n', '\r'], " "));
    }
}

/// The tag's name, lowercased, and whether it closes; `None` when the `<` does
/// not start a tag (`a < b` in text).
fn tag_name(after_lt: &str) -> Option<(&str, bool)> {
    let (closing, body) = match after_lt.strip_prefix('/') {
        Some(body) => (true, body),
        None => (false, after_lt),
    };
    if !body.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let end = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ':'))
        .unwrap_or(body.len());
    Some((&body[..end], closing))
}

/// The byte after the `>` that ends the tag starting at `at`, skipping `>`
/// inside quoted attribute values.
fn tag_end(html: &str, at: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, b) in html.as_bytes()[at..].iter().enumerate() {
        match (quote, *b) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(*b),
            (None, b'>') => return Some(at + offset + 1),
            _ => {}
        }
    }
    None
}

/// The byte after `</name ...>` from `from` on, in the lowercased text.
fn find_close(lower: &str, from: usize, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    let mut search = from;
    while let Some(at) = lower[search..].find(&needle).map(|a| a + search) {
        let after = lower.as_bytes().get(at + needle.len()).copied();
        if matches!(after, None | Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
            return Some(lower[at..].find('>').map_or(lower.len(), |e| at + e + 1));
        }
        search = at + needle.len();
    }
    None
}

/// Whitespace runs to one space, on one line.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Each line collapsed and trimmed; empty lines dropped.
fn lines(text: &str) -> String {
    text.split('\n')
        .map(collapse)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The common named entities and every numeric one; an unknown or malformed
/// reference stays as written.
pub fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.as_bytes()[1..]
            .iter()
            .take(32)
            .position(|b| *b == b';')
            .map(|p| p + 1);
        let decoded = end.and_then(|end| entity(&rest[1..end]).map(|c| (c, end + 1)));
        match decoded {
            Some((value, used)) => {
                out.push_str(value.as_str());
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<String> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        let c = match code {
            0 => '\u{fffd}',
            0xa0 => ' ',
            code => char::from_u32(code)?,
        };
        return Some(c.to_string());
    }
    let s = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" | "ensp" | "emsp" | "thinsp" => " ",
        "shy" | "zwnj" | "zwj" => "",
        "ndash" => "\u{2013}",
        "mdash" => "\u{2014}",
        "hellip" => "\u{2026}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "sbquo" => "\u{201a}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "bdquo" => "\u{201e}",
        "laquo" => "\u{ab}",
        "raquo" => "\u{bb}",
        "bull" => "\u{2022}",
        "middot" => "\u{b7}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "trade" => "\u{2122}",
        "deg" => "\u{b0}",
        "plusmn" => "\u{b1}",
        "times" => "\u{d7}",
        "divide" => "\u{f7}",
        "sect" => "\u{a7}",
        "para" => "\u{b6}",
        "euro" => "\u{20ac}",
        "pound" => "\u{a3}",
        "yen" => "\u{a5}",
        "cent" => "\u{a2}",
        "frac12" => "\u{bd}",
        "frac14" => "\u{bc}",
        "frac34" => "\u{be}",
        "eacute" => "\u{e9}",
        "egrave" => "\u{e8}",
        "aacute" => "\u{e1}",
        "agrave" => "\u{e0}",
        "ouml" => "\u{f6}",
        "uuml" => "\u{fc}",
        "auml" => "\u{e4}",
        "szlig" => "\u{df}",
        "ccedil" => "\u{e7}",
        "ntilde" => "\u{f1}",
        _ => return None,
    };
    Some(s.to_owned())
}
