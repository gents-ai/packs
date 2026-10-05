//! One call, start to finish: read the request, resolve the sources, run each
//! through the chain, and page the results. The result is `{"results": [...]}`
//! with one record per image, plus `next.cursor` when the call stopped before
//! the end; image parts ride alongside as `{"response": ..., "parts": [...]}`.
use serde_json::{Value, json};

use crate::chain::{file_identity, run_item};
use crate::cursor::{self, Cursor, fingerprint};
use crate::cx::{Cx, Fail, Prepared};
use crate::draw::parse_color;
use crate::encode::encode;
use crate::input::{Input, MontageOp, Op, Plan};
use crate::model::OVERHEAD_BYTES;
use crate::montage;
use crate::src::{Source, resolve};

/// The output ceiling of a plugin call.
const OUTPUT_CEILING: usize = 4 * 1024 * 1024;

/// The finished result of a call.
pub struct Done {
    /// The records: `{"results": [...], "warnings": [...], "next"?: ...}`.
    pub response: Value,
    /// The attached images, `{"type":"image","data":...,"mimeType":...}` each.
    pub parts: Vec<Value>,
}

impl Done {
    /// The JSON text a call prints: the response alone, or the response with its image parts.
    pub fn render(self) -> Result<String, String> {
        let out = if self.parts.is_empty() {
            self.response
        } else {
            json!({"response": self.response, "parts": self.parts})
        };
        let text = out.to_string();
        if text.len() > OUTPUT_CEILING - OVERHEAD_BYTES / 2 {
            return Err(
                "the result is over the output limit; ask for fewer images or a smaller page_bytes"
                    .into(),
            );
        }
        Ok(text)
    }
}

/// Runs one request and returns the JSON text to print.
pub fn execute(input: &Input) -> Result<String, String> {
    run(input)?.render()
}

/// Runs one request.
pub fn run(input: &Input) -> Result<Done, String> {
    let plan = input.plan()?;
    let files = merge_files(input)?;
    let resolved = resolve(
        input.path.as_deref(),
        &files,
        input.data_base64.as_deref(),
        input.name.as_deref(),
    )?;
    let mut cx = Cx::new(resolved.root.clone(), plan.output.clone(), plan.page_bytes);
    cx.format()?;
    if plan.output.file.is_some() && resolved.sources.len() > 1 && !plan.is_montage() {
        return Err(
            "output.file names one file; use output.suffix to write one file per image".into(),
        );
    }
    let mut call_warnings = Vec::new();
    if resolved.skipped > 0 {
        call_warnings.push(format!(
            "{} entries in the folder that are hidden, links or not images were left out",
            resolved.skipped
        ));
    }
    if resolved.too_deep > 0 {
        call_warnings.push(format!(
            "{} folders deeper than {} levels were not read; name their images in files to read them",
            resolved.too_deep,
            crate::src::MAX_DEPTH
        ));
    }
    let names: Vec<&str> = resolved.sources.iter().map(|s| s.name.as_str()).collect();
    let req = fingerprint(&json!({
        "names": names, "ops": plan.raw_ops, "output": plan.output, "frame": plan.frame,
        "orient": plan.orient, "page_bytes": plan.page_bytes,
    }));
    let (results, next) = if let Op::Montage(m) = &plan.ops[0] {
        if input.cursor.is_some() {
            return Err("a montage is one image and has no cursor".into());
        }
        (
            vec![run_montage(&mut cx, &resolved.sources, m, &plan)?],
            None,
        )
    } else {
        run_items(
            &mut cx,
            &resolved.sources,
            &plan,
            input.cursor.as_deref(),
            &req,
        )?
    };
    let mut response = json!({"results": results, "warnings": call_warnings});
    if let Some(c) = next {
        response["next"] =
            json!({"cursor": cursor::encode(&c), "source": resolved.sources[c.item].name});
    }
    Ok(Done {
        response,
        parts: cx.parts,
    })
}

/// `file` and `files` as one list.
fn merge_files(input: &Input) -> Result<Vec<String>, String> {
    match (&input.file, &input.files) {
        (Some(_), Some(_)) => Err("give file or files, not both".into()),
        (Some(f), None) => Ok(vec![f.clone()]),
        (None, Some(l)) => Ok(l.clone()),
        (None, None) => Ok(Vec::new()),
    }
}

fn run_items(
    cx: &mut Cx,
    sources: &[Source],
    plan: &Plan,
    cursor_text: Option<&str>,
    req: &str,
) -> Result<(Vec<Value>, Option<Cursor>), String> {
    let (mut idx, mut tile) = (0usize, 0usize);
    if let Some(text) = cursor_text {
        let c = cursor::decode(text, req, sources.len())?;
        if c.tile > 0 {
            let (len, sha) = file_identity(&sources[c.item])?;
            if (len, &sha) != (c.len, &c.sha) {
                return Err(format!(
                    "{} changed since the cursor was returned; start again without a cursor",
                    sources[c.item].name
                ));
            }
        }
        (idx, tile) = (c.item, c.tile);
    }
    let mut results: Vec<Value> = Vec::new();
    let mut next = None;
    while idx < sources.len() {
        if !results.is_empty() && (cx.expired() || cx.used() > cx.budget / 3 * 2) {
            next = Some(stop_at(&sources[idx], idx, 0)?);
            break;
        }
        let had_output = cx.used() > 0;
        let item = run_item(cx, &sources[idx], plan, tile);
        if item.over && had_output && !results.is_empty() {
            next = Some(stop_at(&sources[idx], idx, tile)?);
            break;
        }
        cx.json_bytes += item.json.to_string().len();
        results.push(item.json);
        if let Some(t) = item.next_tile {
            next = Some(stop_at(&sources[idx], idx, t)?);
            break;
        }
        idx += 1;
        tile = 0;
    }
    // The cursor of a stop inside an image is fixed by the caller's request, so fill the request digest in once.
    Ok((
        results,
        next.map(|mut c| {
            c.req = req.to_owned();
            c
        }),
    ))
}

/// A cursor for stopping before tile `tile` of source `idx`; inside an image it pins the file.
fn stop_at(src: &Source, idx: usize, tile: usize) -> Result<Cursor, String> {
    let (len, sha) = if tile > 0 {
        file_identity(src)?
    } else {
        (0, String::new())
    };
    Ok(Cursor {
        req: String::new(),
        item: idx,
        tile,
        len,
        sha,
    })
}

fn run_montage(
    cx: &mut Cx,
    sources: &[Source],
    m: &MontageOp,
    plan: &Plan,
) -> Result<Value, String> {
    let bg = m
        .background
        .as_deref()
        .map_or(Ok([255, 255, 255, 255]), parse_color)?;
    let spec = montage::Spec {
        cols: m.cols,
        cell: m.cell.unwrap_or(256),
        gap: m.gap.unwrap_or(8),
        labels: m.labels.unwrap_or(true),
        background: bg,
    };
    let sheet = montage::make(sources, &spec, plan.frame, plan.orient)?;
    let format = cx.format()?;
    let e = encode(&sheet.img, format, plan.output.quality, None)?;
    let mut warnings = sheet.warnings;
    warnings.extend(e.notes);
    let rec = match cx.deliver(Prepared::new(e.bytes, format, &sheet.img), "montage", None) {
        Ok(r) => r,
        Err(Fail::Msg(m)) => return Err(m),
        Err(Fail::Over(n)) => {
            return Err(format!(
                "the sheet is {n} bytes, over the {} bytes one call can attach; use a smaller cell, or write it to a file with output.file",
                cx.room()
            ));
        }
    };
    if let Some(n) = rec["not_attached"].as_str() {
        warnings.push(n.to_owned());
    }
    let cells: Vec<Value> = sheet
        .cells
        .iter()
        .map(|c| {
            let mut v = json!({
                "n": c.n, "source": c.source, "col": c.col, "row": c.row,
                "x": c.x, "y": c.y, "width": c.width, "height": c.height,
            });
            if let Some(e) = &c.error {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    let facts = json!({
        "op": "montage", "images": sources.len(),
        "cols": sheet.cells.iter().map(|c| c.col).max().map_or(0, |c| c + 1),
        "rows": sheet.cells.iter().map(|c| c.row).max().map_or(0, |r| r + 1),
        "cell": spec.cell, "gap": spec.gap,
        "width": sheet.img.w, "height": sheet.img.h,
        "cells": cells, "output": 0,
    });
    let mut rec = rec;
    rec["role"] = json!("montage");
    Ok(json!({"source": "montage", "steps": [facts], "outputs": [rec], "warnings": warnings}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(v: serde_json::Value) -> Input {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn file_and_files_become_one_list_and_cannot_be_mixed() {
        assert!(merge_files(&input(json!({}))).unwrap().is_empty());
        assert_eq!(
            merge_files(&input(json!({"file": "a.png"}))).unwrap(),
            ["a.png"]
        );
        assert_eq!(
            merge_files(&input(json!({"files": ["a", "b"]}))).unwrap(),
            ["a", "b"]
        );
        let e = merge_files(&input(json!({"file": "a", "files": ["b"]}))).unwrap_err();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn a_result_with_parts_wraps_the_response_and_one_without_does_not() {
        let plain = Done {
            response: json!({"results": []}),
            parts: vec![],
        };
        assert_eq!(plain.render().unwrap(), r#"{"results":[]}"#);
        let with = Done {
            response: json!({"results": []}),
            parts: vec![json!({"type": "image", "data": "AAAA", "mimeType": "image/png"})],
        };
        let v: serde_json::Value = serde_json::from_str(&with.render().unwrap()).unwrap();
        assert_eq!(v["response"], json!({"results": []}));
        assert_eq!(v["parts"][0]["mimeType"], "image/png");
    }

    #[test]
    fn a_result_over_the_output_ceiling_is_refused_in_one_sentence() {
        let huge = Done {
            response: json!({"results": ["x".repeat(OUTPUT_CEILING)]}),
            parts: vec![],
        };
        let e = huge.render().unwrap_err();
        assert!(
            e.contains("over the output limit") && e.contains("page_bytes"),
            "{e}"
        );
    }

    #[test]
    fn stopping_inside_an_image_pins_the_file_and_between_images_does_not() {
        let src = crate::src::inline("aGVsbG8=", Some("h.bin")).unwrap();
        let between = stop_at(&src, 2, 0).unwrap();
        assert_eq!(
            (
                between.item,
                between.tile,
                between.len,
                between.sha.as_str()
            ),
            (2, 0, 0, "")
        );
        let inside = stop_at(&src, 2, 5).unwrap();
        assert_eq!((inside.item, inside.tile, inside.len), (2, 5, 5));
        assert_eq!(inside.sha, src.sha256().unwrap());
    }
}
