//! A quickfix list like Vim's: a list of locations (compiler errors, grep
//! matches, diagnostics) to step through.
//!
//! - Filled by `:compile` / `space x` (from the compiler's `file:line:col`
//!   output), `:grep`, `:cdiagnostics` and `:cbuffer`.
//! - `]q` / `[q` (`:cnext` / `:cprev`) go to the next / previous entry,
//!   `]Q` / `[Q` (`:clast` / `:cfirst`) to the last / first one, `:cc N` to
//!   the Nth; `space q` (`:copen`) shows the list in a picker.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use helix_core::regex::Regex;
use helix_core::{pos_at_coords, Position, Selection};
use helix_event::runtime_local;
use helix_view::{align_view, editor::Action, Align, Editor};
use parking_lot::Mutex;

use crate::ui::{self, Picker, PickerColumn};

/// One location. `line` and `column` are 0-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    /// `column` counts bytes (ripgrep), not characters.
    pub byte_column: bool,
    pub text: String,
}

/// Most entries one list keeps.
pub const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Default)]
pub struct List {
    pub title: String,
    pub entries: Vec<Entry>,
    /// The entry last jumped to.
    pub current: Option<usize>,
}

runtime_local! {
    static LIST: Mutex<List> = Mutex::new(List {
        title: String::new(),
        entries: Vec::new(),
        current: None,
    });
}

/// Replace the quickfix list.
pub fn set(title: impl Into<String>, entries: Vec<Entry>) {
    *LIST.lock() = List {
        title: title.into(),
        entries,
        current: None,
    };
}

/// How many entries the list has.
pub fn list_len() -> usize {
    LIST.lock().entries.len()
}

/// Run `f` with the quickfix list.
pub fn with<R>(f: impl FnOnce(&List) -> R) -> R {
    f(&LIST.lock())
}

/// `file:line:col: message`, `file:line: message`, `file:line:col` (rustc's
/// `--> file:line:col`) and grep's `file:line:col:text`.
static LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:-->\s*|::: )?(?P<path>(?:[A-Za-z]:[\\/])?[^\s:][^:]*?):(?P<line>\d+)(?::(?P<col>\d+))?(?::\s?(?P<text>.*))?$",
    )
    .unwrap()
});

/// Find the locations in compiler / grep output. Only paths of existing files
/// count (relative ones are relative to `cwd`), which filters out things that
/// merely look like locations, like times. A location without a message
/// (rustc's `--> file:line:col`) takes the message from the line before.
pub fn parse(output: &str, cwd: &Path) -> Vec<Entry> {
    parse_with(output, cwd, false)
}

/// Like [`parse`]; `byte_columns` if the producer counts columns in bytes.
/// At most [`MAX_ENTRIES`].
pub fn parse_with(output: &str, cwd: &Path, byte_columns: bool) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut previous = "";
    // Whether each path is a file, asked once per path.
    let mut files = std::collections::HashMap::new();
    for line in output.lines() {
        if entries.len() == MAX_ENTRIES {
            break;
        }
        if let Some(mut entry) = parse_line(line, cwd, previous, &mut files) {
            entry.byte_column = byte_columns;
            if entries.last() != Some(&entry) {
                entries.push(entry);
            }
        }
        if !line.trim().is_empty() {
            previous = line;
        }
    }
    entries
}

fn parse_line(
    line: &str,
    cwd: &Path,
    previous: &str,
    files: &mut std::collections::HashMap<PathBuf, bool>,
) -> Option<Entry> {
    let captures = LOCATION.captures(line)?;
    let path = PathBuf::from(captures.name("path")?.as_str().trim());
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    if !*files.entry(path.clone()).or_insert_with(|| path.is_file()) {
        return None;
    }
    let line_number: usize = captures.name("line")?.as_str().parse().ok()?;
    let column: usize = captures
        .name("col")
        .and_then(|col| col.as_str().parse().ok())
        .unwrap_or(1);
    let text = captures
        .name("text")
        .map(|text| text.as_str().trim())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| previous.trim());
    Some(Entry {
        path: helix_stdx::path::normalize(path),
        line: line_number.saturating_sub(1),
        column: column.saturating_sub(1),
        byte_column: false,
        text: text.to_string(),
    })
}

/// Where to go in the list.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Next(usize),
    Previous(usize),
    First,
    Last,
    /// 0-based.
    Nth(usize),
    /// The current entry again (or the first).
    Current,
}

/// Jump to an entry of the list.
pub fn go(editor: &mut Editor, target: Target) {
    let (entry, index, len) = {
        let mut list = LIST.lock();
        let len = list.entries.len();
        if len == 0 {
            editor.set_error("The quickfix list is empty");
            return;
        }
        let index = match (target, list.current) {
            (Target::Next(n), Some(current)) => current.checked_add(n).filter(|&i| i < len),
            (Target::Next(n), None) => Some(n - 1).filter(|&i| i < len),
            (Target::Previous(n), Some(current)) => current.checked_sub(n),
            (Target::Previous(_), None) => None,
            (Target::First, _) => Some(0),
            (Target::Last, _) => Some(len - 1),
            (Target::Nth(n), _) => Some(n).filter(|&i| i < len),
            (Target::Current, current) => Some(current.unwrap_or(0)),
        };
        let Some(index) = index else {
            editor.set_error(match target {
                Target::Nth(n) => format!("No entry {} (the list has {len})", n + 1),
                Target::Previous(_) => "No previous entry in the quickfix list".to_string(),
                _ => "No more entries in the quickfix list".to_string(),
            });
            return;
        };
        list.current = Some(index);
        (list.entries[index].clone(), index, len)
    };
    jump(editor, &entry, Action::Replace);
    editor.set_status(format!("({} of {len}) {}", index + 1, entry.text));
}

/// Open `entry`'s file and put the cursor on its location, remembering where
/// we came from in the jumplist.
fn jump(editor: &mut Editor, entry: &Entry, action: Action) {
    {
        let (view, doc) = current!(editor);
        super::push_jump(view, doc);
    }
    if let Err(err) = editor.open(&entry.path, action) {
        editor.set_error(format!("Failed to open {}: {err}", entry.path.display()));
        return;
    }
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let column = if entry.byte_column && entry.line < text.len_lines() {
        // The char at that byte of the line.
        let line = text.line(entry.line);
        line.byte_to_char(entry.column.min(line.len_bytes()))
    } else {
        entry.column
    };
    let pos = pos_at_coords(text, Position::new(entry.line, column), true);
    doc.set_selection(view.id, Selection::point(pos));
    align_view(doc, view, Align::Center);
}

/// A row of the quickfix picker.
pub(super) struct Item {
    index: usize,
    entry: Entry,
    /// The path relative to the working directory, for display.
    display_path: PathBuf,
}

type QuickfixPicker = Picker<Item, super::PathStyleConfig>;

/// A picker showing the list. `None` if the list is empty.
pub(super) fn picker(editor: &Editor) -> Option<QuickfixPicker> {
    let (items, current) = with(|list| {
        let items: Vec<Item> = list
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| Item {
                index,
                entry: entry.clone(),
                display_path: helix_stdx::path::get_relative_path(&entry.path).into_owned(),
            })
            .collect();
        (items, list.current)
    });
    if items.is_empty() {
        return None;
    }
    let columns = [
        PickerColumn::new(
            "location",
            |item: &Item, config: &super::PathStyleConfig| {
                config.stylize(Some(&item.display_path), Some(item.entry.line))
            },
        ),
        PickerColumn::new("text", |item: &Item, _| item.entry.text.as_str().into()),
    ];
    let picker = Picker::new(
        columns,
        1,
        items,
        super::PathStyleConfig::new(&editor.theme),
        |cx, item: &Item, action| {
            LIST.lock().current = Some(item.index);
            jump(cx.editor, &item.entry, action);
            let len = with(|list| list.entries.len());
            cx.editor
                .set_status(format!("({} of {len}) {}", item.index + 1, item.entry.text));
        },
    )
    .with_initial_cursor(current.unwrap_or(0) as u32)
    .with_preview(|_editor, item: &Item| {
        Some((
            item.entry.path.as_path().into(),
            Some((item.entry.line, item.entry.line)),
        ))
    });
    Some(picker)
}

/// Show the list in a picker.
pub(super) fn open(editor: &mut Editor) -> Option<ui::overlay::Overlay<QuickfixPicker>> {
    match picker(editor) {
        Some(picker) => {
            let (title, len) = with(|list| (list.title.clone(), list.entries.len()));
            editor.set_status(format!("Quickfix: {title} ({len} entries)"));
            Some(ui::overlay::overlaid(picker))
        }
        None => {
            editor.set_error("The quickfix list is empty");
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("main.cpp"), "int main() {}\n").unwrap();
        dir
    }

    #[test]
    fn parses_compiler_and_grep_output() {
        let dir = dir();
        let output = "\
   Compiling demo v0.1.0
error[E0425]: cannot find value `x` in this scope
 --> src/main.rs:2:5
  |
main.cpp:3:10: error: expected ';' before '}' token
main.cpp:7: warning: something
src/main.rs:1:4:fn main() {}
Compilation finished at Sun Oct 18 00:55:06, duration 0.21 s
nope.rs:1:1: does not exist
";
        let entries = parse(output, dir.path());
        let short: Vec<_> = entries
            .iter()
            .map(|e| {
                (
                    e.path
                        .strip_prefix(dir.path())
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    e.line,
                    e.column,
                    e.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            short,
            vec![
                (
                    "src/main.rs".into(),
                    1,
                    4,
                    "error[E0425]: cannot find value `x` in this scope"
                ),
                (
                    "main.cpp".into(),
                    2,
                    9,
                    "error: expected ';' before '}' token"
                ),
                ("main.cpp".into(), 6, 0, "warning: something"),
                ("src/main.rs".into(), 0, 3, "fn main() {}"),
            ]
        );
    }

    #[test]
    fn absolute_paths_and_duplicates() {
        let dir = dir();
        let file = dir.path().join("main.cpp");
        let output = format!("{0}:1:1: a\n{0}:1:1: a\n{0}:2:1: b\n", file.display());
        let entries = parse(&output, Path::new("/"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, file);
    }
}
