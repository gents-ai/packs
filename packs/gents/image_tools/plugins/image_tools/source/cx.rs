//! The state of one call: the image parts attached so far against the output
//! budget, the folder files are written into, and the clock. Every image a
//! step produces goes through [`Cx::deliver`], which writes the file, attaches
//! the part and reports one record for it.
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::input::Output;
use crate::model::{Format, Img, MAX_PART_SIDE, MAX_PARTS, OVERHEAD_BYTES, WALL_SECS, sha256_hex};
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

/// The format an output file name's extension names: `None` when it has none (the format
/// then decides it), an error when it is not an image extension or the name is a folder.
fn file_format(name: &str) -> Result<Option<Format>, String> {
    if name.ends_with('/') {
        return Err(format!(
            "{name} names a folder; give a file name such as out.png"
        ));
    }
    match std::path::Path::new(name).extension() {
        None => Ok(None),
        Some(e) => Format::parse(&e.to_string_lossy())
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "{name} does not end in an image extension; use a png, jpg, webp, gif, bmp or tif extension"
                )
            }),
    }
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

    /// Raw image bytes the next part may hold: none once the call attaches
    /// [`MAX_PARTS`] images, so the rest pages exactly like the byte budget.
    pub fn room(&self) -> usize {
        if self.parts.len() >= MAX_PARTS {
            return 0;
        }
        self.left() / 4 * 3
    }

    /// Bytes of the call's budget in use.
    pub fn used(&self) -> usize {
        self.part_bytes + self.json_bytes
    }

    /// The format results are written in: the explicit one, else the file name's extension, else PNG.
    pub fn format(&self) -> Result<Format, String> {
        Ok(self.requested_format()?.unwrap_or(Format::Png))
    }

    /// The format `output.format` or the `save.file` extension asks for, if any.
    pub fn requested_format(&self) -> Result<Option<Format>, String> {
        let named = self
            .output
            .format
            .as_deref()
            .map(crate::input::parse_format)
            .transpose()?;
        let by_ext = self
            .output
            .file
            .as_deref()
            .map(file_format)
            .transpose()?
            .flatten();
        match (named, by_ext) {
            (Some(a), Some(b)) if a != b => Err(format!(
                "save.file ends in .{} but output.format is {a}; make them agree",
                b.ext()
            )),
            (Some(f), _) | (None, Some(f)) => Ok(Some(f)),
            (None, None) => Ok(None),
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
                if let Some(named) = file_format(f)?
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
        let too_wide = p.width.max(p.height) > MAX_PART_SIDE;
        let attach = want_part && p.format.viewable() && !too_wide;
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
        if want_part && too_wide {
            out["not_attached"] = json!(format!(
                "the image is {}x{} pixels, over the {MAX_PART_SIDE} on a side a model accepts, so it is not attached; end the chain with a view step to look at it",
                p.width, p.height
            ));
        } else if attach {
            if p.bytes.len() <= self.room() {
                out["part"] = json!(self.parts.len());
                self.part_bytes += p.bytes.len().div_ceil(3) * 4;
                self.parts.push(json!({
                    "type": "image",
                    "data": base64::engine::general_purpose::STANDARD.encode(&p.bytes),
                    "mimeType": p.format.mime(),
                }));
            } else {
                out["not_attached"] = json!(if self.parts.len() >= MAX_PARTS {
                    format!(
                        "this call already attaches {MAX_PARTS} images; it was written to the file"
                    )
                } else {
                    format!(
                        "the image is {} bytes, over the {} bytes one call can attach; it was written to the file",
                        p.bytes.len(),
                        self.room()
                    )
                });
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
    fn an_output_name_must_end_in_an_image_extension_and_name_a_file() {
        let d = dir("ext_names");
        let named = |f: &str| Output {
            file: Some(f.into()),
            ..Output::default()
        };
        for bad in ["x.txt", "x.pdf", "a/b.svg", "x.png.bak", "x.", "sub/"] {
            let c = cx(named(bad), Some(d.clone()));
            let by_format = c.format().err();
            let by_deliver =
                match cx(named(bad), Some(d.clone())).deliver(prep(4, Format::Png), "a", None) {
                    Err(Fail::Msg(m)) => Some(m),
                    _ => None,
                };
            for e in [by_format, by_deliver] {
                let e = e.unwrap_or_else(|| panic!("{bad} was accepted"));
                assert!(
                    e.contains("image extension") || e.contains("names a folder"),
                    "{bad}: {e}"
                );
            }
        }
        assert!(!d.join("x.txt").exists() && !d.join("sub.png").exists());
        // Every image extension in any case is fine, and a name without one takes the format's.
        for (name, format, want) in [
            ("x.JPG", Format::Jpeg, "x.JPG"),
            ("x.tif", Format::Tiff, "x.tif"),
            ("x.tiff", Format::Tiff, "x.tiff"),
            ("x.webp", Format::Webp, "x.webp"),
            ("x", Format::Png, "x.png"),
            ("deep/y", Format::Gif, "deep/y.gif"),
        ] {
            let v = cx(named(name), Some(d.clone())).deliver(prep(4, format), "a", None);
            let v = match v {
                Ok(v) => v,
                Err(Fail::Msg(m)) => panic!("{name}: {m}"),
                Err(_) => panic!("{name}: over"),
            };
            assert_eq!(v["file"], want, "{name}");
        }
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

    #[test]
    fn an_image_over_8000_pixels_a_side_is_not_attached_and_names_view() {
        let wide = |w: u32, h: u32| Prepared {
            width: w,
            height: h,
            ..prep(4, Format::Png)
        };
        let mut c = cx(Output::default(), None);
        let v = c.deliver(wide(8000, 10), "a", None).ok().unwrap();
        assert_eq!(v["part"], 0, "8000 on a side is still attached");
        let v = c.deliver(wide(10, 8001), "a", None).ok().unwrap();
        let note = v["not_attached"].as_str().unwrap();
        assert!(v.get("part").is_none() && note.contains("view step"), "{v}");
        assert_eq!(c.parts.len(), 1);
        let d = dir("wide");
        let out = Output {
            file: Some("w.png".into()),
            part: Some(true),
            ..Output::default()
        };
        let v = cx(out, Some(d.clone()))
            .deliver(wide(9000, 10), "a", None)
            .ok()
            .unwrap();
        assert!(v["file"] == "w.png" && v["not_attached"].is_string(), "{v}");
        assert!(d.join("w.png").exists());
    }

    #[test]
    fn a_call_attaches_at_most_20_images_and_the_next_one_pages() {
        let mut c = cx(Output::default(), None);
        for i in 0..MAX_PARTS {
            let v = c.deliver(prep(4, Format::Png), "a", None).ok().unwrap();
            assert_eq!(v["part"], i);
        }
        assert_eq!(c.room(), 0);
        assert!(matches!(
            c.deliver(prep(4, Format::Png), "a", None),
            Err(Fail::Over(4))
        ));
    }
}
