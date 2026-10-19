//! Glue between the editor and the WebAssembly plugin host (`helix-plugin`).
//!
//! Plugins live in `<config dir>/plugins/<name>/` (a `plugin.toml` manifest and
//! the component). `<config dir>/plugins.toml` holds per-plugin settings:
//!
//! ```toml
//! [text-tools]
//! enabled = false      # skip this plugin
//!
//! [my-plugin]
//! some-option = 3      # everything but `enabled` and `allow-process` is
//!                      # passed to the plugin
//! allow-process = true # let it run programs (it also needs the `process`
//!                      # permission); or a list: ["rg", "git"]
//! allow-files = "read" # let it read ("write": and write) the workspace
//!                      # (it also needs the `filesystem` permission)
//! ```
//!
//! Every plugin can use its own directory, `<data dir>/plugins/<name>/`.

mod picker;
mod process;
mod prompt;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helix_event::{register_hook, runtime_local};
use helix_plugin::{
    Action, Callback, DocumentSnapshot, Event, EventKind, FileGrant, PluginHost, PluginSettings,
    ProcessGrant, Snapshot,
};
use helix_view::document::Mode;
use helix_view::events::{
    DocumentDidChange, DocumentDidClose, DocumentDidOpen, SelectionDidChange,
};
use helix_view::{DocumentId, Editor, ViewId};
use parking_lot::Mutex;

use crate::commands::{self, MappableCommand};
use crate::compositor::Compositor;
use crate::events::OnModeSwitch;
use crate::job::{self, Jobs};

/// Events are delivered after this delay, so a burst of changes (typing)
/// results in one `on-document-change` per document.
const EVENT_DELAY: Duration = Duration::from_millis(50);

runtime_local! {
    static HOST: Mutex<Option<PluginHost>> = Mutex::new(None);
    static PENDING: Mutex<Pending> = Mutex::new(Pending { events: Vec::new(), scheduled: false });
    /// Set while plugin actions are applied, so a plugin's own edits don't
    /// produce change events (which could loop forever).
    static APPLYING: AtomicBool = AtomicBool::new(false);
}

struct Pending {
    events: Vec<Event>,
    scheduled: bool,
}

pub fn plugin_dir() -> PathBuf {
    helix_loader::config_dir().join("plugins")
}

pub fn settings_file() -> PathBuf {
    helix_loader::config_dir().join("plugins.toml")
}

/// Load the user's plugins. Problems are reported in the status line and the
/// log; they never stop the editor from starting.
pub fn load(editor: &mut Editor) {
    let settings = match std::fs::read_to_string(settings_file()) {
        Ok(source) => match toml::from_str::<toml::Table>(&source) {
            Ok(settings) => settings,
            Err(err) => {
                editor.set_error(format!("Invalid {}: {err}", settings_file().display()));
                toml::Table::new()
            }
        },
        Err(_) => toml::Table::new(),
    };
    load_from(editor, &plugin_dir(), &settings);
}

/// Load the plugins in `dir` with `settings` (the contents of `plugins.toml`),
/// replacing any loaded plugins.
pub fn load_from(editor: &mut Editor, dir: &Path, settings: &toml::Table) {
    // The old plugins' programs and pickers are of no use to the new ones.
    process::kill_all();
    if !dir.is_dir() {
        *HOST.lock() = None;
        return;
    }
    let mut host = match PluginHost::new(Some(helix_loader::cache_dir().join("plugins"))) {
        Ok(host) => host,
        Err(err) => {
            log::error!("failed to start the plugin host: {err:#}");
            editor.set_error(format!("Failed to start the plugin host: {err}"));
            return;
        }
    };
    host.set_dirs(
        Some(helix_loader::find_workspace().0),
        Some(helix_loader::data_dir().join("plugins")),
    );
    let mut setting_errors = Vec::new();
    let (actions, mut errors) = host.load_all(dir, |name| {
        plugin_settings(settings, name).map(|settings| {
            settings.unwrap_or_else(|err| {
                setting_errors.push(err);
                PluginSettings::default()
            })
        })
    });
    errors.extend(setting_errors);
    *HOST.lock() = Some(host);

    apply(editor, None, actions);
    for err in &errors {
        log::error!("{err:#}");
    }
    if let Some(err) = errors.first() {
        let more = match errors.len() {
            1 => String::new(),
            n => format!(" (and {} more, see the log)", n - 1),
        };
        editor.set_error(format!("{err:#}{more}"));
    }
}

/// Like [`load_from`] with the settings given as TOML source.
pub fn load_with_settings(editor: &mut Editor, dir: &Path, settings: &str) -> anyhow::Result<()> {
    let settings: toml::Table = toml::from_str(settings)?;
    load_from(editor, dir, &settings);
    Ok(())
}

/// The settings for plugin `name`, or `None` if it is disabled.
fn plugin_settings(settings: &toml::Table, name: &str) -> Option<anyhow::Result<PluginSettings>> {
    let mut table = match settings.get(name) {
        Some(toml::Value::Table(table)) => table.clone(),
        _ => toml::Table::new(),
    };
    if let Some(enabled) = table.remove("enabled") {
        if enabled.as_bool() == Some(false) {
            return None;
        }
    }
    table.remove("allow-process");
    table.remove("allow-files");
    let config = serde_json::to_string(&table).unwrap_or_else(|_| "{}".into());
    Some((|| {
        Ok(PluginSettings {
            config,
            process: process_grant(settings, name)?,
            files: file_grant(settings, name)?,
        })
    })())
}

/// `allow-files` for plugin `name`: `"read"`, `"write"` or `false`.
fn file_grant(settings: &toml::Table, name: &str) -> anyhow::Result<FileGrant> {
    match settings
        .get(name)
        .and_then(|table| table.get("allow-files"))
    {
        None | Some(toml::Value::Boolean(false)) => Ok(FileGrant::None),
        Some(toml::Value::String(grant)) if grant == "read" => Ok(FileGrant::Read),
        Some(toml::Value::String(grant)) if grant == "write" => Ok(FileGrant::Write),
        Some(_) => anyhow::bail!("[{name}] allow-files: expected \"read\", \"write\" or false"),
    }
}

/// `allow-process` for plugin `name`: `true`, `false` or a list of programs.
fn process_grant(settings: &toml::Table, name: &str) -> anyhow::Result<ProcessGrant> {
    let Some(value) = settings
        .get(name)
        .and_then(|table| table.get("allow-process"))
    else {
        return Ok(ProcessGrant::None);
    };
    match value {
        toml::Value::Boolean(true) => Ok(ProcessGrant::Any),
        toml::Value::Boolean(false) => Ok(ProcessGrant::None),
        toml::Value::Array(programs) => programs
            .iter()
            .map(|program| program.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(ProcessGrant::Only)
            .ok_or_else(|| anyhow::anyhow!("[{name}] allow-process: expected a list of strings")),
        _ => anyhow::bail!("[{name}] allow-process: expected true, false or a list of programs"),
    }
}

/// Run `f` with the plugin host, if plugins are loaded.
pub fn with_host<R>(f: impl FnOnce(&mut PluginHost) -> R) -> Option<R> {
    HOST.lock().as_mut().map(f)
}

/// `plugin/command` names of all commands, for completion.
pub fn command_names() -> Vec<String> {
    with_host(|host| {
        host.commands()
            .into_iter()
            .map(|(plugin, command)| format!("{plugin}/{}", command.name))
            .collect()
    })
    .unwrap_or_default()
}

/// Names of the loaded plugins, for completion.
pub fn plugin_names() -> Vec<String> {
    with_host(|host| {
        host.plugins()
            .iter()
            .map(|plugin| plugin.name().to_owned())
            .collect()
    })
    .unwrap_or_default()
}

/// What `plugin` wrote with `log`, oldest first.
pub fn log_lines(plugin: &str) -> anyhow::Result<Vec<String>> {
    with_host(|host| host.log_lines(plugin))
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("no plugin named '{plugin}'"))
}

/// Run a plugin command given as `plugin/command`.
pub fn run_command(
    editor: &mut Editor,
    jobs: &mut Jobs,
    name: &str,
    args: Vec<String>,
) -> anyhow::Result<()> {
    let (plugin, command) = name
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("expected <plugin>/<command>, got '{name}'"))?;
    let snapshot = snapshot(editor);
    let actions = with_host(|host| host.run_command(plugin, command, args, snapshot))
        .ok_or_else(|| anyhow::anyhow!("no plugins are loaded"))??;
    apply(editor, Some(jobs), actions);
    Ok(())
}

fn plugin_mode(mode: Mode) -> helix_plugin::Mode {
    match mode {
        Mode::Normal => helix_plugin::Mode::Normal,
        Mode::Insert => helix_plugin::Mode::Insert,
        Mode::Select => helix_plugin::Mode::Select,
    }
}

fn plugin_doc_id(id: DocumentId) -> u32 {
    // `DocumentId` keeps its number private but displays it.
    id.to_string().parse().unwrap_or(0)
}

fn editor_doc_id(editor: &Editor, id: u32) -> Option<DocumentId> {
    editor
        .documents
        .keys()
        .copied()
        .find(|doc| plugin_doc_id(*doc) == id)
}

/// The view a plugin's changes to `doc` go to: the focused view if it shows
/// the document, otherwise the first view showing it.
fn view_for(editor: &Editor, doc: DocumentId) -> Option<ViewId> {
    let focused = editor.tree.get(editor.tree.focus);
    if focused.doc == doc {
        return Some(focused.id);
    }
    editor
        .tree
        .views()
        .find(|(view, _)| view.doc == doc)
        .map(|(view, _)| view.id)
}

/// What plugins can see of the editor. Cheap: ropes are reference counted.
pub fn snapshot(editor: &Editor) -> Snapshot {
    let documents = editor
        .documents()
        .map(|doc| {
            let selection = view_for(editor, doc.id()).map(|view| doc.selection(view).clone());
            (
                plugin_doc_id(doc.id()),
                DocumentSnapshot {
                    text: doc.text().clone(),
                    path: doc.path().map(Path::to_path_buf),
                    language: doc.language_name().map(str::to_owned),
                    selection,
                    modified: doc.is_modified(),
                },
            )
        })
        .collect();
    Snapshot {
        documents,
        current: Some(plugin_doc_id(editor.tree.get(editor.tree.focus).doc)),
        mode: plugin_mode(editor.mode()),
        workspace: Some(helix_loader::find_workspace().0),
        cwd: helix_stdx::env::current_working_dir(),
        config_dir: helix_loader::config_dir(),
    }
}

/// Apply the actions of a plugin call. `jobs` is needed to run commands; it is
/// only available when a plugin command runs (not for events).
fn apply(editor: &mut Editor, mut jobs: Option<&mut Jobs>, actions: Vec<Action>) {
    APPLYING.store(true, Ordering::Relaxed);
    let mut edited: HashMap<DocumentId, ViewId> = HashMap::new();
    for action in actions {
        match action {
            Action::Edit { doc, transaction } => {
                let Some(doc_id) = editor_doc_id(editor, doc) else {
                    continue; // closed in the meantime
                };
                let Some(view_id) = view_for(editor, doc_id) else {
                    editor.set_error("Plugins can only edit documents shown in a window");
                    continue;
                };
                let doc = editor.documents.get_mut(&doc_id).unwrap();
                if doc.apply(&transaction, view_id) {
                    edited.insert(doc_id, view_id);
                }
            }
            Action::SetSelection { doc, selection } => {
                let Some(doc_id) = editor_doc_id(editor, doc) else {
                    continue;
                };
                let Some(view_id) = view_for(editor, doc_id) else {
                    continue;
                };
                let doc = editor.documents.get_mut(&doc_id).unwrap();
                let selection = selection.ensure_invariants(doc.text().slice(..));
                doc.set_selection(view_id, selection);
            }
            Action::OpenFile(path) => {
                let path = helix_stdx::path::canonicalize(path);
                if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
                    editor.set_error(format!("Failed to open {}: {err}", path.display()));
                }
            }
            Action::RunCommand(command) => match jobs.as_deref_mut() {
                Some(jobs) => run_editor_command(editor, jobs, &command),
                None => editor.set_error("Plugins can only run commands from their commands"),
            },
            Action::Status(message) => editor.set_status(message),
            Action::Error(message) => editor.set_error(message),
            Action::Spawn {
                plugin,
                id,
                argv,
                cwd,
                stdin,
                timeout,
            } => process::spawn(plugin, id, argv, cwd, stdin, timeout),
            Action::RunInteractive {
                plugin,
                id,
                argv,
                cwd,
                chooser_file,
                wait_for_key,
            } => match process::interactive(
                plugin.clone(),
                id,
                argv,
                cwd,
                chooser_file,
                wait_for_key,
            ) {
                Ok(interactive) => later(move |_, compositor| {
                    compositor.interactive = Some(interactive);
                }),
                Err(err) => deliver_later(
                    plugin,
                    Callback::ProcessDone {
                        id,
                        result: Err(err),
                    },
                ),
            },
            Action::Kill { plugin, id } => process::kill(&plugin, id),
            Action::ShowPicker { plugin, id, picker } => {
                later(move |_, compositor| picker::show(compositor, plugin, id, picker))
            }
            Action::PushItems { plugin, id, items } => {
                later(move |_, compositor| picker::push_items(compositor, &plugin, id, items))
            }
            Action::SetItems { plugin, id, items } => {
                later(move |_, compositor| picker::set_items(compositor, &plugin, id, items))
            }
            Action::Popup(markdown) => later(move |editor, compositor| {
                let contents = crate::ui::Markdown::new(markdown, editor.syn_loader.clone());
                let popup = crate::ui::Popup::new("plugin-popup", contents).auto_close(true);
                compositor.replace_or_push("plugin-popup", popup);
            }),
            Action::ShowPrompt { plugin, id, prompt } => later(move |editor, compositor| {
                prompt::show(editor, compositor, plugin, id, prompt)
            }),
        }
    }
    // All edits of one call are one undo step.
    let scrolloff = editor.config().scrolloff;
    for (doc_id, view_id) in edited {
        if !editor.tree.contains(view_id) {
            continue;
        }
        let view = editor.tree.get_mut(view_id);
        let doc = editor.documents.get_mut(&doc_id).unwrap();
        doc.append_changes_to_history(view);
        view.ensure_cursor_in_view(doc, scrolloff);
    }
    APPLYING.store(false, Ordering::Relaxed);
}

/// Run `f` once the current event is handled (with the compositor, which
/// plugin calls don't have).
fn later(f: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    tokio::spawn(job::dispatch(f));
}

/// Deliver a result (a program's output, a picker choice) to `plugin` and
/// apply what it does.
fn deliver(editor: &mut Editor, jobs: &mut Jobs, plugin: &str, callback: Callback) {
    let snapshot = snapshot(editor);
    match with_host(|host| host.callback(plugin, callback, &snapshot)) {
        Some(Ok(actions)) => apply(editor, Some(jobs), actions),
        Some(Err(err)) => editor.set_error(err.to_string()),
        None => {}
    }
}

/// [`deliver`] from outside the editor's event loop.
fn deliver_later(plugin: String, callback: Callback) {
    tokio::spawn(job::dispatch_callback(job::Callback::EditorCompositorJobs(
        Box::new(move |editor, _, jobs| deliver(editor, jobs, &plugin, callback)),
    )));
}

fn run_editor_command(editor: &mut Editor, jobs: &mut Jobs, command: &str) {
    let command = match MappableCommand::from_str(command) {
        Ok(command) => command,
        Err(err) => {
            editor.set_error(format!("Plugin command failed: {err}"));
            return;
        }
    };
    let mut cx = commands::Context {
        register: None,
        count: None,
        editor,
        callback: Vec::new(),
        on_next_key_callback: None,
        jobs,
    };
    command.execute(&mut cx);
    if !cx.callback.is_empty() || cx.on_next_key_callback.is_some() {
        log::warn!("plugins can't run commands that open a UI: {command}");
    }
}

/// Register the hooks that turn editor events into plugin events.
pub fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        queue(Event::DocumentOpen(plugin_doc_id(event.doc)));
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if !event.ghost_transaction && !APPLYING.load(Ordering::Relaxed) {
            queue(Event::DocumentChange(plugin_doc_id(event.doc.id())));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        queue(Event::DocumentClose(plugin_doc_id(event.doc.id())));
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if !APPLYING.load(Ordering::Relaxed) {
            queue(Event::SelectionChange(plugin_doc_id(event.doc.id())));
        }
        Ok(())
    });
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        queue(Event::ModeSwitch {
            old: plugin_mode(event.old_mode),
            new: plugin_mode(event.new_mode),
        });
        Ok(())
    });
}

/// A document was written to disk.
pub fn document_saved(doc: DocumentId) {
    queue(Event::DocumentSave(plugin_doc_id(doc)));
}

fn queue(event: Event) {
    if !with_host(|host| host.wants(event.kind())).unwrap_or(false) {
        return;
    }
    let mut pending = PENDING.lock();
    // One change (or selection change) event per document is enough.
    if matches!(event, Event::DocumentChange(_) | Event::SelectionChange(_))
        && pending.events.contains(&event)
    {
        return;
    }
    pending.events.push(event);
    if !pending.scheduled {
        pending.scheduled = true;
        tokio::spawn(async {
            tokio::time::sleep(EVENT_DELAY).await;
            job::dispatch(|editor, _| deliver_events(editor)).await;
        });
    }
}

/// Send the queued events to the plugins and apply what they do.
pub fn deliver_events(editor: &mut Editor) {
    let events = {
        let mut pending = PENDING.lock();
        pending.scheduled = false;
        std::mem::take(&mut pending.events)
    };
    for event in events {
        if let Event::DocumentChange(doc)
        | Event::DocumentOpen(doc)
        | Event::DocumentSave(doc)
        | Event::SelectionChange(doc) = event
        {
            if editor_doc_id(editor, doc).is_none() {
                continue; // closed in the meantime
            }
        }
        let snapshot = snapshot(editor);
        let Some(results) = with_host(|host| host.dispatch(event, &snapshot)) else {
            return;
        };
        for (_plugin, result) in results {
            match result {
                Ok(actions) => apply(editor, None, actions),
                Err(err) => editor.set_error(err.to_string()),
            }
        }
    }
}

/// Markdown description of the loaded plugins, for `:plugin-list`.
pub fn describe() -> String {
    let Some(info) = with_host(|host| host.info()) else {
        return format!("No plugins loaded from `{}`.", plugin_dir().display());
    };
    if info.is_empty() {
        return format!("No plugins in `{}`.", plugin_dir().display());
    }
    let mut out = String::new();
    for plugin in info {
        out.push_str(&format!("**{}** {}", plugin.name, plugin.version));
        if let Some(reason) = &plugin.disabled {
            out.push_str(&format!(" — *disabled: {reason}*"));
        }
        out.push('\n');
        if !plugin.description.is_empty() {
            out.push_str(&format!("{}\n", plugin.description));
        }
        let permissions: Vec<_> = plugin.permissions.iter().map(|p| p.as_str()).collect();
        if !permissions.is_empty() {
            out.push_str(&format!("permissions: {}\n", permissions.join(", ")));
        }
        if plugin
            .permissions
            .contains(&helix_plugin::Permission::Process)
        {
            out.push_str(&format!("process: {}\n", plugin.process));
        }
        if plugin
            .permissions
            .contains(&helix_plugin::Permission::Filesystem)
        {
            out.push_str(&format!("files: {}\n", plugin.files));
        }
        for command in &plugin.commands {
            out.push_str(&format!(
                "- `{}/{}` {}\n",
                plugin.name, command.name, command.doc
            ));
        }
        out.push('\n');
    }
    out
}

/// Whether any plugin subscribes to events of `kind`.
pub fn wants(kind: EventKind) -> bool {
    with_host(|host| host.wants(kind)).unwrap_or(false)
}
