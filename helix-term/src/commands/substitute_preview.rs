//! Live preview of `:s` while it is typed, like Neovim's `inccommand=split`:
//! the matches are highlighted in the buffer, and a panel at the bottom of the
//! editor shows the changed lines as they will be after the substitution
//! (or, while the pattern is still being typed, the lines with their matches).
//! The document is not touched until the command runs.

use std::ops::Range;

use helix_core::substitute::{self, Scope, Substitution};
use helix_core::{syntax::OverlayHighlights, Rope, Selection};
use helix_event::runtime_local;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::{DocumentId, Editor, Theme};
use parking_lot::Mutex;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

/// Lines kept for the panel (more than ever fit on a screen).
const MAX_LINES: usize = 200;

/// Don't preview over more text than this on every key press.
const MAX_SCOPE_CHARS: usize = 8 * 1024 * 1024;

struct Preview {
    doc: DocumentId,
    /// Char ranges of the matches in the document.
    matches: Vec<Range<usize>>,
    lines: Vec<Line>,
    /// Lines with a match, including those not kept in `lines`.
    total_lines: usize,
    title: String,
    error: bool,
}

/// One line of the panel.
struct Line {
    /// 0-based line in the document.
    number: usize,
    text: String,
    /// Char ranges in `text` to highlight: the replacements, or the matches.
    highlights: Vec<Range<usize>>,
}

runtime_local! {
    static PREVIEW: Mutex<Option<Preview>> = Mutex::new(None);
}

/// Recompute the preview for the command line `input`.
pub fn update(editor: &Editor, input: &str) {
    *PREVIEW.lock() = compute(editor, input);
}

/// The panel's title (like "2 matches on 1 line") and its lines, if a
/// preview is showing. For tests.
#[doc(hidden)]
pub fn snapshot() -> Option<(String, Vec<String>)> {
    let preview = PREVIEW.lock();
    let preview = preview.as_ref()?;
    Some((
        preview.title.clone(),
        preview.lines.iter().map(|line| line.text.clone()).collect(),
    ))
}

pub fn clear() {
    PREVIEW.lock().take();
}

/// The char ranges `:s` works on in `text` for `scope`: sorted and merged.
pub fn scopes(text: &Rope, selection: &Selection, scope: &Scope) -> Vec<(usize, usize)> {
    let line_span = |from_line: usize, to_line: usize| {
        let last = text.len_lines() - 1;
        let (from_line, to_line) = (from_line.min(last), to_line.min(last));
        let (from_line, to_line) = (from_line.min(to_line), from_line.max(to_line));
        (text.line_to_char(from_line), text.line_to_char(to_line + 1))
    };
    let mut scopes: Vec<(usize, usize)> = match scope {
        Scope::Document => vec![(0, text.len_chars())],
        Scope::Lines(from, to) => {
            let current = text.char_to_line(selection.primary().cursor(text.slice(..)));
            vec![line_span(from.unwrap_or(current), to.unwrap_or(current))]
        }
        Scope::Selections => selection
            .ranges()
            .iter()
            .map(|range| {
                if range.len() > 1 {
                    (range.from(), range.to())
                } else {
                    // Just a cursor: its line.
                    let line = text.char_to_line(range.cursor(text.slice(..)));
                    line_span(line, line)
                }
            })
            .collect(),
    };
    // Merge overlapping scopes (several cursors on one line).
    scopes.sort_unstable();
    scopes.dedup_by(|next, prev| {
        if next.0 <= prev.1 {
            prev.1 = prev.1.max(next.1);
            true
        } else {
            false
        }
    });
    scopes
}

fn compute(editor: &Editor, input: &str) -> Option<Preview> {
    let view = editor.tree.get(editor.tree.focus);
    let doc = editor.document(view.doc)?;
    let error = |message: String| {
        Some(Preview {
            doc: doc.id(),
            matches: Vec::new(),
            lines: Vec::new(),
            total_lines: 0,
            title: message,
            error: true,
        })
    };

    let command = match substitute::parse(input) {
        Ok(command) => command,
        Err(err) => return error(err),
    };
    // `s/` alone: nothing typed yet.
    if command.pattern.is_empty() && !command.has_replacement {
        return None;
    }
    let last_search = editor.registers.first('/', editor).map(|s| s.into_owned());
    let substitution = match Substitution::new(&command, last_search.as_deref()) {
        Ok(substitution) => substitution,
        Err(err) => return error(err),
    };

    let text = doc.text();
    let scopes = scopes(text, doc.selection(view.id), &command.scope);
    if scopes.iter().map(|(from, to)| to - from).sum::<usize>() > MAX_SCOPE_CHARS {
        return error("too much text to preview".into());
    }
    let outcome = substitution.apply(text, &scopes);

    let replacing = command.has_replacement && !command.flags.count_only;
    let mut lines = Vec::new();
    let mut i = 0;
    let changes = &outcome.changes;
    while i < changes.len() && lines.len() < MAX_LINES {
        // A hunk: whole lines covering a run of changes that share lines.
        let first_line = text.char_to_line(changes[i].0);
        let mut last_line = text.char_to_line(changes[i].1);
        let mut j = i + 1;
        while j < changes.len() && text.char_to_line(changes[j].0) <= last_line {
            last_line = last_line.max(text.char_to_line(changes[j].1));
            j += 1;
        }
        let start = text.line_to_char(first_line);
        let end = line_end(text, last_line);

        // The hunk's text after the changes, and where the highlights go.
        let mut new = String::new();
        let mut highlights = Vec::new();
        let mut len = 0;
        let mut pos = start;
        for (from, to, replacement) in &changes[i..j] {
            let before: String = text.slice(pos..*from).into();
            len += before.chars().count();
            new.push_str(&before);
            let shown: String = if replacing {
                replacement.as_deref().unwrap_or("").into()
            } else {
                text.slice(*from..*to).into()
            };
            let shown_len = shown.chars().count();
            highlights.push(len..len + shown_len);
            len += shown_len;
            new.push_str(&shown);
            pos = *to;
        }
        new.push_str(&String::from(text.slice(pos..end.max(pos))));

        split_lines(&new, &highlights, first_line, &mut lines);
        i = j;
    }
    lines.truncate(MAX_LINES);

    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let title = format!(
        "{} match{} on {} line{}",
        outcome.matches,
        if outcome.matches == 1 { "" } else { "es" },
        outcome.lines,
        plural(outcome.lines)
    );
    Some(Preview {
        doc: doc.id(),
        matches: changes
            .iter()
            .map(|(from, to, _)| *from..*to)
            .filter(|range| !range.is_empty())
            .collect(),
        lines,
        total_lines: outcome.lines,
        title,
        error: false,
    })
}

/// The end of `line`, before its line break.
fn line_end(text: &Rope, line: usize) -> usize {
    let line = line.min(text.len_lines() - 1);
    let end = text.line_to_char(line + 1);
    let slice = text.line(line);
    let ending = helix_core::line_ending::get_line_ending(&slice).map_or(0, |e| e.len_chars());
    end - ending
}

/// Split `text` (starting at document line `first_line`) into panel lines,
/// keeping the highlights.
fn split_lines(text: &str, highlights: &[Range<usize>], first_line: usize, out: &mut Vec<Line>) {
    let mut offset = 0;
    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let len = line.chars().count();
        let line_highlights = highlights
            .iter()
            .filter_map(|h| {
                let from = h.start.max(offset);
                let to = h.end.min(offset + len);
                (from < to).then(|| from - offset..to - offset)
            })
            .collect();
        out.push(Line {
            number: first_line + n,
            // Tabs as one space each, so char offsets stay valid.
            text: line.replace('\t', " "),
            highlights: line_highlights,
        });
        offset += raw.chars().count() + 1;
    }
}

fn highlight_style(theme: &Theme) -> Style {
    ["ui.highlight", "ui.cursor.match", "ui.selection.primary"]
        .iter()
        .find_map(|scope| theme.try_get_exact(scope))
        .unwrap_or_else(|| Style::default().add_modifier(Modifier::REVERSED))
}

/// The matches in `doc`, highlighted in the buffer.
pub fn overlay(doc: DocumentId, theme: &Theme) -> Option<OverlayHighlights> {
    let preview = PREVIEW.lock();
    let preview = preview.as_ref().filter(|preview| preview.doc == doc)?;
    if preview.matches.is_empty() {
        return None;
    }
    let highlight = ["ui.highlight", "ui.cursor.match", "ui.selection.primary"]
        .iter()
        .find_map(|scope| theme.find_highlight_exact(scope))?;
    Some(OverlayHighlights::Homogeneous {
        highlight,
        ranges: preview.matches.clone(),
    })
}

/// Draw the panel at the bottom of `area` (the editor area, without the
/// command line).
pub fn render(editor: &Editor, area: Rect, surface: &mut Surface) {
    let preview = PREVIEW.lock();
    let Some(preview) = preview.as_ref() else {
        return;
    };
    let theme = &editor.theme;
    let background = theme.get("ui.popup");
    let text_style = theme.get("ui.text");
    let number_style = theme.get("ui.linenr");
    let highlight = highlight_style(theme);
    let title_style = if preview.error {
        theme.get("error")
    } else {
        theme.get("ui.text.focus")
    };

    // Above the bottom view's statusline; at most a third of the screen.
    let max_height = (area.height / 3).max(3);
    let height = (preview.lines.len() as u16 + 2).clamp(3, max_height);
    if area.height < height + 2 || area.width < 10 {
        return;
    }
    let rect = Rect::new(area.x, area.bottom() - 1 - height, area.width, height);
    surface.clear_with(rect, background);

    let shown = (height - 2) as usize;
    let mut title = format!(" :s  {} ", preview.title);
    if !preview.error && preview.total_lines > shown.min(preview.lines.len()) {
        title.push_str(&format!(
            "(showing {} of {}) ",
            shown.min(preview.lines.len()),
            preview.total_lines
        ));
    }
    let block = Block::bordered()
        .title(tui::text::Span::styled(title, title_style))
        .border_style(theme.get("ui.popup.info").patch(background));
    let inner = block.inner(rect);
    block.render(rect, surface);

    let number_width = preview
        .lines
        .iter()
        .take(shown)
        .map(|line| (line.number + 1).to_string().len())
        .max()
        .unwrap_or(1);
    for (row, line) in preview.lines.iter().take(shown).enumerate() {
        let y = inner.y + row as u16;
        let right = inner.right();
        let (mut x, _) = surface.set_stringn(
            inner.x,
            y,
            &format!("{:>number_width$} │ ", line.number + 1),
            inner.width as usize,
            background.patch(number_style),
        );
        let segment = |surface: &mut Surface, text: &str, style: Style, x: &mut u16| {
            if *x < right && !text.is_empty() {
                let (nx, _) = surface.set_stringn(
                    *x,
                    y,
                    text,
                    (right - *x) as usize,
                    background.patch(style),
                );
                *x = nx;
            }
        };
        let chars: Vec<char> = line.text.chars().collect();
        let mut pos = 0;
        for h in &line.highlights {
            let plain: String = chars[pos..h.start].iter().collect();
            segment(surface, &plain, text_style, &mut x);
            let marked: String = chars[h.start..h.end].iter().collect();
            segment(surface, &marked, highlight, &mut x);
            pos = h.end;
        }
        let rest: String = chars[pos..].iter().collect();
        segment(surface, &rest, text_style, &mut x);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn splits_hunks_into_lines() {
        let mut out = Vec::new();
        let highlights = [Range { start: 1, end: 4 }];
        split_lines("ab\ncd", &highlights, 3, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].number, out[0].text.as_str()), (3, "ab"));
        assert_eq!(out[0].highlights, vec![1..2]);
        assert_eq!((out[1].number, out[1].text.as_str()), (4, "cd"));
        assert_eq!(out[1].highlights, vec![0..1]);
    }
}
