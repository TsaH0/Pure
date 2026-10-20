//! Decorations plugins put on a document: highlights, virtual text and
//! gutter signs, in layers named `<plugin>/<layer>`. Positions are char
//! offsets, mapped through every edit like inlay hints, so they stay in place
//! while the user types until the plugin sends new ones. An empty store
//! costs nothing (`is_empty` is checked before any work).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU8, Ordering};

use helix_core::syntax::OverlayHighlights;
use helix_core::text_annotations::{Conceal, InlineAnnotation, Overlay, TextAnnotations};
use helix_core::{Assoc, ChangeSet, RopeSlice};

use crate::document::Mode;
use crate::graphics::Style;
use crate::Theme;

fn mode_bit(mode: Mode) -> u8 {
    match mode {
        Mode::Normal => 1,
        Mode::Select => 2,
        Mode::Insert => 4,
    }
}

/// The mode the document was last drawn in, for layers drawn only in some
/// modes. Set by the renderer each frame; motions computed between frames
/// see what is on screen.
#[derive(Debug, Default)]
struct RenderMode(AtomicU8);

impl RenderMode {
    fn get(&self) -> u8 {
        match self.0.load(Ordering::Relaxed) {
            0 => mode_bit(Mode::Normal),
            bit => bit,
        }
    }
}

impl Clone for RenderMode {
    fn clone(&self) -> Self {
        Self(AtomicU8::new(self.0.load(Ordering::Relaxed)))
    }
}

/// One layer's virtual text, grouped by scope (one style per group, which is
/// what `TextAnnotations` takes).
#[derive(Debug, Default, Clone)]
pub struct AnnotationLayer {
    /// `(scope, annotations sorted by position)`.
    pub groups: Vec<(String, Vec<InlineAnnotation>)>,
}

/// A gutter sign: shown on the line of `pos`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sign {
    pub pos: usize,
    pub text: String,
    pub scope: String,
}

#[derive(Debug, Default, Clone)]
pub struct PluginDecorations {
    /// Sorted, non-overlapping `(range, scope)` per layer.
    highlights: BTreeMap<String, Vec<(Range<usize>, String)>>,
    annotations: BTreeMap<String, AnnotationLayer>,
    /// Chars drawn over the text: `(scope, overlays sorted by position)`
    /// per layer.
    overlays: BTreeMap<String, Vec<(String, Vec<Overlay>)>>,
    /// Sorted by position per layer.
    signs: BTreeMap<String, Vec<Sign>>,
    /// Whole-line backgrounds: `(line start, theme scope)`, sorted per layer.
    line_backgrounds: BTreeMap<String, Vec<(usize, String)>>,
    /// Hidden text: `(scope, conceals sorted by start, not overlapping)`
    /// per layer.
    conceals: BTreeMap<String, Vec<(String, Vec<Conceal>)>>,
    /// Layers not drawn on the line with the view's cursor, so it shows the
    /// raw text (e.g. rendered Markdown).
    raw_on_cursor_line: std::collections::BTreeSet<String>,
    /// Layers drawn only in some modes: a mask of [`mode_bit`]s.
    render_modes: BTreeMap<String, u8>,
    mode: RenderMode,
}

/// The part of `items` (sorted by `pos`) outside `skip`: before and after.
fn outside<'a, T>(
    items: &'a [T],
    skip: &Option<Range<usize>>,
    pos: impl Fn(&T) -> usize,
) -> [&'a [T]; 2] {
    match skip {
        Some(skip) => {
            let from = items.partition_point(|item| pos(item) < skip.start);
            let to = items.partition_point(|item| pos(item) < skip.end);
            [&items[..from], &items[to..]]
        }
        None => [items, &[]],
    }
}

/// The old-text ranges `changes` deletes, and the points where it inserts
/// (empty ranges), in order.
fn edited_spans(changes: &ChangeSet) -> Vec<Range<usize>> {
    use helix_core::Operation;
    let mut spans = Vec::new();
    let mut pos = 0;
    for change in changes.changes() {
        match change {
            Operation::Retain(n) => pos += n,
            Operation::Delete(n) => {
                spans.push(pos..pos + n);
                pos += n;
            }
            Operation::Insert(_) => spans.push(pos..pos),
        }
    }
    spans
}

/// Whether an edit in `spans` changes text inside `start..end`: a deletion
/// overlapping it, or an insertion strictly inside (one at either edge only
/// adds text next to it).
fn touches(spans: &[Range<usize>], start: usize, end: usize) -> bool {
    // The first span that doesn't end before `start`.
    let first = spans.partition_point(|span| span.end < start);
    spans[first..]
        .iter()
        .take_while(|span| span.start <= end)
        .any(|span| {
            if span.is_empty() {
                start < span.start && span.start < end
            } else {
                span.start < end && span.end > start
            }
        })
}

impl PluginDecorations {
    pub fn is_empty(&self) -> bool {
        self.highlights.is_empty()
            && self.annotations.is_empty()
            && self.overlays.is_empty()
            && self.signs.is_empty()
            && self.line_backgrounds.is_empty()
            && self.conceals.is_empty()
    }

    /// Whether `layer` is drawn in the current mode.
    fn shown(&self, layer: &str) -> bool {
        self.render_modes
            .get(layer)
            .is_none_or(|modes| modes & self.mode.get() != 0)
    }

    /// Record the mode the document is drawn in (for
    /// [`set_render_modes`](Self::set_render_modes)).
    pub fn set_render_mode(&self, mode: Mode) {
        self.mode.0.store(mode_bit(mode), Ordering::Relaxed);
    }

    /// Replace `layer`'s highlights. Ranges past `len` are dropped, and later
    /// ranges overlapping earlier ones are dropped (overlays can't overlap).
    pub fn set_highlights(
        &mut self,
        layer: String,
        mut highlights: Vec<(Range<usize>, String)>,
        len: usize,
    ) {
        highlights.retain(|(range, _)| range.start < range.end && range.end <= len);
        highlights.sort_by_key(|(range, _)| (range.start, range.end));
        let mut end = 0;
        highlights.retain(|(range, _)| {
            let keep = range.start >= end;
            if keep {
                end = range.end;
            }
            keep
        });
        if highlights.is_empty() {
            self.highlights.remove(&layer);
        } else {
            self.highlights.insert(layer, highlights);
        }
    }

    /// Replace `layer`'s virtual text: `(position, text, scope)`.
    pub fn set_annotations(
        &mut self,
        layer: String,
        annotations: Vec<(usize, String, String)>,
        len: usize,
    ) {
        let mut groups: BTreeMap<String, Vec<InlineAnnotation>> = BTreeMap::new();
        for (pos, text, scope) in annotations {
            if pos <= len && !text.is_empty() {
                groups
                    .entry(scope)
                    .or_default()
                    .push(InlineAnnotation::new(pos, text));
            }
        }
        if groups.is_empty() {
            self.annotations.remove(&layer);
            return;
        }
        let groups = groups
            .into_iter()
            .map(|(scope, mut annotations)| {
                annotations.sort_by_key(|annotation| annotation.char_idx);
                (scope, annotations)
            })
            .collect();
        self.annotations.insert(layer, AnnotationLayer { groups });
    }

    /// Replace `layer`'s overlays: `(position, text, scope)`, each char of
    /// `text` over one char of the document. Later overlays on a char
    /// already covered are dropped.
    pub fn set_overlays(
        &mut self,
        layer: String,
        overlays: Vec<(usize, String, String)>,
        len: usize,
    ) {
        let mut chars: Vec<(usize, char, String)> = Vec::new();
        for (pos, text, scope) in overlays {
            for (i, ch) in text.chars().enumerate() {
                if pos + i < len {
                    chars.push((pos + i, ch, scope.clone()));
                }
            }
        }
        chars.sort_by_key(|(pos, ..)| *pos);
        chars.dedup_by_key(|(pos, ..)| *pos);
        let mut groups: BTreeMap<String, Vec<Overlay>> = BTreeMap::new();
        for (pos, ch, scope) in chars {
            groups
                .entry(scope)
                .or_default()
                .push(Overlay::new(pos, ch.to_string()));
        }
        if groups.is_empty() {
            self.overlays.remove(&layer);
        } else {
            self.overlays.insert(layer, groups.into_iter().collect());
        }
    }

    /// Replace `layer`'s line backgrounds: `(char position of the line
    /// start, theme scope)`.
    pub fn set_line_backgrounds(
        &mut self,
        layer: String,
        mut lines: Vec<(usize, String)>,
        len: usize,
    ) {
        lines.retain(|(pos, _)| *pos <= len);
        lines.sort_by_key(|(pos, _)| *pos);
        lines.dedup_by_key(|(pos, _)| *pos);
        if lines.is_empty() {
            self.line_backgrounds.remove(&layer);
        } else {
            self.line_backgrounds.insert(layer, lines);
        }
    }

    /// The background style of each line in `lines` (first layer wins);
    /// raw-on-cursor-line layers skip `cursor_line`.
    pub fn line_backgrounds(
        &self,
        text: RopeSlice,
        theme: &Theme,
        lines: Range<usize>,
        cursor_line: Option<usize>,
    ) -> BTreeMap<usize, Style> {
        let mut styles = BTreeMap::new();
        if lines.start >= text.len_lines() {
            return styles;
        }
        let from = text.line_to_char(lines.start);
        let to = text.line_to_char(lines.end.min(text.len_lines()));
        for (layer, backgrounds) in &self.line_backgrounds {
            if !self.shown(layer) {
                continue;
            }
            let raw = self.raw_on_cursor_line.contains(layer);
            let first = backgrounds.partition_point(|(pos, _)| *pos < from);
            for (pos, scope) in backgrounds[first..].iter().take_while(|(pos, _)| *pos < to) {
                let line = text.char_to_line(*pos);
                if raw && Some(line) == cursor_line {
                    continue;
                }
                // Comma-separated scopes: the first the theme has.
                if let Some(style) = scope.split(',').find_map(|s| theme.try_get(s.trim())) {
                    styles.entry(line).or_insert(style);
                }
            }
        }
        styles
    }

    /// Replace `layer`'s conceals: `(char range, replacement, scope)`.
    /// Ranges are expected within one line and grapheme aligned (the host
    /// checks); later ranges overlapping earlier ones are dropped.
    pub fn set_conceals(
        &mut self,
        layer: String,
        mut conceals: Vec<(Range<usize>, String, String)>,
        len: usize,
    ) {
        conceals.retain(|(range, ..)| range.start < range.end && range.end <= len);
        conceals.sort_by_key(|(range, ..)| (range.start, range.end));
        let mut end = 0;
        let mut groups: BTreeMap<String, Vec<Conceal>> = BTreeMap::new();
        for (range, replacement, scope) in conceals {
            if range.start >= end {
                end = range.end;
                groups.entry(scope).or_default().push(Conceal::new(
                    range.start,
                    range.end,
                    replacement,
                ));
            }
        }
        if groups.is_empty() {
            self.conceals.remove(&layer);
        } else {
            self.conceals.insert(layer, groups.into_iter().collect());
        }
    }

    /// The modes `layer` is drawn in; empty means all.
    pub fn set_render_modes(&mut self, layer: String, modes: &[Mode]) {
        let mask = modes.iter().fold(0, |mask, mode| mask | mode_bit(*mode));
        if mask == 0 || mask == 7 {
            self.render_modes.remove(&layer);
        } else {
            self.render_modes.insert(layer, mask);
        }
    }

    /// Replace `layer`'s gutter signs.
    pub fn set_signs(&mut self, layer: String, mut signs: Vec<Sign>, len: usize) {
        signs.retain(|sign| sign.pos <= len && !sign.text.is_empty());
        signs.sort_by_key(|sign| sign.pos);
        if signs.is_empty() {
            self.signs.remove(&layer);
        } else {
            self.signs.insert(layer, signs);
        }
    }

    /// Whether `layer` is hidden on the cursor's line.
    pub fn set_raw_on_cursor_line(&mut self, layer: String, raw: bool) {
        if raw {
            self.raw_on_cursor_line.insert(layer);
        } else {
            self.raw_on_cursor_line.remove(&layer);
        }
    }

    /// Remove every layer of `plugin` (it was unloaded or reloaded).
    pub fn clear_plugin(&mut self, plugin: &str) {
        let prefix = format!("{plugin}/");
        self.highlights
            .retain(|layer, _| !layer.starts_with(&prefix));
        self.annotations
            .retain(|layer, _| !layer.starts_with(&prefix));
        self.overlays.retain(|layer, _| !layer.starts_with(&prefix));
        self.signs.retain(|layer, _| !layer.starts_with(&prefix));
        self.line_backgrounds
            .retain(|layer, _| !layer.starts_with(&prefix));
        self.conceals.retain(|layer, _| !layer.starts_with(&prefix));
        self.raw_on_cursor_line
            .retain(|layer| !layer.starts_with(&prefix));
        self.render_modes
            .retain(|layer, _| !layer.starts_with(&prefix));
    }

    /// Follow an edit: `len` is the new text's length.
    pub fn map(&mut self, changes: &ChangeSet, len: usize) {
        // Each layer's positions go through the changes in one pass (they
        // are mostly sorted): O(decorations + changes), not their product.
        for highlights in self.highlights.values_mut() {
            changes.update_positions(highlights.iter_mut().flat_map(|(range, _)| {
                [
                    (&mut range.start, Assoc::After),
                    (&mut range.end, Assoc::Before),
                ]
            }));
            highlights.retain(|(range, _)| range.start < range.end && range.end <= len);
        }
        self.highlights
            .retain(|_, highlights| !highlights.is_empty());
        for layer in self.annotations.values_mut() {
            for (_, annotations) in layer.groups.iter_mut() {
                changes.update_positions(
                    annotations
                        .iter_mut()
                        .map(|annotation| (&mut annotation.char_idx, Assoc::After)),
                );
            }
        }
        for groups in self.overlays.values_mut() {
            for (_, overlays) in groups.iter_mut() {
                // Drawn over a char: it moves with the char, and goes when the
                // char is deleted (its start and end then map to one place).
                let mut ends: Vec<usize> = overlays.iter().map(|o| o.char_idx + 1).collect();
                changes.update_positions(
                    overlays
                        .iter_mut()
                        .map(|overlay| &mut overlay.char_idx)
                        .zip(ends.iter_mut())
                        .flat_map(|(start, end)| [(start, Assoc::After), (end, Assoc::Before)]),
                );
                let mut ends = ends.into_iter();
                overlays.retain(|overlay| {
                    let end = ends.next().unwrap();
                    end == overlay.char_idx + 1 && overlay.char_idx < len
                });
            }
        }
        if !self.conceals.is_empty() {
            let edited = edited_spans(changes);
            for groups in self.conceals.values_mut() {
                for (_, conceals) in groups.iter_mut() {
                    // Hidden text that is edited (even keeping its length)
                    // shows again until the plugin sets the layer anew.
                    conceals.retain(|conceal| !touches(&edited, conceal.start, conceal.end));
                    changes.update_positions(conceals.iter_mut().flat_map(|conceal| {
                        [
                            (&mut conceal.start, Assoc::After),
                            (&mut conceal.end, Assoc::Before),
                        ]
                    }));
                    conceals.retain(|conceal| conceal.start < conceal.end && conceal.end <= len);
                }
                groups.retain(|(_, conceals)| !conceals.is_empty());
            }
        }
        self.conceals.retain(|_, groups| !groups.is_empty());
        for lines in self.line_backgrounds.values_mut() {
            changes.update_positions(lines.iter_mut().map(|(pos, _)| (pos, Assoc::After)));
        }
        for signs in self.signs.values_mut() {
            changes.update_positions(signs.iter_mut().map(|sign| (&mut sign.pos, Assoc::After)));
        }
    }

    /// Overlays for the highlights intersecting `visible` (char range);
    /// raw-on-cursor-line layers skip `cursor_line`.
    pub fn overlays_into(
        &self,
        theme: &Theme,
        visible: Range<usize>,
        cursor_line: Option<Range<usize>>,
        overlays: &mut Vec<OverlayHighlights>,
    ) {
        for (layer, highlights) in &self.highlights {
            if !self.shown(layer) {
                continue;
            }
            let skip = cursor_line
                .clone()
                .filter(|_| self.raw_on_cursor_line.contains(layer));
            // Sorted and non-overlapping: ends are sorted too.
            let first = highlights.partition_point(|(range, _)| range.end <= visible.start);
            let shown: Vec<_> = highlights[first..]
                .iter()
                .take_while(|(range, _)| range.start < visible.end)
                .filter(|(range, _)| {
                    skip.as_ref()
                        .is_none_or(|skip| range.end <= skip.start || range.start >= skip.end)
                })
                .filter_map(|(range, scope)| Some((theme.find_highlight(scope)?, range.clone())))
                .collect();
            if !shown.is_empty() {
                overlays.push(OverlayHighlights::Heterogenous { highlights: shown });
            }
        }
    }

    /// Add the virtual text and overlays to `text_annotations`;
    /// raw-on-cursor-line layers skip `cursor_line`.
    pub fn annotate<'a>(
        &'a self,
        theme: Option<&Theme>,
        cursor_line: Option<Range<usize>>,
        text_annotations: &mut TextAnnotations<'a>,
    ) {
        let skip = |layer: &String| {
            cursor_line
                .clone()
                .filter(|_| self.raw_on_cursor_line.contains(layer))
        };
        for (layer, groups) in &self.overlays {
            if !self.shown(layer) {
                continue;
            }
            let skip = skip(layer);
            for (scope, overlays) in groups {
                let style = theme.and_then(|theme| theme.find_highlight(scope));
                for part in outside(overlays, &skip, |overlay| overlay.char_idx) {
                    text_annotations.add_overlay(part, style);
                }
            }
        }
        for (layer, groups) in &self.conceals {
            if !self.shown(layer) {
                continue;
            }
            let skip = skip(layer);
            for (scope, conceals) in groups {
                let style = theme.and_then(|theme| theme.find_highlight(scope));
                for part in outside(conceals, &skip, |conceal| conceal.start) {
                    text_annotations.add_conceals(part, style);
                }
            }
        }
        for (layer, annotations) in &self.annotations {
            if !self.shown(layer) {
                continue;
            }
            let skip = skip(layer);
            for (scope, annotations) in &annotations.groups {
                let style = theme.and_then(|theme| theme.find_highlight(scope));
                // End-of-line text sits at the line break: part of the line.
                let skip = skip.clone().map(|range| range.start..range.end + 1);
                for part in outside(annotations, &skip, |annotation| annotation.char_idx) {
                    text_annotations.add_inline_annotations(part, style);
                }
            }
        }
    }

    /// Whether any layer is raw on the cursor line (then the caller needs to
    /// know that line).
    pub fn wants_cursor_line(&self) -> bool {
        !self.raw_on_cursor_line.is_empty()
    }

    /// The signs by line, for the `plugins` gutter: the first layer's sign on
    /// a line wins.
    pub fn signs_by_line(&self, text: RopeSlice) -> BTreeMap<usize, (&str, &str)> {
        let mut by_line = BTreeMap::new();
        let len = text.len_chars();
        for (layer, signs) in &self.signs {
            if !self.shown(layer) {
                continue;
            }
            for sign in signs {
                let line = text.char_to_line(sign.pos.min(len));
                by_line
                    .entry(line)
                    .or_insert((sign.text.as_str(), sign.scope.as_str()));
            }
        }
        by_line
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use helix_core::{Rope, Transaction};

    #[test]
    fn highlights_are_cleaned_and_follow_edits() {
        let mut text = Rope::from("hello world\n");
        let mut decorations = PluginDecorations::default();
        decorations.set_highlights(
            "p/l".into(),
            vec![
                (6..11, "b".into()),
                (0..5, "a".into()),
                (2..4, "overlap".into()),
                (3..99, "out of range".into()),
            ],
            text.len_chars(),
        );
        assert_eq!(
            decorations.highlights["p/l"],
            [(0..5, "a".to_string()), (6..11, "b".to_string())]
        );
        // Insert 3 chars at the start: everything moves.
        let transaction = Transaction::change(&text, [(0, 0, Some("// ".into()))].into_iter());
        transaction.apply(&mut text);
        decorations.map(transaction.changes(), text.len_chars());
        assert_eq!(
            decorations.highlights["p/l"],
            [(3..8, "a".to_string()), (9..14, "b".to_string())]
        );
        // Deleting a highlighted word removes its highlight.
        let transaction = Transaction::change(&text, [(9, 14, None)].into_iter());
        transaction.apply(&mut text);
        decorations.map(transaction.changes(), text.len_chars());
        assert_eq!(decorations.highlights["p/l"], [(3..8, "a".to_string())]);

        decorations.clear_plugin("p");
        assert!(decorations.is_empty());
    }

    #[test]
    fn overlays_follow_their_chars() {
        let mut text = Rope::from("# a\n- b\n");
        let mut decorations = PluginDecorations::default();
        decorations.set_overlays(
            "md/marks".into(),
            vec![(0, "◉".into(), "h".into()), (4, "•".into(), "l".into())],
            text.len_chars(),
        );
        let positions = |d: &PluginDecorations| -> Vec<usize> {
            d.overlays["md/marks"]
                .iter()
                .flat_map(|(_, overlays)| overlays.iter().map(|o| o.char_idx))
                .collect()
        };
        assert_eq!(positions(&decorations), [0, 4]);
        // Typing before the bullet moves it; deleting the heading's `#` drops
        // its overlay.
        let transaction =
            Transaction::change(&text, [(0, 1, None), (4, 4, Some("x".into()))].into_iter());
        transaction.apply(&mut text);
        decorations.map(transaction.changes(), text.len_chars());
        assert_eq!(positions(&decorations), [4]);
        assert_eq!(text.char(4), '-');
    }

    #[test]
    fn signs_and_annotations() {
        let text = Rope::from("a\nb\nc\n");
        let mut decorations = PluginDecorations::default();
        let sign = |pos, text: &str| Sign {
            pos,
            text: text.into(),
            scope: "s".into(),
        };
        decorations.set_signs("p/one".into(), vec![sign(2, "x"), sign(0, "y")], 6);
        decorations.set_signs("p/two".into(), vec![sign(3, "z"), sign(4, "w")], 6);
        let by_line = decorations.signs_by_line(text.slice(..));
        assert_eq!(by_line[&0], ("y", "s"));
        assert_eq!(by_line[&1], ("x", "s"));
        assert_eq!(by_line[&2], ("w", "s"));

        decorations.set_annotations(
            "p/a".into(),
            vec![
                (5, "late".into(), "s".into()),
                (1, "early".into(), "s".into()),
            ],
            6,
        );
        let groups = &decorations.annotations["p/a"].groups;
        assert_eq!(groups[0].1[0].char_idx, 1);
        decorations.set_annotations("p/a".into(), vec![], 6);
        assert!(!decorations.annotations.contains_key("p/a"));
    }

    #[test]
    fn raw_layers_skip_the_cursor_line() {
        let items = [0, 3, 4, 7, 9];
        let parts = outside(&items, &Some(3..8), |i| *i);
        assert_eq!(parts, [&[0][..], &[9][..]]);
        assert_eq!(outside(&items, &None, |i| *i), [&items[..], &[][..]]);

        let mut decorations = PluginDecorations::default();
        assert!(!decorations.wants_cursor_line());
        decorations.set_raw_on_cursor_line("md/marks".into(), true);
        assert!(decorations.wants_cursor_line());
        decorations.clear_plugin("md");
        assert!(!decorations.wants_cursor_line());
    }

    #[test]
    fn line_backgrounds_follow_edits_and_skip_the_raw_cursor_line() {
        let mut text = Rope::from("a\nb\nc\n");
        let mut decorations = PluginDecorations::default();
        let theme: Theme = toml::from_str(r#""ui.selection" = { bg = "red" }"#).unwrap();
        decorations.set_line_backgrounds(
            "md/blocks".into(),
            vec![(2, "ui.selection".into()), (4, "ui.selection".into())],
            text.len_chars(),
        );
        let lines = |d: &PluginDecorations, text: &Rope, cursor| {
            d.line_backgrounds(text.slice(..), &theme, 0..10, cursor)
                .into_keys()
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(&decorations, &text, None), [1, 2]);
        // A new line above moves them down.
        let transaction = Transaction::change(&text, [(0, 0, Some("x\n".into()))].into_iter());
        transaction.apply(&mut text);
        decorations.map(transaction.changes(), text.len_chars());
        assert_eq!(lines(&decorations, &text, None), [2, 3]);
        // Only the visible lines are looked at.
        assert!(decorations
            .line_backgrounds(text.slice(..), &theme, 3..4, None)
            .contains_key(&3));
        decorations.set_raw_on_cursor_line("md/blocks".into(), true);
        assert_eq!(lines(&decorations, &text, Some(2)), [3]);
    }

    #[test]
    fn conceals_follow_edits_and_render_modes() {
        use helix_core::doc_formatter::{DocumentFormatter, TextFormat};
        use helix_core::{Rope, Transaction};

        let shown = |decorations: &PluginDecorations, text: &Rope, cursor_line| {
            let mut annotations = TextAnnotations::default();
            decorations.annotate(None, cursor_line, &mut annotations);
            let format = TextFormat::default();
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0)
                .map(|g| g.raw.to_string())
                .collect::<String>()
        };
        let mut text = Rope::from("a **b** c\n");
        let mut decorations = PluginDecorations::default();
        decorations.set_conceals(
            "md/conceal".into(),
            vec![
                (2..4, String::new(), "markup.bold".into()),
                (5..7, String::new(), "markup.bold".into()),
                // overlapping: dropped
                (3..6, "x".into(), "markup.bold".into()),
            ],
            text.len_chars(),
        );
        assert!(!decorations.is_empty());
        assert_eq!(shown(&decorations, &text, None), "a b c  ");
        decorations.set_raw_on_cursor_line("md/conceal".into(), true);
        assert_eq!(shown(&decorations, &text, Some(0..9)), "a **b** c  ");

        // Typing before a conceal moves it; typing inside one drops it.
        let edit = Transaction::insert(&text, &helix_core::Selection::point(0), "z".into());
        decorations.map(edit.changes(), text.len_chars() + 1);
        edit.apply(&mut text);
        assert_eq!(shown(&decorations, &text, None), "za b c  ");
        let edit = Transaction::insert(&text, &helix_core::Selection::point(4), "y".into());
        decorations.map(edit.changes(), text.len_chars() + 1);
        edit.apply(&mut text);
        assert_eq!(shown(&decorations, &text, None), "za *y*b c  ");

        // Drawn in normal mode only.
        decorations.set_render_modes("md/conceal".into(), &[Mode::Normal]);
        decorations.set_render_mode(Mode::Insert);
        assert_eq!(shown(&decorations, &text, None), "za *y*b** c  ");
        decorations.set_render_mode(Mode::Normal);
        assert_eq!(shown(&decorations, &text, None), "za *y*b c  ");
        decorations.clear_plugin("md");
        assert!(decorations.is_empty());
    }

    #[test]
    fn edits_inside_hidden_text_show_it_even_keeping_the_length() {
        let text = Rope::from("a **b** c\n");
        let conceals = |decorations: &PluginDecorations| -> Vec<(usize, usize)> {
            decorations.conceals["md/c"]
                .iter()
                .flat_map(|(_, c)| c.iter().map(|c| (c.start, c.end)))
                .collect()
        };
        let set = || {
            let mut decorations = PluginDecorations::default();
            decorations.set_conceals(
                "md/c".into(),
                vec![
                    (2..4, String::new(), "s".into()),
                    (5..7, String::new(), "s".into()),
                ],
                text.len_chars(),
            );
            decorations
        };
        // `*` replaced by `_`: same length, inside the first range.
        let mut decorations = set();
        let edit = Transaction::change(&text, [(3, 4, Some("_".into()))].into_iter());
        decorations.map(edit.changes(), text.len_chars());
        assert_eq!(conceals(&decorations), [(5, 7)]);
        // Typing right before or right after a range keeps it.
        let mut decorations = set();
        let edit = Transaction::change(
            &text,
            [(2, 2, Some("x".into())), (7, 7, Some("y".into()))].into_iter(),
        );
        decorations.map(edit.changes(), text.len_chars() + 2);
        assert_eq!(conceals(&decorations), [(3, 5), (6, 8)]);
        // Deleting into a range from outside drops it.
        let mut decorations = set();
        let edit = Transaction::change(&text, [(1, 3, None)].into_iter());
        decorations.map(edit.changes(), text.len_chars() - 2);
        assert_eq!(conceals(&decorations), [(3, 5)]);
    }

    #[test]
    fn batched_mapping_matches_mapping_each_position() {
        let mut seed: u64 = 7;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n.max(1)
        };
        for _ in 0..200 {
            let text = Rope::from("x".repeat(200));
            let mut ranges: Vec<Range<usize>> = (0..20)
                .map(|_| {
                    let start = next(190);
                    start..start + 1 + next(9)
                })
                .collect();
            ranges.sort_by_key(|r| r.start);
            let mut decorations = PluginDecorations::default();
            decorations.set_highlights(
                "p/h".into(),
                ranges
                    .iter()
                    .map(|r| (r.clone(), "s".to_string()))
                    .collect(),
                text.len_chars(),
            );
            decorations.set_overlays(
                "p/o".into(),
                ranges
                    .iter()
                    .map(|r| (r.start, "*".to_string(), "s".to_string()))
                    .collect(),
                text.len_chars(),
            );
            let mut edits: Vec<(usize, usize, Option<helix_core::Tendril>)> = Vec::new();
            let mut pos = 0;
            while pos < 195 && edits.len() < 8 {
                pos += next(30);
                let len = next(4);
                if pos + len > 200 {
                    break;
                }
                let insert = if next(2) == 0 {
                    Some("ab".into())
                } else {
                    None
                };
                edits.push((pos, pos + len, insert));
                pos += len + 1;
            }
            let edit = Transaction::change(&text, edits.into_iter());
            let mut new = text.clone();
            edit.apply(&mut new);
            let expected_highlights: Vec<_> = decorations.highlights["p/h"]
                .iter()
                .map(|(r, _)| {
                    let start = edit.changes().map_pos(r.start, Assoc::After);
                    let end = edit.changes().map_pos(r.end, Assoc::Before);
                    start..end
                })
                .filter(|r| r.start < r.end && r.end <= new.len_chars())
                .collect();
            let expected_overlays: Vec<_> = decorations.overlays["p/o"]
                .iter()
                .flat_map(|(_, o)| o.iter())
                .filter_map(|o| {
                    let start = edit.changes().map_pos(o.char_idx, Assoc::After);
                    let end = edit.changes().map_pos(o.char_idx + 1, Assoc::Before);
                    (end == start + 1 && start < new.len_chars()).then_some(start)
                })
                .collect();
            decorations.map(edit.changes(), new.len_chars());
            let highlights: Vec<_> = decorations
                .highlights
                .get("p/h")
                .map(|h| h.iter().map(|(r, _)| r.clone()).collect())
                .unwrap_or_default();
            assert_eq!(highlights, expected_highlights);
            let overlays: Vec<_> = decorations
                .overlays
                .get("p/o")
                .map(|g| {
                    g.iter()
                        .flat_map(|(_, o)| o.iter().map(|o| o.char_idx))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(overlays, expected_overlays);
        }
    }
}
