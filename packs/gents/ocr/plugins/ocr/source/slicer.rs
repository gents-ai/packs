//! The loop every converter shares: ask the reader for the next unit of
//! Markdown, offer it to the output budget, and stop with a continuation when
//! the budget or the wall clock says the call has done enough.
use serde_json::Value;

use crate::ctx::{Ctx, Emit};
use crate::model::DocAcc;
use crate::resume::Resume;

/// Where a reader stands: what [`Steps::step`] would read next.
pub struct Snap {
    pub unit: u32,
    pub pos: u64,
    pub st: Option<Value>,
}

/// What one step of a reader produced.
pub enum Step {
    /// The next unit of Markdown.
    Chunk(String),
    /// The last unit this call can hold, and where the next call continues; the
    /// text already respects the budget (a table that stopped at its row limit).
    Stop(String, Snap, u8),
    /// The unit needs OCR and the wall clock has no room for it: leave it to the next call.
    Wait,
}

/// A reader that produces a document unit by unit.
pub trait Steps {
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc);
    fn snapshot(&self) -> Snap;
    /// The next unit, or `None` when the document is finished.
    fn step(&mut self) -> Result<Option<Step>, String>;
}

/// Runs a reader until it finishes or the call is full; `skip` is how much of
/// the first unit an earlier call already delivered. Returns the continuation
/// when the document is not finished.
pub fn run(steps: &mut dyn Steps, mut skip: u64) -> Result<Option<Resume>, String> {
    loop {
        let (ctx, acc) = steps.parts();
        if ctx.emitted > 0 && !ctx.clock.fits() {
            let snap = steps.snapshot();
            let (ctx, acc) = steps.parts();
            let mark = ctx.mark(acc);
            return Ok(Some(resume_from(ctx, acc, &mark, snap, 0, 2)));
        }
        let mark = ctx.mark(acc);
        let snap = steps.snapshot();
        let Some(step) = steps.step()? else {
            return Ok(None);
        };
        let (ctx, acc) = steps.parts();
        match step {
            Step::Wait => {
                ctx.rewind(acc, &mark);
                return Ok(Some(resume_from(ctx, acc, &mark, snap, 0, 2)));
            }
            Step::Chunk(chunk) => {
                match ctx.emit(acc, &mark, &chunk, take_skip(&mut skip, &chunk)) {
                    Emit::Added => {}
                    Emit::Deferred => return Ok(Some(resume_from(ctx, acc, &mark, snap, 0, 2))),
                    Emit::Cut { skip, joint } => {
                        return Ok(Some(resume_from(ctx, acc, &mark, snap, skip, joint)));
                    }
                }
            }
            Step::Stop(chunk, at, joint) => {
                match ctx.emit(acc, &mark, &chunk, take_skip(&mut skip, &chunk)) {
                    Emit::Added => {
                        let mut r = ctx.resume_now(acc, at.unit, at.pos);
                        r.st = at.st;
                        r.joint = joint;
                        return Ok(Some(r));
                    }
                    Emit::Deferred => return Ok(Some(resume_from(ctx, acc, &mark, snap, 0, 2))),
                    Emit::Cut { skip, joint } => {
                        return Ok(Some(resume_from(ctx, acc, &mark, snap, skip, joint)));
                    }
                }
            }
        }
    }
}

/// A marker comment followed by the unit's body, or the marker alone.
pub fn with_marker(marker: &str, body: &str) -> String {
    if body.is_empty() {
        marker.to_string()
    } else {
        format!("{marker}\n\n{body}")
    }
}

/// A reader over units numbered `1..=total` (slides, sheets, chapters), each
/// rendered whole by `make`; units the page selection leaves out are skipped.
pub struct Units<'a, F> {
    ctx: &'a mut Ctx,
    acc: &'a mut DocAcc,
    next: u32,
    total: u32,
    filter: bool,
    make: F,
}

impl<'a, F> Units<'a, F> {
    pub fn new(ctx: &'a mut Ctx, acc: &'a mut DocAcc, start: u32, total: u32, make: F) -> Self {
        Self {
            ctx,
            acc,
            next: start.max(1),
            total,
            filter: true,
            make,
        }
    }

    /// Reads every unit whatever the page selection says (a format with one unit and no pages).
    pub fn unfiltered(mut self) -> Self {
        self.filter = false;
        self
    }
}

impl<F> Steps for Units<'_, F>
where
    F: FnMut(&mut Ctx, &mut DocAcc, u32) -> Result<String, String>,
{
    fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
        (&mut *self.ctx, &mut *self.acc)
    }

    fn snapshot(&self) -> Snap {
        Snap {
            unit: self.next,
            pos: 0,
            st: None,
        }
    }

    fn step(&mut self) -> Result<Option<Step>, String> {
        while self.filter && self.next <= self.total && !self.ctx.opts.selected(self.next) {
            self.next += 1;
        }
        if self.next > self.total {
            return Ok(None);
        }
        let chunk = (self.make)(&mut *self.ctx, &mut *self.acc, self.next)?;
        if self.ctx.waiting {
            return Ok(Some(Step::Wait));
        }
        self.next += 1;
        Ok(Some(Step::Chunk(chunk)))
    }
}

/// The skip belongs to the first unit that has text: an empty one before it
/// (a body that ended, ahead of its footnotes) does not use it up.
fn take_skip(skip: &mut u64, chunk: &str) -> u64 {
    if chunk.is_empty() {
        0
    } else {
        std::mem::take(skip)
    }
}

/// The continuation that starts again at the unit `mark` and `snap` were taken before.
fn resume_from(
    ctx: &Ctx,
    acc: &DocAcc,
    mark: &crate::ctx::Mark,
    snap: Snap,
    skip: u64,
    joint: u8,
) -> Resume {
    let mut r = ctx.resume_at(acc, mark, snap.unit, snap.pos);
    r.st = snap.st;
    r.skip = skip;
    r.joint = joint;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Options;

    /// Units of fixed text; the third one is marked as needing OCR.
    struct Fake<'a> {
        ctx: &'a mut Ctx,
        acc: &'a mut DocAcc,
        n: u32,
        units: Vec<String>,
        wait_at: Option<u32>,
    }

    impl Steps for Fake<'_> {
        fn parts(&mut self) -> (&mut Ctx, &mut DocAcc) {
            (self.ctx, self.acc)
        }
        fn snapshot(&self) -> Snap {
            Snap {
                unit: self.n + 1,
                pos: 0,
                st: None,
            }
        }
        fn step(&mut self) -> Result<Option<Step>, String> {
            let Some(text) = self.units.get(self.n as usize).cloned() else {
                return Ok(None);
            };
            if self.wait_at == Some(self.n) {
                return Ok(Some(Step::Wait));
            }
            self.n += 1;
            Ok(Some(Step::Chunk(text)))
        }
    }

    fn opts(max_bytes: usize) -> Options {
        Options {
            max_bytes,
            ..Options::default()
        }
    }

    fn fake(
        units: Vec<String>,
        start: u32,
        max_bytes: usize,
        skip: u64,
    ) -> (String, Option<Resume>) {
        let mut ctx = Ctx::new(opts(max_bytes));
        let mut acc = DocAcc::default();
        let mut steps = Fake {
            ctx: &mut ctx,
            acc: &mut acc,
            n: start,
            units,
            wait_at: None,
        };
        let next = run(&mut steps, skip).unwrap();
        (acc.md, next)
    }

    #[test]
    fn units_that_fit_are_all_delivered_and_the_rest_waits_for_the_next_call() {
        let units: Vec<String> = (0..5)
            .map(|i| format!("unit {i} {}", "x".repeat(1500)))
            .collect();
        let (md, next) = fake(units.clone(), 0, 4096, 0);
        assert!(md.contains("unit 0") && md.contains("unit 1") && !md.contains("unit 2"));
        let next = next.expect("the call is full");
        assert_eq!(next.unit, 3);
        let (md2, next2) = fake(units, next.unit - 1, 4096, next.skip);
        assert!(md2.starts_with("unit 2") && next2.is_some());
    }

    #[test]
    fn a_unit_larger_than_a_whole_call_is_cut_and_continues_with_a_skip() {
        let big = (0..400)
            .map(|i| format!("line number {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut rest = big.clone();
        let mut got = String::new();
        let mut skip = 0u64;
        for _ in 0..50 {
            let (md, next) = fake(vec![big.clone()], 0, 4096, skip);
            if !got.is_empty() {
                got.push('\n');
            }
            got.push_str(&md);
            match next {
                Some(r) => {
                    assert_eq!(r.unit, 1);
                    skip = r.skip;
                    assert!(skip > 0 && r.joint == 1);
                }
                None => break,
            }
            rest.clear();
        }
        assert_eq!(got, big);
    }

    #[test]
    fn a_unit_waiting_for_ocr_time_is_left_for_the_next_call() {
        let mut ctx = Ctx::new(opts(1 << 20));
        let mut acc = DocAcc::default();
        let mut steps = Fake {
            ctx: &mut ctx,
            acc: &mut acc,
            n: 0,
            units: vec!["a".into(), "b".into(), "c".into()],
            wait_at: Some(1),
        };
        let next = run(&mut steps, 0).unwrap().expect("unit 2 waits");
        assert_eq!((next.unit, acc.md.as_str()), (2, "a"));
    }
}
