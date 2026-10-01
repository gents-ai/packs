use super::*;

fn sp(text: &str, x0: f32, top: f32, size: f32) -> Item {
    Item::Text(Span {
        text: text.into(),
        x0,
        x1: x0 + text.chars().count() as f32 * size * 0.5,
        top,
        bottom: top + size,
        size,
    })
}

fn render(out: Vec<Out>) -> String {
    let blocks: Vec<Block> = out
        .into_iter()
        .filter_map(|o| if let Out::Block(b) = o { Some(b) } else { None })
        .collect();
    crate::md::render(&blocks)
}

#[test]
fn two_columns_read_left_then_right() {
    let mut items = Vec::new();
    for i in 0..6 {
        let y = 100.0 + i as f32 * 14.0;
        items.push(sp(
            &format!("left column line number {i} here"),
            50.0,
            y,
            10.0,
        ));
        items.push(sp(
            &format!("right column line number {i} here"),
            330.0,
            y,
            10.0,
        ));
    }
    let text = render(layout(&items));
    let left_last = text.find("left column line number 5").unwrap();
    let right_first = text.find("right column line number 0").unwrap();
    assert!(left_last < right_first, "{text}");
}

#[test]
fn large_text_becomes_a_heading_and_gaps_split_paragraphs() {
    let items = vec![
        sp("Annual Report", 50.0, 50.0, 24.0),
        sp("First paragraph line one of text.", 50.0, 100.0, 10.0),
        sp("continues on the second line.", 50.0, 112.0, 10.0),
        sp("Second paragraph starts here.", 50.0, 140.0, 10.0),
    ];
    let text = render(layout(&items));
    assert_eq!(
        text,
        "# Annual Report\n\nFirst paragraph line one of text. continues on the second line.\n\nSecond paragraph starts here."
    );
}

#[test]
fn aligned_short_cells_become_a_table() {
    let mut items = Vec::new();
    for (r, row) in [
        ["Item", "Qty", "Price"],
        ["Apple", "3", "1.20"],
        ["Pear", "5", "2.00"],
    ]
    .iter()
    .enumerate()
    {
        for (c, cell) in row.iter().enumerate() {
            items.push(sp(
                cell,
                50.0 + c as f32 * 120.0,
                100.0 + r as f32 * 14.0,
                10.0,
            ));
        }
    }
    assert_eq!(
        render(layout(&items)),
        "| Item | Qty | Price |\n| --- | --- | --- |\n| Apple | 3 | 1.20 |\n| Pear | 5 | 2.00 |"
    );
}

#[test]
fn lists_and_hyphenation() {
    let items = vec![
        sp("- first point", 50.0, 100.0, 10.0),
        sp("\u{2022} second point", 50.0, 112.0, 10.0),
        sp("Some hyphen-", 50.0, 150.0, 10.0),
        sp("ated text", 50.0, 162.0, 10.0),
    ];
    assert_eq!(
        render(layout(&items)),
        "- first point\n- second point\n\nSome hyphenated text"
    );
}

#[test]
fn figure_takes_its_caption_paragraph() {
    let items = vec![
        sp("Body text before the figure.", 50.0, 50.0, 10.0),
        Item::Fig(FigBox {
            x0: 50.0,
            x1: 300.0,
            top: 100.0,
            bottom: 250.0,
            index: 0,
        }),
        sp("Figure 3. Revenue by quarter", 50.0, 262.0, 10.0),
    ];
    let mut out = layout(&items);
    let captions = take_captions(&mut out, 1);
    assert_eq!(captions, vec!["Figure 3. Revenue by quarter".to_string()]);
    assert_eq!(out.len(), 2);
}

#[test]
fn a_table_next_to_a_figure_is_still_a_table() {
    let mut items = Vec::new();
    for (r, row) in [
        ["Region", "Revenue", "Growth"],
        ["North", "42", "12%"],
        ["South", "37", "8%"],
    ]
    .iter()
    .enumerate()
    {
        for (c, cell) in row.iter().enumerate() {
            items.push(sp(
                cell,
                50.0 + c as f32 * 120.0,
                100.0 + r as f32 * 16.0,
                11.0,
            ));
        }
    }
    items.push(Item::Fig(FigBox {
        x0: 50.0,
        x1: 300.0,
        top: 146.0,
        bottom: 296.0,
        index: 0,
    }));
    let out = layout(&items);
    assert!(
        matches!(out[0], Out::Block(Block::Table(_))) && matches!(out[1], Out::Fig(0)),
        "{}",
        out.len()
    );
}

#[test]
fn a_heading_above_a_table_does_not_glue_its_columns() {
    let mut items = vec![sp("Results by region", 56.0, 60.0, 15.0)];
    for (r, row) in [
        ["Region", "Revenue", "Growth"],
        ["North", "42", "12%"],
        ["South", "37", "8%"],
    ]
    .iter()
    .enumerate()
    {
        for (c, cell) in row.iter().enumerate() {
            items.push(sp(
                cell,
                56.0 + c as f32 * 120.0,
                103.0 + r as f32 * 16.0,
                11.0,
            ));
        }
    }
    let text = render(layout(&items));
    assert_eq!(
        text,
        "### Results by region\n\n| Region | Revenue | Growth |\n| --- | --- | --- |\n| North | 42 | 12% |\n| South | 37 | 8% |"
    );
}

#[test]
fn numbered_headings_with_a_tab_gap_stay_on_one_line() {
    let items = vec![
        sp("1.", 50.0, 100.0, 10.0),
        sp("Introduction to the plugin", 80.0, 100.0, 10.0),
        sp("2.", 50.0, 114.0, 10.0),
        sp("Background and scope", 80.0, 114.0, 10.0),
    ];
    assert_eq!(
        render(layout(&items)),
        "1. Introduction to the plugin\n2. Background and scope"
    );
}

#[test]
fn narrow_gaps_between_cells_still_make_a_table() {
    // Cells 10 points apart, as a default word-processor table draws them.
    let widths = [40.0, 20.0, 20.0, 30.0];
    let mut items = Vec::new();
    for (r, row) in [
        ["Region", "Q1", "Q2", "Notes"],
        ["North", "10", "12", "good"],
        ["South", "8", "9", "ok"],
        ["East", "7", "6", "weak"],
    ]
    .iter()
    .enumerate()
    {
        let mut x = 50.0;
        for (c, cell) in row.iter().enumerate() {
            items.push(Item::Text(Span {
                text: (*cell).into(),
                x0: x,
                x1: x + widths[c],
                top: 100.0 + r as f32 * 14.0,
                bottom: 110.0 + r as f32 * 14.0,
                size: 10.0,
            }));
            x += widths[c] + 10.0;
        }
    }
    items.insert(
        0,
        sp(
            "A paragraph of prose just above the table.",
            50.0,
            70.0,
            10.0,
        ),
    );
    assert_eq!(
        render(layout(&items)),
        "A paragraph of prose just above the table.\n\n| Region | Q1 | Q2 | Notes |\n| --- | --- | --- | --- |\n| North | 10 | 12 | good |\n| South | 8 | 9 | ok |\n| East | 7 | 6 | weak |"
    );
}

#[test]
fn stretched_prose_lines_are_not_a_table() {
    // Every line is split in a different place, as justified text is.
    let mut items = Vec::new();
    for (r, cuts) in [[80.0, 210.0], [130.0, 300.0], [95.0, 260.0], [150.0, 330.0]]
        .iter()
        .enumerate()
    {
        let top = 100.0 + r as f32 * 12.0;
        let mut x = 50.0;
        for cut in [cuts[0], cuts[1], 440.0] {
            items.push(Item::Text(Span {
                text: "some words here".into(),
                x0: x,
                x1: cut,
                top,
                bottom: top + 10.0,
                size: 10.0,
            }));
            x = cut + 6.0;
        }
    }
    assert!(!render(layout(&items)).contains('|'));
}

fn figure_page(caption_y: f32, tail: &str) -> Vec<Item> {
    vec![
        sp(
            "Body text before the figure that is long enough.",
            50.0,
            50.0,
            10.0,
        ),
        Item::Fig(FigBox {
            x0: 50.0,
            x1: 250.0,
            top: 70.0,
            bottom: 250.0,
            index: 0,
        }),
        sp("Figure 2. Chart of sales", 50.0, caption_y, 10.0),
        sp(tail, 50.0, caption_y + 12.0, 10.0),
    ]
}

#[test]
fn a_caption_takes_only_its_own_line_not_the_paragraph_after() {
    let mut out = layout(&figure_page(254.0, "Tail paragraph."));
    let captions = take_captions(&mut out, 1);
    assert_eq!(captions, vec!["Figure 2. Chart of sales".to_string()]);
    let rest: Vec<String> = out
        .iter()
        .filter_map(|o| match o {
            Out::Block(Block::Para(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        rest,
        vec![
            "Body text before the figure that is long enough.",
            "Tail paragraph."
        ]
    );
}

#[test]
fn a_wrapped_caption_keeps_its_second_line() {
    let mut items = figure_page(254.0, "by region and quarter");
    if let Item::Text(s) = &mut items[2] {
        s.text = "Figure 2. Chart of sales across every".into();
    }
    items.push(sp("A new body paragraph starts here.", 50.0, 290.0, 10.0));
    let mut out = layout(&items);
    let captions = take_captions(&mut out, 1);
    assert_eq!(
        captions,
        vec!["Figure 2. Chart of sales across every by region and quarter".to_string()]
    );
}

#[test]
fn a_figure_stays_in_its_column() {
    let mut items = Vec::new();
    for i in 0..8 {
        let y = 100.0 + i as f32 * 14.0;
        items.push(sp(
            &format!("right column line number {i} here"),
            330.0,
            y,
            10.0,
        ));
    }
    items.push(sp(
        "left intro line that is fairly long text",
        50.0,
        100.0,
        10.0,
    ));
    items.push(Item::Fig(FigBox {
        x0: 50.0,
        x1: 250.0,
        top: 120.0,
        bottom: 200.0,
        index: 0,
    }));
    items.push(sp(
        "left tail line after the figure here",
        50.0,
        210.0,
        10.0,
    ));
    let out = layout(&items);
    let fig_at = out.iter().position(|o| matches!(o, Out::Fig(_))).unwrap();
    let right_first = out
        .iter()
        .position(|o| matches!(o, Out::Block(Block::Para(t)) if t.starts_with("right column line number 0")))
        .unwrap();
    let tail = out
        .iter()
        .position(|o| matches!(o, Out::Block(Block::Para(t)) if t.starts_with("left tail")))
        .unwrap();
    assert!(
        fig_at < tail && tail < right_first,
        "{fig_at} {tail} {right_first}"
    );
}

#[test]
fn a_centred_line_under_a_paragraph_is_not_a_column() {
    let items = vec![
        sp("After figure paragraph.", 50.0, 100.0, 10.0),
        sp("Figure 1: Sales chart", 250.0, 88.0, 10.0),
    ];
    assert_eq!(
        render(layout(&items)),
        "Figure 1: Sales chart\n\nAfter figure paragraph."
    );
}
