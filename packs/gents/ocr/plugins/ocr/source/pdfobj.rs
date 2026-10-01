//! A small PDF object parser for the lazy reader: it finds the values of a
//! dictionary and the references inside it, and remembers where each value
//! sits in the bytes so a dictionary can be rewritten without re-encoding it.
//! It reads only the syntax the lazy reader needs, never a content stream.

/// Nesting deeper than this is refused (a crafted file could nest forever).
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Null,
    Bool(bool),
    Int(i64),
    Real,
    Name(Vec<u8>),
    Str,
    Ref(u32, u16),
    Arr(Vec<Val>),
    Dict(Vec<(Vec<u8>, Val)>),
    /// A bare keyword such as `stream` or `endobj`.
    Word(Vec<u8>),
}

/// A parsed value and the byte range it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Val {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    /// The bytes end inside a value: more may complete it.
    Incomplete,
    Bad,
}

pub fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Skips white space and comments; `eof` says the buffer is the end of the file.
pub fn skip_ws(buf: &[u8], mut pos: usize, eof: bool) -> Result<usize, Fail> {
    while let Some(&b) = buf.get(pos) {
        if is_space(b) {
            pos += 1;
        } else if b == b'%' {
            match buf[pos..].iter().position(|&c| c == b'\n' || c == b'\r') {
                Some(n) => pos += n,
                None if eof => return Ok(buf.len()),
                None => return Err(Fail::Incomplete),
            }
        } else {
            break;
        }
    }
    Ok(pos)
}

/// The end of a run of regular characters from `pos`.
fn word_end(buf: &[u8], pos: usize, eof: bool) -> Result<usize, Fail> {
    let n = buf[pos..]
        .iter()
        .position(|&b| is_space(b) || is_delim(b))
        .map(|n| pos + n);
    match n {
        Some(end) => Ok(end),
        None if eof => Ok(buf.len()),
        None => Err(Fail::Incomplete),
    }
}

fn unescape_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if raw[i] == b'#'
            && let (Some(h), Some(l)) = (
                raw.get(i + 1).and_then(|&b| hex(b)),
                raw.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

fn number(token: &[u8]) -> Option<Kind> {
    let text = std::str::from_utf8(token).ok()?;
    if text
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'+' || b == b'-')
    {
        return text.parse::<i64>().ok().map(Kind::Int);
    }
    text.parse::<f64>().ok().map(|_| Kind::Real)
}

/// Parses one value starting at `pos` (white space before it is skipped).
pub fn parse_value(buf: &[u8], pos: usize, eof: bool) -> Result<Val, Fail> {
    value(buf, pos, eof, 0)
}

fn value(buf: &[u8], pos: usize, eof: bool, depth: usize) -> Result<Val, Fail> {
    if depth > MAX_DEPTH {
        return Err(Fail::Bad);
    }
    let start = skip_ws(buf, pos, eof)?;
    let Some(&c) = buf.get(start) else {
        return Err(Fail::Incomplete);
    };
    let done = |end: usize, kind: Kind| Ok(Val { start, end, kind });
    match c {
        b'/' => {
            let end = word_end(buf, start + 1, eof)?;
            done(end, Kind::Name(unescape_name(&buf[start + 1..end])))
        }
        b'(' => {
            let (mut level, mut i) = (1usize, start + 1);
            while level > 0 {
                match buf.get(i) {
                    None => return Err(Fail::Incomplete),
                    Some(b'\\') => i += 1,
                    Some(b'(') => level += 1,
                    Some(b')') => level -= 1,
                    _ => {}
                }
                i += 1;
            }
            if i > buf.len() {
                return Err(Fail::Incomplete);
            }
            done(i, Kind::Str)
        }
        b'<' if buf.get(start + 1) == Some(&b'<') => {
            let mut entries = Vec::new();
            let mut i = start + 2;
            loop {
                i = skip_ws(buf, i, eof)?;
                match (buf.get(i), buf.get(i + 1)) {
                    (None, _) | (Some(b'>'), None) => return Err(Fail::Incomplete),
                    (Some(b'>'), Some(b'>')) => return done(i + 2, Kind::Dict(entries)),
                    (Some(b'/'), _) => {}
                    _ => return Err(Fail::Bad),
                }
                let key = value(buf, i, eof, depth + 1)?;
                let Kind::Name(name) = key.kind else {
                    return Err(Fail::Bad);
                };
                let val = value(buf, key.end, eof, depth + 1)?;
                if matches!(val.kind, Kind::Word(_)) {
                    return Err(Fail::Bad);
                }
                i = val.end;
                entries.push((name, val));
            }
        }
        b'<' => match buf[start..].iter().position(|&b| b == b'>') {
            Some(n) => done(start + n + 1, Kind::Str),
            None => Err(Fail::Incomplete),
        },
        b'[' => {
            let mut items = Vec::new();
            let mut i = start + 1;
            loop {
                i = skip_ws(buf, i, eof)?;
                match buf.get(i) {
                    None => return Err(Fail::Incomplete),
                    Some(b']') => return done(i + 1, Kind::Arr(items)),
                    _ => {}
                }
                let v = value(buf, i, eof, depth + 1)?;
                if matches!(v.kind, Kind::Word(_)) {
                    return Err(Fail::Bad);
                }
                i = v.end;
                items.push(v);
            }
        }
        b')' | b'>' | b']' | b'{' | b'}' => Err(Fail::Bad),
        _ => {
            let end = word_end(buf, start, eof)?;
            let token = &buf[start..end];
            match token {
                b"true" => return done(end, Kind::Bool(true)),
                b"false" => return done(end, Kind::Bool(false)),
                b"null" => return done(end, Kind::Null),
                _ => {}
            }
            let Some(kind) = number(token) else {
                return done(end, Kind::Word(token.to_vec()));
            };
            if let Kind::Int(n) = kind
                && token.iter().all(u8::is_ascii_digit)
                && let Some(r) = reference(buf, n, end, eof)?
            {
                return Ok(Val {
                    start,
                    end: r.0,
                    kind: Kind::Ref(r.1, r.2),
                });
            }
            done(end, kind)
        }
    }
}

/// `rev R` after the object number `num` that ended at `at`: the end of the
/// reference, its number and generation.
fn reference(
    buf: &[u8],
    num: i64,
    at: usize,
    eof: bool,
) -> Result<Option<(usize, u32, u16)>, Fail> {
    let Ok(num) = u32::try_from(num) else {
        return Ok(None);
    };
    let g0 = skip_ws(buf, at, eof)?;
    if g0 == at {
        return Ok(None);
    }
    let g1 = word_end(buf, g0, eof)?;
    let Some(rev) = std::str::from_utf8(&buf[g0..g1])
        .ok()
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|t| t.parse::<u16>().ok())
    else {
        return Ok(None);
    };
    let r0 = skip_ws(buf, g1, eof)?;
    if r0 == g1 {
        return Ok(None);
    }
    match buf.get(r0) {
        Some(b'R') => {
            let r1 = word_end(buf, r0, eof)?;
            Ok((r1 == r0 + 1).then_some((r1, num, rev)))
        }
        None if !eof => Err(Fail::Incomplete),
        _ => Ok(None),
    }
}

impl Val {
    pub fn get(&self, key: &str) -> Option<&Val> {
        match &self.kind {
            Kind::Dict(entries) => entries
                .iter()
                .find(|(k, _)| k == key.as_bytes())
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self.kind {
            Kind::Int(n) => Some(n),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<&[u8]> {
        match &self.kind {
            Kind::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn reference(&self) -> Option<(u32, u16)> {
        match self.kind {
            Kind::Ref(n, g) => Some((n, g)),
            _ => None,
        }
    }

    /// Every reference inside this value, nested ones included.
    pub fn refs(&self, out: &mut Vec<(u32, u16)>) {
        match &self.kind {
            Kind::Ref(n, g) => out.push((*n, *g)),
            Kind::Arr(items) => items.iter().for_each(|v| v.refs(out)),
            Kind::Dict(entries) => entries.iter().for_each(|(_, v)| v.refs(out)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Val {
        parse_value(text.as_bytes(), 0, true).unwrap()
    }

    #[test]
    fn a_dictionary_gives_its_values_and_their_spans() {
        let text =
            "<< /Type /Page /Kids [1 0 R 22 3 R] /Count 2 /S (a (b) \\) c) /H <4142> /R 1.5 >>";
        let v = parse(text);
        assert_eq!(v.get("Type").unwrap().name(), Some(&b"Page"[..]));
        assert_eq!(v.get("Count").unwrap().int(), Some(2));
        let mut refs = Vec::new();
        v.get("Kids").unwrap().refs(&mut refs);
        assert_eq!(refs, vec![(1, 0), (22, 3)]);
        let s = v.get("S").unwrap();
        assert_eq!(&text[s.start..s.end], "(a (b) \\) c)");
        assert_eq!(v.get("R").unwrap().kind, Kind::Real);
        assert_eq!(v.end, text.len());
    }

    #[test]
    fn two_integers_are_not_a_reference_without_the_r() {
        let v = parse("[1 0 2 0 R 7]");
        let Kind::Arr(items) = v.kind else { panic!() };
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].int(), Some(1));
        assert_eq!(items[2].reference(), Some((2, 0)));
        assert_eq!(items[3].int(), Some(7));
    }

    #[test]
    fn names_decode_escapes_and_comments_are_skipped() {
        let v = parse("<< /A#20B 1 % note\n /C 2 >>");
        assert_eq!(v.get("A B").unwrap().int(), Some(1));
        assert_eq!(v.get("C").unwrap().int(), Some(2));
    }

    #[test]
    fn a_truncated_value_is_incomplete_and_garbage_is_bad() {
        assert_eq!(parse_value(b"<< /A [1 2", 0, false), Err(Fail::Incomplete));
        assert_eq!(parse_value(b"<< /A 1", 0, true), Err(Fail::Incomplete));
        assert_eq!(parse_value(b"<< 1 2 >>", 0, true), Err(Fail::Bad));
        let deep = "[".repeat(200);
        assert_eq!(parse_value(deep.as_bytes(), 0, true), Err(Fail::Bad));
    }

    #[test]
    fn a_trailing_keyword_is_a_word() {
        let v = parse("<< /Length 3 >> stream");
        let after = parse_value(b"<< /Length 3 >> stream", v.end, true).unwrap();
        assert_eq!(after.kind, Kind::Word(b"stream".to_vec()));
    }
}
