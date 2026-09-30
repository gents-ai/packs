//! Reading-order layout for positioned text: spans from a PDF text layer or
//! from OCR lines become headings, paragraphs, lists and tables.
//!
//! The page is cut recursively, XY-cut style: a region that looks like a table
//! becomes a table; otherwise a vertical gutter splits it into columns, read
//! left to right; otherwise a large horizontal gap splits it into bands, read
//! top to bottom; what remains is one run of lines, grouped into paragraphs.
use crate::model::Block;

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub bottom: f32,
    pub size: f32,
}

impl Span {
    fn cy(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

#[derive(Clone, Debug)]
pub struct FigBox {
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub bottom: f32,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub enum Item {
    Text(Span),
    Fig(FigBox),
}

pub enum Out {
    Block(Block),
    Fig(usize),
}

const MAX_DEPTH: u32 = 48;

fn bbox(it: &Item) -> (f32, f32, f32, f32) {
    match it {
        Item::Text(s) => (s.x0, s.x1, s.top, s.bottom),
        Item::Fig(f) => (f.x0, f.x1, f.top, f.bottom),
    }
}

/// The most common font size, weighted by characters: the body text size.
fn body_size(items: &[Item]) -> f32 {
    let mut hist: Vec<(i32, usize)> = Vec::new();
    for it in items {
        if let Item::Text(s) = it {
            let key = (s.size * 2.0).round() as i32;
            let n = s.text.chars().count();
            match hist.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => e.1 += n,
                None => hist.push((key, n)),
            }
        }
    }
    hist.into_iter()
        .max_by_key(|&(k, n)| (n, -k))
        .map_or(10.0, |(k, _)| (k as f32 / 2.0).max(1.0))
}

pub fn layout(items: &[Item]) -> Vec<Out> {
    let mut out = Vec::new();
    if items.is_empty() {
        return out;
    }
    let body = body_size(items);
    region(items, (0..items.len()).collect(), body, &mut out, 0);
    out
}

fn region(items: &[Item], idx: Vec<usize>, body: f32, out: &mut Vec<Out>, depth: u32) {
    if idx.is_empty() {
        return;
    }
    if depth < MAX_DEPTH {
        if let Some(groups) = fcut(items, &idx, body) {
            for g in groups {
                region(items, g, body, out, depth + 1);
            }
            return;
        }
        if let Some(rows) = table(items, &idx, body) {
            out.push(Out::Block(Block::Table(rows)));
            return;
        }
        if let Some(groups) = [vcut(items, &idx, body), hcut(items, &idx, body)]
            .into_iter()
            .flatten()
            .next()
        {
            for g in groups {
                region(items, g, body, out, depth + 1);
            }
            return;
        }
    }
    leaf(items, idx, body, out);
}

/// Splits a region around its first figure: what sits above it, the figure with
/// anything beside it, and what sits below, so a figure never hides a table.
fn fcut(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Vec<usize>>> {
    let fig = idx
        .iter()
        .copied()
        .filter(|&i| matches!(items[i], Item::Fig(_)))
        .min_by(|&a, &b| bbox(&items[a]).2.total_cmp(&bbox(&items[b]).2))?;
    let (_, _, top, bottom) = bbox(&items[fig]);
    let tol = 0.5 * body;
    let (mut above, mut mid, mut below) = (Vec::new(), Vec::new(), Vec::new());
    for &i in idx {
        let (_, _, t, b) = bbox(&items[i]);
        if b <= top + tol {
            above.push(i);
        } else if t >= bottom - tol {
            below.push(i);
        } else {
            mid.push(i);
        }
    }
    let groups: Vec<Vec<usize>> = [above, mid, below]
        .into_iter()
        .filter(|g| !g.is_empty())
        .collect();
    (groups.len() > 1).then_some(groups)
}

/// Splits at vertical gutters; columns narrower than a few em merge into the next.
fn vcut(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Vec<usize>>> {
    let mut order = idx.to_vec();
    order.sort_by(|&a, &b| bbox(&items[a]).0.total_cmp(&bbox(&items[b]).0));
    let gutter = 1.2 * body;
    let mut groups: Vec<(Vec<usize>, f32, f32)> = Vec::new();
    for &i in &order {
        let (x0, x1, ..) = bbox(&items[i]);
        match groups.last_mut() {
            Some(g) if x0 - g.2 < gutter => {
                g.0.push(i);
                g.2 = g.2.max(x1);
            }
            _ => groups.push((vec![i], x0, x1)),
        }
    }
    let min_width = 5.0 * body;
    let mut merged: Vec<(Vec<usize>, f32, f32)> = Vec::new();
    for g in groups {
        match merged.last_mut() {
            Some(prev) if prev.2 - prev.1 < min_width => {
                prev.0.extend(g.0);
                prev.2 = prev.2.max(g.2);
            }
            _ => merged.push(g),
        }
    }
    if merged.len() > 1
        && merged
            .last()
            .is_some_and(|last| last.2 - last.1 < min_width)
    {
        let tail = merged.pop()?;
        merged.last_mut()?.0.extend(tail.0);
    }
    (merged.len() > 1).then(|| merged.into_iter().map(|g| g.0).collect())
}

/// Splits at horizontal gaps clearly larger than the region's own line gaps.
fn hcut(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Vec<usize>>> {
    let mut order = idx.to_vec();
    order.sort_by(|&a, &b| bbox(&items[a]).2.total_cmp(&bbox(&items[b]).2));
    let mut gaps = Vec::with_capacity(order.len());
    let mut max_bottom = f32::MIN;
    for (k, &i) in order.iter().enumerate() {
        let (_, _, top, bottom) = bbox(&items[i]);
        if k > 0 {
            gaps.push((top - max_bottom).max(0.0));
        }
        max_bottom = max_bottom.max(bottom);
    }
    if gaps.is_empty() {
        return None;
    }
    let mut sorted = gaps.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let threshold = (1.2 * body).max(2.0 * median);
    let mut groups: Vec<Vec<usize>> = vec![vec![order[0]]];
    for (k, &i) in order.iter().enumerate().skip(1) {
        if gaps[k - 1] >= threshold {
            groups.push(Vec::new());
        }
        if let Some(g) = groups.last_mut() {
            g.push(i);
        }
    }
    (groups.len() > 1).then_some(groups)
}

struct Row {
    cy: f32,
    spans: Vec<usize>,
}

fn rows_of(items: &[Item], spans: &[usize], tol: f32) -> Vec<Row> {
    let mut order = spans.to_vec();
    order.sort_by(|&a, &b| span(items, a).cy().total_cmp(&span(items, b).cy()));
    let mut rows: Vec<Row> = Vec::new();
    for i in order {
        let cy = span(items, i).cy();
        match rows.last_mut() {
            Some(r) if (cy - r.cy).abs() <= tol => {
                r.spans.push(i);
                r.cy = (r.cy * (r.spans.len() - 1) as f32 + cy) / r.spans.len() as f32;
            }
            _ => rows.push(Row { cy, spans: vec![i] }),
        }
    }
    for r in &mut rows {
        r.spans
            .sort_by(|&a, &b| span(items, a).x0.total_cmp(&span(items, b).x0));
    }
    rows
}

fn span(items: &[Item], i: usize) -> &Span {
    match &items[i] {
        Item::Text(s) => s,
        Item::Fig(_) => unreachable_span(),
    }
}

fn unreachable_span() -> &'static Span {
    static EMPTY: Span = Span {
        text: String::new(),
        x0: 0.0,
        x1: 0.0,
        top: 0.0,
        bottom: 0.0,
        size: 1.0,
    };
    &EMPTY
}

/// Recognises aligned short cells in three or more rows as a table.
fn table(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Vec<String>>> {
    if idx.len() < 6 || idx.iter().any(|&i| matches!(items[i], Item::Fig(_))) {
        return None;
    }
    let rows = rows_of(items, idx, 0.45 * body);
    // Columns come from the rows that have several cells; a single wide span
    // (a heading above the table) must not glue two columns together.
    let multi_rows: Vec<&Row> = rows.iter().filter(|r| r.spans.len() >= 2).collect();
    if multi_rows.len() < 3 {
        return None;
    }
    let mut lens: Vec<usize> = idx
        .iter()
        .map(|&i| span(items, i).text.chars().count())
        .collect();
    lens.sort_unstable();
    if lens[lens.len() / 2] > 22 {
        return None;
    }
    let mut spans: Vec<usize> = multi_rows
        .iter()
        .flat_map(|r| r.spans.iter().copied())
        .collect();
    spans.sort_by(|&a, &b| span(items, a).x0.total_cmp(&span(items, b).x0));
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for &i in &spans {
        let s = span(items, i);
        match cols.last_mut() {
            Some(c) if s.x0 - c.1 < 0.8 * body => c.1 = c.1.max(s.x1),
            _ => cols.push((s.x0, s.x1)),
        }
    }
    if cols.len() < 2 || cols.len() > 30 {
        return None;
    }
    let bridges = idx.iter().any(|&i| {
        let s = span(items, i);
        cols.iter().filter(|c| s.x0 < c.1 && s.x1 > c.0).count() > 1
    });
    if bridges {
        return None;
    }
    let col_of = |s: &Span| {
        let cx = (s.x0 + s.x1) / 2.0;
        cols.iter()
            .position(|c| cx >= c.0 && cx <= c.1)
            .unwrap_or(0)
    };
    let mut grid = Vec::with_capacity(rows.len());
    let mut multi = 0;
    for r in &rows {
        let mut cells = vec![String::new(); cols.len()];
        for &i in &r.spans {
            let s = span(items, i);
            let cell = &mut cells[col_of(s)];
            if !cell.is_empty() {
                cell.push(' ');
            }
            cell.push_str(s.text.trim());
        }
        if cells.iter().filter(|c| !c.is_empty()).count() >= 2 {
            multi += 1;
        }
        grid.push(cells);
    }
    (multi * 10 >= rows.len() * 7).then_some(grid)
}

struct Line {
    text: String,
    x0: f32,
    x1: f32,
    cy: f32,
    size: f32,
}

enum Piece {
    Line(Line),
    Fig(usize),
}

fn join_spans(items: &[Item], spans: &[usize]) -> Line {
    let mut text = String::new();
    let mut prev_x1 = None;
    let (mut x0, mut x1, mut cy_sum, mut size) = (f32::MAX, f32::MIN, 0.0, 0.0f32);
    for &i in spans {
        let s = span(items, i);
        if let Some(px1) = prev_x1
            && s.x0 - px1 > 0.12 * s.size.max(1.0)
            && !text.ends_with(' ')
            && !s.text.starts_with(' ')
        {
            text.push(' ');
        }
        text.push_str(&s.text);
        prev_x1 = Some(s.x1);
        x0 = x0.min(s.x0);
        x1 = x1.max(s.x1);
        cy_sum += s.cy();
        size = size.max(s.size);
    }
    Line {
        text: text.trim().to_string(),
        x0,
        x1,
        cy: cy_sum / spans.len().max(1) as f32,
        size,
    }
}

fn leaf(items: &[Item], idx: Vec<usize>, body: f32, out: &mut Vec<Out>) {
    let mut order = idx;
    order.sort_by(|&a, &b| {
        let (ay, by) = (bbox(&items[a]), bbox(&items[b]));
        ((ay.2 + ay.3) / 2.0).total_cmp(&((by.2 + by.3) / 2.0))
    });
    let mut pieces: Vec<Piece> = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, pieces: &mut Vec<Piece>| {
        if run.is_empty() {
            return;
        }
        for r in rows_of(items, run, 0.5 * body) {
            let line = join_spans(items, &r.spans);
            if !line.text.is_empty() {
                pieces.push(Piece::Line(line));
            }
        }
        run.clear();
    };
    for i in order {
        match &items[i] {
            Item::Fig(f) => {
                flush(&mut run, &mut pieces);
                pieces.push(Piece::Fig(f.index));
            }
            Item::Text(_) => run.push(i),
        }
    }
    flush(&mut run, &mut pieces);
    paragraphs(pieces, body, out);
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .ends_with(['.', '!', '?', ':', '"', '\u{201d}', ')'])
}

/// A list marker at the start of a line, normalised, and the text after it.
pub fn list_marker(text: &str) -> Option<(String, &str)> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if "\u{2022}\u{25aa}\u{25e6}\u{2023}\u{25cf}\u{25cb}\u{25a0}\u{25a1}\u{2013}\u{2014}\u{b7}*-"
        .contains(first)
    {
        let rest = chars.as_str();
        if rest.starts_with(' ') || (first != '-' && first != '*' && !rest.is_empty()) {
            let rest = rest.trim_start();
            return (!rest.is_empty()).then(|| ("-".to_string(), rest));
        }
        return None;
    }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let bytes = text.as_bytes();
    if (1..=3).contains(&digits)
        && matches!(bytes.get(digits), Some(b'.' | b')'))
        && bytes.get(digits + 1) == Some(&b' ')
    {
        return Some((
            format!("{}.", &text[..digits]),
            text[digits + 2..].trim_start(),
        ));
    }
    if first.is_ascii_lowercase() && bytes.get(1) == Some(&b')') && bytes.get(2) == Some(&b' ') {
        return Some((format!("{first})"), text[3..].trim_start()));
    }
    None
}

fn paragraphs(pieces: Vec<Piece>, body: f32, out: &mut Vec<Out>) {
    let lines: Vec<&Line> = pieces
        .iter()
        .filter_map(|p| {
            if let Piece::Line(l) = p {
                Some(l)
            } else {
                None
            }
        })
        .collect();
    let left = lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
    let right = lines.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
    let mut deltas: Vec<f32> = Vec::new();
    let mut prev: Option<&Line> = None;
    for p in &pieces {
        match p {
            Piece::Line(l) => {
                if let Some(pl) = prev
                    && (pl.size / l.size.max(0.1)).max(l.size / pl.size.max(0.1)) < 1.1
                {
                    deltas.push(l.cy - pl.cy);
                }
                prev = Some(l);
            }
            Piece::Fig(_) => prev = None,
        }
    }
    deltas.sort_by(f32::total_cmp);
    let reference = if deltas.is_empty() {
        1.3 * body
    } else {
        deltas[deltas.len() / 4].clamp(0.95 * body, 2.5 * body)
    };

    let mut cur: Vec<&Line> = Vec::new();
    for p in &pieces {
        match p {
            Piece::Fig(i) => {
                emit(&mut cur, body, left, out);
                out.push(Out::Fig(*i));
            }
            Piece::Line(l) => {
                if let Some(pl) = cur.last()
                    && breaks(pl, l, reference, left, right)
                {
                    emit(&mut cur, body, left, out);
                }
                cur.push(l);
            }
        }
    }
    emit(&mut cur, body, left, out);
}

fn breaks(prev: &Line, cur: &Line, reference: f32, left: f32, right: f32) -> bool {
    let ratio = (prev.size / cur.size.max(0.1)).max(cur.size / prev.size.max(0.1));
    let size = prev.size.max(cur.size);
    if ratio > 1.15 || cur.cy - prev.cy > reference * 1.35 {
        return true;
    }
    if list_marker(&cur.text).is_some() {
        return true;
    }
    if list_marker(&prev.text).is_some() && cur.x0 < prev.x0 - 0.5 * size {
        return true;
    }
    let indented = cur.x0 > left + 0.8 * size && prev.x0 <= left + 0.3 * size;
    (indented && ends_sentence(&prev.text))
        || (prev.x1 < right - 5.0 * size && ends_sentence(&prev.text))
}

fn join_lines(texts: &[&str]) -> String {
    let mut text = String::new();
    for t in texts {
        if text.is_empty() {
            text.push_str(t);
            continue;
        }
        let hyphenated = text.ends_with('-')
            && text.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && t.chars().next().is_some_and(char::is_lowercase);
        if hyphenated {
            text.pop();
        } else {
            text.push(' ');
        }
        text.push_str(t);
    }
    text
}

fn heading_level(ratio: f32) -> u8 {
    match ratio {
        r if r >= 1.9 => 1,
        r if r >= 1.5 => 2,
        r if r >= 1.25 => 3,
        _ => 4,
    }
}

fn emit(cur: &mut Vec<&Line>, body: f32, left: f32, out: &mut Vec<Out>) {
    if cur.is_empty() {
        return;
    }
    let size = cur.iter().map(|l| l.size).fold(0.0f32, f32::max);
    let mut texts: Vec<&str> = cur.iter().map(|l| l.text.as_str()).collect();
    let text = crate::md::esc(&join_lines(&texts));
    if let Some((marker, rest)) = list_marker(texts[0]) {
        texts[0] = rest;
        let depth = (((cur[0].x0 - left) / (2.0 * body)).round() as i32).clamp(0, 4) as u8;
        out.push(Out::Block(Block::Item {
            depth,
            marker,
            text: crate::md::esc(&join_lines(&texts)),
        }));
    } else if size >= body * 1.12 && text.chars().count() <= 200 {
        out.push(Out::Block(Block::Heading(heading_level(size / body), text)));
    } else {
        out.push(Out::Block(Block::Para(text)));
    }
    cur.clear();
}

/// Pairs each figure with a neighbouring "Figure N ..." paragraph, removing that
/// paragraph from the flow; returns the caption per figure index.
pub fn take_captions(out: &mut Vec<Out>, figures: usize) -> Vec<String> {
    let mut captions = vec![String::new(); figures];
    let mut k = 0;
    while k < out.len() {
        if let Out::Fig(i) = out[k] {
            let next = out.get(k + 1).and_then(para_text);
            let prev = k
                .checked_sub(1)
                .and_then(|p| out.get(p))
                .and_then(para_text);
            if let Some(t) = next.filter(|t| crate::md::caption_start(t)) {
                captions[i] = crate::md::unescape(&t);
                out.remove(k + 1);
            } else if let Some(t) = prev.filter(|t| crate::md::caption_start(t)) {
                captions[i] = crate::md::unescape(&t);
                out.remove(k - 1);
                k -= 1;
            }
        }
        k += 1;
    }
    captions
}

fn para_text(o: &Out) -> Option<String> {
    match o {
        Out::Block(Block::Para(t)) => Some(t.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(text: &str, x0: f32, top: f32, size: f32) -> Item {
        Item::Text(Span {
            text: text.into(),
            x0,
            x1: x0 + text.chars().count() as f32 * size * 0.5,
            top,
            bottom: top + size,
            size,
        })
    }

    fn render(out: Vec<Out>) -> String {
        let blocks: Vec<Block> = out
            .into_iter()
            .filter_map(|o| if let Out::Block(b) = o { Some(b) } else { None })
            .collect();
        crate::md::render(&blocks)
    }

    #[test]
    fn two_columns_read_left_then_right() {
        let mut items = Vec::new();
        for i in 0..6 {
            let y = 100.0 + i as f32 * 14.0;
            items.push(sp(
                &format!("left column line number {i} here"),
                50.0,
                y,
                10.0,
            ));
            items.push(sp(
                &format!("right column line number {i} here"),
                330.0,
                y,
                10.0,
            ));
        }
        let text = render(layout(&items));
        let left_last = text.find("left column line number 5").unwrap();
        let right_first = text.find("right column line number 0").unwrap();
        assert!(left_last < right_first, "{text}");
    }

    #[test]
    fn large_text_becomes_a_heading_and_gaps_split_paragraphs() {
        let items = vec![
            sp("Annual Report", 50.0, 50.0, 24.0),
            sp("First paragraph line one of text.", 50.0, 100.0, 10.0),
            sp("continues on the second line.", 50.0, 112.0, 10.0),
            sp("Second paragraph starts here.", 50.0, 140.0, 10.0),
        ];
        let text = render(layout(&items));
        assert_eq!(
            text,
            "# Annual Report\n\nFirst paragraph line one of text. continues on the second line.\n\nSecond paragraph starts here."
        );
    }

    #[test]
    fn aligned_short_cells_become_a_table() {
        let mut items = Vec::new();
        for (r, row) in [
            ["Item", "Qty", "Price"],
            ["Apple", "3", "1.20"],
            ["Pear", "5", "2.00"],
        ]
        .iter()
        .enumerate()
        {
            for (c, cell) in row.iter().enumerate() {
                items.push(sp(
                    cell,
                    50.0 + c as f32 * 120.0,
                    100.0 + r as f32 * 14.0,
                    10.0,
                ));
            }
        }
        assert_eq!(
            render(layout(&items)),
            "| Item | Qty | Price |\n| --- | --- | --- |\n| Apple | 3 | 1.20 |\n| Pear | 5 | 2.00 |"
        );
    }

    #[test]
    fn lists_and_hyphenation() {
        let items = vec![
            sp("- first point", 50.0, 100.0, 10.0),
            sp("\u{2022} second point", 50.0, 112.0, 10.0),
            sp("Some hyphen-", 50.0, 150.0, 10.0),
            sp("ated text", 50.0, 162.0, 10.0),
        ];
        assert_eq!(
            render(layout(&items)),
            "- first point\n- second point\n\nSome hyphenated text"
        );
    }

    #[test]
    fn figure_takes_its_caption_paragraph() {
        let items = vec![
            sp("Body text before the figure.", 50.0, 50.0, 10.0),
            Item::Fig(FigBox {
                x0: 50.0,
                x1: 300.0,
                top: 100.0,
                bottom: 250.0,
                index: 0,
            }),
            sp("Figure 3. Revenue by quarter", 50.0, 262.0, 10.0),
        ];
        let mut out = layout(&items);
        let captions = take_captions(&mut out, 1);
        assert_eq!(captions, vec!["Figure 3. Revenue by quarter".to_string()]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_table_next_to_a_figure_is_still_a_table() {
        let mut items = Vec::new();
        for (r, row) in [
            ["Region", "Revenue", "Growth"],
            ["North", "42", "12%"],
            ["South", "37", "8%"],
        ]
        .iter()
        .enumerate()
        {
            for (c, cell) in row.iter().enumerate() {
                items.push(sp(
                    cell,
                    50.0 + c as f32 * 120.0,
                    100.0 + r as f32 * 16.0,
                    11.0,
                ));
            }
        }
        items.push(Item::Fig(FigBox {
            x0: 50.0,
            x1: 300.0,
            top: 146.0,
            bottom: 296.0,
            index: 0,
        }));
        let out = layout(&items);
        assert!(
            matches!(out[0], Out::Block(Block::Table(_))) && matches!(out[1], Out::Fig(0)),
            "{}",
            out.len()
        );
    }

    #[test]
    fn a_heading_above_a_table_does_not_glue_its_columns() {
        let mut items = vec![sp("Results by region", 56.0, 60.0, 15.0)];
        for (r, row) in [
            ["Region", "Revenue", "Growth"],
            ["North", "42", "12%"],
            ["South", "37", "8%"],
        ]
        .iter()
        .enumerate()
        {
            for (c, cell) in row.iter().enumerate() {
                items.push(sp(
                    cell,
                    56.0 + c as f32 * 120.0,
                    103.0 + r as f32 * 16.0,
                    11.0,
                ));
            }
        }
        let text = render(layout(&items));
        assert_eq!(
            text,
            "### Results by region\n\n| Region | Revenue | Growth |\n| --- | --- | --- |\n| North | 42 | 12% |\n| South | 37 | 8% |"
        );
    }

    #[test]
    fn numbered_headings_with_a_tab_gap_stay_on_one_line() {
        let items = vec![
            sp("1.", 50.0, 100.0, 10.0),
            sp("Introduction to the plugin", 80.0, 100.0, 10.0),
            sp("2.", 50.0, 114.0, 10.0),
            sp("Background and scope", 80.0, 114.0, 10.0),
        ];
        assert_eq!(
            render(layout(&items)),
            "1. Introduction to the plugin\n2. Background and scope"
        );
    }
}
