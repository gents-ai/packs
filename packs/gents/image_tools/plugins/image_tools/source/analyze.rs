//! The steps that look at an image and report facts without changing it:
//! info, palette, decode_codes, hash and diff. Each returns the JSON record
//! of its step; diff also returns the highlight picture.
use serde_json::{Value, json};

use crate::codes;
use crate::decode::Header;
use crate::diff::{self, Spec};
use crate::hash::{self, dhash, hex16, phash};
use crate::input::{CodesOp, DiffOp, HashOp, InfoOp, Rect};
use crate::model::{Img, hex};
use crate::palette::palette;
use crate::src::Source;

/// `x` rounded to `places` decimals, as a number that prints the same everywhere.
pub fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

/// The `info` record: what the file says about itself.
pub fn info(h: &Header, op: &InfoOp) -> Value {
    let o = h.orientation();
    let (ow, oh) = if (5..=8).contains(&o) {
        (h.height, h.width)
    } else {
        (h.width, h.height)
    };
    let mut exif = json!({"present": h.exif.is_some(), "orientation": h.exif.and_then(|e| e.orientation), "gps": h.exif.is_some_and(|e| e.has_gps)});
    if op.gps && h.exif.is_some_and(|e| e.has_gps) {
        exif["position"] = match h.exif.and_then(|e| e.position) {
            Some(p) => json!({"lat": p.lat, "lon": p.lon}),
            None => Value::Null,
        };
    }
    json!({
        "op": "info",
        "format": h.format.name(),
        "width": h.width,
        "height": h.height,
        "oriented_width": ow,
        "oriented_height": oh,
        "color": h.color,
        "bit_depth": h.bit_depth,
        "has_alpha": h.has_alpha,
        "frames": h.frames,
        "bytes": h.bytes,
        "exif": exif,
        "icc": {"present": h.icc.is_some(), "bytes": h.icc.as_ref().map_or(0, Vec::len)},
        "xmp": h.xmp,
    })
}

/// The `palette` record.
pub fn palette_step(img: &Img, colors: u32) -> Value {
    let p = palette(img, colors as usize);
    let list: Vec<Value> = p
        .swatches
        .iter()
        .map(|s| {
            json!({
                "hex": format!("#{}", hex(&s.rgb)),
                "rgb": s.rgb,
                "share": round(s.pixels as f64 / p.counted.max(1) as f64, 4),
            })
        })
        .collect();
    let mut out = json!({"op": "palette", "colors": list, "pixels": p.counted});
    if p.transparent > 0 {
        out["transparent_share"] = json!(round(
            p.transparent as f64 / (u64::from(img.w) * u64::from(img.h)) as f64,
            4
        ));
    }
    out
}

/// The `decode_codes` record.
pub fn codes_step(img: &Img, op: &CodesOp) -> Result<Value, String> {
    let formats = codes::parse_formats(op.formats.as_deref().unwrap_or(&[]))?;
    let f = codes::decode(img, &formats);
    let list: Vec<Value> = f
        .codes
        .iter()
        .map(|c| json!({"format": c.format, "text": c.text, "points": c.points, "bbox": c.bbox}))
        .collect();
    let mut out =
        json!({"op": "decode_codes", "found": f.codes.len(), "attempt": f.attempt, "codes": list});
    if f.omitted > 0 {
        out["omitted"] = json!(f.omitted);
    }
    Ok(out)
}

/// The `hash` record; `other` is the image to compare against, with its source.
pub fn hash_step(
    src: &Source,
    img: &Img,
    op: &HashOp,
    other: Option<(&Source, &Img)>,
) -> Result<Value, String> {
    let (ph, dh) = (phash(img), dhash(img));
    let sha = src.sha256()?;
    let px = crate::model::sha256_hex(&img.px);
    let mut out = json!({
        "op": "hash", "sha256": sha, "pixels_sha256": px, "width": img.w, "height": img.h,
        "phash": hex16(ph), "dhash": hex16(dh),
    });
    if let Some((osrc, oimg)) = other {
        let threshold = op.threshold.unwrap_or(10);
        let (pd, dd) = (
            hash::distance(ph, phash(oimg)),
            hash::distance(dh, dhash(oimg)),
        );
        out["against"] = json!({
            "source": osrc.name,
            "identical_bytes": osrc.sha256()? == sha,
            "identical_pixels": crate::model::sha256_hex(&oimg.px) == px && (oimg.w, oimg.h) == (img.w, img.h),
            "phash_distance": pd,
            "dhash_distance": dd,
            "threshold": threshold,
            "similar": pd <= threshold,
        });
    }
    Ok(out)
}

/// The `diff` record and the highlight picture, when asked for.
pub fn diff_step(
    a: &Img,
    b: &Img,
    op: &DiffOp,
    against: &str,
) -> Result<(Value, Option<Img>), String> {
    let ignore: &[Rect] = &op.ignore;
    let spec = Spec {
        tolerance: op.tolerance.unwrap_or(0),
        ignore,
        merge_gap: op.merge_gap.unwrap_or(8),
    };
    let o = diff::compare(a, b, &spec);
    let regions: Vec<Value> = o
        .regions
        .iter()
        .map(|r| json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height, "changed": r.changed}))
        .collect();
    let ratio = if o.compared == 0 {
        0.0
    } else {
        round(o.changed as f64 / o.compared as f64, 6)
    };
    let mut out = json!({
        "op": "diff",
        "against": against,
        "same_size": o.same_size,
        "width": o.width,
        "height": o.height,
        "tolerance": spec.tolerance,
        "compared": o.compared,
        "changed": o.changed,
        "changed_ratio": ratio,
        "identical": o.changed == 0 && o.same_size,
        "ssim": o.ssim.map(|s| round(s, 4)),
        "regions_total": o.regions_total,
        "regions": regions,
    });
    if o.regions_total > o.regions.len() {
        out["regions_listed"] = json!(o.regions.len());
    }
    let image = if op.highlight.unwrap_or(true) {
        Some(diff::highlight(a, &o)?)
    } else {
        None
    };
    Ok((out, image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::header;
    use crate::fixtures as fx;
    use crate::src::Data;
    use std::sync::Arc;

    fn mem(bytes: Vec<u8>) -> Source {
        Source {
            name: "t".into(),
            data: Data::Mem(Arc::new(bytes)),
        }
    }

    #[test]
    fn info_reports_the_header_and_hides_gps_unless_asked() {
        let img = fx::scene(16, 12);
        let exif = fx::exif_block(Some(6), Some((48.8566, 2.3522)));
        let h = header(&mem(fx::jpeg_with_exif(&fx::jpeg(&img, 80), &exif))).unwrap();
        let v = info(&h, &InfoOp { gps: false });
        assert_eq!(
            (v["width"].clone(), v["height"].clone()),
            (json!(16), json!(12))
        );
        assert_eq!(
            (v["oriented_width"].clone(), v["oriented_height"].clone()),
            (json!(12), json!(16))
        );
        assert_eq!(
            v["exif"],
            json!({"present": true, "orientation": 6, "gps": true})
        );
        assert_eq!(v["format"], "jpeg");
        assert_eq!(v["frames"], 1);
        assert_eq!(v["icc"], json!({"present": false, "bytes": 0}));
        let v = info(&h, &InfoOp { gps: true });
        assert_eq!(
            v["exif"]["position"],
            json!({"lat": 48.8566, "lon": 2.3522})
        );
        let plain = header(&mem(fx::png(&img))).unwrap();
        let v = info(&plain, &InfoOp { gps: true });
        assert_eq!(
            v["exif"],
            json!({"present": false, "orientation": null, "gps": false})
        );
        assert_eq!(
            (
                v["color"].clone(),
                v["bit_depth"].clone(),
                v["has_alpha"].clone()
            ),
            (json!("rgba8"), json!(8), json!(true))
        );
    }

    #[test]
    fn palette_step_formats_hex_share_and_transparency() {
        let mut img = Img::filled(4, 1, [255, 0, 0, 255]).unwrap();
        img.px[4..8].copy_from_slice(&[0, 0, 255, 255]);
        img.px[8..12].copy_from_slice(&[0, 0, 255, 255]);
        img.px[12..16].copy_from_slice(&[9, 9, 9, 0]);
        let v = palette_step(&img, 4);
        assert_eq!(v["pixels"], 3);
        assert_eq!(
            v["colors"][0],
            json!({"hex": "#0000ff", "rgb": [0, 0, 255], "share": 0.6667})
        );
        assert_eq!(
            v["colors"][1],
            json!({"hex": "#ff0000", "rgb": [255, 0, 0], "share": 0.3333})
        );
        assert_eq!(v["transparent_share"], 0.25);
    }

    #[test]
    fn hash_step_compares_two_images() {
        let a = Img::from_raw(64, 48, fx::scene(64, 48).into_raw()).unwrap();
        let sa = mem(fx::png(&fx::scene(64, 48)));
        let v = hash_step(
            &sa,
            &a,
            &HashOp {
                against: None,
                against_base64: None,
                threshold: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(v["phash"].as_str().unwrap().len(), 16);
        assert_eq!(v["sha256"], sa.sha256().unwrap());
        assert!(v.get("against").is_none());
        let v = hash_step(
            &sa,
            &a,
            &HashOp {
                against: None,
                against_base64: None,
                threshold: Some(3),
            },
            Some((&sa, &a)),
        )
        .unwrap();
        assert_eq!(v["against"]["identical_bytes"], true);
        assert_eq!(v["against"]["identical_pixels"], true);
        assert_eq!(
            (
                v["against"]["phash_distance"].clone(),
                v["against"]["dhash_distance"].clone()
            ),
            (json!(0), json!(0))
        );
        assert_eq!(
            (
                v["against"]["similar"].clone(),
                v["against"]["threshold"].clone()
            ),
            (json!(true), json!(3))
        );
        let n = Img::from_raw(64, 48, fx::noise(64, 48, 3).into_raw()).unwrap();
        let sn = mem(fx::png(&fx::noise(64, 48, 3)));
        let v = hash_step(
            &sa,
            &a,
            &HashOp {
                against: None,
                against_base64: None,
                threshold: None,
            },
            Some((&sn, &n)),
        )
        .unwrap();
        assert_eq!(v["against"]["identical_bytes"], false);
        assert_eq!(v["against"]["similar"], false);
    }

    #[test]
    fn diff_step_reports_ratio_regions_and_a_highlight() {
        let a = Img::filled(40, 30, [255, 255, 255, 255]).unwrap();
        let mut b = a.clone();
        for y in 10..16 {
            for x in 10..16 {
                let i = b.at(x, y);
                b.px[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let op = DiffOp {
            against: Some("b.png".into()),
            against_base64: None,
            tolerance: None,
            ignore: vec![],
            merge_gap: None,
            highlight: None,
        };
        let (v, hl) = diff_step(&a, &b, &op, "b.png").unwrap();
        assert_eq!(v["changed"], 36);
        assert_eq!(v["compared"], 1200);
        assert_eq!(v["changed_ratio"], 0.03);
        assert_eq!(v["identical"], false);
        assert_eq!(
            v["regions"],
            json!([{"x": 10, "y": 10, "width": 6, "height": 6, "changed": 36}])
        );
        assert!(hl.is_some());
        let off = DiffOp {
            highlight: Some(false),
            ..op
        };
        assert!(diff_step(&a, &b, &off, "b.png").unwrap().1.is_none());
        let same = diff_step(&a, &a.clone(), &off, "x").unwrap().0;
        assert_eq!(
            (
                same["identical"].clone(),
                same["ssim"].clone(),
                same["changed_ratio"].clone()
            ),
            (json!(true), json!(1.0), json!(0.0))
        );
    }

    #[test]
    fn rounding_is_to_fixed_places() {
        assert_eq!(round(0.123456789, 4), 0.1235);
        assert_eq!(round(2.0 / 3.0, 6), 0.666667);
        assert_eq!(round(1.0, 4), 1.0);
    }
}
