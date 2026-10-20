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
mod popup;
mod process;
mod prompt;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
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
use crate::events::{OnModeSwitch, PostCommand};
use crate::job::{self, Jobs};

/// Events are delivered after this delay, so a burst of changes (typing)
/// results in one `on-document-change` per document.
const EVENT_DELAY: Duration = Duration::from_millis(50);

runtime_local! {
    static HOST: Mutex<Option<PluginHost>> = Mutex::new(None);
    static PENDING: Mutex<Pending> = Mutex::new(Pending { events: Vec::new(), scheduled: false });
    /// Set while plugin actions are applied, so a plugin's own edits don't
    /// produce change events (which could loop forever).
    static APPLYING: AtomicUsize = AtomicUsize::new(0);
    /// Plugin commands running inside each other (a plugin command running
    /// `:plugin-run`), to stop runaway recursion.
    static DEPTH: AtomicUsize = AtomicUsize::new(0);
    /// The events some plugin subscribed to (bits of `event_bit`), so editor
    /// hooks cost nothing when no plugin listens.
    static WANTED: AtomicU32 = AtomicU32::new(0);
    /// The workspace of the working directory, cached: `(cwd, workspace)`.
    static WORKSPACE: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);
    /// Counts loads, so a slow background load can't replace a newer one.
    static GENERATION: AtomicUsize = AtomicUsize::new(0);
    /// The plugin directory and settings loaded last, to reload them.
    static LOADED: Mutex<Option<(PathBuf, toml::Table)>> = Mutex::new(None);
}

fn event_bit(kind: EventKind) -> u32 {
    1 << match kind {
        EventKind::DocumentOpen => 0,
        EventKind::DocumentChange => 1,
        EventKind::ModeSwitch => 2,
        EventKind::DocumentSave => 3,
        EventKind::DocumentClose => 4,
        EventKind::SelectionChange => 5,
        EventKind::PostCommand => 6,
        EventKind::FocusGained => 7,
        EventKind::FocusLost => 8,
        EventKind::EditorIdle => 9,
    }
}

const EVENT_KINDS: [EventKind; 10] = [
    EventKind::DocumentOpen,
    EventKind::DocumentChange,
    EventKind::ModeSwitch,
    EventKind::DocumentSave,
    EventKind::DocumentClose,
    EventKind::SelectionChange,
    EventKind::PostCommand,
    EventKind::FocusGained,
    EventKind::FocusLost,
    EventKind::EditorIdle,
];

/// The workspace root for the current working directory.
fn workspace() -> PathBuf {
    let cwd = helix_stdx::env::current_working_dir();
    let mut cache = WORKSPACE.lock();
    match &*cache {
        Some((cached_cwd, workspace)) if *cached_cwd == cwd => workspace.clone(),
        _ => {
            let workspace = helix_loader::find_workspace_in(&cwd).0;
            *cache = Some((cwd, workspace.clone()));
            workspace
        }
    }
}

struct Pending {
    events: Vec<Event>,
    scheduled: bool,
}

/// The directory plugins with `allow-files` may use: the workspace, but only
/// a real project (with a `.git`, `.helix`, ... marker), never the home
/// directory, `/`, or anything containing Pure's own configuration or data
/// (a plugin could otherwise grant itself permissions in `plugins.toml`).
fn file_workspace() -> Option<PathBuf> {
    let cwd = helix_stdx::env::current_working_dir();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    file_workspace_in(
        &cwd,
        home.as_deref(),
        &helix_loader::config_dir(),
        &helix_loader::data_dir(),
    )
}

fn file_workspace_in(
    cwd: &Path,
    home: Option<&Path>,
    config: &Path,
    data: &Path,
) -> Option<PathBuf> {
    let (workspace, is_fallback) = helix_loader::find_workspace_in(cwd);
    let too_broad = is_fallback
        || workspace.parent().is_none()
        || home == Some(workspace.as_path())
        || config.starts_with(&workspace)
        || data.starts_with(&workspace);
    if too_broad {
        log::info!(
            "plugins get no file access to {}: not a project directory",
            workspace.display()
        );
        return None;
    }
    Some(workspace)
}

pub fn plugin_dir() -> PathBuf {
    helix_loader::config_dir().join("plugins")
}

pub fn settings_file() -> PathBuf {
    helix_loader::config_dir().join("plugins.toml")
}

/// The user's `plugins.toml`, and an error if it is invalid.
fn read_settings() -> (toml::Table, Option<anyhow::Error>) {
    match std::fs::read_to_string(settings_file()) {
        Ok(source) => match toml::from_str::<toml::Table>(&source) {
            Ok(settings) => (settings, None),
            Err(err) => (
                toml::Table::new(),
                Some(anyhow::anyhow!(
                    "Invalid {}: {err}",
                    settings_file().display()
                )),
            ),
        },
        Err(_) => (toml::Table::new(), None),
    }
}

/// Load the user's plugins now (`:plugin-reload`). Problems are reported in
/// the status line and the log.
pub fn load(editor: &mut Editor) {
    let (settings, error) = read_settings();
    if let Some(err) = error {
        editor.set_error(err.to_string());
    }
    load_from(editor, &plugin_dir(), &settings);
}

/// Load the user's plugins without delaying startup: compiling and starting
/// them happens on another thread, then they are installed and see the
/// documents opened in the meantime. Without plugins this costs nothing.
pub fn load_in_background() {
    let dir = plugin_dir();
    if !has_plugins(&dir) {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    tokio::spawn(async move {
        let (settings, error) = read_settings();
        let built = tokio::task::spawn_blocking(move || {
            let (host, actions, mut errors) = build_host(&dir, &settings);
            errors.extend(error);
            (host, actions, errors, (dir, settings))
        })
        .await;
        let Ok((host, actions, errors, loaded)) = built else {
            return;
        };
        job::dispatch(move |editor, _| {
            install_background(editor, generation, host, actions, errors, loaded);
        })
        .await;
    });
}

/// Complete a background load only if it is still the newest request.
fn install_background(
    editor: &mut Editor,
    generation: usize,
    host: Option<PluginHost>,
    actions: Vec<Action>,
    errors: Vec<anyhow::Error>,
    loaded: (PathBuf, toml::Table),
) {
    if GENERATION.load(Ordering::Relaxed) != generation {
        return;
    }
    *LOADED.lock() = Some(loaded);
    install(editor, host, actions, errors);
    let open: Vec<_> = editor.documents().map(|doc| doc.id()).collect();
    for doc in open {
        queue(Event::DocumentOpen(plugin_doc_id(doc)));
    }
}

/// Whether `dir` has any plugin (a subdirectory with a `plugin.toml`).
fn has_plugins(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.path().join("plugin.toml").is_file())
    })
}

/// Start a plugin host with the plugins in `dir`; `None` if there are none.
fn build_host(
    dir: &Path,
    settings: &toml::Table,
) -> (Option<PluginHost>, Vec<Action>, Vec<anyhow::Error>) {
    if !has_plugins(dir) {
        return (None, Vec::new(), Vec::new());
    }
    let mut host = match PluginHost::new(Some(helix_loader::cache_dir().join("plugins"))) {
        Ok(host) => host,
        Err(err) => {
            return (
                None,
                Vec::new(),
                vec![err.context("failed to start the plugin host")],
            )
        }
    };
    host.set_dirs(
        file_workspace(),
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
    (Some(host), actions, errors)
}

/// Make `host` the editor's plugins and report what went wrong.
fn install(
    editor: &mut Editor,
    host: Option<PluginHost>,
    actions: Vec<Action>,
    errors: Vec<anyhow::Error>,
) {
    let wanted = host.as_ref().map_or(0, |host| {
        EVENT_KINDS
            .into_iter()
            .filter(|kind| host.wants(*kind))
            .map(event_bit)
            .fold(0, |mask, bit| mask | bit)
    });
    WANTED.store(wanted, Ordering::Relaxed);
    *HOST.lock() = host;

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

/// Load the plugins in `dir` with `settings` (the contents of `plugins.toml`),
/// replacing any loaded plugins.
pub fn load_from(editor: &mut Editor, dir: &Path, settings: &toml::Table) {
    // The old plugins' programs are of no use to the new ones.
    process::kill_all();
    GENERATION.fetch_add(1, Ordering::Relaxed);
    *HOST.lock() = None;
    *LOADED.lock() = Some((dir.to_path_buf(), settings.clone()));
    let (host, actions, errors) = build_host(dir, settings);
    install(editor, host, actions, errors);
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
        // `name = false` disables it too.
        Some(toml::Value::Boolean(false)) => return None,
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

/// The working directory changed (`:cd`). Plugins with access to the
/// workspace keep the directory they were started with (WASI preopens are
/// fixed when a plugin starts), so reload them if the workspace changed.
pub fn directory_changed(editor: &mut Editor) {
    let stale = with_host(|host| {
        host.workspace() != file_workspace().as_deref()
            && host.info().iter().any(|plugin| {
                plugin.files != FileGrant::None
                    && plugin
                        .permissions
                        .contains(&helix_plugin::Permission::Filesystem)
            })
    })
    .unwrap_or(false);
    let loaded = LOADED.lock().clone();
    if let (true, Some((dir, settings))) = (stale, loaded) {
        load_from(editor, &dir, &settings);
    }
}

/// How many plugins are loaded.
pub fn count() -> usize {
    with_host(|host| host.plugins().len()).unwrap_or(0)
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
    // A plugin command may run other plugin commands, but not endlessly.
    const MAX_DEPTH: usize = 4;
    if DEPTH.load(Ordering::Relaxed) >= MAX_DEPTH {
        anyhow::bail!("plugin commands nested too deeply (running {name})");
    }
    let snapshot = snapshot(editor);
    let result = with_host(|host| host.run_command(plugin, command, args, snapshot))
        .ok_or_else(|| anyhow::anyhow!("no plugins are loaded"))?;
    let actions = match result {
        Ok(actions) => actions,
        Err(error) => {
            refresh_wanted();
            return Err(error);
        }
    };
    DEPTH.fetch_add(1, Ordering::Relaxed);
    apply(editor, Some(jobs), actions);
    DEPTH.fetch_sub(1, Ordering::Relaxed);
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
        workspace: Some(workspace()),
        cwd: helix_stdx::env::current_working_dir(),
        config_dir: helix_loader::config_dir(),
    }
}

/// Apply the actions of a plugin call. `jobs` is needed to run commands; it is
/// only available when a plugin command runs (not for events).
type UiJob = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;

fn apply(editor: &mut Editor, mut jobs: Option<&mut Jobs>, actions: Vec<Action>) {
    APPLYING.fetch_add(1, Ordering::Relaxed);
    let mut edited: HashMap<DocumentId, ViewId> = HashMap::new();
    // UI changes need the compositor: run them later, together, in order.
    let mut ui: Vec<UiJob> = Vec::new();
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
                // Refused if the text changed since the plugin read it.
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
                // The text may have changed since the plugin read it.
                let len = doc.text().len_chars();
                if selection.ranges().iter().any(|range| range.to() > len) {
                    continue;
                }
                let selection = selection.ensure_invariants(doc.text().slice(..));
                doc.set_selection(view_id, selection);
            }
            Action::OpenFile(path) => {
                let path = helix_stdx::path::canonicalize(path);
                if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
                    editor.set_error(format!("Failed to open {}: {err}", path.display()));
                }
            }
            Action::OpenFileAt {
                path,
                action,
                line,
                column,
            } => {
                if let Err(error) = open_file_at(editor, &path, action, line, column) {
                    editor.set_error(format!("Failed to open {}: {error}", path.display()));
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
                stream,
            } => ui.push(Box::new(move |_, _| {
                if with_host(|host| {
                    host.has_request(&plugin, id, helix_plugin::PendingKind::Process)
                })
                .unwrap_or(false)
                {
                    process::spawn(plugin, id, argv, cwd, stdin, timeout, stream);
                }
            })),
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
                Ok(interactive) => ui.push(Box::new(move |_, compositor| {
                    if interactive.can_run() {
                        compositor.interactive = Some(interactive);
                    }
                })),
                Err(err) => deliver_later(
                    plugin,
                    Callback::ProcessDone {
                        id,
                        result: Err(err),
                    },
                ),
            },
            Action::Kill { plugin, id } => {
                ui.push(Box::new(move |_, _| process::kill(&plugin, id)))
            }
            Action::ShowPicker {
                plugin,
                id,
                picker,
                generation,
            } => ui.push(Box::new(move |_, compositor| {
                picker::show(compositor, plugin, id, picker, generation)
            })),
            Action::PushItems {
                plugin,
                id,
                items,
                generation,
            } => ui.push(Box::new(move |_, compositor| {
                picker::push_items(compositor, &plugin, id, generation, items)
            })),
            Action::SetItems {
                plugin,
                id,
                items,
                generation,
            } => {
                // Now: the plugin's list already changed.
                let revision = picker::set_items_requested(&plugin, id);
                ui.push(Box::new(move |_, compositor| {
                    picker::set_items(compositor, &plugin, id, generation, revision, items)
                }))
            }
            Action::Popup {
                markdown,
                centered,
                owner,
            } => ui.push(Box::new(move |editor, compositor| {
                popup::show(editor, compositor, markdown, centered, owner);
            })),
            Action::ClosePopup { plugin, id } => ui.push(Box::new(move |_, compositor| {
                popup::close(compositor, &plugin, id)
            })),
            Action::ShowPrompt { plugin, id, prompt } => {
                ui.push(Box::new(move |editor, compositor| {
                    prompt::show(editor, compositor, plugin, id, prompt)
                }))
            }
        }
    }
    // All edits of one call are one undo step.
    let scrolloff = editor.config().scrolloff;
    for (doc_id, view_id) in edited {
        // A command the plugin ran may have closed the document or shown
        // another one in the view.
        if !editor.tree.contains(view_id) || editor.tree.get(view_id).doc != doc_id {
            continue;
        }
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            continue;
        };
        let view = editor.tree.get_mut(view_id);
        doc.append_changes_to_history(view);
        view.ensure_cursor_in_view(doc, scrolloff);
    }
    if !ui.is_empty() {
        later(move |editor, compositor| {
            for job in ui {
                job(editor, compositor);
            }
        });
    }
    APPLYING.fetch_sub(1, Ordering::Relaxed);
}

fn open_file_at(
    editor: &mut Editor,
    path: &Path,
    action: helix_plugin::OpenAction,
    line: Option<u32>,
    column: Option<u32>,
) -> anyhow::Result<()> {
    let action = match action {
        helix_plugin::OpenAction::Replace => helix_view::editor::Action::Replace,
        helix_plugin::OpenAction::Hsplit => helix_view::editor::Action::HorizontalSplit,
        helix_plugin::OpenAction::Vsplit => helix_view::editor::Action::VerticalSplit,
    };
    let id = editor.open(&helix_stdx::path::canonicalize(path), action)?;
    let scrolloff = editor.config().scrolloff;
    let view = editor.tree.get_mut(editor.tree.focus);
    let doc = editor.documents.get_mut(&id).unwrap();
    let row = (line.unwrap_or(1).saturating_sub(1) as usize).min(doc.text().len_lines() - 1);
    let end = helix_core::line_ending::line_end_char_index(&doc.text().slice(..), row);
    let pos = doc
        .text()
        .line_to_char(row)
        .saturating_add(column.unwrap_or(1).saturating_sub(1) as usize)
        .min(end);
    let selection = helix_core::Selection::point(pos).ensure_invariants(doc.text().slice(..));
    doc.set_selection(view.id, selection);
    view.ensure_cursor_in_view(doc, scrolloff);
    Ok(())
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
        Some(Err(err)) => {
            refresh_wanted();
            editor.set_error(err.to_string());
        }
        None => {}
    }
}

/// [`deliver`] from outside the editor's event loop.
fn deliver_later(plugin: String, callback: Callback) {
    tokio::spawn(deliver_async(plugin, callback));
}

/// Await the bounded job queue, preserving stream batches before completion.
async fn deliver_async(plugin: String, callback: Callback) {
    job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
        move |editor, _, jobs| deliver(editor, jobs, &plugin, callback),
    )))
    .await;
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
    register_hook!(move |event: &mut PostCommand<'_, '_>| {
        // Do not allocate a command name or lock anything unless requested.
        if wants(EventKind::PostCommand) && APPLYING.load(Ordering::Relaxed) == 0 {
            queue(Event::PostCommand(event.command.name().to_owned()));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        queue(Event::DocumentOpen(plugin_doc_id(event.doc)));
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if !event.ghost_transaction && APPLYING.load(Ordering::Relaxed) == 0 {
            queue(Event::DocumentChange(plugin_doc_id(event.doc.id())));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        queue(Event::DocumentClose(plugin_doc_id(event.doc.id())));
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if APPLYING.load(Ordering::Relaxed) == 0 {
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

/// Terminal focus notifications (no work unless subscribed).
pub fn focus_changed(focused: bool) {
    queue(if focused {
        Event::FocusGained
    } else {
        Event::FocusLost
    });
}

/// Called by the editor's existing idle timer. Dispatch immediately: adding
/// another debounce timer here would wake the editor again while idle.
pub fn editor_idle(editor: &mut Editor) {
    if wants(EventKind::EditorIdle) {
        deliver_event(editor, Event::EditorIdle);
    }
}

fn refresh_wanted() {
    let (mask, disabled) = with_host(|host| {
        let mask = EVENT_KINDS
            .into_iter()
            .filter(|kind| host.wants(*kind))
            .map(event_bit)
            .fold(0, |mask, bit| mask | bit);
        let disabled: Vec<_> = host
            .plugins()
            .iter()
            .filter(|plugin| plugin.disabled().is_some())
            .map(|plugin| plugin.name().to_owned())
            .collect();
        (mask, disabled)
    })
    .unwrap_or_default();
    WANTED.store(mask, Ordering::Relaxed);
    for plugin in disabled {
        process::kill_plugin(&plugin);
    }
}

/// A document was written to disk.
pub fn document_saved(doc: DocumentId) {
    queue(Event::DocumentSave(plugin_doc_id(doc)));
}

fn queue(event: Event) {
    if WANTED.load(Ordering::Relaxed) & event_bit(event.kind()) == 0 {
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
        deliver_event(editor, event);
    }
}

fn deliver_event(editor: &mut Editor, event: Event) {
    if let Event::DocumentChange(doc)
    | Event::DocumentOpen(doc)
    | Event::DocumentSave(doc)
    | Event::SelectionChange(doc) = &event
    {
        if editor_doc_id(editor, *doc).is_none() {
            return;
        }
    }
    let Some(plugins) = with_host(|host| host.subscribers(event.kind())) else {
        return;
    };
    for plugin in plugins {
        let snapshot = snapshot(editor);
        let result = with_host(|host| host.dispatch_to(&plugin, event.clone(), &snapshot));
        match result.flatten() {
            Some(Ok(actions)) => apply(editor, None, actions),
            Some(Err(error)) => {
                refresh_wanted();
                editor.set_error(error.to_string());
            }
            None => {}
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
    WANTED.load(Ordering::Relaxed) & event_bit(kind) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_grants_require_a_project_outside_home_config_and_data() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let home = root.path().join("home");
        let config = home.join(".config/helix");
        let data = home.join(".local/share/helix");
        let check = |cwd: &Path| file_workspace_in(cwd, Some(&home), &config, &data);
        assert_eq!(
            check(&project),
            None,
            "unmarked directories are not projects"
        );
        std::fs::create_dir(project.join(".git")).unwrap();
        assert_eq!(check(&project), Some(project.clone()));
        std::fs::create_dir_all(home.join(".git")).unwrap();
        assert_eq!(check(&home), None, "even a HOME with a marker is refused");
        assert_eq!(check(Path::new("/")), None);
        assert_eq!(
            file_workspace_in(&project, Some(&home), &project.join("config"), &data),
            None
        );
        assert_eq!(
            file_workspace_in(&project, Some(&home), &config, &project.join("data")),
            None
        );
    }

    #[test]
    fn boolean_false_disables_a_plugin_without_disabling_others() {
        let settings = toml::from_str("one = false\n[two]\nsetting = 42").unwrap();
        assert!(plugin_settings(&settings, "one").is_none());
        assert_eq!(
            plugin_settings(&settings, "two").unwrap().unwrap().config,
            r#"{"setting":42}"#
        );
    }

    // The integration feature gives Application a terminal test backend.
    #[cfg(feature = "integration")]
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_background_load_cannot_replace_a_reload() {
        let mut app = crate::application::Application::new(
            crate::args::Args::default(),
            crate::config::Config::default(),
            helix_core::syntax::Loader::default(),
            helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        )
        .unwrap();
        let old = GENERATION.load(Ordering::Relaxed);
        let empty = tempfile::tempdir().unwrap();
        let newer_settings = toml::from_str("[newer]\nallow-process = [\"rg\"]\n").unwrap();
        load_from(&mut app.editor, empty.path(), &newer_settings);
        install(
            &mut app.editor,
            Some(PluginHost::new(None).unwrap()),
            vec![],
            vec![],
        );
        app.editor.set_status("newer load");
        install_background(
            &mut app.editor,
            old,
            None,
            vec![Action::Status("stale init".into())],
            vec![anyhow::anyhow!("stale error")],
            (empty.path().join("stale"), toml::Table::new()),
        );
        assert_eq!(
            *LOADED.lock(),
            Some((empty.path().to_path_buf(), newer_settings)),
            "stale settings must not replace the reload's grants or directory"
        );
        assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "newer load");
        assert!(
            with_host(|_| ()).is_some(),
            "stale load must not replace the current host"
        );
        install_background(
            &mut app.editor,
            GENERATION.load(Ordering::Relaxed),
            None,
            vec![Action::Status("current init".into())],
            vec![],
            (empty.path().join("current"), toml::Table::new()),
        );
        assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "current init");
        assert_eq!(
            *LOADED.lock(),
            Some((empty.path().join("current"), toml::Table::new()))
        );
    }
}
