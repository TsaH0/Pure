//! Example plugin: render Markdown in the buffer, like render-markdown.nvim,
//! with decorations (API 0.3).
//!
//! Headings get an icon per level and their colour, bullets become `•`,
//! checkboxes `☐` / `✔`, quotes `▋`, code fences a rule with the language,
//! `---` a full-width rule, and tables box-drawing borders. The line with the
//! cursor shows the raw text, so it can be edited as usual. Characters are
//! drawn *over* the markup (overlays), so the text itself never changes;
//! `markdown/toggle` (`space M`) turns rendering off and on.
//!
//! Settings in `plugins.toml`:
//!
//! ```toml
//! [markdown]
//! width = 80   # how far rules extend
//! ```
//!
//! Not possible in Pure (yet): hiding markup such as `**` or the link URL of
//! `[text](url)` (concealing), since Helix has no way to draw fewer
//! characters than there are.

use std::cell::Cell;

use helix_plugin_api::{
    decorations::{self, annotation, highlight, overlay, Annotation, Highlight},
    editor, register_plugin, text, CommandInfo, DocumentId, EventKind, Mode, Plugin, Registration,
};

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static WIDTH: Cell<u32> = const { Cell::new(80) };
}

const HEADINGS: [char; 6] = ['◉', '◎', '○', '◆', '◇', '▸'];
const BULLETS: [char; 4] = ['•', '◦', '▪', '▫'];

#[derive(Default)]
struct Rendered {
    highlights: Vec<Highlight>,
    overlays: Vec<Annotation>,
    annotations: Vec<Annotation>,
}

impl Rendered {
    fn over(&mut self, at: u32, text: impl Into<String>, scope: &str) {
        self.overlays.push(overlay(at, text, scope));
    }
}

fn is_markdown(doc: DocumentId) -> bool {
    matches!(editor::document_language(doc), Ok(Some(language)) if language == "markdown")
        || matches!(editor::document_path(doc), Ok(Some(path)) if path.ends_with(".md") || path.ends_with(".markdown"))
}

/// Render `source`, leaving line `raw` (the cursor's) as it is.
fn render(source: &str, raw: Option<usize>) -> Rendered {
    let mut out = Rendered::default();
    let width = WIDTH.get();
    let starts = text::line_starts(source);
    let mut in_code = false;
    for (number, line) in source.split('\n').enumerate() {
        let Some(&start) = starts.get(number) else {
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len() as u32;
        let indent = chars.iter().take_while(|c| **c == ' ' || **c == '\t').count();
        let rest: String = chars[indent..].iter().collect();
        let at = |i: usize| start + i as u32;

        // Code fences and what's between them.
        if rest.starts_with("```") || rest.starts_with("~~~") {
            if Some(number) != raw {
                let fence = rest.chars().take_while(|c| *c == '`' || *c == '~').count();
                out.over(at(indent), "─".repeat(fence), "comment");
                let language = rest[fence..].trim();
                if !in_code && !language.is_empty() {
                    out.highlights.push(highlight(at(indent + fence), start + len, "label"));
                }
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.highlights.push(highlight(start, start + len, "markup.raw.block"));
            continue;
        }
        if Some(number) == raw || chars.is_empty() {
            continue;
        }

        // Headings.
        let hashes = rest.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
            let scope = format!("markup.heading.{hashes}");
            let marker = format!("{}{}", HEADINGS[hashes - 1], " ".repeat(hashes - 1));
            out.over(at(indent), marker, &scope);
            out.highlights.push(highlight(at(indent + hashes), start + len, &scope));
            continue;
        }

        // Rules: three or more of the same of `-`, `*`, `_` (and spaces).
        let marks: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
        if marks.len() >= 3 && ["-", "*", "_"].iter().any(|m| marks.chars().all(|c| c.to_string() == *m)) {
            out.over(start, "─".repeat(chars.len()), "comment");
            if len < width {
                out.annotations
                    .push(annotation(start + len, "─".repeat((width - len) as usize), "comment"));
            }
            continue;
        }

        // Quotes.
        if rest.starts_with('>') {
            out.over(at(indent), "▋", "markup.quote");
            out.highlights.push(highlight(at(indent + 1), start + len, "markup.quote"));
            continue;
        }

        // Tables: rows starting with `|`.
        if rest.starts_with('|') {
            let separator = rest.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
            let last = chars.iter().rposition(|c| *c == '|').unwrap_or(indent);
            for (i, c) in chars.iter().enumerate().skip(indent) {
                let drawn = match (c, separator) {
                    ('|', false) => '│',
                    ('|', true) if i == indent => '├',
                    ('|', true) if i == last => '┤',
                    ('|', true) => '┼',
                    ('-' | ':', true) => '─',
                    _ => continue,
                };
                out.over(at(i), drawn.to_string(), "comment");
            }
            continue;
        }

        // Bullets, with checkboxes.
        if matches!(rest.chars().next(), Some('-' | '*' | '+')) && rest[1..].starts_with(' ') {
            let level = (indent / 2).min(BULLETS.len() - 1);
            let after = &rest[2..];
            let checkbox = [("[ ]", "☐", "markup.list.unchecked"), ("[x]", "✔", "markup.list.checked"), ("[X]", "✔", "markup.list.checked")]
                .into_iter()
                .find(|(mark, ..)| after.starts_with(mark));
            match checkbox {
                Some((_, icon, scope)) => {
                    // `- [ ] task` -> `  ☐  task`
                    out.over(at(indent), " ", scope);
                    out.over(at(indent + 2), format!("{icon}  "), scope);
                    if scope == "markup.list.checked" {
                        out.highlights.push(highlight(at(indent + 5), start + len, scope));
                    }
                }
                None => out.over(at(indent), BULLETS[level].to_string(), "markup.list"),
            }
        }
    }
    out
}

fn update(doc: DocumentId) -> Result<(), String> {
    if !is_markdown(doc) {
        return Ok(());
    }
    let rendered = if ENABLED.get() {
        let source = editor::text(doc)?;
        // The cursor's line stays raw (only in the window showing it).
        let raw = (editor::current_document() == Some(doc))
            .then(|| editor::selections(doc).ok())
            .flatten()
            .and_then(|ranges| ranges.first().map(|range| range.head))
            .map(|head| text::line_starts(&source).partition_point(|&start| start <= head) - 1);
        render(&source, raw)
    } else {
        Rendered::default()
    };
    decorations::set_highlights(doc, "text", &rendered.highlights)?;
    decorations::set_overlays(doc, "marks", &rendered.overlays)?;
    decorations::set_annotations(doc, "rules", &rendered.annotations)
}

fn report(result: Result<(), String>) {
    if let Err(err) = result {
        editor::log(&err);
    }
}

struct Markdown;

impl Plugin for Markdown {
    fn init(config: String) -> Result<Registration, String> {
        // `{"width": 100}`: a number after "width".
        if let Some(width) = config
            .split("\"width\"")
            .nth(1)
            .and_then(|rest| rest.trim_start_matches([':', ' ']).split([',', '}']).next())
            .and_then(|n| n.trim().parse().ok())
        {
            WIDTH.set(width);
        }
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "toggle".into(),
                doc: "Turn Markdown rendering off or on".into(),
            }],
            events: vec![
                EventKind::DocumentOpen,
                EventKind::DocumentChange,
                EventKind::SelectionChange,
                EventKind::ModeSwitch,
            ],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "toggle" => {
                ENABLED.set(!ENABLED.get());
                for doc in editor::documents() {
                    update(doc)?;
                }
                editor::set_status(if ENABLED.get() { "Markdown rendering on" } else { "Markdown rendering off" });
                Ok(())
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_open(doc: DocumentId) {
        report(update(doc));
    }
    fn on_document_change(doc: DocumentId) {
        report(update(doc));
    }
    fn on_selection_change(doc: DocumentId) {
        report(update(doc));
    }
    fn on_mode_switch(_old: Mode, _new: Mode) {
        if let Some(doc) = editor::current_document() {
            report(update(doc));
        }
    }
}

register_plugin!(Markdown);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn renders_markup() {
        let source = "# Title\n- item\n  - [x] done\n> quote\n---\n| a | b |\n|---|---|\n```rust\nlet x;\n```\n";
        let out = render(source, None);
        let overlays: Vec<_> = out.overlays.iter().map(|o| (o.position, o.text.as_str())).collect();
        assert!(overlays.contains(&(0, "◉")));
        assert!(overlays.contains(&(8, "•")));
        assert!(overlays.iter().any(|(_, t)| *t == "✔  "));
        assert!(overlays.iter().any(|(_, t)| *t == "▋"));
        assert!(overlays.iter().any(|(_, t)| *t == "├"));
        // The cursor's line stays raw.
        let raw = render(source, Some(0));
        assert!(!raw.overlays.iter().any(|o| o.position == 0));
    }
}
