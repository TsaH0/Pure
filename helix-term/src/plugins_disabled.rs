//! Stand-in for `plugins` when Pure is built without the `plugins` feature:
//! the same functions, doing nothing.

use std::path::PathBuf;

use helix_view::{DocumentId, Editor};

use crate::job::Jobs;

const DISABLED: &str = "Pure was built without plugin support (the `plugins` feature)";

pub fn plugin_dir() -> PathBuf {
    helix_loader::config_dir().join("plugins")
}

pub fn load(editor: &mut Editor) {
    editor.set_error(DISABLED);
}

pub fn load_in_background() {}

pub fn register_hooks() {}

pub fn document_saved(_doc: DocumentId) {}

pub fn focus_changed(_focused: bool) {}

pub fn editor_idle(_editor: &mut Editor) {}

pub fn directory_changed(_editor: &mut Editor) {}

pub fn count() -> usize {
    0
}

pub fn command_names() -> Vec<String> {
    Vec::new()
}

pub fn install_command(editor: &mut Editor, _source: String) {
    editor.set_error(DISABLED);
}

pub fn update_command(editor: &mut Editor, _names: Vec<String>) {
    editor.set_error(DISABLED);
}

pub fn remove_command(_compositor: &mut crate::compositor::Compositor, _name: String) {}

pub fn commands_with_docs() -> Vec<(String, String)> {
    Vec::new()
}

pub fn plugin_names() -> Vec<String> {
    Vec::new()
}

pub fn completion_menu_event(
    _editor: &Editor,
    _event: crate::ui::completion::menu_events::MenuEvent,
) {
}

pub fn pre_command(_editor: &mut Editor, _jobs: &mut Jobs, _command: &str) -> Result<(), String> {
    Ok(())
}

pub fn pre_save(
    _editor: &mut Editor,
    _jobs: &mut Jobs,
    _doc: helix_view::DocumentId,
) -> Result<(), String> {
    Ok(())
}

pub fn check_focus(_editor: &Editor) {}

pub fn reads_menu() -> bool {
    false
}

pub fn set_ui_layers(_layers: impl Iterator<Item = &'static str>) {}

pub fn set_pending_keys(_keys: &[helix_view::input::KeyEvent]) {}

pub fn shutdown(_editor: &mut Editor) {}

pub fn config_reloaded(_editor: &mut Editor) {}

pub fn doctor() -> String {
    "Plugins are not built into this Pure.".into()
}

pub fn set_trace(_plugin: Option<&str>, _on: bool) -> usize {
    0
}

pub fn key(
    _editor: &mut Editor,
    _jobs: &mut Jobs,
    _plugin: &str,
    _name: &str,
    _keys: &str,
    _count: Option<u32>,
) -> bool {
    false
}

pub fn run_command(
    _editor: &mut Editor,
    _jobs: &mut Jobs,
    _name: &str,
    _args: Vec<String>,
) -> anyhow::Result<()> {
    anyhow::bail!(DISABLED)
}

pub fn describe() -> String {
    DISABLED.to_string()
}

pub fn log_lines(_plugin: &str) -> anyhow::Result<Vec<String>> {
    anyhow::bail!(DISABLED)
}

pub fn write_buffer(
    _editor: &mut Editor,
    _jobs: &mut Jobs,
    _doc: DocumentId,
) -> anyhow::Result<()> {
    anyhow::bail!(DISABLED)
}

pub fn request_completions(
    _editor: &mut Editor,
) -> Vec<(u16, tokio::sync::oneshot::Receiver<Vec<PluginCompletion>>)> {
    Vec::new()
}

/// What a plugin completion source answers (never, without plugins).
pub struct PluginCompletion {
    pub label: String,
    pub insert: Option<String>,
    pub kind: Option<String>,
    pub documentation: Option<String>,
    pub replace_from: Option<usize>,
    pub snippet: bool,
}

/// Without plugins there is no directory handler: the file picker opens.
pub fn open_directory(_editor: &Editor, _dir: PathBuf) -> bool {
    false
}

pub fn programs_blocked() -> bool {
    false
}

pub fn block_programs(_blocked: bool) -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {}
    }
    Guard
}

pub fn starts_program(_command: &str) -> bool {
    false
}

pub fn pre_command_with(
    _editor: &mut Editor,
    _jobs: &mut Jobs,
    _command: &str,
    _register: Option<char>,
    _count: Option<std::num::NonZeroUsize>,
) -> Result<(), String> {
    Ok(())
}

pub fn lsp_notification(_server: &str, _method: &str, _params: &helix_lsp::jsonrpc::Params) {}
