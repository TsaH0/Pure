//! API 0.3 adapters. Older worlds retain their generated types unchanged.
use super::{types, v0_3, CommandInfo, DocumentId, Event, EventKind, Mode, State};
use crate::{Callback, OpenAction};
use v0_3::helix::plugin::{decorations, process, ui};
use wasmtime::{Result, Store};

impl v0_3::helix::plugin::types::Host for State {}
impl v0_3::helix::plugin::host::Host for State {
    fn current_document(&mut self) -> Option<DocumentId> {
        State::current_document(self)
    }
    fn documents(&mut self) -> Vec<DocumentId> {
        State::documents(self)
    }
    fn current_mode(&mut self) -> v0_3::helix::plugin::types::Mode {
        to_v3_mode(State::current_mode(self))
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
    ) -> Result<Vec<v0_3::helix::plugin::types::Range>, String> {
        State::selections(self, doc).map(|ranges| {
            ranges
                .into_iter()
                .map(|r| v0_3::helix::plugin::types::Range {
                    anchor: r.anchor,
                    head: r.head,
                })
                .collect()
        })
    }
    fn apply_edits(
        &mut self,
        doc: DocumentId,
        edits: Vec<v0_3::helix::plugin::types::Edit>,
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
        ranges: Vec<v0_3::helix::plugin::types::Range>,
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
    fn open_file_at(
        &mut self,
        path: String,
        action: v0_3::helix::plugin::types::OpenAction,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<(), String> {
        State::open_file_at(self, path, from_action(action), line, column)
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

fn to_v3_mode(mode: Mode) -> v0_3::helix::plugin::types::Mode {
    use v0_3::helix::plugin::types::Mode as M;
    match mode {
        Mode::Normal => M::Normal,
        Mode::Insert => M::Insert,
        Mode::Select => M::Select,
    }
}
fn from_action(action: v0_3::helix::plugin::types::OpenAction) -> OpenAction {
    use v0_3::helix::plugin::types::OpenAction as A;
    match action {
        A::Replace => OpenAction::Replace,
        A::Hsplit => OpenAction::Hsplit,
        A::Vsplit => OpenAction::Vsplit,
    }
}
fn to_action(action: OpenAction) -> v0_3::helix::plugin::types::OpenAction {
    use v0_3::helix::plugin::types::OpenAction as A;
    match action {
        OpenAction::Replace => A::Replace,
        OpenAction::Hsplit => A::Hsplit,
        OpenAction::Vsplit => A::Vsplit,
    }
}
fn item(item: ui::Item) -> crate::PickerItem {
    crate::PickerItem {
        columns: item.columns,
        data: item.data,
        preview: item.preview.map(|preview| match preview {
            ui::Preview::File(file) => crate::PickerPreview::File(file),
            ui::Preview::Text(text) => crate::PickerPreview::Text(text),
        }),
    }
}
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
            options.stream,
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
impl decorations::Host for State {
    fn set_highlights(
        &mut self,
        doc: DocumentId,
        layer: String,
        highlights: Vec<decorations::Highlight>,
    ) -> std::result::Result<(), String> {
        let highlights = highlights
            .into_iter()
            .map(|h| (h.from, h.to, h.scope))
            .collect();
        State::set_highlights(self, doc, layer, highlights)
    }
    fn set_annotations(
        &mut self,
        doc: DocumentId,
        layer: String,
        annotations: Vec<decorations::Annotation>,
    ) -> std::result::Result<(), String> {
        let annotations = annotations
            .into_iter()
            .map(|a| (a.position, a.text, a.scope))
            .collect();
        State::set_annotations(self, doc, layer, annotations)
    }
    fn set_signs(
        &mut self,
        doc: DocumentId,
        layer: String,
        signs: Vec<decorations::Sign>,
    ) -> std::result::Result<(), String> {
        let signs = signs
            .into_iter()
            .map(|s| (s.line, s.text, s.scope))
            .collect();
        State::set_signs(self, doc, layer, signs)
    }
    fn set_statusline(&mut self, segment: String, text: String) -> std::result::Result<(), String> {
        State::set_statusline(self, segment, text)
    }
}

impl ui::Host for State {
    fn show_picker(&mut self, picker: ui::Picker) -> std::result::Result<u32, String> {
        let live = picker.live;
        State::show_picker(
            self,
            crate::PickerSpec {
                title: picker.title,
                columns: picker.columns,
                items: picker.items.into_iter().map(item).collect(),
                initial: picker.initial,
                live,
            },
            live,
        )
    }
    fn push_items(
        &mut self,
        id: u32,
        generation: Option<u64>,
        items: Vec<ui::Item>,
    ) -> std::result::Result<(), String> {
        State::push_items(self, id, generation, items.into_iter().map(item).collect())
    }
    fn set_items(
        &mut self,
        id: u32,
        generation: Option<u64>,
        items: Vec<ui::Item>,
    ) -> std::result::Result<(), String> {
        State::set_items(self, id, generation, items.into_iter().map(item).collect())
    }
    fn show_popup(&mut self, popup: ui::Popup) -> std::result::Result<u32, String> {
        State::show_popup_with_id(self, popup.markdown, popup.centered)
    }
    fn close_popup(&mut self, id: u32) {
        State::close_popup(self, id)
    }
    fn show_prompt(&mut self, prompt: ui::Prompt) -> std::result::Result<u32, String> {
        State::show_prompt(
            self,
            crate::PromptSpec {
                message: prompt.message,
                initial: prompt.initial,
                completions: prompt.completions,
            },
        )
    }
}

#[allow(clippy::type_complexity)]
pub(super) async fn init(
    instance: &v0_3::HelixPlugin,
    store: &mut Store<State>,
    config: &str,
) -> Result<std::result::Result<(Vec<CommandInfo>, Vec<EventKind>), String>> {
    Ok(instance
        .helix_plugin_plugin()
        .call_init(store, config)
        .await?
        .map(|registration| {
            use v0_3::helix::plugin::types::EventKind as E;
            let commands = registration
                .commands
                .into_iter()
                .map(|command| CommandInfo {
                    name: command.name,
                    doc: command.doc,
                })
                .collect();
            let events = registration
                .events
                .into_iter()
                .map(|event| match event {
                    E::DocumentOpen => EventKind::DocumentOpen,
                    E::DocumentChange => EventKind::DocumentChange,
                    E::ModeSwitch => EventKind::ModeSwitch,
                    E::DocumentSave => EventKind::DocumentSave,
                    E::DocumentClose => EventKind::DocumentClose,
                    E::SelectionChange => EventKind::SelectionChange,
                    E::PostCommand => EventKind::PostCommand,
                    E::FocusGained => EventKind::FocusGained,
                    E::FocusLost => EventKind::FocusLost,
                    E::EditorIdle => EventKind::EditorIdle,
                })
                .collect();
            (commands, events)
        }))
}

pub(super) async fn callback(
    instance: &v0_3::HelixPlugin,
    store: &mut Store<State>,
    callback: &Callback,
) -> Result<()> {
    let callbacks = instance.helix_plugin_callbacks();
    match callback {
        Callback::ProcessOutput { id, chunk } => {
            callbacks.call_on_process_output(store, *id, chunk).await
        }
        Callback::PopupClosed { id } => callbacks.call_on_popup_close(store, *id).await,
        Callback::ProcessDone { id, result } => {
            let result = result.as_ref().map(|output| process::Output {
                exit_code: output.exit_code,
                stdout: output.stdout.clone(),
                stderr: output.stderr.clone(),
            });
            callbacks
                .call_on_process_done(store, *id, result.as_ref().map_err(|error| error.as_str()))
                .await
        }
        Callback::PickerDone { id, result } => {
            let result = match result {
                crate::PickerResult::Accepted((indices, action)) => {
                    ui::PickerResult::Accepted((indices.clone(), to_action(*action)))
                }
                crate::PickerResult::Cancelled => ui::PickerResult::Cancelled,
            };
            callbacks.call_on_picker_done(store, *id, &result).await
        }
        Callback::PickerQuery {
            id,
            query,
            generation,
        } => {
            callbacks
                .call_on_picker_query(store, *id, generation.unwrap_or_default(), query)
                .await
        }
        Callback::PromptDone { id, result } => {
            callbacks
                .call_on_prompt_done(store, *id, result.as_deref())
                .await
        }
    }
}

pub(super) async fn event(
    instance: &v0_3::HelixPlugin,
    store: &mut Store<State>,
    event: Event,
) -> Result<()> {
    let guest = instance.helix_plugin_plugin();
    match event {
        Event::PostCommand(command) => {
            instance
                .helix_plugin_events()
                .call_on_post_command(store, &command)
                .await
        }
        Event::FocusGained => {
            instance
                .helix_plugin_events()
                .call_on_focus_gained(store)
                .await
        }
        Event::FocusLost => {
            instance
                .helix_plugin_events()
                .call_on_focus_lost(store)
                .await
        }
        Event::EditorIdle => {
            instance
                .helix_plugin_events()
                .call_on_editor_idle(store)
                .await
        }
        Event::DocumentOpen(doc) => guest.call_on_document_open(store, doc).await,
        Event::DocumentChange(doc) => guest.call_on_document_change(store, doc).await,
        Event::ModeSwitch { old, new } => {
            guest
                .call_on_mode_switch(store, to_v3_mode(old), to_v3_mode(new))
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
    }
}
