//! Differential tests for `DocumentFormatter::skip_plain_text`.
//!
//! The position functions skip over plain text instead of traversing it grapheme by grapheme.
//! These tests check that they return exactly the same results (and call annotations exactly
//! the same way) as when every grapheme is traversed.

use std::cell::RefCell;
use std::rc::Rc;

use helix_core::doc_formatter::{set_skip_plain_text, FormattedGrapheme, TextFormat};
use helix_core::text_annotations::{InlineAnnotation, LineAnnotation, Overlay, TextAnnotations};
use helix_core::{
    char_idx_at_visual_offset, visual_offset_from_anchor, visual_offset_from_block, Position, Rope,
};
use unicode_segmentation::UnicodeSegmentation;

/// xorshift: deterministic and good enough for generating test cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "\t",
    "\n",
    "\r\n",
    " ",
    "  ",
    "e\u{301}",
    "\u{301}",
    "日本",
    "😶\u{200d}🌫\u{fe0f}",
    "ü",
    "\u{a0}",
    "\u{1}",
    "\u{7f}",
];

fn random_text(rng: &mut Rng) -> String {
    let mut text = String::new();
    for _ in 0..1 + rng.below(4) {
        // mostly long lines of plain ASCII
        let len = rng.below(3000);
        let mut line = String::new();
        while line.len() < len {
            if rng.below(40) == 0 {
                line.push_str(PIECES[rng.below(PIECES.len())]);
            } else {
                line.push((b' ' + rng.below(95) as u8) as char);
            }
        }
        text.push_str(&line);
        if rng.below(4) != 0 {
            text.push('\n');
        }
    }
    text
}

/// Records every call made to it and inserts virtual lines at its anchors.
struct RecordingAnnotation {
    anchors: Vec<usize>,
    next: usize,
    calls: Rc<RefCell<Vec<String>>>,
}

impl LineAnnotation for RecordingAnnotation {
    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.calls.borrow_mut().push(format!("reset {char_idx}"));
        self.next = self.anchors.partition_point(|&anchor| anchor < char_idx);
        self.anchors.get(self.next).copied().unwrap_or(usize::MAX)
    }

    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.calls
            .borrow_mut()
            .push(format!("skip {conceal_end_char_idx}"));
        self.next = self
            .anchors
            .partition_point(|&anchor| anchor < conceal_end_char_idx);
        self.anchors.get(self.next).copied().unwrap_or(usize::MAX)
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        self.calls.borrow_mut().push(format!(
            "anchor {} {:?}",
            grapheme.char_idx, grapheme.visual_pos
        ));
        self.next += 1;
        self.anchors.get(self.next).copied().unwrap_or(usize::MAX)
    }

    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position {
        self.calls.borrow_mut().push(format!(
            "lines {line_end_char_idx} {line_end_visual_pos:?} {doc_line}"
        ));
        Position::new(line_end_char_idx % 2, 0)
    }
}

struct Case {
    text: Rope,
    text_fmt: TextFormat,
    inline_annotations: Vec<InlineAnnotation>,
    overlays: Vec<Overlay>,
    anchors: Vec<usize>,
}

impl Case {
    fn random(rng: &mut Rng) -> Self {
        let text = Rope::from(random_text(rng));
        let len = text.len_chars();
        let text_fmt = TextFormat {
            soft_wrap: rng.below(4) == 0,
            tab_width: [1, 2, 4, 8][rng.below(4)],
            max_wrap: rng.below(20) as u16,
            max_indent_retain: rng.below(40) as u16,
            wrap_indicator: Box::from(["", "↪ ", " "][rng.below(3)]),
            wrap_indicator_highlight: None,
            viewport_width: 10 + rng.below(200) as u16,
            soft_wrap_at_text_width: rng.below(2) == 0,
        };
        // annotations are placed at grapheme boundaries
        let string = text.to_string();
        let mut boundaries: Vec<usize> = string
            .grapheme_indices(true)
            .map(|(byte_idx, _)| text.byte_to_char(byte_idx))
            .collect();
        boundaries.push(len);
        let positions = |rng: &mut Rng, _len: usize| {
            let count = rng.below(4);
            let mut positions: Vec<usize> = (0..count)
                .map(|_| boundaries[rng.below(boundaries.len())])
                .collect();
            positions.sort_unstable();
            positions
        };
        let inline_annotations = positions(rng, len)
            .into_iter()
            .map(|char_idx| InlineAnnotation::new(char_idx, "virtual"))
            .collect();
        let overlays = positions(rng, len)
            .into_iter()
            .map(|char_idx| Overlay::new(char_idx, "*"))
            .collect();
        let anchors = positions(rng, len);
        Case {
            text,
            text_fmt,
            inline_annotations,
            overlays,
            anchors,
        }
    }

    /// Runs `f` with the annotations of this case and returns its result together with all
    /// calls made to the line annotation.
    fn run<T>(&self, f: impl FnOnce(&TextAnnotations) -> T) -> (T, Vec<String>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut annotations = TextAnnotations::default();
        annotations.add_inline_annotations(&self.inline_annotations, None);
        annotations.add_overlay(&self.overlays, None);
        annotations.add_line_annotation(Box::new(RecordingAnnotation {
            anchors: self.anchors.clone(),
            next: 0,
            calls: calls.clone(),
        }));
        let result = f(&annotations);
        drop(annotations);
        let calls = calls.borrow().clone();
        (result, calls)
    }

    /// Checks that `f` behaves the same with and without skipping plain text.
    fn check<T: PartialEq + std::fmt::Debug>(&self, what: &str, f: impl Fn(&TextAnnotations) -> T) {
        set_skip_plain_text(false);
        let expected = self.run(&f);
        set_skip_plain_text(true);
        let actual = self.run(&f);
        assert_eq!(
            actual, expected,
            "{what} differs when skipping plain text\ntext: {:?}\nformat: {:?}",
            self.text, self.text_fmt
        );
    }
}

#[test]
fn skipping_plain_text_does_not_change_positions() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for _ in 0..400 {
        let case = Case::random(&mut rng);
        let text = case.text.slice(..);
        let len = text.len_chars();
        for _ in 0..20 {
            let anchor = rng.below(len + 1);
            let pos = rng.below(len + 1);
            let column = rng.below(4000);
            let max_rows = 1 + rng.below(10);
            let row_offset = rng.below(10) as isize - 4;
            let text_fmt = &case.text_fmt;
            case.check("visual_offset_from_block", |annotations| {
                visual_offset_from_block(text, anchor, pos, text_fmt, annotations)
            });
            case.check("visual_offset_from_anchor", |annotations| {
                visual_offset_from_anchor(text, anchor, pos, text_fmt, annotations, max_rows)
            });
            // also covers `char_idx_at_visual_block_offset`
            case.check("char_idx_at_visual_offset", |annotations| {
                char_idx_at_visual_offset(text, anchor, row_offset, column, text_fmt, annotations)
            });
        }
    }
}
