//! Example plugin: pickers fed by programs.
//!
//! - `finder/files`: the files `rg --files` lists, with a preview.
//! - `finder/grep`: a live picker running `rg` for what you type.
//! - `finder/branches`: switch git branch.
//!
//! In the pickers, Enter opens the choice, `C-s` / `C-v` in a split. It needs
//! the user's grant in `plugins.toml`:
//!
//! ```toml
//! [finder]
//! allow-process = ["rg", "git"]
//! ```

use std::cell::Cell;
use std::rc::Rc;

use helix_plugin_api::{
    editor, register_plugin, Choice, CommandInfo, Item, OpenAction, Picker, Plugin, Registration,
    SpawnOptions,
};

struct Finder;

impl Plugin for Finder {
    fn init(_config: String) -> Result<Registration, String> {
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("files", "Pick a file (rg --files)"),
                command("grep", "Search the workspace as you type (rg)"),
                command("branches", "Switch git branch"),
            ],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "files" => files(),
            "grep" => grep(),
            "branches" => branches(),
            _ => Err(format!("unknown command {name}")),
        }
    }
}

/// Where to search: the workspace, or the working directory.
fn root() -> String {
    editor::workspace_root().unwrap_or_else(editor::cwd)
}

fn in_root() -> SpawnOptions {
    SpawnOptions {
        cwd: Some(root()),
        ..Default::default()
    }
}

/// Resolve against the root captured when this picker was opened.
fn file_path(root: &str, path: &str) -> String {
    std::path::Path::new(root)
        .join(path)
        .to_string_lossy()
        .into_owned()
}

fn open(
    root: &str,
    path: &str,
    line: Option<u32>,
    column: Option<u32>,
    action: Option<OpenAction>,
) -> Result<(), String> {
    let Some(action) = action else { return Ok(()) };
    editor::open_file_at(&file_path(root, path), action, line, column)
}

fn file_items(root: &str, output: &str) -> Vec<Item> {
    output
        .lines()
        .filter(|path| !path.is_empty())
        .map(|path| Item::text(path).with_file_preview(file_path(root, path), None))
        .collect()
}

fn files() -> Result<(), String> {
    let root = root();
    let closed = Rc::new(Cell::new(false));
    let running = Rc::new(Cell::new(None));
    let on_close = closed.clone();
    let on_choose = running.clone();
    let chosen_root = root.clone();
    let id = editor::pick(Picker::new("Files", vec![]), move |choice: Choice| {
        on_close.set(true);
        if let Some(process) = on_choose.take() {
            editor::kill(process);
        }
        match choice.items.first() {
            Some(item) => open(&chosen_root, &item.data, None, None, choice.action),
            None => Ok(()),
        }
    })?;
    let on_output = closed.clone();
    let output_root = root.clone();
    let options = SpawnOptions {
        cwd: Some(root.clone()),
        ..Default::default()
    };
    let completed = running.clone();
    let process = editor::spawn_stream(
        &["rg", "--files"],
        options,
        move |chunk| {
            if on_output.get() {
                return Ok(());
            }
            let items = file_items(&output_root, chunk);
            if !items.is_empty() {
                editor::push_items(id, items)?;
            }
            Ok(())
        },
        move |output| {
            completed.set(None);
            if closed.get() {
                return Ok(());
            }
            let output = output?;
            if !matches!(output.exit_code, Some(0 | 1)) {
                return Err(output.stderr);
            }
            let items = file_items(&root, &output.stdout);
            if !items.is_empty() {
                editor::push_items(id, items)?;
            }
            if !output.stderr.is_empty() {
                editor::set_status(output.stderr.trim());
            }
            Ok(())
        },
    )?;
    // The process starts only after this command's picker is installed.
    // Its callbacks cannot run until the guest call has returned.
    running.set(Some(process));
    Ok(())
}

fn grep() -> Result<(), String> {
    let root = root();
    let closed = Rc::new(Cell::new(false));
    let running = Rc::new(Cell::new(None));
    // Suppress obsolete process errors locally; every item batch also carries
    // the SDK/host generation, which protects the debounce gap.
    let latest = Rc::new(Cell::new(0));
    let on_close = closed.clone();
    let on_choose = running.clone();
    let chosen_root = root.clone();
    let mut picker = Picker::new("Grep", vec![]);
    picker.columns = vec!["Location".into(), "Text".into()];
    editor::pick_live(
        picker,
        move |id, generation, query| {
            latest.set(generation);
            if let Some(process) = running.take() {
                editor::kill(process);
            }
            if query.is_empty() {
                return Ok(Some(vec![]));
            }
            let request = Rc::new(Cell::new(None));
            let completed_request = request.clone();
            let active = running.clone();
            let done = closed.clone();
            let current = latest.clone();
            let output_root = root.clone();
            let options = SpawnOptions {
                cwd: Some(root.clone()),
                ..Default::default()
            };
            let process = editor::spawn(
                &[
                    "rg",
                    "--vimgrep",
                    "--smart-case",
                    "--max-count",
                    "100",
                    "--",
                    query,
                ],
                options,
                move |output| {
                    if active.get() == completed_request.get() {
                        active.set(None);
                    }
                    if done.get() || current.get() != generation {
                        return Ok(());
                    }
                    let output = output?;
                    if !matches!(output.exit_code, Some(0 | 1)) {
                        return Err(output.stderr);
                    }
                    let items = output
                        .stdout
                        .lines()
                        .filter_map(|line| {
                            let mut parts = line.splitn(4, ':');
                            let (path, line, column, text) =
                                (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
                            let number: u32 = line.parse().ok()?;
                            // ripgrep reports byte columns; the editor API uses
                            // one-based character columns. vimgrep includes the
                            // full source line, so no file read is needed.
                            let byte_column: usize = column.parse().ok()?;
                            let prefix = text.get(..byte_column.checked_sub(1)?)?;
                            let column =
                                u32::try_from(prefix.chars().count()).ok()?.checked_add(1)?;
                            Some(
                                Item::new(
                                    vec![format!("{path}:{line}:{column}"), text.trim().into()],
                                    format!("{path}:{line}:{column}"),
                                )
                                .with_file_preview(
                                    file_path(&output_root, path),
                                    Some((number.saturating_sub(1), number.saturating_sub(1))),
                                ),
                            )
                        })
                        .collect();
                    editor::set_items_for_query(id, generation, items)
                },
            )?;
            request.set(Some(process));
            running.set(Some(process));
            Ok(None)
        },
        move |choice| {
            on_close.set(true);
            if let Some(process) = on_choose.take() {
                editor::kill(process);
            }
            let Some(item) = choice.items.first() else {
                return Ok(());
            };
            let (location, column) = item.data.rsplit_once(':').ok_or("bad item column")?;
            let (path, line) = location.rsplit_once(':').ok_or("bad item line")?;
            open(
                &chosen_root,
                path,
                line.parse().ok(),
                column.parse().ok(),
                choice.action,
            )
        },
    )?;
    Ok(())
}

fn branches() -> Result<(), String> {
    editor::spawn(
        &["git", "branch", "--format=%(refname:short)"],
        in_root(),
        |output| {
            let output = output?;
            if output.exit_code != Some(0) {
                return Err(output.stderr);
            }
            let items = output.stdout.lines().map(Item::text).collect();
            editor::pick(Picker::new("Branches", items), |choice| {
                let Some(branch) = choice.items.first() else {
                    return Ok(());
                };
                let branch = branch.data.clone();
                let argv = ["git".to_string(), "switch".into(), branch.clone()];
                editor::spawn(&argv, in_root(), move |output| {
                    let output = output?;
                    if output.exit_code == Some(0) {
                        editor::set_status(&format!("Switched to {branch}"));
                        Ok(())
                    } else {
                        Err(output.stderr.trim().to_string())
                    }
                })?;
                Ok(())
            })?;
            Ok(())
        },
    )?;
    Ok(())
}

register_plugin!(Finder);
