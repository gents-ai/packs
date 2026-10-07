//! Dominant colours by median cut over a 5-bit-per-channel histogram. The cut
//! order, the tie-breaks and the final sort are fixed, so the same pixels
//! always give the same palette.
use crate::model::Img;

/// One dominant colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    /// Mean red, green and blue of the pixels it stands for.
    pub rgb: [u8; 3],
    /// How many pixels it stands for.
    pub pixels: u64,
}

/// The palette of an image.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Swatches, most pixels first, equal counts by colour.
    pub swatches: Vec<Swatch>,
    /// Pixels counted: those at least half opaque.
    pub counted: u64,
    /// Pixels left out as mostly transparent.
    pub transparent: u64,
}

struct Bin {
    key: u16,
    count: u64,
    sum: [u64; 3],
}

fn channel(key: u16, c: usize) -> u16 {
    (key >> (10 - 5 * c)) & 31
}

/// Squared error of treating the bins as one colour: the spread of their means around the mean.
fn spread(count: u64, sum: [u64; 3], sq: f64) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let n = count as f64;
    let s: f64 = sum.iter().map(|&v| (v as f64) * (v as f64)).sum();
    sq - s / n
}

/// The part of a box's spread that a bin adds: count times its squared mean colour.
fn weight(b: &Bin) -> f64 {
    let n = b.count as f64;
    b.sum.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / n
}

fn box_spread(bins: &[Bin]) -> f64 {
    let (mut count, mut sum, mut sq) = (0u64, [0u64; 3], 0.0);
    for b in bins {
        count += b.count;
        for (s, v) in sum.iter_mut().zip(b.sum) {
            *s += v;
        }
        sq += weight(b);
    }
    spread(count, sum, sq)
}

/// Splits `bins` in two along the colour axis and position that leave the least spread.
fn split(mut bins: Vec<Bin>) -> (Vec<Bin>, Vec<Bin>) {
    let mut best: Option<(f64, usize, usize)> = None;
    for axis in 0..3 {
        bins.sort_by_key(|b| (channel(b.key, axis), b.key));
        let mut lo = (0u64, [0u64; 3], 0.0f64);
        let hi_all = bins.iter().fold((0u64, [0u64; 3], 0.0f64), |mut a, b| {
            a.0 += b.count;
            for c in 0..3 {
                a.1[c] += b.sum[c];
            }
            a.2 += weight(b);
            a
        });
        for k in 1..bins.len() {
            let b = &bins[k - 1];
            lo.0 += b.count;
            for c in 0..3 {
                lo.1[c] += b.sum[c];
            }
            lo.2 += weight(b);
            let hi = (
                hi_all.0 - lo.0,
                [
                    hi_all.1[0] - lo.1[0],
                    hi_all.1[1] - lo.1[1],
                    hi_all.1[2] - lo.1[2],
                ],
                hi_all.2 - lo.2,
            );
            let cost = spread(lo.0, lo.1, lo.2) + spread(hi.0, hi.1, hi.2);
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, k));
            }
        }
    }
    let (_, axis, cut) = best.unwrap_or((0.0, 0, 1));
    bins.sort_by_key(|b| (channel(b.key, axis), b.key));
    let upper = bins.split_off(cut.clamp(1, bins.len().saturating_sub(1).max(1)));
    (bins, upper)
}

/// Up to `n` dominant colours of `img`.
pub fn palette(img: &Img, n: usize) -> Palette {
    let mut hist: Vec<Option<Bin>> = (0..32768).map(|_| None).collect();
    let (mut counted, mut transparent) = (0u64, 0u64);
    for p in img.px.as_chunks::<4>().0.iter() {
        if p[3] < 128 {
            transparent += 1;
            continue;
        }
        counted += 1;
        let key = u16::from(p[0] >> 3) << 10 | u16::from(p[1] >> 3) << 5 | u16::from(p[2] >> 3);
        let bin = hist[usize::from(key)].get_or_insert(Bin {
            key,
            count: 0,
            sum: [0; 3],
        });
        bin.count += 1;
        for (s, &v) in bin.sum.iter_mut().zip(&p[..3]) {
            *s += u64::from(v);
        }
    }
    let all: Vec<Bin> = hist.into_iter().flatten().collect();
    let mut boxes: Vec<(f64, Vec<Bin>)> = vec![(box_spread(&all), all)];
    while boxes.len() < n {
        // The splittable box with the most spread; the first on a tie.
        let pick = boxes
            .iter()
            .enumerate()
            .filter(|(_, (_, b))| b.len() > 1)
            .fold(None, |best: Option<(usize, f64)>, (i, (s, _))| match best {
                Some((_, bs)) if bs >= *s => best,
                _ => Some((i, *s)),
            });
        let Some((i, _)) = pick else { break };
        let (_, bins) = boxes.remove(i);
        let (a, b) = split(bins);
        boxes.insert(i, (box_spread(&b), b));
        boxes.insert(i, (box_spread(&a), a));
    }
    let mut swatches: Vec<Swatch> = boxes
        .iter()
        .filter(|(_, b)| !b.is_empty())
        .map(|(_, b)| {
            let pixels: u64 = b.iter().map(|x| x.count).sum();
            let mut rgb = [0u8; 3];
            for (c, out) in rgb.iter_mut().enumerate() {
                let sum: u64 = b.iter().map(|x| x.sum[c]).sum();
                *out = ((sum + pixels / 2) / pixels) as u8;
            }
            Swatch { rgb, pixels }
        })
        .collect();
    swatches.sort_by(|a, b| b.pixels.cmp(&a.pixels).then(a.rgb.cmp(&b.rgb)));
    Palette {
        swatches,
        counted,
        transparent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(colors: &[[u8; 3]], each: u32) -> Img {
        let mut img = Img::new(colors.len() as u32 * each, 4).unwrap();
        for (i, c) in colors.iter().enumerate() {
            for x in 0..each {
                for y in 0..4 {
                    let at = img.at(i as u32 * each + x, y);
                    img.px[at..at + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        }
        img
    }

    #[test]
    fn a_solid_image_has_one_swatch_with_the_exact_colour() {
        let p = palette(&Img::filled(5, 5, [12, 200, 77, 255]).unwrap(), 5);
        assert_eq!(
            p.swatches,
            vec![Swatch {
                rgb: [12, 200, 77],
                pixels: 25
            }]
        );
        assert_eq!((p.counted, p.transparent), (25, 0));
    }

    #[test]
    fn separate_colours_come_back_in_order_of_pixel_count() {
        let mut img = bands(&[[255, 0, 0], [0, 255, 0], [0, 0, 255]], 10);
        // Shrink the blue band to one column by painting the rest red: red 20 columns, green 10, blue 0 -> use explicit counts instead.
        for y in 0..4 {
            let at = img.at(29, y);
            img.px[at..at + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        let p = palette(&img, 3);
        let got: Vec<_> = p.swatches.iter().map(|s| (s.rgb, s.pixels)).collect();
        assert_eq!(
            got,
            vec![([255, 0, 0], 44), ([0, 255, 0], 40), ([0, 0, 255], 36)]
        );
    }

    #[test]
    fn asking_for_fewer_colours_merges_the_closest_groups() {
        let img = bands(&[[250, 0, 0], [255, 8, 8], [0, 0, 255]], 8);
        let p = palette(&img, 2);
        assert_eq!(p.swatches.len(), 2);
        assert_eq!(p.swatches[0].pixels, 64, "the two reds merge");
        assert_eq!(p.swatches[0].rgb, [253, 4, 4]);
        assert_eq!(p.swatches[1].rgb, [0, 0, 255]);
    }

    #[test]
    fn fewer_distinct_colours_than_asked_gives_fewer_swatches() {
        let p = palette(&bands(&[[1, 2, 3], [200, 100, 50]], 6), 16);
        assert_eq!(p.swatches.len(), 2);
    }

    #[test]
    fn mostly_transparent_pixels_are_counted_apart() {
        let mut img = Img::filled(4, 1, [10, 10, 10, 255]).unwrap();
        img.px[3] = 0;
        img.px[7] = 127;
        let p = palette(&img, 3);
        assert_eq!((p.counted, p.transparent), (2, 2));
        assert_eq!(p.swatches[0].pixels, 2);
        let empty = palette(&Img::filled(2, 2, [1, 2, 3, 0]).unwrap(), 3);
        assert!(empty.swatches.is_empty());
        assert_eq!(empty.transparent, 4);
    }

    #[test]
    fn alpha_127_is_transparent_and_alpha_128_is_counted() {
        let mut img = Img::filled(3, 1, [9, 9, 9, 255]).unwrap();
        img.px[3] = 127;
        img.px[7] = 128;
        let p = palette(&img, 3);
        assert_eq!((p.counted, p.transparent), (2, 1));
    }

    #[test]
    fn the_same_pixels_give_the_same_palette() {
        let img = crate::fixtures::scene(64, 48);
        let img = Img::from_raw(64, 48, img.into_raw()).unwrap();
        assert_eq!(palette(&img, 6), palette(&img.clone(), 6));
        let p = palette(&img, 6);
        assert_eq!(p.swatches.len(), 6);
        assert_eq!(p.swatches.iter().map(|s| s.pixels).sum::<u64>(), 64 * 48);
    }
}
