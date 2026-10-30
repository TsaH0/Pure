//! Example plugin: a file tree in a sidebar (API 0.4), like nvim-tree.
//!
//! `space B` (or `:tree/toggle`) shows the workspace as a tree on the left
//! and gives it the keys; again hides it. In the tree: `j`/`k` move, `enter`
//! (or `l`, or a click) opens a file or folds a folder open and shut, `esc`
//! gives the keys back to the editor, `q` closes it. Opening a file moves the
//! keys to the editor; the tree follows the file you're in.
//!
//! Settings in `plugins.toml`:
//!
//! ```toml
//! [tree]
//! allow-files = "read"   # needed: the workspace
//! width = 32
//! hidden = false         # show dot files
//! icons = true           # file type icons (needs a Nerd Font)
//! ```

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use helix_plugin_api::{
    editor, panels, panels2, register_plugin,
    sidebar::{self, item, on_enter, SidebarItem, SidebarView},
    CommandInfo, DocumentId, EventKind, OpenAction, Plugin, Registration,
};

struct Tree {
    root: Option<PathBuf>,
    /// Folders shown open.
    open: BTreeSet<PathBuf>,
    shown: bool,
    hidden: bool,
    width: u32,
    /// Nerd Font file type icons.
    icons: bool,
    /// The path of each item, to select an entry.
    paths: Vec<PathBuf>,
}

thread_local! {
    static TREE: RefCell<Tree> = const {
        RefCell::new(Tree {
            root: None,
            open: BTreeSet::new(),
            shown: false,
            hidden: false,
            width: 32,
            icons: false,
            paths: Vec::new(),
        })
    };
}

fn root() -> PathBuf {
    TREE.with_borrow(|tree| tree.root.clone())
        .or_else(|| editor::workspace_root().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(editor::cwd()))
}

/// A folder's entries, folders first, then by name.
fn entries(dir: &Path, hidden: bool) -> Vec<(String, bool)> {
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|read| {
            read.flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let is_dir = entry.path().is_dir();
                    (name, is_dir)
                })
                .filter(|(name, _)| hidden || !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by_cached_key(|(name, is_dir)| (!is_dir, name.to_lowercase()));
    entries
}

/// The Nerd Font icon of a file or folder (and a space), and its colour,
/// like nvim-web-devicons.
fn icon(name: &str, is_dir: bool, open: bool) -> (&'static str, &'static str) {
    if is_dir {
        return (if open { "\u{f07c} " } else { "\u{f07b} " }, "#7ebae4");
    }
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    match (name, extension.to_ascii_lowercase().as_str()) {
        ("Cargo.toml" | "Cargo.lock", _) | (_, "rs") => ("\u{e7a8} ", "#dea584"),
        (".gitignore" | ".gitattributes" | ".gitmodules", _) => ("\u{e702} ", "#f54d27"),
        ("Makefile", _) => ("\u{e779} ", "#6d8086"),
        (_, "py") => ("\u{e606} ", "#ffbc03"),
        (_, "js" | "mjs") => ("\u{e74e} ", "#cbcb41"),
        (_, "ts") => ("\u{e628} ", "#519aba"),
        (_, "jsx" | "tsx") => ("\u{e7ba} ", "#20c2e3"),
        (_, "json") => ("\u{e60b} ", "#cbcb41"),
        (_, "toml") => ("\u{e6b2} ", "#9c4221"),
        (_, "yaml" | "yml") => ("\u{e6a8} ", "#6d8086"),
        (_, "md") => ("\u{e73e} ", "#dddddd"),
        (_, "lua") => ("\u{e620} ", "#51a0cf"),
        (_, "go") => ("\u{e627} ", "#00add8"),
        (_, "c" | "h") => ("\u{e61e} ", "#599eff"),
        (_, "cpp" | "cc" | "hpp") => ("\u{e61d} ", "#f34b7d"),
        (_, "java") => ("\u{e738} ", "#cc3e44"),
        (_, "sh" | "bash" | "zsh" | "fish") => ("\u{e795} ", "#89e051"),
        (_, "html") => ("\u{e736} ", "#e44d26"),
        (_, "css") => ("\u{e749} ", "#42a5f5"),
        (_, "nix") => ("\u{f313} ", "#7ebae4"),
        (_, "wasm" | "wit") => ("\u{e6a1} ", "#5c4cdb"),
        (_, "png" | "jpg" | "jpeg" | "gif" | "svg") => ("\u{e60d} ", "#a074c4"),
        (_, "lock") => ("\u{f023} ", "#bbbbbb"),
        _ => ("\u{f15b} ", "#6d8086"),
    }
}

/// A row: the item, its path, and its icon's colour.
type Row = (SidebarItem, PathBuf, Option<&'static str>);

/// The visible rows.
fn rows(root: &Path, open: &BTreeSet<PathBuf>, hidden: bool) -> Vec<Row> {
    fn walk(
        dir: &Path,
        depth: u32,
        open: &BTreeSet<PathBuf>,
        hidden: bool,
        icons: bool,
        out: &mut Vec<Row>,
    ) {
        for (name, is_dir) in entries(dir, hidden) {
            let path = dir.join(&name);
            let arg = vec![path.to_string_lossy().into_owned()];
            if is_dir {
                let is_open = open.contains(&path);
                let marker = if is_open { "▾ " } else { "▸ " };
                let (glyph, colour) = if icons {
                    icon(&name, true, is_open)
                } else {
                    ("", "")
                };
                let row = item(
                    format!("{marker}{glyph}{name}/"),
                    "ui.text.directory",
                    depth,
                );
                let colour = icons.then_some(colour);
                out.push((on_enter(row, "fold", arg), path.clone(), colour));
                if is_open {
                    walk(&path, depth + 1, open, hidden, icons, out);
                }
            } else {
                let scope = if name.starts_with('.') { "comment" } else { "" };
                let (glyph, colour) = if icons {
                    icon(&name, false, false)
                } else {
                    ("", "")
                };
                let row = item(format!("  {glyph}{name}"), scope, depth);
                out.push((on_enter(row, "open", arg), path, icons.then_some(colour)));
            }
        }
    }
    let mut out = Vec::new();
    let icons = TREE.with_borrow(|tree| tree.icons);
    walk(root, 0, open, hidden, icons, &mut out);
    out
}

/// Show the tree, selecting `select` if it is shown.
fn show(select: Option<&Path>) -> Result<(), String> {
    let root = root();
    let (open, hidden, width) =
        TREE.with_borrow(|tree| (tree.open.clone(), tree.hidden, tree.width));
    let rows = rows(&root, &open, hidden);
    let selected = select.and_then(|path| rows.iter().position(|(_, p, _)| p == path));
    // Each icon (after the two-column marker) in its own colour.
    let spans: Vec<panels2::Span> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, (_, _, colour))| {
            Some(panels2::Span {
                item: i as u32,
                start: 2,
                end: 3,
                scope: (*colour)?.to_string(),
            })
        })
        .collect();
    let title = root.file_name().map_or_else(
        || root.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let (items, paths): (Vec<_>, Vec<_>) =
        rows.into_iter().map(|(item, path, _)| (item, path)).unzip();
    TREE.with_borrow_mut(|tree| {
        tree.shown = true;
        tree.paths = paths;
    });
    sidebar::show(&SidebarView {
        title: format!(" {title}/"),
        items,
        width,
        selected: selected.map(|i| i as u32),
    })?;
    if spans.is_empty() {
        return Ok(());
    }
    panels2::style_spans(panels::Dock::Left, &spans)
}

/// Open the folders down to `path` and select it.
fn reveal(path: &Path) -> Result<(), String> {
    let root = root();
    if !path.starts_with(&root) {
        // Not in the tree's folder: show the tree as it is.
        return show(None);
    }
    TREE.with_borrow_mut(|tree| {
        for dir in path
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&root) && *dir != root)
        {
            tree.open.insert(dir.to_path_buf());
        }
    });
    show(Some(path))
}

fn current_path() -> Option<PathBuf> {
    let doc = editor::current_document()?;
    editor::document_path(doc).ok().flatten().map(PathBuf::from)
}

struct TreePlugin;

impl Plugin for TreePlugin {
    fn init(config: String) -> Result<Registration, String> {
        if let Some(width) = config
            .split("\"width\":")
            .nth(1)
            .and_then(|rest| rest.split([',', '}']).next())
            .and_then(|n| n.trim().parse().ok())
        {
            TREE.with_borrow_mut(|tree| tree.width = width);
        }
        let hidden = config.contains("\"hidden\":true");
        let icons = config.contains("\"icons\":true");
        TREE.with_borrow_mut(|tree| {
            tree.hidden = hidden;
            tree.icons = icons;
        });
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("toggle", "Show the file tree and focus it, or hide it"),
                command("focus", "Show the file tree and focus it"),
                command("reveal", "Show the current file in the tree"),
                command("refresh", "Read the folders again"),
                command("toggle-hidden", "Show or hide dot files"),
                command("open", "Open a file (from the tree)"),
                command("fold", "Open or shut a folder (from the tree)"),
            ],
            events: vec![EventKind::DocumentOpen],
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "toggle" if TREE.with_borrow(|tree| tree.shown) => {
                TREE.with_borrow_mut(|tree| tree.shown = false);
                sidebar::close();
                Ok(())
            }
            "toggle" | "focus" => {
                match current_path() {
                    Some(path) => reveal(&path)?,
                    None => show(None)?,
                }
                sidebar::focus(true);
                Ok(())
            }
            "reveal" => current_path().map_or(Ok(()), |path| reveal(&path)),
            "refresh" => show(None),
            "toggle-hidden" => {
                TREE.with_borrow_mut(|tree| tree.hidden = !tree.hidden);
                show(None)
            }
            "open" => {
                let path = args.first().ok_or("open: missing path")?;
                editor::open_file_at(path, OpenAction::Replace, None, None)?;
                // The keys go to the file.
                sidebar::focus(false);
                Ok(())
            }
            "fold" => {
                let path = PathBuf::from(args.first().ok_or("fold: missing path")?);
                TREE.with_borrow_mut(|tree| {
                    if !tree.open.remove(&path) {
                        tree.open.insert(path.clone());
                    }
                });
                show(Some(&path))
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    /// Follow the file in the editor while the tree is shown.
    fn on_document_open(doc: DocumentId) {
        if !TREE.with_borrow(|tree| tree.shown) {
            return;
        }
        if let Ok(Some(path)) = editor::document_path(doc) {
            if let Err(err) = reveal(Path::new(&path)) {
                editor::log(&err);
            }
        }
    }
}

register_plugin!(TreePlugin);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn rows_follow_open_folders() {
        let root = std::env::temp_dir().join(format!("tree-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        std::fs::write(root.join(".hidden"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        let texts = |open: &BTreeSet<PathBuf>, hidden| {
            rows(&root, open, hidden)
                .into_iter()
                .map(|(item, _)| (item.text, item.depth))
                .collect::<Vec<_>>()
        };
        let mut open = BTreeSet::new();
        assert_eq!(
            texts(&open, false),
            [("▸ src/".to_string(), 0), ("  README.md".to_string(), 0)]
        );
        open.insert(root.join("src"));
        assert_eq!(
            texts(&open, false),
            [
                ("▾ src/".to_string(), 0),
                ("▸ deep/".to_string(), 1),
                ("  main.rs".to_string(), 1),
                ("  README.md".to_string(), 0),
            ]
        );
        assert_eq!(texts(&BTreeSet::new(), true).len(), 3);
        let (row, path) = rows(&root, &open, false).swap_remove(2);
        assert_eq!(row.command.as_deref(), Some("open"));
        assert_eq!(path, root.join("src/main.rs"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
