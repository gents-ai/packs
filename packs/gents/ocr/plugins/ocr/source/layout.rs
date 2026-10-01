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
/// Median cell length of a table found among the rows of a whole region, where
/// text columns are still possible: short cells only.
const REGION_CELL_CHARS: usize = 22;
/// The same for a region no column gutter cuts, where longer cells are safe.
const LEAF_CELL_CHARS: usize = 40;

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
        // A figure belongs to its column: columns are cut first, so a figure in
        // the left column stays in the flow of the left column, not after the
        // right one. Not when the text around it is a table, which fcut frees.
        if idx.iter().any(|&i| matches!(items[i], Item::Fig(_))) {
            let text: Vec<usize> = idx
                .iter()
                .copied()
                .filter(|&i| matches!(items[i], Item::Text(_)))
                .collect();
            if table(items, &text, body).is_none()
                && let Some(groups) = vcut(items, &idx, body)
            {
                for g in groups {
                    region(items, g, body, out, depth + 1);
                }
                return;
            }
        }
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
        if let Some(parts) = tcut(items, &idx, body) {
            for part in parts {
                match part {
                    Part::Items(g) => region(items, g, body, out, depth + 1),
                    Part::Table(grid) => out.push(Out::Block(Block::Table(grid))),
                }
            }
            return;
        }
        if let Some(groups) = [
            vcut(items, &idx, body),
            scut(items, &idx, body),
            hcut(items, &idx, body),
        ]
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

enum Part {
    Items(Vec<usize>),
    Table(Vec<Vec<String>>),
}

/// Cuts tables out of a region: runs of rows whose short cells line up in
/// columns, wherever they sit among other text. What is above and below stays
/// in the flow. `None` when the region holds no such run.
fn tcut(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Part>> {
    if idx.len() < 6 || idx.iter().any(|&i| matches!(items[i], Item::Fig(_))) {
        return None;
    }
    let rows = rows_of(items, idx, 0.5 * body);
    let mut parts = Vec::new();
    let mut pending: Vec<usize> = Vec::new();
    let mut found = false;
    let mut k = 0;
    while k < rows.len() {
        if rows[k].spans.len() >= 2 {
            let end = k + rows[k..].iter().take_while(|r| r.spans.len() >= 2).count();
            if let Some((lo, hi, grid)) = table_run(items, &rows[k..end], body, REGION_CELL_CHARS) {
                pending.extend(rows[k..k + lo].iter().flat_map(|r| r.spans.iter().copied()));
                if !pending.is_empty() {
                    parts.push(Part::Items(std::mem::take(&mut pending)));
                }
                parts.push(Part::Table(grid));
                found = true;
                pending.extend(
                    rows[end - hi..end]
                        .iter()
                        .flat_map(|r| r.spans.iter().copied()),
                );
                k = end;
                continue;
            }
        }
        pending.extend(rows[k].spans.iter().copied());
        k += 1;
    }
    if !found {
        return None;
    }
    if !pending.is_empty() {
        parts.push(Part::Items(pending));
    }
    Some(parts)
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
    let groups: Vec<Vec<usize>> = merged.into_iter().map(|g| g.0).collect();
    // Columns sit side by side: items of one share lines with items of the next.
    // Text that only differs in x (a centred caption under a paragraph) is not.
    let side_by_side = groups.windows(2).all(|w| shares_lines(items, &w[0], &w[1]));
    (groups.len() > 1 && side_by_side).then_some(groups)
}

/// Whether two groups of items sit side by side: their vertical extents
/// overlap by at least half of the shorter one. Two lines that only differ in x
/// (a centred caption under a paragraph) do not overlap at all.
fn shares_lines(items: &[Item], a: &[usize], b: &[usize]) -> bool {
    let extent = |g: &[usize]| {
        g.iter().fold((f32::MAX, f32::MIN), |(t, bt), &i| {
            let (_, _, top, bottom) = bbox(&items[i]);
            (t.min(top), bt.max(bottom))
        })
    };
    let ((t1, b1), (t2, b2)) = (extent(a), extent(b));
    (b1.min(b2) - t1.max(t2)) >= 0.5 * (b1 - t1).min(b2 - t2)
}

/// Cuts a page whose columns are bridged by a few wide items, such as a title
/// or an abstract across both columns: those items become bands, read top to
/// bottom, and the text between them is cut into columns on the next round.
fn scut(items: &[Item], idx: &[usize], body: f32) -> Option<Vec<Vec<usize>>> {
    let n = idx.len();
    if n < 8 {
        return None;
    }
    let mut events: Vec<(f32, i32)> = Vec::with_capacity(2 * n);
    for &i in idx {
        let (x0, x1, ..) = bbox(&items[i]);
        events.push((x0, 1));
        events.push((x1, -1));
    }
    // Closings first at equal x, so touching items do not cover each other.
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let tolerated = (n / 12).max(1) as i32;
    let min_gap = 1.2 * body;
    let mut best: Option<(f32, f32)> = None;
    let mut cover = 0;
    let mut start: Option<f32> = None;
    for (x, d) in events {
        cover += d;
        if cover <= tolerated {
            if start.is_none() && d < 0 {
                start = Some(x);
            }
        } else if let Some(st) = start.take()
            && x - st >= min_gap
            && best.is_none_or(|(a, b)| x - st > b - a)
        {
            best = Some((st, x));
        }
    }
    let (a, b) = best?;
    let crosses = |i: usize| {
        let (x0, x1, ..) = bbox(&items[i]);
        x0 < b && x1 > a
    };
    let (mut wide, narrow): (Vec<usize>, Vec<usize>) = idx.iter().partition(|&&i| crosses(i));
    let left = narrow
        .iter()
        .filter(|&&i| bbox(&items[i]).1 <= a + 0.01)
        .count();
    let right = narrow
        .iter()
        .filter(|&&i| bbox(&items[i]).0 >= b - 0.01)
        .count();
    if wide.is_empty() || left < 3 || right < 3 {
        return None;
    }
    wide.sort_by(|&p, &q| bbox(&items[p]).2.total_cmp(&bbox(&items[q]).2));
    // Wide items that touch in height are one band.
    let mut bands: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for &i in &wide {
        let (_, _, top, bottom) = bbox(&items[i]);
        match bands.last_mut() {
            Some(band) if top <= band.1 + 0.5 * body => {
                band.1 = band.1.max(bottom);
                band.2.push(i);
            }
            _ => bands.push((top, bottom, vec![i])),
        }
    }
    let mut slabs: Vec<Vec<usize>> = vec![Vec::new(); bands.len() + 1];
    for &i in &narrow {
        let (_, _, top, bottom) = bbox(&items[i]);
        let cy = (top + bottom) / 2.0;
        slabs[bands.iter().take_while(|band| cy > band.1).count()].push(i);
    }
    let mut groups = Vec::new();
    for (k, slab) in slabs.into_iter().enumerate() {
        if !slab.is_empty() {
            groups.push(slab);
        }
        if let Some(band) = bands.get(k) {
            groups.push(band.2.clone());
        }
    }
    (groups.len() > 1).then_some(groups)
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

/// A run of consecutive rows whose cells line up in columns is a table, however
/// narrow the gaps between the cells. Up to two rows may be dropped from either
/// end (a caption or prose line that also has two spans); returns how many were
/// dropped at the start and at the end, and the grid.
fn table_run(
    items: &[Item],
    rows: &[Row],
    body: f32,
    max_len: usize,
) -> Option<(usize, usize, Vec<Vec<String>>)> {
    for trim in 0..=4usize {
        for lo in 0..=trim.min(2) {
            let hi = trim - lo;
            if hi > 2 || rows.len() < lo + hi + 3 {
                continue;
            }
            if let Some(grid) = grid_of(items, &rows[lo..rows.len() - hi], body, max_len) {
                return Some((lo, hi, grid));
            }
        }
    }
    None
}

/// Columns are the gaps left in the horizontal projection of every cell in the
/// rows; each must hold aligned cells (left, right or centre) in two rows.
fn grid_of(items: &[Item], rows: &[Row], body: f32, max_len: usize) -> Option<Vec<Vec<String>>> {
    let mut spans: Vec<usize> = rows.iter().flat_map(|r| r.spans.iter().copied()).collect();
    spans.sort_by(|&a, &b| span(items, a).x0.total_cmp(&span(items, b).x0));
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for &i in &spans {
        let s = span(items, i);
        match cols.last_mut() {
            Some(c) if s.x0 - c.1 < 0.5 * body => c.1 = c.1.max(s.x1),
            _ => cols.push((s.x0, s.x1)),
        }
    }
    if cols.len() < 2 || cols.len() > 30 {
        return None;
    }
    let col_of = |s: &Span| {
        cols.iter()
            .position(|c| s.x0 <= c.1)
            .unwrap_or(cols.len() - 1)
    };
    let tol = 0.6 * body;
    let mut members: Vec<Vec<&Span>> = vec![Vec::new(); cols.len()];
    for &i in &spans {
        let s = span(items, i);
        members[col_of(s)].push(s);
    }
    let aligned = |m: &[&Span]| {
        let fraction = |key: fn(&Span) -> f32| {
            let mut v: Vec<f32> = m.iter().map(|s| key(s)).collect();
            v.sort_by(f32::total_cmp);
            let mid = v[v.len() / 2];
            v.iter().filter(|x| (**x - mid).abs() <= tol).count() as f32 / v.len() as f32
        };
        [
            fraction(|s| s.x0),
            fraction(|s| s.x1),
            fraction(|s| (s.x0 + s.x1) / 2.0),
        ]
        .into_iter()
        .fold(0.0, f32::max)
            >= 0.6
    };
    if members.iter().any(|m| m.len() < 2 || !aligned(m)) {
        return None;
    }
    let mut lens: Vec<usize> = spans
        .iter()
        .map(|&i| span(items, i).text.chars().count())
        .collect();
    lens.sort_unstable();
    if lens[lens.len() / 2] > max_len {
        return None;
    }
    let mut grid = Vec::with_capacity(rows.len());
    let mut multi = 0;
    for r in rows {
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
    Table(Vec<Vec<String>>),
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
        let rows = rows_of(items, run, 0.5 * body);
        let push_line = |r: &Row, pieces: &mut Vec<Piece>| {
            let line = join_spans(items, &r.spans);
            if !line.text.is_empty() {
                pieces.push(Piece::Line(line));
            }
        };
        let mut k = 0;
        while k < rows.len() {
            if rows[k].spans.len() >= 2 {
                let end = k + rows[k..].iter().take_while(|r| r.spans.len() >= 2).count();
                if let Some((lo, hi, grid)) = table_run(items, &rows[k..end], body, LEAF_CELL_CHARS)
                {
                    rows[k..k + lo].iter().for_each(|r| push_line(r, pieces));
                    pieces.push(Piece::Table(grid));
                    rows[end - hi..end]
                        .iter()
                        .for_each(|r| push_line(r, pieces));
                    k = end;
                    continue;
                }
            }
            push_line(&rows[k], pieces);
            k += 1;
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

/// Whether a line opens a figure caption that sits next to a figure: directly
/// before or after one, or at the edge of its region, which is where the cut
/// around a figure leaves its caption.
fn caption_lines(pieces: &[Piece]) -> Vec<bool> {
    let last = pieces.len().saturating_sub(1);
    pieces
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let Piece::Line(l) = p else { return false };
            let edge = |o: Option<&Piece>| !matches!(o, Some(Piece::Line(_)));
            crate::md::caption_start(&l.text)
                && (k == 0
                    || k == last
                    || edge(k.checked_sub(1).and_then(|p| pieces.get(p)))
                    || edge(pieces.get(k + 1)))
        })
        .collect()
}

/// A caption goes on only while the next line does not start a new sentence
/// (it begins with a lowercase letter or a digit); otherwise the caption is over
/// and the line is body text. A wrapped caption whose second line starts with a
/// capital is cut there: the body text after a caption is never swallowed.
fn caption_continues(cur: &Line) -> bool {
    cur.text
        .chars()
        .next()
        .is_some_and(|c| c.is_lowercase() || c.is_ascii_digit())
}

fn paragraphs(pieces: Vec<Piece>, body: f32, out: &mut Vec<Out>) {
    let cap = caption_lines(&pieces);
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
            Piece::Fig(_) | Piece::Table(_) => prev = None,
        }
    }
    deltas.sort_by(f32::total_cmp);
    let reference = if deltas.is_empty() {
        1.3 * body
    } else {
        deltas[deltas.len() / 4].clamp(0.95 * body, 2.5 * body)
    };

    let mut cur: Vec<&Line> = Vec::new();
    let mut in_caption = false;
    for (p, &starts_caption) in pieces.iter().zip(&cap) {
        match p {
            Piece::Fig(i) => {
                emit(&mut cur, body, left, out);
                in_caption = false;
                out.push(Out::Fig(*i));
            }
            Piece::Table(grid) => {
                emit(&mut cur, body, left, out);
                in_caption = false;
                out.push(Out::Block(Block::Table(grid.clone())));
            }
            Piece::Line(l) => {
                if let Some(pl) = cur.last()
                    && (breaks(pl, l, reference, left, right)
                        || starts_caption
                        || (in_caption && !caption_continues(l)))
                {
                    emit(&mut cur, body, left, out);
                    in_caption = false;
                }
                in_caption |= starts_caption;
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
            let is_caption = |t: &String| crate::md::caption_start(t) && t.chars().count() <= 300;
            if let Some(t) = next.filter(is_caption) {
                captions[i] = crate::md::unescape(&t);
                out.remove(k + 1);
            } else if let Some(t) = prev.filter(is_caption) {
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
#[path = "layout_tests.rs"]
mod tests;
