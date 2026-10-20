//! Example plugin: render Markdown in the buffer, like render-markdown.nvim,
//! with decorations (API 0.3).
//!
//! Headings get an icon per level and their colour, bullets become `•`,
//! checkboxes `☐` / `✔`, quotes `▋`, code fences a rule with the language,
//! `---` a full-width rule, and tables box-drawing borders. The line with the
//! cursor shows the raw text, so it can be edited as usual: the editor does
//! that itself (`set-raw-on-cursor-line`), so moving the cursor costs the
//! plugin nothing and it only re-renders on edits. Characters are
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
    editor, register_plugin, text, CommandInfo, DocumentId, EventKind, Plugin, Registration,
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

/// Render `source`.
fn render(source: &str) -> Rendered {
    let mut out = Rendered::default();
    let width = WIDTH.get();
    let starts = text::line_starts(source);
    let mut in_code = false;
    for (number, line) in source.split('\n').enumerate() {
        let Some(&start) = starts.get(number) else {
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let len = text::char_len(line);
        // Spaces and tabs are one byte: byte and char offsets agree here.
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let rest = &line[indent..];
        let at = |i: usize| start + i as u32;

        // Code fences and what's between them.
        if rest.starts_with("```") || rest.starts_with("~~~") {
            let fence = rest.chars().take_while(|c| *c == '`' || *c == '~').count();
            out.over(at(indent), "─".repeat(fence), "comment");
            let language = rest[fence..].trim();
            if !in_code && !language.is_empty() {
                out.highlights.push(highlight(at(indent + fence), start + len, "label"));
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.highlights.push(highlight(start, start + len, "markup.raw.block"));
            continue;
        }
        if line.is_empty() {
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
        let first = rest.chars().next().unwrap_or(' ');
        let is_rule = matches!(first, '-' | '*' | '_')
            && rest.chars().all(|c| c == first || c == ' ' || c == '\t')
            && rest.chars().filter(|c| *c == first).count() >= 3;
        if is_rule {
            out.over(start, "─".repeat(len as usize), "comment");
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
            if rest.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                // The separator row, drawn in one go: `├──┼──┤`.
                let last = rest.char_indices().filter(|(_, c)| *c == '|').count();
                let mut pipes = 0;
                let row: String = rest
                    .chars()
                    .map(|c| match c {
                        '|' => {
                            pipes += 1;
                            match pipes {
                                1 => '├',
                                n if n == last => '┤',
                                _ => '┼',
                            }
                        }
                        '-' | ':' => '─',
                        c => c,
                    })
                    .collect();
                out.over(at(indent), row, "comment");
            } else {
                // Cells may hold any text: count chars, not bytes.
                for (i, c) in rest.chars().enumerate() {
                    if c == '|' {
                        out.over(at(indent + i), "│", "comment");
                    }
                }
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
        render(&editor::text(doc)?)
    } else {
        Rendered::default()
    };
    // The cursor's line stays raw; the editor follows the cursor.
    for layer in ["text", "marks", "rules"] {
        decorations::set_raw_on_cursor_line(doc, layer, true)?;
    }
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
}

register_plugin!(Markdown);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn renders_markup() {
        let source = "# Title\n- item\n  - [x] done\n> quote\n---\n| a | b |\n|---|---|\n```rust\nlet x;\n```\n";
        let out = render(source);
        let overlays: Vec<_> = out.overlays.iter().map(|o| (o.position, o.text.as_str())).collect();
        assert!(overlays.contains(&(0, "◉")));
        assert!(overlays.contains(&(8, "•")));
        assert!(overlays.iter().any(|(_, t)| *t == "✔  "));
        assert!(overlays.iter().any(|(_, t)| *t == "▋"));
        assert!(overlays.iter().any(|(_, t)| *t == "├───┼───┤"));
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn render_time() {
        let book: String = std::fs::read_dir("../../../book/src")
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
            .collect::<String>()
            .repeat(2);
        let started = std::time::Instant::now();
        let out = super::render(&book);
        eprintln!("native render: {:?} ({} overlays)", started.elapsed(), out.overlays.len());
    }
}
