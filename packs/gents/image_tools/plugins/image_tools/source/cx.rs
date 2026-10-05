//! The state of one call: the image parts attached so far against the output
//! budget, the folder files are written into, and the clock. Every image a
//! step produces goes through [`Cx::deliver`], which writes the file, attaches
//! the part and reports one record for it.
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::input::Output;
use crate::model::{Format, Img, OVERHEAD_BYTES, WALL_SECS, sha256_hex};
use crate::src::write_file;

/// Why an image could not be delivered.
pub enum Fail {
    /// A sentence for the caller.
    Msg(String),
    /// The part does not fit what is left of the output budget; nothing was written.
    Over(usize),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Self::Msg(s)
    }
}

/// An encoded image ready to hand over.
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// SHA-256 of the RGBA pixels the file was encoded from.
    pub pixels_sha256: String,
}

impl Prepared {
    /// Wraps `bytes`, which encode `img` as `format`.
    pub fn new(bytes: Vec<u8>, format: Format, img: &Img) -> Self {
        Self {
            bytes,
            format,
            width: img.w,
            height: img.h,
            pixels_sha256: sha256_hex(&img.px),
        }
    }
}

/// One call's mutable state.
pub struct Cx {
    /// Attached images, in order: `{"type":"image","data":..,"mimeType":..}`.
    pub parts: Vec<Value>,
    /// Bytes of the attached images once base64 encoded.
    pub part_bytes: usize,
    /// Bytes of JSON the call has produced so far.
    pub json_bytes: usize,
    /// Bytes of result the call may return before it pages.
    pub budget: usize,
    /// The bound folder files may be written into.
    pub root: Option<PathBuf>,
    /// Where and how results go.
    pub output: Output,
    /// When the call started.
    pub started: Instant,
}

impl Cx {
    /// A fresh call state.
    pub fn new(root: Option<PathBuf>, output: Output, budget: usize) -> Self {
        Self {
            parts: Vec::new(),
            part_bytes: 0,
            json_bytes: 0,
            budget,
            root,
            output,
            started: Instant::now(),
        }
    }

    /// Whether the call has used its wall-clock budget.
    pub fn expired(&self) -> bool {
        self.started.elapsed().as_secs() >= WALL_SECS
    }

    /// Bytes of the call's budget not yet used.
    pub fn left(&self) -> usize {
        self.budget
            .saturating_sub(self.part_bytes + self.json_bytes + OVERHEAD_BYTES)
    }

    /// Raw image bytes the next part may hold.
    pub fn room(&self) -> usize {
        self.left() / 4 * 3
    }

    /// Bytes of the call's budget in use.
    pub fn used(&self) -> usize {
        self.part_bytes + self.json_bytes
    }

    /// The format results are written in: the explicit one, else the file name's extension, else PNG.
    pub fn format(&self) -> Result<Format, String> {
        let named = self
            .output
            .format
            .as_deref()
            .map(crate::input::parse_format)
            .transpose()?;
        let by_ext = self.output.file.as_deref().and_then(|f| {
            std::path::Path::new(f)
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Format::parse)
        });
        match (named, by_ext) {
            (Some(a), Some(b)) if a != b => Err(format!(
                "output.file ends in .{} but output.format is {a}; make them agree",
                b.ext()
            )),
            (Some(f), _) | (None, Some(f)) => Ok(f),
            (None, None) => Ok(Format::Png),
        }
    }

    /// Whether results are attached as image parts by default: unless a file is written.
    pub fn wants_part(&self) -> bool {
        self.output
            .part
            .unwrap_or(self.output.file.is_none() && self.output.suffix.is_none())
    }

    /// The file name a result is written to, or `None` when no file is asked for.
    fn target(
        &self,
        source: &str,
        format: Format,
        tag: Option<&str>,
    ) -> Result<Option<String>, String> {
        let o = &self.output;
        let stem_ext = |name: &str| -> (String, String) {
            let p = std::path::Path::new(name);
            let ext = p.extension().map_or_else(
                || format.ext().to_owned(),
                |e| e.to_string_lossy().into_owned(),
            );
            (p.with_extension("").to_string_lossy().into_owned(), ext)
        };
        let name = match (&o.file, &o.suffix) {
            (Some(f), _) => {
                let (stem, ext) = stem_ext(f);
                if let Some(named) = Format::parse(&ext)
                    && named != format
                {
                    return Err(format!(
                        "{f} ends in .{ext} but the image is {format}; change the file extension or the format"
                    ));
                }
                match tag {
                    Some(t) => format!("{stem}_{t}.{ext}"),
                    None => format!("{stem}.{ext}"),
                }
            }
            (None, Some(suffix)) => {
                let (stem, _) = stem_ext(source);
                match tag {
                    Some(t) => format!("{stem}{suffix}_{t}.{}", format.ext()),
                    None => format!("{stem}{suffix}.{}", format.ext()),
                }
            }
            (None, None) => return Ok(None),
        };
        if self.root.is_none() {
            return Err("files can only be written into a bound folder; bind the folder that holds the image and name the image in files".into());
        }
        Ok(Some(name))
    }

    /// Delivers `p`: writes its file when asked, attaches it as a part when
    /// wanted, and returns the record that describes it. `source` names the
    /// image it came from and `tag` tells tiles apart in file names.
    pub fn deliver(&mut self, p: Prepared, source: &str, tag: Option<&str>) -> Result<Value, Fail> {
        let file = self.target(source, p.format, tag)?;
        let want_part = self.wants_part();
        if want_part && !p.format.viewable() && self.output.part == Some(true) {
            return Err(Fail::Msg(format!(
                "{} images cannot be shown to a model; use png, jpeg, webp or gif for a part, or write a file",
                p.format
            )));
        }
        let attach = want_part && p.format.viewable();
        if attach && p.bytes.len() > self.room() && file.is_none() {
            return Err(Fail::Over(p.bytes.len()));
        }
        let mut out = json!({
            "format": p.format.name(),
            "width": p.width,
            "height": p.height,
            "bytes": p.bytes.len(),
            "sha256": sha256_hex(&p.bytes),
            "pixels_sha256": p.pixels_sha256,
        });
        if let (Some(name), Some(root)) = (&file, &self.root) {
            write_file(root, name, &p.bytes, self.output.overwrite)?;
            out["file"] = json!(name);
        }
        if attach {
            if p.bytes.len() <= self.room() {
                out["part"] = json!(self.parts.len());
                self.part_bytes += p.bytes.len().div_ceil(3) * 4;
                self.parts.push(json!({
                    "type": "image",
                    "data": base64::engine::general_purpose::STANDARD.encode(&p.bytes),
                    "mimeType": p.format.mime(),
                }));
            } else {
                out["not_attached"] = json!(format!(
                    "the image is {} bytes, over the {} bytes one call can attach; it was written to the file",
                    p.bytes.len(),
                    self.room()
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prep(n: usize, f: Format) -> Prepared {
        Prepared {
            bytes: vec![7; n],
            format: f,
            width: 2,
            height: 2,
            pixels_sha256: "px".into(),
        }
    }

    fn cx(output: Output, root: Option<PathBuf>) -> Cx {
        Cx::new(root, output, 3_800_000)
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("image_tools_cx_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn by_default_an_image_becomes_a_part_with_its_hashes() {
        let mut c = cx(Output::default(), None);
        let v = c
            .deliver(prep(10, Format::Png), "a.png", None)
            .ok()
            .unwrap();
        assert_eq!(v["part"], 0);
        assert_eq!(
            (v["width"].clone(), v["height"].clone(), v["bytes"].clone()),
            (json!(2), json!(2), json!(10))
        );
        assert_eq!(v["sha256"], sha256_hex(&[7; 10]));
        assert_eq!(v["pixels_sha256"], "px");
        assert_eq!(c.parts.len(), 1);
        assert_eq!(c.parts[0]["mimeType"], "image/png");
        assert_eq!(c.parts[0]["type"], "image");
        assert_eq!(c.part_bytes, 16, "ten raw bytes are 16 base64 characters");
        let again = c
            .deliver(prep(5, Format::Jpeg), "a.png", None)
            .ok()
            .unwrap();
        assert_eq!(again["part"], 1);
        assert_eq!(c.parts[1]["mimeType"], "image/jpeg");
    }

    #[test]
    fn a_file_is_written_and_the_part_is_skipped_unless_asked_for() {
        let d = dir("file");
        let out = Output {
            file: Some("out/x.png".into()),
            ..Output::default()
        };
        let mut c = cx(out, Some(d.clone()));
        let v = c
            .deliver(prep(10, Format::Png), "a.png", None)
            .ok()
            .unwrap();
        assert_eq!(v["file"], "out/x.png");
        assert!(v.get("part").is_none());
        assert_eq!(std::fs::read(d.join("out/x.png")).unwrap(), vec![7; 10]);
        assert!(c.parts.is_empty());
        let both = Output {
            file: Some("y.png".into()),
            part: Some(true),
            ..Output::default()
        };
        let mut c = cx(both, Some(d.clone()));
        let v = c
            .deliver(prep(10, Format::Png), "a.png", None)
            .ok()
            .unwrap();
        assert!(v["file"] == "y.png" && v["part"] == 0);
    }

    #[test]
    fn a_suffix_names_the_file_beside_the_source_and_tags_tiles() {
        let d = dir("suffix");
        let out = Output {
            suffix: Some("_small".into()),
            ..Output::default()
        };
        let mut c = cx(out, Some(d.clone()));
        let v = c
            .deliver(prep(4, Format::Jpeg), "photos/cat.png", None)
            .ok()
            .unwrap();
        assert_eq!(v["file"], "photos/cat_small.jpg");
        let v = c
            .deliver(prep(4, Format::Png), "cat.png", Some("r2c3"))
            .ok()
            .unwrap();
        assert_eq!(v["file"], "cat_small_r2c3.png");
        let named = Output {
            file: Some("tiles/t.png".into()),
            ..Output::default()
        };
        let mut c = cx(named, Some(d));
        let v = c
            .deliver(prep(4, Format::Png), "cat.png", Some("r1c1"))
            .ok()
            .unwrap();
        assert_eq!(v["file"], "tiles/t_r1c1.png");
    }

    #[test]
    fn a_part_of_exactly_the_room_is_attached_and_one_byte_more_is_not() {
        let mut c = cx(Output::default(), None);
        c.json_bytes = 3_800_000 - OVERHEAD_BYTES - 40;
        let room = c.room();
        assert_eq!(room, 30);
        let v = c.deliver(prep(room, Format::Png), "a", None).ok().unwrap();
        assert_eq!(v["part"], 0, "{v}");
        assert_eq!(c.parts.len(), 1);
        // The part now uses the room up: the next byte does not fit and is refused.
        let before = c.room();
        assert!(matches!(
            c.deliver(prep(before + 1, Format::Png), "a", None),
            Err(Fail::Over(n)) if n == before + 1
        ));
        // With a file to fall back on, the same size is written and reported not attached.
        let d = dir("room");
        let out = Output {
            file: Some("r.png".into()),
            part: Some(true),
            ..Output::default()
        };
        let mut c = cx(out, Some(d));
        c.json_bytes = 3_800_000 - OVERHEAD_BYTES - 40;
        let v = c
            .deliver(prep(room + 1, Format::Png), "a", None)
            .ok()
            .unwrap();
        assert!(
            v.get("part").is_none() && v["not_attached"].is_string(),
            "{v}"
        );
        assert!(c.parts.is_empty());
    }

    #[test]
    fn writing_needs_a_bound_folder_and_never_leaves_it() {
        let out = Output {
            file: Some("x.png".into()),
            ..Output::default()
        };
        let e = match cx(out, None).deliver(prep(4, Format::Png), "a.png", None) {
            Err(Fail::Msg(m)) => m,
            _ => panic!("expected a sentence"),
        };
        assert!(e.contains("bound folder"), "{e}");
        let d = dir("escape");
        let id = std::process::id();
        let (up, abs) = (
            format!("../escape_{id}.png"),
            format!("/tmp/escape_abs_{id}.png"),
        );
        for bad in [up.as_str(), abs.as_str(), "a/../../x.png"] {
            let out = Output {
                file: Some(bad.into()),
                ..Output::default()
            };
            let r = cx(out, Some(d.clone())).deliver(prep(4, Format::Png), "a.png", None);
            assert!(
                matches!(r, Err(Fail::Msg(ref m)) if m.contains("bound folder")),
                "{bad}"
            );
        }
        assert!(
            !d.parent()
                .unwrap()
                .join(format!("escape_{id}.png"))
                .exists()
        );
        assert!(!std::path::Path::new(&abs).exists());
    }

    #[test]
    fn an_existing_file_is_not_replaced_without_overwrite() {
        let d = dir("exists");
        std::fs::write(d.join("x.png"), b"old").unwrap();
        let out = Output {
            file: Some("x.png".into()),
            ..Output::default()
        };
        let r = cx(out.clone(), Some(d.clone())).deliver(prep(4, Format::Png), "a.png", None);
        assert!(matches!(r, Err(Fail::Msg(ref m)) if m.contains("already exists")));
        assert_eq!(std::fs::read(d.join("x.png")).unwrap(), b"old");
        let ow = Output {
            overwrite: true,
            ..out
        };
        cx(ow, Some(d.clone()))
            .deliver(prep(4, Format::Png), "a.png", None)
            .ok()
            .unwrap();
        assert_eq!(std::fs::read(d.join("x.png")).unwrap(), vec![7; 4]);
    }

    #[test]
    fn a_part_over_the_budget_is_refused_but_a_file_still_lands() {
        let mut c = cx(Output::default(), None);
        c.json_bytes = 3_800_000 - OVERHEAD_BYTES - 4;
        assert_eq!(c.room(), 3);
        assert!(matches!(
            c.deliver(prep(10, Format::Png), "a", None),
            Err(Fail::Over(10))
        ));
        assert!(
            c.parts.is_empty(),
            "nothing is attached when the budget is exceeded"
        );
        let d = dir("over");
        let both = Output {
            file: Some("z.png".into()),
            part: Some(true),
            ..Output::default()
        };
        let mut c = cx(both, Some(d.clone()));
        c.json_bytes = 3_800_000 - OVERHEAD_BYTES - 4;
        let v = c.deliver(prep(10, Format::Png), "a", None).ok().unwrap();
        assert_eq!(v["file"], "z.png");
        assert!(
            v.get("part").is_none()
                && v["not_attached"]
                    .as_str()
                    .unwrap()
                    .contains("written to the file")
        );
        assert!(d.join("z.png").exists());
    }

    #[test]
    fn a_format_a_model_cannot_see_is_refused_as_a_part_but_fine_as_a_file() {
        let explicit = Output {
            part: Some(true),
            ..Output::default()
        };
        let r = cx(explicit, None).deliver(prep(4, Format::Tiff), "a", None);
        assert!(matches!(r, Err(Fail::Msg(ref m)) if m.contains("tiff images cannot be shown")));
        let d = dir("tiff");
        let out = Output {
            file: Some("x.tif".into()),
            ..Output::default()
        };
        let v = cx(out, Some(d))
            .deliver(prep(4, Format::Tiff), "a", None)
            .ok()
            .unwrap();
        assert_eq!(v["file"], "x.tif");
        let none = cx(Output::default(), None)
            .deliver(prep(4, Format::Bmp), "a", None)
            .ok()
            .unwrap();
        assert!(
            none.get("part").is_none(),
            "the default silently skips a part a model cannot see"
        );
    }

    #[test]
    fn the_output_format_comes_from_the_option_then_the_file_extension() {
        let f = |format: Option<&str>, file: Option<&str>| {
            cx(
                Output {
                    format: format.map(Into::into),
                    file: file.map(Into::into),
                    ..Output::default()
                },
                None,
            )
            .format()
        };
        assert_eq!(f(None, None), Ok(Format::Png));
        assert_eq!(f(Some("jpeg"), None), Ok(Format::Jpeg));
        assert_eq!(f(None, Some("a/b.webp")), Ok(Format::Webp));
        assert_eq!(f(Some("jpg"), Some("b.jpeg")), Ok(Format::Jpeg));
        assert!(
            f(Some("png"), Some("b.jpg"))
                .unwrap_err()
                .contains("make them agree")
        );
        let d = dir("ext");
        let out = Output {
            file: Some("x.png".into()),
            ..Output::default()
        };
        let r = cx(out, Some(d.clone())).deliver(prep(4, Format::Jpeg), "a", None);
        assert!(
            matches!(r, Err(Fail::Msg(ref m)) if m.contains("ends in .png but the image is jpeg"))
        );
        assert!(!d.join("x.png").exists());
        assert_eq!(f(None, Some("noext")), Ok(Format::Png));
    }
}
