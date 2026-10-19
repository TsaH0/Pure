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

use helix_plugin_api::{
    editor, register_plugin, Choice, CommandInfo, Item, OpenAction, Picker, Plugin,
    Registration, SpawnOptions,
};

struct Finder;

thread_local! {
    /// The latest live grep query, so results of older ones are dropped.
    static GREP_GENERATION: Cell<u64> = const { Cell::new(0) };
}

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

/// Open `path` (relative to the root), at `line` (1-based), as chosen.
fn open(path: &str, line: Option<u32>, action: Option<OpenAction>) -> Result<(), String> {
    let path = format!("{}/{path}", root());
    let quoted = format!("\"{}\"", path.replace('"', "\\\""));
    match action {
        Some(OpenAction::Hsplit) => editor::run_command(&format!(":hsplit {quoted}"))?,
        Some(OpenAction::Vsplit) => editor::run_command(&format!(":vsplit {quoted}"))?,
        Some(OpenAction::Replace) => editor::open_file(&path)?,
        None => return Ok(()), // cancelled
    }
    if let Some(line) = line {
        editor::run_command(&format!(":goto {line}"))?;
    }
    Ok(())
}

fn files() -> Result<(), String> {
    editor::spawn(&["rg", "--files"], in_root(), |output| {
        let output = output?;
        if output.exit_code != Some(0) {
            return Err(output.stderr);
        }
        let root = root();
        let items = output
            .stdout
            .lines()
            .map(|path| Item::text(path).with_file_preview(format!("{root}/{path}"), None))
            .collect();
        editor::pick(Picker::new("Files", items), |choice: Choice| match choice.items.first() {
            Some(item) => open(&item.data, None, choice.action),
            None => Ok(()),
        })?;
        Ok(())
    })?;
    Ok(())
}

fn grep() -> Result<(), String> {
    let mut picker = Picker::new("Grep", vec![]);
    picker.columns = vec!["Location".into(), "Text".into()];
    editor::pick_live(
        picker,
        |id, query| {
            let generation = GREP_GENERATION.get() + 1;
            GREP_GENERATION.set(generation);
            if query.is_empty() {
                return Ok(Some(vec![]));
            }
            editor::spawn(
                &["rg", "--vimgrep", "--smart-case", "--max-count", "100", "--", query],
                in_root(),
                move |output| {
                    if GREP_GENERATION.get() != generation {
                        return Ok(()); // a newer query is running
                    }
                    let root = root();
                    let output = output?;
                    let items = output.stdout.lines().filter_map(|line| {
                        // file:line:column:text
                        let mut parts = line.splitn(4, ':');
                        let (path, line, _column, text) =
                            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
                        let number: u32 = line.parse().ok()?;
                        Some(
                            Item::new(
                                vec![format!("{path}:{line}"), text.trim().to_string()],
                                format!("{path}:{line}"),
                            )
                            .with_file_preview(
                                format!("{root}/{path}"),
                                Some((number - 1, number - 1)),
                            ),
                        )
                    });
                    editor::set_items(id, items.collect())
                },
            )?;
            Ok(None)
        },
        |choice| match choice.items.first() {
            Some(item) => {
                let (path, line) = item.data.rsplit_once(':').ok_or("bad item")?;
                open(path, line.parse().ok(), choice.action)
            }
            None => Ok(()),
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
