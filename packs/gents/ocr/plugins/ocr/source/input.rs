//! The plugin's input: validation of every field at the trust boundary, the
//! page selection grammar and the bounds on each size.
use serde::Deserialize;

pub const DEFAULT_MAX_IMAGE_PX: u32 = 2000;
pub const DEFAULT_MIN_FIGURE_PX: u32 = 96;
const MAX_FILES: usize = 10_000;
const MAX_PAGE: u32 = 1_000_000;
const MAX_RANGES: usize = 1_000;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub path: Option<String>,
    pub files: Option<Vec<String>>,
    pub name: Option<String>,
    pub data_base64: Option<String>,
    pub pages: Option<String>,
    pub ocr: Option<OcrMode>,
    pub figure_images: Option<bool>,
    pub max_image_px: Option<u32>,
    pub min_figure_px: Option<u32>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum OcrMode {
    Auto,
    Always,
    Never,
}

/// Selected 1-based page, slide, sheet or section numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageSel(Vec<(u32, u32)>);

impl PageSel {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "pages {spec:?} is not a list like \"1-3,7\" or \"5-\" of numbers from 1 to {MAX_PAGE}"
            )
        };
        let mut ranges = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (a.trim(), Some(b.trim())),
                None => (part, None),
            };
            let lo: u32 = a.parse().map_err(|_| bad())?;
            let hi: u32 = match b {
                None => lo,
                Some("") => MAX_PAGE,
                Some(b) => b.parse().map_err(|_| bad())?,
            };
            if lo == 0 || hi < lo || hi > MAX_PAGE {
                return Err(bad());
            }
            ranges.push((lo, hi));
            if ranges.len() > MAX_RANGES {
                return Err(format!("pages has more than {MAX_RANGES} ranges"));
            }
        }
        Ok(Self(ranges))
    }

    pub fn contains(&self, n: u32) -> bool {
        self.0.iter().any(|&(lo, hi)| (lo..=hi).contains(&n))
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub pages: Option<PageSel>,
    pub ocr: OcrMode,
    pub figure_images: bool,
    pub max_image_px: u32,
    pub min_figure_px: u32,
}

impl Options {
    pub fn selected(&self, n: u32) -> bool {
        self.pages.as_ref().is_none_or(|p| p.contains(n))
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pages: None,
            ocr: OcrMode::Auto,
            figure_images: false,
            max_image_px: DEFAULT_MAX_IMAGE_PX,
            min_figure_px: DEFAULT_MIN_FIGURE_PX,
        }
    }
}

impl Input {
    pub fn options(&self) -> Result<Options, String> {
        let max_image_px = self.max_image_px.unwrap_or(DEFAULT_MAX_IMAGE_PX);
        if !(256..=4096).contains(&max_image_px) {
            return Err(format!(
                "max_image_px {max_image_px} must be between 256 and 4096"
            ));
        }
        let min_figure_px = self.min_figure_px.unwrap_or(DEFAULT_MIN_FIGURE_PX);
        if !(1..=4096).contains(&min_figure_px) {
            return Err(format!(
                "min_figure_px {min_figure_px} must be between 1 and 4096"
            ));
        }
        let pages = match self.pages.as_deref() {
            Some(spec) => Some(PageSel::parse(spec)?),
            None => None,
        };
        Ok(Options {
            pages,
            ocr: self.ocr.unwrap_or(OcrMode::Auto),
            figure_images: self.figure_images.unwrap_or(false),
            max_image_px,
            min_figure_px,
        })
    }

    /// Checks the source fields: a path (with optional relative files) or an
    /// inline name with base64 data, never both.
    pub fn validate_source(&self) -> Result<(), String> {
        match (&self.path, &self.data_base64, &self.name) {
            (Some(_), None, None) => {}
            (None, Some(_), Some(name)) if !name.trim().is_empty() => {}
            (None, Some(_), _) => {
                return Err(
                    "data_base64 needs a name with the file extension, such as \"report.pdf\""
                        .into(),
                );
            }
            (None, None, _) => {
                return Err(
                    "give a path (a bound file or directory) or a name with data_base64".into(),
                );
            }
            _ => return Err("give either path or name with data_base64, not both".into()),
        }
        if let Some(files) = &self.files {
            if self.path.is_none() {
                return Err("files only applies together with path".into());
            }
            if files.is_empty() || files.len() > MAX_FILES {
                return Err(format!(
                    "files must list between 1 and {MAX_FILES} relative paths"
                ));
            }
            for f in files {
                let p = std::path::Path::new(f);
                let escapes = p.is_absolute()
                    || p.components()
                        .any(|c| matches!(c, std::path::Component::ParentDir));
                if f.is_empty() || escapes {
                    return Err(format!(
                        "files entry {f:?} must be a relative path inside the bound directory"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges_parse_and_match() {
        let sel = PageSel::parse("1-3, 7,10-").unwrap();
        assert!(sel.contains(1) && sel.contains(3) && sel.contains(7) && sel.contains(500));
        assert!(!sel.contains(4) && !sel.contains(8) && !sel.contains(9));
    }

    #[test]
    fn bad_page_specs_are_rejected() {
        for bad in ["", "0", "3-1", "a", "1,,2", "-4", "2000000"] {
            assert!(PageSel::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn source_fields_are_checked() {
        let parse = |json: &str| serde_json::from_str::<Input>(json).unwrap();
        assert!(parse(r#"{"path":"/x"}"#).validate_source().is_ok());
        assert!(
            parse(r#"{"name":"a.pdf","data_base64":"AA=="}"#)
                .validate_source()
                .is_ok()
        );
        assert!(
            parse(r#"{"data_base64":"AA=="}"#)
                .validate_source()
                .is_err()
        );
        assert!(parse(r#"{}"#).validate_source().is_err());
        assert!(
            parse(r#"{"path":"/x","files":["../a.pdf"]}"#)
                .validate_source()
                .is_err()
        );
        assert!(
            parse(r#"{"path":"/x","files":["/etc/a.pdf"]}"#)
                .validate_source()
                .is_err()
        );
        assert!(
            parse(r#"{"path":"/x","files":["sub/a.pdf"]}"#)
                .validate_source()
                .is_ok()
        );
        assert!(
            parse(r#"{"path":"/x","max_image_px":10}"#)
                .options()
                .is_err()
        );
    }
}
