//! The OCR engine: the two embedded recognition models, loaded once and only
//! when a page or figure actually needs them, and returning laid-out lines.
use std::time::{Duration, Instant};

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

use crate::ctx::Clock;
use crate::pix::Pix;

static DETECTION: &[u8] = include_bytes!("../models/text-detection.rten");
static RECOGNITION: &[u8] = include_bytes!("../models/text-recognition.rten");

/// A gap between two words wider than this many line heights is a column gutter
/// or a cell gap, not a word space; the line is read as separate pieces so text
/// in neighbouring columns is never joined into one line.
const SPLIT_GAP: f32 = 0.9;

/// Splits every line where two neighbouring words are a gutter apart; `bounds`
/// gives a word's (left, right, height).
fn split_at_gutters<T>(lines: Vec<Vec<T>>, bounds: impl Fn(&T) -> (f32, f32, f32)) -> Vec<Vec<T>> {
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let height = line.iter().map(|w| bounds(w).2).fold(0.0, f32::max);
        let mut piece: Vec<T> = Vec::new();
        for word in line {
            let gap = piece.last().map(|prev| bounds(&word).0 - bounds(prev).1);
            if gap.is_some_and(|g| g > SPLIT_GAP * height) {
                out.push(std::mem::take(&mut piece));
            }
            piece.push(word);
        }
        if !piece.is_empty() {
            out.push(piece);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// What a read dropped, which the lines alone no longer show.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Lines recognized but dropped as noise (fewer than two letters or digits).
    pub dropped: usize,
}

/// Needs at least this many words before the fragment share is judged.
const MIN_WORDS: usize = 8;
/// More than this share of 1 or 2 character words reads as broken recognition
/// (running English prose is roughly a quarter to a third short words).
const MAX_FRAGMENT_SHARE: f32 = 0.6;
/// Fewer letters and digits than this share of the visible characters reads as symbol noise.
const MIN_ALNUM_SHARE: f32 = 0.6;
/// Needs at least this many detected lines before the noise-drop rate is judged.
const MIN_DETECTED: usize = 8;
/// More than this share of detected lines dropped as noise reads as a failed page.
const MAX_DROP_SHARE: f32 = 0.5;

/// Why a built-in read looks unreliable, or `None` when it looks usable. The
/// engine exposes no confidence, so this judges the output: nothing found on a
/// page that has ink (`inked`), mostly symbols, mostly 1 or 2 character
/// fragments, or most detected lines dropped as noise. It is a heuristic: it
/// catches garbled and empty reads, not a fluent misreading.
pub fn unreliable(lines: &[OcrLine], stats: Stats, inked: bool) -> Option<&'static str> {
    if lines.is_empty() {
        return inked.then_some("no text was found on an inked page");
    }
    let (mut alnum, mut visible, mut words, mut short) = (0usize, 0usize, 0usize, 0usize);
    for line in lines {
        for w in line.text.split_whitespace() {
            let n = w.chars().count();
            words += 1;
            short += usize::from(n <= 2);
            visible += n;
            alnum += w.chars().filter(|c| c.is_alphanumeric()).count();
        }
    }
    let detected = lines.len() + stats.dropped;
    if detected >= MIN_DETECTED && stats.dropped as f32 > MAX_DROP_SHARE * detected as f32 {
        return Some("most detected lines were noise");
    }
    if visible > 0 && (alnum as f32) < MIN_ALNUM_SHARE * visible as f32 {
        return Some("the text is mostly symbols");
    }
    if words >= MIN_WORDS && short as f32 > MAX_FRAGMENT_SHARE * words as f32 {
        return Some("the text is mostly fragments");
    }
    None
}

pub struct Ocr {
    engine: Option<OcrEngine>,
    clock: Clock,
}

impl Ocr {
    /// `limit` is the call's wall clock; OCR only starts another image when it
    /// is expected to finish inside it, so a partial result still returns.
    pub fn new(started: Instant, limit: Duration) -> Self {
        Self {
            engine: None,
            clock: Clock::new(started, limit),
        }
    }

    /// Whether another image is expected to finish inside the call's wall clock.
    pub fn has_time(&self) -> bool {
        self.clock.fits()
    }

    fn engine(&mut self) -> Result<&OcrEngine, String> {
        if self.engine.is_none() {
            let det = Model::load_static_slice(DETECTION)
                .map_err(|e| format!("cannot load the text detection model: {e}"))?;
            let rec = Model::load_static_slice(RECOGNITION)
                .map_err(|e| format!("cannot load the text recognition model: {e}"))?;
            let engine = OcrEngine::new(OcrEngineParams {
                detection_model: Some(det),
                recognition_model: Some(rec),
                ..Default::default()
            })
            .map_err(|e| format!("cannot start the OCR engine: {e}"))?;
            self.engine = Some(engine);
        }
        self.engine
            .as_ref()
            .ok_or_else(|| "the OCR engine is unavailable".to_string())
    }

    /// Reads the text lines of an image in reading order. Lines with fewer
    /// than two letters or digits are dropped as recognition noise.
    pub fn read(&mut self, pix: &Pix) -> Result<Vec<OcrLine>, String> {
        self.read_scored(pix).map(|(lines, _)| lines)
    }

    /// [`Ocr::read`] plus what the read dropped, for [`unreliable`].
    pub fn read_scored(&mut self, pix: &Pix) -> Result<(Vec<OcrLine>, Stats), String> {
        let began = Instant::now();
        let lines = self.read_lines(pix);
        self.clock.record(began.elapsed());
        lines
    }

    fn read_lines(&mut self, pix: &Pix) -> Result<(Vec<OcrLine>, Stats), String> {
        let engine = self.engine()?;
        let luma = pix.luma();
        let source = ImageSource::from_bytes(&luma, (pix.w, pix.h))
            .map_err(|e| format!("cannot prepare the image for OCR: {e}"))?;
        let input = engine
            .prepare_input(source)
            .map_err(|e| format!("cannot prepare the image for OCR: {e}"))?;
        let words = engine
            .detect_words(&input)
            .map_err(|e| format!("text detection failed: {e}"))?;
        let lines = split_at_gutters(engine.find_text_lines(&input, &words), |w| {
            let xs = w.corners().map(|p| p.x);
            (
                xs.into_iter().fold(f32::MAX, f32::min),
                xs.into_iter().fold(f32::MIN, f32::max),
                w.height(),
            )
        });
        let recognized = engine
            .recognize_text(&input, &lines)
            .map_err(|e| format!("text recognition failed: {e}"))?;
        let mut stats = Stats::default();
        let lines = recognized
            .into_iter()
            .flatten()
            .filter_map(|line| {
                let text = line.to_string();
                if text.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
                    stats.dropped += 1;
                    return None;
                }
                let r = line.bounding_rect();
                Some(OcrLine {
                    text: text.trim().to_string(),
                    left: r.left() as f32,
                    top: r.top() as f32,
                    right: r.right() as f32,
                    bottom: r.bottom() as f32,
                })
            })
            .collect();
        Ok((lines, stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<OcrLine> {
        texts
            .iter()
            .map(|t| OcrLine {
                text: (*t).to_string(),
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            })
            .collect()
    }

    const PROSE: &[&str] = &[
        "The quick brown fox jumps over the lazy dog.",
        "Revenue grew steadily across every region in the year.",
    ];

    #[test]
    fn fluent_prose_is_usable() {
        assert_eq!(unreliable(&lines(PROSE), Stats::default(), true), None);
        // A few dropped lines among many good ones do not fail the page.
        assert_eq!(unreliable(&lines(PROSE), Stats { dropped: 2 }, true), None);
    }

    #[test]
    fn nothing_found_is_unreliable_only_on_an_inked_page() {
        assert!(unreliable(&[], Stats::default(), true).is_some());
        assert_eq!(unreliable(&[], Stats::default(), false), None);
    }

    #[test]
    fn symbol_noise_is_unreliable() {
        let l = lines(&["~~ ^^ ,, .. -- ;; ::", "ab ## // \\ || {{ }}"]);
        assert_eq!(
            unreliable(&l, Stats::default(), true),
            Some("the text is mostly symbols")
        );
    }

    #[test]
    fn mostly_short_fragments_are_unreliable() {
        let l = lines(&["ab c de f gh i jk l", "mn o pq r st u vw x"]);
        assert_eq!(
            unreliable(&l, Stats::default(), true),
            Some("the text is mostly fragments")
        );
        // Too few words to judge.
        assert_eq!(
            unreliable(&lines(&["ab c de"]), Stats::default(), true),
            None
        );
    }

    #[test]
    fn a_high_noise_drop_rate_is_unreliable() {
        let kept = lines(&["Some real words here", "and a few more words"]);
        assert_eq!(
            unreliable(&kept, Stats { dropped: 20 }, true),
            Some("most detected lines were noise")
        );
        assert_eq!(unreliable(&kept, Stats { dropped: 3 }, true), None);
    }
}
