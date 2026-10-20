//! Example plugin: edit the file system like a buffer, like oil.nvim.
//!
//! `-` (or `:oil/open`) opens the directory of the current file as a buffer,
//! one line per entry. Edit it like any text, then `:w`:
//!
//! - change a name to rename, delete a line to delete, add a line to create
//!   (a trailing `/` makes a directory, `a/b/c.txt` creates the parents);
//! - cut lines and paste them into another directory's buffer to move,
//!   copy and paste them to copy (also between directories);
//! - the changes of all modified directory buffers are applied together,
//!   after a confirmation listing them.
//!
//! Renames, moves, creates and deletes go through the editor, so open
//! buffers follow their files and language servers can update imports.
//! In directory buffers: `enter` opens, `-` goes up, `_` to the working
//! directory, `ctrl-s`/`ctrl-h` open in a split, `ctrl-p` previews, `ctrl-l`
//! refreshes, `ctrl-c` closes, `g.` shows hidden files, `gs` changes the
//! sort order, `gc` shows sizes and times, `` ` `` makes the directory the
//! working directory, `gy` yanks the entry's path, `gx` opens it with the
//! system's default program, `g?` shows these keys.
//!
//! Each line starts with the entry's id (`/001 `), drawn as its icon: it is
//! how the plugin knows a renamed or moved line is the same entry. Keep it.
//!
//! Settings in `plugins.toml`:
//!
//! ```toml
//! [oil]
//! allow-files = "write"        # needed: the workspace
//! allow-dirs = ["~"]           # more directories it may show and change
//! confirm = true               # ask before applying changes
//! hidden = false               # show dot files
//! details = false              # show sizes and modification times
//! icons = "unicode"            # or "nerd" (a Nerd Font) or "none"
//! allow-process = ["xdg-open"] # for `gx` (`open` on macOS)
//! ```
//!
//! Not (yet) possible: a trash (deleting is final, after the confirmation),
//! Unix permissions as a column (WASI doesn't expose them), and opening
//! directories given on the command line (Pure opens its file picker).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use helix_plugin_api::{
    decorations::{self, annotation, highlight, overlay, Annotation, Highlight},
    editor, files, register_plugin, text, CommandInfo, DocumentId, Edit, OpenAction, Plugin,
    PopupOptions, Range, Registration,
};

const SCHEME: &str = "oil://";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Dir,
    /// A symbolic link, to a directory or not.
    Link { dir: bool },
}

impl Kind {
    fn is_dir(self) -> bool {
        matches!(self, Kind::Dir | Kind::Link { dir: true })
    }
}

#[derive(Clone, Debug)]
struct Entry {
    dir: PathBuf,
    name: String,
    kind: Kind,
    size: u64,
    modified: Option<SystemTime>,
    target: Option<String>,
}

impl Entry {
    fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icons {
    Unicode,
    Nerd,
    None,
}

struct Oil {
    entries: HashMap<u32, Entry>,
    /// Ids by path, so an entry keeps its id across refreshes.
    ids: HashMap<PathBuf, u32>,
    next_id: u32,
    /// The ids each directory buffer listed at its last refresh, in order.
    listed: HashMap<PathBuf, Vec<u32>>,
    hidden: bool,
    sort: Sort,
    details: bool,
    confirm: bool,
    icons: Icons,
    /// Put the cursor on this entry when its directory's buffer opens.
    focus: Option<(PathBuf, String)>,
    /// A confirmation is open: further writes wait for it.
    confirming: bool,
}

thread_local! {
    static OIL: RefCell<Oil> = RefCell::new(Oil {
        entries: HashMap::new(),
        ids: HashMap::new(),
        next_id: 1,
        listed: HashMap::new(),
        hidden: false,
        sort: Sort::Name,
        details: false,
        confirm: true,
        icons: Icons::Unicode,
        focus: None,
        confirming: false,
    });
}

fn buffer_name(dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    if dir.ends_with('/') {
        format!("{SCHEME}{dir}")
    } else {
        format!("{SCHEME}{dir}/")
    }
}

/// The directory of one of our buffers.
fn buffer_dir(doc: DocumentId) -> Option<PathBuf> {
    let name = editor::buffer_name(doc)?;
    let path = name.strip_prefix(SCHEME)?.trim_end_matches('/');
    Some(PathBuf::from(if path.is_empty() { "/" } else { path }))
}

fn our_buffers() -> Vec<(DocumentId, PathBuf)> {
    editor::documents()
        .into_iter()
        .filter_map(|doc| Some((doc, buffer_dir(doc)?)))
        .collect()
}

/// Read `dir`, giving each entry an id (keeping known ones).
fn read_dir(dir: &Path) -> Result<Vec<u32>, String> {
    let read = std::fs::read_dir(dir).map_err(|err| {
        format!(
            "can't read {}: {err} (allow-files = \"write\" and allow-dirs in plugins.toml)",
            dir.display()
        )
    })?;
    let mut entries = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let path = dir.join(&name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let (kind, target) = if meta.file_type().is_symlink() {
            let dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
            let target = std::fs::read_link(&path)
                .ok()
                .map(|t| t.to_string_lossy().into_owned());
            (Kind::Link { dir }, target)
        } else if meta.is_dir() {
            (Kind::Dir, None)
        } else {
            (Kind::File, None)
        };
        entries.push(Entry {
            dir: dir.to_path_buf(),
            name,
            kind,
            size: meta.len(),
            modified: meta.modified().ok(),
            target,
        });
    }
    OIL.with_borrow_mut(|oil| {
        if !oil.hidden {
            entries.retain(|entry| !entry.name.starts_with('.'));
        }
        match oil.sort {
            Sort::Name => entries.sort_by_cached_key(|e| (!e.kind.is_dir(), e.name.to_lowercase())),
            Sort::Size => entries.sort_by_key(|e| (!e.kind.is_dir(), std::cmp::Reverse(e.size))),
            Sort::Modified => {
                entries.sort_by_key(|e| (!e.kind.is_dir(), std::cmp::Reverse(e.modified)))
            }
        }
        let ids: Vec<u32> = entries
            .into_iter()
            .map(|entry| {
                let path = entry.path();
                let id = match oil.ids.get(&path) {
                    Some(id) => *id,
                    None => {
                        let id = oil.next_id;
                        oil.next_id += 1;
                        oil.ids.insert(path, id);
                        id
                    }
                };
                oil.entries.insert(id, entry);
                id
            })
            .collect();
        oil.listed.insert(dir.to_path_buf(), ids.clone());
        Ok(ids)
    })
}

/// Digits of the ids: at least 3, so lines line up.
fn id_width() -> usize {
    OIL.with_borrow(|oil| oil.next_id.saturating_sub(1).to_string().len().max(3))
}

fn render(ids: &[u32]) -> String {
    let width = id_width();
    OIL.with_borrow(|oil| {
        ids.iter()
            .map(|id| {
                let entry = &oil.entries[id];
                let slash = if entry.kind.is_dir() { "/" } else { "" };
                format!("/{id:0width$} {}{slash}\n", entry.name)
            })
            .collect()
    })
}

/// A buffer line: its id (if it has one), its name and whether it names a
/// directory.
#[derive(Debug, PartialEq)]
struct Line {
    id: Option<u32>,
    name: String,
    dir: bool,
}

fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (id, rest) = match line.strip_prefix('/').and_then(|rest| rest.split_once(' ')) {
        Some((digits, rest)) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            (digits.parse().ok(), rest)
        }
        _ => (None, line),
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let dir = rest.ends_with('/');
    let name = rest.trim_end_matches('/').to_string();
    Some(Line { id, name, dir })
}

/// One change to the file system.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Create { path: PathBuf, dir: bool },
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf, dir: bool },
}

impl Op {
    fn describe(&self) -> String {
        match self {
            Op::Create { path, dir } => {
                format!("CREATE {}{}", path.display(), if *dir { "/" } else { "" })
            }
            Op::Copy { from, to } => format!("COPY   {} → {}", from.display(), to.display()),
            Op::Move { from, to } => format!("MOVE   {} → {}", from.display(), to.display()),
            Op::Delete { path, dir } => {
                format!("DELETE {}{}", path.display(), if *dir { "/" } else { "" })
            }
        }
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("invalid name '{name}'"));
    }
    Ok(())
}

/// What the edited buffers ask for. `buffers` are all our buffers with their
/// text; only `modified` ones are compared with their listing.
fn plan(buffers: &[(PathBuf, String, bool)]) -> Result<Vec<Op>, String> {
    OIL.with_borrow(|oil| {
        // Where each id is now, over all buffers.
        let mut placed: HashMap<u32, Vec<PathBuf>> = HashMap::new();
        let mut creates = Vec::new();
        let mut targets = HashSet::new();
        for (dir, text, modified) in buffers {
            for line in text.lines().filter_map(parse_line) {
                check_name(&line.name)?;
                let path = dir.join(&line.name);
                if *modified && !targets.insert(path.clone()) {
                    return Err(format!("{} is listed twice", path.display()));
                }
                match line.id.filter(|id| oil.entries.contains_key(id)) {
                    Some(id) => placed.entry(id).or_default().push(path),
                    None if *modified => creates.push(Op::Create { path, dir: line.dir }),
                    None => {}
                }
            }
        }
        let mut ops = Vec::new();
        let mut moves = Vec::new();
        // Copies and moves, in id order for a stable listing.
        let mut ids: Vec<_> = placed.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let entry = &oil.entries[&id];
            let origin = entry.path();
            let paths = &placed[&id];
            let kept = paths.contains(&origin);
            let mut moved = kept;
            for path in paths.iter().filter(|path| **path != origin) {
                if moved {
                    ops.push(Op::Copy {
                        from: origin.clone(),
                        to: path.clone(),
                    });
                } else {
                    moved = true;
                    moves.push(Op::Move {
                        from: origin.clone(),
                        to: path.clone(),
                    });
                }
            }
        }
        ops.extend(moves);
        // Deleted: listed in a modified buffer and now nowhere.
        for (dir, _, modified) in buffers {
            if !modified {
                continue;
            }
            for id in oil.listed.get(dir).into_iter().flatten() {
                if !placed.contains_key(id) {
                    let entry = &oil.entries[id];
                    ops.push(Op::Delete {
                        path: entry.path(),
                        dir: entry.kind == Kind::Dir,
                    });
                }
            }
        }
        ops.extend(creates);
        // Nothing may overwrite an existing entry that stays.
        let leaving: HashSet<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Move { from, .. } | Op::Delete { path: from, .. } => Some(from.clone()),
                _ => None,
            })
            .collect();
        for op in &ops {
            if let Op::Create { path, .. } | Op::Copy { to: path, .. } | Op::Move { to: path, .. } =
                op
            {
                if std::fs::symlink_metadata(path).is_ok() && !leaving.contains(path) {
                    return Err(format!("{} already exists", path.display()));
                }
            }
        }
        Ok(ops)
    })
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Apply `ops`, then refresh every directory buffer once the editor has
/// carried them out.
fn execute(ops: Vec<Op>) -> Result<(), String> {
    let mut errors = Vec::new();
    for op in &ops {
        let result = match op {
            // Copies happen here, through the plugin's file access.
            Op::Copy { from, to } => copy_recursive(from, to).map_err(|err| err.to_string()),
            Op::Move { from, to } => {
                // Moving into a directory that doesn't exist yet.
                let parent = to.parent().filter(|parent| !parent.exists());
                parent
                    .map_or(Ok(()), |parent| {
                        files::create_path(&parent.to_string_lossy(), true)
                    })
                    .and_then(|()| files::rename_path(&from.to_string_lossy(), &to.to_string_lossy()))
            }
            Op::Delete { path, .. } => files::delete_path(&path.to_string_lossy()),
            Op::Create { path, dir } => files::create_path(&path.to_string_lossy(), *dir),
        };
        if let Err(err) = result {
            errors.push(format!("{}: {err}", op.describe()));
        }
    }
    // The editor applies the file operations after this call: look again
    // right after.
    editor::set_timer(1, || {
        refresh_all(true)?;
        Ok(())
    })?;
    if errors.is_empty() {
        editor::set_status(&format!("oil: {} change(s) applied", ops.len()));
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Show the current listing in `doc`, keeping the cursor's line.
fn refresh(doc: DocumentId, dir: &Path) -> Result<(), String> {
    let ids = read_dir(dir)?;
    let new = render(&ids);
    let old = editor::text(doc)?;
    let cursor_line = editor::selections(doc)
        .ok()
        .and_then(|ranges| ranges.first().map(|range| range.head))
        .map(|head| text::line_starts(&old).partition_point(|&start| start <= head) - 1);
    if new != old {
        editor::apply_edits(
            doc,
            &[Edit {
                from: 0,
                to: text::char_len(&old),
                text: new.clone(),
            }],
        )?;
    }
    editor::mark_saved(doc)?;
    decorate(doc, &new)?;
    let focus = OIL.with_borrow_mut(|oil| {
        oil.focus
            .take_if(|(focus_dir, _)| focus_dir == dir)
            .map(|(_, name)| name)
    });
    let line = match focus {
        Some(name) => ids.iter().position(|id| {
            OIL.with_borrow(|oil| oil.entries.get(id).is_some_and(|e| e.name == name))
        }),
        None => cursor_line,
    };
    place_cursor(doc, &new, line.unwrap_or(0))
}

fn refresh_all(force: bool) -> Result<(), String> {
    for (doc, dir) in our_buffers() {
        if force || !editor::document_modified(doc)? {
            refresh(doc, &dir)?;
        }
    }
    Ok(())
}

/// The cursor on the name of entry `line`.
fn place_cursor(doc: DocumentId, text: &str, line: usize) -> Result<(), String> {
    let starts = text::line_starts(text);
    let line = line.min(starts.len().saturating_sub(2));
    let Some(&start) = starts.get(line) else {
        return Ok(());
    };
    let prefix = 2 + id_width() as u32;
    let len = text::char_len(text);
    let pos = (start + prefix).min(len.saturating_sub(1));
    editor::set_selections(doc, &[Range { anchor: pos, head: pos }])
}

fn icon(kind: Kind, icons: Icons) -> &'static str {
    match (icons, kind) {
        (Icons::None, _) => " ",
        (Icons::Unicode, Kind::Dir) => "▸",
        (Icons::Unicode, Kind::File) => "·",
        (Icons::Unicode, Kind::Link { .. }) => "↪",
        (Icons::Nerd, Kind::Dir) => "\u{f07b}",
        (Icons::Nerd, Kind::File) => "\u{f15b}",
        (Icons::Nerd, Kind::Link { .. }) => "\u{f0c1}",
    }
}

fn human_size(size: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size}B")
    } else {
        format!("{value:.1}{}", units[unit])
    }
}

/// `YYYY-MM-DD HH:MM` (UTC: WASI has no time zone).
fn date(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rest / 3600, rest % 3600 / 60)
}

/// Icons over the ids, names coloured, details and link targets as virtual
/// text.
fn decorate(doc: DocumentId, text: &str) -> Result<(), String> {
    let (icons, details) = OIL.with_borrow(|oil| (oil.icons, oil.details));
    let starts = text::line_starts(text);
    let mut overlays: Vec<Annotation> = Vec::new();
    let mut highlights: Vec<Highlight> = Vec::new();
    let mut annotations: Vec<Annotation> = Vec::new();
    OIL.with_borrow(|oil| {
        for (number, line) in text.lines().enumerate() {
            let Some(&start) = starts.get(number) else {
                break;
            };
            let Some(parsed) = parse_line(line) else {
                continue;
            };
            let entry = parsed.id.and_then(|id| oil.entries.get(&id));
            let prefix = match parsed.id {
                Some(_) => line.find(' ').map_or(0, |space| space + 1),
                None => line.len() - line.trim_start().len(),
            };
            if parsed.id.is_some() && prefix >= 2 {
                // `/001 ` becomes `   ▸ `.
                let kind = entry.map_or(Kind::File, |e| e.kind);
                let drawn = format!("{}{} ", " ".repeat(prefix - 2), icon(kind, icons));
                overlays.push(overlay(start, drawn, kind_scope(kind)));
            }
            let name_start = start + prefix as u32;
            let end = start + text::char_len(line);
            let scope = match (parsed.dir, entry) {
                (_, Some(e)) if matches!(e.kind, Kind::Link { .. }) => "string.special.symbol",
                (true, _) => "ui.text.directory",
                (false, None) => "diff.plus",
                _ if parsed.name.starts_with('.') => "comment",
                _ => "ui.text",
            };
            highlights.push(highlight(name_start, end, scope));
            if let Some(entry) = entry {
                if details {
                    let size = if entry.kind.is_dir() {
                        String::new()
                    } else {
                        human_size(entry.size)
                    };
                    let time = entry.modified.map(date).unwrap_or_default();
                    annotations.push(annotation(
                        name_start,
                        format!("{size:>7}  {time:16}  "),
                        "comment",
                    ));
                }
                if let Some(target) = &entry.target {
                    annotations.push(annotation(end, format!(" → {target}"), "comment"));
                }
            }
        }
    });
    decorations::set_overlays(doc, "icons", &overlays)?;
    decorations::set_highlights(doc, "names", &highlights)?;
    decorations::set_annotations(doc, "details", &annotations)
}

fn kind_scope(kind: Kind) -> &'static str {
    match kind {
        Kind::Dir => "ui.text.directory",
        Kind::Link { .. } => "string.special.symbol",
        Kind::File => "comment",
    }
}

/// Open `dir` in the current view, with the cursor on `focus` if given.
fn open_dir(dir: PathBuf, focus: Option<String>, action: OpenAction) -> Result<(), String> {
    let ids = read_dir(&dir)?;
    OIL.with_borrow_mut(|oil| oil.focus = Some((dir.clone(), focus.unwrap_or_default())));
    editor::open_buffer(&buffer_name(&dir), &render(&ids), None, action)
}

/// The entry under the primary cursor of the current buffer.
fn current_line() -> Result<(DocumentId, PathBuf, Line), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let dir = buffer_dir(doc).ok_or("not a directory buffer")?;
    let text = editor::text(doc)?;
    let head = editor::selections(doc)?
        .first()
        .map_or(0, |range| range.head);
    let number = text::line_starts(&text).partition_point(|&start| start <= head) - 1;
    let line = text
        .lines()
        .nth(number)
        .and_then(parse_line)
        .ok_or("no entry on this line")?;
    Ok((doc, dir, line))
}

fn select(action: OpenAction) -> Result<(), String> {
    let (_, dir, line) = current_line()?;
    let path = dir.join(&line.name);
    let is_dir = match line.id.and_then(|id| OIL.with_borrow(|oil| oil.entries.get(&id).cloned())) {
        Some(entry) if entry.path() == path => entry.kind.is_dir(),
        Some(_) | None if !path.exists() => {
            return Err("save (:w) first: this entry doesn't exist yet".into())
        }
        _ => path.is_dir(),
    };
    if is_dir {
        open_dir(path, None, action)
    } else {
        editor::open_file_at(&path.to_string_lossy(), action, None, None)
    }
}

/// The directory to start in: the current file's, or the working directory.
fn start_dir() -> (PathBuf, Option<String>) {
    let current = editor::current_document();
    if let Some(dir) = current.and_then(buffer_dir) {
        // Already in a directory buffer: go up.
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        return (dir.parent().map_or(dir.clone(), Path::to_path_buf), name);
    }
    match current
        .and_then(|doc| editor::document_path(doc).ok().flatten())
        .map(PathBuf::from)
    {
        Some(path) => (
            path.parent().map_or_else(|| PathBuf::from(editor::cwd()), Path::to_path_buf),
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
        ),
        None => (PathBuf::from(editor::cwd()), None),
    }
}

fn preview() -> Result<(), String> {
    let (_, dir, line) = current_line()?;
    let path = dir.join(&line.name);
    let body = if path.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&path)
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        names.sort();
        names.join("\n")
    } else {
        let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
        if bytes.iter().take(8000).any(|b| *b == 0) {
            format!("(binary, {})", human_size(bytes.len() as u64))
        } else {
            String::from_utf8_lossy(&bytes)
                .lines()
                .take(40)
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    editor::popup(&format!("**{}**\n\n```\n{}\n```", path.display(), body.replace("```", "ˋˋˋ")))
        .map(|_| ())
}

const HELP: &str = "**oil** — edit, then `:w` to apply\n\n\
| key | |\n|---|---|\n\
| `enter` | open |\n| `-` | parent directory |\n| `_` | working directory |\n\
| `ctrl-s` / `ctrl-h` | open in a vertical / horizontal split |\n\
| `ctrl-p` | preview |\n| `ctrl-l` | refresh |\n| `ctrl-c` | close |\n\
| `g.` | show hidden files |\n| `gs` | sort by name, size, time |\n\
| `gc` | sizes and times |\n| `` ` `` | make it the working directory |\n\
| `gy` | yank the path |\n| `gx` | open with the system |\n| `g?` | this help |";

/// On `:w`: plan the changes of all modified directory buffers, confirm,
/// apply.
fn write() -> Result<(), String> {
    if OIL.with_borrow(|oil| oil.confirming) {
        return Ok(());
    }
    let buffers: Vec<(PathBuf, String, bool)> = our_buffers()
        .into_iter()
        .map(|(doc, dir)| {
            Ok((dir, editor::text(doc)?, editor::document_modified(doc)?))
        })
        .collect::<Result<_, String>>()?;
    let ops = plan(&buffers)?;
    if ops.is_empty() {
        return refresh_all(true);
    }
    if !OIL.with_borrow(|oil| oil.confirm) {
        return execute(ops);
    }
    let list = ops
        .iter()
        .map(|op| format!("- `{}`", op.describe()))
        .collect::<Vec<_>>()
        .join("\n");
    let popup = editor::popup_with_options(
        &format!("**Apply these changes?**\n\n{list}"),
        PopupOptions { centered: true },
        |_| Ok(()),
    )?;
    OIL.with_borrow_mut(|oil| oil.confirming = true);
    let count = ops.len();
    editor::prompt(
        &format!("apply {count} change(s)? (y/n):"),
        "",
        vec!["y".into(), "n".into()],
        move |answer| {
            OIL.with_borrow_mut(|oil| oil.confirming = false);
            editor::close_popup(popup);
            match answer.as_deref().map(str::trim) {
                Some("y" | "yes") => execute(ops),
                _ => {
                    editor::set_status("oil: nothing changed");
                    Ok(())
                }
            }
        },
    )?;
    Ok(())
}

struct OilPlugin;

impl Plugin for OilPlugin {
    fn init(config: String) -> Result<Registration, String> {
        let flag = |key: &str| {
            config
                .split(&format!("\"{key}\":"))
                .nth(1)
                .map(|rest| rest.trim_start().starts_with("true"))
        };
        let icons = config
            .split("\"icons\":")
            .nth(1)
            .map(|rest| rest.trim_start().trim_start_matches('"'));
        OIL.with_borrow_mut(|oil| {
            oil.confirm = flag("confirm").unwrap_or(true);
            oil.hidden = flag("hidden").unwrap_or(false);
            oil.details = flag("details").unwrap_or(false);
            oil.icons = match icons {
                Some(rest) if rest.starts_with("nerd") => Icons::Nerd,
                Some(rest) if rest.starts_with("none") => Icons::None,
                _ => Icons::Unicode,
            };
        });
        let commands = [
            ("open", "Open a directory (default: the current file's) as a buffer"),
            ("open-parent", "Open the current file's directory"),
            ("select", "Open the entry under the cursor"),
            ("select-vsplit", "Open the entry in a vertical split"),
            ("select-hsplit", "Open the entry in a horizontal split"),
            ("parent", "Go to the parent directory"),
            ("cwd", "Go to the working directory"),
            ("preview", "Preview the entry under the cursor"),
            ("refresh", "Read the directory again (drops unsaved changes)"),
            ("close", "Close the directory buffer"),
            ("toggle-hidden", "Show or hide dot files"),
            ("cycle-sort", "Sort by name, size or modification time"),
            ("toggle-details", "Show or hide sizes and modification times"),
            ("help", "Show the keys of directory buffers"),
            ("cd", "Make the current directory the working directory"),
            ("yank-path", "Yank the path of the entry under the cursor"),
            ("open-external", "Open the entry with the system's default program"),
        ];
        Ok(Registration {
            commands: commands
                .into_iter()
                .map(|(name, doc)| CommandInfo {
                    name: name.into(),
                    doc: doc.into(),
                })
                .collect(),
            events: Vec::new(),
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "open" => match args.first() {
                Some(dir) => {
                    let dir = PathBuf::from(dir);
                    let dir = if dir.is_absolute() {
                        dir
                    } else {
                        PathBuf::from(editor::cwd()).join(dir)
                    };
                    open_dir(dir, None, OpenAction::Replace)
                }
                None => {
                    let (dir, focus) = start_dir();
                    open_dir(dir, focus, OpenAction::Replace)
                }
            },
            "open-parent" | "parent" => {
                let (dir, focus) = start_dir();
                open_dir(dir, focus, OpenAction::Replace)
            }
            "cwd" => open_dir(PathBuf::from(editor::cwd()), None, OpenAction::Replace),
            "select" => select(OpenAction::Replace),
            "select-vsplit" => select(OpenAction::Vsplit),
            "select-hsplit" => select(OpenAction::Hsplit),
            "preview" => preview(),
            "refresh" => {
                let doc = editor::current_document().ok_or("no document")?;
                let dir = buffer_dir(doc).ok_or("not a directory buffer")?;
                refresh(doc, &dir)
            }
            "close" => editor::run_command(":buffer-close"),
            "toggle-hidden" => {
                OIL.with_borrow_mut(|oil| oil.hidden = !oil.hidden);
                refresh_all(false)
            }
            "cycle-sort" => {
                let sort = OIL.with_borrow_mut(|oil| {
                    oil.sort = match oil.sort {
                        Sort::Name => Sort::Size,
                        Sort::Size => Sort::Modified,
                        Sort::Modified => Sort::Name,
                    };
                    oil.sort
                });
                editor::set_status(match sort {
                    Sort::Name => "oil: sorted by name",
                    Sort::Size => "oil: sorted by size",
                    Sort::Modified => "oil: sorted by modification time",
                });
                refresh_all(false)
            }
            "toggle-details" => {
                OIL.with_borrow_mut(|oil| oil.details = !oil.details);
                for (doc, _) in our_buffers() {
                    decorate(doc, &editor::text(doc)?)?;
                }
                Ok(())
            }
            "help" => editor::popup(HELP).map(|_| ()),
            "cd" => {
                let doc = editor::current_document().ok_or("no document")?;
                let dir = buffer_dir(doc).ok_or("not a directory buffer")?;
                editor::run_command(&format!(":cd {}", dir.display()))
            }
            "yank-path" => {
                let (_, dir, line) = current_line()?;
                let path = dir.join(&line.name).to_string_lossy().into_owned();
                editor::set_register('"', std::slice::from_ref(&path))?;
                editor::set_status(&format!("yanked {path}"));
                Ok(())
            }
            "open-external" => {
                let (_, dir, line) = current_line()?;
                let path = dir.join(&line.name).to_string_lossy().into_owned();
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                editor::spawn(&[opener, path.as_str()], Default::default(), |result| {
                    match result {
                        Ok(output) if output.exit_code == Some(0) => Ok(()),
                        Ok(output) => Err(output.stderr.trim().to_string()),
                        Err(err) => Err(err),
                    }
                })
                .map(|_| ())
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    /// One of our buffers opened (or was shown again).
    fn on_document_open(doc: DocumentId) {
        let Some(dir) = buffer_dir(doc) else {
            return;
        };
        let result = match editor::document_modified(doc) {
            // Unsaved edits stay; just draw them.
            Ok(true) => editor::text(doc).and_then(|text| decorate(doc, &text)),
            _ => refresh(doc, &dir),
        };
        if let Err(err) = result {
            editor::set_error(&err);
        }
    }

    fn on_buffer_write(_doc: DocumentId) -> Result<(), String> {
        write()
    }
}

register_plugin!(OilPlugin);

#[cfg(test)]
mod test {
    use super::*;

    fn entry(id: u32, dir: &str, name: &str, kind: Kind) {
        OIL.with_borrow_mut(|oil| {
            oil.entries.insert(
                id,
                Entry {
                    dir: dir.into(),
                    name: name.into(),
                    kind,
                    size: 0,
                    modified: None,
                    target: None,
                },
            );
            oil.listed.entry(dir.into()).or_default().push(id);
        });
    }

    #[test]
    fn lines_parse() {
        assert_eq!(
            parse_line("/012 src/"),
            Some(Line { id: Some(12), name: "src".into(), dir: true })
        );
        assert_eq!(
            parse_line("new file.txt"),
            Some(Line { id: None, name: "new file.txt".into(), dir: false })
        );
        assert_eq!(parse_line("   "), None);
        assert!(check_name("a/../b").is_err());
        assert!(check_name("a/b.txt").is_ok());
    }

    #[test]
    fn edits_become_operations() {
        entry(1, "/nonexistent-a", "keep.txt", Kind::File);
        entry(2, "/nonexistent-a", "old.txt", Kind::File);
        entry(3, "/nonexistent-a", "gone", Kind::Dir);
        entry(4, "/nonexistent-a", "moved.txt", Kind::File);
        entry(5, "/nonexistent-a", "copied.txt", Kind::File);
        let a = "/001 keep.txt\n/002 new-name.txt\n/005 copied.txt\nfresh/\n";
        let b = "/004 moved.txt\n/005 copy.txt\n";
        let ops = plan(&[
            ("/nonexistent-a".into(), a.into(), true),
            ("/nonexistent-b".into(), b.into(), true),
        ])
        .unwrap();
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            ops,
            [
                Op::Copy { from: p("/nonexistent-a/copied.txt"), to: p("/nonexistent-b/copy.txt") },
                Op::Move { from: p("/nonexistent-a/old.txt"), to: p("/nonexistent-a/new-name.txt") },
                Op::Move { from: p("/nonexistent-a/moved.txt"), to: p("/nonexistent-b/moved.txt") },
                Op::Delete { path: p("/nonexistent-a/gone"), dir: true },
                Op::Create { path: p("/nonexistent-a/fresh"), dir: true },
            ]
        );
        assert!(plan(&[("/nonexistent-a".into(), "x\nx\n".into(), true)]).is_err());
    }

    #[test]
    fn dates_and_sizes() {
        assert_eq!(date(UNIX_EPOCH), "1970-01-01 00:00");
        assert_eq!(
            date(UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            "2023-11-14 22:13"
        );
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(1536), "1.5K");
    }
}
