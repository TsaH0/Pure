//! Example Helix plugin: small text transformations on the selections.
//!
//! Commands (run with `:plugin-run text-tools/<command>`):
//! - `upper`, `lower`: change the case of the selected text
//! - `sort-lines`: sort the lines covered by the primary selection, or the
//!   whole document when the selection is just the cursor
//! - `count`: show the number of lines, words and chars of the selection, or
//!   of the whole document when the selection is just the cursor

use helix_plugin_api::{
    editor, register_plugin, text, CommandInfo, Edit, Plugin, Range, Registration,
};

struct TextTools;

/// In Helix a cursor is a selection of one char (or none, right after some
/// commands), so treat those as "nothing selected".
fn is_cursor(range: &Range) -> bool {
    range.anchor.abs_diff(range.head) <= 1
}

fn command(name: &str, doc: &str) -> CommandInfo {
    CommandInfo {
        name: name.into(),
        doc: doc.into(),
    }
}

/// The selections as `(from, to)`, with cursors extended to the char under
/// them like in Helix.
fn spans(ranges: &[Range], len: u32) -> Vec<(u32, u32)> {
    ranges
        .iter()
        .map(|range| {
            let (from, to) = (range.anchor.min(range.head), range.anchor.max(range.head));
            if from == to {
                (from, (to + 1).min(len))
            } else {
                (from, to)
            }
        })
        .collect()
}

fn change_case(upper: bool) -> Result<(), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let content = editor::text(doc)?;
    let ranges = editor::selections(doc)?;
    let edits: Vec<Edit> = spans(&ranges, text::char_len(&content))
        .into_iter()
        .map(|(from, to)| {
            let selected = text::slice(&content, from, to);
            let text = if upper {
                selected.to_uppercase()
            } else {
                selected.to_lowercase()
            };
            Edit { from, to, text }
        })
        .collect();
    editor::apply_edits(doc, &edits)?;
    // Keep the same text selected.
    editor::set_selections(doc, &ranges)
}

fn sort_lines() -> Result<(), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let content = editor::text(doc)?;
    let primary = editor::selections(doc)?[0];
    let starts = text::line_starts(&content);
    let len = text::char_len(&content);
    let (from, to) = if is_cursor(&primary) {
        (0, len)
    } else {
        let (a, b) = (
            primary.anchor.min(primary.head),
            primary.anchor.max(primary.head),
        );
        // Extend to whole lines.
        let from = *starts.iter().rev().find(|&&s| s <= a).unwrap_or(&0);
        let to = *starts.iter().find(|&&s| s >= b).unwrap_or(&len);
        (from, to)
    };
    let region = text::slice(&content, from, to);
    let trailing_newline = region.ends_with('\n');
    let mut lines: Vec<&str> = region.trim_end_matches('\n').split('\n').collect();
    lines.sort_unstable();
    let mut sorted = lines.join("\n");
    if trailing_newline {
        sorted.push('\n');
    }
    editor::apply_edits(
        doc,
        &[Edit {
            from,
            to,
            text: sorted,
        }],
    )?;
    editor::set_status(&format!("sorted {} lines", lines.len()));
    Ok(())
}

fn count() -> Result<(), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let content = editor::text(doc)?;
    let primary = editor::selections(doc)?[0];
    let (scope, region) = if is_cursor(&primary) {
        ("document", content.as_str())
    } else {
        let (a, b) = (
            primary.anchor.min(primary.head),
            primary.anchor.max(primary.head),
        );
        ("selection", text::slice(&content, a, b))
    };
    editor::set_status(&format!(
        "{scope}: {} lines, {} words, {} chars",
        region.lines().count(),
        region.split_whitespace().count(),
        text::char_len(region)
    ));
    Ok(())
}

impl Plugin for TextTools {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![
                command("upper", "Uppercase the selections"),
                command("lower", "Lowercase the selections"),
                command(
                    "sort-lines",
                    "Sort the selected lines (or the whole document)",
                ),
                command("count", "Count lines, words and chars"),
            ],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "upper" => change_case(true),
            "lower" => change_case(false),
            "sort-lines" => sort_lines(),
            "count" => count(),
            _ => Err(format!("unknown command {name}")),
        }
    }
}

register_plugin!(TextTools);
