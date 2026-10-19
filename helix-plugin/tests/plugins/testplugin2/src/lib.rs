wit_bindgen::generate!({ path: "../../../wit/v0.2", world: "helix-plugin" });

use helix::plugin::{host, types::*};

struct TestPlugin;
impl exports::helix::plugin::plugin::Guest for TestPlugin {
    fn init(config: String) -> Result<Registration, String> {
        host::set_status(&format!("0.2 init:{config}"));
        Ok(Registration {
            commands: vec![CommandInfo { name: "inspect".into(), doc: "Exercise shared host functions".into() }],
            events: vec![EventKind::DocumentOpen, EventKind::ModeSwitch],
        })
    }
    fn run_command(_name: String, _args: Vec<String>) -> Result<(), String> {
        let doc = host::current_document().ok_or("no document")?;
        let text = host::text(doc)?;
        let ranges = host::selections(doc)?;
        host::set_selections(doc, &ranges)?;
        host::apply_edits(doc, &[Edit { from: 0, to: 0, text: "v2:".into() }])?;
        host::set_status(&format!("{text}:{:?}", host::current_mode()));
        Ok(())
    }
    fn on_document_open(doc: DocumentId) { host::set_status(&format!("open:{doc}")); }
    fn on_document_change(_doc: DocumentId) {}
    fn on_mode_switch(old: Mode, new: Mode) { host::set_status(&format!("{old:?}->{new:?}")); }
}
impl exports::helix::plugin::callbacks::Guest for TestPlugin {
    fn on_process_done(id: u32, result: Result<helix::plugin::process::Output, String>) {
        host::set_status(&format!("process:{id}:{result:?}"));
    }
    fn on_picker_done(id: u32, result: helix::plugin::ui::PickerResult) {
        host::set_status(&format!("picker:{id}:{result:?}"));
    }
}
export!(TestPlugin);
