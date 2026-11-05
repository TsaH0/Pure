//! Drawing plugins' bars (`helix_view::bars`). `%{name}` in their text is
//! filled in here, when drawn, so a statusline showing the position needs
//! no plugin call on every cursor move:
//!
//! `mode`, `file` (relative path), `filename`, `modified` ("[+]" or
//! nothing), `line`, `column`, `lines`, `percent`, `language`, `branch`,
//! `selections`, `errors`, `warnings`, `buffers`; `%%` is a `%`. The scope
//! "mode" is the current mode's statusline colours.

use std::borrow::Cow;

use helix_core::{coords_at_pos, diagnostic::Severity, unicode::width::UnicodeWidthStr};
use helix_view::bars::{Bar, Span};
use helix_view::document::Mode;
use helix_view::graphics::Rect;
use helix_view::theme::Style;
use helix_view::{Document, Editor, View};
use tui::buffer::Buffer as Surface;

/// The value of `%{name}`; `None` for a name it doesn't know.
fn variable(editor: &Editor, at: Option<(&Document, &View)>, name: &str) -> Option<String> {
    if name == "buffers" {
        return Some(editor.documents.len().to_string());
    }
    if name == "mode" {
        let modes = &editor.config().statusline.mode;
        return Some(match editor.mode() {
            Mode::Normal => modes.normal.clone(),
            Mode::Insert => modes.insert.clone(),
            Mode::Select => modes.select.clone(),
        });
    }
    let Some((doc, view)) = at else {
        return Some(String::new());
    };
    let text = doc.text().slice(..);
    let cursor = || {
        let pos = doc.selection(view.id).primary().cursor(text);
        coords_at_pos(text, pos)
    };
    let count = |severity: Severity| {
        doc.diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Some(severity))
            .count()
            .to_string()
    };
    Some(match name {
        "file" => doc
            .relative_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| doc.display_name().into_owned()),
        "filename" => doc
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| doc.display_name().into_owned()),
        "modified" => if doc.is_modified() { "[+]" } else { "" }.into(),
        "line" => (cursor().row + 1).to_string(),
        "column" => (cursor().col + 1).to_string(),
        "lines" => text.len_lines().to_string(),
        "percent" => {
            let lines = text.len_lines().max(1);
            format!("{}%", (cursor().row + 1) * 100 / lines)
        }
        "language" => doc.language_name().unwrap_or("text").into(),
        "branch" => doc
            .version_control_head()
            .map(|head| head.to_string())
            .unwrap_or_default(),
        "selections" => doc.selection(view.id).len().to_string(),
        "errors" => count(Severity::Error),
        "warnings" => count(Severity::Warning),
        _ => return None,
    })
}

/// `text` with its `%{name}`s filled in.
pub fn expand<'a>(editor: &Editor, at: Option<(&Document, &View)>, text: &'a str) -> Cow<'a, str> {
    if !text.contains('%') {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(after) = after.strip_prefix('%') {
            out.push('%');
            rest = after;
            continue;
        }
        let name = after
            .strip_prefix('{')
            .and_then(|after| after.split_once('}'));
        match name.and_then(|(name, tail)| Some((variable(editor, at, name)?, tail))) {
            Some((value, tail)) => {
                out.push_str(&value);
                rest = tail;
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.into()
}

/// The width of `spans` once filled in, and the spans.
fn fill<'a>(
    editor: &Editor,
    at: Option<(&Document, &View)>,
    spans: &'a [Span],
) -> (usize, Vec<(Cow<'a, str>, &'a str)>) {
    let spans: Vec<_> = spans
        .iter()
        .map(|span| {
            let text: Cow<str> = match expand(editor, at, &span.text) {
                // No control characters on the screen.
                Cow::Borrowed(text) if !text.chars().any(char::is_control) => text.into(),
                text => text
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect::<String>()
                    .into(),
            };
            (text, span.scope.as_str())
        })
        .collect();
    let width = spans.iter().map(|(text, _)| text.width()).sum();
    (width, spans)
}

/// Draw `bar` on the row `area`, over `base`.
pub fn render(
    editor: &Editor,
    at: Option<(&Document, &View)>,
    bar: &Bar,
    area: Rect,
    surface: &mut Surface,
    base: Style,
) {
    let area = area.with_height(1);
    surface.set_style(area, base);
    let theme = &editor.theme;
    let mut draw = |x: u16, spans: &[(Cow<str>, &str)]| {
        let mut x = x;
        for (text, scope) in spans {
            let left = area.right().saturating_sub(x) as usize;
            if left == 0 {
                break;
            }
            let style = match *scope {
                "" => base,
                // The current mode's statusline colours.
                "mode" => base.patch(theme.get(match editor.mode() {
                    Mode::Normal => "ui.statusline.normal",
                    Mode::Insert => "ui.statusline.insert",
                    Mode::Select => "ui.statusline.select",
                })),
                scope => base.patch(theme.get(scope)),
            };
            (x, _) = surface.set_stringn(x, area.y, text, left, style);
        }
    };
    let (left_width, left) = fill(editor, at, &bar.left);
    let (right_width, right) = fill(editor, at, &bar.right);
    let (center_width, center) = fill(editor, at, &bar.center);
    draw(area.x, &left);
    let width = area.width as usize;
    if !center.is_empty() {
        let x = width.saturating_sub(center_width) / 2;
        // Never over the left part.
        draw(area.x + x.max(left_width).min(width) as u16, &center);
    }
    if !right.is_empty() {
        let x = width.saturating_sub(right_width).max(left_width);
        draw(area.x + x.min(width) as u16, &right);
    }
}
