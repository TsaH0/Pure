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
