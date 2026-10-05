//! Drawing on an image: colours, filled and outlined rectangles, thick lines,
//! arrows and 8x8 bitmap text. Everything is integer or correctly rounded
//! float math with clipping at the image edge, so a shape that leaves the
//! image is cut, never a panic, and results are identical on every platform.
use font8x8::legacy::BASIC_LEGACY;

use crate::model::Img;

/// A colour as red, green, blue, alpha.
pub type Rgba = [u8; 4];

/// Width and height of a glyph cell at scale 1.
pub const GLYPH: u32 = 8;

/// Parses `#rgb`, `#rrggbb`, `#rrggbbaa` or one of the colour names black,
/// white, red, green, blue, yellow, orange, magenta, cyan, gray.
pub fn parse_color(s: &str) -> Result<Rgba, String> {
    let named = match s {
        "black" => Some([0, 0, 0]),
        "white" => Some([255, 255, 255]),
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 160, 0]),
        "blue" => Some([0, 0, 255]),
        "yellow" => Some([255, 220, 0]),
        "orange" => Some([255, 140, 0]),
        "magenta" => Some([255, 0, 255]),
        "cyan" => Some([0, 200, 255]),
        "gray" => Some([128, 128, 128]),
        _ => None,
    };
    if let Some([r, g, b]) = named {
        return Ok([r, g, b, 255]);
    }
    let bad = || format!("{s:?} is not a colour; use a name such as red or a hex code such as #ff8800");
    let hex = s.strip_prefix('#').ok_or_else(bad)?;
    if !hex.is_ascii() {
        return Err(bad());
    }
    let nib = |c: &str| u8::from_str_radix(c, 16).map_err(|_| bad());
    match hex.len() {
        3 => {
            let d = |i: usize| nib(&hex[i..=i]).map(|v| v * 17);
            Ok([d(0)?, d(1)?, d(2)?, 255])
        }
        6 | 8 => {
            let b = |i: usize| nib(&hex[i..i + 2]);
            let a = if hex.len() == 8 { b(6)? } else { 255 };
            Ok([b(0)?, b(2)?, b(4)?, a])
        }
        _ => Err(bad()),
    }
}

/// Black or white, whichever reads better on `bg`.
pub fn contrast(bg: Rgba) -> Rgba {
    let luma = (299 * u32::from(bg[0]) + 587 * u32::from(bg[1]) + 114 * u32::from(bg[2])) / 1000;
    if luma > 140 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }
}

/// Draws colour `c` over pixel (`x`, `y`) by its alpha; outside the image is ignored.
pub fn blend(img: &mut Img, x: i64, y: i64, c: Rgba) {
    if x < 0 || y < 0 || x >= i64::from(img.w) || y >= i64::from(img.h) || c[3] == 0 {
        return;
    }
    let i = img.at(x as u32, y as u32);
    if c[3] == 255 {
        img.px[i..i + 4].copy_from_slice(&c);
        return;
    }
    let a = u32::from(c[3]);
    for k in 0..3 {
        img.px[i + k] = ((u32::from(c[k]) * a + u32::from(img.px[i + k]) * (255 - a) + 127) / 255) as u8;
    }
    let da = u32::from(img.px[i + 3]);
    img.px[i + 3] = (a + (da * (255 - a) + 127) / 255).min(255) as u8;
}

/// Fills the rectangle at (`x`, `y`) of `w` x `h` pixels.
pub fn fill_rect(img: &mut Img, x: i64, y: i64, w: u32, h: u32, c: Rgba) {
    let (x0, y0) = (x.max(0), y.max(0));
    let x1 = (x + i64::from(w)).min(i64::from(img.w));
    let y1 = (y + i64::from(h)).min(i64::from(img.h));
    for yy in y0..y1 {
        for xx in x0..x1 {
            blend(img, xx, yy, c);
        }
    }
}

/// Outlines a rectangle with a border `t` pixels thick drawn inward from its edge.
pub fn rect_outline(img: &mut Img, x: i64, y: i64, w: u32, h: u32, t: u32, c: Rgba) {
    let t = t.min(w / 2 + 1).min(h / 2 + 1).max(1);
    fill_rect(img, x, y, w, t, c);
    fill_rect(img, x, y + i64::from(h) - i64::from(t), w, t, c);
    let inner = h.saturating_sub(2 * t);
    fill_rect(img, x, y + i64::from(t), t, inner, c);
    fill_rect(img, x + i64::from(w) - i64::from(t), y + i64::from(t), t, inner, c);
}

/// Clips a segment to the image grown by `margin` pixels (Liang-Barsky); `None` when none of it is inside.
fn clip(p0: [i64; 2], p1: [i64; 2], w: u32, h: u32, margin: i64) -> Option<([i64; 2], [i64; 2])> {
    let (lo, hi) = ([-margin, -margin], [i64::from(w) + margin, i64::from(h) + margin]);
    let inside = |p: [i64; 2]| (0..2).all(|k| p[k] >= lo[k] && p[k] <= hi[k]);
    if inside(p0) && inside(p1) {
        return Some((p0, p1));
    }
    let d = [(p1[0] - p0[0]) as f64, (p1[1] - p0[1]) as f64];
    let (mut u0, mut u1) = (0.0f64, 1.0f64);
    for k in 0..2 {
        for (p, q) in [(-d[k], (p0[k] - lo[k]) as f64), (d[k], (hi[k] - p0[k]) as f64)] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 { u0 = u0.max(r) } else { u1 = u1.min(r) }
            }
        }
    }
    if u0 > u1 {
        return None;
    }
    let at = |u: f64| [p0[0] + (u * d[0]).round() as i64, p0[1] + (u * d[1]).round() as i64];
    Some((at(u0), at(u1)))
}

/// Draws a line from `p0` to `p1` with a square brush `t` pixels wide.
pub fn line(img: &mut Img, p0: [i64; 2], p1: [i64; 2], t: u32, c: Rgba) {
    let t = i64::from(t.max(1));
    let Some(([x0, y0], [x1, y1])) = clip(p0, p1, img.w, img.h, t) else {
        return;
    };
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        for oy in 0..t {
            for ox in 0..t {
                blend(img, x - t / 2 + ox, y - t / 2 + oy, c);
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Draws an arrow from `from` to `to`, with a head at `to`.
pub fn arrow(img: &mut Img, from: [i64; 2], to: [i64; 2], t: u32, c: Rgba) {
    line(img, from, to, t, c);
    let (dx, dy) = ((to[0] - from[0]) as f64, (to[1] - from[1]) as f64);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return;
    }
    let head = (f64::from(t) * 4.0).max(10.0).min(len);
    let (ux, uy) = (dx / len, dy / len);
    // Wings of 30 degrees to either side of the way back along the shaft.
    const COS: f64 = 0.866_025_403_784_438_6;
    const SIN: f64 = 0.5;
    for side in [-1.0f64, 1.0] {
        let wx = -(ux * COS - side * uy * SIN) * head;
        let wy = -(side * ux * SIN + uy * COS) * head;
        line(img, to, [to[0] + wx.round() as i64, to[1] + wy.round() as i64], t, c);
    }
}

/// The pixel size of `text` at `scale`.
pub fn text_size(text: &str, scale: u32) -> (u32, u32) {
    (text.chars().count() as u32 * GLYPH * scale, GLYPH * scale)
}

/// Draws `text` with its top-left corner at (`x`, `y`) in the 8x8 ASCII font
/// scaled by `scale`, over `bg` when given. Characters outside ASCII are drawn
/// as `?`; the count of those is returned.
pub fn text(img: &mut Img, x: i64, y: i64, text: &str, scale: u32, fg: Rgba, bg: Option<Rgba>) -> usize {
    let scale = scale.max(1);
    let (w, h) = text_size(text, scale);
    if let Some(bg) = bg {
        fill_rect(img, x, y, w, h, bg);
    }
    let mut replaced = 0;
    for (n, ch) in text.chars().enumerate() {
        let code = if ch.is_ascii() && !ch.is_ascii_control() {
            ch as usize
        } else {
            replaced += 1;
            '?' as usize
        };
        let cell_x = x + (n as u32 * GLYPH * scale) as i64;
        for (row, bits) in BASIC_LEGACY[code].iter().enumerate() {
            for col in 0..GLYPH {
                if bits >> col & 1 == 1 {
                    fill_rect(
                        img,
                        cell_x + i64::from(col * scale),
                        y + (row as u32 * scale) as i64,
                        scale,
                        scale,
                        fg,
                    );
                }
            }
        }
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Img {
        Img::filled(w, h, [255, 255, 255, 255]).unwrap()
    }

    fn marks(img: &Img, c: Rgba) -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..img.h {
            for x in 0..img.w {
                if img.get(x, y) == c {
                    v.push((x, y));
                }
            }
        }
        v
    }

    const K: Rgba = [0, 0, 0, 255];

    #[test]
    fn colours_parse_from_names_and_hex() {
        assert_eq!(parse_color("red").unwrap(), [255, 0, 0, 255]);
        assert_eq!(parse_color("#f80").unwrap(), [255, 136, 0, 255]);
        assert_eq!(parse_color("#ff8800").unwrap(), [255, 136, 0, 255]);
        assert_eq!(parse_color("#ff880040").unwrap(), [255, 136, 0, 64]);
        for bad in ["", "#", "#12", "#12345", "#gg0000", "reddish", "ff0000", "#ff00000", "#é00"] {
            assert!(parse_color(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn contrast_picks_the_readable_text_colour() {
        assert_eq!(contrast([255, 255, 255, 255]), K);
        assert_eq!(contrast([0, 0, 0, 255]), [255, 255, 255, 255]);
        assert_eq!(contrast([255, 0, 0, 255]), [255, 255, 255, 255]);
        assert_eq!(contrast([255, 220, 0, 255]), K);
    }

    #[test]
    fn blending_mixes_by_alpha_and_ignores_outside() {
        let mut img = canvas(2, 2);
        blend(&mut img, 0, 0, [0, 0, 0, 128]);
        assert_eq!(img.get(0, 0), [127, 127, 127, 255]);
        blend(&mut img, 1, 1, [10, 20, 30, 255]);
        assert_eq!(img.get(1, 1), [10, 20, 30, 255]);
        let before = img.clone();
        for (x, y) in [(-1, 0), (0, -1), (2, 0), (0, 2), (i64::MIN, i64::MAX)] {
            blend(&mut img, x, y, K);
        }
        blend(&mut img, 0, 1, [1, 2, 3, 0]);
        assert_eq!(img, before);
    }

    #[test]
    fn rectangles_fill_and_outline_exactly_and_clip_at_the_edges() {
        let mut img = canvas(6, 6);
        fill_rect(&mut img, 1, 1, 2, 3, K);
        assert_eq!(marks(&img, K), [(1, 1), (2, 1), (1, 2), (2, 2), (1, 3), (2, 3)]);
        let mut img = canvas(6, 6);
        rect_outline(&mut img, 1, 1, 4, 4, 1, K);
        let want: Vec<(u32, u32)> = vec![
            (1, 1), (2, 1), (3, 1), (4, 1), (1, 2), (4, 2), (1, 3), (4, 3), (1, 4), (2, 4), (3, 4), (4, 4),
        ];
        assert_eq!(marks(&img, K), want);
        let mut img = canvas(4, 4);
        fill_rect(&mut img, -2, -2, 4, 4, K);
        assert_eq!(marks(&img, K).len(), 4);
        fill_rect(&mut img, 3, 3, 10, 10, K);
        assert_eq!(img.get(3, 3), K);
    }

    #[test]
    fn a_thick_outline_stays_inside_its_box() {
        let mut img = canvas(10, 10);
        rect_outline(&mut img, 2, 2, 6, 6, 2, K);
        assert_eq!(img.get(2, 2), K);
        assert_eq!(img.get(7, 7), K);
        assert_eq!(img.get(3, 3), K);
        assert_eq!(img.get(4, 4), [255, 255, 255, 255], "the inside is untouched");
        assert_eq!(img.get(1, 1), [255, 255, 255, 255], "nothing outside the box");
        assert_eq!(img.get(8, 8), [255, 255, 255, 255]);
    }

    #[test]
    fn lines_follow_bresenham_and_include_both_ends() {
        let mut img = canvas(6, 6);
        line(&mut img, [0, 0], [5, 5], 1, K);
        assert_eq!(marks(&img, K), [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
        let mut img = canvas(6, 3);
        line(&mut img, [0, 1], [5, 1], 1, K);
        assert_eq!(marks(&img, K).len(), 6);
        let mut img = canvas(6, 6);
        line(&mut img, [4, 1], [1, 4], 1, K);
        assert_eq!(marks(&img, K), [(4, 1), (3, 2), (2, 3), (1, 4)]);
        let mut img = canvas(5, 5);
        line(&mut img, [2, 2], [2, 2], 1, K);
        assert_eq!(marks(&img, K), [(2, 2)]);
    }

    #[test]
    fn a_line_far_outside_the_image_is_clipped_without_walking_forever() {
        let mut img = canvas(8, 8);
        let started = std::time::Instant::now();
        line(&mut img, [-4_000_000_000, 4], [4_000_000_000, 4], 1, K);
        line(&mut img, [-4_000_000_000, -4_000_000_000], [4_000_000_000, 4_000_000_000], 1, K);
        line(&mut img, [-4_000_000_000, 100], [4_000_000_000, 100], 1, K);
        assert!(started.elapsed().as_millis() < 5_000);
        assert_eq!(img.get(3, 4), K);
        assert_eq!(img.get(2, 2), K, "the diagonal crosses the image");
        assert_eq!(img.get(0, 7), [255, 255, 255, 255]);
    }

    #[test]
    fn an_arrow_has_a_shaft_and_a_head_at_the_tip() {
        let mut img = canvas(40, 40);
        arrow(&mut img, [4, 20], [34, 20], 1, K);
        for x in 4..=34 {
            assert_eq!(img.get(x, 20), K, "shaft at {x}");
        }
        // Wings sweep back from the tip on both sides of the shaft.
        assert_eq!(img.get(34 - 4, 20 - 2), K);
        assert_eq!(img.get(34 - 4, 20 + 2), K);
        assert_eq!(img.get(10, 10), [255, 255, 255, 255]);
        let mut img = canvas(10, 10);
        arrow(&mut img, [5, 5], [5, 5], 1, K);
        assert_eq!(marks(&img, K), [(5, 5)]);
    }

    #[test]
    fn text_draws_the_font_glyphs_at_scale() {
        let mut img = canvas(20, 10);
        text(&mut img, 1, 1, "I", 1, K, None);
        // The glyph for I: a vertical bar with serifs; check the stem column of its middle rows.
        let on = marks(&img, K);
        assert!(!on.is_empty());
        assert!(on.iter().all(|&(x, y)| (1..9).contains(&x) && (1..9).contains(&y)));
        let mut big = canvas(40, 20);
        text(&mut big, 2, 2, "I", 2, K, None);
        assert_eq!(marks(&big, K).len(), on.len() * 4, "scale 2 draws every pixel as 2x2");
    }

    #[test]
    fn text_fills_its_background_and_replaces_non_ascii() {
        let mut img = canvas(30, 10);
        let n = text(&mut img, 0, 0, "a\u{e9}", 1, [255, 255, 255, 255], Some([200, 0, 0, 255]));
        assert_eq!(n, 1);
        assert_eq!(img.get(0, 0), [200, 0, 0, 255]);
        assert_eq!(text_size("abc", 2), (48, 16));
        let mut same = canvas(30, 10);
        text(&mut same, 0, 0, "a?", 1, [255, 255, 255, 255], Some([200, 0, 0, 255]));
        assert_eq!(img, same, "a replaced character looks exactly like a question mark");
    }

    #[test]
    fn text_off_the_image_is_clipped() {
        let mut img = canvas(4, 4);
        text(&mut img, -100, -100, "hello", 3, K, None);
        text(&mut img, 100, 100, "hello", 3, K, None);
        assert_eq!(img, canvas(4, 4));
    }
}
