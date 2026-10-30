//! Example plugin (API 0.5): Vim's numbered registers, built by a plugin
//! rather than the editor (Pure also has them built in:
//! `editor.registers = "vim"`). Before a yank, delete or change runs, a
//! `pre-command` hook reads what is selected and fills the registers like
//! Vim, unless the command was given a register:
//!
//! - `"0`: the last yank;
//! - `"1`: the last delete or change spanning lines, the older ones moved
//!   to `"2`-`"9`;
//! - `"-`: the last smaller delete.

use helix_plugin_api::{
    editor, register_plugin, state,
    subscriptions::{self, EventType},
    HookOutcome, Plugin, Registration,
};

const YANKS: &[&str] = &["yank"];
const DELETES: &[&str] = &["delete_selection", "change_selection"];

/// What every selection of the current document holds.
fn selected() -> Result<Vec<String>, String> {
    let doc = editor::current_document().ok_or("no document")?;
    let mut texts = Vec::new();
    for range in editor::selections(doc)? {
        let (from, to) = (range.anchor.min(range.head), range.anchor.max(range.head));
        texts.push(editor::text_range(doc, from, to)?);
    }
    Ok(texts)
}

fn record(command: &str) -> Result<(), String> {
    // A named register (`"a`, `"_`) leaves these alone, as in Vim.
    if state::command_register().is_some() {
        return Ok(());
    }
    let texts = selected()?;
    if YANKS.contains(&command) {
        return editor::set_register('0', &texts);
    }
    if texts.iter().any(|text| text.contains('\n')) {
        for n in (1..9u8).rev() {
            let older = editor::register(char::from(b'0' + n))?;
            if !older.is_empty() {
                editor::set_register(char::from(b'0' + n + 1), &older)?;
            }
        }
        editor::set_register('1', &texts)
    } else {
        editor::set_register('-', &texts)
    }
}

struct VimRegisters;

impl Plugin for VimRegisters {
    fn init(_config: String) -> Result<Registration, String> {
        let filter = subscriptions::Filter {
            commands: YANKS.iter().chain(DELETES).map(|c| c.to_string()).collect(),
            ..subscriptions::any()
        };
        subscriptions::subscribe(EventType::PreCommand, &filter)?;
        Ok(Registration {
            commands: vec![],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }

    fn on_pre_command(command: &str) -> HookOutcome {
        if let Err(err) = record(command) {
            editor::log(&format!("vim-registers: {err}"));
        }
        HookOutcome::Proceed
    }
}

register_plugin!(VimRegisters);
