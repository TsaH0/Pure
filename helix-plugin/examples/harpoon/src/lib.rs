//! Example plugin: harpoon. Mark the few files you work on and jump to them
//! with one key, per project.
//!
//! - `add` (`space +`) marks the current file (and remembers the line).
//! - `go-1` … `go-4` (`space 1` … `space 4`) open a mark at its line.
//! - `menu` (`space =`) lists the marks in a buffer: reorder or delete lines
//!   and `:w` to save, enter opens the file on the cursor's line.
//! - `next` / `prev` cycle through the marks; `clear` forgets them.
//!
//! Marks are kept in the plugin's data directory, one list per project.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use helix_plugin_api::{
    editor, register_plugin, text, CommandInfo, DocumentId, OpenAction, Plugin, Registration,
};

const MENU: &str = "harpoon://marks";

#[derive(Debug, Clone, PartialEq)]
struct Mark {
    path: String,
    /// 1-based.
    line: u32,
}

thread_local! {
    static MARKS: RefCell<Option<Vec<Mark>>> = const { RefCell::new(None) };
}

/// The project marks are kept for: the workspace, else the working directory.
fn project() -> String {
    editor::workspace_root().unwrap_or_else(editor::cwd)
}

fn store() -> PathBuf {
    Path::new(&editor::data_dir()).join("marks.txt")
}

/// The stored marks of every project: `[project]` then `line path` lines.
fn parse_store(source: &str) -> Vec<(String, Vec<Mark>)> {
    let mut projects: Vec<(String, Vec<Mark>)> = Vec::new();
    for line in source.lines() {
        if let Some(project) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            projects.push((project.to_string(), Vec::new()));
        } else if let (Some((_, marks)), Some((number, path))) = (projects.last_mut(), line.split_once(' ')) {
            if let Ok(line) = number.parse() {
                marks.push(Mark { path: path.to_string(), line });
            }
        }
    }
    projects
}

fn render_store(projects: &[(String, Vec<Mark>)]) -> String {
    let mut out = String::new();
    for (project, marks) in projects.iter().filter(|(_, marks)| !marks.is_empty()) {
        out.push_str(&format!("[{project}]\n"));
        for mark in marks {
            out.push_str(&format!("{} {}\n", mark.line, mark.path));
        }
    }
    out
}

fn marks() -> Vec<Mark> {
    MARKS.with_borrow_mut(|marks| {
        marks
            .get_or_insert_with(|| {
                let source = std::fs::read_to_string(store()).unwrap_or_default();
                let project = project();
                parse_store(&source)
                    .into_iter()
                    .find(|(p, _)| *p == project)
                    .map(|(_, marks)| marks)
                    .unwrap_or_default()
            })
            .clone()
    })
}

fn save(new: Vec<Mark>) -> Result<(), String> {
    let path = store();
    let mut projects = parse_store(&std::fs::read_to_string(&path).unwrap_or_default());
    let project = project();
    match projects.iter_mut().find(|(p, _)| *p == project) {
        Some((_, marks)) => *marks = new.clone(),
        None => projects.push((project, new.clone())),
    }
    std::fs::write(&path, render_store(&projects)).map_err(|err| format!("{}: {err}", path.display()))?;
    MARKS.set(Some(new));
    Ok(())
}

/// A path shown relative to the project when it is inside.
fn display(path: &str) -> String {
    let project = project();
    path.strip_prefix(&project)
        .map(|rest| rest.trim_start_matches('/').to_string())
        .unwrap_or_else(|| path.to_string())
}

fn absolute(shown: &str) -> String {
    if shown.starts_with('/') {
        shown.to_string()
    } else {
        format!("{}/{shown}", project().trim_end_matches('/'))
    }
}

fn current() -> Result<(DocumentId, String, u32), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let path = editor::document_path(doc)?.ok_or("this buffer has no file to mark")?;
    let text = editor::text(doc)?;
    let head = editor::selections(doc)?.first().map_or(0, |r| r.head);
    let line = text[..text::char_to_byte(&text, head)].matches('\n').count() as u32 + 1;
    Ok((doc, path, line))
}

fn add() -> Result<(), String> {
    let (_, path, line) = current()?;
    let mut marks = marks();
    match marks.iter().position(|m| m.path == path) {
        Some(index) => marks[index].line = line,
        None => marks.push(Mark { path: path.clone(), line }),
    }
    let number = marks.iter().position(|m| m.path == path).unwrap() + 1;
    save(marks)?;
    editor::set_status(&format!("harpoon: {} is mark {number}", display(&path)));
    Ok(())
}

fn go(number: usize) -> Result<(), String> {
    // Remember where we were in the file we leave.
    if let Ok((_, path, line)) = current() {
        let mut marks = marks();
        if let Some(mark) = marks.iter_mut().find(|m| m.path == path) {
            if mark.line != line {
                mark.line = line;
                save(marks)?;
            }
        }
    }
    let marks = marks();
    let mark = marks
        .get(number.wrapping_sub(1))
        .ok_or_else(|| format!("harpoon: no mark {number} ({} marked)", marks.len()))?;
    editor::open_file_at(&mark.path, OpenAction::Replace, Some(mark.line), None)
}

/// The mark after (or before) the current file's.
fn cycle(step: isize) -> Result<(), String> {
    let marks = marks();
    if marks.is_empty() {
        return Err("harpoon: no marks".into());
    }
    let here = current().ok().and_then(|(_, path, _)| marks.iter().position(|m| m.path == path));
    let next = match here {
        Some(index) => (index as isize + step).rem_euclid(marks.len() as isize) as usize,
        None => 0,
    };
    go(next + 1)
}

fn menu_text() -> String {
    marks().iter().map(|m| format!("{}\n", display(&m.path))).collect()
}

/// Save the menu's order: lines are marks (kept lines keep their line).
fn save_menu(text: &str) -> Result<(), String> {
    let old = marks();
    let new: Vec<Mark> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|shown| {
            let path = absolute(shown);
            let line = old.iter().find(|m| m.path == path).map_or(1, |m| m.line);
            Mark { path, line }
        })
        .collect();
    save(new)
}

struct Harpoon;

impl Plugin for Harpoon {
    fn init(_config: String) -> Result<Registration, String> {
        let command = |name: &str, doc: &str| CommandInfo { name: name.into(), doc: doc.into() };
        let mut commands = vec![
            command("add", "Mark the current file"),
            command("menu", "Edit the marks (reorder, delete; :w saves)"),
            command("select", "Open the mark on the cursor's line of the menu"),
            command("clear", "Forget this project's marks"),
            command("next", "Open the next mark"),
            command("prev", "Open the previous mark"),
        ];
        for n in 1..=4 {
            commands.push(command(&format!("go-{n}"), &format!("Open mark {n}")));
        }
        Ok(Registration { commands, events: Vec::new() })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "add" => add(),
            "menu" => editor::open_buffer(MENU, &menu_text(), None, OpenAction::Hsplit),
            "select" => {
                let doc = editor::current_document().ok_or("no document")?;
                let text = editor::text(doc)?;
                let head = editor::selections(doc)?.first().map_or(0, |r| r.head);
                let line = text[..text::char_to_byte(&text, head)].matches('\n').count();
                let shown = text.lines().nth(line).map(str::trim).unwrap_or_default();
                if shown.is_empty() {
                    return Ok(());
                }
                let path = absolute(shown);
                let line = marks().iter().find(|m| m.path == path).map_or(1, |m| m.line);
                editor::run_command(":buffer-close!")?;
                editor::open_file_at(&path, OpenAction::Replace, Some(line), None)
            }
            "clear" => {
                save(Vec::new())?;
                editor::set_status("harpoon: no marks");
                Ok(())
            }
            "next" => cycle(1),
            "prev" => cycle(-1),
            other if other.starts_with("go-") => go(other[3..].parse().map_err(|_| "which mark?")?),
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_buffer_write(doc: DocumentId) -> Result<(), String> {
        save_menu(&editor::text(doc)?)?;
        editor::mark_saved(doc)?;
        editor::set_status(&format!("harpoon: {} marks", marks().len()));
        Ok(())
    }
}

register_plugin!(Harpoon);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn the_store_keeps_projects_apart() {
        let source = "[/a]\n3 /a/x.rs\n1 /a/y.rs\n[/b]\n7 /b/z.rs\n";
        let projects = parse_store(source);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].1[0], Mark { path: "/a/x.rs".into(), line: 3 });
        assert_eq!(render_store(&projects), source);
        // Projects without marks aren't written.
        assert_eq!(render_store(&[("/c".into(), Vec::new())]), "");
    }
}
