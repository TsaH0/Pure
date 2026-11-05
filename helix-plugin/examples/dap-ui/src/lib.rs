//! Example plugin (API 0.5 `debug`): like nvim-dap-ui. While the program
//! being debugged is stopped (`:debug-start`, `space G`), a panel below
//! the views shows its stack and the innermost frame's local variables;
//! it goes when the program runs on or ends. Needs `allow-debug = true`
//! (asking for variables is a debugger request).

use helix_plugin_api::{
    debug, editor,
    panels::{self, Dock},
    register_plugin,
    sidebar::item,
    Plugin, Registration,
};
use serde_json::{json, Value};

const TITLE: &str = "Debug";

/// The stack, innermost first, as panel lines.
fn stack(frames: &[debug::Frame]) -> Vec<(String, &'static str, u32)> {
    let mut lines = vec![("Stack".to_string(), "markup.heading", 0)];
    lines.extend(frames.iter().map(|frame| {
        let place = frame
            .path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .map(|file| format!("  {file}:{}", frame.line + 1))
            .unwrap_or_default();
        (format!("{}{place}", frame.name), "function", 1)
    }));
    lines
}

/// A `variables` reply as panel lines.
fn variables(reply: &Value) -> Vec<(String, &'static str, u32)> {
    let mut lines = vec![("Locals".to_string(), "markup.heading", 0)];
    for variable in reply["variables"].as_array().into_iter().flatten() {
        let name = variable["name"].as_str().unwrap_or("?");
        let value = variable["value"].as_str().unwrap_or("");
        let kind = variable["type"].as_str().map(|t| format!(": {t}")).unwrap_or_default();
        lines.push((format!("{name}{kind} = {value}"), "variable", 1));
    }
    lines
}

fn show(lines: Vec<(String, &'static str, u32)>) -> Result<(), String> {
    let items = lines
        .into_iter()
        .map(|(text, scope, depth)| item(text, scope, depth))
        .collect();
    panels::show(&panels::panel(Dock::Bottom, TITLE, items, 10))
}

/// Show the stack now, then the variables when the debugger answers.
fn stopped() -> Result<(), String> {
    let Some(session) = debug::session()? else {
        return Ok(());
    };
    let frames = stack(&session.frames);
    show(frames.clone())?;
    let Some(frame) = session.frames.first() else {
        return Ok(());
    };
    let arguments = json!({ "frameId": frame.id }).to_string();
    debug::request("scopes", &arguments, move |reply| {
        let scopes: Value = serde_json::from_str(&reply?).map_err(|e| e.to_string())?;
        let Some(locals) = scopes["scopes"]
            .as_array()
            .and_then(|scopes| scopes.first())
            .and_then(|scope| scope["variablesReference"].as_u64())
        else {
            return Ok(());
        };
        let arguments = json!({ "variablesReference": locals }).to_string();
        debug::request("variables", &arguments, move |reply| {
            let reply: Value = serde_json::from_str(&reply?).map_err(|e| e.to_string())?;
            let mut lines = frames;
            lines.extend(variables(&reply));
            show(lines)
        })?;
        Ok(())
    })?;
    Ok(())
}

struct DapUi;

impl Plugin for DapUi {
    fn init(_config: String) -> Result<Registration, String> {
        debug::on(&["stopped", "continued", "terminated", "exited"], |event, _body| {
            let result = match event {
                "stopped" => stopped(),
                _ => {
                    panels::close(Dock::Bottom);
                    Ok(())
                }
            };
            if let Err(err) = &result {
                editor::log(err);
            }
            result
        })?;
        Ok(Registration {
            commands: vec![],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }
}

register_plugin!(DapUi);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn lists_frames_and_variables() {
        let frames = [debug::Frame {
            id: 1,
            name: "main".into(),
            path: Some("/src/main.c".into()),
            line: 2,
            column: 0,
        }];
        assert_eq!(stack(&frames)[1].0, "main  main.c:3");
        let reply = json!({"variables": [{"name": "x", "value": "41", "type": "int"}]});
        assert_eq!(variables(&reply)[1].0, "x: int = 41");
    }
}
