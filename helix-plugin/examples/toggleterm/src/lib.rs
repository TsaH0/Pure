//! Example plugin (API 0.5 `terminal`): like toggleterm.nvim and
//! vim-slime. `space t` opens a terminal below (your shell, or the
//! `command` setting, a REPL say) and closes it again; `space ;` sends the
//! selections (or the cursor's line) to it, each followed by Enter.

use std::cell::RefCell;

use helix_plugin_api::{editor, register_plugin, terminal, windows, CommandInfo, Plugin, Registration};

thread_local! {
    static COMMAND: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The terminal, while open.
    static OPEN: RefCell<Option<u32>> = const { RefCell::new(None) };
}

/// The open terminal, if its program still runs.
fn open_terminal() -> Option<u32> {
    let id = OPEN.with_borrow(|open| *open)?;
    match terminal::exit_status(id) {
        Ok(None) => Some(id),
        // Ended or closed: forget it.
        _ => {
            OPEN.set(None);
            None
        }
    }
}

fn toggle() -> Result<(), String> {
    if let Some(id) = open_terminal() {
        OPEN.set(None);
        return terminal::close(id);
    }
    let command = COMMAND.with_borrow(Clone::clone);
    let id = terminal::open(&command, Some(windows::Side::Below))?;
    OPEN.set(Some(id));
    Ok(())
}

/// What `send` sends: the selected text, or the cursor's line when
/// nothing is selected (a 1-char selection).
fn chunks(text: &str, selections: &[(usize, usize)]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    selections
        .iter()
        .map(|&(from, to)| {
            let (from, to) = (from.min(to), from.max(to).min(chars.len()));
            if to - from <= 1 {
                let start = chars[..from].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
                let end = chars[from..].iter().position(|c| *c == '\n').map_or(chars.len(), |i| from + i);
                chars[start..end].iter().collect()
            } else {
                chars[from..to].iter().collect::<String>()
            }
        })
        .map(|chunk: String| chunk.trim_end_matches('\n').to_string())
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

fn send() -> Result<(), String> {
    let id = open_terminal().ok_or("no terminal open: space t opens one")?;
    let doc = editor::current_document().ok_or("no document")?;
    let text = editor::text(doc)?;
    let selections: Vec<(usize, usize)> = editor::selections(doc)?
        .iter()
        .map(|range| (range.anchor as usize, range.head as usize))
        .collect();
    for chunk in chunks(&text, &selections) {
        // Lines as typed: Enter after each.
        terminal::write(id, &format!("{}\r", chunk.replace('\n', "\r")))?;
    }
    Ok(())
}

fn configure(config: &str) {
    let config: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    let command = config["command"]
        .as_array()
        .map(|args| args.iter().filter_map(|arg| arg.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    COMMAND.set(command);
}

struct ToggleTerm;

impl Plugin for ToggleTerm {
    fn init(config: String) -> Result<Registration, String> {
        configure(&config);
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("toggle", "Open a terminal below, or close it"),
                command("send", "Send the selections (or the line) to the terminal"),
            ],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "toggle" => toggle(),
            "send" => send(),
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_event(event: helix_plugin_api::Event) {
        if let helix_plugin_api::Event::ConfigChange(config) = event {
            configure(&config);
        }
    }
}

register_plugin!(ToggleTerm);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn sends_selections_or_lines() {
        let text = "print(1)\nx = 2\n";
        // A cursor (1 char): its whole line.
        assert_eq!(chunks(text, &[(10, 11)]), ["x = 2"]);
        // A selection: just it.
        assert_eq!(chunks(text, &[(0, 8)]), ["print(1)"]);
        assert_eq!(chunks(text, &[(0, 15)]), ["print(1)\nx = 2"]);
    }
}
