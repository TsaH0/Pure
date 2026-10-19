//! A direct-bindings fixture for the API 0.3 world, independent of the SDK.
wit_bindgen::generate!({ path: "../../../wit/v0.3", world: "helix-plugin" });
use helix::plugin::{host, types};
use std::cell::{Cell, RefCell};
thread_local! {
    static POPUPS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static CLOSED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static POPUP_ON_OPEN: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
fn record(event: String) {
    host::log(&format!("event:{event}"));
    EVENTS.with_borrow_mut(|events| events.push(event));
}
struct TestPlugin;
impl exports::helix::plugin::plugin::Guest for TestPlugin {
    fn init(config: String) -> Result<types::Registration, String> {
        POPUP_ON_OPEN.set(config.contains("popup-on-open"));
        Ok(types::Registration {
            commands: [
                "inspect",
                "open-at",
                "popup",
                "close-popup",
                "popup-close-now",
                "two-popups",
                "popup-events",
                "events",
                "stream",
                "stream-fail",
                "output-events",
                "kill",
            ]
            .into_iter()
            .map(|name| types::CommandInfo {
                name: name.into(),
                doc: name.into(),
            })
            .collect(),
            events: {
                let mut events = vec![types::EventKind::DocumentOpen];
                if config.contains("extended-events") {
                    events.extend([
                        types::EventKind::PostCommand,
                        types::EventKind::FocusGained,
                        types::EventKind::FocusLost,
                        types::EventKind::EditorIdle,
                    ]);
                }
                events
            },
        })
    }
    fn run_command(name: String, args: Vec<String>) -> Result<(), String> {
        match name.as_str() {
            "stream" | "stream-fail" => {
                let id = helix::plugin::process::spawn(
                    &args,
                    &helix::plugin::process::SpawnOptions {
                        cwd: None,
                        stdin: None,
                        timeout_ms: Some(5000),
                        stream: true,
                    },
                )?;
                if name == "stream-fail" {
                    return Err("failed after spawn".into());
                }
                host::set_status(&format!("stream-id:{id}"));
                Ok(())
            }
            "kill" => {
                let id = args
                    .first()
                    .ok_or("missing id")?
                    .parse::<u32>()
                    .map_err(|error| error.to_string())?;
                helix::plugin::process::kill(id);
                Ok(())
            }
            "output-events" => {
                OUTPUT.with_borrow(|output| host::set_status(&format!("output:{output:?}")));
                Ok(())
            }
            "events" => {
                EVENTS.with_borrow(|events| host::set_status(&events.join(",")));
                Ok(())
            }
            "popup" | "popup-close-now" => {
                let id = helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                    markdown: args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
                    centered: args.first().is_some_and(|arg| arg == "centered"),
                })?;
                POPUPS.with_borrow_mut(|popups| popups.push(id));
                host::set_status(&format!("popup:{id}"));
                if name == "popup-close-now" {
                    helix::plugin::ui::close_popup(id);
                }
                Ok(())
            }
            "two-popups" => {
                let mut ids = Vec::new();
                for text in ["first", "second"] {
                    let id = helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                        markdown: text.into(),
                        centered: false,
                    })?;
                    POPUPS.with_borrow_mut(|popups| popups.push(id));
                    ids.push(id);
                }
                host::set_status(&format!("popups:{},{}", ids[0], ids[1]));
                Ok(())
            }
            "close-popup" => {
                let id = args
                    .first()
                    .map(|arg| arg.parse::<u32>().map_err(|error| error.to_string()))
                    .transpose()?
                    .or_else(|| POPUPS.with_borrow(|popups| popups.last().copied()))
                    .ok_or("no popup")?;
                helix::plugin::ui::close_popup(id);
                Ok(())
            }
            "popup-events" => {
                CLOSED.with_borrow(|closed| host::set_status(&format!("closed:{closed:?}")));
                Ok(())
            }
            "inspect" => {
                let doc = host::current_document().ok_or("no document")?;
                let text = host::text(doc)?;
                host::apply_edits(
                    doc,
                    &[types::Edit {
                        from: 0,
                        to: 0,
                        text: "v3:".into(),
                    }],
                )?;
                host::set_status(&format!("0.3:{text}"));
                Ok(())
            }
            "open-at" => {
                let path = args.first().ok_or("missing path")?;
                let action = match args.get(1).map(String::as_str).unwrap_or("replace") {
                    "replace" => types::OpenAction::Replace,
                    "hsplit" => types::OpenAction::Hsplit,
                    "vsplit" => types::OpenAction::Vsplit,
                    _ => return Err("invalid action".into()),
                };
                let number = |index| {
                    args.get(index)
                        .map(|value: &String| {
                            value.parse::<u32>().map_err(|error| error.to_string())
                        })
                        .transpose()
                };
                host::open_file_at(path, action, number(2)?, number(3)?)
            }
            _ => Err("unknown command".into()),
        }
    }
    fn on_document_open(doc: u32) {
        if POPUP_ON_OPEN.get() {
            host::set_status(
                &helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                    markdown: "event".into(),
                    centered: false,
                })
                .unwrap_err(),
            );
        } else {
            host::set_status(&format!("v3-open:{doc}"));
        }
    }
    fn on_document_change(_doc: u32) {}
    fn on_mode_switch(_old: types::Mode, _new: types::Mode) {}
}
impl exports::helix::plugin::callbacks::Guest for TestPlugin {
    fn on_process_done(_id: u32, result: Result<helix::plugin::process::Output, String>) {
        let output = match result {
            Ok(output) => format!(
                "done:{:?}:{}:{}",
                output.exit_code, output.stdout, output.stderr
            ),
            Err(error) => format!("error:{error}"),
        };
        OUTPUT.with_borrow_mut(|events| events.push(output.clone()));
        host::set_status(&output);
    }
    fn on_picker_done(_id: u32, _result: helix::plugin::ui::PickerResult) {}
    fn on_picker_query(_id: u32, _generation: u64, _query: String) {}
    fn on_process_output(_id: u32, chunk: String) {
        let text = host::current_document()
            .and_then(|doc| host::text(doc).ok())
            .unwrap_or_default();
        let output = format!("chunk:{chunk}:snapshot:{text}");
        OUTPUT.with_borrow_mut(|events| events.push(output.clone()));
        host::set_status(&output);
    }
    fn on_popup_close(id: u32) {
        CLOSED.with_borrow_mut(|closed| closed.push(id));
        POPUPS.with_borrow_mut(|popups| popups.retain(|popup| *popup != id));
        host::set_status(&format!("popup-closed:{id}"));
    }
    fn on_prompt_done(_id: u32, _answer: Option<String>) {}
}
impl exports::helix::plugin::events::Guest for TestPlugin {
    fn on_document_save(_doc: u32) {}
    fn on_document_close(_doc: u32) {}
    fn on_selection_change(_doc: u32) {}
    fn on_post_command(command: String) {
        record(format!("command:{command}"));
    }
    fn on_focus_gained() {
        record("focus:gained".into());
    }
    fn on_focus_lost() {
        record("focus:lost".into());
    }
    fn on_editor_idle() {
        record("idle".into());
    }
}
export!(TestPlugin);
