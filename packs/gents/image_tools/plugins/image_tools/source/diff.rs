//! Comparing two images for visual regression: the share of changed pixels,
//! a structural similarity score, the bounding boxes of the changed regions
//! and a highlighted picture. When the sizes differ the comparison runs over
//! the larger canvas and every pixel one image does not cover counts as
//! changed; the similarity score is then not defined.
use crate::draw;
use crate::hash::luma_milli;
use crate::input::Rect;
use crate::model::Img;

/// Block size, in pixels, of the grid regions are grown on.
const BLOCK: u32 = 8;
/// Regions listed in a result; the rest are counted.
pub const MAX_REGIONS: usize = 200;
/// Window side and step of the similarity score.
const WINDOW: u32 = 8;
const STEP: u32 = 4;

/// What counts as a change.
pub struct Spec<'a> {
    /// A pixel changed when any channel differs by more than this.
    pub tolerance: u8,
    /// Regions left out of the comparison.
    pub ignore: &'a [Rect],
    /// Changed areas closer than this many pixels merge into one region.
    pub merge_gap: u32,
}

/// A rectangle of changed pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Changed pixels inside the rectangle.
    pub changed: u64,
}

/// The outcome of a comparison.
pub struct Outcome {
    /// Canvas width: the larger of the two images.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
    /// Whether both images have the same size.
    pub same_size: bool,
    /// Pixels compared: the canvas minus the ignored area.
    pub compared: u64,
    /// Pixels that changed.
    pub changed: u64,
    /// Structural similarity from 0 to 1, `None` when the sizes differ.
    pub ssim: Option<f64>,
    /// The listed regions, top to bottom then left to right.
    pub regions: Vec<Region>,
    /// All regions found, which can exceed the listed ones.
    pub regions_total: usize,
    mask: Vec<u8>,
}

fn ignored(canvas: (u32, u32), rects: &[Rect]) -> Vec<bool> {
    let (w, h) = canvas;
    let mut mask = vec![false; w as usize * h as usize];
    for r in rects {
        let x1 = (u64::from(r.x) + u64::from(r.width)).min(u64::from(w)) as u32;
        let y1 = (u64::from(r.y) + u64::from(r.height)).min(u64::from(h)) as u32;
        for y in r.y.min(h)..y1 {
            let row = y as usize * w as usize;
            for x in r.x.min(w)..x1 {
                mask[row + x as usize] = true;
            }
        }
    }
    mask
}

/// Compares `a` (the reference) with `b`.
pub fn compare(a: &Img, b: &Img, spec: &Spec) -> Outcome {
    let (w, h) = (a.w.max(b.w), a.h.max(b.h));
    let skip = ignored((w, h), spec.ignore);
    let mut mask = vec![0u8; w as usize * h as usize];
    let (mut changed, mut compared) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let at = y as usize * w as usize + x as usize;
            if skip[at] {
                continue;
            }
            compared += 1;
            let in_a = x < a.w && y < a.h;
            let in_b = x < b.w && y < b.h;
            let differs = match (in_a, in_b) {
                (true, true) => {
                    let (pa, pb) = (a.at(x, y), b.at(x, y));
                    a.px[pa..pa + 4]
                        .iter()
                        .zip(&b.px[pb..pb + 4])
                        .any(|(p, q)| p.abs_diff(*q) > spec.tolerance)
                }
                _ => true,
            };
            if differs {
                mask[at] = 1;
                changed += 1;
            }
        }
    }
    let same_size = (a.w, a.h) == (b.w, b.h);
    let ssim = same_size.then(|| similarity(a, b, &skip));
    let mut regions = find_regions(&mask, w, h, spec.merge_gap);
    let regions_total = regions.len();
    if regions.len() > MAX_REGIONS {
        regions.sort_by(|p, q| q.changed.cmp(&p.changed).then((p.y, p.x).cmp(&(q.y, q.x))));
        regions.truncate(MAX_REGIONS);
    }
    regions.sort_by_key(|r| (r.y, r.x, r.height, r.width));
    Outcome {
        width: w,
        height: h,
        same_size,
        compared,
        changed,
        ssim,
        regions,
        regions_total,
        mask,
    }
}

/// Mean structural similarity over 8x8 windows every 4 pixels, on luma.
/// Ignored pixels read as equal in both images.
fn similarity(a: &Img, b: &Img, skip: &[bool]) -> f64 {
    let luma = |img: &Img, x: u32, y: u32| luma_milli(&img.px[img.at(x, y)..]) / 1000;
    let (w, h) = (a.w, a.h);
    let win = WINDOW.min(w).min(h);
    let (c1, c2) = (6.5025f64, 58.5225f64);
    let (mut total, mut windows) = (0.0f64, 0u64);
    let mut y = 0;
    loop {
        let mut x = 0;
        loop {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for yy in y..y + win {
                for xx in x..x + win {
                    let va = luma(a, xx, yy);
                    let vb = if skip[yy as usize * w as usize + xx as usize] {
                        va
                    } else {
                        luma(b, xx, yy)
                    };
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let n = f64::from(win * win);
            let (ma, mb) = (sa as f64 / n, sb as f64 / n);
            let (va, vb) = (saa as f64 / n - ma * ma, sbb as f64 / n - mb * mb);
            let cov = sab as f64 / n - ma * mb;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            windows += 1;
            if x + win >= w {
                break;
            }
            x = (x + STEP).min(w - win);
        }
        if y + win >= h {
            break;
        }
        y = (y + STEP).min(h - win);
    }
    (total / windows as f64).clamp(0.0, 1.0)
}

/// Groups changed pixels into rectangles: changed 8x8 blocks closer than `gap` pixels join one region.
fn find_regions(mask: &[u8], w: u32, h: u32, gap: u32) -> Vec<Region> {
    let (bw, bh) = (w.div_ceil(BLOCK) as usize, h.div_ceil(BLOCK) as usize);
    // Per block: the bounds and count of its changed pixels.
    let mut blocks: Vec<Option<[u32; 5]>> = vec![None; bw * bh];
    for y in 0..h {
        for x in 0..w {
            if mask[y as usize * w as usize + x as usize] == 0 {
                continue;
            }
            let slot = &mut blocks[(y / BLOCK) as usize * bw + (x / BLOCK) as usize];
            let e = slot.get_or_insert([x, y, x, y, 0]);
            e[0] = e[0].min(x);
            e[1] = e[1].min(y);
            e[2] = e[2].max(x);
            e[3] = e[3].max(y);
            e[4] += 1;
        }
    }
    let reach = (1 + gap / BLOCK) as i64;
    let mut seen = vec![false; bw * bh];
    let mut out = Vec::new();
    for start in 0..bw * bh {
        if blocks[start].is_none() || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut bounds = [u32::MAX, u32::MAX, 0, 0];
        let mut changed = 0u64;
        while let Some(cur) = stack.pop() {
            if let Some(e) = blocks[cur] {
                bounds = [
                    bounds[0].min(e[0]),
                    bounds[1].min(e[1]),
                    bounds[2].max(e[2]),
                    bounds[3].max(e[3]),
                ];
                changed += u64::from(e[4]);
            }
            let (cx, cy) = ((cur % bw) as i64, (cur / bw) as i64);
            for ny in (cy - reach).max(0)..=(cy + reach).min(bh as i64 - 1) {
                for nx in (cx - reach).max(0)..=(cx + reach).min(bw as i64 - 1) {
                    let n = ny as usize * bw + nx as usize;
                    if blocks[n].is_some() && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        out.push(Region {
            x: bounds[0],
            y: bounds[1],
            width: bounds[2] - bounds[0] + 1,
            height: bounds[3] - bounds[1] + 1,
            changed,
        });
    }
    out
}

/// The reference faded to a pale ghost with changed pixels in red and each region boxed in magenta.
pub fn highlight(a: &Img, o: &Outcome) -> Result<Img, String> {
    let mut out = Img::filled(o.width, o.height, [255, 255, 255, 255])?;
    for y in 0..a.h {
        for x in 0..a.w {
            let p = a.get(x, y);
            let at = out.at(x, y);
            let alpha = u32::from(p[3]);
            for (dst, &src) in out.px[at..at + 3].iter_mut().zip(&p[..3]) {
                let on_white = (u32::from(src) * alpha + 255 * (255 - alpha) + 127) / 255;
                *dst = ((on_white * 3 + 255 * 7 + 5) / 10) as u8;
            }
        }
    }
    for (i, m) in o.mask.iter().enumerate() {
        if *m == 1 {
            out.px[i * 4..i * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    for r in &o.regions {
        draw::rect_outline(
            &mut out,
            i64::from(r.x) - 1,
            i64::from(r.y) - 1,
            r.width + 2,
            r.height + 2,
            1,
            [255, 0, 255, 255],
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(tolerance: u8, ignore: &'a [Rect], merge_gap: u32) -> Spec<'a> {
        Spec {
            tolerance,
            ignore,
            merge_gap,
        }
    }

    fn white(w: u32, h: u32) -> Img {
        Img::filled(w, h, [255, 255, 255, 255]).unwrap()
    }

    fn paint(img: &mut Img, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                let i = img.at(xx, yy);
                img.px[i..i + 4].copy_from_slice(&c);
            }
        }
    }

    #[test]
    fn identical_images_have_no_change_no_regions_and_similarity_one() {
        let a = white(40, 30);
        let o = compare(&a, &a.clone(), &spec(0, &[], 8));
        assert_eq!((o.changed, o.compared, o.regions_total), (0, 1200, 0));
        assert_eq!(o.ssim, Some(1.0));
        assert!(o.same_size);
    }

    #[test]
    fn a_painted_rectangle_is_found_exactly() {
        let a = white(64, 48);
        let mut b = a.clone();
        paint(&mut b, 10, 12, 20, 9, [0, 0, 0, 255]);
        let o = compare(&a, &b, &spec(0, &[], 8));
        assert_eq!(o.changed, 180);
        assert_eq!(
            o.regions,
            vec![Region {
                x: 10,
                y: 12,
                width: 20,
                height: 9,
                changed: 180
            }]
        );
        let ssim = o.ssim.unwrap();
        assert!(ssim < 1.0 && ssim > 0.5, "{ssim}");
    }

    #[test]
    fn tolerance_hides_small_differences_but_not_larger_ones() {
        let a = Img::filled(16, 16, [100, 100, 100, 255]).unwrap();
        let b = Img::filled(16, 16, [105, 100, 100, 255]).unwrap();
        assert_eq!(compare(&a, &b, &spec(4, &[], 8)).changed, 256);
        assert_eq!(compare(&a, &b, &spec(5, &[], 8)).changed, 0);
        let c = Img::filled(16, 16, [100, 100, 100, 200]).unwrap();
        assert_eq!(
            compare(&a, &c, &spec(54, &[], 8)).changed,
            256,
            "alpha counts as a channel"
        );
        assert_eq!(compare(&a, &c, &spec(55, &[], 8)).changed, 0);
    }

    #[test]
    fn ignored_regions_are_not_compared_and_shrink_the_denominator() {
        let a = white(20, 10);
        let mut b = a.clone();
        paint(&mut b, 0, 0, 10, 10, [0, 0, 0, 255]);
        let ignore = [Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }];
        let o = compare(&a, &b, &spec(0, &ignore, 8));
        assert_eq!((o.changed, o.compared, o.regions_total), (0, 100, 0));
        assert_eq!(o.ssim, Some(1.0), "ignored pixels read as equal");
        let half = [Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        }];
        let o = compare(&a, &b, &spec(0, &half, 8));
        assert_eq!((o.changed, o.compared), (50, 150));
        // An ignore box past the edge is clipped, not an error.
        let far = [Rect {
            x: 15,
            y: 5,
            width: 100,
            height: 100,
        }];
        assert_eq!(compare(&a, &b, &spec(0, &far, 8)).compared, 200 - 25);
    }

    #[test]
    fn regions_merge_within_the_gap_and_stay_apart_beyond_it() {
        let a = white(120, 40);
        let mut b = a.clone();
        paint(&mut b, 8, 8, 8, 8, [0, 0, 0, 255]);
        paint(&mut b, 28, 8, 8, 8, [0, 0, 0, 255]);
        paint(&mut b, 100, 30, 4, 4, [0, 0, 0, 255]);
        let near = compare(&a, &b, &spec(0, &[], 16));
        assert_eq!(
            near.regions
                .iter()
                .map(|r| (r.x, r.y, r.width, r.height))
                .collect::<Vec<_>>(),
            vec![(8, 8, 28, 8), (100, 30, 4, 4)],
            "the first two are 12 px apart and merge"
        );
        let far = compare(&a, &b, &spec(0, &[], 0));
        assert_eq!(far.regions_total, 3);
        assert_eq!(far.regions[0].x, 8);
        assert_eq!(far.regions[1].x, 28);
        assert_eq!(far.regions[2].x, 100);
    }

    /// Two full blocks `empty` empty 8 px blocks apart, as the regions found at `merge_gap`.
    fn pair_regions(empty: u32, merge_gap: u32) -> Vec<(u32, u32, u32, u32)> {
        let a = white(8 * (2 + empty), 8);
        let mut b = a.clone();
        paint(&mut b, 0, 0, 8, 8, [0, 0, 0, 255]);
        paint(&mut b, 8 * (1 + empty), 0, 8, 8, [0, 0, 0, 255]);
        let o = compare(&a, &b, &spec(0, &[], merge_gap));
        assert_eq!(o.regions_total, o.regions.len());
        o.regions
            .iter()
            .map(|r| (r.x, r.y, r.width, r.height))
            .collect()
    }

    #[test]
    fn the_merge_gap_is_exact_at_each_block_boundary() {
        // One empty block (8 px) between: apart below a gap of 8, joined from 8 up.
        let one = |gap| pair_regions(1, gap).len();
        assert_eq!(
            [0, 7, 8, 15, 16, 24].map(one),
            [2, 2, 1, 1, 1, 1],
            "gaps 0, 7, 8, 15, 16, 24"
        );
        assert_eq!(pair_regions(1, 8), vec![(0, 0, 24, 8)]);
        // Two empty blocks (16 px) between: apart through 15, joined from 16 up.
        let two = |gap| pair_regions(2, gap).len();
        assert_eq!([0, 8, 15, 16, 23, 24].map(two), [2, 2, 2, 1, 1, 1]);
        // Touching blocks join even with no gap.
        assert_eq!(pair_regions(0, 0), vec![(0, 0, 16, 8)]);
    }

    #[test]
    fn the_cap_drops_the_smallest_region_and_keeps_the_rest_in_reading_order() {
        // 201 changed spots, 16 px apart: all 2 px wide except one 1 px spot in the middle.
        let n = MAX_REGIONS as u32 + 1;
        let a = white(16 * n, 16);
        let mut b = a.clone();
        for i in 0..n {
            paint(
                &mut b,
                i * 16,
                0,
                if i == 100 { 1 } else { 2 },
                1,
                [0, 0, 0, 255],
            );
        }
        let o = compare(&a, &b, &spec(0, &[], 0));
        assert_eq!(o.regions_total, MAX_REGIONS + 1);
        assert_eq!(o.regions.len(), MAX_REGIONS);
        assert!(
            o.regions.iter().all(|r| r.changed == 2),
            "the one-pixel region is the one dropped"
        );
        assert!(!o.regions.iter().any(|r| r.x == 1600));
        assert_eq!(o.regions[99].x, 99 * 16);
        assert_eq!(
            o.regions[100].x,
            101 * 16,
            "reading order skips only the dropped one"
        );
        assert_eq!(
            o.changed,
            2 * MAX_REGIONS as u64 + 1,
            "the pixel count still includes it"
        );
        // Equal sizes tie-break on position: the last in reading order is dropped.
        let mut c = a.clone();
        for i in 0..n {
            paint(&mut c, i * 16, 0, 2, 1, [0, 0, 0, 255]);
        }
        let o = compare(&a, &c, &spec(0, &[], 0));
        assert_eq!(o.regions.last().map(|r| r.x), Some(199 * 16));
    }

    #[test]
    fn regions_are_listed_top_to_bottom_then_left_to_right() {
        let a = white(100, 100);
        let mut b = a.clone();
        paint(&mut b, 80, 10, 3, 3, [0, 0, 0, 255]);
        paint(&mut b, 10, 10, 3, 3, [0, 0, 0, 255]);
        paint(&mut b, 50, 80, 3, 3, [0, 0, 0, 255]);
        let o = compare(&a, &b, &spec(0, &[], 0));
        let at: Vec<_> = o.regions.iter().map(|r| (r.y, r.x)).collect();
        assert_eq!(at, vec![(10, 10), (10, 80), (80, 50)]);
    }

    #[test]
    fn more_regions_than_the_cap_keep_the_largest_and_count_them_all() {
        let a = white(16 * 40, 16);
        let mut b = a.clone();
        for i in 0..40 {
            paint(&mut b, i * 16, 0, 1 + (i % 3), 1, [0, 0, 0, 255]);
        }
        let o = compare(&a, &b, &spec(0, &[], 0));
        assert_eq!(o.regions_total, 40);
        assert_eq!(o.regions.len(), 40, "under the cap everything is listed");
        let mut wide = white(16 * 250, 16);
        let base = wide.clone();
        for i in 0..250 {
            paint(&mut wide, i * 16, 0, 1, 1, [0, 0, 0, 255]);
        }
        let o = compare(&base, &wide, &spec(0, &[], 0));
        assert_eq!(o.regions_total, 250);
        assert_eq!(o.regions.len(), MAX_REGIONS);
        assert!(
            o.regions
                .windows(2)
                .all(|w| (w[0].y, w[0].x) <= (w[1].y, w[1].x))
        );
    }

    #[test]
    fn different_sizes_compare_over_the_larger_canvas() {
        let a = white(10, 10);
        let b = white(14, 10);
        let o = compare(&a, &b, &spec(0, &[], 8));
        assert!(!o.same_size);
        assert_eq!((o.width, o.height), (14, 10));
        assert_eq!(o.changed, 40, "the 4 extra columns are all changed");
        assert_eq!(o.compared, 140);
        assert_eq!(o.ssim, None);
        assert_eq!(
            o.regions,
            vec![Region {
                x: 10,
                y: 0,
                width: 4,
                height: 10,
                changed: 40
            }]
        );
    }

    #[test]
    fn diff_is_symmetric_in_what_it_counts() {
        let a = crate::fixtures::scene(48, 40);
        let a = Img::from_raw(48, 40, a.into_raw()).unwrap();
        let mut b = a.clone();
        paint(&mut b, 5, 5, 11, 7, [1, 2, 3, 255]);
        let ab = compare(&a, &b, &spec(0, &[], 8));
        let ba = compare(&b, &a, &spec(0, &[], 8));
        assert_eq!(ab.changed, ba.changed);
        assert_eq!(ab.regions, ba.regions);
        assert_eq!(ab.ssim, ba.ssim);
    }

    #[test]
    fn similarity_orders_pictures_by_how_alike_they_are() {
        let a = crate::fixtures::scene(64, 64);
        let a = Img::from_raw(64, 64, a.into_raw()).unwrap();
        let mut slight = a.clone();
        paint(&mut slight, 0, 0, 4, 4, [0, 0, 0, 255]);
        let n = crate::fixtures::noise(64, 64, 9);
        let noise = Img::from_raw(64, 64, n.into_raw()).unwrap();
        let s = |x: &Img| compare(&a, x, &spec(0, &[], 8)).ssim.unwrap();
        assert_eq!(s(&a), 1.0);
        assert!(s(&slight) > 0.97 && s(&slight) < 1.0, "{}", s(&slight));
        assert!(s(&noise) < 0.2, "{}", s(&noise));
    }

    #[test]
    fn a_tiny_image_is_scored_over_one_window() {
        let a = white(3, 2);
        let mut b = a.clone();
        paint(&mut b, 0, 0, 3, 2, [0, 0, 0, 255]);
        let o = compare(&a, &b, &spec(0, &[], 8));
        assert!(o.ssim.unwrap() < 0.01);
        assert_eq!(compare(&a, &a.clone(), &spec(0, &[], 8)).ssim, Some(1.0));
    }

    #[test]
    fn the_highlight_marks_changes_red_and_boxes_them() {
        let a = white(40, 30);
        let mut b = a.clone();
        paint(&mut b, 10, 10, 6, 6, [0, 0, 0, 255]);
        let o = compare(&a, &b, &spec(0, &[], 8));
        let hl = highlight(&a, &o).unwrap();
        assert_eq!((hl.w, hl.h), (40, 30));
        assert_eq!(hl.get(12, 12), [255, 0, 0, 255]);
        assert_eq!(
            hl.get(9, 9),
            [255, 0, 255, 255],
            "box corner one pixel outside the change"
        );
        assert_eq!(hl.get(16, 16), [255, 0, 255, 255]);
        assert_eq!(
            hl.get(30, 20),
            [255, 255, 255, 255],
            "unchanged white stays white"
        );
        let mut dark = a.clone();
        paint(&mut dark, 0, 0, 40, 30, [0, 0, 0, 255]);
        let hl = highlight(&dark, &compare(&dark, &dark.clone(), &spec(0, &[], 8))).unwrap();
        assert_eq!(
            hl.get(5, 5),
            [179, 179, 179, 255],
            "unchanged areas fade to a pale ghost"
        );
    }
}
