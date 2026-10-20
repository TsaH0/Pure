//! Example plugin: a linter (API 0.3 diagnostics). Its findings show like a
//! language server's: in the gutter, inline, with `]d`/`[d` and in the
//! diagnostics pickers (`space d`).
//!
//! Checks trailing whitespace, lines longer than `max-line`, mixed tabs and
//! spaces in indentation, and leftover merge-conflict markers.
//!
//! ```toml
//! [lint]
//! max-line = 100   # 0 turns the check off
//! ```

use std::cell::Cell;

use helix_plugin_api::{
    diagnostics::{self, diagnostic, Diagnostic, Severity},
    editor, register_plugin, text, CommandInfo, DocumentId, EventKind, Plugin, Registration,
};

thread_local! {
    static MAX_LINE: Cell<u32> = const { Cell::new(100) };
}

fn check(source: &str, max_line: u32) -> Vec<Diagnostic> {
    let starts = text::line_starts(source);
    let mut found = Vec::new();
    for (number, line) in source.split('\n').enumerate() {
        let Some(&start) = starts.get(number) else {
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let len = text::char_len(line);
        let trimmed = line.trim_end_matches([' ', '\t']);
        if trimmed.len() < line.len() {
            let from = start + text::char_len(trimmed);
            let mut d = diagnostic(from, start + len, Severity::Hint, "trailing whitespace");
            d.code = Some("trailing-space".into());
            found.push(d);
        }
        if max_line > 0 && len > max_line {
            let mut d = diagnostic(
                start + max_line,
                start + len,
                Severity::Info,
                format!("line is {len} characters long (max {max_line})"),
            );
            d.code = Some("line-length".into());
            found.push(d);
        }
        let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        if indent.contains(' ') && indent.contains('\t') {
            let mut d = diagnostic(
                start,
                start + indent.len() as u32,
                Severity::Warning,
                "indentation mixes tabs and spaces",
            );
            d.code = Some("mixed-indent".into());
            found.push(d);
        }
        if ["<<<<<<< ", "=======", ">>>>>>> "]
            .iter()
            .any(|marker| line.starts_with(marker) && (marker.ends_with(' ') || line == *marker))
        {
            let mut d = diagnostic(start, start + len, Severity::Error, "merge conflict marker");
            d.code = Some("conflict".into());
            found.push(d);
        }
    }
    found
}

fn lint(doc: DocumentId) -> Result<(), String> {
    let source = editor::text(doc)?;
    diagnostics::set_diagnostics(doc, &check(&source, MAX_LINE.get()))
}

fn report(result: Result<(), String>) {
    if let Err(err) = result {
        editor::log(&err);
    }
}

struct Lint;

impl Plugin for Lint {
    fn init(config: String) -> Result<Registration, String> {
        if let Some(max) = config
            .split("\"max-line\":")
            .nth(1)
            .and_then(|rest| rest.split([',', '}']).next())
            .and_then(|n| n.trim().parse().ok())
        {
            MAX_LINE.set(max);
        }
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "clear".into(),
                doc: "Remove this document's lint diagnostics until it changes".into(),
            }],
            events: vec![EventKind::DocumentOpen, EventKind::DocumentChange],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "clear" => {
                let doc = editor::current_document().ok_or("no document")?;
                diagnostics::set_diagnostics(doc, &[])
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_open(doc: DocumentId) {
        report(lint(doc));
    }
    fn on_document_change(doc: DocumentId) {
        report(lint(doc));
    }
}

register_plugin!(Lint);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn finds_problems() {
        let found = check("ok\nspace  \n\t  mixed\n<<<<<<< HEAD\n0123456789\n", 8);
        let codes: Vec<_> = found.iter().map(|d| (d.code.clone().unwrap(), d.from, d.to)).collect();
        assert_eq!(
            codes,
            [
                ("trailing-space".to_string(), 8, 10),
                ("mixed-indent".into(), 11, 14),
                ("line-length".into(), 20 + 8, 32),
                ("conflict".into(), 20, 32),
                ("line-length".into(), 33 + 8, 43),
            ]
        );
    }
}
