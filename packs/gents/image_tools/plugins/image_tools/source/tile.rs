//! Splitting a large image into overlapping tiles for a vision model, with an
//! index picture that shows where each tile sits. The grid is exact: every
//! pixel is inside at least one tile, neighbouring tiles overlap by at least
//! the requested amount, and the same image and options always give the same
//! tiles in the same order (left to right, top to bottom).
use crate::draw;
use crate::geom::{crop, fit_dims};
use crate::model::Img;
use crate::resize::{Filter, resample};

/// Tiles one request may produce.
pub const MAX_TILES: usize = 2000;
/// Longest side of the index picture.
const INDEX_SIDE: u32 = 1024;

/// One tile's place in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// 1-based tile number, row-major.
    pub n: usize,
    /// 0-based row.
    pub row: u32,
    /// 0-based column.
    pub col: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The tile grid of an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    xs: Vec<u32>,
    ys: Vec<u32>,
    /// Tile width (the image width when it is narrower than a tile).
    pub tile_w: u32,
    /// Tile height.
    pub tile_h: u32,
}

/// Start positions along one axis of length `len` for tiles of `size` that overlap by at least `overlap`,
/// spread evenly so no tile is a sliver of its neighbour.
fn axis(len: u32, size: u32, overlap: u32) -> Vec<u32> {
    if len <= size {
        return vec![0];
    }
    let stride = u64::from(size - overlap);
    let n = (u64::from(len - overlap)).div_ceil(stride);
    let span = u64::from(len - size);
    (0..n)
        .map(|i| ((i * span + (n - 1) / 2) / (n - 1)) as u32)
        .collect()
}

/// The grid for a `w` x `h` image, or an error sentence when it would hold too many tiles.
pub fn grid(w: u32, h: u32, size: u32, overlap: u32) -> Result<Grid, String> {
    let (xs, ys) = (axis(w, size, overlap), axis(h, size, overlap));
    if xs.len() * ys.len() > MAX_TILES {
        return Err(format!(
            "that would be {} tiles, over the {MAX_TILES} one request may make; use a larger size or less overlap",
            xs.len() * ys.len()
        ));
    }
    Ok(Grid {
        xs,
        ys,
        tile_w: size.min(w),
        tile_h: size.min(h),
    })
}

impl Grid {
    /// Columns.
    pub fn cols(&self) -> u32 {
        self.xs.len() as u32
    }

    /// Rows.
    pub fn rows(&self) -> u32 {
        self.ys.len() as u32
    }

    /// Tiles in all.
    pub fn len(&self) -> usize {
        self.xs.len() * self.ys.len()
    }

    /// The tile with 0-based index `i`.
    pub fn cell(&self, i: usize) -> Cell {
        let (row, col) = ((i / self.xs.len()) as u32, (i % self.xs.len()) as u32);
        Cell {
            n: i + 1,
            row,
            col,
            x: self.xs[col as usize],
            y: self.ys[row as usize],
            width: self.tile_w,
            height: self.tile_h,
        }
    }
}

/// Cuts tile `c` out of `img`.
pub fn cut(img: &Img, c: &Cell) -> Result<Img, String> {
    crop(img, c.x, c.y, c.width, c.height)
}

/// The index picture: the whole image shrunk to fit 1024 pixels with every tile outlined and numbered.
pub fn index_image(img: &Img, grid: &Grid) -> Result<Img, String> {
    let (w, h) = if img.w.max(img.h) > INDEX_SIDE {
        fit_dims(img.w, img.h, Some(INDEX_SIDE), Some(INDEX_SIDE))
    } else {
        (img.w, img.h)
    };
    let mut out = resample(img.clone(), w, h, Filter::Box)?;
    let scale =
        |v: u32, from: u32, to: u32| (u64::from(v) * u64::from(to) / u64::from(from)) as i64;
    const COLOURS: [draw::Rgba; 4] = [
        [255, 0, 0, 255],
        [0, 90, 255, 255],
        [0, 160, 0, 255],
        [255, 140, 0, 255],
    ];
    for i in 0..grid.len() {
        let c = grid.cell(i);
        let col = COLOURS[((c.row + c.col) % 4) as usize];
        let (x0, y0) = (scale(c.x, img.w, w), scale(c.y, img.h, h));
        let (x1, y1) = (
            scale(c.x + c.width, img.w, w),
            scale(c.y + c.height, img.h, h),
        );
        draw::rect_outline(
            &mut out,
            x0,
            y0,
            (x1 - x0).max(1) as u32,
            (y1 - y0).max(1) as u32,
            1,
            col,
        );
        draw::text(
            &mut out,
            x0 + 2,
            y0 + 2,
            &c.n.to_string(),
            1,
            draw::contrast(col),
            Some(col),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(len: u32, size: u32, overlap: u32) -> Vec<u32> {
        let a = axis(len, size, overlap);
        let mut hits = vec![0u32; len as usize];
        for s in &a {
            for p in *s..(*s + size.min(len)) {
                hits[p as usize] += 1;
            }
        }
        hits
    }

    #[test]
    fn a_small_image_is_one_tile() {
        assert_eq!(axis(500, 1024, 64), vec![0]);
        assert_eq!(axis(1024, 1024, 64), vec![0]);
        let g = grid(500, 300, 1024, 64).unwrap();
        assert_eq!((g.cols(), g.rows(), g.len()), (1, 1, 1));
        let c = g.cell(0);
        assert_eq!((c.x, c.y, c.width, c.height, c.n), (0, 0, 500, 300, 1));
    }

    #[test]
    fn positions_are_exact_for_a_known_case() {
        // 100 wide, tiles of 40 overlapping by 10: stride 30, three tiles end at 100 exactly.
        assert_eq!(axis(100, 40, 10), vec![0, 30, 60]);
        // 101 wide needs four tiles, spread evenly instead of a sliver at the end.
        assert_eq!(axis(101, 40, 10), vec![0, 20, 41, 61]);
        assert_eq!(axis(41, 40, 0), vec![0, 1]);
        assert_eq!(axis(80, 40, 0), vec![0, 40]);
    }

    #[test]
    fn every_pixel_is_covered_and_neighbours_overlap_enough() {
        for (len, size, overlap) in [
            (100, 40, 10),
            (101, 40, 10),
            (1000, 256, 32),
            (4097, 1024, 64),
            (65, 64, 63),
            (500, 64, 0),
        ] {
            let a = axis(len, size, overlap);
            assert!(
                covered(len, size, overlap).iter().all(|&h| h >= 1),
                "{len} {size} {overlap}"
            );
            assert_eq!(
                *a.last().unwrap() + size,
                len,
                "the last tile ends at the edge"
            );
            for w in a.windows(2) {
                assert!(w[0] < w[1], "strictly increasing");
                assert!(
                    w[0] + size >= w[1] + overlap,
                    "overlap {} < {overlap} for {len}/{size}",
                    w[0] + size - w[1]
                );
            }
        }
    }

    #[test]
    fn cells_run_left_to_right_then_top_to_bottom() {
        let g = grid(100, 80, 40, 10).unwrap();
        assert_eq!((g.cols(), g.rows()), (3, 3));
        let c4 = g.cell(3);
        assert_eq!((c4.n, c4.row, c4.col, c4.x, c4.y), (4, 1, 0, 0, 20));
        let c9 = g.cell(8);
        assert_eq!(
            (c9.row, c9.col, c9.x, c9.y, c9.width, c9.height),
            (2, 2, 60, 40, 40, 40)
        );
    }

    #[test]
    fn tiles_reassemble_to_the_source() {
        let src = crate::fixtures::scene(97, 61);
        let src = Img::from_raw(97, 61, src.into_raw()).unwrap();
        let g = grid(97, 61, 32, 8).unwrap();
        let mut out = Img::new(97, 61).unwrap();
        let mut seen = vec![0u32; 97 * 61];
        for i in 0..g.len() {
            let c = g.cell(i);
            let t = cut(&src, &c).unwrap();
            assert_eq!((t.w, t.h), (c.width, c.height));
            for y in 0..t.h {
                for x in 0..t.w {
                    let d = out.at(c.x + x, c.y + y);
                    out.px[d..d + 4].copy_from_slice(&t.get(x, y));
                    seen[((c.y + y) * 97 + c.x + x) as usize] += 1;
                }
            }
        }
        assert_eq!(out, src);
        assert!(seen.iter().all(|&n| n >= 1));
    }

    #[test]
    fn too_many_tiles_are_refused_with_the_count() {
        let e = grid(30_000, 30_000, 64, 0).err().unwrap();
        assert!(e.contains("tiles, over the 2000"), "{e}");
        assert!(grid(10_000, 10_000, 1024, 64).is_ok());
    }

    #[test]
    fn the_index_outlines_and_numbers_every_tile() {
        let src = Img::filled(200, 100, [255, 255, 255, 255]).unwrap();
        let g = grid(200, 100, 100, 20).unwrap();
        let idx = index_image(&src, &g).unwrap();
        assert_eq!((idx.w, idx.h), (200, 100), "a small image is not scaled");
        assert_eq!(
            idx.get(0, 0),
            [255, 0, 0, 255],
            "tile 1 starts red with its number"
        );
        let c2 = g.cell(1);
        assert_eq!(
            idx.get(c2.x, 50),
            [0, 90, 255, 255],
            "the second tile's left edge is blue"
        );
        assert_eq!(
            idx.get(150, 50),
            [255, 255, 255, 255],
            "inside a tile the picture shows through"
        );
        let big = Img::filled(4000, 2000, [255, 255, 255, 255]).unwrap();
        let idx = index_image(&big, &grid(4000, 2000, 1024, 64).unwrap()).unwrap();
        assert_eq!((idx.w, idx.h), (1024, 512));
    }
}
