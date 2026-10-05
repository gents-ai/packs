//! Drawing boxes, arrows, lines and labels on an image from pixel coordinates.
//! Coordinates are in the image as it is at that step (after orientation and
//! any earlier crop or resize). Shapes are drawn in order, later over earlier.
use crate::draw::{self, Rgba};
use crate::input::Shape;
use crate::model::Img;

/// Default stroke colour.
const RED: Rgba = [255, 0, 0, 255];
/// Default stroke width.
const THICKNESS: u32 = 2;

/// What drawing did, for the step's facts.
pub struct Report {
    /// Shapes drawn.
    pub shapes: usize,
    /// 0-based indexes of shapes that lie wholly outside the image and drew nothing.
    pub outside: Vec<usize>,
    /// Label characters outside the plain ASCII font that were drawn as `?`.
    pub replaced: usize,
}

fn colour(c: &Option<String>, default: Rgba) -> Rgba {
    // Validated when the request was read, so a failure here cannot happen; red is the safe fallback.
    c.as_deref()
        .and_then(|s| draw::parse_color(s).ok())
        .unwrap_or(default)
}

/// Whether the rectangle (`x`, `y`, `w`, `h`) misses the image entirely.
fn misses(img: &Img, x: i64, y: i64, w: i64, h: i64) -> bool {
    x + w <= 0 || y + h <= 0 || x >= i64::from(img.w) || y >= i64::from(img.h)
}

/// Draws `shapes` on `img`.
pub fn apply(img: &mut Img, shapes: &[Shape]) -> Report {
    let mut report = Report {
        shapes: 0,
        outside: Vec::new(),
        replaced: 0,
    };
    for (i, shape) in shapes.iter().enumerate() {
        let hidden = match shape {
            Shape::Box {
                x,
                y,
                width,
                height,
                color,
                thickness,
                label,
                fill,
            } => {
                let (w, h) = (i64::from(*width), i64::from(*height));
                let stroke = colour(color, RED);
                if let Some(f) = fill {
                    draw::fill_rect(
                        img,
                        *x,
                        *y,
                        *width,
                        *height,
                        colour(&Some(f.clone()), stroke),
                    );
                }
                draw::rect_outline(
                    img,
                    *x,
                    *y,
                    *width,
                    *height,
                    thickness.unwrap_or(THICKNESS),
                    stroke,
                );
                if let Some(text) = label {
                    let scale = if img.w.max(img.h) >= 800 { 2 } else { 1 };
                    let (_, th) = draw::text_size(text, scale);
                    // Above the box when there is room, else just inside its top edge.
                    let ty = if *y >= i64::from(th) + 2 {
                        *y - i64::from(th) - 2
                    } else {
                        *y + 1
                    };
                    report.replaced += draw::text(
                        img,
                        *x,
                        ty,
                        text,
                        scale,
                        draw::contrast(stroke),
                        Some(stroke),
                    );
                }
                misses(img, *x, *y, w, h)
            }
            Shape::Arrow {
                from,
                to,
                color,
                thickness,
            } => {
                draw::arrow(
                    img,
                    *from,
                    *to,
                    thickness.unwrap_or(THICKNESS),
                    colour(color, RED),
                );
                segment_misses(img, *from, *to)
            }
            Shape::Line {
                from,
                to,
                color,
                thickness,
            } => {
                draw::line(
                    img,
                    *from,
                    *to,
                    thickness.unwrap_or(THICKNESS),
                    colour(color, RED),
                );
                segment_misses(img, *from, *to)
            }
            Shape::Label {
                x,
                y,
                text,
                color,
                background,
                scale,
            } => {
                let scale = scale.unwrap_or(if img.w.max(img.h) >= 800 { 2 } else { 1 });
                let fg = colour(color, [0, 0, 0, 255]);
                let bg = background
                    .as_ref()
                    .map(|b| colour(&Some(b.clone()), [255, 255, 255, 255]));
                report.replaced += draw::text(img, *x, *y, text, scale, fg, bg);
                let (tw, th) = draw::text_size(text, scale);
                misses(img, *x, *y, i64::from(tw), i64::from(th))
            }
        };
        if hidden {
            report.outside.push(i);
        } else {
            report.shapes += 1;
        }
    }
    report
}

/// Whether a segment's bounding box misses the image.
fn segment_misses(img: &Img, a: [i64; 2], b: [i64; 2]) -> bool {
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    x1 < 0 || y1 < 0 || x0 >= i64::from(img.w) || y0 >= i64::from(img.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(v: serde_json::Value) -> Vec<Shape> {
        serde_json::from_value(v).unwrap()
    }

    fn white(w: u32, h: u32) -> Img {
        Img::filled(w, h, [255, 255, 255, 255]).unwrap()
    }

    #[test]
    fn a_box_is_outlined_in_the_default_red_with_the_given_thickness() {
        let mut img = white(20, 20);
        let r = apply(
            &mut img,
            &shapes(serde_json::json!([{"type": "box", "x": 4, "y": 5, "width": 10, "height": 8}])),
        );
        assert_eq!((r.shapes, r.outside.len()), (1, 0));
        let red = [255, 0, 0, 255];
        for x in 4..14 {
            assert_eq!(img.get(x, 5), red);
            assert_eq!(img.get(x, 6), red, "thickness 2");
            assert_eq!(img.get(x, 12), red);
        }
        for y in 5..13 {
            assert_eq!(img.get(4, y), red);
            assert_eq!(img.get(13, y), red);
        }
        assert_eq!(
            img.get(8, 9),
            [255, 255, 255, 255],
            "inside stays untouched without fill"
        );
        assert_eq!(img.get(3, 5), [255, 255, 255, 255]);
        assert_eq!(img.get(14, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn a_box_can_be_filled_translucently_and_recoloured() {
        let mut img = white(12, 12);
        apply(
            &mut img,
            &shapes(serde_json::json!([
            {"type": "box", "x": 2, "y": 2, "width": 8, "height": 8, "color": "#0000ff", "fill": "#0000ff80", "thickness": 1}])),
        );
        assert_eq!(img.get(2, 2), [0, 0, 255, 255]);
        assert_eq!(
            img.get(5, 5),
            [127, 127, 255, 255],
            "blue at half alpha over white"
        );
    }

    #[test]
    fn a_box_label_sits_above_the_box_or_inside_when_there_is_no_room() {
        let mut img = white(60, 40);
        apply(
            &mut img,
            &shapes(
                serde_json::json!([{"type": "box", "x": 5, "y": 20, "width": 30, "height": 15, "label": "cat"}]),
            ),
        );
        // Label background: 3 chars x 8 px = 24 wide, 8 high, ending 2 px above the box top.
        assert_eq!(img.get(5, 10), [255, 0, 0, 255]);
        assert_eq!(img.get(5 + 23, 17), [255, 0, 0, 255]);
        assert_eq!(img.get(5 + 24, 17), [255, 255, 255, 255]);
        let mut top = white(60, 40);
        apply(
            &mut top,
            &shapes(
                serde_json::json!([{"type": "box", "x": 5, "y": 0, "width": 30, "height": 15, "label": "cat"}]),
            ),
        );
        assert_eq!(top.get(5 + 23, 1), [255, 0, 0, 255], "inside the top edge");
    }

    #[test]
    fn lines_and_arrows_draw_between_their_points() {
        let mut img = white(30, 30);
        apply(
            &mut img,
            &shapes(serde_json::json!([
            {"type": "line", "from": [2, 2], "to": [20, 2], "thickness": 1, "color": "black"},
            {"type": "arrow", "from": [2, 15], "to": [25, 15], "thickness": 1, "color": "black"}])),
        );
        let black = [0, 0, 0, 255];
        for x in 2..=20 {
            assert_eq!(img.get(x, 2), black);
        }
        for x in 2..=25 {
            assert_eq!(img.get(x, 15), black);
        }
        assert_eq!(img.get(21, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn a_label_draws_text_with_an_optional_background() {
        let mut img = white(40, 20);
        let r = apply(
            &mut img,
            &shapes(serde_json::json!([
            {"type": "label", "x": 2, "y": 3, "text": "Hi", "background": "yellow", "scale": 1}])),
        );
        assert_eq!(r.shapes, 1);
        assert_eq!(
            img.get(2, 10),
            [255, 220, 0, 255],
            "the background fills the text box (last glyph row is blank)"
        );
        assert_eq!(img.get(17, 10), [255, 220, 0, 255]);
        assert_eq!(img.get(18, 10), [255, 255, 255, 255]);
        assert!(
            (0..8).any(|y| img.get(2 + 1, 3 + y) == [0, 0, 0, 255]
                || img.get(2 + 2, 3 + y) == [0, 0, 0, 255])
        );
    }

    #[test]
    fn shapes_outside_the_image_are_listed_and_the_rest_still_draw() {
        let mut img = white(20, 20);
        let r = apply(
            &mut img,
            &shapes(serde_json::json!([
            {"type": "box", "x": 100, "y": 100, "width": 5, "height": 5},
            {"type": "box", "x": 2, "y": 2, "width": 5, "height": 5},
            {"type": "line", "from": [-5, -5], "to": [-1, -1]},
            {"type": "label", "x": 50, "y": 0, "text": "gone"},
            {"type": "arrow", "from": [30, 30], "to": [40, 40]}])),
        );
        assert_eq!(r.outside, vec![0, 2, 3, 4]);
        assert_eq!(r.shapes, 1);
        assert_eq!(img.get(2, 2), [255, 0, 0, 255]);
    }

    #[test]
    fn a_box_straddling_the_edge_is_clipped_and_counts_as_drawn() {
        let mut img = white(10, 10);
        let r = apply(
            &mut img,
            &shapes(
                serde_json::json!([{"type": "box", "x": -3, "y": -3, "width": 8, "height": 8, "thickness": 1}]),
            ),
        );
        assert_eq!((r.shapes, r.outside.len()), (1, 0));
        assert_eq!(img.get(4, 0), [255, 0, 0, 255]);
        assert_eq!(img.get(0, 4), [255, 0, 0, 255]);
    }

    #[test]
    fn non_ascii_label_characters_are_counted() {
        let mut img = white(60, 20);
        let r = apply(
            &mut img,
            &shapes(
                serde_json::json!([{"type": "label", "x": 0, "y": 0, "text": "caf\u{e9} \u{2603}"}]),
            ),
        );
        assert_eq!(r.replaced, 2);
    }

    #[test]
    fn drawing_order_puts_later_shapes_over_earlier_ones() {
        let mut img = white(20, 20);
        apply(
            &mut img,
            &shapes(serde_json::json!([
            {"type": "box", "x": 0, "y": 0, "width": 20, "height": 20, "fill": "red", "color": "red"},
            {"type": "box", "x": 5, "y": 5, "width": 10, "height": 10, "fill": "blue", "color": "blue"}])),
        );
        assert_eq!(img.get(10, 10), [0, 0, 255, 255]);
        assert_eq!(img.get(2, 2), [255, 0, 0, 255]);
    }
}
