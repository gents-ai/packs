//! Streaming Markdown tables for CSV and spreadsheets: rows are written as they
//! arrive and writing stops at the output budget, so a huge sheet never has to
//! be held as rows.
use crate::md::cell;
use crate::model::json_len;

pub struct TableWriter {
    md: String,
    /// JSON bytes of `md` so far, kept incrementally so a row costs O(row).
    json: usize,
    cols: usize,
    pub rows: usize,
    budget: usize,
    pub full: bool,
    /// Whether the first row is a header followed by the rule; a table that
    /// continues an earlier call writes plain rows.
    header: bool,
}

impl TableWriter {
    /// `budget` is the JSON bytes this table may use.
    pub fn new(cols: usize, budget: usize, header: bool) -> Self {
        Self {
            md: String::new(),
            json: 2,
            cols: cols.max(1),
            rows: 0,
            budget,
            full: false,
            header,
        }
    }

    /// Forgets that the budget ran out, after a row too large for any budget was skipped.
    pub fn reset_full(&mut self) {
        self.full = false;
    }

    /// Adds a row; returns false once the budget is used up (the row is dropped).
    pub fn row<S: AsRef<str>>(&mut self, cells: &[S]) -> bool {
        if self.full {
            return false;
        }
        let mut line = String::from("|");
        for c in 0..self.cols {
            line.push(' ');
            line.push_str(&cell(cells.get(c).map_or("", AsRef::as_ref)));
            line.push_str(" |");
        }
        line.push('\n');
        if self.rows == 0 && self.header {
            line.push('|');
            line.push_str(&" --- |".repeat(self.cols));
            line.push('\n');
        }
        let cost = json_len(&line) - 2;
        if self.json + cost + 64 > self.budget {
            self.full = true;
            return false;
        }
        self.md.push_str(&line);
        self.json += cost;
        self.rows += 1;
        true
    }

    /// The header and rule lines a table with these columns opens with.
    pub fn header_of<S: AsRef<str>>(cols: usize, first: &[S]) -> String {
        let mut h = Self::new(cols, usize::MAX, true);
        h.row(first);
        h.finish()
    }

    pub fn finish(mut self) -> String {
        self.md.truncate(self.md.trim_end().len());
        self.md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_rule_and_stops_at_budget() {
        let mut t = TableWriter::new(2, 10_000, true);
        assert!(t.row(&["a", "b"]) && t.row(&["1", "2|3"]));
        assert_eq!(t.finish(), "| a | b |\n| --- | --- |\n| 1 | 2\\|3 |");
        let mut small = TableWriter::new(2, 120, true);
        let mut written = 0;
        while small.row(&["some text", "more text"]) {
            written += 1;
        }
        assert!(small.full && (1..5).contains(&written));
    }
}
