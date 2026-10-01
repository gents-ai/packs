//! Plan mode: what each file holds and how to read it in pieces, without
//! reading its content. A file made of pages, slides, sheets or sections is
//! split into `pages` ranges that cover every unit exactly once; any other
//! file is read by following the cursor.
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::detect::{self, Kind};
use crate::input::Options;
use crate::model::json_len;
use crate::src::Src;
use crate::{Source, epub, odf, pdf, pptx, xlsx};

/// A file is split into at most this many ranges; larger ranges are used past it.
const MAX_CHUNKS: u32 = 64;
/// Units per range for pages, slides and sections.
const UNITS_PER_CHUNK: u32 = 20;

#[derive(Serialize)]
struct Chunk {
    pages: String,
}

#[derive(Serialize)]
struct Entry {
    source: String,
    format: &'static str,
    bytes: u64,
    /// `pages` when the ranges in `chunks` can be passed as the pages option;
    /// `cursor` when the file is read by following `next.cursor`.
    read: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    chunks: Vec<Chunk>,
    /// A rough number of calls a cursor read takes, from the file size.
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_calls: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Plan {
    documents: Vec<Entry>,
    /// Files left out because the plan reached its size or time limit.
    #[serde(skip_serializing_if = "is_zero")]
    omitted: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Ranges of `per` units that cover `1..=count` once each, widened so there are at most [`MAX_CHUNKS`].
fn ranges(count: u32, per: u32) -> Vec<Chunk> {
    let per = per.max(count.div_ceil(MAX_CHUNKS)).max(1);
    (1..=count)
        .step_by(per as usize)
        .map(|lo| {
            let hi = (lo + per - 1).min(count);
            Chunk {
                pages: if lo == hi {
                    lo.to_string()
                } else {
                    format!("{lo}-{hi}")
                },
            }
        })
        .collect()
}

/// What a file is made of: the unit's name, how many, and units per range.
fn units(kind: Kind, src: &mut Src) -> Result<Option<(&'static str, u32, u32)>, String> {
    Ok(match kind {
        Kind::Pdf => Some(("page", pdf::page_count(src)?, UNITS_PER_CHUNK)),
        Kind::Epub => Some(("section", epub::section_count(src)?, UNITS_PER_CHUNK)),
        Kind::Pptx => Some(("slide", pptx::slide_count(src)?, UNITS_PER_CHUNK)),
        Kind::Odp => Some(("slide", odf::unit_count(kind, src)?, UNITS_PER_CHUNK)),
        Kind::Xlsx => Some(("sheet", xlsx::sheet_count(src)?, 1)),
        Kind::Ods => Some(("sheet", odf::unit_count(kind, src)?, 1)),
        _ => None,
    })
}

fn describe(source: Source, opts: &Options) -> Entry {
    let name = source.name.clone();
    let failed = |format, bytes, why: String| Entry {
        source: name.clone(),
        format,
        bytes,
        read: "cursor",
        unit: None,
        count: None,
        chunks: Vec::new(),
        estimated_calls: None,
        error: Some(why),
    };
    let mut src = match source.open() {
        Ok(s) => s,
        Err(why) => return failed("unknown", 0, why),
    };
    let bytes = src.len();
    let kind = match detect::detect(&name, &mut src) {
        Ok(k) => k,
        Err(why) => return failed("unknown", bytes, why),
    };
    match units(kind, &mut src) {
        Ok(Some((unit, count, per))) => Entry {
            source: name,
            format: kind.name(),
            bytes,
            read: "pages",
            unit: Some(unit),
            count: Some(count),
            chunks: ranges(count, per),
            estimated_calls: None,
            error: None,
        },
        Ok(None) => Entry {
            source: name,
            format: kind.name(),
            bytes,
            read: "cursor",
            unit: None,
            count: None,
            chunks: Vec::new(),
            estimated_calls: Some(bytes.div_ceil(opts.max_bytes as u64).max(1)),
            error: None,
        },
        Err(why) => failed(kind.name(), bytes, why),
    }
}

pub fn run(opts: &Options, sources: Vec<Source>, started: Instant) -> Result<String, String> {
    let limit = Duration::from_secs(opts.max_seconds);
    let mut documents = Vec::new();
    let (mut used, mut omitted) = (2usize, 0usize);
    for source in sources {
        if used >= opts.max_bytes || started.elapsed() >= limit {
            omitted += 1;
            continue;
        }
        let entry = describe(source, opts);
        let json =
            serde_json::to_string(&entry).map_err(|e| format!("serializing the plan: {e}"))?;
        used += json_len(&json);
        documents.push(entry);
    }
    serde_json::to_string(&Plan { documents, omitted })
        .map_err(|e| format!("serializing the plan: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_every_unit_once_and_stay_bounded() {
        for count in [1u32, 19, 20, 21, 100, 1281, 1_000_000] {
            let chunks = ranges(count, 20);
            assert!(chunks.len() <= MAX_CHUNKS as usize, "{count}");
            let mut next = 1u32;
            for c in &chunks {
                let (lo, hi): (u32, u32) = c.pages.split_once('-').map_or_else(
                    || (c.pages.parse().unwrap(), c.pages.parse().unwrap()),
                    |(a, b)| (a.parse().unwrap(), b.parse().unwrap()),
                );
                assert_eq!(lo, next, "{count}");
                assert!(hi >= lo);
                next = hi + 1;
            }
            assert_eq!(next, count + 1, "{count}");
        }
        assert_eq!(
            ranges(3, 1)
                .iter()
                .map(|c| c.pages.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
    }
}
