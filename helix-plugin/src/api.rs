//! Versioned component bindings and thin adapters over the shared host state.
use super::{types, CommandInfo, DocumentId, Event, EventKind, State};
use wasmtime::{Result, Store};

pub(crate) mod v0_1 {
    wasmtime::component::bindgen!({ path: "wit/v0.1", world: "helix-plugin", exports: { default: async } });
}
pub(crate) mod v0_2 {
    wasmtime::component::bindgen!({ path: "wit/v0.2", world: "helix-plugin", exports: { default: async } });
}

pub(crate) mod v0_3 {
    wasmtime::component::bindgen!({ path: "wit/v0.3", world: "helix-plugin", exports: { default: async } });
}
pub(crate) mod v0_4 {
    wasmtime::component::bindgen!({ path: "wit/v0.4", world: "helix-plugin", exports: { default: async } });
}
mod api03;
mod api04;
mod world;

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
    fn workspace_root(&mut self) -> Option<String> {
        State::workspace_root(self)
    }
    fn cwd(&mut self) -> String {
        State::cwd(self)
    }
    fn config_dir(&mut self) -> String {
        State::config_dir(self)
    }
    fn data_dir(&mut self) -> String {
        State::data_dir(self)
    }
    fn document_modified(&mut self, doc: DocumentId) -> Result<bool, String> {
        State::document_modified(self, doc)
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
        State::spawn(
            self,
            argv,
            options.cwd,
            options.stdin,
            options.timeout_ms,
            false,
        )
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
        State::show_picker(self, picker, false)
    }
    fn push_items(&mut self, id: u32, items: Vec<ui::Item>) -> std::result::Result<(), String> {
        State::push_items(self, id, None, items)
    }
    fn set_items(&mut self, id: u32, items: Vec<ui::Item>) -> std::result::Result<(), String> {
        State::set_items(self, id, None, items)
    }
    fn show_popup(&mut self, markdown: String) -> std::result::Result<(), String> {
        State::show_popup(self, markdown)
    }
    fn show_prompt(&mut self, prompt: ui::Prompt) -> std::result::Result<u32, String> {
        State::show_prompt(self, prompt)
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
    V0_3(Box<v0_3::HelixPlugin>),
    V0_4(Box<v0_4::HelixPlugin>),
}
impl Instance {
    /// `init`: the plugin's commands and the events it subscribes to.
    #[allow(clippy::type_complexity)]
    pub async fn call_init(
        &self,
        store: &mut Store<State>,
        config: &str,
    ) -> Result<std::result::Result<(Vec<CommandInfo>, Vec<EventKind>), String>> {
        match self {
            Self::V0_3(instance) => api03::init(instance, store, config).await,
            Self::V0_4(instance) => api04::init(instance, store, config).await,
            Self::V0_1(instance) => instance
                .helix_plugin_plugin()
                .call_init(store, config)
                .await
                .map(|r| {
                    r.map(|r| {
                        let events = r
                            .events
                            .into_iter()
                            .map(|e| match e {
                                types::EventKind::DocumentOpen => EventKind::DocumentOpen,
                                types::EventKind::DocumentChange => EventKind::DocumentChange,
                                types::EventKind::ModeSwitch => EventKind::ModeSwitch,
                            })
                            .collect();
                        (r.commands, events)
                    })
                }),
            Self::V0_2(instance) => instance
                .helix_plugin_plugin()
                .call_init(store, config)
                .await
                .map(|r| {
                    r.map(|r| {
                        use v0_2::helix::plugin::types::EventKind as E;
                        let commands = r
                            .commands
                            .into_iter()
                            .map(|c| CommandInfo {
                                name: c.name,
                                doc: c.doc,
                            })
                            .collect();
                        let events = r
                            .events
                            .into_iter()
                            .map(|e| match e {
                                E::DocumentOpen => EventKind::DocumentOpen,
                                E::DocumentChange => EventKind::DocumentChange,
                                E::ModeSwitch => EventKind::ModeSwitch,
                                E::DocumentSave => EventKind::DocumentSave,
                                E::DocumentClose => EventKind::DocumentClose,
                                E::SelectionChange => EventKind::SelectionChange,
                            })
                            .collect();
                        (commands, events)
                    })
                }),
        }
    }
    pub async fn call_run_command(
        &self,
        store: &mut Store<State>,
        name: &str,
        args: &[String],
    ) -> Result<std::result::Result<(), String>> {
        match self {
            Self::V0_3(instance) => {
                instance
                    .helix_plugin_plugin()
                    .call_run_command(store, name, args)
                    .await
            }
            Self::V0_4(instance) => {
                instance
                    .helix_plugin_plugin()
                    .call_run_command(store, name, args)
                    .await
            }
            Self::V0_1(instance) => {
                instance
                    .helix_plugin_plugin()
                    .call_run_command(store, name, args)
                    .await
            }
            Self::V0_2(instance) => {
                instance
                    .helix_plugin_plugin()
                    .call_run_command(store, name, args)
                    .await
            }
        }
    }
    pub async fn call_callback(
        &self,
        store: &mut Store<State>,
        callback: &super::Callback,
    ) -> Result<()> {
        if let Self::V0_3(instance) = self {
            return api03::callback(instance, store, callback).await;
        }
        if let Self::V0_4(instance) = self {
            return api04::callback(instance, store, callback).await;
        }
        let Self::V0_2(instance) = self else {
            anyhow::bail!("callbacks require API 0.2")
        };
        let callbacks = instance.helix_plugin_callbacks();
        match callback {
            super::Callback::ProcessOutput { .. } => anyhow::bail!("streaming requires API 0.3"),
            super::Callback::PopupClosed { .. } => anyhow::bail!("popup callbacks require API 0.3"),
            super::Callback::Timer { .. } => anyhow::bail!("timers require API 0.3"),
            super::Callback::FilesDone { .. } => anyhow::bail!("file results require API 0.4"),
            super::Callback::BackgroundDone { .. } => {
                anyhow::bail!("background tasks require API 0.3")
            }
            super::Callback::ProcessDone { id, result } => {
                callbacks
                    .call_on_process_done(store, *id, result.as_ref().map_err(|err| err.as_str()))
                    .await
            }
            super::Callback::PickerDone { id, result } => {
                callbacks.call_on_picker_done(store, *id, result).await
            }
            super::Callback::PromptDone { id, result } => {
                callbacks
                    .call_on_prompt_done(store, *id, result.as_deref())
                    .await
            }
            super::Callback::PickerQuery { id, query, .. } => {
                callbacks.call_on_picker_query(store, *id, query).await
            }
        }
    }

    /// The `background.run` export (API 0.3).
    pub async fn call_background(
        &self,
        store: &mut Store<State>,
        task: &str,
        input: &str,
    ) -> Result<std::result::Result<String, String>> {
        match self {
            Self::V0_3(instance) => {
                instance
                    .helix_plugin_background()
                    .call_run(store, task, input)
                    .await
            }
            Self::V0_4(instance) => {
                instance
                    .helix_plugin_background()
                    .call_run(store, task, input)
                    .await
            }
            _ => anyhow::bail!("background tasks require API 0.3"),
        }
    }

    pub async fn call_event(&self, store: &mut Store<State>, event: Event) -> Result<()> {
        match self {
            Self::V0_3(instance) => api03::event(instance, store, event).await,
            Self::V0_4(instance) => api04::event(instance, store, event).await,
            Self::V0_1(instance) => {
                let guest = instance.helix_plugin_plugin();
                match event {
                    Event::DocumentOpen(doc) => guest.call_on_document_open(store, doc).await,
                    Event::DocumentChange(doc) => guest.call_on_document_change(store, doc).await,
                    Event::ModeSwitch { old, new } => {
                        guest.call_on_mode_switch(store, old, new).await
                    }
                    // 0.1 plugins can't subscribe to these.
                    Event::DocumentSave(_)
                    | Event::DocumentClose(_)
                    | Event::SelectionChange(_)
                    | Event::PostCommand(_)
                    | Event::FocusGained
                    | Event::FocusLost
                    | Event::EditorIdle
                    | Event::Completion { .. }
                    | Event::BufferWrite(_) => Ok(()),
                }
            }
            Self::V0_2(instance) => {
                let guest = instance.helix_plugin_plugin();
                match event {
                    Event::DocumentOpen(doc) => guest.call_on_document_open(store, doc).await,
                    Event::DocumentChange(doc) => guest.call_on_document_change(store, doc).await,
                    Event::ModeSwitch { old, new } => {
                        guest
                            .call_on_mode_switch(store, to_v2_mode(old), to_v2_mode(new))
                            .await
                    }
                    Event::DocumentSave(doc) => {
                        instance
                            .helix_plugin_events()
                            .call_on_document_save(store, doc)
                            .await
                    }
                    Event::DocumentClose(doc) => {
                        instance
                            .helix_plugin_events()
                            .call_on_document_close(store, doc)
                            .await
                    }
                    Event::SelectionChange(doc) => {
                        instance
                            .helix_plugin_events()
                            .call_on_selection_change(store, doc)
                            .await
                    }
                    Event::PostCommand(_)
                    | Event::FocusGained
                    | Event::FocusLost
                    | Event::EditorIdle
                    | Event::Completion { .. }
                    | Event::BufferWrite(_) => Ok(()),
                }
            }
        }
    }
}
