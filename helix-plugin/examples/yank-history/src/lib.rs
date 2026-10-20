//! Example plugin: yank history (API 0.3 registers). Remembers what you
//! yank; `:yank-history/list` shows it in a picker, and the chosen entry
//! goes back into the `"` register for `p`. Like yanky.nvim.

use std::cell::RefCell;

use helix_plugin_api::{
    editor, register_plugin, CommandInfo, EventKind, Item, Picker, Plugin, Registration,
};

/// Entries kept.
const KEEP: usize = 50;

thread_local! {
    /// Newest first; each entry has one value per selection.
    static HISTORY: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
}

fn remember(values: Vec<String>) {
    if values.iter().all(|value| value.is_empty()) {
        return;
    }
    HISTORY.with_borrow_mut(|history| {
        history.retain(|entry| *entry != values);
        history.insert(0, values);
        history.truncate(KEEP);
    });
}

/// One line describing an entry.
fn summary(values: &[String]) -> String {
    let first = values.first().map(String::as_str).unwrap_or_default();
    let mut line: String = first.lines().next().unwrap_or_default().chars().take(80).collect();
    let lines = first.lines().count();
    if lines > 1 {
        line.push_str(&format!("  (+{} lines)", lines - 1));
    }
    if values.len() > 1 {
        line.push_str(&format!("  [{} selections]", values.len()));
    }
    line
}

struct YankHistory;

impl Plugin for YankHistory {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "list".into(),
                doc: "Pick an earlier yank to paste".into(),
            }],
            events: vec![EventKind::PostCommand],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        if name != "list" {
            return Err(format!("unknown command {name}"));
        }
        let history = HISTORY.with_borrow(Clone::clone);
        if history.is_empty() {
            editor::set_status("Nothing yanked yet");
            return Ok(());
        }
        let items = history
            .iter()
            .enumerate()
            .map(|(index, values)| Item::new(vec![summary(values)], index.to_string()))
            .collect();
        editor::pick(Picker::new("Yank history", items), move |choice| {
            let Some(index) = choice.items.first().and_then(|item| item.data.parse::<usize>().ok())
            else {
                return Ok(());
            };
            let values = history[index].clone();
            editor::set_register('"', &values)?;
            remember(values);
            editor::set_status("Put in the \" register: paste with p");
            Ok(())
        })?;
        Ok(())
    }

    fn on_post_command(command: &str) {
        // `yank`, `yank_joined`, ... and `delete_selection`, which yanks too.
        if command.starts_with("yank") || command == "delete_selection" || command == "change_selection" {
            match editor::register('"') {
                Ok(values) => remember(values),
                Err(err) => editor::log(&err),
            }
        }
    }
}

register_plugin!(YankHistory);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn keeps_recent_unique_entries() {
        remember(vec!["a".into()]);
        remember(vec!["b\nc".into()]);
        remember(vec!["a".into()]);
        HISTORY.with_borrow(|history| {
            assert_eq!(history.len(), 2);
            assert_eq!(history[0], ["a"]);
        });
        assert_eq!(summary(&["b\nc".into()]), "b  (+1 lines)");
        assert_eq!(summary(&["x".into(), "y".into()]), "x  [2 selections]");
    }
}
