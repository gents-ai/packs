//! The OCR engine: the two embedded recognition models, loaded once and only
//! when a page or figure actually needs them, and returning laid-out lines.
use std::time::{Duration, Instant};

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

use crate::pix::Pix;

static DETECTION: &[u8] = include_bytes!("../models/text-detection.rten");
static RECOGNITION: &[u8] = include_bytes!("../models/text-recognition.rten");

/// The plugin's declared wall clock is 900 s; OCR only starts another image
/// when it is expected to finish by this point, so a partial result still returns.
const OCR_DEADLINE: Duration = Duration::from_secs(860);
/// The next image is assumed to take up to this many times the slowest so far.
const SLOWEST_FACTOR: f64 = 1.5;

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
    started: Instant,
    slowest: Duration,
}

impl Ocr {
    pub fn new(started: Instant) -> Self {
        Self {
            engine: None,
            started,
            slowest: Duration::ZERO,
        }
    }

    /// Whether another image is expected to finish inside the call's wall clock.
    pub fn has_time(&self) -> bool {
        self.started.elapsed() + self.slowest.mul_f64(SLOWEST_FACTOR) < OCR_DEADLINE
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
        self.slowest = self.slowest.max(began.elapsed());
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
        let lines = engine.find_text_lines(&input, &words);
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
