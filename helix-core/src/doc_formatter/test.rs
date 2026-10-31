use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{Conceal, InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \n.fooo fooo fooo \n.fooo fooo fooo \n.fooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n.....foo3 foo4 \n.....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \n.foo3 foo4 foo5 \n.foo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n. \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx \n.....2xxxx3xxxx4x\n.....xxx5xxxx6xxx\n.....x7xxxx8xxxx9\n.....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxx 2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \n.ábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \n.foo Xoo foo foo \n.foo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

fn conceal_text(text: &str, softwrap: bool, conceals: &[Conceal]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_conceals(conceals, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn conceal() {
    // hide markup entirely
    assert_eq!(
        conceal_text(
            "a **b** c\n",
            false,
            &[Conceal::new(2, 4, ""), Conceal::new(5, 7, "")]
        ),
        "a b c \n "
    );
    // a replacement takes one cell
    assert_eq!(
        conceal_text("\\alpha + 1\n", false, &[Conceal::new(0, 6, "α")]),
        "α + 1 \n "
    );
    // a conceal never crosses a line break
    assert_eq!(
        conceal_text("ab\ncd\n", false, &[Conceal::new(1, 5, "")]),
        "a \ncd \n "
    );
    // multi-codepoint graphemes are swallowed whole
    assert_eq!(
        conceal_text("x👨‍👩‍👧y\n", false, &[Conceal::new(1, 6, "…")]),
        "x…y \n "
    );
    // overlapping conceals: the first to start wins
    assert_eq!(
        conceal_text(
            "abcdef\n",
            false,
            &[Conceal::new(1, 3, ""), Conceal::new(2, 5, "X")]
        ),
        "adef \n "
    );
}

#[test]
fn conceal_positions() {
    let text = "a **b** c\n";
    let conceals = [Conceal::new(2, 4, ""), Conceal::new(5, 7, "")];
    let mut annotations = TextAnnotations::default();
    annotations.add_conceals(&conceals, None);
    let fmt = TextFormat::new_test(false);
    let graphemes: Vec<_> =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &fmt, &annotations, 0)
            .map(|g| (g.char_idx, g.visual_pos.col, g.doc_chars(), g.width()))
            .collect();
    assert_eq!(
        graphemes,
        [
            (0, 0, 1, 1),
            (1, 1, 1, 1),
            (2, 2, 2, 0), // `**` hidden
            (4, 2, 1, 1),
            (5, 3, 2, 0), // `**` hidden
            (7, 3, 1, 1),
            (8, 4, 1, 1),
            (9, 5, 1, 1),
            (10, 0, 0, 1),
        ]
    );
}

#[test]
fn conceal_with_annotations_inside() {
    // inline annotations and overlays inside hidden text are not shown
    let conceals = [Conceal::new(1, 4, "")];
    let inline = [
        InlineAnnotation::new(2, "VIRT"),
        InlineAnnotation::new(4, "<"),
    ];
    let overlays = [Overlay::new(3, "O"), Overlay::new(5, "P")];
    let fmt = TextFormat::new_test(false);
    let mut annotations = TextAnnotations::default();
    annotations
        .add_conceals(&conceals, None)
        .add_inline_annotations(&inline, None)
        .add_overlay(&overlays, None);
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint("abcdefg\n".into(), &fmt, &annotations, 0)
            .collect_to_str(),
        "a<ePg \n "
    );
}

#[test]
fn conceal_softwrap() {
    // concealed text takes no columns when wrapping
    let text = format!("{}\n", "**foo** ".repeat(4));
    let conceals: Vec<_> = (0..4)
        .flat_map(|i| {
            [
                Conceal::new(i * 8, i * 8 + 2, ""),
                Conceal::new(i * 8 + 5, i * 8 + 7, ""),
            ]
        })
        .collect();
    assert_eq!(conceal_text(&text, true, &conceals), "foo foo foo foo  \n ");
}

#[test]
fn conceal_position_round_trip() {
    use crate::position::{char_idx_at_visual_block_offset, visual_offset_from_block};
    let text = crate::Rope::from("a **bold** and [link](https://x.y) end\nnext **x**\n");
    let conceals = [
        Conceal::new(2, 4, ""),
        Conceal::new(8, 10, ""),
        Conceal::new(15, 16, ""),
        Conceal::new(20, 34, "…"),
        Conceal::new(44, 46, ""),
        Conceal::new(47, 49, ""),
    ];
    let mut annotations = TextAnnotations::default();
    annotations.add_conceals(&conceals, None);
    for soft_wrap in [false, true] {
        let fmt = TextFormat::new_test(soft_wrap);
        for pos in 0..text.len_chars() {
            let (visual, _) = visual_offset_from_block(text.slice(..), 0, pos, &fmt, &annotations);
            let (back, _) = char_idx_at_visual_block_offset(
                text.slice(..),
                0,
                visual.row,
                visual.col,
                &fmt,
                &annotations,
            );
            // a hidden position maps to the first visible position after it
            // (or the replacement's start); visible positions round-trip
            match conceals.iter().find(|c| (c.start..c.end).contains(&pos)) {
                Some(c) if c.replacement.is_empty() => assert_eq!(back, c.end, "pos {pos}"),
                Some(c) => assert_eq!(back, c.start, "pos {pos}"),
                None => assert_eq!(back, pos, "pos {pos}"),
            }
        }
    }
}

#[test]
fn folds_hide_whole_lines() {
    // "a {" then two folded lines, then "}": the fold runs from the end of
    // line 0's text to the end of line 2's.
    let text = "a {\nb\nc\n}\n";
    let folds = [Conceal::fold(3, 7, "⋯")];
    assert_eq!(conceal_text(text, false, &folds), "a {⋯ \n} \n ");
    // Folded lines still count: "}" is line 3.
    let mut annotations = TextAnnotations::default();
    annotations.add_conceals(&folds, None);
    let fmt = TextFormat::new_test(false);
    let last = DocumentFormatter::new_at_prev_checkpoint(text.into(), &fmt, &annotations, 0)
        .find(|g| g.char_idx == 8)
        .unwrap();
    assert_eq!((last.line_idx, last.visual_pos.row), (3, 1));
    // Positions inside the fold are on the marker's row.
    let inside = DocumentFormatter::new_at_prev_checkpoint(text.into(), &fmt, &annotations, 0)
        .find(|g| g.char_idx == 3)
        .unwrap();
    assert_eq!(inside.visual_pos.row, 0);
    // A plain conceal still stops at the line break.
    assert_eq!(
        conceal_text("ab\ncd\n", false, &[Conceal::new(1, 5, "")]),
        "a \ncd \n "
    );
}
