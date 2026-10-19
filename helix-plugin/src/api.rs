//! Versioned component bindings and thin adapters over the shared host state.
use super::{types, DocumentId, Event, State};
use wasmtime::{Result, Store};

pub(crate) mod v0_1 {
    wasmtime::component::bindgen!({ path: "wit/v0.1", world: "helix-plugin" });
}
pub(crate) mod v0_2 {
    wasmtime::component::bindgen!({ path: "wit/v0.2", world: "helix-plugin" });
}

impl types::Host for State {}
impl v0_2::helix::plugin::types::Host for State {}
impl v0_1::helix::plugin::host::Host for State {
    fn current_document(&mut self) -> Option<DocumentId> {
        State::current_document(self)
    }
    fn documents(&mut self) -> Vec<DocumentId> {
        State::documents(self)
    }
    fn current_mode(&mut self) -> Mode {
        State::current_mode(self)
    }
    fn document_path(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        State::document_path(self, doc)
    }
    fn document_language(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        State::document_language(self, doc)
    }
    fn text(&mut self, doc: DocumentId) -> Result<String, String> {
        State::text(self, doc)
    }
    fn text_range(&mut self, doc: DocumentId, from: u32, to: u32) -> Result<String, String> {
        State::text_range(self, doc, from, to)
    }
    fn selections(&mut self, doc: DocumentId) -> Result<Vec<types::Range>, String> {
        State::selections(self, doc)
    }
    fn apply_edits(&mut self, doc: DocumentId, edits: Vec<types::Edit>) -> Result<(), String> {
        State::apply_edits(self, doc, edits)
    }
    fn set_selections(&mut self, doc: DocumentId, ranges: Vec<types::Range>) -> Result<(), String> {
        State::set_selections(self, doc, ranges)
    }
    fn open_file(&mut self, path: String) -> Result<(), String> {
        State::open_file(self, path)
    }
    fn run_command(&mut self, command: String) -> Result<(), String> {
        State::run_command(self, command)
    }
    fn set_status(&mut self, message: String) {
        State::set_status(self, message)
    }
    fn set_error(&mut self, message: String) {
        State::set_error(self, message)
    }
    fn log(&mut self, message: String) {
        State::log(self, message)
    }
}
impl v0_2::helix::plugin::host::Host for State {
    fn current_document(&mut self) -> Option<DocumentId> {
        State::current_document(self)
    }
    fn documents(&mut self) -> Vec<DocumentId> {
        State::documents(self)
    }
    fn current_mode(&mut self) -> v0_2::helix::plugin::types::Mode {
        to_v2_mode(State::current_mode(self))
    }
    fn document_path(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        State::document_path(self, doc)
    }
    fn document_language(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        State::document_language(self, doc)
    }
    fn text(&mut self, doc: DocumentId) -> Result<String, String> {
        State::text(self, doc)
    }
    fn text_range(&mut self, doc: DocumentId, from: u32, to: u32) -> Result<String, String> {
        State::text_range(self, doc, from, to)
    }
    fn selections(
        &mut self,
        doc: DocumentId,
    ) -> Result<Vec<v0_2::helix::plugin::types::Range>, String> {
        State::selections(self, doc).map(|ranges| {
            ranges
                .into_iter()
                .map(|r| v0_2::helix::plugin::types::Range {
                    anchor: r.anchor,
                    head: r.head,
                })
                .collect()
        })
    }
    fn apply_edits(
        &mut self,
        doc: DocumentId,
        edits: Vec<v0_2::helix::plugin::types::Edit>,
    ) -> Result<(), String> {
        State::apply_edits(
            self,
            doc,
            edits
                .into_iter()
                .map(|e| types::Edit {
                    from: e.from,
                    to: e.to,
                    text: e.text,
                })
                .collect(),
        )
    }
    fn set_selections(
        &mut self,
        doc: DocumentId,
        ranges: Vec<v0_2::helix::plugin::types::Range>,
    ) -> Result<(), String> {
        State::set_selections(
            self,
            doc,
            ranges
                .into_iter()
                .map(|r| types::Range {
                    anchor: r.anchor,
                    head: r.head,
                })
                .collect(),
        )
    }
    fn open_file(&mut self, path: String) -> Result<(), String> {
        State::open_file(self, path)
    }
    fn run_command(&mut self, command: String) -> Result<(), String> {
        State::run_command(self, command)
    }
    fn set_status(&mut self, message: String) {
        State::set_status(self, message)
    }
    fn set_error(&mut self, message: String) {
        State::set_error(self, message)
    }
    fn log(&mut self, message: String) {
        State::log(self, message)
    }
}

use super::Mode;
use v0_2::helix::plugin::{process, ui};

impl process::Host for State {
    fn spawn(
        &mut self,
        argv: Vec<String>,
        options: process::SpawnOptions,
    ) -> std::result::Result<u32, String> {
        State::spawn(self, argv, options.cwd, options.stdin, options.timeout_ms)
    }
    fn run_interactive(
        &mut self,
        argv: Vec<String>,
        options: process::InteractiveOptions,
    ) -> std::result::Result<u32, String> {
        State::run_interactive(
            self,
            argv,
            options.cwd,
            options.chooser_file,
            options.wait_for_key,
        )
    }
    fn kill(&mut self, id: u32) {
        State::kill(self, id)
    }
}
impl ui::Host for State {
    fn show_picker(&mut self, picker: ui::Picker) -> std::result::Result<u32, String> {
        State::show_picker(self, picker)
    }
    fn push_items(&mut self, id: u32, items: Vec<ui::Item>) -> std::result::Result<(), String> {
        State::push_items(self, id, items)
    }
}

fn to_v2_mode(mode: Mode) -> v0_2::helix::plugin::types::Mode {
    match mode {
        Mode::Normal => v0_2::helix::plugin::types::Mode::Normal,
        Mode::Insert => v0_2::helix::plugin::types::Mode::Insert,
        Mode::Select => v0_2::helix::plugin::types::Mode::Select,
    }
}

pub(crate) enum Instance {
    V0_1(v0_1::HelixPlugin),
    V0_2(v0_2::HelixPlugin),
}
impl Instance {
    pub fn call_init(
        &self,
        store: &mut Store<State>,
        config: &str,
    ) -> Result<std::result::Result<types::Registration, String>> {
        match self {
            Self::V0_1(instance) => instance.helix_plugin_plugin().call_init(store, config),
            Self::V0_2(instance) => {
                instance
                    .helix_plugin_plugin()
                    .call_init(store, config)
                    .map(|r| {
                        r.map(|r| types::Registration {
                            commands: r
                                .commands
                                .into_iter()
                                .map(|c| types::CommandInfo {
                                    name: c.name,
                                    doc: c.doc,
                                })
                                .collect(),
                            events: r
                                .events
                                .into_iter()
                                .map(|e| match e {
                                    v0_2::helix::plugin::types::EventKind::DocumentOpen => {
                                        types::EventKind::DocumentOpen
                                    }
                                    v0_2::helix::plugin::types::EventKind::DocumentChange => {
                                        types::EventKind::DocumentChange
                                    }
                                    v0_2::helix::plugin::types::EventKind::ModeSwitch => {
                                        types::EventKind::ModeSwitch
                                    }
                                })
                                .collect(),
                        })
                    })
            }
        }
    }
    pub fn call_run_command(
        &self,
        store: &mut Store<State>,
        name: &str,
        args: &[String],
    ) -> Result<std::result::Result<(), String>> {
        match self {
            Self::V0_1(instance) => instance
                .helix_plugin_plugin()
                .call_run_command(store, name, args),
            Self::V0_2(instance) => instance
                .helix_plugin_plugin()
                .call_run_command(store, name, args),
        }
    }
    pub fn call_callback(
        &self,
        store: &mut Store<State>,
        callback: &super::Callback,
    ) -> Result<()> {
        let Self::V0_2(instance) = self else {
            anyhow::bail!("callbacks require API 0.2")
        };
        let callbacks = instance.helix_plugin_callbacks();
        match callback {
            super::Callback::ProcessDone { id, result } => callbacks.call_on_process_done(
                store,
                *id,
                result.as_ref().map_err(|err| err.as_str()),
            ),
            super::Callback::PickerDone { id, result } => {
                callbacks.call_on_picker_done(store, *id, result)
            }
            super::Callback::PromptDone { .. } => {
                anyhow::bail!("API 0.2 does not export prompt callbacks")
            }
        }
    }

    pub fn call_event(&self, store: &mut Store<State>, event: Event) -> Result<()> {
        match self {
            Self::V0_1(instance) => {
                let guest = instance.helix_plugin_plugin();
                match event {
                    Event::DocumentOpen(doc) => guest.call_on_document_open(store, doc),
                    Event::DocumentChange(doc) => guest.call_on_document_change(store, doc),
                    Event::ModeSwitch { old, new } => guest.call_on_mode_switch(store, old, new),
                }
            }
            Self::V0_2(instance) => {
                let guest = instance.helix_plugin_plugin();
                match event {
                    Event::DocumentOpen(doc) => guest.call_on_document_open(store, doc),
                    Event::DocumentChange(doc) => guest.call_on_document_change(store, doc),
                    Event::ModeSwitch { old, new } => {
                        guest.call_on_mode_switch(store, to_v2_mode(old), to_v2_mode(new))
                    }
                }
            }
        }
    }
}
