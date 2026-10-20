//! Decorations plugins put on a document: highlights, virtual text and
//! gutter signs, in layers named `<plugin>/<layer>`. Positions are char
//! offsets, mapped through every edit like inlay hints, so they stay in place
//! while the user types until the plugin sends new ones. An empty store
//! costs nothing (`is_empty` is checked before any work).

use std::collections::BTreeMap;
use std::ops::Range;

use helix_core::syntax::OverlayHighlights;
use helix_core::text_annotations::{InlineAnnotation, TextAnnotations};
use helix_core::{Assoc, ChangeSet, RopeSlice};

use crate::Theme;

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
    /// Sorted by position per layer.
    signs: BTreeMap<String, Vec<Sign>>,
}

impl PluginDecorations {
    pub fn is_empty(&self) -> bool {
        self.highlights.is_empty() && self.annotations.is_empty() && self.signs.is_empty()
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

    /// Remove every layer of `plugin` (it was unloaded or reloaded).
    pub fn clear_plugin(&mut self, plugin: &str) {
        let prefix = format!("{plugin}/");
        self.highlights
            .retain(|layer, _| !layer.starts_with(&prefix));
        self.annotations
            .retain(|layer, _| !layer.starts_with(&prefix));
        self.signs.retain(|layer, _| !layer.starts_with(&prefix));
    }

    /// Follow an edit: `len` is the new text's length.
    pub fn map(&mut self, changes: &ChangeSet, len: usize) {
        for highlights in self.highlights.values_mut() {
            for (range, _) in highlights.iter_mut() {
                changes.update_positions(
                    [
                        (&mut range.start, Assoc::After),
                        (&mut range.end, Assoc::Before),
                    ]
                    .into_iter(),
                );
            }
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
        for signs in self.signs.values_mut() {
            changes.update_positions(signs.iter_mut().map(|sign| (&mut sign.pos, Assoc::After)));
        }
    }

    /// Overlays for the highlights intersecting `visible` (char range).
    pub fn overlays_into(
        &self,
        theme: &Theme,
        visible: Range<usize>,
        overlays: &mut Vec<OverlayHighlights>,
    ) {
        for highlights in self.highlights.values() {
            // Sorted and non-overlapping: ends are sorted too.
            let first = highlights.partition_point(|(range, _)| range.end <= visible.start);
            let shown: Vec<_> = highlights[first..]
                .iter()
                .take_while(|(range, _)| range.start < visible.end)
                .filter_map(|(range, scope)| Some((theme.find_highlight(scope)?, range.clone())))
                .collect();
            if !shown.is_empty() {
                overlays.push(OverlayHighlights::Heterogenous { highlights: shown });
            }
        }
    }

    /// Add the virtual text to `text_annotations`.
    pub fn annotate<'a>(
        &'a self,
        theme: Option<&Theme>,
        text_annotations: &mut TextAnnotations<'a>,
    ) {
        for layer in self.annotations.values() {
            for (scope, annotations) in &layer.groups {
                let style = theme.and_then(|theme| theme.find_highlight(scope));
                text_annotations.add_inline_annotations(annotations, style);
            }
        }
    }

    /// The signs by line, for the `plugins` gutter: the first layer's sign on
    /// a line wins.
    pub fn signs_by_line(&self, text: RopeSlice) -> BTreeMap<usize, (&str, &str)> {
        let mut by_line = BTreeMap::new();
        let len = text.len_chars();
        for signs in self.signs.values() {
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
}
