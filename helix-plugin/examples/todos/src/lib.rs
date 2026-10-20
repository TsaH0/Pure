//! Example plugin: TODO, FIXME and NOTE comments, using every kind of
//! decoration (API 0.3).
//!
//! - highlights the words (`warning`, `error`, `info` scopes),
//! - a sign in the `plugins` gutter on their lines,
//! - virtual text at the end of the line (`◂ todo`),
//! - their count in the `plugins` statusline element,
//! - `todos/list`: a picker of them in the current document.
//!
//! The gutter and statusline are opt-in, in `config.toml`:
//!
//! ```toml
//! [editor]
//! gutters = ["diagnostics", "plugins", "line-numbers", "spacer", "diff"]
//! [editor.statusline]
//! right = ["plugins", "diagnostics", "selections", "position"]
//! ```

use helix_plugin_api::{
    decorations, editor, register_plugin, text, CommandInfo, DocumentId, EventKind, Item,
    OpenAction, Picker, Plugin, Registration,
};

const WORDS: [(&str, &str, char); 3] = [("FIXME", "error", '!'), ("TODO", "warning", '●'), ("NOTE", "info", '·')];

struct Todos;

/// `(char offset, word, scope, sign)` of each marker in `text`.
fn find(text: &str) -> Vec<(u32, &'static str, &'static str, char)> {
    let mut found = Vec::new();
    let mut chars = 0u32;
    let mut rest = text;
    while !rest.is_empty() {
        let hit = WORDS
            .iter()
            .filter_map(|&(word, scope, sign)| Some((rest.find(word)?, word, scope, sign)))
            .min_by_key(|(at, ..)| *at);
        let Some((at, word, scope, sign)) = hit else {
            break;
        };
        chars += text::char_len(&rest[..at]);
        // A whole word only.
        let before_ok = !rest[..at].chars().next_back().is_some_and(char::is_alphanumeric);
        let after_ok = !rest[at + word.len()..].chars().next().is_some_and(char::is_alphanumeric);
        if before_ok && after_ok {
            found.push((chars, word, scope, sign));
        }
        chars += word.len() as u32;
        rest = &rest[at + word.len()..];
    }
    found
}

fn decorate(doc: DocumentId) -> Result<(), String> {
    let source = editor::text(doc)?;
    let found = find(&source);
    let starts = text::line_starts(&source);
    let line_of = |pos: u32| (starts.partition_point(|&start| start <= pos) - 1) as u32;
    // The end of a line: the offset of its line break.
    let line_end = |line: u32| {
        starts
            .get(line as usize + 1)
            .map_or(text::char_len(&source), |next| next - 1)
    };
    let mut highlights = Vec::new();
    let mut signs = Vec::new();
    let mut notes = Vec::new();
    let mut last_line = None;
    for &(pos, word, scope, sign) in &found {
        highlights.push(decorations::highlight(pos, pos + word.len() as u32, scope));
        let line = line_of(pos);
        if last_line != Some(line) {
            signs.push(decorations::sign(line, sign.to_string(), scope));
            notes.push(decorations::annotation(
                line_end(line),
                format!("  ◂ {}", word.to_lowercase()),
                "comment",
            ));
            last_line = Some(line);
        }
    }
    decorations::set_highlights(doc, "words", &highlights)?;
    decorations::set_signs(doc, "lines", &signs)?;
    decorations::set_annotations(doc, "notes", &notes)?;
    if editor::current_document() == Some(doc) {
        let status = match found.len() {
            0 => String::new(),
            1 => "1 todo".into(),
            n => format!("{n} todos"),
        };
        decorations::set_statusline("count", &status)?;
    }
    Ok(())
}

fn list() -> Result<(), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let source = editor::text(doc)?;
    let path = editor::document_path(doc)?;
    let lines: Vec<&str> = source.lines().collect();
    let starts = text::line_starts(&source);
    let items: Vec<Item> = find(&source)
        .into_iter()
        .map(|(pos, word, _, _)| {
            let line = starts.partition_point(|&start| start <= pos) - 1;
            let shown = lines.get(line).map_or("", |text| text.trim());
            let item = Item::new(vec![format!("{}", line + 1), word.into(), shown.into()], line.to_string());
            match &path {
                Some(path) => item.with_file_preview(path.clone(), Some((line as u32, line as u32))),
                None => item.with_text_preview(shown),
            }
        })
        .collect();
    if items.is_empty() {
        editor::set_status("No TODO, FIXME or NOTE here");
        return Ok(());
    }
    let mut picker = Picker::new("Todos", items);
    picker.columns = vec!["Line".into(), "Kind".into(), "Text".into()];
    editor::pick(picker, move |choice| {
        let (Some(item), Some(action)) = (choice.items.first(), choice.action) else {
            return Ok(());
        };
        let line: u32 = item.data.parse().map_err(|_| "bad item")?;
        match &path {
            Some(path) => editor::open_file_at(path, action, Some(line + 1), None),
            // A scratch buffer: just move there.
            None if action == OpenAction::Replace => {
                let start = text::line_starts(&editor::text(doc)?)[line as usize];
                editor::set_selections(doc, &[helix_plugin_api::Range { anchor: start, head: start }])
            }
            None => Ok(()),
        }
    })?;
    Ok(())
}

impl Plugin for Todos {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "list".into(),
                doc: "Pick a TODO, FIXME or NOTE in this document".into(),
            }],
            events: vec![EventKind::DocumentOpen, EventKind::DocumentChange],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "list" => list(),
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_open(doc: DocumentId) {
        if let Err(err) = decorate(doc) {
            editor::log(&err);
        }
    }

    fn on_document_change(doc: DocumentId) {
        if let Err(err) = decorate(doc) {
            editor::log(&err);
        }
    }
}

register_plugin!(Todos);
