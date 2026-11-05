//! Example plugin (API 0.5 `mouse`): mouse actions, like a GUI editor's.
//! Ctrl+click goes to the definition of what was clicked (through the
//! language server), alt+click to its references, and a click on the
//! gutter folds the block there, or unfolds it.

use helix_plugin_api::{commands, editor, folds, mouse, register_plugin, Plugin, Registration};
use serde_json::Value;

/// What to do with a click: `None` leaves it to the editor.
fn action(event: &Value) -> Option<&'static str> {
    if event["button"] != "left" {
        return None;
    }
    let modifiers = event["modifiers"].as_array()?;
    let has = |name: &str| modifiers.iter().any(|m| m == name);
    if event["gutter"] == true {
        return event["line"].as_u64().map(|_| "fold");
    }
    match (has("ctrl"), has("alt")) {
        (true, false) => Some("goto_definition"),
        (false, true) => Some("goto_reference"),
        _ => None,
    }
}

fn click(payload: &str) -> Result<bool, String> {
    let event: Value = serde_json::from_str(payload).map_err(|err| err.to_string())?;
    let Some(action) = action(&event) else {
        return Ok(false);
    };
    if action == "fold" {
        let doc = event["doc"].as_u64().ok_or("no document")? as u32;
        let line = event["line"].as_u64().ok_or("no line")? as u32;
        let folded = folds::list(doc)?
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&line));
        // The editor's `fold` finds the block at the cursor: put it there.
        let command = if folded { "unfold" } else { "fold" };
        commands::batch(
            &[
                commands::step("goto_line", &[], Some(line + 1)),
                commands::step(command, &[], None),
            ],
            |results| {
                if let Some(Err(err)) = results.into_iter().find(Result::is_err) {
                    editor::set_status(&err);
                }
                Ok(())
            },
        )?;
        return Ok(true);
    }
    // Move the cursor to the click first, then ask the language server.
    let pos = event["pos"].as_u64().ok_or("not on text")? as u32;
    let doc = event["doc"].as_u64().ok_or("no document")? as u32;
    editor::set_selections(doc, &[helix_plugin_api::Range { anchor: pos, head: pos + 1 }])?;
    commands::execute(action, &[] as &[&str], None, |_| Ok(()))?;
    Ok(true)
}

struct Click;

impl Plugin for Click {
    fn init(_config: String) -> Result<Registration, String> {
        mouse::on(&["down"], |_kind, payload| click(payload))?;
        Ok(Registration {
            commands: vec![],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }
}

register_plugin!(Click);

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    #[test]
    fn chooses_by_place_and_modifiers() {
        let click = |gutter: bool, modifiers: &[&str]| {
            json!({"button": "left", "gutter": gutter, "line": 3, "pos": 10, "modifiers": modifiers})
        };
        assert_eq!(action(&click(false, &["ctrl"])), Some("goto_definition"));
        assert_eq!(action(&click(false, &["alt"])), Some("goto_reference"));
        assert_eq!(action(&click(false, &[])), None);
        assert_eq!(action(&click(true, &[])), Some("fold"));
        assert_eq!(action(&json!({"button": "right", "gutter": true, "modifiers": []})), None);
    }
}
