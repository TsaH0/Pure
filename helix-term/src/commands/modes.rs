//! `:mode`, `:modes` and `:describe-mode`: key modes (see
//! [`crate::keymap::modes`]).

use std::str::FromStr;

use helix_view::{document::Mode, Editor};

use crate::commands::{self, MappableCommand};
use crate::compositor::{self, Compositor};
use crate::keymap::modes::{self, Switch};
use crate::ui::{self, Picker, PickerColumn, Popup};

/// The current buffer and its language, as modes see them.
fn here(editor: &Editor) -> (helix_view::DocumentId, Option<String>) {
    let doc = helix_view::doc!(editor);
    (doc.id(), doc.language_name().map(str::to_string))
}

/// Run a mode's `on-enable` / `on-disable` commands.
pub fn run_commands(cx: &mut compositor::Context, commands: &[String]) {
    for name in commands {
        let command = match MappableCommand::from_str(name) {
            Ok(command) => command,
            Err(err) => {
                cx.editor.set_error(format!("mode command '{name}': {err}"));
                return;
            }
        };
        let mut ctx = commands::Context {
            register: None,
            count: None,
            editor: cx.editor,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: cx.jobs,
        };
        command.execute(&mut ctx);
        for callback in ctx.callback {
            crate::compositor::defer_ui(crate::compositor::DeferredUi::Callback(callback));
        }
    }
}

/// Turn `name` on or off here; tells what happened in the status.
pub fn switch(cx: &mut compositor::Context, name: &str, how: Switch) -> anyhow::Result<()> {
    let (doc, language) = here(cx.editor);
    let (on, commands) = modes::switch(Some(doc), language.as_deref(), name, how)
        .map_err(|err| anyhow::anyhow!(err))?;
    run_commands(cx, &commands);
    let global = modes::find(name).is_some_and(|mode| mode.config.global);
    cx.editor.set_status(format!(
        "Mode {name} {}{}",
        if on { "on" } else { "off" },
        if global { " (everywhere)" } else { "" }
    ));
    helix_event::request_redraw();
    Ok(())
}

pub struct Item {
    name: String,
    on: bool,
    global: bool,
    lighter: String,
    description: String,
}

/// The `:modes` picker: every mode, on or off here; Enter toggles one.
pub fn picker(editor: &Editor) -> Picker<Item, ()> {
    let (doc, language) = here(editor);
    let active = modes::active(Some(doc), language.as_deref());
    let items: Vec<Item> = modes::all()
        .iter()
        .map(|mode| Item {
            name: mode.name.clone(),
            on: active.iter().any(|on| on.name == mode.name),
            global: mode.config.global,
            lighter: mode.lighter().to_string(),
            description: mode.config.description.clone(),
        })
        .collect();
    let columns = [
        PickerColumn::new("mode", |item: &Item, _| item.name.as_str().into()),
        PickerColumn::new("state", |item: &Item, _| {
            match (item.on, item.global) {
                (true, true) => "on everywhere",
                (true, false) => "on",
                (false, _) => "off",
            }
            .into()
        }),
        PickerColumn::new("lighter", |item: &Item, _| item.lighter.as_str().into()),
        PickerColumn::new("description", |item: &Item, _| {
            item.description.as_str().into()
        }),
    ];
    Picker::new(columns, 0, items, (), |cx, item: &Item, _action| {
        if let Err(err) = switch(cx, &item.name, Switch::Toggle) {
            cx.editor.set_error(err.to_string());
        }
    })
}

/// `:describe-mode`: the modes on here and the keys each adds, highest
/// precedence first, like Emacs' `C-h m`.
pub fn describe(editor: &Editor, compositor: &mut Compositor, name: Option<&str>) {
    let (doc, language) = here(editor);
    let mut shown = match name {
        Some(name) => match modes::find(name) {
            Some(mode) => vec![mode],
            None => {
                return;
            }
        },
        None => modes::active(Some(doc), language.as_deref()),
    };
    shown.reverse();
    let mut text = String::new();
    if shown.is_empty() {
        text.push_str("No key modes are on here. `:modes` lists them all.");
    }
    for mode in shown {
        let config = &mode.config;
        text.push_str(&format!("# {}", mode.name));
        if mode.lighter() != mode.name {
            text.push_str(&format!(" ({})", mode.lighter()));
        }
        text.push('\n');
        if !config.description.is_empty() {
            text.push_str(&format!("\n{}\n", config.description));
        }
        let mut facts = vec![if config.global {
            "global"
        } else {
            "per buffer"
        }
        .to_string()];
        if !config.languages.is_empty() {
            facts.push(format!("on in {}", config.languages.join(", ")));
        }
        if !config.conflicts.is_empty() {
            facts.push(format!("turns off {}", config.conflicts.join(", ")));
        }
        if !config.requires.is_empty() {
            facts.push(format!("needs {}", config.requires.join(", ")));
        }
        if let modes::Owner::Plugin(plugin) = &mode.owner {
            facts.push(format!("from plugin {plugin}"));
        }
        text.push_str(&format!("\n{}\n", facts.join("; ")));
        let keys = modes::describe(&mode);
        if !keys.is_empty() {
            text.push_str("\n```\n");
            for (vim_mode, keys, what) in keys {
                let vim_mode = match vim_mode {
                    Mode::Normal => "normal",
                    Mode::Insert => "insert",
                    Mode::Select => "select",
                };
                text.push_str(&format!("{vim_mode:<7} {keys:<14} {what}\n"));
            }
            text.push_str("```\n");
        }
        text.push('\n');
    }
    let contents = ui::Markdown::new(text, editor.syn_loader.clone());
    let popup = Popup::new("describe-mode", contents).auto_close(true);
    compositor.replace_or_push("describe-mode", popup);
}
