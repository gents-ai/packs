//! Running one image through the steps of a request. The image is read lazily:
//! a chain of `info` steps never decodes a pixel, and a chain that ends in a
//! `view` of a large JPEG decodes it small. Pixels move from step to step in
//! memory without re-encoding; the chain ends by writing one image, or the
//! terminal step (view, tile, diff) writes its own.
use serde_json::{Value, json};

use crate::analyze;
use crate::annotate;
use crate::cx::{Cx, Fail, Prepared};
use crate::decode::Header;
use crate::encode::encode;
use crate::fit::{Prefer, fit};
use crate::geom;
use crate::input::{
    AnnotateOp, ConvertOp, DiffOp, FlipOp, HashOp, Op, Plan, ResizeOp, TileOp, ViewOp,
    parse_format, resize_mode,
};
use crate::load::{load, pixels};
use crate::model::{Format, Img};
use crate::resize::{self, Filter};
use crate::src::{Source, inline, named};
use crate::tile;

/// Longest side of a view when none is given.
const VIEW_SIDE: u32 = 1568;
/// Bytes of a view when none is given.
const VIEW_BYTES: usize = 1_000_000;
/// Bytes of one tile when none is given.
const TILE_BYTES: usize = 700_000;
/// Tile side when none is given.
const TILE_SIDE: u32 = 1024;
/// Tile overlap when none is given: 64 pixels, or a quarter of a small tile.
fn default_overlap(size: u32) -> u32 {
    64.min(size / 4)
}
/// Room kept for the index picture of a tile page.
const INDEX_BYTES: usize = 400_000;

/// The result of running one image through the chain.
pub struct Item {
    /// The record for the result.
    pub json: Value,
    /// A part did not fit the output budget; nothing of this image was kept.
    pub over: bool,
    /// The next tile to produce, when the tile step stopped early.
    pub next_tile: Option<usize>,
}

struct Work<'a> {
    src: &'a Source,
    header: Header,
    img: Option<Img>,
    turned: u8,
    scale: Option<u32>,
    format: Option<Format>,
    quality: Option<u8>,
    keep_icc: bool,
}

/// What a step returns.
struct Done {
    facts: Value,
    next_tile: Option<usize>,
}

fn facts(v: Value) -> Result<Done, Fail> {
    Ok(Done {
        facts: v,
        next_tile: None,
    })
}

impl Work<'_> {
    /// The pixels, decoded on first use.
    fn take(&mut self, plan: &Plan, view_max: Option<u32>) -> Result<Img, String> {
        if let Some(i) = self.img.take() {
            return Ok(i);
        }
        let (img, scale, turned) =
            pixels(self.src, &self.header, plan.frame, plan.orient, view_max)?;
        self.scale = scale;
        self.turned = turned;
        Ok(img)
    }

    fn icc(&self, plan: &Plan) -> Option<Vec<u8>> {
        (plan.output.keep_icc || self.keep_icc)
            .then(|| self.header.icc.clone())
            .flatten()
    }
}

fn error_item(
    src: &Source,
    header: Option<&Header>,
    e: String,
    step: Option<usize>,
    steps: Vec<Value>,
    warnings: Vec<String>,
) -> Item {
    let mut v = json!({"source": src.name, "error": e, "steps": steps, "outputs": [], "warnings": warnings});
    if let Some(h) = header {
        v["input"] = json!({"format": h.format.name(), "width": h.width, "height": h.height, "bytes": h.bytes});
    }
    if let Some(s) = step {
        v["step"] = json!(s);
    }
    Item {
        json: v,
        over: false,
        next_tile: None,
    }
}

/// Runs `src` through `plan`, starting its tile step at tile `tile_from`.
pub fn run_item(cx: &mut Cx, src: &Source, plan: &Plan, tile_from: usize) -> Item {
    let header = match crate::decode::header(src) {
        Ok(h) => h,
        Err(e) => return error_item(src, None, e, None, Vec::new(), Vec::new()),
    };
    let mut warnings = header.warnings.clone();
    let mut w = Work {
        src,
        header,
        img: None,
        turned: 1,
        scale: None,
        format: None,
        quality: None,
        keep_icc: false,
    };
    let view_max = plan.view_only().map(|v| v.max_side.unwrap_or(VIEW_SIDE));
    let (mut steps, mut outputs, mut next_tile) = (Vec::new(), Vec::new(), None);
    let mut terminal = false;
    for (i, op) in plan.ops.iter().enumerate() {
        let before = (cx.parts.len(), cx.part_bytes);
        let r = step(
            cx,
            &mut w,
            op,
            plan,
            tile_from,
            view_max,
            &mut warnings,
            &mut outputs,
        );
        match r {
            Ok(d) => {
                steps.push(d.facts);
                next_tile = d.next_tile;
                terminal |= op.terminal();
            }
            Err(Fail::Over(n)) => {
                cx.parts.truncate(before.0);
                cx.part_bytes = before.1;
                let mut item = error_item(
                    src,
                    Some(&w.header),
                    too_big(n, cx.room()),
                    Some(i + 1),
                    steps,
                    warnings,
                );
                item.over = true;
                return item;
            }
            Err(Fail::Msg(e)) => {
                // Parts a step attached before it failed belong to no record: drop them.
                cx.parts.truncate(before.0);
                cx.part_bytes = before.1;
                return error_item(src, Some(&w.header), e, Some(i + 1), steps, warnings);
            }
        }
    }
    if plan.emits_image() && !terminal {
        let before = (cx.parts.len(), cx.part_bytes);
        if let Err(f) = emit_final(cx, &mut w, plan, view_max, &mut warnings, &mut outputs) {
            let (e, over) = match f {
                Fail::Msg(m) => {
                    cx.parts.truncate(before.0);
                    cx.part_bytes = before.1;
                    (m, false)
                }
                Fail::Over(n) => {
                    cx.parts.truncate(before.0);
                    cx.part_bytes = before.1;
                    (too_big(n, cx.room()), true)
                }
            };
            let mut item = error_item(src, Some(&w.header), e, None, steps, warnings);
            item.over = over;
            return item;
        }
    }
    if let Some(n) = w.scale {
        warnings.push(format!(
            "the JPEG was decoded at 1/{n} of its size, which is all the view needs"
        ));
    }
    let h = &w.header;
    let mut v = json!({
        "source": src.name,
        "input": {"format": h.format.name(), "width": h.width, "height": h.height, "bytes": h.bytes},
        "steps": steps, "outputs": outputs, "warnings": warnings,
    });
    if plan.frame > 0 {
        v["frame"] = json!(plan.frame);
    }
    if w.turned > 1 {
        v["orientation_applied"] = json!(w.turned);
    }
    Item {
        json: v,
        over: false,
        next_tile,
    }
}

fn too_big(n: usize, room: usize) -> String {
    format!(
        "the image is {n} bytes, over the {room} bytes one call can attach; use view to fit it, or write it to a file with output.file"
    )
}

fn emit_final(
    cx: &mut Cx,
    w: &mut Work,
    plan: &Plan,
    view_max: Option<u32>,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<Value>,
) -> Result<(), Fail> {
    let img = w.take(plan, view_max)?;
    let format = match w.format {
        Some(f) => f,
        None => cx.format()?,
    };
    let quality = w.quality.or(plan.output.quality);
    let icc = w.icc(plan);
    let e = encode(&img, format, quality, icc.as_deref())?;
    warnings.extend(e.notes);
    let rec = cx.deliver(Prepared::new(e.bytes, format, &img), &w.src.name, None)?;
    note_unattached(&rec, warnings);
    push(outputs, rec, "result");
    w.img = Some(img);
    Ok(())
}

/// Adds an output record tagged with its role and returns its index.
fn push(outputs: &mut Vec<Value>, mut rec: Value, role: &str) -> usize {
    rec["role"] = json!(role);
    outputs.push(rec);
    outputs.len() - 1
}

fn note_unattached(rec: &Value, warnings: &mut Vec<String>) {
    if let Some(n) = rec["not_attached"].as_str() {
        warnings.push(n.to_owned());
    }
}

#[allow(clippy::too_many_arguments)]
fn step(
    cx: &mut Cx,
    w: &mut Work,
    op: &Op,
    plan: &Plan,
    tile_from: usize,
    view_max: Option<u32>,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<Value>,
) -> Result<Done, Fail> {
    match op {
        Op::Info(o) => facts(analyze::info(&w.header, o)),
        Op::Resize(o) => facts(do_resize(w, o, plan, view_max)?),
        Op::Crop(o) => {
            let img = w.take(plan, view_max)?;
            let from = [img.w, img.h];
            let out = geom::crop(&img, o.x, o.y, o.width, o.height)?;
            w.img = Some(out);
            facts(json!({"op": "crop", "box": [o.x, o.y, o.width, o.height], "from": from}))
        }
        Op::Rotate(o) => {
            let img = w.take(plan, view_max)?;
            let out = geom::rotate(&img, o.degrees)?;
            let to = [out.w, out.h];
            w.img = Some(out);
            facts(json!({"op": "rotate", "degrees": o.degrees.rem_euclid(360), "to": to}))
        }
        Op::Flip(FlipOp { axis }) => {
            let img = w.take(plan, view_max)?;
            w.img = Some(geom::flip(&img, axis == "horizontal"));
            facts(json!({"op": "flip", "axis": axis}))
        }
        Op::AutoOrient(_) => {
            let o = w.header.orientation();
            let mut img = w.take(plan, view_max)?;
            if !plan.orient && o != 1 {
                img = geom::orient(&img, o);
                w.turned = o;
            }
            w.img = Some(img);
            facts(json!({"op": "auto_orient", "orientation": o, "applied": o != 1}))
        }
        Op::Convert(ConvertOp { format, quality }) => {
            let f = parse_format(format)?;
            w.format = Some(f);
            w.quality = *quality;
            facts(json!({"op": "convert", "format": f.name(), "quality": quality}))
        }
        Op::StripMetadata(o) => {
            w.keep_icc = o.keep_icc;
            let h = &w.header;
            facts(json!({
                "op": "strip_metadata",
                "removed": {
                    "exif": h.exif.is_some(),
                    "gps": h.exif.is_some_and(|e| e.has_gps),
                    "icc": h.icc.is_some() && !(o.keep_icc || plan.output.keep_icc),
                    "xmp": h.xmp,
                },
                "orientation_baked_in": h.orientation() != 1,
            }))
        }
        Op::Annotate(o) => facts(do_annotate(w, o, plan, view_max)?),
        Op::Palette(o) => {
            let img = w.take(plan, view_max)?;
            let v = analyze::palette_step(&img, o.colors.unwrap_or(5));
            w.img = Some(img);
            facts(v)
        }
        Op::DecodeCodes(o) => {
            let img = w.take(plan, view_max)?;
            let v = analyze::codes_step(&img, o);
            w.img = Some(img);
            facts(v?)
        }
        Op::Hash(o) => facts(do_hash(cx, w, o, plan, view_max)?),
        Op::Diff(o) => do_diff(cx, w, o, plan, warnings, outputs),
        Op::View(o) => do_view(cx, w, o, plan, view_max, warnings, outputs),
        Op::Tile(o) => do_tile(cx, w, o, plan, tile_from, warnings, outputs),
        Op::Montage(_) => Err(Fail::Msg(
            "montage takes all the images itself and cannot be chained".into(),
        )),
    }
}

fn do_resize(
    w: &mut Work,
    o: &ResizeOp,
    plan: &Plan,
    view_max: Option<u32>,
) -> Result<Value, String> {
    let img = w.take(plan, view_max)?;
    let from = [img.w, img.h];
    let filter = o
        .filter
        .as_deref()
        .and_then(Filter::parse)
        .unwrap_or(Filter::Lanczos3);
    let spec = resize::Spec {
        mode: resize_mode(o),
        width: o.width,
        height: o.height,
        filter,
        upscale: o.upscale,
    };
    let (out, changed) = resize::apply(img, &spec)?;
    let to = [out.w, out.h];
    w.img = Some(out);
    let mode = o.mode.as_deref().unwrap_or("fit");
    Ok(
        json!({"op": "resize", "mode": mode, "filter": filter.name(), "from": from, "to": to, "changed": changed}),
    )
}

fn do_annotate(
    w: &mut Work,
    o: &AnnotateOp,
    plan: &Plan,
    view_max: Option<u32>,
) -> Result<Value, String> {
    let mut img = w.take(plan, view_max)?;
    let r = annotate::apply(&mut img, &o.shapes);
    w.img = Some(img);
    let mut v = json!({"op": "annotate", "shapes": r.shapes});
    if !r.outside.is_empty() {
        v["outside"] = json!(r.outside);
    }
    if r.replaced > 0 {
        v["replaced_characters"] = json!(r.replaced);
    }
    Ok(v)
}

/// The image to compare against: a file in the bound folder or inline data.
fn other_source(cx: &Cx, name: Option<&String>, b64: Option<&String>) -> Result<Source, String> {
    match (name, b64) {
        (Some(n), _) => match &cx.root {
            Some(root) => Ok(named(root, n)),
            None => Err("against names an image in the bound folder; bind the folder instead of a single file".into()),
        },
        (None, Some(b)) => inline(b, Some("against")),
        (None, None) => Err("name the image to compare with in against or against_base64".into()),
    }
}

fn do_hash(
    cx: &Cx,
    w: &mut Work,
    o: &HashOp,
    plan: &Plan,
    view_max: Option<u32>,
) -> Result<Value, String> {
    let img = w.take(plan, view_max)?;
    let other = if o.against.is_some() || o.against_base64.is_some() {
        let s = other_source(cx, o.against.as_ref(), o.against_base64.as_ref())?;
        let img = load(&s, plan.frame, plan.orient, None)?;
        Some((s, img))
    } else {
        None
    };
    let v = analyze::hash_step(w.src, &img, o, other.as_ref().map(|(s, i)| (s, i)));
    w.img = Some(img);
    v
}

fn do_diff(
    cx: &mut Cx,
    w: &mut Work,
    o: &DiffOp,
    plan: &Plan,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<Value>,
) -> Result<Done, Fail> {
    let a = w.take(plan, None)?;
    let s = other_source(cx, o.against.as_ref(), o.against_base64.as_ref())?;
    let b = load(&s, plan.frame, plan.orient, None)?;
    let (mut v, highlight) = analyze::diff_step(&a, &b, o, &s.name)?;
    if let Some(img) = highlight {
        let format = cx.format()?;
        let e = encode(&img, format, plan.output.quality, None)?;
        warnings.extend(e.notes);
        let rec = cx.deliver(
            Prepared::new(e.bytes, format, &img),
            &w.src.name,
            Some("diff"),
        )?;
        note_unattached(&rec, warnings);
        v["highlight"] = json!(push(outputs, rec, "diff"));
    }
    w.img = Some(a);
    facts(v)
}

fn prefer(f: Option<&str>) -> Prefer {
    match f {
        Some("png") => Prefer::Png,
        Some("jpeg") => Prefer::Jpeg,
        _ => Prefer::Auto,
    }
}

fn do_view(
    cx: &mut Cx,
    w: &mut Work,
    o: &ViewOp,
    plan: &Plan,
    view_max: Option<u32>,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<Value>,
) -> Result<Done, Fail> {
    let img = w.take(plan, view_max)?;
    // After a reduced decode the picture is smaller than the file's: name the file's size.
    let full = if (5..=8).contains(&w.turned) {
        (w.header.height, w.header.width)
    } else {
        (w.header.width, w.header.height)
    };
    let origin = w.scale.map(|_| full);
    let from = origin.map_or([img.w, img.h], |(a, b)| [a, b]);
    let max_side = o.max_side.unwrap_or(VIEW_SIDE);
    let max_bytes = o.max_bytes.unwrap_or(VIEW_BYTES).min(cx.room().max(10_000));
    let icc = w.icc(plan);
    let f = fit(
        img,
        origin,
        max_side,
        max_bytes,
        prefer(o.format.as_deref()),
        icc.as_deref(),
    )?;
    warnings.extend(f.notes.iter().cloned());
    let to = [f.img.w, f.img.h];
    let rec = cx.deliver(Prepared::new(f.bytes, f.format, &f.img), &w.src.name, None)?;
    note_unattached(&rec, warnings);
    let mut v = json!({"op": "view", "from": from, "to": to, "format": f.format.name(), "max_side": max_side, "max_bytes": max_bytes});
    if let Some(q) = f.quality {
        v["quality"] = json!(q);
    }
    v["output"] = json!(push(outputs, rec, "view"));
    Ok(Done {
        facts: v,
        next_tile: None,
    })
}

fn do_tile(
    cx: &mut Cx,
    w: &mut Work,
    o: &TileOp,
    plan: &Plan,
    tile_from: usize,
    warnings: &mut Vec<String>,
    outputs: &mut Vec<Value>,
) -> Result<Done, Fail> {
    let img = w.take(plan, None)?;
    let size = o.size.unwrap_or(TILE_SIDE);
    let overlap = o.overlap.unwrap_or_else(|| default_overlap(size));
    let grid = tile::grid(img.w, img.h, size, overlap)?;
    let mut v = json!({
        "op": "tile", "size": size, "overlap": overlap, "from": [img.w, img.h],
        "cols": grid.cols(), "rows": grid.rows(), "tiles_total": grid.len(),
    });
    let part = cx.wants_part();
    if tile_from == 0 && o.index.unwrap_or(true) {
        let idx = tile::index_image(&img, &grid)?;
        let rec = if part {
            // The index never takes more than half of what is left, so the tiles always have room.
            let f = fit(
                idx,
                None,
                1024,
                INDEX_BYTES.min(cx.room() / 2),
                Prefer::Auto,
                None,
            )?;
            warnings.extend(f.notes.iter().cloned());
            cx.deliver(
                Prepared::new(f.bytes, f.format, &f.img),
                &w.src.name,
                Some("index"),
            )?
        } else {
            let format = cx.format()?;
            let e = encode(&idx, format, plan.output.quality, None)?;
            cx.deliver(
                Prepared::new(e.bytes, format, &idx),
                &w.src.name,
                Some("index"),
            )?
        };
        v["index"] = json!(push(outputs, rec, "index"));
    }
    let max_bytes = o.max_bytes.unwrap_or(TILE_BYTES);
    let mut listed = Vec::new();
    let mut next = None;
    for i in tile_from..grid.len() {
        // Stop where a part would no longer fit, or when the call's time is spent.
        if !listed.is_empty() && (cx.expired() || (part && cx.room() < 20_000)) {
            next = Some(i);
            break;
        }
        let c = grid.cell(i);
        let t = tile::cut(&img, &c)?;
        let tag = format!("r{}c{}", c.row + 1, c.col + 1);
        let prepared = if part {
            let want = max_bytes.min(cx.room());
            match fit(
                t,
                None,
                size,
                want,
                prefer(plan.output.format.as_deref()),
                None,
            ) {
                Ok(f) => {
                    warnings.extend(f.notes.iter().map(|n| format!("tile {}: {n}", c.n)));
                    Prepared::new(f.bytes, f.format, &f.img)
                }
                Err(e) => return Err(Fail::Msg(format!("tile {}: {e}", c.n))),
            }
        } else {
            let format = cx.format()?;
            let e = encode(&t, format, plan.output.quality, None)?;
            warnings.extend(e.notes.iter().map(|n| format!("tile {}: {n}", c.n)));
            Prepared::new(e.bytes, format, &t)
        };
        let mut rec = cx.deliver(prepared, &w.src.name, Some(&tag))?;
        note_unattached(&rec, warnings);
        let mut entry = json!({"n": c.n, "row": c.row, "col": c.col, "x": c.x, "y": c.y, "width": c.width, "height": c.height});
        rec["tile"] = json!(c.n);
        entry["output"] = json!(push(outputs, rec, "tile"));
        listed.push(entry);
    }
    v["tiles"] = json!(listed);
    if let Some(n) = next {
        v["next_tile"] = json!(n + 1);
    }
    w.img = Some(img);
    Ok(Done {
        facts: v,
        next_tile: next,
    })
}

/// The SHA-256 of a file and its size, for a cursor that stops inside it.
pub fn file_identity(src: &Source) -> Result<(u64, String), String> {
    Ok((src.len()?, src.sha256()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_overlap_is_64_or_a_quarter_of_a_small_tile() {
        assert_eq!(default_overlap(1024), 64);
        assert_eq!(default_overlap(256), 64);
        assert_eq!(default_overlap(200), 50);
        assert_eq!(default_overlap(64), 16);
        for size in 64..=4096 {
            assert!(default_overlap(size) < size, "{size}");
        }
    }

    #[test]
    fn an_error_record_names_the_source_the_step_and_what_ran() {
        let src = Source {
            name: "a.png".into(),
            data: crate::src::Data::Mem(std::sync::Arc::new(Vec::new())),
        };
        let item = error_item(
            &src,
            None,
            "broken".into(),
            Some(3),
            vec![json!({"op": "info"})],
            vec!["w".into()],
        );
        assert!(!item.over && item.next_tile.is_none());
        assert_eq!(
            item.json,
            json!({"source": "a.png", "error": "broken", "step": 3, "steps": [{"op": "info"}], "outputs": [], "warnings": ["w"]})
        );
    }

    #[test]
    fn an_over_budget_sentence_says_how_much_and_what_to_do() {
        let s = too_big(3_000_000, 1_000);
        assert!(
            s.contains("3000000 bytes")
                && s.contains("1000 bytes")
                && s.contains("view")
                && s.contains("output.file"),
            "{s}"
        );
    }
}
