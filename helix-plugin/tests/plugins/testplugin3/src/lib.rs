//! A direct-bindings fixture for the API 0.3 world, independent of the SDK.
wit_bindgen::generate!({ path: "../../../wit/v0.3", world: "helix-plugin" });
use helix::plugin::{host, types};
struct TestPlugin;
impl exports::helix::plugin::plugin::Guest for TestPlugin {
    fn init(_config: String) -> Result<types::Registration, String> {
        Ok(types::Registration {
            commands: ["inspect", "open-at"]
                .into_iter()
                .map(|name| types::CommandInfo {
                    name: name.into(),
                    doc: name.into(),
                })
                .collect(),
            events: vec![types::EventKind::DocumentOpen],
        })
    }
    fn run_command(name: String, args: Vec<String>) -> Result<(), String> {
        match name.as_str() {
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
        host::set_status(&format!("v3-open:{doc}"));
    }
    fn on_document_change(_doc: u32) {}
    fn on_mode_switch(_old: types::Mode, _new: types::Mode) {}
}
impl exports::helix::plugin::callbacks::Guest for TestPlugin {
    fn on_process_done(_id: u32, _result: Result<helix::plugin::process::Output, String>) {}
    fn on_picker_done(_id: u32, _result: helix::plugin::ui::PickerResult) {}
    fn on_picker_query(_id: u32, _generation: u64, _query: String) {}
    fn on_process_output(_id: u32, _chunk: String) {}
    fn on_popup_close(_id: u32) {}
    fn on_prompt_done(_id: u32, _answer: Option<String>) {}
}
impl exports::helix::plugin::events::Guest for TestPlugin {
    fn on_document_save(_doc: u32) {}
    fn on_document_close(_doc: u32) {}
    fn on_selection_change(_doc: u32) {}
    fn on_post_command(_command: String) {}
    fn on_focus_gained() {}
    fn on_focus_lost() {}
    fn on_editor_idle() {}
}
export!(TestPlugin);
