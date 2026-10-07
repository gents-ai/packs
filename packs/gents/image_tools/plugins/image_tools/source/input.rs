//! The request: which images, which steps, and where the results go. A
//! request names one `op` with its options at the top level, or a chain in
//! `ops`; both are normalised to a list of steps before anything is read.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::draw::parse_color;
use crate::model::{Format, MAX_PART_BYTES, MAX_PART_SIDE, MIN_PAGE_BYTES, PAGE_BYTES};
use crate::resize::{Filter, Mode};

/// Steps one chain may hold.
pub const MAX_STEPS: usize = 16;
/// Shapes one annotate step may draw.
pub const MAX_SHAPES: usize = 1000;

/// Keys that belong to the request, not to a step.
const COMMON: [&str; 15] = [
    "path",
    "path_original",
    "file",
    "files",
    "data_base64",
    "name",
    "cursor",
    "frame",
    "orient",
    "output",
    "save",
    "ops",
    "op",
    "run_id",
    "page_bytes",
];

/// The request as sent, with the step options still loose.
#[derive(Deserialize, Debug, Default)]
pub struct Input {
    pub path: Option<String>,
    pub file: Option<String>,
    pub files: Option<Vec<String>>,
    pub data_base64: Option<String>,
    pub name: Option<String>,
    pub cursor: Option<String>,
    pub frame: Option<u32>,
    pub orient: Option<bool>,
    pub page_bytes: Option<usize>,
    pub output: Option<OutputIn>,
    pub save: Option<Save>,
    pub ops: Option<Vec<Value>>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// How the produced image is encoded and whether it is attached.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct OutputIn {
    pub format: Option<String>,
    pub quality: Option<u8>,
    pub part: Option<bool>,
    #[serde(default)]
    pub keep_icc: bool,
}

/// Where the produced image is written. A separate request field from
/// `output` because the host grants write access per call by the presence of
/// this field alone (the manifest's `bind_dir.write_fields`).
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct Save {
    pub file: Option<String>,
    pub suffix: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

/// Where and how results are written: `output` and `save` together.
#[derive(Serialize, Debug, Default, Clone)]
pub struct Output {
    /// Output format; PNG when absent.
    pub format: Option<String>,
    /// JPEG quality 1 to 100, or GIF palette effort.
    pub quality: Option<u8>,
    /// A file name inside the bound folder to write the one result to.
    pub file: Option<String>,
    /// A suffix added to each source's name to write one file per image.
    pub suffix: Option<String>,
    /// Replace an existing file of that name.
    pub overwrite: bool,
    /// Attach the image to the result; the default is to attach unless a file is written.
    pub part: Option<bool>,
    /// Keep the ICC colour profile in the result where the format can carry it.
    pub keep_icc: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// One rectangle, in pixels.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct InfoOp {
    #[serde(default)]
    pub gps: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ViewOp {
    pub max_side: Option<u32>,
    pub max_bytes: Option<usize>,
    pub format: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResizeOp {
    pub mode: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub filter: Option<String>,
    #[serde(default)]
    pub upscale: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CropOp {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RotateOp {
    pub degrees: i64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FlipOp {
    pub axis: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConvertOp {
    pub format: String,
    pub quality: Option<u8>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StripOp {
    #[serde(default)]
    pub keep_icc: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct TileOp {
    pub size: Option<u32>,
    pub overlap: Option<u32>,
    pub index: Option<bool>,
    pub max_bytes: Option<usize>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MontageOp {
    pub cols: Option<u32>,
    pub cell: Option<u32>,
    pub gap: Option<u32>,
    pub labels: Option<bool>,
    pub background: Option<String>,
}

/// One drawn shape.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Box {
        x: i64,
        y: i64,
        width: u32,
        height: u32,
        color: Option<String>,
        thickness: Option<u32>,
        label: Option<String>,
        fill: Option<String>,
    },
    Arrow {
        from: [i64; 2],
        to: [i64; 2],
        color: Option<String>,
        thickness: Option<u32>,
    },
    Line {
        from: [i64; 2],
        to: [i64; 2],
        color: Option<String>,
        thickness: Option<u32>,
    },
    Label {
        x: i64,
        y: i64,
        text: String,
        color: Option<String>,
        background: Option<String>,
        scale: Option<u32>,
    },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AnnotateOp {
    pub shapes: Vec<Shape>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DiffOp {
    pub against: Option<String>,
    pub against_base64: Option<String>,
    pub tolerance: Option<u8>,
    #[serde(default)]
    pub ignore: Vec<Rect>,
    pub merge_gap: Option<u32>,
    pub highlight: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PaletteOp {
    pub colors: Option<u32>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CodesOp {
    pub formats: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct HashOp {
    pub against: Option<String>,
    pub against_base64: Option<String>,
    pub threshold: Option<u32>,
}

/// One step of a chain.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Info(InfoOp),
    View(ViewOp),
    Resize(ResizeOp),
    Crop(CropOp),
    Rotate(RotateOp),
    Flip(FlipOp),
    AutoOrient(Empty),
    Convert(ConvertOp),
    StripMetadata(StripOp),
    Tile(TileOp),
    Montage(MontageOp),
    Annotate(AnnotateOp),
    Diff(DiffOp),
    Palette(PaletteOp),
    DecodeCodes(CodesOp),
    Hash(HashOp),
}

/// The operation names, for the sentence that lists them.
pub const OP_NAMES: &str = "info, view, resize, crop, rotate, flip, auto_orient, convert, strip_metadata, tile, montage, annotate, diff, palette, decode_codes, hash";

impl Op {
    /// The name of the step.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Info(_) => "info",
            Self::View(_) => "view",
            Self::Resize(_) => "resize",
            Self::Crop(_) => "crop",
            Self::Rotate(_) => "rotate",
            Self::Flip(_) => "flip",
            Self::AutoOrient(_) => "auto_orient",
            Self::Convert(_) => "convert",
            Self::StripMetadata(_) => "strip_metadata",
            Self::Tile(_) => "tile",
            Self::Montage(_) => "montage",
            Self::Annotate(_) => "annotate",
            Self::Diff(_) => "diff",
            Self::Palette(_) => "palette",
            Self::DecodeCodes(_) => "decode_codes",
            Self::Hash(_) => "hash",
        }
    }

    /// Whether the step ends the chain by producing the output image itself.
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::View(_) | Self::Tile(_) | Self::Diff(_) | Self::Montage(_)
        )
    }

    /// Whether the step changes the picture or its encoding, so the chain ends by writing an image.
    pub fn transforms(&self) -> bool {
        matches!(
            self,
            Self::Resize(_)
                | Self::Crop(_)
                | Self::Rotate(_)
                | Self::Flip(_)
                | Self::AutoOrient(_)
                | Self::Convert(_)
                | Self::StripMetadata(_)
                | Self::Annotate(_)
        )
    }
}

/// The request after normalisation.
pub struct Plan {
    pub ops: Vec<Op>,
    pub raw_ops: Vec<Value>,
    pub output: Output,
    pub frame: u32,
    pub orient: bool,
    pub page_bytes: usize,
}

fn parse_op(v: &Value, label: &str) -> Result<Op, String> {
    let name = v
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label} needs an op; use one of {OP_NAMES}"))?;
    let name = if name == "contact_sheet" {
        "montage"
    } else {
        name
    };
    if !OP_NAMES.split(", ").any(|n| n == name) {
        return Err(format!(
            "{label} has the unknown op {name:?}; use one of {OP_NAMES}"
        ));
    }
    // `contact_sheet` is another name for `montage`.
    let mut step = v.clone();
    step["op"] = Value::from(name);
    crate::typed::from_value(step, &format!("{label} ({name})"), &["op"])
}

/// The format named `name`, or a sentence listing the choices.
pub fn parse_format(name: &str) -> Result<Format, String> {
    Format::parse(name).ok_or_else(|| {
        format!("{name:?} is not an image format; use png, jpeg, webp, gif, bmp or tiff")
    })
}

impl Input {
    /// Normalises the request into steps and checks everything that does not need an image.
    pub fn plan(&self) -> Result<Plan, String> {
        let raw_ops: Vec<Value> = match (&self.ops, self.rest.get("op")) {
            (Some(_), Some(_)) => {
                return Err("give either op with its options or ops, not both".into());
            }
            (Some(ops), None) => {
                if let Some(k) = self.rest.keys().find(|k| !COMMON.contains(&k.as_str())) {
                    return Err(format!("{k} belongs inside a step of ops"));
                }
                ops.clone()
            }
            (None, Some(_)) => {
                let mut step = self.rest.clone();
                step.retain(|k, _| k != "run_id" && k != "path_original");
                vec![Value::Object(step)]
            }
            (None, None) => {
                return Err(format!("give an op to run; use one of {OP_NAMES}"));
            }
        };
        if raw_ops.is_empty() || raw_ops.len() > MAX_STEPS {
            return Err(format!("ops must hold between 1 and {MAX_STEPS} steps"));
        }
        let ops = raw_ops
            .iter()
            .enumerate()
            .map(|(i, v)| parse_op(v, &format!("step {}", i + 1)))
            .collect::<Result<Vec<_>, _>>()?;
        if self
            .save
            .as_ref()
            .is_some_and(|s| s.file.is_none() && s.suffix.is_none())
        {
            return Err("save needs file or suffix; leave save out to only read".into());
        }
        let (o, w) = (
            self.output.clone().unwrap_or_default(),
            self.save.clone().unwrap_or_default(),
        );
        let output = Output {
            format: o.format,
            quality: o.quality,
            file: w.file,
            suffix: w.suffix,
            overwrite: w.overwrite,
            part: o.part,
            keep_icc: o.keep_icc,
        };
        let plan = Plan {
            ops,
            raw_ops,
            output,
            frame: self.frame.unwrap_or(0),
            orient: self.orient.unwrap_or(true),
            page_bytes: self.page_bytes.unwrap_or(PAGE_BYTES),
        };
        plan.validate()?;
        Ok(plan)
    }
}

impl Plan {
    fn validate(&self) -> Result<(), String> {
        let n = self.ops.len();
        for (i, op) in self.ops.iter().enumerate() {
            if op.terminal() && i + 1 != n {
                return Err(format!(
                    "{} produces the final image, so it must be the last step",
                    op.name()
                ));
            }
            if matches!(op, Op::Montage(_)) && n != 1 {
                return Err("montage takes all the images itself and cannot be chained".into());
            }
            validate_op(op)?;
        }
        if !(MIN_PAGE_BYTES..=PAGE_BYTES).contains(&self.page_bytes) {
            return Err(format!(
                "page_bytes must be between {MIN_PAGE_BYTES} and {PAGE_BYTES}"
            ));
        }
        let o = &self.output;
        if let Some(f) = &o.format {
            parse_format(f)?;
        }
        if o.quality.is_some_and(|q| !(1..=100).contains(&q)) {
            return Err("quality must be between 1 and 100".into());
        }
        if o.file.is_some() && o.suffix.is_some() {
            return Err("give save.file or save.suffix, not both".into());
        }
        if (o.file.is_some() || o.suffix.is_some())
            && !self.ops.iter().any(|op| op.transforms() || op.terminal())
        {
            return Err("nothing is produced to write; add a step such as resize or view".into());
        }
        if let Some(s) = &o.suffix
            && (s.is_empty() || s.contains(['/', '\\', '\0']))
        {
            return Err("save.suffix must be a short text without slashes".into());
        }
        Ok(())
    }

    /// Whether the chain ends by writing an image.
    pub fn emits_image(&self) -> bool {
        self.ops.iter().any(|op| op.transforms() || op.terminal())
    }

    /// The `view` step when the chain only needs a shrunken picture, so a JPEG may be decoded small.
    pub fn view_only(&self) -> Option<&ViewOp> {
        match self.ops.as_slice() {
            [Op::View(v)] | [Op::AutoOrient(_), Op::View(v)] => Some(v),
            _ => None,
        }
    }

    /// Images one item attaches before it can stop part way: a tile step's
    /// index picture and its first tile, otherwise the one result.
    pub fn first_parts(&self) -> usize {
        match self.ops.last() {
            Some(Op::Tile(t)) if t.index.unwrap_or(true) => 2,
            _ => 1,
        }
    }

    /// Whether the chain's only source is the montage's own list.
    pub fn is_montage(&self) -> bool {
        matches!(self.ops.as_slice(), [Op::Montage(_)])
    }
}

fn range(name: &str, v: Option<u32>, lo: u32, hi: u32) -> Result<(), String> {
    match v {
        Some(x) if !(lo..=hi).contains(&x) => Err(format!("{name} must be between {lo} and {hi}")),
        _ => Ok(()),
    }
}

fn validate_op(op: &Op) -> Result<(), String> {
    match op {
        Op::View(v) => {
            range("max_side", v.max_side, 16, MAX_PART_SIDE)?;
            if v.max_bytes
                .is_some_and(|b| !(10_000..=MAX_PART_BYTES).contains(&b))
            {
                return Err(format!(
                    "max_bytes must be between 10000 and {MAX_PART_BYTES}"
                ));
            }
            if let Some(f) = &v.format
                && !matches!(f.as_str(), "auto" | "png" | "jpeg")
            {
                return Err("view format must be auto, png or jpeg".into());
            }
        }
        Op::Resize(r) => {
            if let Some(m) = &r.mode
                && !matches!(m.as_str(), "fit" | "fill" | "exact")
            {
                return Err("resize mode must be fit, fill or exact".into());
            }
            if let Some(f) = &r.filter
                && Filter::parse(f).is_none()
            {
                return Err(
                    "resize filter must be nearest, box, bilinear, catmull_rom or lanczos3".into(),
                );
            }
        }
        Op::Flip(f) if !matches!(f.axis.as_str(), "horizontal" | "vertical") => {
            return Err("flip axis must be horizontal or vertical".into());
        }
        Op::Convert(c) => {
            parse_format(&c.format)?;
            if c.quality.is_some_and(|q| !(1..=100).contains(&q)) {
                return Err("quality must be between 1 and 100".into());
            }
        }
        Op::Tile(t) => {
            range("size", t.size, 64, 4096)?;
            let size = t.size.unwrap_or(1024);
            if t.overlap.is_some_and(|o| o >= size) {
                return Err("overlap must be smaller than the tile size".into());
            }
            if t.max_bytes
                .is_some_and(|b| !(10_000..=MAX_PART_BYTES).contains(&b))
            {
                return Err(format!(
                    "max_bytes must be between 10000 and {MAX_PART_BYTES}"
                ));
            }
        }
        Op::Montage(m) => {
            range("cell", m.cell, 16, 1024)?;
            range("cols", m.cols, 1, 100)?;
            range("gap", m.gap, 0, 256)?;
            if let Some(b) = &m.background {
                parse_color(b)?;
            }
        }
        Op::Annotate(a) => {
            if a.shapes.is_empty() || a.shapes.len() > MAX_SHAPES {
                return Err(format!("annotate needs between 1 and {MAX_SHAPES} shapes"));
            }
            for s in &a.shapes {
                validate_shape(s)?;
            }
        }
        Op::Diff(d) => {
            if d.against.is_none() == d.against_base64.is_none() {
                return Err(
                    "diff needs exactly one of against (a file name) or against_base64".into(),
                );
            }
            if d.ignore.len() > 1000 {
                return Err("ignore holds at most 1000 regions".into());
            }
        }
        Op::Hash(h) => {
            if h.against.is_some() && h.against_base64.is_some() {
                return Err("give either against or against_base64, not both".into());
            }
            range("threshold", h.threshold, 0, 64)?;
        }
        Op::Palette(p) => range("colors", p.colors, 1, 16)?,
        _ => {}
    }
    Ok(())
}

fn validate_shape(s: &Shape) -> Result<(), String> {
    let colour = |c: &Option<String>| c.as_deref().map_or(Ok([0; 4]), parse_color).map(|_| ());
    match s {
        Shape::Box {
            color,
            fill,
            thickness,
            width,
            height,
            label,
            ..
        } => {
            colour(color)?;
            colour(fill)?;
            if *width == 0 || *height == 0 {
                return Err("a box needs a width and a height of at least 1".into());
            }
            range("thickness", *thickness, 1, 64)?;
            if label.as_ref().is_some_and(|l| l.chars().count() > 200) {
                return Err("a label holds at most 200 characters".into());
            }
        }
        Shape::Arrow {
            color, thickness, ..
        }
        | Shape::Line {
            color, thickness, ..
        } => {
            colour(color)?;
            range("thickness", *thickness, 1, 64)?;
        }
        Shape::Label {
            color,
            background,
            scale,
            text,
            ..
        } => {
            colour(color)?;
            colour(background)?;
            range("scale", *scale, 1, 16)?;
            if text.chars().count() > 200 {
                return Err("a label holds at most 200 characters".into());
            }
        }
    }
    Ok(())
}

/// The resize mode named in a request.
pub fn resize_mode(op: &ResizeOp) -> Mode {
    match op.mode.as_deref() {
        Some("fill") => Mode::Fill,
        Some("exact") => Mode::Exact,
        _ => Mode::Fit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(v: Value) -> Result<Plan, String> {
        serde_json::from_value::<Input>(v)
            .map_err(|e| e.to_string())?
            .plan()
    }

    #[test]
    fn a_single_op_takes_its_options_from_the_top_level() {
        let p =
            plan(json!({"path": "/x", "op": "resize", "width": 100, "mode": "fill", "height": 50}))
                .unwrap();
        assert_eq!(p.ops.len(), 1);
        assert!(
            matches!(&p.ops[0], Op::Resize(r) if r.width == Some(100) && r.mode.as_deref() == Some("fill"))
        );
        assert_eq!(p.raw_ops[0]["op"], "resize");
        assert!(p.orient && p.frame == 0);
    }

    #[test]
    fn a_chain_keeps_options_inside_each_step() {
        let p = plan(json!({"path": "/x", "ops": [
            {"op": "crop", "x": 1, "y": 2, "width": 3, "height": 4},
            {"op": "rotate", "degrees": 90},
            {"op": "view", "max_side": 512}]}))
        .unwrap();
        let names: Vec<_> = p.ops.iter().map(Op::name).collect();
        assert_eq!(names, ["crop", "rotate", "view"]);
        let e = plan(json!({"path": "/x", "width": 5, "ops": [{"op": "info"}]}))
            .err()
            .unwrap();
        assert!(e.contains("width belongs inside a step"), "{e}");
        let e = plan(json!({"path": "/x", "op": "info", "ops": [{"op": "info"}]}))
            .err()
            .unwrap();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn a_missing_empty_or_unknown_op_is_named() {
        assert!(
            plan(json!({"path": "/x"}))
                .err()
                .unwrap()
                .contains("give an op")
        );
        assert!(
            plan(json!({"path": "/x", "ops": []}))
                .err()
                .unwrap()
                .contains("between 1 and 16")
        );
        assert!(
            plan(json!({"path": "/x", "ops": [{"width": 1}]}))
                .err()
                .unwrap()
                .contains("step 1 needs an op")
        );
        let e = plan(json!({"path": "/x", "op": "sharpen"})).err().unwrap();
        assert!(e.contains("unknown op") && e.contains("palette"), "{e}");
        let steps = |n: usize| -> Vec<Value> { (0..n).map(|_| json!({"op": "info"})).collect() };
        assert_eq!(
            plan(json!({"path": "/x", "ops": steps(1)}))
                .unwrap()
                .ops
                .len(),
            1
        );
        assert_eq!(
            plan(json!({"path": "/x", "ops": steps(16)}))
                .unwrap()
                .ops
                .len(),
            16
        );
        let e = plan(json!({"path": "/x", "ops": steps(17)})).err().unwrap();
        assert_eq!(e, "ops must hold between 1 and 16 steps");
        let e = plan(json!({"path": "/x", "ops": steps(0)})).err().unwrap();
        assert_eq!(e, "ops must hold between 1 and 16 steps");
    }

    #[test]
    fn bad_numeric_options_are_one_sentence_without_a_type_name() {
        for (v, want) in [
            (
                json!({"path": "/x", "op": "resize", "width": -5}),
                "step 1 (resize) is not valid: width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"path": "/x", "op": "resize", "width": 1000.0}),
                "step 1 (resize) is not valid: width must be a whole number from 0 to 4294967295",
            ),
            (
                json!({"path": "/x", "ops": [{"op": "info"}, {"op": "crop", "x": 0, "y": "top", "width": 1, "height": 1}]}),
                "step 2 (crop) is not valid: y must be a whole number from 0 to 4294967295",
            ),
        ] {
            assert_eq!(plan(v).err().unwrap(), want);
        }
    }

    #[test]
    fn unknown_and_mistyped_options_are_refused_by_step() {
        let e = plan(json!({"path": "/x", "op": "resize", "widht": 5}))
            .err()
            .unwrap();
        assert!(e.contains("resize") && e.contains("widht"), "{e}");
        let e =
            plan(json!({"path": "/x", "op": "crop", "x": "a", "y": 0, "width": 1, "height": 1}))
                .err()
                .unwrap();
        assert!(e.contains("crop"), "{e}");
        let e = plan(json!({"path": "/x", "op": "crop", "x": -1, "y": 0, "width": 1, "height": 1}))
            .err()
            .unwrap();
        assert!(e.contains("crop"), "{e}");
        assert!(
            plan(json!({"path": "/x", "op": "rotate"}))
                .err()
                .unwrap()
                .contains("degrees")
        );
    }

    #[test]
    fn step_order_rules_hold() {
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "view"}, {"op": "resize", "width": 5}]}))
                .err()
                .unwrap()
                .contains("must be the last")
        );
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "tile"}, {"op": "info"}]}))
                .err()
                .unwrap()
                .contains("must be the last")
        );
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "info"}, {"op": "montage"}]}))
                .err()
                .unwrap()
                .contains("cannot be chained")
        );
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "resize", "width": 5}, {"op": "view"}]}))
                .is_ok()
        );
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "info"}, {"op": "hash"}, {"op": "palette"}]}))
                .is_ok()
        );
    }

    #[test]
    fn option_ranges_and_enums_are_checked() {
        let bad = |v: Value| plan(v).err().unwrap();
        assert!(bad(json!({"op": "view", "max_side": 8})).contains("max_side"));
        assert!(bad(json!({"op": "view", "max_bytes": 5})).contains("max_bytes"));
        assert!(bad(json!({"op": "view", "format": "gif"})).contains("view format"));
        assert!(bad(json!({"op": "resize", "mode": "zoom"})).contains("mode"));
        assert!(bad(json!({"op": "resize", "filter": "sinc"})).contains("filter"));
        assert!(bad(json!({"op": "flip", "axis": "up"})).contains("axis"));
        assert!(bad(json!({"op": "convert", "format": "svg"})).contains("not an image format"));
        assert!(bad(json!({"op": "convert", "format": "png", "quality": 0})).contains("quality"));
        assert!(bad(json!({"op": "tile", "size": 10})).contains("size"));
        assert!(bad(json!({"op": "tile", "size": 100, "overlap": 100})).contains("overlap"));
        assert!(bad(json!({"op": "montage", "cell": 4})).contains("cell"));
        assert!(bad(json!({"op": "montage", "background": "nope"})).contains("not a colour"));
        assert!(bad(json!({"op": "palette", "colors": 0})).contains("colors"));
        assert!(bad(json!({"op": "hash", "threshold": 65})).contains("threshold"));
        assert!(bad(json!({"op": "diff"})).contains("exactly one"));
        assert!(
            bad(json!({"op": "diff", "against": "a", "against_base64": "b"}))
                .contains("exactly one")
        );
        assert!(bad(json!({"op": "annotate", "shapes": []})).contains("between 1 and"));
        assert!(bad(json!({"op": "annotate", "shapes": [{"type": "box", "x": 0, "y": 0, "width": 0, "height": 5}]})).contains("width"));
        assert!(bad(json!({"op": "annotate", "shapes": [{"type": "line", "from": [0, 0], "to": [1, 1], "color": "x"}]})).contains("not a colour"));
        assert!(bad(json!({"op": "annotate", "shapes": [{"type": "star"}]})).contains("annotate"));
    }

    #[test]
    fn output_rules_hold() {
        let bad = |v: Value| plan(v).err().unwrap();
        assert!(
            bad(json!({"op": "resize", "output": {"format": "svg"}}))
                .contains("not an image format")
        );
        assert!(bad(json!({"op": "resize", "output": {"quality": 101}})).contains("quality"));
        assert!(
            bad(json!({"op": "resize", "save": {"file": "a.png", "suffix": "_x"}}))
                .contains("not both")
        );
        assert!(bad(json!({"op": "resize", "save": {"suffix": "a/b"}})).contains("slashes"));
        assert!(
            bad(json!({"op": "info", "save": {"file": "a.png"}})).contains("nothing is produced")
        );
        assert!(bad(json!({"op": "resize", "output": {"colour": 1}})).contains("colour"));
        assert!(bad(json!({"op": "resize", "output": {"file": "a.png"}})).contains("file"));
        assert!(
            bad(json!({"op": "resize", "save": {"overwrite": true}})).contains("file or suffix")
        );
        assert!(
            plan(json!({"op": "resize", "width": 4, "save": {"file": "a.png", "overwrite": true}}))
                .is_ok()
        );
    }

    #[test]
    fn emits_image_and_view_only_follow_the_steps() {
        let p = plan(json!({"op": "info"})).unwrap();
        assert!(!p.emits_image() && p.view_only().is_none());
        let p = plan(json!({"op": "view"})).unwrap();
        assert!(p.emits_image() && p.view_only().is_some());
        let p = plan(json!({"ops": [{"op": "auto_orient"}, {"op": "view"}]})).unwrap();
        assert!(p.view_only().is_some());
        let p = plan(json!({"ops": [{"op": "crop", "x": 0, "y": 0, "width": 1, "height": 1}, {"op": "view"}]})).unwrap();
        assert!(p.view_only().is_none(), "a crop needs the full-size pixels");
        assert!(plan(json!({"op": "montage"})).unwrap().is_montage());
    }

    #[test]
    fn contact_sheet_is_another_name_for_montage() {
        let p = plan(json!({"path": "/x", "op": "contact_sheet", "cell": 64})).unwrap();
        assert!(p.is_montage());
        let p = plan(json!({"path": "/x", "ops": [{"op": "contact_sheet"}]})).unwrap();
        assert!(p.is_montage());
        assert!(
            plan(json!({"path": "/x", "ops": [{"op": "info"}, {"op": "contact_sheet"}]}))
                .err()
                .unwrap()
                .contains("cannot be chained")
        );
    }

    #[test]
    fn frame_orient_and_page_size_pass_through() {
        let p =
            plan(json!({"op": "info", "frame": 2, "orient": false, "page_bytes": 50000})).unwrap();
        assert_eq!((p.frame, p.orient, p.page_bytes), (2, false, 50_000));
        assert_eq!(plan(json!({"op": "info"})).unwrap().page_bytes, PAGE_BYTES);
        assert!(
            plan(json!({"op": "info", "page_bytes": 10}))
                .err()
                .unwrap()
                .contains("page_bytes")
        );
        assert!(
            plan(json!({"op": "info", "page_bytes": 9_000_000}))
                .err()
                .unwrap()
                .contains("page_bytes")
        );
    }
}
