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
        let began = Instant::now();
        let lines = self.read_lines(pix);
        self.clock.record(began.elapsed());
        lines
    }

    fn read_lines(&mut self, pix: &Pix) -> Result<Vec<OcrLine>, String> {
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
        Ok(recognized
            .into_iter()
            .flatten()
            .filter_map(|line| {
                let text = line.to_string();
                if text.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
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
            .collect())
    }
}
