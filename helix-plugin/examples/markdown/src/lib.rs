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
//! code-background = "ui.cursorline.primary,ui.virtual.ruler"  # first defined
//! ```
//!
//! Code blocks get a background across the whole window width.
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
    /// The theme scope whose background code blocks get.
    static CODE_BACKGROUND: std::cell::RefCell<String> =
        std::cell::RefCell::new("ui.cursorline.primary,ui.virtual.ruler".into());
}

const HEADINGS: [char; 6] = ['◉', '◎', '○', '◆', '◇', '▸'];
const BULLETS: [char; 4] = ['•', '◦', '▪', '▫'];

#[derive(Default, Clone)]
struct Rendered {
    highlights: Vec<Highlight>,
    overlays: Vec<Annotation>,
    annotations: Vec<Annotation>,
    /// Whole-line backgrounds (code blocks).
    blocks: Vec<decorations::LineBackground>,
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

/// Render `source` (the tests compare with it).
#[cfg(test)]
fn render(source: &str) -> Rendered {
    let mut out = Rendered::default();
    let starts = text::line_starts(source);
    let mut in_code = false;
    for (number, line) in source.split('\n').enumerate() {
        let Some(&start) = starts.get(number) else {
            break;
        };
        render_line(&mut out, number, line, start, &mut in_code);
    }
    out
}

/// The last rendering of a document, to render only what an edit changed.
struct Cache {
    source: String,
    /// Char offset of each line's start.
    starts: Vec<u32>,
    /// Whether line `i` starts inside a code fence (one more than lines).
    in_code: Vec<bool>,
    out: Rendered,
}

thread_local! {
    static CACHE: std::cell::RefCell<std::collections::HashMap<DocumentId, Cache>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Render `source` from scratch, keeping what an incremental update needs.
fn render_full(source: String) -> Cache {
    let mut out = Rendered::default();
    let starts = text::line_starts(&source);
    let mut in_code = vec![false];
    let mut state = false;
    for (number, line) in source.split('\n').enumerate() {
        let Some(&start) = starts.get(number) else {
            break;
        };
        render_line(&mut out, number, line, start, &mut state);
        in_code.push(state);
    }
    Cache {
        source,
        starts,
        in_code,
        out,
    }
}

/// Render `source` given the rendering of an earlier version: re-render
/// from the first changed line until the code-fence state agrees with the
/// old rendering again (a fence can change everything after it), and reuse
/// the rest, moved by the edit.
fn render_update(old: Cache, source: String) -> Cache {
    if old.source == source {
        return old;
    }
    let old_lines: Vec<&str> = old.source.split('\n').collect();
    let new_lines: Vec<&str> = source.split('\n').collect();
    let shortest = old_lines.len().min(new_lines.len());
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count()
        .min(shortest);
    let suffix = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(shortest - prefix);
    let starts = text::line_starts(&source);
    let old_start = |line: usize| old.starts.get(line).copied().unwrap_or(u32::MAX);
    let cut = old_start(prefix);

    // What comes before the change stays as it was.
    let mut out = Rendered {
        highlights: old.out.highlights.iter().filter(|h| h.from < cut).cloned().collect(),
        overlays: old.out.overlays.iter().filter(|o| o.position < cut).cloned().collect(),
        annotations: old.out.annotations.iter().filter(|a| a.position < cut).cloned().collect(),
        blocks: old.out.blocks.iter().filter(|b| (b.line as usize) < prefix).cloned().collect(),
    };
    let mut in_code = old.in_code[..=prefix].to_vec();
    let mut state = in_code[prefix];
    let changed_end = new_lines.len() - suffix;
    let mut line = prefix;
    while line < new_lines.len() {
        render_line(&mut out, line, new_lines[line], starts[line], &mut state);
        in_code.push(state);
        line += 1;
        // Past the change, the old rendering is right again once the fence
        // state agrees.
        if line < changed_end || line >= new_lines.len() {
            continue;
        }
        let old_line = line + old_lines.len() - new_lines.len();
        if old.in_code[old_line] == state {
            let from = old_start(old_line);
            let shift = |pos: u32| (pos as i64 + starts[line] as i64 - from as i64) as u32;
            let lines_shift = line as i64 - old_line as i64;
            for h in old.out.highlights.iter().filter(|h| h.from >= from) {
                out.highlights.push(highlight(shift(h.from), shift(h.to), &h.scope));
            }
            for o in old.out.overlays.iter().filter(|o| o.position >= from) {
                out.overlays.push(overlay(shift(o.position), o.text.clone(), &o.scope));
            }
            for a in old.out.annotations.iter().filter(|a| a.position >= from) {
                out.annotations
                    .push(annotation(shift(a.position), a.text.clone(), &a.scope));
            }
            for b in old.out.blocks.iter().filter(|b| b.line as usize >= old_line) {
                out.blocks.push(decorations::line_background(
                    (b.line as i64 + lines_shift) as u32,
                    &b.scope,
                ));
            }
            in_code.extend_from_slice(&old.in_code[old_line + 1..]);
            break;
        }
    }
    Cache {
        source,
        starts,
        in_code,
        out,
    }
}

/// Render line `number` (`line`, starting at char `start`) into `out`.
/// `in_code`: inside a code fence before it, and after it once rendered.
fn render_line(out: &mut Rendered, number: usize, line: &str, start: u32, in_code: &mut bool) {
    let width = WIDTH.get();
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
        if !*in_code && !language.is_empty() {
            out.highlights.push(highlight(at(indent + fence), start + len, "label"));
        }
        *in_code = !*in_code;
        out.blocks.push(line_background(number as u32));
        return;
    }
    if *in_code {
        out.blocks.push(line_background(number as u32));
        out.highlights.push(highlight(start, start + len, "markup.raw.block"));
        return;
    }
    if line.is_empty() {
        return;
    }

    // Headings.
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        let scope = format!("markup.heading.{hashes}");
        let marker = format!("{}{}", HEADINGS[hashes - 1], " ".repeat(hashes - 1));
        out.over(at(indent), marker, &scope);
        out.highlights.push(highlight(at(indent + hashes), start + len, &scope));
        return;
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
        return;
    }

    // Quotes.
    if rest.starts_with('>') {
        out.over(at(indent), "▋", "markup.quote");
        out.highlights.push(highlight(at(indent + 1), start + len, "markup.quote"));
        return;
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
        return;
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

fn line_background(line: u32) -> decorations::LineBackground {
    CODE_BACKGROUND.with_borrow(|scope| decorations::line_background(line, scope))
}

fn update(doc: DocumentId) -> Result<(), String> {
    if !is_markdown(doc) {
        return Ok(());
    }
    let rendered = if ENABLED.get() {
        let source = editor::text(doc)?;
        let cache = match CACHE.with_borrow_mut(|cache| cache.remove(&doc)) {
            Some(old) => render_update(old, source),
            None => render_full(source),
        };
        let out = cache.out.clone();
        CACHE.with_borrow_mut(|caches| caches.insert(doc, cache));
        out
    } else {
        CACHE.with_borrow_mut(|cache| cache.remove(&doc));
        Rendered::default()
    };
    // The cursor's line stays raw; the editor follows the cursor.
    for layer in ["text", "marks", "rules"] {
        decorations::set_raw_on_cursor_line(doc, layer, true)?;
    }
    decorations::set_highlights(doc, "text", &rendered.highlights)?;
    decorations::set_overlays(doc, "marks", &rendered.overlays)?;
    decorations::set_line_backgrounds(doc, "blocks", &rendered.blocks)?;
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
        if let Some(scope) = config
            .split("\"code-background\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
        {
            CODE_BACKGROUND.set(scope.to_string());
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
        // The code block, fences included, gets a line background.
        let blocks: Vec<_> = out.blocks.iter().map(|b| b.line).collect();
        assert_eq!(blocks, [7, 8, 9]);
    }
}


#[cfg(test)]
mod incremental {
    use super::*;

    /// Everything a rendering produces, comparable.
    fn flat(out: &Rendered) -> Vec<String> {
        let mut all = Vec::new();
        all.extend(out.highlights.iter().map(|h| format!("h{} {} {}", h.from, h.to, h.scope)));
        all.extend(out.overlays.iter().map(|o| format!("o{} {} {}", o.position, o.text, o.scope)));
        all.extend(out.annotations.iter().map(|a| format!("a{} {} {}", a.position, a.text, a.scope)));
        all.extend(out.blocks.iter().map(|b| format!("b{} {}", b.line, b.scope)));
        all.sort();
        all
    }

    #[test]
    fn incremental_updates_match_full_renders() {
        let pieces = [
            "# Title\n", "- item\n", "  - [x] done\n", "> quote\n", "---\n", "| a | b |\n",
            "|---|---|\n", "```rust\n", "let x;\n", "```\n", "text\n", "\n", "~~~\n", "é ✓\n",
        ];
        let mut seed: u64 = 42;
        let mut next = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n.max(1)
        };
        for _ in 0..300 {
            let mut lines: Vec<&str> = (0..next(30)).map(|_| pieces[next(pieces.len())]).collect();
            let old_source: String = lines.concat();
            let cache = render_full(old_source.clone());
            // Insert, delete or replace a few lines.
            for _ in 0..1 + next(3) {
                match next(3) {
                    0 => lines.insert(next(lines.len() + 1), pieces[next(pieces.len())]),
                    1 if !lines.is_empty() => {
                        lines.remove(next(lines.len()));
                    }
                    _ if !lines.is_empty() => {
                        let at = next(lines.len());
                        lines[at] = pieces[next(pieces.len())];
                    }
                    _ => {}
                }
            }
            let source: String = lines.concat();
            let updated = render_update(cache, source.clone());
            let full = render_full(source.clone());
            assert_eq!(flat(&updated.out), flat(&full.out), "{old_source:?} -> {source:?}");
            assert_eq!(updated.in_code, full.in_code, "{source:?}");
        }
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
