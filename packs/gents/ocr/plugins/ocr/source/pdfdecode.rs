//! Stream decoding for the lazy PDF reader: FlateDecode (or no filter) with a
//! PNG predictor undone, bounded by the reader's decoded-size cap.
use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::pdflazy::MAX_DECODED;
use crate::pdfobj::{Kind, Val};

/// Undoes a PNG predictor (10 to 15) over rows of `cols` samples.
fn unpredict(data: &[u8], colors: usize, bpc: usize, cols: usize) -> Result<Vec<u8>, String> {
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row = (colors * bpc * cols).div_ceil(8);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let (filter, line) = chunk.split_first().ok_or("a predictor row is empty")?;
        let mut cur = line.to_vec();
        cur.resize(row, 0);
        for i in 0..row {
            let left = if i >= bpp { cur[i - bpp] } else { 0 };
            let up = prev[i];
            let upleft = if i >= bpp { prev[i - bpp] } else { 0 };
            let add = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => {
                    let (a, b, c) = (i32::from(left), i32::from(up), i32::from(upleft));
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    (if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }) as u8
                }
                _ => return Err("a predictor row has an unknown filter".into()),
            };
            cur[i] = cur[i].wrapping_add(add);
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    Ok(out)
}

/// Decodes a stream whose filter is none or FlateDecode, with a PNG predictor.
pub fn decode_stream(dict: &Val, raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let unsupported = || "the file uses a stream filter the lazy reader does not read".to_string();
    let first = |v: &Val| -> Option<Val> {
        match &v.kind {
            Kind::Arr(items) => items.first().cloned(),
            _ => Some(v.clone()),
        }
    };
    let filters = match dict.get("Filter") {
        None => 0,
        Some(f) => match &f.kind {
            Kind::Arr(items) => items.len(),
            _ => 1,
        },
    };
    if filters == 0 {
        return Ok(raw);
    }
    let filter = dict.get("Filter").and_then(first).ok_or_else(unsupported)?;
    if filters > 1 || !matches!(filter.name(), Some(b"FlateDecode" | b"Fl")) {
        return Err(unsupported());
    }
    let mut out = Vec::new();
    let read = ZlibDecoder::new(&raw[..])
        .take(MAX_DECODED as u64 + 1)
        .read_to_end(&mut out);
    if out.len() > MAX_DECODED || (read.is_err() && out.is_empty()) {
        return Err("a stream does not inflate".into());
    }
    drop(raw);
    let parms = dict
        .get("DecodeParms")
        .or_else(|| dict.get("DP"))
        .and_then(first);
    let get = |k: &str, d: usize| {
        parms
            .as_ref()
            .and_then(|p| p.get(k))
            .and_then(Val::int)
            .map_or(d, |n| n.max(0) as usize)
    };
    match get("Predictor", 1) {
        1 => Ok(out),
        10..=15 => unpredict(
            &out,
            get("Colors", 1),
            get("BitsPerComponent", 8),
            get("Columns", 1),
        ),
        _ => Err(unsupported()),
    }
}
