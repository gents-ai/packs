//! Mutation fuzzing: every committed fixture is corrupted many times with a
//! fixed seed and run through the steps that read pixels. A panic, a hang or an
//! error that is not one plain sentence fails the test, with the fixture, the
//! mutation number and the bytes' digest in the message so it can be replayed.
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::testkit::{call, fixtures};

/// Multiplies the number of mutations; `IMAGE_TOOLS_FUZZ_SCALE=10 cargo test fuzz` digs deeper.
fn scale() -> usize {
    std::env::var("IMAGE_TOOLS_FUZZ_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1)
}

/// xorshift64*: a small deterministic generator, so the corruption is the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// One corrupted copy of `bytes`.
fn mutate(bytes: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = bytes.to_vec();
    if v.is_empty() {
        return v;
    }
    match rng.below(7) {
        0 => {
            for _ in 0..1 + rng.below(8) {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
        }
        1 => v.truncate(rng.below(v.len())),
        2 => {
            let at = rng.below(v.len());
            for _ in 0..1 + rng.below(16) {
                v.insert(at, rng.next() as u8);
            }
        }
        3 => {
            let (at, n) = (rng.below(v.len()), 1 + rng.below(64));
            for b in v.iter_mut().skip(at).take(n) {
                *b = 0;
            }
        }
        4 => {
            // The header region is where sizes live: scramble a few bytes of it.
            for _ in 0..1 + rng.below(4) {
                let i = rng.below(v.len().min(64));
                v[i] = rng.next() as u8;
            }
        }
        5 => {
            let (a, n) = (rng.below(v.len()), 1 + rng.below(32));
            let chunk: Vec<u8> = v.iter().skip(a).take(n).copied().collect();
            let at = rng.below(v.len());
            for (k, b) in chunk.into_iter().enumerate() {
                v.insert(at + k, b);
            }
        }
        _ => {
            let n = rng.below(v.len());
            v.drain(..n.min(v.len() - 1));
        }
    }
    v
}

fn fixture_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    all_files(Path::new(&fixtures()), &mut files);
    files.retain(|p| p.extension().is_some_and(|e| e != "txt"));
    files
}

#[test]
fn corrupted_fixtures_never_panic_hang_or_give_a_messy_error() {
    let started = Instant::now();
    let files = fixture_files();
    assert!(files.len() > 40, "the fixtures are there: {}", files.len());
    let mut runs = 0usize;
    let mut errors = 0usize;
    for (n, path) in files.iter().enumerate() {
        let original = std::fs::read(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ ((n as u64 + 1) << 20));
        for m in 0..150 * scale() {
            let bytes = mutate(&original, &mut rng);
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let digest = crate::model::sha256_hex(&bytes);
            let t = Instant::now();
            for ops in [
                json!([{"op": "info", "gps": true}]),
                json!([{"op": "view", "max_side": 48}]),
                json!([{"op": "hash"}, {"op": "palette"}, {"op": "decode_codes"}]),
                json!([{"op": "crop", "x": 0, "y": 0, "width": 4, "height": 4}, {"op": "rotate", "degrees": 90}, {"op": "strip_metadata"}]),
                json!([{"op": "tile", "size": 64, "overlap": 8, "max_bytes": 10000}]),
            ] {
                let r = call(
                    &json!({"data_base64": b64, "name": name, "ops": ops, "page_bytes": 200_000}),
                );
                runs += 1;
                let v = r.unwrap_or_else(|e| {
                    panic!("{name} mutation {m} ({digest}): the call failed: {e}")
                });
                let response = v.get("response").unwrap_or(&v);
                let rec = &response["results"][0];
                if let Some(e) = rec["error"].as_str() {
                    errors += 1;
                    assert!(
                        !e.is_empty() && !e.contains('\n') && e.len() < 300,
                        "{name} mutation {m} ({digest}): {e}"
                    );
                    let low = e.to_lowercase();
                    assert!(
                        !low.contains("panic") && !low.contains("unwrap") && !low.contains("src/"),
                        "{name} mutation {m} ({digest}): {e}"
                    );
                }
            }
            assert!(
                t.elapsed().as_secs() < 20,
                "{name} mutation {m} ({digest}) took {:?}",
                t.elapsed()
            );
        }
    }
    assert!(
        errors > runs / 10,
        "the mutations really do break files: {errors} of {runs}"
    );
    assert!(
        started.elapsed().as_secs() < 300 * scale() as u64,
        "fuzzing took {:?}",
        started.elapsed()
    );
}

#[test]
fn corrupted_fixtures_through_the_folder_path_stay_one_record_per_file() {
    // The same corrupt bytes read from a folder: every file still gets its own record.
    let dir = std::env::temp_dir().join(format!("image_tools_fuzz_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    let files = fixture_files();
    let mut names = Vec::new();
    for (i, p) in files.iter().enumerate().take(30) {
        let name = format!("m{i:02}.bin");
        std::fs::write(
            dir.join(&name),
            mutate(&std::fs::read(p).unwrap(), &mut rng),
        )
        .unwrap();
        names.push(name);
    }
    let r = call(&json!({"path": dir.to_str().unwrap(), "files": names, "op": "view", "max_side": 32, "page_bytes": 3_000_000})).unwrap();
    let mut seen = Vec::<String>::new();
    let mut cur = r.clone();
    loop {
        let response = cur.get("response").unwrap_or(&cur).clone();
        for rec in response["results"].as_array().unwrap() {
            seen.push(rec["source"].as_str().unwrap().to_owned());
        }
        match response["next"]["cursor"].as_str() {
            Some(c) => {
                cur = call(&json!({"path": dir.to_str().unwrap(), "files": names, "op": "view", "max_side": 32, "page_bytes": 3_000_000, "cursor": c})).unwrap();
            }
            None => break,
        }
    }
    assert_eq!(seen, names);
    let _: Value = r;
}

#[test]
fn damaged_code_pictures_never_panic_the_reader() {
    // Byte corruption mostly breaks the file, so damage the picture itself: noise, blocks, shifts, scale.
    let started = Instant::now();
    let mut rng = Rng(0xC0DE_C0DE_0BAD_F00D);
    let dir = format!("{}/codes", fixtures());
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    names.sort();
    let mut found = 0;
    for p in names {
        let base = image::open(&p).unwrap().to_rgba8();
        for _ in 0..40 * scale() {
            let mut img = base.clone();
            let (w, h) = img.dimensions();
            for _ in 0..rng.below(6) {
                let (x, y) = (rng.below(w as usize) as u32, rng.below(h as usize) as u32);
                let (bw, bh) = (1 + rng.below(30) as u32, 1 + rng.below(30) as u32);
                let v = rng.next() as u8;
                for yy in y..(y + bh).min(h) {
                    for xx in x..(x + bw).min(w) {
                        img.put_pixel(xx, yy, image::Rgba([v, v, v, 255]));
                    }
                }
            }
            for _ in 0..rng.below(400) {
                let (x, y) = (rng.below(w as usize) as u32, rng.below(h as usize) as u32);
                let v = rng.next() as u8;
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
            let (nw, nh) = (
                1 + rng.below(w as usize * 2) as u32,
                1 + rng.below(h as usize * 2) as u32,
            );
            let img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Nearest);
            let img =
                crate::model::Img::from_raw(img.width(), img.height(), img.into_raw()).unwrap();
            found += crate::codes::decode(&img, &[]).codes.len();
        }
    }
    assert!(found > 0, "some damaged codes are still read");
    // Pure noise and flat pictures of odd sizes.
    for i in 0..200 * scale() as u64 {
        let (w, h) = (1 + rng.below(300) as u32, 1 + rng.below(300) as u32);
        let img = if i % 3 == 0 {
            crate::model::Img::filled(w, h, [rng.next() as u8, 7, 9, 255]).unwrap()
        } else {
            let n = crate::fixtures::noise(w, h, i + 1);
            crate::model::Img::from_raw(w, h, n.into_raw()).unwrap()
        };
        let _ = crate::codes::decode(&img, &[]);
    }
    assert!(
        started.elapsed().as_secs() < 240 * scale() as u64,
        "{:?}",
        started.elapsed()
    );
}
